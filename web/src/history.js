// CPU history shared by rendering, readouts and cross-language reference fixtures.
export const HISTORY_ROWS = 4096;
export const LEVELS = 6;
const POWER = Float32Array.from({ length: 256 }, (_, b) => 10 ** ((b / 2 - 120) / 10));

export class History {
  constructor(count, rows = HISTORY_ROWS) {
    this.count = count;
    this.rows = rows;
    this.history = Array.from({ length: LEVELS }, () => new Uint8Array(count * rows));
    this.sums = Array.from({ length: LEVELS }, () => new Float32Array(count));
    this.rowsPushed = 0;
    this.dirty = new Uint32Array(LEVELS);
  }
  push(bytes) {
    const n = this.rowsPushed++, { count, rows } = this;
    this.history[0].set(bytes, (n % rows) * count);
    this.dirty[0]++;
    for (let l = 1; l < LEVELS; l++) {
      const sum = this.sums[l], span = 2 ** l;
      for (let k = 0; k < count; k++) sum[k] += POWER[bytes[k]];
      if ((n + 1) % span) continue;
      const base = (Math.floor(n / span) % rows) * count;
      for (let k = 0; k < count; k++) {
        this.history[l][base + k] = Math.max(0, Math.min(255, Math.round((10 * Math.log10(sum[k] / span + 1e-30) + 120) * 2)));
        sum[k] = 0;
      }
      this.dirty[l]++;
    }
  }
  // Each level is one or two contiguous ranges, even after more than a full ring was queued.
  flush(upload) {
    for (let l = 0; l < LEVELS; l++) {
      const count = Math.min(this.rows, this.dirty[l]);
      if (!count) continue;
      const end = Math.floor(this.rowsPushed / 2 ** l) % this.rows;
      const start = (end - count + this.rows) % this.rows;
      const first = Math.min(count, this.rows - start);
      upload(l, start, first, this.history[l].subarray(start * this.count, (start + first) * this.count));
      if (first < count) upload(l, 0, count - first, this.history[l].subarray(0, (count - first) * this.count));
      this.dirty[l] = 0;
    }
  }
}
