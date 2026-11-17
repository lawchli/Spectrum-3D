// WebGL2 renderer for the waterfall. Host-side code only: the history ring texture, the
// grid mesh, the live front spectrum and static guides. The Android port rewrites this
// file in Kotlin (GLES30) and keeps the shaders unchanged.

import { loadShader, createProgram, hexToRgb, paletteBytes } from './gl.js';
import { BANDS, SUBDIVISIONS, bandCount, hzToBand, bandToU, hzTicks } from './bands.js';
import { invert, transform } from './mat4.js';

import { History, HISTORY_ROWS, LEVELS } from './history.js';
export { HISTORY_ROWS, LEVELS } from './history.js';
const NEAR_MESH_ROWS = 360;

function smoothstep(e0, e1, x) {
  const t = Math.max(0, Math.min(1, (x - e0) / (e1 - e0)));
  return t * t * (3 - 2 * t);
}

function unproject(inv, nx, ny, nz) {
  const p = transform(inv, nx, ny, nz, 1);
  const w = p[3] || 1e-7;
  return [p[0] / w, p[1] / w, p[2] / w];
}

// Smooth view-orientation weights derived solely from u_viewProj^-1 so transitions
// between Front (1), Top (2), and 3D Perspective (3) blend continuously.
export function viewMetrics(inv) {
  if (!inv) return { topness: 0, frontness: 0, fadeDb: 1, timeVis: 1 };
  const n0 = unproject(inv, 0, 0, -1), f0 = unproject(inv, 0, 0, 1);
  let bx = n0[0] - f0[0], by = n0[1] - f0[1], bz = n0[2] - f0[2];
  const bl = Math.hypot(bx, by, bz) || 1;
  bx /= bl; by /= bl; bz /= bl;

  const pitchRad = Math.asin(Math.max(-1, Math.min(1, by)));
  const pitchDeg = pitchRad * (180 / Math.PI);

  const nt = unproject(inv, 0, 1, -1), ft = unproject(inv, 0, 1, 1);
  const nb = unproject(inv, 0, -1, -1), fb = unproject(inv, 0, -1, 1);
  let tx = ft[0] - nt[0], ty = ft[1] - nt[1], tz = ft[2] - nt[2];
  const tl = Math.hypot(tx, ty, tz) || 1; tx /= tl; ty /= tl; tz /= tl;
  let rx = fb[0] - nb[0], ry = fb[1] - nb[1], rz = fb[2] - nb[2];
  const rl = Math.hypot(rx, ry, rz) || 1; rx /= rl; ry /= rl; rz /= rl;
  const fovDeg = Math.acos(Math.max(-1, Math.min(1, tx * rx + ty * ry + tz * rz))) * (180 / Math.PI);

  const topness = smoothstep(62, 86, pitchDeg);
  const flatLens = 1 - smoothstep(3, 18, fovDeg);
  const frontness = smoothstep(0.95, 0.998, bz) * (0.30 + 0.70 * flatLens);
  const fadeDb = Math.max(0, Math.min(1, Math.cos(pitchRad) * 1.75 - 0.35));
  const timeVis = Math.max(0, Math.min(1, (Math.hypot(bx, by) - 0.12) * 3.2));
  return { topness, frontness, fadeDb, timeVis };
}

export class Renderer {
  static async create(gl, look) {
    const src = Object.fromEntries(await Promise.all(
      ['terrain.vert', 'surface.frag', 'grid.frag', 'front.vert', 'front.frag', 'line.vert', 'line.frag', 'keys.vert', 'keys.frag']
        .map(async (n) => [n, await loadShader(n)]),
    ));
    return new Renderer(gl, look, src);
  }

  constructor(gl, look, src) {
    this.gl = gl;
    this.look = look;
    this.melAxis = 0;  // frequency axis: 0 a semitone everywhere the same width, 1 mel (bands.js bandToU)
    this.surface = createProgram(gl, src['terrain.vert'], src['surface.frag'], 'surface');
    this.grid = createProgram(gl, src['terrain.vert'], src['grid.frag'], 'grid');
    this.front = createProgram(gl, src['front.vert'], src['front.frag'], 'front');
    this.lines = createProgram(gl, src['line.vert'], src['line.frag'], 'line');
    this.keyProgram = createProgram(gl, src['keys.vert'], src['keys.frag'], 'keys');

    this.historyTex = gl.createTexture();
    this.maxTexture = gl.getParameter(gl.MAX_TEXTURE_SIZE);
    this.setBands(1);

    this.paletteTex = gl.createTexture();
    this.setPalette(look.palette);
    this.keys = this.makeKeyBuffer();

    // Per-frame counts for the monitor.
    this.stats = { calls: 0, vertices: 0 };

    this.guides = { vao: gl.createVertexArray(), vbo: gl.createBuffer(), slices: {} };
    this.mesh = null;
  }

