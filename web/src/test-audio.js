// The two test tracks, so the analyser can be judged without a microphone. Both are
// sequenced here from plain Web Audio nodes (no samples, no libraries) and are the same
// every time: noise and "random" notes come from a seeded generator.
//
//   1  "Pulse", 124 BPM, F minor. Four-on-the-floor kick with a pumping side-chain, a rolling
//      bass, a resonant acid line whose filter peak wanders up and down the spectrum, delayed
//      chord stabs, metallic hats, noise risers. Shows transients, time resolution, and a
//      narrow peak moving through the bands.
//   2  "Drift", 85 BPM half-time, D minor. A held supersaw pad, a gliding sub with a detuned
//      reese layer, FM bells with inharmonic partials, a lead with portamento and vibrato,
//      hat rolls, a five-octave riser. Shows frequency resolution, harmonic series, pitch
//      glides, long reverb tails and quiet detail high up.
//
// Each track is 32 bars and loops. A track is built once (buses, effects, the voices that
// never stop) and then asked for one sixteenth-note step at a time, a little ahead of the
// audio clock; in an OfflineAudioContext every step is scheduled up front, which is how
// renderTestAudio() measures them.

const midiHz = (m) => 440 * 2 ** ((m - 69) / 12);
const STEPS = 16;   // sixteenths in a bar
const BARS = 32;

