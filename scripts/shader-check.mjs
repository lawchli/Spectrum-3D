#!/usr/bin/env node
// Input is an include-expanded shader/pair manifest made by web-check.py.
import { readFile } from 'node:fs/promises';
import { options, withBrowser } from './browser-session.mjs';

const opts = options({ manifest: { type: 'string' } });
if (!opts.manifest) throw new Error('--manifest PATH required');
const manifest = JSON.parse(await readFile(opts.manifest, 'utf8'));
const result = await withBrowser(opts, async a => a.eval(`(() => {
  const input = ${JSON.stringify(manifest)};
  const canvas = document.createElement('canvas');
  const gl = canvas.getContext('webgl2');
  if (!gl) throw new Error('WebGL2 unavailable');
  const shaders = new Map(), errors = [];
  for (const entry of input.shaders) {
    const shader = gl.createShader(entry.kind === 'vert' ? gl.VERTEX_SHADER : gl.FRAGMENT_SHADER);
    gl.shaderSource(shader, entry.source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
      errors.push({ shader: entry.name, stage: 'compile', log: gl.getShaderInfoLog(shader) });
      gl.deleteShader(shader);
    } else shaders.set(entry.name, shader);
  }
  let linked = 0;
  for (const [vert, frag] of input.pairs) {
    if (!shaders.has(vert) || !shaders.has(frag)) continue;
    const p = gl.createProgram();
    gl.attachShader(p, shaders.get(vert)); gl.attachShader(p, shaders.get(frag));
    gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) errors.push({ shader: vert + ' + ' + frag, stage: 'link', log: gl.getProgramInfoLog(p) });
    else linked++;
    gl.deleteProgram(p);
  }
  for (const shader of shaders.values()) gl.deleteShader(shader);
  return { compiled: shaders.size, linked, errors, renderer: gl.getParameter(gl.RENDERER) };
})()`));
console.log(JSON.stringify(result));
if (result.errors.length) process.exitCode = 1;
