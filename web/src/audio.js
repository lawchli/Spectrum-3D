// Audio sources for the Mac preview (test synth, input device, file) and the analysis
// that turns them into the fixed semitone bands of bands.js (C0 to 24 kHz). An AudioWorklet
// passes the raw samples on and we run our own FFTs, so data rows come at a fixed audio
// rate (rowRate a second, more than one per display frame if needed) instead of once per
// frame from an AnalyserNode.

import { bandCount, bandHz, buildBandMap, applyBandMap } from './bands.js';
import { RealFFT } from './fft.js';
import { startTestAudio } from './test-audio.js';

// Multi-resolution analysis: long FFT for the bass, short FFTs above, so the highs react
// within a few milliseconds while low notes still resolve to the semitone.
const SPLITS = [
  { fftSize: 8192, hiHz: 350 },
  { fftSize: 2048, hiHz: 1500 },
  { fftSize: 1024, hiHz: Infinity },
];
const CROSSFADE_SEMITONES = 2;  // each side of a split, so the top view shows no seam

export const ROW_RATES = [60, 120, 240, 480];  // data rows a second
const SAMPLE_RATE = 48000;

// A window is recomputed at least every 1/16 of its length. At high row rates the long
// bass window is therefore analysed only every few rows (8192 at 480 rows/s: every 5th), and
// the rows in between glide in a straight line from the previous analysis to the new one, so
// the top view shows no steps. That puts those bands `every - 1` rows late (8 ms here),
// next to a window of 170 ms.
const MIN_HOP_OF_WINDOW = 1 / 16;

const RING = 1 << 15;          // samples kept, more than the longest FFT plus a stalled frame
const LAG_S = 0.004;           // the row clock runs this far behind the newest sample
const SERVO_S = 0.5;           // and follows the audio clock with this time constant
const RESYNC_S = 0.05;         // further off than this (a stall, freezing), it jumps
const CATCH_UP_S = 0.1;        // after a stall at most this much audio is turned into rows

const ramp = (hz, edge) => Math.min(1, Math.max(0, (12 * Math.log2(hz / edge) + CROSSFADE_SEMITONES) / (2 * CROSSFADE_SEMITONES)));

// The audio inputs, or null when the browser does not answer within `ms`. The XZ1 Compact's
// Chrome (157, Android 16) never resolves enumerateDevices(), not even on its own; a call still
// pending is shared, so asking again every second does not pile them up.
let listing = null;
export function listInputs(ms = 3000) {
  listing ??= navigator.mediaDevices.enumerateDevices()
    .then((all) => all.filter((d) => d.kind === 'audioinput'))
    .finally(() => { listing = null; });
  let timer;
  const late = new Promise((resolve) => { timer = setTimeout(resolve, ms, null); });
  return Promise.race([listing.catch(() => null), late]).finally(() => clearTimeout(timer));
}

// Chrome lists the system default and (on Windows) the communications device as entries of
// their own, labelled "Default - <real name>"; they are never the device to look for.
const VIRTUAL_IDS = new Set(['', 'default', 'communications']);

// The entry of `devices` that is the device `want` ({ id, groupId, label } from startInput):
// same id, else same group, else same name. Ids normally survive unplugging, the rest are
// fallbacks. Last, a default entry under exactly the old name (a default with no entry of its
// own, as Chrome's fake device has): once the device is gone the default carries another name.
// Undefined if it is not there.
export function findInput(devices, want) {
  const real = devices.filter((d) => !VIRTUAL_IDS.has(d.deviceId));
  return real.find((d) => !VIRTUAL_IDS.has(want.id) && d.deviceId === want.id)
    ?? (want.groupId && real.find((d) => d.groupId === want.groupId))
    ?? (want.label && real.find((d) => d.label && (d.label === want.label || want.label.endsWith(` - ${d.label}`))))
    ?? (want.label && devices.find((d) => d.label === want.label))
    ?? undefined;
}

export class AudioEngine {
  constructor() {
    this.ctx = null;
    this.input = null;
    this.stopSource = null;
    this.sourceSeq = 0;
    this.multiRes = true;
    this.singleSize = 8192;
    this.monitor = false;
    this.rowRate = 60;
    this.sub = 1;
    this.ring = new Float32Array(RING);
    this.written = 0;   // samples received, absolute
    this.clock = 0;     // row clock in samples, smooth
    this.nextRow = 0;   // absolute sample where the next row's windows end
    this.rowIndex = 0;
    this.bands = new Float32Array(bandCount(1)).fill(-120);
  }

