// Hardware and timing telemetry deck. Browsers do not let a page read machine-wide CPU or
// GPU load, so everything here is this page's own share: frame pacing, main-thread phases,
// GPU render time (where EXT_disjoint_timer_query_webgl2 exists) and WebGL allocations.
// Hierarchy follows one hero chart (10 s frame-interval histogram against frame budget)
// paired with heterogeneous secondary gauges (GPU budget bar, CPU phase split bar, memory
// history chart, aligned audio table). The groups sit in as few rows as the width allows,
// each row filling the width, as the desktop app's monitor (rust/spectrum-wgpu/src/monitor.rs).

import { t } from './i18n.js';

const WINDOW_S = 4;        // frame pacing over the last few seconds
const LONG_FRAME_S = 10;   // count main-thread stalls over this long
const MAX_QUERIES = 8;     // GPU timer queries in flight
const SAMPLE_S = 0.1;      // one chart sample per 100 ms
const HISTORY = 100;       // ten seconds of samples
const COLUMN_GAP = 25;     // 12 px either side of the hairline between groups
const MEM_SAMPLE_S = 1;    // one memory reading a second
export const MEM_WINDOW_S = 120;  // the memory chart's span
export const MEM_GAP_S = 2.5;     // readings further apart are not joined

function shortGpuName(name) {
  // "ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)" -> "Apple M2"
  const angle = name.match(/^ANGLE \((.*)\)$/);
  if (!angle) return name;
  const parts = angle[1].split(', ');
  const device = parts[1] || parts[0];
  return device.replace(/^ANGLE \w+ Renderer: /, '').replace(/ Direct3D.*$/, '').replace(/ \(0x[0-9A-F]+\)/i, '');
}

const mb = (bytes) => (bytes / 1048576).toFixed(bytes < 10485760 ? 1 : 0);
const ms = (v) => v.toFixed(v < 10 ? 2 : 1);

// Ring of chart samples; `pending` collects the worst value inside the current sample.
class Series {
  constructor() { this.values = []; this.pending = null; }
  note(v) { this.pending = this.pending === null ? v : Math.max(this.pending, v); }
  commit(fallback = null) {
    const v = this.pending ?? fallback;
    this.pending = null;
    if (v === null) return;
    this.values.push(v);
    if (this.values.length > HISTORY) this.values.shift();
  }
}

// Splits `widths.length` groups, in order, into the fewest rows whose groups fit in `room`
// side by side, `gap` apart; earlier rows take the extra group (five go three over two).
// Returns how many groups each row has. The desktop monitor packs the same way.
export function pack(widths, room, gap) {
  const n = widths.length;
  for (let rows = 1; rows <= n; rows++) {
    const counts = Array.from({ length: rows }, (_, r) => Math.floor(n / rows) + (r < n % rows ? 1 : 0));
    let start = 0;
    const fits = counts.every((count) => {
      const row = widths.slice(start, start + count);
      start += count;
      return row.reduce((a, b) => a + b, 0) + gap * (count - 1) <= room;
    });
    if (fits) return counts;
  }
  return widths.map(() => 1);
}

// The smallest of 1, 1.5, 2, 3, 4, 5, 6, 8 times a power of ten at or above `value`.
export function niceCeiling(value) {
  if (!(value > 1)) return 1;
  let power = 10 ** Math.floor(Math.log10(value));
  for (;;) {
    for (const step of [1, 1.5, 2, 3, 4, 5, 6, 8]) {
      if (step * power >= value * (1 - 1e-9)) return step * power;
    }
    power *= 10;
  }
}

// Sizes a chart's drawing buffer to its laid-out size; null while it is not laid out.
function fitCanvas(canvas) {
  const w = canvas.clientWidth, h = canvas.clientHeight;
  if (!w || !h) return null;
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  const bw = Math.round(w * dpr), bh = Math.round(h * dpr);
  if (canvas.width !== bw || canvas.height !== bh) {
    canvas.width = bw;
    canvas.height = bh;
  }
  const g = canvas.getContext('2d');
  g.setTransform(bw / w, 0, 0, bh / h, 0, 0);
  g.clearRect(0, 0, w, h);
  return { g, w, h };
}