  // Data bands per semitone (bands.js). Reallocates the history, which starts over empty,
  // and the front spectrum buffers. The history texture holds the LEVELS levels side by side.
  setBands(sub) {
    const gl = this.gl;
    const count = bandCount(sub);
    this.sub = sub;
    this.count = count;
    // Every level is also kept on the CPU for readouts.
    this.historyStore = new History(count);
    this.history = this.historyStore.history;
    this.rowsPushed = 0;
    gl.bindTexture(gl.TEXTURE_2D, this.historyTex);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.R8, count * LEVELS, HISTORY_ROWS, 0, gl.RED, gl.UNSIGNED_BYTE,
      new Uint8Array(count * LEVELS * HISTORY_ROWS));
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.REPEAT);

    // Front spectrum: line (band, dB) and curtain (band, dB, top) vertices.
    for (const b of [this.frontLine, this.peakLine, this.curtain, this.maxLine]) {
      if (!b) continue;
      gl.deleteVertexArray(b.vao);
      gl.deleteBuffer(b.vbo);
    }
    this.frontLine = this.makeFrontBuffer(1);
    this.peakLine = this.makeFrontBuffer(1);
    this.curtain = this.makeFrontBuffer(2);
    this.maxLine = this.makeFrontBuffer(1);
  }

  // Finer settings than the GPU's texture width allows are left out.
  get subdivisions() {
    return SUBDIVISIONS.filter((sub) => bandCount(sub) * LEVELS <= this.maxTexture);
  }

  setPalette(name) {
    const gl = this.gl;
    gl.bindTexture(gl.TEXTURE_2D, this.paletteTex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, 256, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, paletteBytes(this.look.palettes[name]));
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    this.palette = name;
  }

  makeFrontBuffer(perBand) {
    const gl = this.gl;
    const vao = gl.createVertexArray();
    const vbo = gl.createBuffer();
    const { count, sub } = this;
    const data = new Float32Array(count * perBand * 3);
    for (let k = 0; k < count; k++) {
      for (let j = 0; j < perBand; j++) {
        const i = (k * perBand + j) * 3;
        data[i] = k / sub;
        data[i + 1] = -120;
        data[i + 2] = perBand === 2 ? 1 - j : 1;
      }
    }
    gl.bindVertexArray(vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
    gl.bufferData(gl.ARRAY_BUFFER, data, gl.DYNAMIC_DRAW);
    for (let a = 0; a < 3; a++) {
      gl.enableVertexAttribArray(a);
      gl.vertexAttribPointer(a, 1, gl.FLOAT, false, 12, a * 4);
    }
    gl.bindVertexArray(null);
    return { vao, vbo, data, perBand };
  }

  updateFront(buffer, db) {
    const { data, perBand } = buffer;
    let changed = false;
    for (let k = 0; k < this.count; k++) if (data[k * perBand * 3 + 1] !== db[k]) { changed = true; break; }
    if (!changed) return;
    for (let k = 0; k < this.count; k++) {
      for (let j = 0; j < perBand; j++) data[(k * perBand + j) * 3 + 1] = db[k];
    }
    const gl = this.gl;
    gl.bindBuffer(gl.ARRAY_BUFFER, buffer.vbo);
    gl.bufferSubData(gl.ARRAY_BUFFER, 0, data);
  }

  // Piano strip: two triangles per key, vertices (edge band, side, key, lit).
  makeKeyBuffer() {
    const gl = this.gl;
    const vao = gl.createVertexArray();
    const vbo = gl.createBuffer();
    const data = new Float32Array(BANDS * 6 * 4);
    const corners = [[-0.5, 0], [0.5, 0], [-0.5, 1], [0.5, 0], [0.5, 1], [-0.5, 1]];
    for (let k = 0; k < BANDS; k++) {
      corners.forEach(([e, side], j) => data.set([k + e, side, k, 0], (k * 6 + j) * 4));
    }
    gl.bindVertexArray(vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
    gl.bufferData(gl.ARRAY_BUFFER, data, gl.DYNAMIC_DRAW);
    for (let a = 0; a < 4; a++) {
      gl.enableVertexAttribArray(a);
      gl.vertexAttribPointer(a, 1, gl.FLOAT, false, 16, a * 4);
    }
    gl.bindVertexArray(null);
    return { vao, vbo, data };
  }

  // lit[k]: 0..1 per key, how strongly a labelled peak sits on it.
  updateKeys(lit) {
    const { data } = this.keys;
    let changed = false;
    for (let k = 0; k < BANDS; k++) if (data[k * 6 * 4 + 3] !== lit[k]) { changed = true; break; }
    if (!changed) return;
    for (let k = 0; k < BANDS; k++) for (let j = 0; j < 6; j++) data[(k * 6 + j) * 4 + 3] = lit[k];
    const gl = this.gl;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.keys.vbo);
    gl.bufferSubData(gl.ARRAY_BUFFER, 0, data);
  }

  // Grid of (u, nominal age, step) vertices with triangles for the surface and two line sets:
  // lines along frequency at each time step (ridges) and lines along time at each frequency.
  // Mesh rows sit a power-of-two number of data rows apart, so each holds whole blocks of the
  // matching history level (terrain.vert): one data row apart across the framed part while
  // that stays under NEAR_MESH_ROWS, then doubling each time the age doubles down the tail,
  // up to 2^(LEVELS-1).
  buildMesh(cols, nearRows, spanRows) {
    const gl = this.gl;
    if (this.mesh) {
      gl.deleteVertexArray(this.mesh.vao);
      for (const b of this.mesh.buffers) gl.deleteBuffer(b);
    }
    const { everyFreq, everyTime } = this.look.grid;
    const maxStep = 2 ** (LEVELS - 1);
    let near = 1;
    while (near < maxStep && nearRows / near > NEAR_MESH_ROWS) near *= 2;
    const stepAt = (a) => (a < nearRows ? near : Math.min(maxStep, near * 2 ** (Math.floor(Math.log2(a / nearRows)) + 1)));
    const ages = [0];
    while (ages[ages.length - 1] < spanRows) ages.push(ages[ages.length - 1] + stepAt(ages[ages.length - 1]));
    const rows = ages.length;
    const verts = new Float32Array(cols * rows * 3);
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        const i = (r * cols + c) * 3;
        verts[i] = c / (cols - 1);
        verts[i + 1] = ages[r];
        verts[i + 2] = stepAt(ages[r]);
      }
    }
    const tris = [];
    for (let r = 0; r < rows - 1; r++) for (let c = 0; c < cols - 1; c++) {
      const i = r * cols + c;
      tris.push(i, i + 1, i + cols, i + 1, i + cols + 1, i + cols);
    }
    const ridges = [];
    for (let r = 0; r < rows; r += everyTime) for (let c = 0; c < cols - 1; c++) ridges.push(r * cols + c, r * cols + c + 1);
    const ribs = [];
    const colsPerBand = (cols - 1) / (BANDS - 1);
    for (let c = 0; c < cols; c += Math.max(1, Math.round(everyFreq * colsPerBand))) {
      for (let r = 0; r < rows - 1; r++) ribs.push(r * cols + c, (r + 1) * cols + c);
    }

    const vao = gl.createVertexArray();
    gl.bindVertexArray(vao);
    const vbo = gl.createBuffer();
    gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
    gl.bufferData(gl.ARRAY_BUFFER, verts, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 0, 0);
    gl.bindVertexArray(null);
    const index = (arr) => {
      const b = gl.createBuffer();
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, b);
      gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, new Uint32Array(arr), gl.STATIC_DRAW);
      return { buffer: b, count: arr.length };
    };
    gl.bindVertexArray(vao);
    const t = index(tris), rd = index(ridges), rb = index(ribs);
    gl.bindVertexArray(null);
    this.mesh = { vao, cols, rows, tris: t, ridges: rd, ribs: rb, buffers: [vbo, t.buffer, rd.buffer, rb.buffer] };
  }

  // Architectural instrument graticule: baseline & rails, front Hz/octave caliper ticks,
  // right-rail 1s ticks, graduated left dB spine with top-right 0 dB caliper bracket,
  // and view-adaptive horizontal dB reference lines (with -12 dB headroom accent).
  buildGuides(seconds = 4) {
    const gl = this.gl;
    const { width: w, height: h, depth: d, tail } = this.look.size;
    const { floor: dbFloor, ceiling: dbCeil } = this.look.db;
    const x0 = -w / 2, x1 = w / 2, zf = d / 2, zEnd = d / 2 - d * tail;
    const yFloor = h * 0.0006;

    const verts = [];
    const slices = {};
    const group = (name, fn) => {
      const start = verts.length / 3;
      fn();
      slices[name] = { start, count: verts.length / 3 - start };
    };
    const seg = (ax, ay, az, bx, by, bz) => { verts.push(ax, ay, az, bx, by, bz); };
    const yAtDb = (db) => ((db - dbFloor) / (dbCeil - dbFloor)) * h;

    group('floorGrid', () => {
      for (const hz of hzTicks(this.look.labels, this.melAxis)) {
        const u = bandToU(hzToBand(hz), this.melAxis);
        if (u < 0 || u > 1) continue;
        const x = (u - 0.5) * w;
        seg(x, yFloor, zf, x, yFloor, zEnd);
      }
      const every = this.look.labels.seconds;
      const totalSec = seconds * tail;
      for (let s = every; s <= totalSec + 1e-6; s += every) {
        const z = zf - (s / seconds) * d;
        seg(x0, yFloor, z, x1, yFloor, z);
      }
    });

    group('sideRails', () => {
      seg(x0, 0, zf, x0, 0, zEnd);
      seg(x1, 0, zf, x1, 0, zEnd);
    });

    group('rails', () => {
      seg(x0, 0, zf, x1, 0, zf);
      for (const hz of hzTicks(this.look.labels, this.melAxis)) {
        const u = bandToU(hzToBand(hz), this.melAxis);
        if (u < 0 || u > 1) continue;
        const x = (u - 0.5) * w;
        seg(x, 0, zf, x, 0, zf + d * 0.022);
      }
      for (let band = 12; band <= 120; band += 12) {
        const x = (bandToU(band, this.melAxis) - 0.5) * w;
        seg(x, 0, zf, x, 0, zf + d * 0.010);
      }
    });

    group('timeTicks', () => {
      const every = this.look.labels.seconds;
      for (let s = 0; s <= seconds + 1e-6; s += every) {
        const z = zf - (s / seconds) * d;
        seg(x1, 0, z, x1 + w * 0.014, 0, z);
      }
    });

    group('a4Tick', () => {
      const x440 = (bandToU(hzToBand(440), this.melAxis) - 0.5) * w;
      seg(x440, 0, zf, x440, 0, zf + d * 0.018);
    });

    group('dbSpine', () => {
      seg(x0, 0, zf, x0, h, zf);
      // Precision optical L-bracket marking 0 dBFS ceiling at right edge
      seg(x1, h, zf, x1 - w * 0.014, h, zf);
      seg(x1, h, zf, x1, h - h * 0.045, zf);
      const hr = this.look.labels.headroomDb;
      for (let db = 0; db >= dbFloor - 1e-6; db -= 12) {
        if (db === hr) continue;
        const y = yAtDb(db);
        const major = db % 24 === 0;
        seg(x0, y, zf, x0 - w * (major ? 0.015 : 0.008), y, zf);
      }
    });

    group('dbAccents', () => {
      const yHr = yAtDb(this.look.labels.headroomDb);
      seg(x0, yHr, zf, x0 - w * 0.016, yHr, zf);
      seg(x1, yHr, zf, x1 + w * 0.010, yHr, zf);
    });

    group('frontGrid', () => {
      for (let db = 0; db > dbFloor + 1e-6; db -= 24) {
        const y = yAtDb(db);
        if (y >= 0 && y <= h) seg(x0, y, zf, x1, y, zf);
      }
    });

    group('headroomLine', () => {
      const yHr = yAtDb(this.look.labels.headroomDb);
      if (yHr >= 0 && yHr <= h) seg(x0, yHr, zf, x1, yHr, zf);
    });

    gl.bindVertexArray(this.guides.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.guides.vbo);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(verts), gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 3, gl.FLOAT, false, 0, 0);
    gl.bindVertexArray(null);
    this.guides.slices = slices;
  }

  // Adds the newest data row. Level l gets a row every 2^l data rows: the power mean of
  // those rows, so a short loud hit still shows far down the tail.
  pushRow(bytes) {
    this.historyStore.push(bytes);
    this.rowsPushed = this.historyStore.rowsPushed;
  }

  flushHistory() {
    const gl = this.gl;
    if (!this.historyStore.dirty.some(Boolean)) return;
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.historyTex);
    this.historyStore.flush((level, row, height, data) => {
      gl.texSubImage2D(gl.TEXTURE_2D, 0, level * this.count, row, this.count, height,
        gl.RED, gl.UNSIGNED_BYTE, data);
    });
  }

  // dB stored at semitone position `band`, `age` data rows back from the newest, from the
  // finest level whose ring still holds that row. Used for readouts.
  sample(band, age) {
    const a = Math.max(0, Math.round(age));
    let l = 0;
    while (l < LEVELS - 1 && a >= (HISTORY_ROWS - 2) * 2 ** l) l++;
    const row = this.rowsPushed - 1 - a;
    if (row < 0) return -120;
    const k = Math.max(0, Math.min(this.count - 1, Math.round(band * this.sub)));
    return this.history[l][(Math.floor(row / 2 ** l) % HISTORY_ROWS) * this.count + k] / 2 - 120;
  }

  setCommon(p, s, invViewProj, vpW, vpH) {
    const gl = this.gl;
    const u = p.uniforms;
    const { size } = this.look;
    gl.uniform1f(u.u_rows, HISTORY_ROWS);
    if (u.u_sub) gl.uniform1f(u.u_sub, this.sub);
    if (u.u_rowScale) gl.uniform1f(u.u_rowScale, (s.rowRate || 60) / 60);
    gl.uniform1f(u.u_newest, (this.rowsPushed - 1 + HISTORY_ROWS * 2 ** (LEVELS - 1)) % (HISTORY_ROWS * 2 ** (LEVELS - 1)));
    gl.uniform1f(u.u_phase, s.phase);
    gl.uniform1f(u.u_spanRows, s.spanRows);
    gl.uniform2f(u.u_dbRange, this.look.db.floor, this.look.db.ceiling);
    gl.uniform3f(u.u_size, size.width, size.height, size.depth);
    gl.uniform1f(u.u_tail, size.tail);
    if (u.u_melAxis) gl.uniform1f(u.u_melAxis, this.melAxis);
    if (u.u_fogStart) gl.uniform1f(u.u_fogStart, this.look.fog.start);
    if (u.u_fogPower) gl.uniform1f(u.u_fogPower, this.look.fog.power);
    gl.uniformMatrix4fv(u.u_viewProj, false, s.viewProj);
    if (u.u_invViewProj && invViewProj) gl.uniformMatrix4fv(u.u_invViewProj, false, invViewProj);
    if (u.u_viewport) gl.uniform2f(u.u_viewport, vpW, vpH);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.historyTex);
    gl.uniform1i(u.u_history, 0);
    gl.activeTexture(gl.TEXTURE1);
    gl.bindTexture(gl.TEXTURE_2D, this.paletteTex);
    if (u.u_palette) gl.uniform1i(u.u_palette, 1);
  }

  // s.layers turns the waterfall and the live front spectrum on or off; s.maxHold and
  // s.keys add the max-hold trace and the piano strip. s.rowRate: data rows a second.
  // s.tilt: dB per octave about 1 kHz added to the front spectrum's drawn level.
  draw(s) {
    this.flushHistory();
    const gl = this.gl;
    const L = this.look;
    const layers = s.layers || { waterfall: true, spectrum: true };
    const stats = this.stats;
    stats.calls = 0;
    stats.vertices = 0;
    const style = L.styles[L.style] || Object.values(L.styles)[0];
    const bg = hexToRgb(L.background);
    const vpW = Math.max(1, gl.drawingBufferWidth);
    const vpH = Math.max(1, gl.drawingBufferHeight);
    const cssW = Math.max(1, gl.canvas?.clientWidth || vpW);
    const dpr = Math.max(1, vpW / cssW);
    const inv = invert(s.viewProj);
    const { topness, frontness, fadeDb, timeVis } = viewMetrics(inv);

    gl.clearColor(bg[0], bg[1], bg[2], 1);
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    gl.enable(gl.DEPTH_TEST);
    gl.depthFunc(gl.LEQUAL);
    gl.depthMask(true);

    // 1. Opaque waterfall terrain (sculpting + analytical anti-aliased slices, octaves & dB contours in one pass).
    if (layers.waterfall) {
      const m = this.mesh;
      const p = this.surface;
      const u = p.uniforms;
      const regTint = L.paletteRegister?.[L.palette] ?? 0.6;
      gl.useProgram(p.program);
      this.setCommon(p, s, inv, vpW, vpH);
      gl.uniform1f(u.u_stepU, 1 / (m.cols - 1));
      gl.uniform1f(u.u_lift, 0);
      gl.uniform3fv(u.u_background, bg);
      gl.uniform3fv(u.u_keyLight, L.surface.keyLight);
      gl.uniform3fv(u.u_rimLight, L.surface.rimLight);
      gl.uniform1f(u.u_ambient, L.surface.ambient);
      gl.uniform1f(u.u_exposure, L.surface.exposure ?? 1);
      gl.uniform1f(u.u_surfaceMix, style.surfaceMix);
      gl.uniform1f(u.u_sliceStrength, style.sliceStrength);
      gl.uniform1f(u.u_sliceEvery, style.sliceEvery);
      gl.uniform1f(u.u_ribStrength, style.ribStrength);
      gl.uniform1f(u.u_ribEvery, style.ribEvery);
      gl.uniform1f(u.u_contourStrength, style.contourStrength);
      gl.uniform1f(u.u_octaveStrength, style.octaveStrength);
      gl.uniform1f(u.u_lineFloor, style.lineFloor);
      gl.uniform1f(u.u_registerTint, regTint);
      gl.uniform1f(u.u_topness, topness);
      gl.uniform1f(u.u_frontness, frontness);

      gl.enable(gl.POLYGON_OFFSET_FILL);
      gl.polygonOffset(1, 1);
      gl.disable(gl.BLEND);
      gl.bindVertexArray(m.vao);
      gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, m.tris.buffer);
      gl.drawElements(gl.TRIANGLES, m.tris.count, gl.UNSIGNED_INT, 0);
      stats.calls++;
      stats.vertices += m.cols * m.rows;
      gl.disable(gl.POLYGON_OFFSET_FILL);
      gl.bindVertexArray(null);
    }

    // 2. Architectural instrument graticule (occluded behind active 3D peaks via depth buffer).
    {
      const p = this.lines;
      const u = p.uniforms;
      const G = L.guides;
      const sl = this.guides.slices;
      const drawSlice = (slice, hex, alpha) => {
        if (!slice || slice.count <= 0 || alpha <= 0.002) return;
        gl.uniform3fv(u.u_color, hexToRgb(hex));
        gl.uniform1f(u.u_alpha, alpha);
        gl.drawArrays(gl.LINES, slice.start, slice.count);
        stats.calls++;
        stats.vertices += slice.count;
      };

      gl.useProgram(p.program);
      this.setCommon(p, s, inv, vpW, vpH);
      gl.enable(gl.BLEND);
      gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
      gl.depthMask(false);
      gl.bindVertexArray(this.guides.vao);

      const floorAlpha = (G.floorAlpha3d + (G.floorAlphaTop - G.floorAlpha3d) * topness) * (1 - frontness);
      if (layers.waterfall) {
        drawSlice(sl.floorGrid, G.floorColor, floorAlpha);
        drawSlice(sl.sideRails, G.railColor, G.railAlpha);
        drawSlice(sl.timeTicks, G.railColor, G.timeTickAlpha * timeVis);
      }
      drawSlice(sl.rails, G.railColor, G.railAlpha);
      drawSlice(sl.a4Tick, G.accentColor, G.accentAlpha);
      drawSlice(sl.dbSpine, G.spineColor, G.spineAlpha * fadeDb);
      drawSlice(sl.dbAccents, G.accentColor, G.accentAlpha * fadeDb);
      drawSlice(sl.frontGrid, G.frontGridColor, G.frontGridAlpha * frontness);
      drawSlice(sl.headroomLine, G.accentColor, G.headroomLineAlpha * frontness);

      gl.bindVertexArray(null);
    }

    // 3. Live leading-edge spectrum: premultiplied curtain + anti-aliased trace, peak hold, core 1px edge.
    if (layers.spectrum) {
      const p = this.front;
      const u = p.uniforms;
      const F = L.front;
      const { count } = this;

      gl.useProgram(p.program);
      this.setCommon(p, s, inv, vpW, vpH);
      gl.uniform1f(u.u_lift, L.size.height * 0.0025);
      gl.uniform1f(u.u_tilt, s.tilt || 0);
      gl.uniform1f(u.u_dpr, dpr);
      gl.uniform3fv(u.u_traceColor, hexToRgb(F.trace));
      gl.uniform3fv(u.u_peakColor, hexToRgb(F.peak));
      gl.uniform3fv(u.u_headroomColor, hexToRgb(F.headroom));
      gl.uniform3fv(u.u_maxColor, hexToRgb(F.maxHold));
      gl.uniform1f(u.u_exposure, L.surface.exposure ?? 1);
      gl.uniform1f(u.u_frontness, frontness);
      gl.uniform1f(u.u_topness, topness);

      gl.enable(gl.BLEND);
      gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
      gl.depthMask(false);

      const drawFront = (mode, buffer, primitive, n) => {
        gl.uniform1f(u.u_mode, mode);
        gl.bindVertexArray(buffer.vao);
        gl.drawArrays(primitive, 0, n);
        stats.calls++;
        stats.vertices += n;
      };
      drawFront(0, this.curtain, gl.TRIANGLE_STRIP, count * 2);
      if (topness < 0.98) {
        if (s.maxHold) drawFront(3, this.maxLine, gl.LINE_STRIP, count);
        drawFront(1, this.peakLine, gl.LINE_STRIP, count);
      }
      drawFront(2, this.frontLine, gl.LINE_STRIP, count);

      gl.depthMask(true);
      gl.bindVertexArray(null);
    }

    // 4. Piano strip below the front edge. Depth-tested without writing depth: from the front
    // nothing of the box sits there, from behind the terrain hides it.
    if (s.keys) {
      const K = L.keys;
      const p = this.keyProgram;
      const u = p.uniforms;
      gl.useProgram(p.program);
      this.setCommon(p, s, inv, vpW, vpH);
      gl.uniform1f(u.u_keyGapPx, s.keys.gapPx * dpr);
      gl.uniform1f(u.u_keyHeightPx, s.keys.heightPx * dpr);
      gl.uniform1f(u.u_hoverBand, s.keys.hoverBand ?? -10);
      gl.uniform3fv(u.u_whiteKey, hexToRgb(K.white));
      gl.uniform3fv(u.u_blackKey, hexToRgb(K.black));
      gl.uniform3fv(u.u_litKey, hexToRgb(K.lit));
      gl.uniform3fv(u.u_hoverKey, hexToRgb(K.hover));
      gl.uniform3fv(u.u_octaveEdge, hexToRgb(K.octave));
      gl.uniform1f(u.u_alpha, K.alpha);
      gl.depthMask(false);
      gl.enable(gl.BLEND);
      gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA);
      gl.bindVertexArray(this.keys.vao);
      gl.drawArrays(gl.TRIANGLES, 0, BANDS * 6);
      stats.calls++;
      stats.vertices += BANDS * 6;
      gl.bindVertexArray(null);
      gl.depthMask(true);
    }
  }

  // Release resources owned by a renderer after a style switch. CPU history may be shared.
  dispose() {
    const gl = this.gl, seen = new Set();
    const visit = value => {
      if (!value || typeof value !== 'object' || seen.has(value)) return;
      seen.add(value);
      if (value instanceof WebGLBuffer) { gl.deleteBuffer(value); return; }
      if (value instanceof WebGLVertexArrayObject) { gl.deleteVertexArray(value); return; }
      if (value instanceof WebGLTexture) { gl.deleteTexture(value); return; }
      if (value instanceof WebGLProgram) { gl.deleteProgram(value); return; }
      if (ArrayBuffer.isView(value)) return;
      if (Array.isArray(value) || Object.getPrototypeOf(value) === Object.prototype) for (const item of Object.values(value)) visit(item);
    };
    for (const [key, value] of Object.entries(this)) if (!['gl', 'look', 'historyStore', 'history'].includes(key)) visit(value);
  }

  // Bytes this renderer has allocated on the GPU: textures and buffers, not the canvas.
  get gpuBytes() {
    const m = this.mesh;
    const mesh = m ? m.cols * m.rows * 12 + (m.tris.count + m.ridges.count + m.ribs.count) * 4 : 0;
    const front = [this.frontLine, this.peakLine, this.curtain, this.maxLine].reduce((n, b) => n + b.data.byteLength, 0);
    return this.count * LEVELS * HISTORY_ROWS + 256 * 4 + mesh + front + this.keys.data.byteLength;
  }
}
