// Small WebGL2 helpers. Shaders are plain GLSL ES 3.00 files with a one-level #include,
// so the same files can be copied into the Android app's assets.

export async function loadShader(name, root = 'shaders') {
  const text = await (await fetch(`${root}/${name}`, { cache: 'no-store' })).text();
  const parts = await Promise.all(text.split('\n').map(async (line) => {
    const m = line.match(/^#include "(.+)"/);
    return m ? (await fetch(`shaders/${m[1]}`, { cache: 'no-store' })).text() : line;
  }));
  return parts.join('\n');
}

function compile(gl, type, source, name) {
  const shader = gl.createShader(type);
  gl.shaderSource(shader, source);
  gl.compileShader(shader);
  if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
    throw new Error(`${name}: ${gl.getShaderInfoLog(shader)}`);
  }
  return shader;
}

export function createProgram(gl, vsSource, fsSource, name) {
  const program = gl.createProgram();
  const vs = compile(gl, gl.VERTEX_SHADER, vsSource, `${name}.vert`);
  const fs = compile(gl, gl.FRAGMENT_SHADER, fsSource, `${name}.frag`);
  gl.attachShader(program, vs); gl.attachShader(program, fs);
  gl.linkProgram(program);
  gl.deleteShader(vs); gl.deleteShader(fs);
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
    throw new Error(`${name}: ${gl.getProgramInfoLog(program)}`);
  }
  const uniforms = {};
  const count = gl.getProgramParameter(program, gl.ACTIVE_UNIFORMS);
  for (let i = 0; i < count; i++) {
    const info = gl.getActiveUniform(program, i);
    uniforms[info.name] = gl.getUniformLocation(program, info.name);
  }
  return { program, uniforms };
}

export function hexToRgb(hex) {
  const n = parseInt(hex.slice(1), 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
}

// OKLab interpolation keeps palette steps perceptually even.
function srgbToLinear(c) { return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; }
function linearToSrgb(c) { return c <= 0.0031308 ? c * 12.92 : 1.055 * c ** (1 / 2.4) - 0.055; }

function toOklab([r, g, b]) {
  r = srgbToLinear(r); g = srgbToLinear(g); b = srgbToLinear(b);
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [
    0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
  ];
}

function fromOklab([L, a, b]) {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.2914855480 * b) ** 3;
  const rgb = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
  ];
  return rgb.map((c) => Math.min(1, Math.max(0, linearToSrgb(c))));
}

export function paletteBytes(stops, size = 256) {
  const labs = stops.map((h) => toOklab(hexToRgb(h)));
  const out = new Uint8Array(size * 4);
  for (let i = 0; i < size; i++) {
    const x = (i / (size - 1)) * (labs.length - 1);
    const k = Math.min(Math.floor(x), labs.length - 2);
    const f = x - k;
    const lab = labs[k].map((v, j) => v + (labs[k + 1][j] - v) * f);
    const rgb = fromOklab(lab);
    out.set([rgb[0] * 255, rgb[1] * 255, rgb[2] * 255, 255], i * 4);
  }
  return out;
}
