// Fixed frequency axis: equal-tempered semitones (A4 = 440 Hz) from C0 (16.35 Hz, the
// lowest musical note) to F#10 (23.7 kHz, just under 24 kHz, the Nyquist frequency at
// 48 kHz). Positions along the axis are semitones from C0 ("band" 0 to 126, MIDI note
// band + 12), so note names sit on whole numbers. The data has `sub` bands per semitone:
// data band i sits at semitone i / sub. One per semitone (127 bands) is the byte format
// the phone receives. Bytes are 0.5 dB steps from -120 dBFS.

export const BANDS = 127;  // semitone positions, C0 .. F#10
export const SUBDIVISIONS = [1, 2, 4, 8];
const FIRST_MIDI = 12;  // C0

export function bandCount(sub) { return (BANDS - 1) * sub + 1; }
export function bandHz(band) { return 440 * 2 ** ((band + FIRST_MIDI - 69) / 12); }
export function hzToBand(hz) { return 69 - FIRST_MIDI + 12 * Math.log2(hz / 440); }

// Where a semitone position sits across the frequency axis, 0..1. mel 0 is the axis of this
// file, the same width for every semitone. mel 1 is the mel scale between the same two end
// frequencies, as audio editors draw spectrograms: everything up to 1 kHz takes the first
// quarter and the top two octaves almost half. In between the two are blended (the axis
// glides when it is switched). common.glsl has the same function for the shaders.
const melOf = (hz) => Math.log(1 + hz / 700);
const MEL_LO = melOf(bandHz(0)), MEL_HI = melOf(bandHz(BANDS - 1));
export function bandToU(band, mel = 0) {
  const lin = band / (BANDS - 1);
  return mel ? lin + ((melOf(bandHz(band)) - MEL_LO) / (MEL_HI - MEL_LO) - lin) * mel : lin;
}
// The inverse, by bisection (the blend of the two axes has no closed form).
export function uToBand(u, mel = 0) {
  if (!mel) return u * (BANDS - 1);
  let lo = 0, hi = BANDS - 1;
  for (let i = 0; i < 30; i++) {
    const mid = (lo + hi) / 2;
    if (bandToU(mid, mel) < u) lo = mid; else hi = mid;
  }
  return (lo + hi) / 2;
}
// Frequencies to mark along the axis: the look's usual list, or its mel list once the axis
// is more mel than not (the low decades crowd together there).
export function hzTicks(labels, mel = 0, compact = false) {
  const useMel = mel > 0.5 && labels.hzMel;
  if (compact) return (useMel ? labels.hzMelCompact : labels.hzCompact) || (useMel ? labels.hzMel : labels.hz);
  return useMel ? labels.hzMel : labels.hz;
}

export function dbToByte(db) {
  return Math.max(0, Math.min(255, Math.round((db + 120) * 2)));
}

const NOTE_NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];

export function noteName(hz) {
  const midi = Math.round(69 + 12 * Math.log2(hz / 440));
  const cents = Math.round((69 + 12 * Math.log2(hz / 440) - midi) * 100);
  return { name: `${NOTE_NAMES[((midi % 12) + 12) % 12]}${Math.floor(midi / 12) - 1}`, cents };
}

export function hzText(hz) {
  return hz >= 1000 ? `${+(hz / 1000).toFixed(hz >= 10000 ? 0 : 1)}k` : `${Math.round(hz)}`;
}

// Maps FFT bins to the data bands. Each band takes the loudest bin inside its width
// (1/sub semitone), or, when it is narrower than one bin (the low end, more so with finer
// bands), interpolates the bins around its centre: that draws the window's smooth peak
// shape, not detail the FFT does not have. Bands entirely above Nyquist stay empty.
// Returns the entries and the highest bin any of them reads.
export function buildBandMap(sampleRate, fftSize, sub, loHz = 0, hiHz = Infinity) {
  const hzPerBin = sampleRate / fftSize;
  const bins = fftSize / 2;
  const half = 2 ** (1 / (24 * sub));
  const map = [];
  let top = 0;
  for (let i = 0; i < bandCount(sub); i++) {
    const fc = bandHz(i / sub);
    if (fc < loHz || fc >= hiHz || fc / half >= sampleRate / 2) { map.push(null); continue; }
    const b0 = Math.ceil(fc / half / hzPerBin);
    const b1 = Math.min(Math.floor((fc * half) / hzPerBin), bins - 1);
    if (b1 >= b0 && b0 >= 1) {
      map.push({ b0, b1 });
      top = Math.max(top, b1);
    } else {
      const pos = Math.min(fc / hzPerBin, bins - 1);
      map.push({ pos });
      top = Math.max(top, Math.min(Math.floor(pos) + 2, bins - 1));
    }
  }
  return { map, bins: top + 1 };
}

// Catmull-Rom through the dB of four bins: smooth across bins, unlike straight lines.
function cubic(d, pos) {
  const last = d.length - 1;
  const i = Math.floor(pos), f = pos - i;
  const p0 = d[Math.max(0, i - 1)], p1 = d[i], p2 = d[Math.min(last, i + 1)], p3 = d[Math.min(last, i + 2)];
  return p1 + 0.5 * f * (p2 - p0 + f * (2 * p0 - 5 * p1 + 4 * p2 - p3 + f * (3 * (p1 - p2) + p3 - p0)));
}

export function applyBandMap(map, spectrumDb, out) {
  for (let k = 0; k < map.length; k++) {
    const m = map[k];
    if (!m) { out[k] = -120; continue; }
    let db;
    if (m.pos !== undefined) {
      db = cubic(spectrumDb, m.pos);
    } else {
      db = -Infinity;
      for (let b = m.b0; b <= m.b1; b++) if (spectrumDb[b] > db) db = spectrumDb[b];
    }
    out[k] = Math.max(-120, Math.min(0, Number.isFinite(db) ? db : -120));
  }
}
