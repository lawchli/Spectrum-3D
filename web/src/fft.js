// Real FFT with the Web Audio AnalyserNode's window, for audio.js. Running our own FFT
// instead of reading an AnalyserNode lets data rows come at a fixed audio rate (up to
// several per display frame). The Android port does the same in Kotlin.

// Blackman window, alpha 0.16, as the AnalyserNode uses. Its coherent gain is 0.42.
const COHERENT_GAIN = 0.42;

export class RealFFT {
  constructor(n) {
    const h = n / 2;
    this.n = n;
    this.h = h;
    this.re = new Float64Array(h);
    this.im = new Float64Array(h);
    this.window = new Float64Array(n);
    for (let i = 0; i < n; i++) {
      const x = (2 * Math.PI * i) / n;
      this.window[i] = 0.42 - 0.5 * Math.cos(x) + 0.08 * Math.cos(2 * x);
    }
    const bits = Math.log2(h);
    this.rev = new Uint32Array(h);
    for (let i = 0; i < h; i++) {
      let r = 0;
      for (let b = 0; b < bits; b++) r |= ((i >> b) & 1) << (bits - 1 - b);
      this.rev[i] = r;
    }
    // Twiddles of the half-size complex FFT, and of the step that splits it into the real one.
    this.cosH = Float64Array.from({ length: h / 2 }, (_, k) => Math.cos((2 * Math.PI * k) / h));
    this.sinH = Float64Array.from({ length: h / 2 }, (_, k) => Math.sin((2 * Math.PI * k) / h));
    this.cosN = Float64Array.from({ length: h }, (_, k) => Math.cos((2 * Math.PI * k) / n));
    this.sinN = Float64Array.from({ length: h }, (_, k) => Math.sin((2 * Math.PI * k) / n));
    // Full-scale sine reads 0 dB.
    this.scale = (2 / (n * COHERENT_GAIN)) ** 2;
  }

  // dB of bins 0 .. bins-1 (bins <= n/2) for the n samples of `ring` (a power-of-two ring
  // buffer) that end just before absolute sample `end`.
  run(ring, end, out, bins = this.h) {
    const { n, h, re, im, window: w, rev } = this;
    const mask = ring.length - 1;
    const start = end - n;
    // Pack even samples into re and odd into im, in bit-reversed order.
    for (let m = 0; m < h; m++) {
      const j = rev[m];
      const i = 2 * m;
      re[j] = ring[(start + i) & mask] * w[i];
      im[j] = ring[(start + i + 1) & mask] * w[i + 1];
    }
    const { cosH, sinH } = this;
    for (let size = 2; size <= h; size <<= 1) {
      const half = size >> 1;
      const step = h / size;
      for (let i = 0; i < h; i += size) {
        for (let j = 0, k = 0; j < half; j++, k += step) {
          const a = i + j, b = a + half;
          const wr = cosH[k], wi = -sinH[k];
          const tr = re[b] * wr - im[b] * wi;
          const ti = re[b] * wi + im[b] * wr;
          re[b] = re[a] - tr; im[b] = im[a] - ti;
          re[a] += tr; im[a] += ti;
        }
      }
    }
    // Split the packed spectrum Z into the real input's X[k] = E[k] + e^(-2 pi i k / n) O[k].
    const { cosN, sinN, scale } = this;
    for (let k = 0; k < bins; k++) {
      const q = k ? h - k : 0;
      const er = 0.5 * (re[k] + re[q]), ei = 0.5 * (im[k] - im[q]);
      const or = 0.5 * (im[k] + im[q]), oi = -0.5 * (re[k] - re[q]);
      const c = cosN[k], s = sinN[k];
      const xr = er + c * or + s * oi;
      const xi = ei + c * oi - s * or;
      out[k] = 10 * Math.log10((xr * xr + xi * xi) * scale + 1e-30);
    }
  }
}