// A figure on a chart, on a patch of the chart's own background so lines stay behind it.
function chartLabel(g, text, x, y, baseline) {
  g.font = '9px ui-monospace, "SF Mono", Menlo, Consolas, monospace';
  const tw = g.measureText(text).width;
  const top = baseline === 'top' ? y : y - 10;
  g.fillStyle = 'rgba(7, 8, 10, 0.82)';
  g.fillRect(x - 2, top, tw + 4, 10);
  g.fillStyle = 'rgba(142, 151, 164, 0.78)';
  g.fillText(text, x, top + 8);
}

function drawFrameChart(canvas, values, budget) {
  const fitted = fitCanvas(canvas);
  if (!fitted) return;
  const { g, w: HERO_W, h: HERO_H } = fitted;

  // Baseline and 5 s / 10 s subtle calibration ticks
  g.fillStyle = 'rgba(255, 255, 255, 0.07)';
  g.fillRect(0, HERO_H - 1, HERO_W, 1);
  g.fillRect(0, HERO_H - 3, 1, 2);
  g.fillRect(Math.round(HERO_W / 2), HERO_H - 3, 1, 2);
  g.fillRect(HERO_W - 1, HERO_H - 3, 1, 2);

  if (!values.length) return;
  const top = Math.max(budget * 1.45, ...values) * 1.06;
  const plotH = HERO_H - 4;
  const step = HERO_W / HISTORY;
  const barW = Math.max(1, step - 0.55);
  const lateLimit = budget * 1.25;

  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    const h = Math.max(1, Math.min(plotH, (v / top) * plotH));
    const x = HERO_W - (values.length - i) * step;
    g.fillStyle = v > lateLimit ? 'rgba(232, 162, 70, 0.94)' : 'rgba(142, 151, 164, 0.56)';
    g.fillRect(x, HERO_H - 1 - h, barW, h);
  }

  // Frame budget dashed reference line
  const yBudget = Math.max(2, Math.min(HERO_H - 3, Math.round(HERO_H - 1 - (budget / top) * plotH)));
  g.fillStyle = 'rgba(232, 162, 70, 0.44)';
  for (let x = 0; x < HERO_W; x += 4) g.fillRect(x, yBudget, 2, 1);

  // Quiet budget ms tag in top-left with dark knock-out so bars underneath stay legible
  const label = `${budget.toFixed(1)}\u202fms`;
  g.font = '9px ui-monospace, "SF Mono", Menlo, Consolas, monospace';
  const tw = g.measureText(label).width;
  g.fillStyle = 'rgba(7, 8, 10, 0.82)';
  g.fillRect(3, 2, tw + 4, 10);
  g.fillStyle = 'rgba(142, 151, 164, 0.78)';
  g.fillText(label, 5, 10);
}

// The last two minutes of `samples` ([seconds, MB], oldest first), newest on the right,
// from zero to a round top a tenth above the highest reading in the window; the top only
// moves when a reading passes it or the last high one has scrolled out.
function drawMemoryChart(canvas, samples, now) {
  const fitted = fitCanvas(canvas);
  if (!fitted) return;
  const { g, w, h } = fitted;
  const top = niceCeiling(samples.reduce((m, s) => Math.max(m, s[1]), 0) * 1.1);
  const x = (t) => w - 2 - ((now - t) / MEM_WINDOW_S) * (w - 4);
  const y = (mb) => h - 2 - Math.min(1, Math.max(0, mb / top)) * (h - 4);
  g.fillStyle = 'rgba(255, 255, 255, 0.07)';
  g.fillRect(0, Math.round(y(top / 2)), w, 1);

  g.strokeStyle = 'rgba(142, 151, 164, 0.9)';
  g.fillStyle = 'rgba(142, 151, 164, 0.9)';
  g.lineWidth = 1.2;
  let run = [];
  const flush = () => {
    if (run.length === 1) {
      g.beginPath();
      g.arc(run[0][0], run[0][1], 1.2, 0, Math.PI * 2);
      g.fill();
    } else if (run.length > 1) {
      g.beginPath();
      run.forEach(([px, py], i) => (i ? g.lineTo(px, py) : g.moveTo(px, py)));
      g.stroke();
    }
    run = [];
  };
  let previous = null;
  for (const [t, mb] of samples) {
    if (previous !== null && t - previous > MEM_GAP_S) flush();
    run.push([x(t), y(mb)]);
    previous = t;
  }
  flush();
  const last = samples[samples.length - 1];
  if (last) {
    g.fillStyle = '#eaedf2';
    g.beginPath();
    g.arc(x(last[0]), y(last[1]), 1.6, 0, Math.PI * 2);
    g.fill();
  }
  chartLabel(g, `${Number.isInteger(top) ? top : top.toFixed(1)}\u202fMB`, 4, 2, 'top');
  chartLabel(g, `${MEM_WINDOW_S}\u202fs`, 4, h - 2, 'bottom');
}