// mulberry32
function seeded(seed) {
  return () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function noiseBuffer(ctx, seconds, seed) {
  const buf = ctx.createBuffer(1, Math.round(ctx.sampleRate * seconds), ctx.sampleRate);
  const d = buf.getChannelData(0);
  const rand = seeded(seed);
  for (let i = 0; i < d.length; i++) d[i] = rand() * 2 - 1;
  return buf;
}

// Stereo impulse response: decaying noise that gets darker as it decays, after a short gap.
function reverb(ctx, seconds, seed) {
  const sr = ctx.sampleRate;
  const buf = ctx.createBuffer(2, Math.round(sr * seconds), sr);
  const gap = Math.round(sr * 0.012);
  for (let ch = 0; ch < 2; ch++) {
    const d = buf.getChannelData(ch);
    const rand = seeded(seed + ch * 7919);
    let lp = 0;
    for (let i = gap; i < d.length; i++) {
      const x = (i - gap) / (d.length - gap);
      const keep = 0.25 + 0.6 * x;  // one-pole low-pass, closing with time
      lp = lp * keep + (rand() * 2 - 1) * (1 - keep);
      d[i] = lp * Math.exp(-6.2 * x);
    }
  }
  const node = ctx.createConvolver();
  node.buffer = buf;
  return node;
}

// tanh soft clipper. drive 1 is nearly clean below half scale and rounds the peaks.
function softClip(ctx, drive) {
  const curve = new Float32Array(2049);
  const norm = Math.tanh(drive);
  for (let i = 0; i < curve.length; i++) curve[i] = Math.tanh(drive * (i / 1024 - 1)) / norm;
  const node = ctx.createWaveShaper();
  node.curve = curve;
  node.oversample = '2x';
  return node;
}

function gainNode(ctx, value) {
  const g = ctx.createGain();
  g.gain.value = value;
  return g;
}

// A gain node feeding each [destination, amount].
function bus(ctx, level, ...outs) {
  const g = gainNode(ctx, level);
  for (const [node, amount = 1] of outs) {
    if (amount === 1) g.connect(node);
    else g.connect(gainNode(ctx, amount)).connect(node);
  }
  return g;
}

// Attack, hold, then an exponential fall that is about 35 dB down after `decay` seconds.
function shape(param, t, peak, attack, decay, hold = 0) {
  param.setValueAtTime(0, t);
  param.linearRampToValueAtTime(peak, t + attack);
  param.setTargetAtTime(0, t + attack + hold, decay / 4);
  return t + attack + hold + decay * 1.6;  // when the voice may stop
}

// The mix bus every track ends in, with a reverb and a filtered feedback delay to send to.
function mixer(ctx, out, { level, drive, reverbSeconds, delaySeconds, feedback, seed }) {
  const master = softClip(ctx, drive);
  const outGain = gainNode(ctx, 0);
  outGain.gain.setTargetAtTime(level, ctx.currentTime, 0.03);
  master.connect(outGain).connect(out);

  const rev = reverb(ctx, reverbSeconds, seed);
  rev.connect(gainNode(ctx, 0.9)).connect(master);

  const delay = ctx.createDelay(2);
  delay.delayTime.value = delaySeconds;
  const tone = ctx.createBiquadFilter();
  tone.type = 'lowpass';
  tone.frequency.value = 3600;
  const loop = gainNode(ctx, feedback);
  delay.connect(tone).connect(loop).connect(delay);
  tone.connect(master);
  tone.connect(gainNode(ctx, 0.25)).connect(rev);

  // Everything but the drums goes through `duck`, which each kick pulls down for a moment.
  const duck = gainNode(ctx, 1);
  duck.connect(master);
  const pump = (t, depth, release) => {
    duck.gain.setTargetAtTime(depth, t - 0.004, 0.003);
    duck.gain.setTargetAtTime(1, t + 0.045, release);
  };
  return { master, rev, delay, duck, pump, close: () => outGain.disconnect() };
}

// ---- Voices that start and stop per note ----

function kick(ctx, dest, t, { from = 165, to = 46, sweep = 0.075, decay = 0.36, gain = 0.72 }) {
  const o = ctx.createOscillator(), g = ctx.createGain();
  o.frequency.setValueAtTime(from, t);
  o.frequency.exponentialRampToValueAtTime(to, t + sweep);
  o.connect(g).connect(dest);
  o.start(t);
  o.stop(shape(g.gain, t, gain, 0.002, decay, 0.035));
}

function noiseHit(ctx, dest, buffer, t, { type = 'highpass', freq = 8000, sweepTo = 0, q = 0.7, attack = 0.001, decay = 0.05, hold = 0, gain = 0.1, offset = 0 }) {
  const src = ctx.createBufferSource(), f = ctx.createBiquadFilter(), g = ctx.createGain();
  src.buffer = buffer;
  src.loop = true;
  f.type = type;
  f.Q.value = q;
  f.frequency.setValueAtTime(freq, t);
  if (sweepTo) f.frequency.exponentialRampToValueAtTime(sweepTo, t + attack + hold);
  src.connect(f).connect(g).connect(dest);
  src.start(t, offset % buffer.duration);
  src.stop(shape(g.gain, t, gain, attack, decay, hold));
}

// Detuned oscillators through a low-pass whose cutoff falls from `bright` to `dark`.
function pluck(ctx, dest, t, notes, { type = 'sawtooth', detune = [-8, 8], bright = 3500, dark = 500, q = 1, sweep = 0.15, attack = 0.003, decay = 0.2, hold = 0, gain = 0.03 }) {
  const f = ctx.createBiquadFilter(), g = ctx.createGain();
  f.type = 'lowpass';
  f.Q.value = q;
  f.frequency.setValueAtTime(bright, t);
  f.frequency.setTargetAtTime(dark, t + attack, sweep / 3);
  f.connect(g).connect(dest);
  const stop = shape(g.gain, t, gain, attack, decay, hold);
  for (const note of notes) for (const cents of detune) {
    const o = ctx.createOscillator();
    o.type = type;
    o.frequency.value = midiHz(note);
    o.detune.value = cents;
    o.connect(f);
    o.start(t);
    o.stop(stop);
  }
}

// Two-operator FM: a modulator at a non-integer ratio gives the bell its inharmonic
// partials, and they die away faster than the fundamental as the index falls.
function bell(ctx, dest, t, note, { ratio = 3.5, index = 2.4, decay = 1.8, gain = 0.1 }) {
  const hz = midiHz(note);
  const car = ctx.createOscillator(), mod = ctx.createOscillator(), depth = ctx.createGain(), g = ctx.createGain();
  car.frequency.value = hz;
  mod.frequency.value = hz * ratio;
  depth.gain.setValueAtTime(index * hz, t);
  depth.gain.setTargetAtTime(0.12 * index * hz, t, 0.16);
  mod.connect(depth).connect(car.frequency);
  car.connect(g).connect(dest);
  const stop = shape(g.gain, t, gain, 0.003, decay);
  car.start(t); mod.start(t);
  car.stop(stop); mod.stop(stop);
}

// ---- Track 1: Pulse ----

function pulse(ctx, out) {
  const step = 60 / 124 / 4;
  const mix = mixer(ctx, out, { level: 0.72, drive: 1.15, reverbSeconds: 2.2, delaySeconds: step * 3, feedback: 0.42, seed: 11 });
  const { master, rev, delay, duck } = mix;
  const noise = noiseBuffer(ctx, 2, 101);
  const live = [];  // sources that run until stop()

  const drums = bus(ctx, 1, [master]);
  const clapBus = bus(ctx, 1, [master], [rev, 0.28]);
  const fxBus = bus(ctx, 1, [master], [rev, 0.35]);
  const bassBus = bus(ctx, 1, [duck]);
  const stabBus = bus(ctx, 1, [duck], [delay, 0.34], [rev, 0.22]);
  const arpBus = bus(ctx, 1, [duck, 0.8], [delay, 0.42], [rev, 0.3]);

  // Fm9, Dbmaj7, Ab/Eb, Eb add9: two bars each. Bass roots F1, Db2, Ab1, Eb2.
  const chords = [[53, 56, 60, 63], [49, 53, 56, 60], [51, 56, 60, 63], [51, 55, 58, 65]];
  const roots = [29, 37, 32, 39];

  // Hats: six square waves at unrelated pitches, high-passed, so they show as a comb of
  // thin lines above 7 kHz instead of a noise smear. One envelope: a closed hat chokes an open one.
  const hatAmp = gainNode(ctx, 0);
  {
    const band = ctx.createBiquadFilter(), high = ctx.createBiquadFilter();
    band.type = 'bandpass'; band.frequency.value = 10500; band.Q.value = 0.8;
    high.type = 'highpass'; high.frequency.value = 7200;
    band.connect(high).connect(hatAmp).connect(drums);
    for (const hz of [411, 609, 739, 1045, 1080, 1600]) {
      const o = ctx.createOscillator();
      o.type = 'square';
      o.frequency.value = hz;
      o.connect(band);
      o.start();
      live.push(o);
    }
  }
  const hat = (t, level, decay) => {
    hatAmp.gain.setTargetAtTime(level, t, 0.0008);
    hatAmp.gain.setTargetAtTime(0, t + 0.004, decay / 4);
  };

  // Acid line: one sawtooth that never stops, so notes can slide into each other, through a
  // 24 dB/octave resonant low-pass. One control signal moves both filter stages.
  const acid = ctx.createOscillator();
  acid.type = 'sawtooth';
  acid.frequency.value = midiHz(41);
  const cutoff = ctx.createConstantSource();
  cutoff.offset.value = 200;
  const acidAmp = gainNode(ctx, 0);
  {
    const f1 = ctx.createBiquadFilter(), f2 = ctx.createBiquadFilter();
    f1.type = f2.type = 'lowpass';
    f1.frequency.value = f2.frequency.value = 0;
    f1.Q.value = 15;
    f2.Q.value = 1;
    cutoff.connect(f1.frequency);
    cutoff.connect(f2.frequency);
    const post = bus(ctx, 0.62, [duck], [delay, 0.22], [rev, 0.12]);
    acid.connect(f1).connect(f2).connect(softClip(ctx, 2.2)).connect(acidAmp).connect(post);
    acid.start(); cutoff.start();
    live.push(acid, cutoff);
  }
  //                 F2      F2  F3      Ab2 F2      C3  F2      Eb3 F3      Bb2 C3
  const acidNotes = [41, 0, 41, 53, 0, 44, 41, 0, 48, 41, 0, 51, 53, 0, 46, 48];
  const acidAccent = [1, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1];
  const acidSlide = [0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 1];  // glide into this step
  const acidRun = [53, 56, 58, 60];  // the last four steps of every fourth bar, slurred
  const acidStep = (bar, s) => (bar % 4 === 3 && s >= 12
    ? { note: acidRun[s - 12], slide: s > 12, accent: s === 15 }
    : { note: acidNotes[s], slide: acidSlide[s], accent: acidAccent[s] });

  // How far the acid filter is open, 0..1, by position in the loop.
  const acidOpen = (bar, s) => {
    const x = bar + s / STEPS;
    if (x < 8) return 0.12 + 0.38 * (x / 8);
    if (x < 16) return 0.5 + 0.5 * ((x - 8) / 8);
    if (x < 24) return 0.72 + 0.16 * Math.sin((x - 16) * Math.PI / 2);
    if (x < 28) return 0.05 + 0.85 * ((x - 24) / 4) ** 1.6;
    return 1;
  };

  const riser = (t, seconds) => {
    noiseHit(ctx, fxBus, noise, t, { type: 'bandpass', freq: 260, sweepTo: 9500, q: 3.5, attack: seconds * 0.98, hold: seconds * 0.02, decay: 0.08, gain: 0.3 });
  };
  const crash = (t) => noiseHit(ctx, fxBus, noise, t, { freq: 5200, decay: 1.9, gain: 0.2, offset: 0.7 });

  const stabSteps = new Set([0, 3, 6, 10, 13]);
  const arpOrder = [0, 1, 2, 3, 2, 1, 3, 0, 1, 2, 3, 2, 0, 3, 1, 2];

  function schedule(n, t) {
    const bar = Math.floor(n / STEPS), s = n % STEPS;
    const chord = Math.floor(bar / 2) % 4;
    const breakdown = bar >= 24 && bar < 28;
    const gap = bar === 15 && s >= 8;  // half a bar without the kick before bar 16
    const full = bar >= 16 && !breakdown;

    if (s === 0 && (bar === 0 || bar === 16 || bar === 28)) crash(t);
    if (s === 0 && (bar === 14 || bar === 26)) riser(t, step * STEPS * 2);
    if (s === 0 && bar === 28) kick(ctx, fxBus, t, { from: 130, to: 33, sweep: 0.7, decay: 1.3, gain: 0.4 });

    if (!breakdown && !gap) {
      if (s % 4 === 0) {
        kick(ctx, drums, t, {});
        noiseHit(ctx, drums, noise, t, { freq: 2600, decay: 0.012, gain: 0.16, offset: 0.3 });
        mix.pump(t, 0.22, 0.085);
      } else {
        // Rolling bass on the three sixteenths between kicks: a sine for the sub and a
        // filtered saw an octave up, which jumps another octave twice a bar.
        const root = roots[chord];
        const level = [0, 0.75, 1, 0.85][s % 4];
        const up = s === 6 || s === 14 ? 12 : 0;
        pluck(ctx, bassBus, t, [root + up], { type: 'sine', detune: [0], bright: 900, dark: 900, decay: step * 1.1, gain: 0.27 * level });
        pluck(ctx, bassBus, t, [root + 12 + up], { detune: [-6, 6], bright: 950, dark: 130, q: 4, sweep: 0.1, decay: step * 1.0, gain: 0.15 * level });
      }
    }

    // Hats: off-beat from the start, sixteenths from bar 4, an open one before each bar line.
    if (!breakdown) {
      if (s % 4 === 2) hat(t, 0.42, s === 14 && bar >= 8 ? 0.3 : 0.05);
      else if (bar >= 4 && s % 2 === 1) hat(t, 0.16, 0.03);
      else if (full && s % 4 === 0) hat(t, 0.1, 0.03);
    }

    // Clap on two and four: three quick bursts and a tail.
    if (bar >= 4 && !breakdown && !gap && (s === 4 || s === 12)) {
      for (const [dt, g] of [[0, 0.5], [0.011, 0.6], [0.023, 1]]) {
        noiseHit(ctx, clapBus, noise, t + dt, { type: 'bandpass', freq: 1350, q: 1.1, decay: g === 1 ? 0.16 : 0.012, gain: 0.34 * g, offset: 1.1 + dt * 9 });
      }
    }
    // Snare roll into the drops, getting louder and brighter.
    const roll = bar === 15 && s >= 8 ? (s - 8) / 8 : bar === 27 ? s / 16 : -1;
    if (roll >= 0) {
      const hits = roll > 0.74 ? 2 : 1;
      for (let i = 0; i < hits; i++) {
        noiseHit(ctx, clapBus, noise, t + (i * step) / 2, { type: 'bandpass', freq: 1700 + 2200 * roll, q: 0.9, decay: 0.07, gain: 0.1 + 0.22 * roll, offset: 0.2 + s * 0.05 });
      }
    }

    // Acid. A note that the next one slides into is held instead of gated off.
    {
      const { note, slide, accent } = acidStep(bar, s);
      const nextSlides = acidStep(bar + (s === 15 ? 1 : 0), (s + 1) % STEPS).slide;
      const open = acidOpen(bar, s);
      const base = 170 * 2 ** (open * 3.1);
      if (note) {
        const hz = midiHz(note);
        if (slide) acid.frequency.setTargetAtTime(hz, t, 0.028);
        else acid.frequency.setValueAtTime(hz, t);
        cutoff.offset.setTargetAtTime(base * (1 + (accent ? 5.5 : 3) * (0.35 + 0.65 * open)), t, 0.003);
        cutoff.offset.setTargetAtTime(base, t + 0.012, accent ? 0.05 : 0.085);
        acidAmp.gain.setTargetAtTime((accent ? 0.3 : 0.2) * (0.55 + 0.45 * open), t, 0.004);
        if (!nextSlides) acidAmp.gain.setTargetAtTime(0, t + step * 0.62, 0.012);
      } else {
        acidAmp.gain.setTargetAtTime(0, t, 0.012);
      }
    }

    // Chords: short delayed stabs, or held and slowly opening through the breakdown.
    if (breakdown) {
      if (s === 0 && bar % 2 === 0) {
        const bright = bar === 24 ? 900 : 2600;
        pluck(ctx, stabBus, t, chords[chord], { detune: [-11, 0, 11], bright: bright * 0.4, dark: bright, sweep: step * STEPS * 3, attack: 0.5, hold: step * STEPS * 2 - 0.9, decay: 1.2, gain: 0.024 });
      }
    } else if (bar >= 8 && stabSteps.has(s)) {
      pluck(ctx, stabBus, t, chords[chord], { bright: 2400 + 1800 * acidOpen(bar, s), dark: 420, q: 2, sweep: 0.16, decay: 0.22, gain: 0.042 });
    }

    // Arpeggio two octaves above the chord.
    if (bar >= 16) {
      const note = chords[chord][arpOrder[s]] + 24 + ((s * 5) % STEPS < 5 ? 12 : 0);
      pluck(ctx, arpBus, t, [note], { type: 'square', detune: [-4, 5], bright: 6500, dark: 1800, sweep: 0.1, decay: 0.15, gain: breakdown ? 0.02 : 0.028 });
    }
  }

  return {
    step,
    schedule,
    stop() {
      for (const node of live) node.stop();
      mix.close();
    },
  };
}

// ---- Track 2: Drift ----

function drift(ctx, out) {
  const step = 60 / 85 / 4;
  const bar = step * STEPS;
  const mix = mixer(ctx, out, { level: 0.72, drive: 1.1, reverbSeconds: 3.4, delaySeconds: step * 3, feedback: 0.46, seed: 23 });
  const { master, rev, delay, duck } = mix;
  const noise = noiseBuffer(ctx, 2, 202);
  const rand = seeded(303);
  const live = [];

  const drums = bus(ctx, 1, [master]);
  const snareBus = bus(ctx, 1, [master], [rev, 0.3]);
  const fxBus = bus(ctx, 1, [master], [rev, 0.4]);
  const bellBus = bus(ctx, 1, [master, 0.7], [delay, 0.36], [rev, 0.5]);
  const airBus = bus(ctx, 1, [master, 0.5], [rev, 0.8]);

  // Dm9, Bbmaj9, Fmaj9, C6/9 with the top three notes barely moving. Roots D2, Bb1, F1, C2.
  const chords = [[53, 57, 60, 64], [53, 57, 60, 62], [52, 55, 57, 60], [52, 55, 57, 62]];
  const roots = [38, 34, 29, 36];

  // Pad: four voices of three detuned saws that never stop and glide from chord to chord,
  // so their harmonics draw unbroken ridges down the waterfall.
  const padFilter = ctx.createBiquadFilter();
  padFilter.type = 'lowpass';
  padFilter.Q.value = 2.5;
  padFilter.frequency.value = 350;
  const padAmp = gainNode(ctx, 0);
  padFilter.connect(padAmp);
  padAmp.connect(duck);
  padAmp.connect(gainNode(ctx, 0.35)).connect(rev);
  const pad = chords[0].map((note) => [-11, 0, 11].map((cents) => {
    const o = ctx.createOscillator();
    o.type = 'sawtooth';
    o.detune.value = cents;
    o.frequency.value = midiHz(note);
    o.connect(padFilter);
    o.start();
    live.push(o);
    return o;
  }));

  // Sub: one sine that glides between roots. Reese: two saws an octave up, 28 cents apart,
  // low-passed; their beating shows as a slow ripple on the low harmonics.
  const sub = ctx.createOscillator();
  sub.frequency.value = midiHz(roots[0]);
  const subAmp = gainNode(ctx, 0);
  sub.connect(subAmp).connect(duck);
  sub.start();
  live.push(sub);
  const reeseFilter = ctx.createBiquadFilter();
  reeseFilter.type = 'lowpass';
  reeseFilter.Q.value = 5;
  reeseFilter.frequency.value = 260;
  const reeseAmp = gainNode(ctx, 0);
  reeseFilter.connect(reeseAmp).connect(duck);
  const reese = [-14, 14].map((cents) => {
    const o = ctx.createOscillator();
    o.type = 'sawtooth';
    o.detune.value = cents;
    o.frequency.value = midiHz(roots[0] + 12);
    o.connect(reeseFilter);
    o.start();
    live.push(o);
    return o;
  });

  // Lead: a saw and a quieter square an octave down, gliding between notes, with a vibrato
  // that grows in on held notes.
  const leadAmp = gainNode(ctx, 0);
  const leadVoices = [['sawtooth', 0, 1], ['square', -12, 0.45]].map(([type, shift, level]) => {
    const osc = ctx.createOscillator();
    osc.type = type;
    osc.frequency.value = midiHz(69 + shift);
    osc.connect(gainNode(ctx, level)).connect(leadAmp);
    osc.start();
    live.push(osc);
    return { osc, shift };
  });
  const vibrato = ctx.createOscillator();
  vibrato.frequency.value = 5.6;
  const vibratoDepth = gainNode(ctx, 0);
  vibrato.connect(vibratoDepth);
  for (const v of leadVoices) vibratoDepth.connect(v.osc.detune);
  vibrato.start();
  live.push(vibrato);
  {
    const f = ctx.createBiquadFilter();
    f.type = 'lowpass';
    f.Q.value = 4;
    f.frequency.value = 3000;
    leadAmp.connect(f);
    f.connect(bus(ctx, 1, [duck, 0.9], [delay, 0.3], [rev, 0.3]));
  }

  // Eight-bar melodies as [bar, step, note] and, for the lead, the length in steps.
  const bells = [
    [0, 0, 81], [0, 6, 77], [0, 10, 76], [1, 4, 74], [1, 12, 81],
    [2, 0, 84], [2, 6, 81], [2, 10, 77], [3, 4, 86], [3, 12, 84],
    [4, 0, 81], [4, 6, 79], [4, 10, 76], [5, 4, 72], [5, 12, 79],
    [6, 0, 76], [6, 6, 79], [6, 10, 86], [7, 4, 81], [7, 8, 79], [7, 12, 76],
  ];
  const lead = [
    [0, 0, 69, 6], [0, 6, 74, 4], [0, 10, 77, 6], [1, 0, 76, 12], [1, 12, 74, 4],
    [2, 0, 77, 6], [2, 6, 74, 4], [2, 10, 72, 6], [3, 0, 74, 12], [3, 12, 77, 4],
    [4, 0, 81, 6], [4, 6, 79, 4], [4, 10, 76, 6], [5, 0, 72, 8], [5, 8, 76, 4], [5, 12, 79, 4],
    [6, 0, 76, 6], [6, 6, 74, 4], [6, 10, 76, 6], [7, 0, 79, 4], [7, 4, 81, 12],
  ];
  const at = (list) => {
    const map = new Map();
    for (const e of list) map.set(e[0] * STEPS + e[1], e);
    return map;
  };
  const bellAt = at(bells), leadAt = at(lead);

  // Quiet pings between 2 and 7 kHz, drawn once from the seeded generator: about a third
  // of the steps get a note from D minor pentatonic.
  const scale = [0, 3, 5, 7, 10];
  const air = Array.from({ length: BARS * STEPS }, () => (rand() < 0.34 ? 96 + 2 + scale[Math.floor(rand() * 5)] + 12 * Math.floor(rand() * 2) : 0));

  // Pad filter cutoff by position in the loop.
  const padOpen = (x) => {
    if (x < 8) return 350 + 1050 * (x / 8);
    if (x < 16) return 1400 + 1000 * ((x - 8) / 8);
    if (x < 24) return 2400 + 1800 * ((x - 16) / 8);
    if (x < 28) return 500 * 2 ** (3.3 * ((x - 24) / 4));
    return 4600;
  };

  function schedule(n, t) {
    const b = Math.floor(n / STEPS), s = n % STEPS;
    const chord = Math.floor(b / 2) % 4;
    const x = b + s / STEPS;
    const breakdown = b >= 24 && b < 28;
    const beat = (b >= 8 && !breakdown);  // the half-time drums play
    const swung = t + (s % 2 ? step * 0.1 : 0);

    // Pad, sub and reese follow the chord; set at every bar line so a skipped step heals.
    if (s === 0) {
      chords[chord].forEach((note, i) => { for (const o of pad[i]) o.frequency.setTargetAtTime(midiHz(note), t, 0.11); });
      sub.frequency.setTargetAtTime(midiHz(roots[chord]), t, 0.05);
      for (const o of reese) o.frequency.setTargetAtTime(midiHz(roots[chord] + 12), t, 0.05);
      padAmp.gain.setTargetAtTime(breakdown ? 0.06 : 0.05, t, 0.4);
      subAmp.gain.setTargetAtTime(b < 2 || breakdown ? 0 : 0.25, t, 0.25);
      reeseAmp.gain.setTargetAtTime(beat ? 0.15 : 0, t, 0.3);
    }
    // Late in the second bar of each chord the sub slides to the next root early.
    if (s === 13 && b % 2 === 1) sub.frequency.setTargetAtTime(midiHz(roots[(chord + 1) % 4]), t, 0.14);
    padFilter.frequency.setTargetAtTime(padOpen(x) * (1 + 0.14 * Math.sin(x * Math.PI / 2)), t, 0.08);
    reeseFilter.frequency.setTargetAtTime(240 + 190 * (0.5 - 0.5 * Math.cos(x * Math.PI)), t, 0.08);

    // Half-time drums: kick on one and the "and" of three, snare on three.
    if (beat) {
      const kicks = b % 2 ? [0, 3, 10] : [0, 10];
      if (kicks.includes(s)) {
        kick(ctx, drums, t, { from: 150, to: 44, sweep: 0.09, decay: 0.42, gain: 0.7 });
        noiseHit(ctx, drums, noise, t, { freq: 2200, decay: 0.012, gain: 0.12, offset: 0.3 });
        mix.pump(t, 0.4, 0.12);
      }
      if (s === 8) {
        noiseHit(ctx, snareBus, noise, t, { type: 'bandpass', freq: 1900, q: 0.8, decay: 0.2, gain: 0.36, offset: 0.9 });
        pluck(ctx, snareBus, t, [54], { type: 'sine', detune: [0], bright: 2000, dark: 2000, decay: 0.09, gain: 0.3 });
      }
      if (s === 15 && b % 2) noiseHit(ctx, snareBus, noise, t, { type: 'bandpass', freq: 2300, q: 1, decay: 0.06, gain: 0.08, offset: 0.5 });
    }
    // Hats: swung sixteenths with rolls of 32nds at the end of some bars.
    if (beat || (b >= 4 && b < 8 && s % 2 === 0) || (b >= 26 && b < 28 && s % 4 === 2)) {
      const fast = beat && ((b % 4 === 3 && s >= 12) || (b % 4 === 1 && (s === 6 || s === 7)));
      const level = [1, 0.35, 0.6, 0.35][s % 4] * (beat ? 1 : 0.5);
      if (fast) {
        for (let i = 0; i < 2; i++) {
          noiseHit(ctx, drums, noise, t + (i * step) / 2, { freq: 9000, decay: 0.022, gain: 0.065 + 0.014 * (s % 4) + 0.012 * i, offset: 0.1 * s + 0.05 * i });
        }
      } else if (s === 14 && beat && b % 2 === 0) {
        noiseHit(ctx, drums, noise, swung, { freq: 7800, decay: 0.24, gain: 0.07, offset: 1.4 });
      } else {
        noiseHit(ctx, drums, noise, swung, { freq: 9000, decay: 0.03, gain: 0.1 * level, offset: 0.1 * s });
      }
    }

    // Bells: the whole phrase, or only its downbeats while the lead has the tune.
    const leadOn = (b >= 16 && b < 24) || b >= 28;
    const phrase = (b % 8) * STEPS + s;
    const bellNote = bellAt.get(phrase);
    if (bellNote && (!leadOn || s === 0)) bell(ctx, bellBus, t, bellNote[2], { gain: leadOn ? 0.08 : 0.12 });

    // Lead. The last four bars play the second half of its phrase.
    if (leadOn) {
      const note = leadAt.get(phrase);
      if (note) {
        for (const v of leadVoices) v.osc.frequency.setTargetAtTime(midiHz(note[2] + v.shift), t, 0.04);
        leadAmp.gain.setTargetAtTime(0.1, t, 0.02);
        vibratoDepth.gain.setTargetAtTime(0, t, 0.02);
        if (note[3] >= 6) vibratoDepth.gain.setTargetAtTime(note[3] >= 12 ? 34 : 20, t + step * 2.5, step * 2);
      }
      if (b % 8 === 7 && s === 15) leadAmp.gain.setTargetAtTime(0, t + step * 0.5, 0.12);
    } else if (s === 0) {
      leadAmp.gain.setTargetAtTime(0, t, 0.1);
    }

    // Air.
    const ping = air[n % air.length];
    if (ping) pluck(ctx, airBus, t, [ping], { type: 'sine', detune: [0], bright: 12000, dark: 12000, decay: 0.3, gain: breakdown || b < 8 ? 0.024 : 0.014 });

    // Two bars of riser through five octaves into bar 28, then a falling sweep and a crash.
    if (b === 26 && s === 0) {
      const o = ctx.createOscillator(), g = ctx.createGain();
      o.type = 'sawtooth';
      o.frequency.setValueAtTime(midiHz(50), t);
      o.frequency.exponentialRampToValueAtTime(midiHz(110), t + bar * 2);
      g.gain.setValueAtTime(0, t);
      g.gain.linearRampToValueAtTime(0.05, t + bar * 2);
      g.gain.setTargetAtTime(0, t + bar * 2, 0.02);
      o.connect(g).connect(fxBus);
      o.start(t);
      o.stop(t + bar * 2 + 0.2);
      noiseHit(ctx, fxBus, noise, t, { type: 'bandpass', freq: 300, sweepTo: 8000, q: 3, attack: bar * 2 - 0.05, hold: 0.05, decay: 0.08, gain: 0.16 });
    }
    if (s === 0 && (b === 8 || b === 16 || b === 28)) {
      noiseHit(ctx, fxBus, noise, t, { freq: 5000, decay: 2.4, gain: 0.14, offset: 0.7 });
      if (b === 28) kick(ctx, fxBus, t, { from: 2600, to: 40, sweep: 0.9, decay: 1.4, gain: 0.2 });
    }
  }

  return {
    step,
    schedule,
    stop() {
      for (const node of live) node.stop();
      mix.close();
    },
  };
}

const TRACKS = [pulse, drift];
export const TEST_TRACKS = TRACKS.length;

// Starts test track `which` (1 or 2) into `out` and returns a function that stops it.
export function startTestAudio(ctx, out, which) {
  const track = TRACKS[which - 1](ctx, out);
  const loop = BARS * STEPS;
  let n = 0;
  let next = ctx.currentTime + 0.1;
  const pump = () => {
    // A hidden tab's timers run about once a second, so it schedules further ahead.
    const ahead = globalThis.document?.hidden ? 1.6 : 0.35;
    if (next < ctx.currentTime) {  // stalled past the lookahead: drop the missed steps
      const missed = Math.ceil((ctx.currentTime - next) / track.step);
      n += missed;
      next += missed * track.step;
    }
    while (next < ctx.currentTime + ahead) {
      track.schedule(n % loop, next);
      n++;
      next += track.step;
    }
  };
  pump();
  const timer = setInterval(pump, 25);
  return () => { clearInterval(timer); track.stop(); };
}

// Renders `seconds` of a track from bar `fromBar` without playing it: mono samples at
// `sampleRate`. For measuring levels and for fixtures.
export async function renderTestAudio(which, seconds, { sampleRate = 48000, fromBar = 0 } = {}) {
  const ctx = new OfflineAudioContext(2, Math.round(seconds * sampleRate), sampleRate);
  const track = TRACKS[which - 1](ctx, ctx.destination);
  const first = fromBar * STEPS;
  for (let n = 0; n * track.step < seconds; n++) track.schedule((first + n) % (BARS * STEPS), 0.05 + n * track.step);
  const buffer = await ctx.startRendering();
  const left = buffer.getChannelData(0), right = buffer.getChannelData(1);
  const mono = new Float32Array(left.length);
  for (let i = 0; i < mono.length; i++) mono[i] = (left[i] + right[i]) / 2;
  return mono;
}
