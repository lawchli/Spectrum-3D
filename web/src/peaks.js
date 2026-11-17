// Main peaks of the live spectrum, labelled with note, cents, frequency and level. Peaks
// are tracked from frame to frame so the labels stay put while a note holds. On the Mac
// preview the frequency is refined from the FFT bins; with only the semitone bands (the
// phone today) it comes from a parabola through three bands and the cents are rough.
// Peaks are found on the data bands (sub per semitone) and reported in semitones.

import { bandHz, noteName, bandToU } from './bands.js';
import { transform } from './mat4.js';

const MAX_LABELS = 5;
const PROMINENCE_DB = 6;  // a peak stands at least this far above the dip toward higher ground
const RANGE_DB = 40;      // and within this of the loudest band
const FLOOR_DB = -84;
const MATCH_BANDS = 1.5;  // a peak this many semitones from last frame's is the same peak
const FADE_S = 0.16;
const TEXT_EVERY_S = 0.125;  // digits change at most 8 times a second, so they can be read

function findPeaks(db, sub) {
  const n = db.length;
  let top = -Infinity;
  for (let k = 0; k < n; k++) top = Math.max(top, db[k]);
  const floor = Math.max(FLOOR_DB, top - RANGE_DB);
  const out = [];
  for (let k = 1; k < n - 1; k++) {
    const v = db[k];
    if (v < floor || v < db[k - 1] || v <= db[k + 1]) continue;
    let left = v, right = v;
    for (let i = k - 1; i >= 0 && db[i] <= v; i--) left = Math.min(left, db[i]);
    for (let i = k + 1; i < n && db[i] <= v; i++) right = Math.min(right, db[i]);
    if (v - Math.max(left, right) < PROMINENCE_DB) continue;
    const a = db[k - 1], c = db[k + 1], d = a - 2 * v + c;
    const off = d < 0 ? Math.max(-0.5, Math.min(0.5, (0.5 * (a - c)) / d)) : 0;
    out.push({ band: (k + off) / sub, db: v - 0.25 * (a - c) * off });
  }
  return out.sort((p, q) => q.db - p.db).slice(0, MAX_LABELS + 3);
}

const signedCents = (c) => (c > 0 ? `+${c}` : c < 0 ? `−${-c}` : '±0');
const hzLabel = (hz) => (hz >= 1000 ? `${(hz / 1000).toFixed(hz >= 10000 ? 1 : 2)}\u202fkHz` : `${hz.toFixed(hz < 100 ? 1 : 0)}\u202fHz`);
const dbLabel = (db) => (db < -0.5 ? `−${Math.round(-db)}` : '0');

export class PeakTracker {
  constructor() {
    this.tracks = [];
    this.textClock = 0;
  }

  clear() { this.tracks = []; }

  // shown: dB per data band as displayed. raw: the latest analysis, to tell whether a peak
  // is still sounding or only falling on the display's release. sub: data bands per
  // semitone. refineHz(band): exact peak frequency near a semitone position, or null.
  update(shown, raw, dt, refineHz, sub = 1) {
    for (const t of this.tracks) t.matched = false;
    for (const c of findPeaks(shown, sub)) {
      let best = null, bestDist = MATCH_BANDS;
      for (const t of this.tracks) {
        const dist = Math.abs(t.band - c.band);
        if (!t.matched && dist < bestDist) { best = t; bestDist = dist; }
      }
      if (!best) {
        best = { band: c.band, db: c.db, hz: bandHz(c.band), alpha: 0, text: null };
        this.tracks.push(best);
      }
      best.matched = true;
      best.band += (c.band - best.band) * 0.5;
      best.db = c.db;
      const k = Math.round(c.band * sub);
      const sounding = raw[k] > shown[k] - 6;
      const hz = sounding ? (refineHz?.(c.band) ?? bandHz(c.band)) : null;
      if (hz && Math.abs(12 * Math.log2(hz / bandHz(c.band))) < 1) {
        best.hz = best.text ? best.hz * (hz / best.hz) ** 0.4 : hz;
      }
    }
    for (const t of this.tracks) t.alpha = Math.max(0, Math.min(1, t.alpha + (t.matched ? dt : -dt) / FADE_S));
    this.tracks = this.tracks.filter((t) => t.matched || t.alpha > 0);

    this.textClock += dt;
    const refresh = this.textClock >= TEXT_EVERY_S;
    if (refresh) this.textClock = 0;
    for (const t of this.tracks) {
      if (t.text && !refresh) continue;
      const { name, cents } = noteName(t.hz);
      t.text = [`${name} ${signedCents(cents)}¢`, `${hzLabel(t.hz)}  ${dbLabel(t.db)}\u202fdB`];
    }
  }
}

