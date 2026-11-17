// WebGL2 renderer for the Dadaspec 3D visual style.
// Extends the shared Renderer with a real-time 6-level tri-band perceptual chroma pyramid
// (8 x 4096 RGBA8 ring texture, 128 KiB VRAM) matching dadabots/dadaspec's per-band normalized
// RGB model (<200 Hz Low, 200-1500 Hz Mid, >=1500 Hz High + rate-invariant onset seam flux)
// without fragment shader loops, and backfills seamlessly from shared CPU history on style switch.

import { Renderer as BaseRenderer, HISTORY_ROWS, LEVELS, viewMetrics } from '../../src/renderer.js';
import { loadShader, hexToRgb } from '../../src/gl.js';
import { BANDS, bandHz, hzToBand, bandToU, hzTicks } from '../../src/bands.js';
import { invert } from '../../src/mat4.js';

const CHROMA_WIDTH = 8; // Power-of-two width >= LEVELS (6) for exact dyadic UV centres on mobile GPUs
const POWER = Float32Array.from({ length: 256 }, (_, b) => 10 ** ((b / 2 - 120) / 10));
const TILT_DB = [0, 5.5, 11.5]; // Natural acoustic tilt offset for cross-band ceiling coupling

export class Renderer extends BaseRenderer {
  static async create(gl, look) {
    const shared = ['terrain.vert', 'grid.frag', 'front.vert', 'line.vert', 'line.frag', 'keys.vert'];
    const custom = ['surface.frag', 'front.frag', 'keys.frag'];
    const entries = await Promise.all([
      ...shared.map(async (n) => [n, await loadShader(n)]),
      ...custom.map(async (n) => [n, await loadShader(n, 'styles/dadaspec/shaders')]),
    ]);
    return new Renderer(gl, look, Object.fromEntries(entries));
  }

  setBands(sub) {
    super.setBands(sub);
    this.initChroma(sub);
  }

  initChroma(sub) {
    const gl = this.gl;
    if (!this.chromaTex) this.chromaTex = gl.createTexture();
    gl.activeTexture(gl.TEXTURE2);
    gl.bindTexture(gl.TEXTURE_2D, this.chromaTex);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    gl.texImage2D(
      gl.TEXTURE_2D, 0, gl.RGBA8, CHROMA_WIDTH, HISTORY_ROWS, 0,
      gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array(CHROMA_WIDTH * HISTORY_ROWS * 4),
    );
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.REPEAT);
    gl.activeTexture(gl.TEXTURE0);

    const count = this.count;
    const [loSplit = 200, hiSplit = 1500] = this.look?.crossovers || [200, 1500];
    this.zoneMap = new Uint8Array(count);
    this.zoneWeight = new Float32Array(count);
    this.zoneDenom = new Float64Array(3);
    for (let i = 0; i < count; i++) {
      const hz = bandHz(i / sub);
      const z = hz < loSplit ? 0 : hz < hiSplit ? 1 : 2;
      const w = hz > 16500 ? 0.30 : 1.0;
      this.zoneMap[i] = z;
      this.zoneWeight[i] = w;
      this.zoneDenom[z] += w;
    }
    for (let z = 0; z < 3; z++) this.zoneDenom[z] = Math.max(1e-6, this.zoneDenom[z]);

