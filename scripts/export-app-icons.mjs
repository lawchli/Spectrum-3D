#!/usr/bin/env node
// Android's adaptive launcher icon is the source of truth. Normal package builds
// copy the checked-in results; regeneration needs rsvg-convert, magick and iconutil (Mac).
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
const root = resolve(import.meta.dirname, '..');
const dir = resolve(root, 'rust/spectrum-wgpu/assets/icons');
const foreground = await readFile(resolve(root, 'android/app/src/main/res/drawable/ic_spectrum_foreground.xml'), 'utf8');
const colors = await readFile(resolve(root, 'android/app/src/main/res/values/colors.xml'), 'utf8');
const sourceSha256 = createHash('sha256').update(foreground).update(colors).digest('hex');
if (process.argv.includes('--check')) {
  const manifest = JSON.parse(await readFile(resolve(dir, 'manifest.json')));
  if (manifest.sourceSha256 !== sourceSha256) throw Error('App icons are stale: run node scripts/export-app-icons.mjs on Mac');
  for (const [name, hash] of Object.entries(manifest.files)) {
    if (createHash('sha256').update(await readFile(resolve(dir, name))).digest('hex') !== hash) throw Error(`App icon damaged: ${name}`);
  }
  console.log('App icons: Android source and generated assets verified');
} else {
  const attrs = text => Object.fromEntries([...text.matchAll(/android:(\w+)="([^"]*)"/g)].map(m => [m[1], m[2]]));
  const vector = attrs(foreground.match(/<vector\b[^>]*>/)[0]);
  if (vector.viewportWidth !== '108' || vector.viewportHeight !== '108') throw Error('Review the adaptive icon viewport before exporting');
  const background = colors.match(/<color name="icon_background">([^<]+)<\/color>/)?.[1];
  if (!background) throw Error('Missing Android icon background');
  const paths = [...foreground.matchAll(/<path\b[^>]*\/>/g)].map(([path]) => {
    const a = attrs(path);
    if (a.fillColor !== '#00000000') throw Error('Review unsupported Android path fill');
    return `<path d="${a.pathData}" fill="none" stroke="${a.strokeColor}" stroke-width="${a.strokeWidth}" stroke-linecap="${a.strokeLineCap || 'butt'}" stroke-linejoin="${a.strokeLineJoin || 'miter'}"/>`;
  });
  // The visible 72 dp centre of the adaptive icon, with the same foreground and
  // background. Desktop mask rounds its corners; no new logo is drawn.
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="18 18 72 72"><rect x="18" y="18" width="72" height="72" rx="16" fill="${background}"/>${paths.join('')}</svg>\n`;
  await mkdir(dir, { recursive: true });
  await writeFile(resolve(dir, 'spectrum.svg'), svg);
  const iconset = resolve(root, 'toolchains/app-icons/Spectrum.iconset');
  await mkdir(iconset, { recursive: true });
  for (const size of [16, 32, 128, 256, 512]) {
    for (const scale of [1, 2]) {
      execFileSync('rsvg-convert', ['-w', String(size * scale), '-h', String(size * scale), '-o',
        resolve(iconset, `icon_${size}x${size}${scale === 2 ? '@2x' : ''}.png`), resolve(dir, 'spectrum.svg')]);
    }
  }
  execFileSync('rsvg-convert', ['-w', '512', '-h', '512', '-o', resolve(dir, 'spectrum.png'), resolve(dir, 'spectrum.svg')]);
  execFileSync('iconutil', ['-c', 'icns', iconset, '-o', resolve(dir, 'Spectrum.icns')]);
  execFileSync('magick', [resolve(dir, 'spectrum.png'), '-define', 'icon:auto-resize=256,128,64,48,32,16', resolve(dir, 'spectrum.ico')]);
  const files = {};
  for (const name of ['spectrum.svg', 'spectrum.png', 'Spectrum.icns', 'spectrum.ico']) {
    files[name] = createHash('sha256').update(await readFile(resolve(dir, name))).digest('hex');
  }
  await writeFile(resolve(dir, 'manifest.json'), JSON.stringify({ sourceSha256, files }, null, 2) + '\n');
  console.log(`Exported Android launcher icon to ${dir}`);
}
