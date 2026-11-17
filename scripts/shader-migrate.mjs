#!/usr/bin/env node
// Experimental build adapter: original GLES shaders remain canonical.
import { readdir, readFile, mkdir, writeFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { execFileSync, spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { parseArgs } from 'node:util';
const root = resolve(import.meta.dirname, '..');
const { values: opts } = parseArgs({ options: { date: { type: 'string', default: new Date().toISOString().slice(0, 10) }, report: { type: 'string' }, output: { type: 'string' } } });
// The report goes under toolchains/ unless a path is given; docs/perf/ holds committed records only.
const reportPath = opts.report ?? opts.output;
if (!/^\d{4}-\d{2}-\d{2}$/.test(opts.date)) throw Error('--date must be YYYY-MM-DD');
const out = resolve(root, 'toolchains/shader-lab'); await mkdir(out, { recursive: true });
execFileSync('cargo', ['build', '--manifest-path', `${root}/rust/Cargo.toml`, '-p', 'shader-lab'], { stdio: 'inherit' });
const report = { date: new Date().toISOString(), adapter: 'GLSL ES 300 -> GLSL 450 uniform block + split samplers + locations -> naga 27 -> WGSL', shaders: [] };
for (const style of ['tungsten', 'dadaspec']) {
 const custom = style === 'tungsten' ? [] : await readdir(`${root}/web/styles/${style}/shaders`);
 const names = [...new Set([...(await readdir(`${root}/web/shaders`)), ...custom])].filter(n => /\.(vert|frag)$/.test(n));
 const sources = {};
 for (const name of names) {
   let src = await readFile(custom.includes(name) ? `${root}/web/styles/${style}/shaders/${name}` : `${root}/web/shaders/${name}`, 'utf8');
   src = src.replace(/^#include "(.+)"/gm, (_, n) => readFileSync(resolve(root, 'web/shaders', n), 'utf8'));
   sources[name] = src;
 }
 const uniforms = new Map(); const varyings = new Map();
 for (const src of Object.values(sources)) {
   for (const m of src.matchAll(/^uniform\s+(\w+)\s+(\w+)\s*;/gm)) if (!m[1].startsWith('sampler')) uniforms.set(m[2], m[1]);
   for (const m of src.matchAll(/^(?:flat\s+)?(?:in|out)\s+\w+\s+(v_\w+)\s*;/gm)) varyings.set(m[1], 0);
 }
 [...varyings.keys()].sort().forEach((n, i) => varyings.set(n, i));
 const fields = [...uniforms].sort(([a], [b]) => a.localeCompare(b));
 let offset = 0; const layout = {};
 for (const [name, type] of fields) {
   const alignment = type === 'mat4' || type === 'vec3' || type === 'vec4' ? 16 : type === 'vec2' ? 8 : 4;
   offset = Math.ceil(offset / alignment) * alignment;
   layout[name] = { type, offset }; offset += type === 'mat4' ? 64 : type === 'vec3' ? 12 : type === 'vec4' ? 16 : type === 'vec2' ? 8 : 4;
 }
 await mkdir(`${out}/${style}`, { recursive: true });
 await writeFile(`${out}/${style}/uniforms.json`, JSON.stringify({ size: Math.ceil(offset / 16) * 16, fields: layout }, null, 2));
 const block = 'layout(set=0,binding=0,std140) uniform Params {\n' + fields.map(([n, t]) => `  ${t} ${n};`).join('\n') + '\n};\n';
 for (const [name, source] of Object.entries(sources)) {
   let src = source.replace(/^#version.*$/m, '#version 450').replace(/^precision\s+.*?;\s*$/gm, '');
   src = src.replace(/^uniform\s+(\w+)\s+(\w+)\s*;/gm, (_, type, name) => {
     if (!type.startsWith('sampler')) return '';
     const binding = name === 'u_history' ? 1 : 3;
     return `layout(set=0,binding=${binding}) uniform texture2D t_${name};\nlayout(set=0,binding=${binding+1}) uniform sampler s_${name};\n#define ${name} sampler2D(t_${name},s_${name})`;
   });
   src = src.replace('#version 450', '#version 450\n' + block);
   src = src.replace(/^(flat\s+)?(in|out)\s+(\w+)\s+(\w+)\s*;/gm, (_, flat, io, type, n) => `layout(location=${varyings.get(n) ?? 0}) ${flat || ''}${io} ${type} ${n};`);
   src = src.replaceAll('gl_VertexID', 'gl_VertexIndex').replaceAll('gl_InstanceID', 'gl_InstanceIndex');
   if (name.endsWith('.vert')) {
     src = src.replace(/texture\(u_history, uv\)/g, 'textureLod(u_history, uv, 0.0)');
     // The canonical GLSL uses OpenGL clip depth [-w,+w]; WebGPU clips at [0,+w].
     // Keep its camera matrices unchanged so host projection and inverse uniforms agree.
     src = src.replace(/}\s*$/, '  gl_Position.z = (gl_Position.z + gl_Position.w) * 0.5;\n}\n');
   }
   if (name.endsWith('.frag') && src.includes('gl_FragCoord')) {
     // WGSL fragment positions have a top-left origin, whereas canonical GLSL's
     // inverse projection expects a bottom-left origin. The built-in itself is read-only.
     src = src.replaceAll('gl_FragCoord', 'spectrum_FragCoord');
     src = src.replace(/void main\(\)\s*{/, 'vec4 spectrum_FragCoord;\nvoid main() {\n  spectrum_FragCoord = gl_FragCoord;\n  spectrum_FragCoord.y = u_viewport.y - spectrum_FragCoord.y;');
   }
   const path = `${out}/${style}/${name}`;await writeFile(path, src);
   const executable = `${root}/rust/target/debug/shader-lab${process.platform === 'win32' ? '.exe' : ''}`;
   const run = spawnSync(executable, [path, path + '.wgsl'], { encoding: 'utf8' });
   report.shaders.push({ style, shader: name, ok: run.status === 0, error: run.status === 0 ? undefined : run.stderr });
 }
}
report.passed = report.shaders.filter(s => s.ok).length; report.total = report.shaders.length;
await writeFile(reportPath ? resolve(reportPath) : `${out}/${opts.date}-shader-migration.json`, JSON.stringify(report, null, 2));
console.log(`${report.passed}/${report.total} shaders migrated`);
for (const s of report.shaders.filter(s => !s.ok)) console.log(s.style, s.shader, s.error.slice(0, 1500));
if (report.passed !== report.total) process.exitCode = 1;