  async ensure() {
    this.setup ??= this.open();
    await this.setup;
    if (this.ctx.state !== 'running') await this.ctx.resume();
  }

  // this.ctx is set only once everything is connected, since the frame loop reads it.
  async open() {
    // Always 48 kHz, whatever the output device runs at: the band axis ends at 24 kHz and the
    // FFT sizes are chosen for this rate. (A USB DAC set to 384 kHz would otherwise shrink the
    // bass window from 170 ms to 21 ms.) The browser resamples to and from the device.
    const ctx = new AudioContext({ latencyHint: 'interactive', sampleRate: SAMPLE_RATE });
    await ctx.audioWorklet.addModule(new URL('./capture-worklet.js', import.meta.url));
    this.input = ctx.createGain();
    this.monitorGain = ctx.createGain();
    this.monitorGain.gain.value = this.monitor ? 1 : 0;
    this.input.connect(this.monitorGain).connect(ctx.destination);
    // Its output is silent; connecting it keeps it running in every browser.
    this.capture = new AudioWorkletNode(ctx, 'capture', { numberOfOutputs: 1, outputChannelCount: [1] });
    this.capture.port.onmessage = (e) => this.receive(e.data);
    this.input.connect(this.capture).connect(ctx.destination);
    this.ctx = ctx;
    this.buildAnalysers();
  }

  receive(block) {
    const mask = RING - 1;
    for (let i = 0; i < block.length; i++) this.ring[(this.written + i) & mask] = block[i];
    this.written += block.length;
  }

  buildAnalysers() {
    const plan = this.multiRes ? SPLITS : [{ fftSize: this.singleSize, hiHz: Infinity }];
    const widen = 2 ** (CROSSFADE_SEMITONES / 12);
    const count = bandCount(this.sub);
    this.bands = new Float32Array(count).fill(-120);
    this.analysers = plan.map(({ fftSize, hiHz }, i) => {
      const loHz = i === 0 ? 0 : plan[i - 1].hiHz;
      const weights = new Float32Array(count);
      for (let k = 0; k < count; k++) {
        const hz = bandHz(k / this.sub);
        weights[k] = (loHz ? ramp(hz, loHz) : 1) * (Number.isFinite(hiHz) ? 1 - ramp(hz, hiHz) : 1);
      }
      const { map, bins } = buildBandMap(this.ctx.sampleRate, fftSize, this.sub, loHz / widen, hiHz * widen);
      return {
        fftSize,
        fft: new RealFFT(fftSize),
        data: new Float32Array(fftSize / 2).fill(-120),
        map,
        bins,
        weights,
        bands: new Float32Array(count).fill(-120),  // the latest analysis
        from: new Float32Array(count).fill(-120),   // the one before, where the glide starts
      };
    });
    this.setRowRate(this.rowRate);
  }

  setResolution(multiRes, singleSize) {
    this.multiRes = multiRes;
    this.singleSize = singleSize;
    if (this.ctx) this.buildAnalysers();
  }

  // Bands per semitone (bands.js SUBDIVISIONS).
  setBands(sub) {
    this.sub = sub;
    if (this.ctx) this.buildAnalysers();
    else this.bands = new Float32Array(bandCount(sub)).fill(-120);
  }

  setRowRate(rate) {
    this.rowRate = rate;
    if (!this.ctx) return;
    const hop = this.ctx.sampleRate / rate;
    for (const a of this.analysers || []) a.every = Math.max(1, Math.floor((a.fftSize * MIN_HOP_OF_WINDOW) / hop));
  }

  setMonitor(on) {
    this.monitor = on;
    if (this.monitorGain) this.monitorGain.gain.setTargetAtTime(on ? 1 : 0, this.ctx.currentTime, 0.02);
  }

  stopActive() {
    if (this.stopSource) this.stopSource();
    this.stopSource = null;
  }

  stop() {
    this.sourceSeq++;
    this.stopActive();
  }

  // Claims a source slot, waits for the AudioContext, and clears the old source if this call
  // is still the latest. Returns null when superseded by a newer start or stop().
  async beginStart() {
    const seq = ++this.sourceSeq;
    const ready = await this.awaitStep(seq, this.ensure().then(() => true));
    if (ready === null) return null;
    this.stopActive();
    return seq;
  }

  async awaitStep(seq, promise, discard) {
    let value;
    try {
      value = await promise;
    } catch (err) {
      if (seq !== this.sourceSeq) return null;
      throw err;
    }
    if (seq !== this.sourceSeq) {
      discard?.(value);
      return null;
    }
    return value;
  }

