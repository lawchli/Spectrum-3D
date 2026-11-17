import test from 'node:test';
import assert from 'node:assert/strict';
import { RealFFT } from '../web/src/fft.js';
import { bandHz, hzToBand, dbToByte, buildBandMap, applyBandMap } from '../web/src/bands.js';
import { AudioEngine } from '../web/src/audio.js';
import { History, LEVELS } from '../web/src/history.js';

test('FFT agrees with independent direct DFT of a Blackman-windowed signal', () => {
  const n = 64, ring = Float32Array.from({ length: 128 }, (_, i) => Math.sin(i * .23) * .3 + Math.cos(i * 1.3) * .2);
  const out = new Float64Array(n / 2);
  new RealFFT(n).run(ring, 103, out);
  for (let k = 0; k < n / 2; k++) {
    let re = 0, im = 0;
    for (let i = 0; i < n; i++) {
      const w = .42 - .5 * Math.cos(2 * Math.PI * i / n) + .08 * Math.cos(4 * Math.PI * i / n);
      const value = ring[(103 - n + i) & 127] * w;
      re += value * Math.cos(2 * Math.PI * k * i / n);
      im -= value * Math.sin(2 * Math.PI * k * i / n);
    }
    const expected = 10 * Math.log10((re * re + im * im) * (2 / (n * .42)) ** 2 + 1e-30);
    assert.ok(Math.abs(expected - out[k]) < 1e-7, `${k}: ${out[k]} / ${expected}`);
  }
});
test('frequency map roundtrips, quantization boundaries and Nyquist silence', () => {
  for (let k = 0; k <= 126; k += .125) assert.ok(Math.abs(hzToBand(bandHz(k)) - k) < 1e-12);
  assert.deepEqual([-Infinity, -120, -119.75, -96, 0, 7.5, Infinity].map(dbToByte), [0, 0, 1, 48, 240, 255, 255]);
  const { map } = buildBandMap(32000, 1024, 8), out = new Float32Array(map.length);
  applyBandMap(map, new Float32Array(512).fill(-24), out);
  for (let i = 0; i < out.length; i++) assert.equal(out[i], map[i] ? -24 : -120);
});
test('pyramid uses power means, preserves impulses, and uploads wrapped rings exactly', () => {
  const h = new History(3, 8);
  const uploaded = Array.from({ length: LEVELS }, () => new Uint8Array(24));
  const source = [];
  const flush = () => h.flush((l, row, height, bytes) => {
    assert.equal(bytes.length, height * 3);
    uploaded[l].set(bytes, row * 3);
  });
  for (let n = 0; n < 160; n++) {
    const row = Uint8Array.from([n % 256, n % 2 ? 0 : 240, 110]);
    source.push(row); h.push(row);
    if (n % 21 === 20) flush();
    for (let l = 1; l < LEVELS; l++) {
      const span = 2 ** l;
      if ((n + 1) % span) continue;
      for (let k = 0; k < 3; k++) {
        const mean = source.slice(-span).reduce((v, r) => v + 10 ** ((r[k] / 2 - 120) / 10), 0) / span;
        const expected = dbToByte(10 * Math.log10(mean + 1e-30));
        assert.equal(h.history[l][(Math.floor(n / span) % 8) * 3 + k], expected);
      }
    }
  }
  flush();
  assert.deepEqual(uploaded, h.history);
  let calls = 0; h.flush(() => calls++); assert.equal(calls, 0);
});
test('audio row clock follows audio, bounds catch-up, and drops frozen input', () => {
  for (const rate of [60, 120, 240, 480]) {
    const a = new AudioEngine(); a.ctx = { state: 'running', sampleRate: 48000 }; a.rowRate = rate;
    a.analyse = () => {};
    let rows = 0;
    for (let frame = 1; frame <= 600; frame++) { a.written = frame * 800; a.pull(1 / 60, () => rows++); }
    assert.ok(Math.abs(rows - rate * 10) <= 1, `${rate}: ${rows}`);
    a.written += 48000; let catchup = 0; a.pull(.1, () => catchup++);
    assert.ok(catchup <= Math.max(8, Math.ceil(rate * .1)));
    a.written += 48000; a.pull(.1, () => assert.fail('frozen emitted a row'), true);
  }
});