    this.chromaLevels = Array.from({ length: LEVELS }, () => new Uint8Array(HISTORY_ROWS * 4));
    this.chromaDirty = new Uint32Array(LEVELS);
    this.bandPowerSums = Array.from({ length: LEVELS }, () => new Float64Array(3));
    this.fluxPeaks = new Float32Array(LEVELS);
    this.bandCeil = new Float32Array([-18, -23.5, -29.5]);
    this.fluxEnvDb = -120;
    this.liveChroma = new Float32Array([0, 0, 0, 0]);
    this.lastRowRate = this.lastRowRate || 60;
    this.syncedStore = this.historyStore;
  }

  // Computes per-band normalized RGB weights [0..1] from zone mean linear powers.
  // Cross-band ceiling coupling prevents trace residual energy in an unexcited band from
  // inflating to 1.0 during warm/dark chords, while still normalizing quiet passages cleanly.
  computeTriadRgb(m0, m1, m2, advanceEnvelope, dtSec) {
    const dbs = [
      10 * Math.log10(m0 + 1e-30),
      10 * Math.log10(m1 + 1e-30),
      10 * Math.log10(m2 + 1e-30),
    ];
    const floor = Math.min(-96, this.look?.db?.floor ?? -96);
    const gates = [floor + 6, floor + 4, floor + 2];
    const minSpan = 26;

    if (advanceEnvelope) {
      const attack = 1 - Math.exp(-dtSec / 0.035);
      const release = 1 - Math.exp(-dtSec / 9.0);
      for (let z = 0; z < 3; z++) {
        const db = dbs[z];
        if (db > gates[z] + 2) {
          const target = Math.max(gates[z] + minSpan, db);
          this.bandCeil[z] += (target - this.bandCeil[z]) * (target > this.bandCeil[z] ? attack : release);
        }
      }
    }

    // Anchor each band's effective ceiling within 10 dB (tilt-adjusted) of the loudest active band,
    // using a 36 dB perceptual chroma window (with instant quiet-passage fallback right after loud drops).
    const refPeak = Math.max(
      this.bandCeil[0] + TILT_DB[0],
      this.bandCeil[1] + TILT_DB[1],
      this.bandCeil[2] + TILT_DB[2],
    );
    const livePeak = Math.max(dbs[0] + TILT_DB[0], dbs[1] + TILT_DB[1], dbs[2] + TILT_DB[2]);
    const activeRef = livePeak < refPeak - 32.0 ? livePeak + 4.0 : refPeak;

    let n0 = 0, n1 = 0, n2 = 0;
    for (let z = 0; z < 3; z++) {
      const coupledCeil = Math.max(gates[z] + 18.0, Math.min(this.bandCeil[z], activeRef - TILT_DB[z]), activeRef - TILT_DB[z] - 10.0);
      const bandFloor = Math.max(gates[z], coupledCeil - 36.0);
      const span = Math.max(18.0, coupledCeil - bandFloor);
      const v = Math.max(0, Math.min(1, (dbs[z] - bandFloor) / span));
      if (z === 0) n0 = v;
      else if (z === 1) n1 = v;
      else n2 = v;
    }
    const mx = Math.max(n0, n1, n2);
    if (mx <= 1e-3) return [0, 0, 0];
    return [n0 / mx, n1 / mx, n2 / mx];
  }

  // Row-rate-invariant transient flux: compares instantaneous broadband dB against a 40 ms
  // exponential follower so 50, 60, 120, 240, and 480 rows/s yield identical onset strength.
  computeOnsetFlux(totalDb, dtSec) {
    if (this.fluxEnvDb <= -118) {
      this.fluxEnvDb = totalDb;
      return totalDb > -85 ? 0.65 : 0;
    }
    const rise = totalDb - this.fluxEnvDb;
    const flux = Math.max(0, Math.min(1, (rise - 1.6) / 11.0));
    const tau = totalDb > this.fluxEnvDb ? 0.042 : 0.095;
    this.fluxEnvDb += (totalDb - this.fluxEnvDb) * (1 - Math.exp(-dtSec / tau));
    return flux;
  }

  measureRowPowers(bytes, offset = 0, stride = 1) {
    let p0 = 0, p1 = 0, p2 = 0;
    const { count, zoneMap, zoneWeight } = this;
    for (let i = 0; i < count; i += stride) {
      const p = POWER[bytes[offset + i]] * zoneWeight[i];
      const z = zoneMap[i];
      if (z === 0) p0 += p;
      else if (z === 1) p1 += p;
      else p2 += p;
    }
    const scale = stride;
    return [
      (p0 * scale) / this.zoneDenom[0],
      (p1 * scale) / this.zoneDenom[1],
      (p2 * scale) / this.zoneDenom[2],
    ];
  }

  pushRow(bytes) {
    if (this.syncedStore !== this.historyStore) this.syncFromHistoryStore();
    const rowIndex = this.historyStore.rowsPushed;
    super.pushRow(bytes);

    const [m0, m1, m2] = this.measureRowPowers(bytes, 0, 1);
    const dtSec = 1 / Math.max(30, this.lastRowRate || 60);
    const [r0, g0, b0] = this.computeTriadRgb(m0, m1, m2, true, dtSec);

    const totalMean = (m0 * this.zoneDenom[0] + m1 * this.zoneDenom[1] + m2 * this.zoneDenom[2])
      / (this.zoneDenom[0] + this.zoneDenom[1] + this.zoneDenom[2]);
    const totalDb = 10 * Math.log10(totalMean + 1e-30);
    const flux = this.computeOnsetFlux(totalDb, dtSec);

    const base0 = (rowIndex % HISTORY_ROWS) * 4;
    const l0 = this.chromaLevels[0];
    l0[base0] = Math.round(r0 * 255);
    l0[base0 + 1] = Math.round(g0 * 255);
    l0[base0 + 2] = Math.round(b0 * 255);
    l0[base0 + 3] = Math.round(flux * 255);
    this.chromaDirty[0]++;
    this.liveChroma[0] = r0;
    this.liveChroma[1] = g0;
    this.liveChroma[2] = b0;
    this.liveChroma[3] = flux;

    for (let l = 1; l < LEVELS; l++) {
      const sums = this.bandPowerSums[l];
      const span = 2 ** l;
      sums[0] += m0;
      sums[1] += m1;
      sums[2] += m2;
      if (flux > this.fluxPeaks[l]) this.fluxPeaks[l] = flux;
      if ((rowIndex + 1) % span) continue;

      const [rl, glCol, bl] = this.computeTriadRgb(sums[0] / span, sums[1] / span, sums[2] / span, false, 0);
      const basel = (Math.floor(rowIndex / span) % HISTORY_ROWS) * 4;
      const dst = this.chromaLevels[l];
      dst[basel] = Math.round(rl * 255);
      dst[basel + 1] = Math.round(glCol * 255);
      dst[basel + 2] = Math.round(bl * 255);
      dst[basel + 3] = Math.round(this.fluxPeaks[l] * 255);
      sums[0] = 0; sums[1] = 0; sums[2] = 0;
      this.fluxPeaks[l] = 0;
      this.chromaDirty[l]++;
    }
  }

  // Rebuilds all 6 chroma pyramid levels when adopting an existing CPU History on style switch.
  syncFromHistoryStore() {
    const store = this.historyStore;
    this.syncedStore = store;
    for (let l = 0; l < LEVELS; l++) {
      this.chromaLevels[l].fill(0);
      this.bandPowerSums[l].fill(0);
      this.fluxPeaks[l] = 0;
      this.chromaDirty[l] = 0;
    }
    this.bandCeil.set([-18, -23.5, -29.5]);
    this.fluxEnvDb = -120;
    this.liveChroma.fill(0);
    if (!store || store.rowsPushed <= 0) return;

    const stride = Math.max(1, this.sub);
    const baseDt = 1 / Math.max(30, this.lastRowRate || 60);

    for (let l = LEVELS - 1; l >= 0; l--) {
      const span = 2 ** l;
      const totalBlocks = Math.floor(store.rowsPushed / span);
      const validBlocks = Math.min(HISTORY_ROWS, totalBlocks);
      if (validBlocks <= 0) continue;

      const src = store.history[l];
      const dst = this.chromaLevels[l];
      let localEnvDb = -120;
      const dtBlock = baseDt * span;
      const startBlock = totalBlocks - validBlocks;
      for (let b = startBlock; b < totalBlocks; b++) {
        const ringRow = b % HISTORY_ROWS;
        const [m0, m1, m2] = this.measureRowPowers(src, ringRow * this.count, stride);
        const [r, g, bl] = this.computeTriadRgb(m0, m1, m2, l === 0, dtBlock);
        const totalMean = (m0 * this.zoneDenom[0] + m1 * this.zoneDenom[1] + m2 * this.zoneDenom[2])
          / (this.zoneDenom[0] + this.zoneDenom[1] + this.zoneDenom[2]);
        const totalDb = 10 * Math.log10(totalMean + 1e-30);
        let flux = 0;
        if (b === startBlock) {
          localEnvDb = totalDb;
        } else {
          flux = Math.max(0, Math.min(1, (totalDb - localEnvDb - 1.6) / 11.0));
          const tau = totalDb > localEnvDb ? 0.042 : 0.095;
          localEnvDb += (totalDb - localEnvDb) * (1 - Math.exp(-dtBlock / tau));
        }

        const o = ringRow * 4;
        dst[o] = Math.round(r * 255);
        dst[o + 1] = Math.round(g * 255);
        dst[o + 2] = Math.round(bl * 255);
        dst[o + 3] = Math.round(flux * 255);

        if (l === 0 && b === totalBlocks - 1) {
          this.fluxEnvDb = localEnvDb;
          this.liveChroma[0] = r;
          this.liveChroma[1] = g;
          this.liveChroma[2] = bl;
          this.liveChroma[3] = flux;
        }
      }
      this.chromaDirty[l] = HISTORY_ROWS;
    }

    for (let l = 1; l < LEVELS; l++) {
      const span = 2 ** l;
      const rem = store.rowsPushed % span;
      if (!rem) continue;
      const src0 = store.history[0];
      const sums = this.bandPowerSums[l];
      for (let r = store.rowsPushed - rem; r < store.rowsPushed; r++) {
        const ringRow = r % HISTORY_ROWS;
        const [m0, m1, m2] = this.measureRowPowers(src0, ringRow * this.count, stride);
        sums[0] += m0;
        sums[1] += m1;
        sums[2] += m2;
      }
    }
  }

  flushHistory() {
    if (this.syncedStore !== this.historyStore) this.syncFromHistoryStore();
    super.flushHistory();

    if (!this.chromaDirty.some(Boolean)) return;
    const gl = this.gl;
    gl.activeTexture(gl.TEXTURE2);
    gl.bindTexture(gl.TEXTURE_2D, this.chromaTex);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    const pushed = this.historyStore.rowsPushed;
    for (let l = 0; l < LEVELS; l++) {
      const dirty = Math.min(HISTORY_ROWS, this.chromaDirty[l]);
      if (!dirty) continue;
      const end = Math.floor(pushed / 2 ** l) % HISTORY_ROWS;
      const start = (end - dirty + HISTORY_ROWS) % HISTORY_ROWS;
      const first = Math.min(dirty, HISTORY_ROWS - start);
      const buf = this.chromaLevels[l];
      gl.texSubImage2D(gl.TEXTURE_2D, 0, l, start, 1, first, gl.RGBA, gl.UNSIGNED_BYTE,
        buf.subarray(start * 4, (start + first) * 4));
      if (first < dirty) {
        const rest = dirty - first;
        gl.texSubImage2D(gl.TEXTURE_2D, 0, l, 0, 1, rest, gl.RGBA, gl.UNSIGNED_BYTE,
          buf.subarray(0, rest * 4));
      }
      this.chromaDirty[l] = 0;
    }
    gl.activeTexture(gl.TEXTURE0);
  }

  // Extends shared instrument graticule with Dadaspec's 200 Hz and 1.5 kHz acoustic crossover markers.
  buildGuides(seconds = 4) {
    super.buildGuides(seconds);
    const gl = this.gl;
    const { width: w, height: h, depth: d, tail } = this.look.size;
    const { floor: dbFloor, ceiling: dbCeil } = this.look.db;
    const x0 = -w / 2, x1 = w / 2, zf = d / 2, zEnd = d / 2 - d * tail;
    const yFloor = h * 0.0006;
    const [loHz = 200, hiHz = 1500] = this.look.crossovers || [200, 1500];

    const verts = [];
    const slices = {};
    const group = (name, fn) => {
      const start = verts.length / 3;
      fn();
      slices[name] = { start, count: verts.length / 3 - start };
    };
    const seg = (ax, ay, az, bx, by, bz) => { verts.push(ax, ay, az, bx, by, bz); };
    const yAtDb = (db) => ((db - dbFloor) / (dbCeil - dbFloor)) * h;
    const xAtHz = (hz) => (bandToU(hzToBand(hz), this.melAxis) - 0.5) * w;

    group('floorGrid', () => {
      for (const hz of hzTicks(this.look.labels, this.melAxis)) {
        if (hz === loHz || hz === hiHz) continue;
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

    group('xoverLow', () => {
      const x = xAtHz(loHz);
      seg(x, 0, zf - d * 0.012, x, 0, zf + d * 0.026);
      seg(x, 0, zf, x, h * 0.032, zf);
    });

    group('xoverHigh', () => {
      const x = xAtHz(hiHz);
      seg(x, 0, zf - d * 0.012, x, 0, zf + d * 0.026);
      seg(x, 0, zf, x, h * 0.032, zf);
    });

    group('xoverFloorLow', () => {
      const x = xAtHz(loHz);
      seg(x, yFloor, zf, x, yFloor, zEnd);
    });

    group('xoverFloorHigh', () => {
      const x = xAtHz(hiHz);
      seg(x, yFloor, zf, x, yFloor, zEnd);
    });

    group('xoverFrontLow', () => {
      const x = xAtHz(loHz);
      seg(x, 0, zf, x, h, zf);
    });

    group('xoverFrontHigh', () => {
      const x = xAtHz(hiHz);
      seg(x, 0, zf, x, h, zf);
    });

    group('timeTicks', () => {
      const every = this.look.labels.seconds;
      for (let s = 0; s <= seconds + 1e-6; s += every) {
        const z = zf - (s / seconds) * d;
        seg(x1, 0, z, x1 + w * 0.014, 0, z);
      }
    });

    group('a4Tick', () => {
      const x440 = xAtHz(440);
      seg(x440, 0, zf, x440, 0, zf + d * 0.018);
    });

    group('dbSpine', () => {
      seg(x0, 0, zf, x0, h, zf);
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

  setTriadUniforms(p, triad) {
    const gl = this.gl;
    const u = p.uniforms;
    if (u.u_lowColor) gl.uniform3fv(u.u_lowColor, hexToRgb(triad.low));
    if (u.u_midColor) gl.uniform3fv(u.u_midColor, hexToRgb(triad.mid));
    if (u.u_highColor) gl.uniform3fv(u.u_highColor, hexToRgb(triad.high));
    if (u.u_whiteColor) gl.uniform3fv(u.u_whiteColor, hexToRgb(triad.white));
    if (u.u_chromaSat) gl.uniform1f(u.u_chromaSat, triad.chromaSat ?? 1.0);
    if (u.u_dispersion) gl.uniform1f(u.u_dispersion, triad.dispersion ?? 0.34);
  }

  draw(s) {
    this.lastRowRate = s.rowRate || 60;
    this.flushHistory();

    const gl = this.gl;
    const L = this.look;
    const layers = s.layers || { waterfall: true, spectrum: true };
    const stats = this.stats;
    stats.calls = 0;
    stats.vertices = 0;

    const style = L.styles[L.style] || Object.values(L.styles)[0];
    const triad = L.triad;
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

    // 1. Opaque Dadaspec waterfall terrain.
    if (layers.waterfall) {
      const m = this.mesh;
      const p = this.surface;
      const u = p.uniforms;
      gl.useProgram(p.program);
      this.setCommon(p, s, inv, vpW, vpH);
      this.setTriadUniforms(p, triad);
      gl.activeTexture(gl.TEXTURE2);
      gl.bindTexture(gl.TEXTURE_2D, this.chromaTex);
      if (u.u_chroma) gl.uniform1i(u.u_chroma, 2);

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
      gl.uniform1f(u.u_channelEtch, style.channelEtch ?? 0.14);
      gl.uniform1f(u.u_seamBoost, style.seamBoost ?? 0.55);
      gl.uniform1f(u.u_contourStrength, style.contourStrength);
      gl.uniform1f(u.u_octaveStrength, style.octaveStrength);
      gl.uniform1f(u.u_crossoverStrength, style.crossoverStrength ?? 0.46);
      gl.uniform1f(u.u_lineFloor, style.lineFloor);
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

    // 2. Precision instrument graticule + 200 Hz / 1.5 kHz crossover markers.
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

      const mixHex = (a, b) => {
        const ca = hexToRgb(a), cb = hexToRgb(b);
        const m = [ca[0] + cb[0], ca[1] + cb[1], ca[2] + cb[2]];
        const pk = Math.max(m[0], m[1], m[2], 1e-4);
        const sat = triad.chromaSat ?? 1.0;
        const w = hexToRgb(triad.white);
        return m.map((c, i) => {
          const n = c / pk;
          const v = Math.round(Math.min(1, Math.max(0, w[i] * (1 - sat * 0.75) + n * sat * 0.75)) * 255);
          return v.toString(16).padStart(2, '0');
        }).reduce((acc, h) => acc + h, '#');
      };
      const lowMidHex = mixHex(triad.low, triad.mid);
      const midHighHex = mixHex(triad.mid, triad.high);

      gl.useProgram(p.program);
      this.setCommon(p, s, inv, vpW, vpH);
      gl.enable(gl.BLEND);
      gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
      gl.depthMask(false);
      gl.bindVertexArray(this.guides.vao);

      const floorAlpha = (G.floorAlpha3d + (G.floorAlphaTop - G.floorAlpha3d) * topness) * (1 - frontness);
      if (layers.waterfall) {
        drawSlice(sl.floorGrid, G.floorColor, floorAlpha);
        drawSlice(sl.xoverFloorLow, lowMidHex, floorAlpha * 1.65);
        drawSlice(sl.xoverFloorHigh, midHighHex, floorAlpha * 1.65);
        drawSlice(sl.sideRails, G.railColor, G.railAlpha);
        drawSlice(sl.timeTicks, G.railColor, G.timeTickAlpha * timeVis);
      }
      drawSlice(sl.rails, G.railColor, G.railAlpha);
      drawSlice(sl.xoverLow, lowMidHex, G.crossoverTickAlpha ?? 0.50);
      drawSlice(sl.xoverHigh, midHighHex, G.crossoverTickAlpha ?? 0.50);
      drawSlice(sl.a4Tick, G.accentColor, G.accentAlpha);
      drawSlice(sl.dbSpine, G.spineColor, G.spineAlpha * fadeDb);
      drawSlice(sl.dbAccents, G.accentColor, G.accentAlpha * fadeDb);
      drawSlice(sl.frontGrid, G.frontGridColor, G.frontGridAlpha * frontness);
      drawSlice(sl.xoverFrontLow, lowMidHex, (G.crossoverLineAlpha ?? 0.11) * frontness);
      drawSlice(sl.xoverFrontHigh, midHighHex, (G.crossoverLineAlpha ?? 0.11) * frontness);
      drawSlice(sl.headroomLine, G.accentColor, G.headroomLineAlpha * frontness);

      gl.bindVertexArray(null);
    }

    // 3. Live leading-edge spectrum (curtain + chalk-white trace + register-tinted peak hold + max hold).
    if (layers.spectrum) {
      const p = this.front;
      const u = p.uniforms;
      const F = L.front;
      const { count } = this;

      gl.useProgram(p.program);
      this.setCommon(p, s, inv, vpW, vpH);
      this.setTriadUniforms(p, triad);
      if (u.u_liveChroma) gl.uniform4fv(u.u_liveChroma, this.liveChroma);
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

    // 4. Piano strip with zone-aware peak illumination.
    if (s.keys) {
      const K = L.keys;
      const p = this.keyProgram;
      const u = p.uniforms;
      gl.useProgram(p.program);
      this.setCommon(p, s, inv, vpW, vpH);
      this.setTriadUniforms(p, triad);
      gl.uniform1f(u.u_keyGapPx, s.keys.gapPx * dpr);
      gl.uniform1f(u.u_keyHeightPx, s.keys.heightPx * dpr);
      gl.uniform1f(u.u_hoverBand, s.keys.hoverBand ?? -10);
      gl.uniform3fv(u.u_whiteKey, hexToRgb(K.white));
      gl.uniform3fv(u.u_blackKey, hexToRgb(K.black));
      gl.uniform3fv(u.u_litKey, hexToRgb(K.lit));
      gl.uniform3fv(u.u_hoverKey, hexToRgb(K.hover));
      gl.uniform3fv(u.u_octaveEdge, hexToRgb(K.octave));
      gl.uniform1f(u.u_alpha, K.alpha);
      gl.depthMask(false);  // depth-tested, so the terrain hides the strip from behind
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

  get gpuBytes() {
    return super.gpuBytes + CHROMA_WIDTH * HISTORY_ROWS * 4;
  }
}
