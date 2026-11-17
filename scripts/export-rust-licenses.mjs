#!/usr/bin/env node
// Writes the third-party licence notice for the Rust desktop program (spectrum-wgpu).
// Usage: node scripts/export-rust-licenses.mjs --output <file> [--target <triple>]
// Only normal dependencies that end up in the binary are listed; build and dev
// dependencies are skipped. Exits 1 on a licence outside the allow-list.
import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync, mkdirSync, writeFileSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const ROOT_PACKAGE = 'spectrum-wgpu';

const ALLOWED = new Set([
  'MIT', 'MIT-0', 'Apache-2.0', 'Apache-2.0 WITH LLVM-exception', 'BSD-2-Clause', 'BSD-3-Clause',
  'ISC', 'Zlib', '0BSD', 'Unlicense', 'CC0-1.0', 'BSL-1.0', 'MPL-2.0', 'Unicode-3.0',
  'Unicode-DFS-2016', 'OFL-1.1', 'Ubuntu-font-1.0', 'LicenseRef-UFL-1.0', 'NCSA',
]);

const LICENSE_NAME = /^(LICEN[CS]E|COPYING|COPYRIGHT|NOTICE|UNLICENSE|PATENTS)/i;
const FONT_LICENSE_NAME = /(OFL|UFL|LICEN[CS]E)/i;

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--output') args.output = argv[++i];
    else if (argv[i] === '--target') args.target = argv[++i];
    else fail(`unknown argument: ${argv[i]}`);
  }
  if (!args.output) fail('usage: node scripts/export-rust-licenses.mjs --output <file> [--target <triple>]');
  return args;
}

function fail(message) {
  console.error(message);
  process.exit(1);
}

function run(cmd, args) {
  const r = spawnSync(cmd, args, {
    cwd: ROOT, encoding: 'utf8', maxBuffer: 512 * 1024 * 1024,
    shell: process.platform === 'win32',
  });
  if (r.error || r.status !== 0) fail(`${cmd} ${args.join(' ')} failed: ${r.error?.message ?? r.stderr}`);
  return r.stdout;
}

function hostTriple() {
  const line = run('rustc', ['-vV']).split(/\r?\n/).find((l) => l.startsWith('host:'));
  if (!line) fail('cannot read the host triple from rustc -vV');
  return line.slice(5).trim();
}

// Identifiers of an SPDX expression, with "X WITH Y" kept together.
function licenseIds(expr) {
  return expr.replace(/[()]/g, ' ').split(/\s+(?:AND|OR)\s+|\//i).flatMap((part) => {
    const id = part.trim().replace(/\s+/g, ' ');
    return id ? [id] : [];
  });
}

function normalise(text) {
  return text.replace(/\r\n?/g, '\n').split('\n').map((l) => l.trimEnd()).join('\n').trim();
}

function readFiles(dir, nameRe) {
  let entries;
  try { entries = readdirSync(dir, { withFileTypes: true }); } catch { return []; }
  return entries
    .filter((e) => e.isFile() && nameRe.test(e.name))
    .map((e) => e.name)
    .sort()
    .map((name) => ({ name, text: normalise(readFileSync(join(dir, name), 'utf8')) }))
    .filter((f) => f.text);
}

// Licence files of one crate: top-level ones, plus font licences one folder down for font crates.
function licenseFiles(pkg) {
  const dir = dirname(pkg.manifest_path);
  const files = readFiles(dir, LICENSE_NAME);
  if (/font/i.test(pkg.name)) {
    const subdirs = readdirSync(dir, { withFileTypes: true }).filter((e) => e.isDirectory()).map((e) => e.name).sort();
    for (const sub of subdirs) {
      for (const f of readFiles(join(dir, sub), FONT_LICENSE_NAME)) files.push({ name: `${sub}/${f.name}`, text: f.text });
    }
  }
  if (!files.length && pkg.license_file) {
    const p = join(dir, pkg.license_file);
    try {
      if (statSync(p).isFile()) files.push({ name: pkg.license_file, text: normalise(readFileSync(p, 'utf8')) });
    } catch { /* reported as missing below */ }
  }
  // Some crates (symphonia, MPL-2.0) publish no licence file at all. Where rust/licenses/ holds
  // the standard text of an identifier in their expression, print that instead of nothing.
  if (!files.length && pkg.license) {
    for (const id of licenseIds(pkg.license)) {
      const standard = join(ROOT, 'rust/licenses', `${id}.txt`);
      if (existsSync(standard)) files.push({ name: `standard ${id} text, the package ships no licence file`, text: normalise(readFileSync(standard, 'utf8')) });
    }
  }
  return files;
}