test('adaptive resolution keeps reductions that help, undoes the rest, backs off failed increases', async () => {
  const { RenderScale } = await import('../web/src/render-scale.js');
  // GPU time grows with the pixel count; a frame lasts the longer of GPU and CPU time, at least 1/60 s.
  const run = (r, seconds, { gpu, cpu = 0, timer = true }) => {
    let t = 0, changes = 0, low = 0;
    while (t < seconds) {
      const g = gpu * r.scale ** 2, dt = Math.max(1 / 60, g / 1000, cpu / 1000);
      if (r.update(dt, timer ? g : null)) changes++;
      if (r.scale < 1) low += dt;
      t += dt;
    }
    return { changes, low };
  };
  // GPU-bound with a timer: 24 ms at full size, 85 % (17 ms) is enough and is kept.
  let r = new RenderScale('auto'); run(r, 60, { gpu: 24 }); assert.equal(r.scale, .85);
  // Heavier: 85 % is still slow, 70 % is not. When the load goes away it climbs back to full size.
  r = new RenderScale('auto'); run(r, 60, { gpu: 34 }); assert.equal(r.scale, .7);
  run(r, 120, { gpu: 4 }); assert.equal(r.scale, 1);
  // GPU-bound without a timer: every try at full size fails, and the tries become rare.
  // (The previous rule swung 1 <-> 0.85 every ~35 s: 35 changes in ten minutes.)
  r = new RenderScale('auto'); const blind = run(r, 600, { gpu: 22, timer: false });
  assert.equal(r.scale, .85); assert.ok(blind.changes <= 12, `${blind.changes} changes in ten minutes`);
  // CPU-bound with a timer: the GPU is mostly idle, fewer pixels would not help, nothing changes.
  r = new RenderScale('auto'); assert.equal(run(r, 60, { gpu: 4, cpu: 25 }).changes, 0); assert.equal(r.scale, 1);
  // CPU-bound without a timer: a reduction that did not shorten frames is undone and not retried soon.
  r = new RenderScale('auto'); const cpu = run(r, 120, { gpu: 4, cpu: 25, timer: false });
  assert.equal(r.scale, 1); assert.ok(cpu.low <= 11, `${cpu.low.toFixed(1)} s at reduced size`);
  // Stalls and hidden tabs count for nothing; fixed modes never change.
  r = new RenderScale('auto');
  for (let i = 0; i < 50; i++) r.update(.5, 40);
  for (let i = 0; i < 500; i++) r.update(1 / 30, 30, false);
  assert.equal(r.scale, 1);
  r.setMode('0.7'); run(r, 60, { gpu: 2 }); assert.equal(r.scale, .7);
});

test('front and piano dirty checks retain buffer layout and upload only changes', async () => {
  const { Renderer } = await import('../web/src/renderer.js');
  let uploads=0; const gl={bindBuffer(){},bufferSubData(){uploads++},ARRAY_BUFFER:1};
  const renderer={gl,count:3,keys:{data:new Float32Array(127*6*4),vbo:{}}};
  const b={data:new Float32Array(18),perBand:2,vbo:{}};
  Renderer.prototype.updateFront.call(renderer,b,Float32Array.of(1,2,3)); assert.equal(uploads,1);
  Renderer.prototype.updateFront.call(renderer,b,Float32Array.of(1,2,3)); assert.equal(uploads,1);
  Renderer.prototype.updateFront.call(renderer,b,Float32Array.of(1,2,4)); assert.equal(uploads,2);
  assert.equal(b.data[13],4);assert.equal(b.data[16],4);
  const lit=new Float32Array(127);
  Renderer.prototype.updateKeys.call(renderer,lit);assert.equal(uploads,2);
  lit[126]=.5;Renderer.prototype.updateKeys.call(renderer,lit);assert.equal(uploads,3);
  Renderer.prototype.updateKeys.call(renderer,lit);assert.equal(uploads,3);
});