function makeEl(tag, className, text) {
  const el = document.createElement(tag);
  if (className) el.className = className;
  if (text !== undefined) el.textContent = text;
  return el;
}

export class Monitor {
  constructor(gl, el) {
    this.gl = gl;
    this.el = el;
    this.timer = gl.getExtension('EXT_disjoint_timer_query_webgl2');
    this.pending = [];
    this.spare = [];
    this.gpuMs = null;
    this.gpuLast = null;
    this.intervals = [];  // [time s, interval s]
    this.sections = { analysis: 0, upload: 0, draw: 0, total: 0 };
    this.longFrames = [];
    this.pressure = null;
    this.textClock = 0;
    this.sampleClock = 0;
    this.rows = { count: 0, time: 0, rate: 0 };
    this.series = { frame: new Series(), gpu: new Series(), cpu: new Series() };
    this.memory = [];      // [seconds, MB] over the last MEM_WINDOW_S
    this.memPeak = null;
    this.memClock = MEM_SAMPLE_S;
    this.layoutKey = '';
    this.parts = {};
    this.cells = [];       // { part, el } in display order
    this.widest = {};      // widest each group has measured, px
    this.arrangement = '';

    // "Frozen" sits in the dock head beside the title rather than taking a group's place.
    this.frozen = makeEl('span', 'mon-frozen');
    this.frozen.hidden = true;
    this.frozenText = makeEl('span', 'mon-frozen-text', t('mon.frozen'));
    this.frozen.append(makeEl('span', 'mon-pip'), this.frozenText);
    el.parentElement?.querySelector('.dock-title')?.after(this.frozen);
    new ResizeObserver(() => this.arrange()).observe(el);

    const dbg = gl.getExtension('WEBGL_debug_renderer_info');
    this.gpuName = shortGpuName(String(dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER)));
    this.samples = gl.getParameter(gl.SAMPLES);

