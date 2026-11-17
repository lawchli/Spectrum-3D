#!/usr/bin/env node
// Renders the two web test tracks (web/src/test-audio.js) to mono 48 kHz WAV files for the
// native app, which has no Web Audio to synthesise them with. The tracks stay defined in one
// place: this runs the web module itself in a headless Chrome OfflineAudioContext.
//
//   node scripts/render-test-audio.mjs [--mp3] [--output DIR]  default toolchains/test-audio/
// --mp3 also writes 192 kbps mono MP3s with a LAME gapless header and manifest.json.
// Review those three files, then copy them to rust/spectrum-wgpu/assets/test-audio/.
//
// Each file is exactly one 32-bar loop. Two loops are rendered and the second one kept, so
// the reverb and delay tails of the end are already under the start. The voices that never
// stop (the detuned pad of track 2) are not periodic over the loop, so the last 20 ms fade
// into the end of the first loop, which runs straight on into the start of the second:
// without that the wrap is a click, a bright vertical line in the waterfall every 90 s.
// 24-bit: the quantisation noise of 16-bit would show in the high bands at −116 dB or so once
// "show low levels" puts the floor at −120 dB. WAVs remain generated; MP3 assets are committed.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { options, withBrowser } from './browser-session.mjs';

const opts = options({ output: { type: 'string' }, mp3: { type: 'boolean' } });
const manifest = { sourceSha256: '', sampleRate: 48000, channels: 1, bitrate: 192000, tracks: [] };
const root = resolve(import.meta.dirname, '..');
const dir = resolve(opts.output || `${root}/toolchains/test-audio`);
const source = await readFile(`${root}/web/src/test-audio.js`);
manifest.sourceSha256 = createHash('sha256').update(source).digest('hex');
const SAMPLE_RATE = 48000;
// Beats a minute of each track, as its header in test-audio.js says; the loop is 32 bars of 4.
const tracks = [{ which: 1, name: 'Pulse', bpm: 124 }, { which: 2, name: 'Drift', bpm: 85 }];

function wav(pcm24) {
  const header = Buffer.alloc(44);
  header.write('RIFF', 0);
  header.writeUInt32LE(36 + pcm24.length, 4);
  header.write('WAVEfmt ', 8);
  header.writeUInt32LE(16, 16);
  header.writeUInt16LE(1, 20);                 // PCM
  header.writeUInt16LE(1, 22);                 // mono
  header.writeUInt32LE(SAMPLE_RATE, 24);
  header.writeUInt32LE(SAMPLE_RATE * 3, 28);
  header.writeUInt16LE(3, 32);
  header.writeUInt16LE(24, 34);
  header.write('data', 36);
  header.writeUInt32LE(pcm24.length, 40);
  return Buffer.concat([header, pcm24]);
}

await mkdir(dir, { recursive: true });
await withBrowser(opts, async (a) => {
  await a.eval(`import('data:text/javascript;base64,${source.toString('base64')}').then((m) => { window.testAudio = m; return true; })`);
  for (const { which, name, bpm } of tracks) {
    const loop = Math.round(32 * 4 * 60 / bpm * SAMPLE_RATE);
    const lead = Math.round(0.05 * SAMPLE_RATE);  // renderTestAudio starts its first step 50 ms in
    const info = await a.eval(`(async () => {
      const mono = await testAudio.renderTestAudio(${which}, ${(2 * loop + lead) / SAMPLE_RATE});
      const bytes = new Uint8Array(${loop} * 3);
      let peak = 0, clipped = 0;
      const FADE = ${Math.round(0.02 * SAMPLE_RATE)};
      for (let i = 0; i < ${loop}; i++) {
        const w = Math.max(0, (i - ${loop} + FADE) / FADE);
        const x = mono[${loop + lead} + i] * (1 - w) + (w ? mono[${lead} + i] * w : 0);
        peak = Math.max(peak, Math.abs(x));
        if (Math.abs(x) > 1) clipped++;
        const v = Math.round(Math.max(-1, Math.min(1, x)) * 8388607);
        bytes[i * 3] = v & 255; bytes[i * 3 + 1] = (v >> 8) & 255; bytes[i * 3 + 2] = (v >> 16) & 255;
      }
      window.rendered = bytes;
      return { peak, clipped, length: bytes.length };
    })()`, 600000);
    const parts = [];
    const CHUNK = 3 * 200000;
    for (let at = 0; at < info.length; at += CHUNK) {
      parts.push(Buffer.from(await a.eval(`(() => {
        const part = rendered.subarray(${at}, ${at + CHUNK});
        let text = '';
        for (let i = 0; i < part.length; i += 16384) text += String.fromCharCode(...part.subarray(i, i + 16384));
        return btoa(text);
      })()`), 'base64'));
    }
    const pcm = Buffer.concat(parts);
    if (pcm.length !== loop * 3) throw Error(`Track ${which}: got ${pcm.length} bytes, expected ${loop * 3}`);
    const file = `${dir}/test-${which}.wav`;
    await writeFile(file, wav(pcm));
    if (opts.mp3) {
      const mp3 = `${dir}/test-${which}.mp3`;
      execFileSync('ffmpeg', ['-y', '-v', 'error', '-i', file, '-ar', '48000', '-ac', '1',
        '-codec:a', 'libmp3lame', '-b:a', '192k', '-write_xing', '1', mp3]);
      const encoded = await readFile(mp3);
      manifest.tracks.push({ file: `test-${which}.mp3`, name, samples: loop, bytes: encoded.length,
        sha256: createHash('sha256').update(encoded).digest('hex') });
      console.log(`${name} MP3: ${encoded.length} bytes`);
    }
    console.log(`${name}: ${(loop / SAMPLE_RATE).toFixed(2)} s, peak ${(20 * Math.log10(info.peak)).toFixed(1)} dBFS, ${info.clipped} samples over full scale, ${file}`);
  }
  if (a.logs.length) throw Error(`Browser errors: ${a.logs.join('\n')}`);
});

if (opts.mp3) await writeFile(`${dir}/manifest.json`, JSON.stringify(manifest, null, 2) + '\n');
