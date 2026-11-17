#!/usr/bin/env node
// No browser or encoder required. Run before native builds on every platform.
import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
const root = new URL('../', import.meta.url);
const dir = new URL('rust/spectrum-wgpu/assets/test-audio/', root);
const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const manifest = JSON.parse(await readFile(new URL('manifest.json', dir)));
if (manifest.sourceSha256 !== hash(await readFile(new URL('web/src/test-audio.js', root)))) {
  throw Error('测试曲过期，重新渲染：node scripts/render-test-audio.mjs --mp3，然后更新 rust/spectrum-wgpu/assets/test-audio/');
}
for (const track of manifest.tracks) {
  const bytes = await readFile(new URL(track.file, dir));
  if (bytes.length !== track.bytes || hash(bytes) !== track.sha256) throw Error(`Test audio damaged: ${track.file}`);
}
console.log('Test audio: current source, both MP3 hashes verified');