  // The test tracks of test-audio.js, 1 or 2. Returns null if superseded.
  async startSynth(which = 1) {
    const seq = await this.beginStart();
    if (seq === null) return null;
    this.stopSource = startTestAudio(this.ctx, this.input, which);
    return true;
  }

  // Returns as soon as the browser hands over the stream, with what identifies the device
  // ({ id, groupId, label }) for finding it again, or null if superseded while waiting.
  // The device list is not awaited here: on one phone's Chrome enumerateDevices() never answers
  // (see listInputs). onEnded runs when the track ends by itself, which is what an unplugged
  // device does; stopping it here does not.
  async startInput(deviceId, onEnded) {
    const seq = await this.beginStart();
    if (seq === null) return null;
    const stream = await this.awaitStep(seq, navigator.mediaDevices.getUserMedia({
      audio: {
        deviceId: deviceId ? { exact: deviceId } : undefined,
        echoCancellation: false, noiseSuppression: false, autoGainControl: false, channelCount: 2,
      },
    }), (s) => s.getTracks().forEach((t) => t.stop()));
    if (stream === null) return null;
    const node = this.ctx.createMediaStreamSource(stream);
    node.connect(this.input);
    const track = stream.getAudioTracks()[0];
    const stop = () => { node.disconnect(); stream.getTracks().forEach((t) => t.stop()); };
    this.stopSource = stop;
    track?.addEventListener('ended', () => {
      if (this.stopSource !== stop) return;
      this.stopActive();  // an unplug must not cancel a start the user has just begun
      onEnded?.();
    });
    const s = track?.getSettings?.() ?? {};
    return { id: s.deviceId || deviceId || '', groupId: s.groupId || '', label: track?.label || '' };
  }

  async startFile(file) {
    const seq = await this.beginStart();
    if (seq === null) return null;
    const bytes = await this.awaitStep(seq, file.arrayBuffer());
    if (bytes === null) return null;
    const buffer = await this.awaitStep(seq, this.ctx.decodeAudioData(bytes));
    if (buffer === null) return null;
    const node = this.ctx.createBufferSource();
    node.buffer = buffer;
    node.loop = true;
    node.connect(this.input);
    node.start();
    this.stopSource = () => { node.stop(); node.disconnect(); };
    return buffer.duration;
  }

  // Advances the row clock by one display frame and calls onRow(bands) (dBFS per data band)
  // for every row that came due: one per 1/rowRate s of audio, whatever the display rate.
  // hold (frozen) drops the audio instead. Returns the progress toward the next row, 0..1,
  // for the smooth scroll between rows.
  pull(dt, onRow, hold = false) {
    if (!this.ctx || this.ctx.state !== 'running') return 0;
    const sr = this.ctx.sampleRate;
    const hop = sr / this.rowRate;
    const target = this.written - LAG_S * sr;
    this.clock += dt * sr;
    const err = target - this.clock;
    if (hold || Math.abs(err) > RESYNC_S * sr) this.clock = target;
    else this.clock += err * Math.min(1, dt / SERVO_S);
    this.clock = Math.min(this.clock, this.written);
    if (hold) {
      this.nextRow = this.clock + hop;
      return 0;
    }
    const most = Math.max(8, Math.ceil(this.rowRate * CATCH_UP_S));
    if (this.clock - this.nextRow >= most * hop) this.nextRow = this.clock - (most - 1) * hop;
    while (this.nextRow <= this.clock) {
      this.analyse(Math.round(this.nextRow));
      onRow(this.bands);
      this.nextRow += hop;
    }
    return Math.min(1, Math.max(0, 1 - (this.nextRow - this.clock) / hop));
  }

  // Fills this.bands from the windows that end just before absolute sample `end`.
  analyse(end) {
    const out = this.bands;
    out.fill(0);
    const n = this.rowIndex++;
    for (const a of this.analysers) {
      const step = n % a.every;
      if (step === 0) {
        if (a.every > 1) a.from.set(a.bands);
        a.fft.run(this.ring, end, a.data, a.bins);
        applyBandMap(a.map, a.data, a.bands);
      }
      const f = (step + 1) / a.every;  // 1 when every row is analysed
      for (let k = 0; k < out.length; k++) {
        if (a.weights[k] > 0) out[k] += a.weights[k] * (f === 1 ? a.bands[k] : a.from[k] + (a.bands[k] - a.from[k]) * f);
      }
    }
  }

