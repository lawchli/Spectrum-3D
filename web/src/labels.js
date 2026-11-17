// Axis labels projected from world caliper anchors every frame.
// The Android port uses the same projection with Compose text over the GLES30 view.

import { transform } from './mat4.js';
import { BANDS, hzToBand, hzText, bandToU, hzTicks } from './bands.js';

// 1 while the camera looks at the front edge from its own side (or straight down), falling
// to 0 once it has gone round behind, where the terrain stands between it and everything
// drawn on that edge. cameraBack: unit vector from the centre toward the camera.
export function frontFacing(cameraBack) {
  const t = Math.max(0, Math.min(1, (-cameraBack[2] - 0.02) / 0.18));
  return 1 - t * t * (3 - 2 * t);
}

export class Labels {
  constructor(root, look) {
    this.root = root;
    this.look = look;
    this.items = [];
    this.seconds = 4;
    this.mel = 0;
  }

  // mel: where the frequency axis is between semitone (0) and mel (1), see bands.js bandToU.
  build(seconds, mel = this.mel) {
    this.seconds = seconds;
    this.mel = mel;
    this.root.replaceChildren();
    this.items = [];
    const { width: w, height: h, depth: d } = this.look.size;
    const compactHz = new Set(hzTicks(this.look.labels, mel, true));
    const compactDb = new Set(this.look.labels.dbCompact || this.look.labels.db);

    const add = (kind, text, pos, extraClass = '', meta = {}) => {
      const el = document.createElement('div');
      el.className = `label label-${kind}${extraClass ? ' ' + extraClass : ''}`;
      el.textContent = text;
      this.root.append(el);
      this.items.push({ el, kind, pos, ...meta });
    };

    for (const hz of hzTicks(this.look.labels, mel)) {
      const u = bandToU(hzToBand(hz), mel);
      if (u < -0.001 || u > 1.001) continue;
      add('hz', hzText(hz), [(u - 0.5) * w, 0, d / 2 + d * 0.026], '', {
        compactKeep: compactHz.has(hz),
      });
    }

    // Below the look's list, while the floor is lower (low levels), every 24 dB down to it.
    const { floor, ceiling } = this.look.db;
    const dbs = [...this.look.labels.db];
    for (let db = Math.min(...dbs) - 24; db >= floor - 1e-6; db -= 24) {
      dbs.push(db);
      compactDb.add(db);
    }
    for (const db of dbs) {
      const level = (db - floor) / (ceiling - floor);
      if (level < 0 || level > 1) continue;
      const text = db === 0 ? '0\u202fdB' : db < 0 ? `−${Math.abs(db)}` : `${db}`;
      const isHeadroom = db === this.look.labels.headroomDb;
      const extra = isHeadroom ? 'label-headroom' : db === 0 ? 'label-zero' : '';
      add('db', text, [-w / 2 - w * 0.018, level * h, d / 2], extra, {
        compactKeep: compactDb.has(db),
      });
    }

    const every = this.look.labels.seconds;
    for (let s = 0; s <= seconds + 1e-6; s += every) {
      const text = s === 0 ? '0\u202fs' : `−${s}\u202fs`;
      add('time', text, [w / 2 + w * 0.017, 0, d / 2 - (s / seconds) * d], s === 0 ? 'label-now' : '', {
        sec: s,
      });
    }

    // Octave names inside the piano strip, at the left edge of every C key (band 0 is C0).
    for (let band = 0; band < BANDS; band += 12) {
      const u = bandToU(Math.max(0, band - 0.5), mel);
      add('key', `C${band / 12}`, [(u - 0.5) * w, 0, d / 2], '', { octave: band / 12 });
    }
  }

  // show.time: time labels (off without the waterfall). show.keys: octave names in the piano strip.
  update(viewProj, cameraBack, pitchDeg, cssW, cssH, show = { time: true, keys: false }) {
    const fadeDb = Math.max(0, Math.min(1, Math.cos(pitchDeg * Math.PI / 180) * 1.75 - 0.35));
    const fadeTime = Math.max(0, Math.min(1, (Math.hypot(cameraBack[0], cameraBack[1]) - 0.12) * 3.2));
    const facing = frontFacing(cameraBack);
    const narrowW = cssW < 430;
    const shortH = cssH < 260;
    const denseTime = this.seconds >= 8 && cssH < 440;
    // An octave name is left out while it would sit within 22 px of the one before it:
    // every other C on a narrow log axis, most of the low octaves on the mel axis.
    let lastKeyX = -Infinity;

    for (const it of this.items) {
      if (it.kind === 'hz' && narrowW && !it.compactKeep) { it.el.style.opacity = 0; continue; }
      if (it.kind === 'db' && shortH && !it.compactKeep) { it.el.style.opacity = 0; continue; }
      if (it.kind === 'time' && (!show.time || (denseTime && it.sec % 2 !== 0))) { it.el.style.opacity = 0; continue; }
      if (it.kind === 'key' && !show.keys) { it.el.style.opacity = 0; continue; }

      const [x, y, , wc] = transform(viewProj, it.pos[0], it.pos[1], it.pos[2]);
      if (wc <= 0) { it.el.style.opacity = 0; continue; }
      const sx = (x / wc * 0.5 + 0.5) * cssW;
      const sy = (1 - (y / wc * 0.5 + 0.5)) * cssH;
      if (it.kind === 'key') {
        if (Math.abs(sx - lastKeyX) < 22) { it.el.style.opacity = 0; continue; }
        lastKeyX = sx;
      }
      const fade = it.kind === 'db' ? fadeDb * facing : it.kind === 'time' ? fadeTime : facing;
      it.el.style.opacity = fade.toFixed(3);
      it.el.style.transform = `translate(${sx.toFixed(1)}px, ${sy.toFixed(1)}px)`;
    }
  }
}