    const types = PerformanceObserver.supportedEntryTypes || [];
    const stalls = types.includes('long-animation-frame') ? 'long-animation-frame' : types.includes('longtask') ? 'longtask' : null;
    this.stallsSupported = !!stalls;
    if (stalls) {
      new PerformanceObserver((list) => {
        for (const e of list.getEntries()) this.longFrames.push(e.startTime / 1000);
      }).observe({ type: stalls });
    }
    if ('PressureObserver' in window) {
      try {
        const po = new PressureObserver((records) => { this.pressure = records[records.length - 1].state; });
        po.observe('cpu', { sampleInterval: 1000 }).catch(() => {});
      } catch { /* not allowed here */ }
    }
  }

  // Called once per display frame with the time since the previous one.
  frame(now, interval) {
    const s = now / 1000;
    this.intervals.push([s, interval]);
    while (this.intervals.length && this.intervals[0][0] < s - WINDOW_S) this.intervals.shift();
    while (this.longFrames.length && this.longFrames[0] < s - LONG_FRAME_S) this.longFrames.shift();
    this.series.frame.note(interval * 1000);
    this.pollGpu();
  }

  // Main-thread milliseconds this frame for one part of the work, smoothed.
  add(name, value) {
    this.sections[name] = this.sections[name] * 0.9 + value * 0.1;
    if (name === 'total') this.series.cpu.note(value);
  }

  gpuBegin() {
    if (!this.timer || this.pending.length >= MAX_QUERIES || this.active) return;
    this.active = this.spare.pop() || this.gl.createQuery();
    this.gl.beginQuery(this.timer.TIME_ELAPSED_EXT, this.active);
  }

  gpuEnd() {
    if (!this.active) return;
    this.gl.endQuery(this.timer.TIME_ELAPSED_EXT);
    this.pending.push(this.active);
    this.active = null;
  }

  pollGpu() {
    const gl = this.gl;
    while (this.pending.length) {
      const q = this.pending[0];
      if (!gl.getQueryParameter(q, gl.QUERY_RESULT_AVAILABLE)) break;
      const disjoint = gl.getParameter(this.timer.GPU_DISJOINT_EXT);
      const value = gl.getQueryParameter(q, gl.QUERY_RESULT) / 1e6;
      if (!disjoint) {
        this.gpuMs = this.gpuMs === null ? value : this.gpuMs * 0.9 + value * 0.1;
        this.series.gpu.note(value);
      }
      this.spare.push(this.pending.shift());
    }
  }

  // Builds the horizontal telemetry deck: 1 hero frame chart + heterogeneous secondary cells.
  layout(show) {
    const key = ['frame', 'gpu', 'cpu', 'memory', 'audio'].map((k) => (show[k] ? k : '')).join(',');
    if (key === this.layoutKey) return;
    this.layoutKey = key;
    this.parts = {};
    this.textClock = 1;  // populate text immediately after rebuilding layout

    const blocks = [];
    const parts = [];

    if (show.frame) {
      const cell = makeEl('div', 'mon-cell mon-cell-chart mon-cell-hero');
      const left = makeEl('div', 'mon-chart-stats mon-hero-stats');
      const head = makeEl('div', 'mon-head');
      const title = makeEl('span', 'mon-title', t('monFrame'));
      const bigWrap = makeEl('span', 'mon-hero-fps');
      const fpsVal = makeEl('span', 'mon-fps-num', '—');
      const fpsUnit = makeEl('span', 'mon-unit', '\u202ffps');
      bigWrap.append(fpsVal, fpsUnit);
      head.append(title, bigWrap);

      const kv = makeEl('div', 'mon-kv3');
      const avgV = makeEl('span', 'mon-num', '—');
      const worstV = makeEl('span', 'mon-num', '—');
      const dropV = makeEl('span', 'mon-num', '0');
      kv.append(
        makeEl('span', 'mon-lbl', t('mon.avg')), avgV,
        makeEl('span', 'mon-lbl', t('mon.worst')), worstV,
        makeEl('span', 'mon-lbl', t('mon.drop')), dropV,
      );
      left.append(head, kv);

      const canvas = makeEl('canvas', 'mon-chart');
      cell.append(left, canvas);
      blocks.push(cell);
      parts.push('frame');
      this.parts.frame = { fpsVal, avgV, worstV, dropV, canvas };
    }

    if (show.gpu) {
      const cell = makeEl('div', 'mon-cell mon-cell-gpu');
      const head = makeEl('div', 'mon-head');
      const titleWrap = makeEl('span', 'mon-title-wrap');
      const title = makeEl('span', 'mon-title', 'GPU');
      const dev = makeEl('span', 'mon-dev', this.gpuName);
      titleWrap.append(title, dev);
      const timeV = makeEl('span', 'mon-val', '—');
      head.append(titleWrap, timeV);

      const meter = makeEl('div', 'mon-meter');
      const fill = makeEl('div', 'mon-meter-fill');
      meter.append(fill);

      const specs = makeEl('div', 'mon-specs');
      const resV = makeEl('span', 'mon-num', '—');
      const vertsV = makeEl('span', 'mon-pair');
      const drawsV = makeEl('span', 'mon-pair');
      const vramV = makeEl('span', 'mon-pair');
      // Resolution on one line, the counts under it, so the group stays narrow enough
      // for all five to share a row in a 1440 px window.
      specs.append(resV);
      const counts = makeEl('div', 'mon-specs');
      counts.append(vertsV, drawsV, vramV);

      cell.append(head, meter, specs, counts);
      blocks.push(cell);
      parts.push('gpu');
      this.parts.gpu = { timeV, meter, fill, resV, vertsV, drawsV, vramV };
    }

    if (show.cpu) {
      const cell = makeEl('div', 'mon-cell mon-cell-cpu');
      const head = makeEl('div', 'mon-head');
      const titleWrap = makeEl('span', 'mon-title-wrap');
      const title = makeEl('span', 'mon-title', 'CPU');
      const sysV = makeEl('span', 'mon-dev', '');
      titleWrap.append(title, sysV);
      const totalV = makeEl('span', 'mon-val', '—');
      head.append(titleWrap, totalV);

      const split = makeEl('div', 'mon-split');
      const segA = makeEl('div', 'mon-seg mon-seg-a');
      const segU = makeEl('div', 'mon-seg mon-seg-u');
      const segD = makeEl('div', 'mon-seg mon-seg-d');
      split.append(segA, segU, segD);

      const specs = makeEl('div', 'mon-specs');
      const aV = makeEl('span', 'mon-pair');
      const uV = makeEl('span', 'mon-pair');
      const dV = makeEl('span', 'mon-pair');
      const stallsV = makeEl('span', 'mon-pair');
      specs.append(aV, uV, dV, stallsV);

      cell.append(head, split, specs);
      blocks.push(cell);
      parts.push('cpu');
      this.parts.cpu = { totalV, sysV, segA, segU, segD, aV, uV, dV, stallsV };
    }

    if (show.memory) {
      const cell = makeEl('div', 'mon-cell mon-cell-chart mon-cell-mem');
      const left = makeEl('div', 'mon-chart-stats');
      const head = makeEl('div', 'mon-head');
      const titleWrap = makeEl('span', 'mon-title-wrap');
      titleWrap.append(makeEl('span', 'mon-title', t('mon.memTitle')), makeEl('span', 'mon-dev', t('mon.jsHeap')));
      const heapV = makeEl('span', 'mon-val', '—');
      head.append(titleWrap, heapV);
      const kv = makeEl('div', 'mon-kv2');
      const peakV = makeEl('span', 'mon-num', '—');
      kv.append(makeEl('span', 'mon-lbl', t('mon.peak')), peakV);
      const devV = makeEl('div', 'mon-specs mon-mem-sub', '');
      left.append(head, kv, devV);

      // Only Chromium exposes the JS heap; elsewhere the group says so and draws no chart.
      const canvas = makeEl('canvas', 'mon-chart');
      canvas.hidden = !performance.memory;
      kv.hidden = !performance.memory;
      cell.append(left, canvas);
      blocks.push(cell);
      parts.push('memory');
      this.parts.memory = { heapV, peakV, canvas, devV };
    }

    if (show.audio) {
      const cell = makeEl('div', 'mon-cell mon-cell-audio');
      const head = makeEl('div', 'mon-head');
      const title = makeEl('span', 'mon-title', t('monAudio'));
      const srV = makeEl('span', 'mon-val', '—');
      head.append(title, srV);

      const grid = makeEl('div', 'mon-audio-grid');
      const rowsV = makeEl('span', 'mon-num', '');
      const winV = makeEl('span', 'mon-pair', '');
      const latV = makeEl('span', 'mon-pair', '');
      grid.append(rowsV, latV, winV);
      cell.append(head, grid);
      blocks.push(cell);
      parts.push('audio');
      this.parts.audio = { srV, grid, rowsV, winV, latV };
    }

    this.cells = blocks.map((el, i) => ({ part: parts[i], el }));
    this.arrangement = '';
    const row = makeEl('div', 'mon-row');
    row.append(...blocks);
    this.el.replaceChildren(...(blocks.length ? [row] : []));
    this.arrange();
  }

  // Each group's natural width, kept at the widest it has been so that a reading gaining a
  // digit does not tip the deck between one row and two.
  measure() {
    this.el.classList.add('is-measuring');
    for (const { part, el } of this.cells) {
      this.widest[part] = Math.max(this.widest[part] || 0, Math.ceil(el.getBoundingClientRect().width));
    }
    this.el.classList.remove('is-measuring');
  }

  // Puts the groups in as few rows as the deck's width allows; each row fills the width,
  // charts taking most of what is spare.
  arrange() {
    if (!this.cells.length) return;
    const style = getComputedStyle(this.el);
    const room = this.el.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
    if (!(room > 0)) return;
    this.measure();
    const widths = this.cells.map((c) => this.widest[c.part]);
    const counts = pack(widths, room, COLUMN_GAP);
    const key = counts.join(',');
    if (key !== this.arrangement) {
      this.arrangement = key;
      let start = 0;
      this.el.replaceChildren(...counts.map((count) => {
        const row = makeEl('div', 'mon-row');
        row.append(...this.cells.slice(start, start + count).map((c) => c.el));
        start += count;
        return row;
      }));
    }
    // Groups are border-box: the basis carries the padding and hairline between groups
    // (COLUMN_GAP in all), so the figures keep the width they were measured at.
    let start = 0;
    for (const count of counts) {
      for (let k = 0; k < count; k++) {
        const { part, el } = this.cells[start + k];
        const edges = (k > 0 ? COLUMN_GAP - 12 : 0) + (k < count - 1 ? 12 : 0);
        const grow = part === 'frame' || part === 'memory' ? 4 : 1;
        el.style.flex = `${grow} 1 ${widths[start + k] + edges}px`;
      }
      start += count;
    }
    this.el.dataset.rows = key;
  }

  // The monitor was switched on: the memory chart starts afresh, as on the desktop.
  clearMemory() {
    this.memory = [];
    this.memPeak = null;
    this.memClock = MEM_SAMPLE_S;
  }

  // Language changed: rebuild the group names on the next render.
  relabel() {
    this.layoutKey = '';
    this.widest = {};
    this.frozenText.textContent = t('mon.frozen');
  }

  // Updates the telemetry deck. show: which groups are on.
  // ctx: { dt, canvas, renderer, audio, rowsPushed, frozen }.
  render(show, ctx) {
    this.layout(show);
    const anyGroup = show.frame || show.gpu || show.cpu || show.memory || show.audio;
    this.el.classList.toggle('is-empty', !anyGroup);

    this.textClock += ctx.dt;
    this.sampleClock += ctx.dt;
    this.rows.time += ctx.dt;
    if (this.rows.time >= 1) {
      this.rows.rate = (ctx.rowsPushed - this.rows.count) / this.rows.time;
      this.rows.count = ctx.rowsPushed;
      this.rows.time = 0;
    }

    const iv = this.intervals.map((e) => e[1]);
    const total = iv.reduce((a, b) => a + b, 0) || 1;
    const fps = iv.length / total;
    const frameMs = 1000 / fps;
    const sorted = [...iv].sort((a, b) => a - b);
    const median = (sorted[Math.floor(sorted.length / 2)] || 1 / 60) * 1000;

    if (this.sampleClock >= SAMPLE_S) {
      this.sampleClock = 0;
      for (const [id, s] of Object.entries(this.series)) {
        s.commit(id === 'gpu' ? this.gpuMs : null);
      }
      if (this.parts.frame?.canvas) drawFrameChart(this.parts.frame.canvas, this.series.frame.values, median);
    }

    // Memory is read once a second against the clock; frozen pictures keep reading.
    this.memClock += ctx.dt;
    if (this.memClock >= MEM_SAMPLE_S && performance.memory) {
      this.memClock = 0;
      const now = performance.now() / 1000;
      const mb = performance.memory.usedJSHeapSize / 1048576;
      this.memory.push([now, mb]);
      while (this.memory.length && this.memory[0][0] < now - MEM_WINDOW_S) this.memory.shift();
      this.memPeak = Math.max(this.memPeak ?? 0, mb);
    }

    if (this.textClock < 0.25) return;
    this.textClock = 0;
    const P = this.parts;
    this.frozen.hidden = !ctx.frozen;

    if (P.frame && iv.length > 2) {
      const worst = sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.99))];
      const dropped = iv.filter((v) => v * 1000 > median * 1.5).length;
      P.frame.fpsVal.textContent = fps.toFixed(0);
      P.frame.avgV.textContent = `${ms(frameMs)}\u202fms`;
      P.frame.worstV.textContent = `${ms(worst * 1000)}\u202fms`;
      P.frame.dropV.textContent = `${dropped}`;
      P.frame.dropV.classList.toggle('is-warn', dropped > 0);
    }

    if (P.gpu) {
      const { calls, vertices } = ctx.renderer.stats;
      const { width: w, height: h } = ctx.canvas;
      const canvasBytes = w * h * (8 * Math.max(1, this.samples) + 8);
      if (this.gpuMs === null) {
        P.gpu.timeV.textContent = t(this.timer ? 'mon.gpuTiming' : 'mon.gpuNoTimer');
        P.gpu.timeV.classList.add('is-muted');
        P.gpu.meter.classList.add('is-muted');
        P.gpu.fill.style.width = '0%';
        P.gpu.fill.classList.remove('is-warn');
      } else {
        const pct = Math.min(100, Math.max(0, (this.gpuMs / frameMs) * 100));
        P.gpu.timeV.textContent = `${ms(this.gpuMs)}\u202fms  ${pct.toFixed(0)}%`;
        P.gpu.timeV.classList.remove('is-muted');
        P.gpu.meter.classList.remove('is-muted');
        P.gpu.fill.style.width = `${pct.toFixed(1)}%`;
        P.gpu.fill.classList.toggle('is-warn', pct >= 85);
      }
      P.gpu.resV.textContent = `${w}×${h}${this.samples > 1 ? ` ${this.samples}x` : ''}${ctx.renderScale ? `  ${Math.round(ctx.renderScale.scale * 100)}%${ctx.renderScale.mode === 'auto' ? ' A' : ''}` : ''}`;
      P.gpu.vertsV.textContent = `${t('mon.verts')} ${(vertices / 1000).toFixed(0)}k`;
      P.gpu.drawsV.textContent = `${t('mon.draws')} ${calls}`;
      P.gpu.vramV.textContent = `${t('mon.vram')} ${mb(ctx.renderer.gpuBytes + canvasBytes)}\u202fMB`;
    }

    if (P.cpu) {
      const s = this.sections;
      const pct = Math.min(100, Math.max(0, (s.total / frameMs) * 100));
      P.cpu.totalV.textContent = `${ms(s.total)}\u202fms  ${pct.toFixed(0)}%`;

      const sysParts = [t('mon.cores', { n: navigator.hardwareConcurrency || '?' })];
      if (this.pressure) sysParts.push(t('mon.pressure', { state: t(`pressure.${this.pressure}`) }));
      P.cpu.sysV.textContent = sysParts.join(' ');

      const phaseSum = Math.max(1e-4, s.analysis + s.upload + s.draw);
      P.cpu.segA.style.width = `${((s.analysis / phaseSum) * 100).toFixed(1)}%`;
      P.cpu.segU.style.width = `${((s.upload / phaseSum) * 100).toFixed(1)}%`;
      P.cpu.segD.style.width = `${((s.draw / phaseSum) * 100).toFixed(1)}%`;

      P.cpu.aV.textContent = `${t('mon.analysis')} ${ms(s.analysis)}`;
      P.cpu.uV.textContent = `${t('mon.upload')} ${ms(s.upload)}`;
      P.cpu.dV.textContent = `${t('mon.draw')} ${ms(s.draw)}`;
      if (this.stallsSupported) {
        const n = this.longFrames.length;
        P.cpu.stallsV.hidden = false;
        P.cpu.stallsV.textContent = t('mon.stalls', { n });
        P.cpu.stallsV.classList.toggle('is-warn', n > 0);
      } else {
        P.cpu.stallsV.hidden = true;
      }
    }

    if (P.memory) {
      if (performance.memory) {
        P.memory.heapV.textContent = `${mb(performance.memory.usedJSHeapSize)}\u202fMB`;
        P.memory.heapV.classList.remove('is-muted');
        if (this.memPeak !== null) P.memory.peakV.textContent = `${mb(this.memPeak * 1048576)}\u202fMB`;
        drawMemoryChart(P.memory.canvas, this.memory, performance.now() / 1000);
      } else {
        P.memory.heapV.textContent = t('mon.noHeap');
        P.memory.heapV.classList.add('is-muted');
      }
      if (navigator.deviceMemory) {
        P.memory.devV.textContent = navigator.deviceMemory >= 8
          ? t('mon.device8')
          : t('mon.device', { gb: navigator.deviceMemory });
      } else {
        P.memory.devV.textContent = '';
      }
    }

    if (P.audio) {
      const info = ctx.audio.info;
      if (info) {
        P.audio.srV.textContent = `${(info.sampleRate / 1000).toFixed(1)}\u202fkHz`;
        P.audio.srV.classList.remove('is-muted');
        P.audio.grid.hidden = false;
        P.audio.rowsV.textContent = t('mon.rowsPerSec', { rows: this.rows.rate.toFixed(0) });
        P.audio.latV.textContent = `${t('mon.lat')} ${info.outputMs.toFixed(0)}\u202fms`;
        P.audio.winV.textContent = `${t('mon.win')} ${info.windowsMs.map((m) => m.toFixed(0)).join('/')}\u202fms`;
      } else {
        P.audio.srV.textContent = t('mon.notStarted');
        P.audio.srV.classList.add('is-muted');
        P.audio.grid.hidden = true;
      }
    }
    this.arrange();
  }
}