  // Exact frequency of the strongest bin within about a semitone of `band` (a semitone
  // position), from the last analysis: a parabola through the dB of three bins (good to a
  // few cents on a steady tone with the Blackman window). Uses the FFT that covers the band most.
  peakHz(band) {
    if (!this.analysers?.length || !this.ctx) return null;
    const k = Math.max(0, Math.min(this.bands.length - 1, Math.round(band * this.sub)));
    let a = this.analysers[0];
    for (const x of this.analysers) if (x.weights[k] > a.weights[k]) a = x;
    const hzPerBin = this.ctx.sampleRate / a.fftSize;
    const d = a.data;
    const lo = Math.max(1, Math.floor(bandHz(band - 0.6) / hzPerBin));
    const hi = Math.min(a.bins - 2, Math.ceil(bandHz(band + 0.6) / hzPerBin));
    if (hi < lo) return null;
    let i = lo;
    for (let b = lo + 1; b <= hi; b++) if (d[b] > d[i]) i = b;
    const curve = d[i - 1] - 2 * d[i] + d[i + 1];
    const off = curve < 0 ? Math.max(-0.5, Math.min(0.5, (0.5 * (d[i - 1] - d[i + 1])) / curve)) : 0;
    return Number.isFinite(off) ? (i + off) * hzPerBin : null;
  }

  get info() {
    if (!this.ctx) return null;
    const sr = this.ctx.sampleRate;
    const sizes = this.analysers.map((a) => a.fftSize);
    return {
      sampleRate: sr,
      sizes,
      windowsMs: sizes.map((n) => (1000 * n) / sr),
      outputMs: 1000 * ((this.ctx.baseLatency || 0) + (this.ctx.outputLatency || 0)),
    };
  }
}

// The pad that was test audio 2 until 2026-11-12. No longer in the panel: it stays as the
// source of tests/fixtures/pad.svf (scripts/pad-fixture.mjs), which the JS, Rust and Kotlin
// analysis are checked against.
//
// A slow pad: five held voices (two detuned saws each, plus a sub sine under the bass)
// glide from chord to chord through a slowly sweeping low-pass. The oscillators never stop,
// so the waterfall shows unbroken ridges, and a stalled main thread only delays the next
// chord change instead of leaving a gap.
export function startTestPad(ctx, out) {
  const master = ctx.createGain();
  master.gain.value = 0;
  master.gain.setTargetAtTime(0.42, ctx.currentTime, 0.4);
  master.connect(out);

  const filter = ctx.createBiquadFilter();
  filter.type = 'lowpass';
  filter.Q.value = 4;
  filter.frequency.value = 1500;
  filter.connect(master);
  const lfo = ctx.createOscillator();
  const lfoDepth = ctx.createGain();
  lfo.frequency.value = 0.05;
  lfoDepth.gain.value = 1000;
  lfo.connect(lfoDepth).connect(filter.frequency);
  lfo.start();

  // Am9, Fmaj9, Cmaj9, G6 (MIDI), bass first; four seconds each.
  const chords = [[45, 52, 55, 59, 60], [41, 48, 52, 55, 57], [48, 55, 59, 62, 64], [43, 50, 55, 59, 64]];
  const midiHz = (m) => 440 * 2 ** ((m - 69) / 12);
  const voices = chords[0].map((note, i) => {
    const g = ctx.createGain();
    g.gain.value = i === 0 ? 0.09 : 0.055;
    g.connect(filter);
    const saws = [-7, 7].map((cents) => {
      const o = ctx.createOscillator();
      o.type = 'sawtooth';
      o.detune.value = cents;
      o.frequency.value = midiHz(note);
      o.connect(g);
      o.start();
      return o;
    });
    return saws;
  });
  const sub = ctx.createOscillator();
  const subGain = ctx.createGain();
  subGain.gain.value = 0.22;
  sub.frequency.value = midiHz(chords[0][0] - 12);
  sub.connect(subGain).connect(master);
  sub.start();

  let chord = 0;
  let next = ctx.currentTime + 4;
  const step = () => {
    while (next < ctx.currentTime + 0.5) {
      chord = (chord + 1) % chords.length;
      chords[chord].forEach((note, i) => {
        for (const o of voices[i]) o.frequency.setTargetAtTime(midiHz(note), next, 0.18);
      });
      sub.frequency.setTargetAtTime(midiHz(chords[chord][0] - 12), next, 0.18);
      next += 4;
    }
  };
  const timer = setInterval(step, 100);
  return () => {
    clearInterval(timer);
    for (const o of [...voices.flat(), sub, lfo]) o.stop();
    master.disconnect();
  };
}