const { output, target: targetArg } = parseArgs(process.argv.slice(2));
const target = targetArg ?? hostTriple();

const meta = JSON.parse(run('cargo', [
  'metadata', '--format-version', '1', '--locked', '--manifest-path', 'rust/Cargo.toml', '--filter-platform', target,
]));
const byId = new Map(meta.packages.map((p) => [p.id, p]));
const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]));
const root = meta.packages.find((p) => p.name === ROOT_PACKAGE && meta.workspace_members.includes(p.id));
if (!root) fail(`workspace package ${ROOT_PACKAGE} not found`);

// Walk normal dependencies only (dep_kinds entry with kind null).
const seen = new Set([root.id]);
const stack = [root.id];
while (stack.length) {
  const node = nodes.get(stack.pop());
  for (const dep of node?.deps ?? []) {
    if (!dep.dep_kinds.some((k) => k.kind === null) || seen.has(dep.pkg)) continue;
    seen.add(dep.pkg);
    stack.push(dep.pkg);
  }
}

const crates = [...seen].map((id) => byId.get(id)).filter((p) => p.source !== null)
  .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : a.version < b.version ? -1 : a.version > b.version ? 1 : 0));

const problems = [];
const idsSeen = new Set();
const entries = crates.map((pkg) => {
  const expr = pkg.license ?? (pkg.license_file ? `(see ${pkg.license_file})` : null);
  if (!pkg.license && !pkg.license_file) problems.push(`${pkg.name} ${pkg.version}: no license or license_file`);
  for (const id of pkg.license ? licenseIds(pkg.license) : []) {
    idsSeen.add(id);
    if (!ALLOWED.has(id)) problems.push(`${pkg.name} ${pkg.version}: licence "${id}" (expression: ${pkg.license}) is not in the allow-list`);
  }
  return { pkg, expr: expr ?? 'UNKNOWN', files: licenseFiles(pkg) };
});

if (problems.length) fail(`Licence check failed:\n${problems.map((p) => `  ${p}`).join('\n')}`);

// Group the licence texts by exact content, in first-seen order of the sorted crate list.
const texts = new Map();
for (const { pkg, files } of entries) {
  for (const f of files) {
    if (!texts.has(f.text)) texts.set(f.text, []);
    texts.get(f.text).push(`${pkg.name} ${pkg.version} (${f.name})`);
  }
}
const noFile = entries.filter((e) => !e.files.length);

const rule = '='.repeat(78);
const out = [];
out.push('Third-party Rust crates in Spectrum 3D (the desktop program, spectrum-wgpu)');
out.push('');
out.push('This file lists the third-party Rust crates linked into the desktop program');
out.push('and reproduces their licence texts. All of them are used unmodified; the source');
out.push('code of each, including the crates under MPL-2.0, is available from the');
out.push('repository listed next to it.');
out.push(`Target: ${target}`);
out.push(`Crates: ${entries.length}`);
out.push('');
out.push(rule, 'Crates', rule, '');
for (const { pkg, expr } of entries) out.push(`${pkg.name} ${pkg.version} — ${expr} — ${pkg.repository ?? 'no repository listed'}`);
out.push('', rule, 'Licence texts', rule);
for (const [text, users] of texts) {
  out.push('', 'Used by:');
  for (const u of users) out.push(`  ${u}`);
  out.push('', '-'.repeat(78), text);
}
out.push('', rule, 'Crates without a licence file in their package', rule, '');
if (noFile.length) {
  for (const { pkg, expr } of noFile) out.push(`${pkg.name} ${pkg.version} — ${expr} — ${pkg.repository ?? 'no repository listed'}`);
} else out.push('(none)');
out.push('');

mkdirSync(dirname(resolve(output)), { recursive: true });
writeFileSync(output, out.join('\n'), 'utf8');
console.log(`${entries.length} crates, ${texts.size} distinct licence texts, ${noFile.length} without a licence file, written to ${output}`);
console.log(`Identifiers: ${[...idsSeen].sort().join(', ')}`);
