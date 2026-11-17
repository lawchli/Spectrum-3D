// Render scale: 100, 85 or 70 % of the canvas resolution, or automatic. Automatic mode keeps only
// changes that helped and backs off from increases that failed, so a device without a GPU timer
// (most phones, Safari, Firefox) does not swing between two sizes every half minute:
//  - lower after 2 s of slow frames, unless the GPU timer shows the GPU busy for under half the
//    frame (the time goes elsewhere, and fewer pixels would not help);
//  - judge every reduction over the next 5 s and undo it when frames did not get 10 % shorter,
//    then pause reductions (60 s, doubling up to 10 min): without a timer this is how CPU load shows;
//  - raise after a run of fast frames (10 s when the timer predicts headroom, 30 s without one);
//    an increase undone within 7 s doubles the wait before the next try at that size (up to 32x).
// Hidden tabs, pauses and single long stalls count for nothing.
const SCALES = [1, 0.85, 0.7];
const SLOW = 1 / 48, FAST = 1 / 55, TRIAL_S = 5, UP_TRIAL_S = 7, BUDGET_MS = 13;

export class RenderScale {
  constructor(mode = '1') { this.setMode(mode); }

  setMode(mode) {
    this.mode = ['auto', '1', '0.85', '0.7'].includes(mode) ? mode : '1';
    this.index = 0; this.scale = this.mode === 'auto' ? 1 : Number(this.mode);
    this.slow = 0; this.fast = 0; this.avg = 1 / 60;
    this.trial = null;                   // the last change, while it is being judged
    this.hold = 0; this.holdNext = 60;   // seconds without reductions after one was undone
    this.penalty = SCALES.map(() => 1);  // wait multiplier for raising to each size
  }

  set(index) { this.index = index; this.scale = SCALES[index]; this.slow = this.fast = 0; }

  // Returns true when the scale changed and the canvas has to be resized.
  update(dt, gpuMs, active = true) {
    if (this.mode !== 'auto' || !active || dt <= 0 || dt > 0.1) { this.slow = this.fast = 0; return false; }
    this.avg += (dt - this.avg) * Math.min(1, dt);  // about the last second of frames
    this.hold = Math.max(0, this.hold - dt);
    const gpuBound = gpuMs === null || gpuMs >= dt * 1000 * 0.5;
    const slow = dt > SLOW && gpuBound;
    // GPU time one size up grows with the pixel count.
    const up = this.index > 0 ? (SCALES[this.index - 1] / SCALES[this.index]) ** 2 : 0;
    const fast = dt < FAST && (gpuMs === null || gpuMs * up < BUDGET_MS);
    this.slow = slow ? this.slow + dt : 0;
    this.fast = fast ? this.fast + dt : 0;

    const t = this.trial;
    if (t) {
      t.time += dt;
      if (t.lowered) {
        t.sum += dt; t.n++;
        if (t.time < TRIAL_S) return false;  // no further change while a reduction is judged
        this.trial = null;
        const mean = t.sum / t.n;
        if (mean > SLOW && mean > t.before * 0.9) {  // still slow and not faster: undo, pause reductions
          this.set(t.from); this.hold = this.holdNext; this.holdNext = Math.min(600, this.holdNext * 2);
          return true;
        }
      } else if (t.time >= UP_TRIAL_S) this.trial = null;  // the increase held
    }

    if (this.slow >= 2 && this.index < SCALES.length - 1 && !this.hold) {
      if (this.trial) this.penalty[this.index] = Math.min(32, this.penalty[this.index] * 2);  // the increase failed
      const from = this.index;
      this.set(from + 1); this.trial = { lowered: true, from, before: this.avg, time: 0, sum: 0, n: 0 };
      return true;
    }
    if (!this.trial && this.index > 0 && this.fast >= (gpuMs === null ? 30 : 10) * this.penalty[this.index - 1]) {
      this.set(this.index - 1); this.trial = { lowered: false, time: 0 };
      return true;
    }
    return false;
  }
}
