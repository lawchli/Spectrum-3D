#!/usr/bin/env node
// Portable little-endian binary format, no JSON parser dependency in Rust/Kotlin.
import { mkdir, writeFile } from 'node:fs/promises';
import { AudioEngine } from '../web/src/audio.js';
import { dbToByte } from '../web/src/bands.js';
const dir = new URL('../tests/fixtures/', import.meta.url);
await mkdir(dir, { recursive: true });
let seed = 0x1839;
const random = () => { seed ^= seed << 13; seed ^= seed >>> 17; seed ^= seed << 5; return (seed >>> 0) / 0x100000000 * 2 - 1; };
const padIndex=process.argv.indexOf('--pad');
const kinds=['tone', 'sweep', 'noise', 'chord']; if(padIndex>=0)kinds.push('pad');
for (const kind of kinds) {
  const sr = 48000, duration = 1, samples = new Float32Array(sr * duration);
  let phase = 0;
  for (let i = 0; i < samples.length; i++) {
    const t = i / sr;
    const hz = kind === 'sweep' ? 25 * 700 ** t : 440 * 2 ** (25 / 1200);
    phase += 2 * Math.PI * hz / sr;
    samples[i] = kind === 'noise' ? random() * .2 : kind === 'chord'
      ? [220, 329.6275569, 391.995436, 493.8833013, 523.2511306].reduce((v, f) => v + .07 * Math.sin(2 * Math.PI * f * t), 0)
      : .5 * Math.sin(phase);
  }
  if (kind === 'pad') {
    const {readFile}=await import('node:fs/promises'); const data=await readFile(process.argv[padIndex+1]);
    if(data.length!==samples.byteLength)throw Error('Pad must be exactly one second of 48 kHz float32 PCM');
    samples.set(new Float32Array(data.buffer.slice(data.byteOffset,data.byteOffset+data.byteLength)));
  }
  const cases = [];
  for (const rate of [60, 120, 240, 480]) for (const sub of [1, 2, 4, 8]) {
    const a = new AudioEngine(); a.ctx = { state: 'running', sampleRate: sr }; a.rowRate = rate; a.sub = sub; a.buildAnalysers();
    const rows = [], hop = sr / rate;
    for (let end = hop; end <= samples.length; end += hop) {
      a.receive(samples.subarray(a.written, Math.round(end)));
      a.analyse(Math.round(end));
      rows.push(Uint8Array.from(a.bands, dbToByte));
    }
    const header = Buffer.alloc(16); [rate, sub, rows.length, rows[0].length].forEach((v, i) => header.writeUInt32LE(v, i * 4));
    cases.push(header, ...rows);
  }
  const header = Buffer.alloc(16); header.write('SVF1'); header.writeUInt32LE(sr, 4); header.writeUInt32LE(samples.length, 8); header.writeUInt32LE(16, 12);
  await writeFile(new URL(`${kind}.svf`, dir), Buffer.concat([header, Buffer.from(samples.buffer), ...cases]));
}
console.log(`Wrote ${kinds.length*16} reference scenarios: ${kinds.length} signals × 4 row rates × 4 subdivisions`);