// DOM labels over the GL view, projected like the axis labels (labels.js).
const TILT_PIVOT_BAND = 71.2131;  // 1 kHz, as in front.vert

export class PeakLabels {
  constructor(root) {
    this.root = root;
    this.pool = [];
  }

  // Returns the labelled peaks, loudest first: { band, alpha, main }. tilt: dB per octave about
  // 1 kHz the front spectrum is drawn with (front.vert), so a label sits on the drawn curve.
  // mel: where the frequency axis is between semitone (0) and mel (1).
  update(tracks, viewProj, look, cssW, cssH, fade, tilt = 0, mel = 0) {
    const { width: w, height: h, depth: d } = look.size;
    const { floor, ceiling } = look.db;
    const shown = [];
    if (fade > 0.01) {
      for (const t of [...tracks].sort((p, q) => q.db - p.db)) {
        if (shown.length >= MAX_LABELS || t.alpha <= 0) continue;
        const drawn = t.db + tilt * (t.band - TILT_PIVOT_BAND) / 12;
        const level = Math.max(0, Math.min(1, (drawn - floor) / (ceiling - floor)));
        const [x, y, , wc] = transform(viewProj, (bandToU(t.band, mel) - 0.5) * w, level * h, d / 2);
        if (wc <= 0) continue;
        const sx = (x / wc * 0.5 + 0.5) * cssW;
        const sy = (1 - (y / wc * 0.5 + 0.5)) * cssH;
        if (shown.some((s) => Math.abs(s.sx - sx) < 88 && Math.abs(s.sy - sy) < 34)) continue;
        shown.push({ t, sx, sy });
      }
    }
    while (this.pool.length < shown.length) {
      const el = document.createElement('div');
      el.className = 'peak';
      const pitch = document.createElement('div');
      pitch.className = 'peak-pitch';
      const note = document.createElement('span');
      note.className = 'peak-note';
      const cents = document.createElement('span');
      cents.className = 'peak-cents';
      pitch.append(note, cents);
      const meta = document.createElement('div');
      meta.className = 'peak-meta';
      el.append(pitch, meta);
      this.root.append(el);
      this.pool.push({ el, note, cents, meta });
    }
    this.pool.forEach((item, i) => {
      const s = shown[i];
      item.el.hidden = !s;
      if (!s) return;
      item.el.classList.toggle('peak-main', i === 0);
      const [note, cents = ''] = s.t.text[0].split(' ');
      if (item.note.textContent !== note) item.note.textContent = note;
      if (item.cents.textContent !== cents) item.cents.textContent = cents;
      if (item.meta.textContent !== s.t.text[1]) item.meta.textContent = s.t.text[1];
      const x = Math.max(48, Math.min(cssW - 48, s.sx));
      const y = Math.max(38, s.sy);
      item.el.style.opacity = (s.t.alpha * fade).toFixed(3);
      item.el.style.transform = `translate(${x.toFixed(1)}px, ${y.toFixed(1)}px)`;
    });
    return shown.map((s, i) => ({ band: s.t.band, alpha: s.t.alpha * fade, main: i === 0 }));
  }
}
