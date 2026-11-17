#!/usr/bin/env node
// One version for every platform. The number lives in the VERSION file at the repository root;
// this script copies it into each place a build reads it from, or checks that they all agree.
//
//   node scripts/set-version.mjs 0.3.0-beta.2   write VERSION and every copy
//   node scripts/set-version.mjs                copy the current VERSION everywhere
//   node scripts/set-version.mjs --check        exit 1 if any copy differs (CI and release)
//   node scripts/set-version.mjs --print-code   print the Android versionCode for VERSION
//
// Versions are MAJOR.MINOR.PATCH, optionally followed by -beta.N or -rc.N. A release is one
// commit and one tag (v + the version) covering the web page, the Android library and app, and
// the desktop program together: no platform gets a number of its own.
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SHAPE = /^(\d+)\.(\d+)\.(\d+)(?:-(beta|rc)\.(\d+))?$/;

function fail(message) { console.error(message); process.exit(1); }

function parse(version) {
  const m = SHAPE.exec(version);
  if (!m) fail(`"${version}" is not MAJOR.MINOR.PATCH, MAJOR.MINOR.PATCH-beta.N or MAJOR.MINOR.PATCH-rc.N`);
  const [major, minor, patch] = m.slice(1, 4).map(Number);
  const n = m[5] === undefined ? 0 : Number(m[5]);
  if (minor > 99 || patch > 99 || n > 49 || (m[4] && n < 1)) fail(`"${version}" is out of range for the Android versionCode`);
  // beta.N -> N, rc.N -> 50 + N, the release itself -> 99, so every later build has a higher code.
  const stage = m[4] === 'beta' ? n : m[4] === 'rc' ? 50 + n : 99;
  return { numeric: `${major}.${minor}.${patch}`, code: major * 1000000 + minor * 10000 + patch * 100 + stage };
}

// Each copy: the file, a pattern with the value in group 2, and the value it must hold.
function copies(version) {
  const { code } = parse(version);
  const cargo = (name) => ({ file: `rust/${name}/Cargo.toml`, pattern: /^(version = ")([^"]*)(")/m, value: version });
  const lock = (name) => ({ file: 'rust/Cargo.lock', pattern: new RegExp(`(name = "${name}"\\nversion = ")([^"]*)(")`), value: version });
  return [
    cargo('spectrum-core'), cargo('spectrum-wgpu'), cargo('shader-lab'),
    lock('spectrum-core'), lock('spectrum-wgpu'), lock('shader-lab'),
    { file: 'android/app/build.gradle.kts', pattern: /(versionName = ")([^"]*)(")/, value: version },
    { file: 'android/app/build.gradle.kts', pattern: /(versionCode = )(\d+)()/, value: String(code) },
    { file: 'web/index.html', pattern: /(<meta name="version" content=")([^"]*)(">)/, value: version },
  ];
}

const args = process.argv.slice(2);
const check = args.includes('--check');
const wanted = args.find((a) => !a.startsWith('--'));
const current = readFileSync(join(ROOT, 'VERSION'), 'utf8').trim();
const version = wanted ?? current;

if (args.includes('--print-code')) { console.log(parse(version).code); process.exit(0); }
if (check && wanted) fail('--check takes no version');

const wrong = [];
for (const { file, pattern, value } of copies(version)) {
  const path = join(ROOT, file);
  const text = readFileSync(path, 'utf8');
  const found = pattern.exec(text);
  if (!found) fail(`${file}: no version field matching ${pattern}`);
  if (found[2] === value) continue;
  wrong.push(`${file}: ${found[2]} (expected ${value})`);
  if (!check) writeFileSync(path, text.replace(pattern, `$1${value}$3`));
}

if (check) {
  if (wrong.length) fail(`Version copies differ from VERSION (${version}):\n  ${wrong.join('\n  ')}\nRun: node scripts/set-version.mjs`);
  console.log(`Version ${version}: every copy agrees (Android versionCode ${parse(version).code})`);
} else {
  if (wanted && wanted !== current) writeFileSync(join(ROOT, 'VERSION'), `${version}\n`);
  console.log(`Version ${version} (Android versionCode ${parse(version).code}): ${wrong.length} place(s) updated`);
}
