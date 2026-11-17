//! Dependency-free analysis and power-history reference. Input is mono f32 PCM.
pub const LEVELS: usize = 6;
pub const HISTORY_ROWS: usize = 4096;
pub fn band_count(sub: usize) -> usize {
    126 * sub + 1
}
pub fn band_hz(band: f64) -> f64 {
    440.0 * 2f64.powf((band - 57.0) / 12.0)
}
pub fn db_to_byte(db: f64) -> u8 {
    ((db + 120.0) * 2.0).round().clamp(0.0, 255.0) as u8
}
pub fn hz_to_band(hz: f64) -> f64 {
    57.0 + 12.0 * (hz / 440.0).log2()
}
fn mel_of(hz: f64) -> f64 {
    (1.0 + hz / 700.0).ln()
}
/// Where a semitone position sits across the frequency axis, 0..1. `mel` 0 gives every
/// semitone the same width, 1 is the mel scale between the same two end frequencies, and
/// in between the two are blended. Same mapping as web/src/bands.js `bandToU` and
/// `bandToU` in web/shaders/common.glsl.
pub fn band_to_u(band: f64, mel: f64) -> f64 {
    let lin = band / 126.0;
    if mel == 0.0 {
        return lin;
    }
    let (lo, hi) = (mel_of(band_hz(0.0)), mel_of(band_hz(126.0)));
    lin + ((mel_of(band_hz(band)) - lo) / (hi - lo) - lin) * mel
}
/// The inverse of `band_to_u`, by bisection (the blend has no closed form).
pub fn u_to_band(u: f64, mel: f64) -> f64 {
    if mel == 0.0 {
        return u * 126.0;
    }
    let (mut lo, mut hi) = (0.0, 126.0);
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if band_to_u(mid, mel) < u {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}
/// The envelope the live spectrum follows (web/src/main.js `envelope`): a running maximum
/// over `r` data bands each side, then a triangular average of the same width. A peak keeps
/// its exact level; the narrow dips between neighbouring partials fill in. `r` 0 copies.
pub fn envelope(src: &[f32], dst: &mut [f32], scratch: &mut [f32], r: usize) {
    let n = src.len();
    assert!(dst.len() == n && scratch.len() == n);
    if r < 1 || n == 0 {
        dst.copy_from_slice(src);
        return;
    }
    for k in 0..n {
        scratch[k] = src[k.saturating_sub(r)..=(k + r).min(n - 1)]
            .iter()
            .copied()
            .fold(src[k], f32::max);
    }
    let r = r as isize;
    for k in 0..n as isize {
        let (mut sum, mut weights) = (0.0f64, 0.0f64);
        for i in -r..=r {
            let w = (r + 1 - i.abs()) as f64;
            sum += w * scratch[(k + i).clamp(0, n as isize - 1) as usize] as f64;
            weights += w;
        }
        dst[k as usize] = (sum / weights) as f32;
    }
}

pub struct Fft {
    n: usize,
    re: Vec<f64>,
    im: Vec<f64>,
    window: Vec<f64>,
    reverse: Vec<usize>,
}
impl Fft {
    pub fn new(n: usize) -> Self {
        assert!(n >= 4 && n.is_power_of_two());
        let bits = n.trailing_zeros();
        Self {
            n,
            re: vec![0.0; n],
            im: vec![0.0; n],
            window: (0..n)
                .map(|i| {
                    let x = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
                    0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
                })
                .collect(),
            reverse: (0..n)
                .map(|i| i.reverse_bits() >> (usize::BITS - bits))
                .collect(),
        }
    }
    pub fn run(&mut self, ring: &[f32], end: usize, out: &mut [f32], bins: usize) {
        let mask = ring.len() - 1;
        for i in 0..self.n {
            let j = self.reverse[i];
            self.re[j] =
                ring[end.wrapping_sub(self.n).wrapping_add(i) & mask] as f64 * self.window[i];
            self.im[j] = 0.0;
        }
        let mut size = 2;
        while size <= self.n {
            let half = size / 2;
            for j in 0..half {
                let angle = -2.0 * std::f64::consts::PI * j as f64 / size as f64;
                let (wi, wr) = angle.sin_cos();
                for i in (0..self.n).step_by(size) {
                    let a = i + j;
                    let b = a + half;
                    let tr = self.re[b] * wr - self.im[b] * wi;
                    let ti = self.re[b] * wi + self.im[b] * wr;
                    self.re[b] = self.re[a] - tr;
                    self.im[b] = self.im[a] - ti;
                    self.re[a] += tr;
                    self.im[a] += ti;
                }
            }
            size *= 2;
        }
        let scale = (2.0 / (self.n as f64 * 0.42)).powi(2);
        for (k, v) in out.iter_mut().enumerate().take(bins) {
            *v = (10.0
                * ((self.re[k] * self.re[k] + self.im[k] * self.im[k]) * scale + 1e-30).log10())
                as f32;
        }
    }
}
#[derive(Clone, Copy)]
enum Band {
    Empty,
    Range(usize, usize),
    Position(f64),
}
fn band_map(sr: f64, n: usize, sub: usize, lo: f64, hi: f64) -> (Vec<Band>, usize) {
    let width = sr / n as f64;
    let half = 2f64.powf(1.0 / (24 * sub) as f64);
    let mut top = 0;
    let map = (0..band_count(sub))
        .map(|k| {
            let fc = band_hz(k as f64 / sub as f64);
            if fc < lo || fc >= hi || fc / half >= sr / 2.0 {
                return Band::Empty;
            }
            let b0 = (fc / half / width).ceil() as usize;
            let b1 = ((fc * half / width).floor() as usize).min(n / 2 - 1);
            if b1 >= b0 && b0 >= 1 {
                top = top.max(b1);
                Band::Range(b0, b1)
            } else {
                let pos = (fc / width).min((n / 2 - 1) as f64);
                top = top.max((pos.floor() as usize + 2).min(n / 2 - 1));
                Band::Position(pos)
            }
        })
        .collect();
    (map, top + 1)
}
fn apply(map: &[Band], data: &[f32], out: &mut [f32]) {
    for (m, v) in map.iter().zip(out) {
        let db = match *m {
            Band::Empty => -120.0,
            Band::Range(a, b) => data[a..=b]
                .iter()
                .map(|v| *v as f64)
                .fold(f64::NEG_INFINITY, f64::max),
            Band::Position(p) => {
                let i = p.floor() as usize;
                let f = p - i as f64;
                let last = data.len() - 1;
                let a = data[i.saturating_sub(1)] as f64;
                let b = data[i] as f64;
                let c = data[(i + 1).min(last)] as f64;
                let d = data[(i + 2).min(last)] as f64;
                b + 0.5
                    * f
                    * (c - a + f * (2.0 * a - 5.0 * b + 4.0 * c - d + f * (3.0 * (b - c) + d - a)))
            }
        };
        *v = if db.is_finite() {
            db.clamp(-120.0, 0.0) as f32
        } else {
            -120.0
        };
    }
}
struct Analyser {
    fft: Fft,
    data: Vec<f32>,
    map: Vec<Band>,
    bins: usize,
    weights: Vec<f32>,
    bands: Vec<f32>,
    /// The analysis before `bands`, where the glide between two analyses starts.
    from: Vec<f32>,
    every: usize,
    last_run: Option<usize>,
}
pub struct Analysis {
    pub sample_rate: u32,
    pub row_rate: u32,
    pub sub: usize,
    pub written: usize,
    pub row_index: usize,
    ring: Vec<f32>,
    analysers: Vec<Analyser>,
    pub bands: Vec<f32>,
}
impl Analysis {
    pub fn new(sample_rate: u32, row_rate: u32, sub: usize) -> Self {
        assert!([50, 60, 120, 240, 480].contains(&row_rate));
        assert!([1, 2, 4, 8].contains(&sub));
        let splits = [(8192, 350.0), (2048, 1500.0), (1024, f64::INFINITY)];
        let count = band_count(sub);
        let mut lo = 0.0;
        let ramp = |hz: f64, edge: f64| ((12.0 * (hz / edge).log2() + 2.0) / 4.0).clamp(0.0, 1.0);
        let analysers = splits
            .into_iter()
            .map(|(n, hi)| {
                let widen = 2f64.powf(2.0 / 12.0);
                let (map, bins) = band_map(sample_rate as f64, n, sub, lo / widen, hi * widen);
                let weights = (0..count)
                    .map(|k| {
                        let hz = band_hz(k as f64 / sub as f64);
                        ((if lo > 0.0 { ramp(hz, lo) } else { 1.0 })
                            * (if hi.is_finite() {
                                1.0 - ramp(hz, hi)
                            } else {
                                1.0
                            })) as f32
                    })
                    .collect();
                lo = hi;
                Analyser {
                    fft: Fft::new(n),
                    data: vec![-120.0; n / 2],
                    map,
                    bins,
                    weights,
                    bands: vec![-120.0; count],
                    from: vec![-120.0; count],
                    every: ((n as f64 / 16.0) / (sample_rate as f64 / row_rate as f64))
                        .floor()
                        .max(1.0) as usize,
                    last_run: None,
                }
            })
            .collect();
        Self {
            sample_rate,
            row_rate,
            sub,
            written: 0,
            row_index: 0,
            ring: vec![0.0; 32768],
            analysers,
            bands: vec![-120.0; count],
        }
    }
    pub fn receive(&mut self, pcm: &[f32]) {
        for v in pcm {
            self.ring[self.written & 32767] = *v;
            self.written += 1;
        }
    }
    pub fn analyse(&mut self, end: usize) -> &[f32] {
        self.analyse_with_refresh(end, true)
    }
    /// Emit one audio-clock row. Under load, repeat the preceding row without running
    /// an FFT; a due analyser runs on the next refreshed row instead of losing its turn.
    pub fn analyse_with_refresh(&mut self, end: usize, refresh: bool) -> &[f32] {
        if !refresh {
            self.row_index += 1;
            return &self.bands;
        }
        self.bands.fill(0.0);
        for a in &mut self.analysers {
            if a.last_run
                .is_none_or(|last| self.row_index - last >= a.every)
            {
                if a.every > 1 {
                    a.from.copy_from_slice(&a.bands);
                }
                a.fft.run(&self.ring, end, &mut a.data, a.bins);
                apply(&a.map, &a.data, &mut a.bands);
                a.last_run = Some(self.row_index);
            }
            // Rows between two analyses of a long window glide from the previous one to the
            // new one (audio.js), instead of holding and then jumping.
            let since = self.row_index - a.last_run.unwrap_or(self.row_index);
            let f = ((since + 1) as f64 / a.every as f64).min(1.0);
            for (((v, w), db), from) in self
                .bands
                .iter_mut()
                .zip(&a.weights)
                .zip(&a.bands)
                .zip(&a.from)
            {
                if *w > 0.0 {
                    let db = if f >= 1.0 {
                        *db as f64
                    } else {
                        *from as f64 + (*db as f64 - *from as f64) * f
                    };
                    *v = (*v as f64 + *w as f64 * db) as f32;
                }
            }
        }
        self.row_index += 1;
        &self.bands
    }
    pub fn peak_hz(&self, band: f64) -> Option<f64> {
        let k = ((band * self.sub as f64).round() as usize).min(self.bands.len() - 1);
        let a = self
            .analysers
            .iter()
            .max_by(|a, b| a.weights[k].total_cmp(&b.weights[k]))?;
        let width = self.sample_rate as f64 / a.fft.n as f64;
        let lo = (band_hz(band - 0.6) / width).floor().max(1.0) as usize;
        let hi = (band_hz(band + 0.6) / width)
            .ceil()
            .min((a.bins - 2) as f64) as usize;
        if hi < lo {
            return None;
        }
        let i = (lo..=hi).max_by(|&i, &j| a.data[i].total_cmp(&a.data[j]))?;
        let curve = a.data[i - 1] as f64 - 2.0 * a.data[i] as f64 + a.data[i + 1] as f64;
        let off = if curve < 0.0 {
            (0.5 * (a.data[i - 1] as f64 - a.data[i + 1] as f64) / curve).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        Some((i as f64 + off) * width)
    }
}
pub struct History {
    pub count: usize,
    pub rows: usize,
    pub written: usize,
    pub levels: Vec<Vec<u8>>,
    sums: Vec<Vec<f32>>,
    power: [f32; 256],
}
impl History {
    pub fn new(count: usize, rows: usize) -> Self {
        Self {
            count,
            rows,
            written: 0,
            levels: vec![vec![0; count * rows]; LEVELS],
            sums: vec![vec![0.0; count]; LEVELS],
            power: std::array::from_fn(|b| 10f64.powf((b as f64 / 2.0 - 120.0) / 10.0) as f32),
        }
    }
    pub fn push(&mut self, bytes: &[u8]) {
        assert_eq!(bytes.len(), self.count);
        let n = self.written;
        self.written += 1;
        let base = (n % self.rows) * self.count;
        self.levels[0][base..base + self.count].copy_from_slice(bytes);
        for l in 1..LEVELS {
            let span = 1 << l;
            for (k, b) in bytes.iter().enumerate() {
                self.sums[l][k] += self.power[*b as usize];
            }
            if (n + 1) % span != 0 {
                continue;
            }
            let base = (n / span % self.rows) * self.count;
            for k in 0..self.count {
                self.levels[l][base + k] =
                    db_to_byte(10.0 * (self.sums[l][k] as f64 / span as f64 + 1e-30).log10());
                self.sums[l][k] = 0.0;
            }
        }
    }
    pub fn sample(&self, band: f64, age: usize, sub: usize) -> f64 {
        let mut l = 0;
        while l < LEVELS - 1 && age >= (self.rows - 2) * (1 << l) {
            l += 1;
        }
        if age >= self.written {
            return -120.0;
        }
        let row = self.written - 1 - age;
        let k = ((band * sub as f64).round() as usize).min(self.count - 1);
        self.levels[l][(row / (1 << l) % self.rows) * self.count + k] as f64 / 2.0 - 120.0
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn u32le(data: &[u8], p: &mut usize) -> usize {
        let n = u32::from_le_bytes(data[*p..*p + 4].try_into().unwrap()) as usize;
        *p += 4;
        n
    }
    #[test]
    fn javascript_reference_scenarios() {
        for name in ["tone", "sweep", "noise", "chord", "pad"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../tests/fixtures/{name}.svf"));
            let data = std::fs::read(path).unwrap();
            assert_eq!(&data[..4], b"SVF1");
            let mut p = 4;
            let sr = u32le(&data, &mut p);
            let len = u32le(&data, &mut p);
            let cases = u32le(&data, &mut p);
            let pcm: Vec<f32> = (0..len)
                .map(|_| {
                    let v = f32::from_le_bytes(data[p..p + 4].try_into().unwrap());
                    p += 4;
                    v
                })
                .collect();
            for _ in 0..cases {
                let rate = u32le(&data, &mut p);
                let sub = u32le(&data, &mut p);
                let rows = u32le(&data, &mut p);
                let count = u32le(&data, &mut p);
                let mut a = Analysis::new(sr as u32, rate as u32, sub);
                let mut h = History::new(count, HISTORY_ROWS);
                for row in 0..rows {
                    let end = (row + 1) * sr / rate;
                    a.receive(&pcm[a.written..end]);
                    let bytes: Vec<_> = a
                        .analyse(end)
                        .iter()
                        .map(|v| db_to_byte(*v as f64))
                        .collect();
                    for (k, b) in bytes.iter().enumerate() {
                        assert!(
                            (*b as i32 - data[p + k] as i32).abs() <= 1,
                            "{name} {rate}x{sub} row {row} band {k}: {b} vs {}",
                            data[p + k]
                        );
                    }
                    p += count;
                    h.push(&bytes);
                }
                if name == "tone" {
                    let hz = a.peak_hz(57.25).unwrap();
                    assert!(
                        (1200.0 * (hz / (440.0 * 2f64.powf(25.0 / 1200.0))).log2()).abs() < 1.0
                    );
                }
                assert_eq!(h.written, rows);
            }
            assert_eq!(p, data.len());
        }
    }
    #[test]
    fn history_power_mean_and_wrap() {
        let mut h = History::new(1, 8);
        for i in 0..160 {
            h.push(&[if i % 2 == 0 { 240 } else { 0 }]);
        }
        assert_eq!(h.levels[1], vec![234; 8]);
        assert_eq!(h.sample(0.0, 0, 1), -120.0);
        assert_eq!(h.sample(0.0, 1, 1), 0.0);
    }
}

/// Display-independent audio row clock, with bounded catch-up and frozen-input dropping.
pub struct RowClock {
    pub sample_rate: f64,
    pub row_rate: f64,
    pub clock: f64,
    pub next: f64,
}
impl RowClock {
    pub fn new(sample_rate: u32, row_rate: u32) -> Self {
        assert!(sample_rate > 0 && row_rate > 0);
        Self {
            sample_rate: sample_rate as f64,
            row_rate: row_rate as f64,
            clock: 0.0,
            next: 0.0,
        }
    }
    /// Next exact audio-sample boundary. Unlike `pull`, these methods have no
    /// display latency or wall-clock correction; use them on the analysis thread.
    pub fn next_sample(&self) -> usize {
        self.next.max(self.sample_rate / self.row_rate).round() as usize
    }
    pub fn consume_samples(&mut self, written: u64) -> Vec<usize> {
        let hop = self.sample_rate / self.row_rate;
        self.next = self.next.max(hop);
        let mut rows = Vec::new();
        while self.next.round() <= written as f64 {
            rows.push(self.next.round() as usize);
            self.next += hop;
        }
        self.clock = written as f64;
        rows
    }
    pub fn sample_phase(&self, written: u64) -> f64 {
        let hop = self.sample_rate / self.row_rate;
        (1.0 - (self.next.max(hop) - written as f64) / hop).clamp(0.0, 1.0)
    }
    pub fn pull(&mut self, dt: f64, written: u64, hold: bool) -> (Vec<usize>, f64) {
        let hop = self.sample_rate / self.row_rate;
        let target = written as f64 - 0.004 * self.sample_rate;
        self.clock += dt * self.sample_rate;
        let error = target - self.clock;
        if hold || error.abs() > 0.05 * self.sample_rate {
            self.clock = target;
        } else {
            self.clock += error * (dt / 0.5).min(1.0);
        }
        self.clock = self.clock.min(written as f64);
        if hold {
            self.next = self.clock + hop;
            return (vec![], 0.0);
        }
        let most = (self.row_rate * 0.1).ceil().max(8.0);
        if self.clock - self.next >= most * hop {
            self.next = self.clock - (most - 1.0) * hop;
        }
        let mut rows = vec![];
        while self.next <= self.clock {
            rows.push(self.next.round().max(0.0) as usize);
            self.next += hop;
        }
        (rows, (1.0 - (self.next - self.clock) / hop).clamp(0.0, 1.0))
    }
}
#[cfg(test)]
mod clock_test {
    use super::*;
    #[test]
    fn accurate_clock_bounded_stalls() {
        for rate in [60, 120, 240, 480] {
            let mut c = RowClock::new(48000, rate);
            let mut count = 0;
            for frame in 1..=600 {
                count += c.pull(1.0 / 60.0, frame * 800, false).0.len();
            }
            assert!((count as i64 - rate as i64 * 10).abs() <= 1);
            assert!(
                c.pull(0.1, 528000, false).0.len() <= (rate as f64 * 0.1).ceil().max(8.0) as usize
            );
            assert!(c.pull(0.1, 576000, true).0.is_empty());
        }
    }
}

/// Dynamics shared by desktop display and any future front ends. These are audio
/// display response constants, independent of the visual look and its dB floor.
#[derive(Clone, Copy, Debug)]
pub struct DisplayDynamics {
    pub release_db_per_s: f32,
    pub peak_hold_s: f32,
    pub peak_fall_db_per_s: f32,
}
impl Default for DisplayDynamics {
    fn default() -> Self {
        Self {
            release_db_per_s: 36.0,
            peak_hold_s: 1.5,
            peak_fall_db_per_s: 18.0,
        }
    }
}

/// Attack is immediate, release is linear in dB, the peak line holds then falls,
/// and the maximum line lasts until explicitly cleared. Update on the audio row
/// clock so a short transient survives between display frames.
///
/// All of them follow `live`, the envelope of the row (see `envelope`), not the row
/// itself; the history keeps the raw rows. The envelope spans half a semitone each side,
/// so with one band per semitone there is none.
#[derive(Clone, Debug)]
pub struct DisplaySpectrum {
    pub raw: Vec<f32>,
    pub live: Vec<f32>,
    pub shown: Vec<f32>,
    pub peak: Vec<f32>,
    pub maximum: Vec<f32>,
    peak_age: Vec<f32>,
    scratch: Vec<f32>,
    /// Envelope radius in data bands: half a semitone.
    pub smooth: usize,
    pub dynamics: DisplayDynamics,
}
impl DisplaySpectrum {
    pub fn new(count: usize) -> Self {
        Self {
            raw: vec![-120.0; count],
            live: vec![-120.0; count],
            shown: vec![-120.0; count],
            peak: vec![-120.0; count],
            maximum: vec![-120.0; count],
            peak_age: vec![0.0; count],
            scratch: vec![-120.0; count],
            smooth: count.saturating_sub(1) / 126 / 2,
            dynamics: DisplayDynamics::default(),
        }
    }
    pub fn update(&mut self, row: &[f32], dt: f64) {
        assert_eq!(row.len(), self.shown.len());
        let dt = if dt.is_finite() {
            dt.max(0.0) as f32
        } else {
            0.0
        };
        for (raw, value) in self.raw.iter_mut().zip(row) {
            *raw = if value.is_finite() {
                value.clamp(-120.0, 0.0)
            } else {
                -120.0
            };
        }
        envelope(&self.raw, &mut self.live, &mut self.scratch, self.smooth);
        for k in 0..self.live.len() {
            let value = self.live[k];
            self.shown[k] = value.max(self.shown[k] - self.dynamics.release_db_per_s * dt);
            if value >= self.peak[k] {
                self.peak[k] = value;
                self.peak_age[k] = 0.0;
            } else {
                self.peak_age[k] += dt;
                if self.peak_age[k] > self.dynamics.peak_hold_s {
                    self.peak[k] =
                        self.shown[k].max(self.peak[k] - self.dynamics.peak_fall_db_per_s * dt);
                }
            }
            self.maximum[k] = self.maximum[k].max(self.shown[k]);
        }
    }
    pub fn clear_maximum(&mut self) {
        self.maximum.fill(-120.0);
    }
    pub fn clear(&mut self) {
        self.raw.fill(-120.0);
        self.live.fill(-120.0);
        self.shown.fill(-120.0);
        self.peak.fill(-120.0);
        self.peak_age.fill(0.0);
        self.clear_maximum();
    }
}

/// One tracked main peak of the live spectrum (web/src/peaks.js).
#[derive(Clone, Debug, PartialEq)]
pub struct PeakTrack {
    /// Semitones from C0, smoothed from frame to frame.
    pub band: f64,
    pub db: f64,
    pub hz: f64,
    /// 0..1, fading in while the peak is found and out once it is gone.
    pub alpha: f64,
    /// Note name, signed cents, then frequency and level. Refreshed at most 8 times a second.
    pub text: Option<[String; 3]>,
    matched: bool,
}
/// Main peaks of the live spectrum, tracked from frame to frame so their labels stay put
/// while a note holds. A port of `PeakTracker` in web/src/peaks.js with the same constants;
/// call it once per display frame (its smoothing is per call, as on the web).
#[derive(Clone, Debug, Default)]
pub struct PeakTracker {
    pub tracks: Vec<PeakTrack>,
    text_clock: f64,
}
pub const PEAK_LABELS: usize = 5;
const PEAK_PROMINENCE_DB: f64 = 6.0;
const PEAK_RANGE_DB: f64 = 40.0;
const PEAK_FLOOR_DB: f64 = -84.0;
const PEAK_MATCH_BANDS: f64 = 1.5;
const PEAK_FADE_S: f64 = 0.16;
const PEAK_TEXT_EVERY_S: f64 = 0.125;
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}
fn find_peaks(db: &[f32], sub: usize) -> Vec<(f64, f64)> {
    let n = db.len();
    let top = db.iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64;
    let floor = PEAK_FLOOR_DB.max(top - PEAK_RANGE_DB);
    let mut out = Vec::new();
    for k in 1..n.saturating_sub(1) {
        let v = db[k];
        if (v as f64) < floor || v < db[k - 1] || v <= db[k + 1] {
            continue;
        }
        let left = db[..k]
            .iter()
            .rev()
            .take_while(|x| **x <= v)
            .copied()
            .fold(v, f32::min);
        let right = db[k + 1..]
            .iter()
            .take_while(|x| **x <= v)
            .copied()
            .fold(v, f32::min);
        if ((v - left.max(right)) as f64) < PEAK_PROMINENCE_DB {
            continue;
        }
        let (a, v, c) = (db[k - 1] as f64, v as f64, db[k + 1] as f64);
        let d = a - 2.0 * v + c;
        let off = if d < 0.0 {
            (0.5 * (a - c) / d).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        out.push(((k as f64 + off) / sub as f64, v - 0.25 * (a - c) * off));
    }
    out.sort_by(|p, q| q.1.total_cmp(&p.1));
    out.truncate(PEAK_LABELS + 3);
    out
}
/// Note name and cents of a frequency, as web/src/bands.js `noteName`.
pub fn note_name(hz: f64) -> (String, i32) {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let exact = 69.0 + 12.0 * (hz / 440.0).log2();
    let midi = js_round(exact) as i32;
    let cents = js_round((exact - midi as f64) * 100.0) as i32;
    (
        format!(
            "{}{}",
            NAMES[midi.rem_euclid(12) as usize],
            midi.div_euclid(12) - 1
        ),
        cents,
    )
}
fn peak_text(hz: f64, db: f64) -> [String; 3] {
    let (name, cents) = note_name(hz);
    let cents = match cents {
        c if c > 0 => format!("+{c}¢"),
        c if c < 0 => format!("−{}¢", -c),
        _ => "±0¢".into(),
    };
    let hz = if hz >= 1000.0 {
        format!(
            "{:.*}\u{202f}kHz",
            if hz >= 10000.0 { 1 } else { 2 },
            hz / 1000.0
        )
    } else {
        format!("{:.*}\u{202f}Hz", if hz < 100.0 { 1 } else { 0 }, hz)
    };
    let db = if db < -0.5 {
        format!("−{}", js_round(-db))
    } else {
        "0".into()
    };
    [name, cents, format!("{hz}  {db}\u{202f}dB")]
}
impl PeakTracker {
    pub fn clear(&mut self) {
        self.tracks.clear();
    }
    /// `shown`: dB per data band as displayed. `raw`: the latest analysis, to tell whether a
    /// peak is still sounding or only falling on the display's release. `refine_hz(band)`:
    /// exact peak frequency near a semitone position, if the FFT bins are at hand.
    pub fn update(
        &mut self,
        shown: &[f32],
        raw: &[f32],
        dt: f64,
        refine_hz: impl Fn(f64) -> Option<f64>,
        sub: usize,
    ) {
        for t in &mut self.tracks {
            t.matched = false;
        }
        for (band, db) in find_peaks(shown, sub) {
            let mut best = None;
            let mut best_dist = PEAK_MATCH_BANDS;
            for (i, t) in self.tracks.iter().enumerate() {
                let dist = (t.band - band).abs();
                if !t.matched && dist < best_dist {
                    best = Some(i);
                    best_dist = dist;
                }
            }
            let i = best.unwrap_or_else(|| {
                self.tracks.push(PeakTrack {
                    band,
                    db,
                    hz: band_hz(band),
                    alpha: 0.0,
                    text: None,
                    matched: false,
                });
                self.tracks.len() - 1
            });
            let t = &mut self.tracks[i];
            t.matched = true;
            t.band += (band - t.band) * 0.5;
            t.db = db;
            let k = (js_round(band * sub as f64) as usize).min(shown.len() - 1);
            if raw[k] > shown[k] - 6.0 {
                let hz = refine_hz(band).unwrap_or_else(|| band_hz(band));
                if hz > 0.0 && (12.0 * (hz / band_hz(band)).log2()).abs() < 1.0 {
                    t.hz = if t.text.is_some() {
                        t.hz * (hz / t.hz).powf(0.4)
                    } else {
                        hz
                    };
                }
            }
        }
        for t in &mut self.tracks {
            let step = if t.matched { dt } else { -dt };
            t.alpha = (t.alpha + step / PEAK_FADE_S).clamp(0.0, 1.0);
        }
        self.tracks.retain(|t| t.matched || t.alpha > 0.0);
        self.text_clock += dt;
        let refresh = self.text_clock >= PEAK_TEXT_EVERY_S;
        if refresh {
            self.text_clock = 0.0;
        }
        for t in &mut self.tracks {
            if t.text.is_none() || refresh {
                t.text = Some(peak_text(t.hz, t.db));
            }
        }
    }
}

/// The deterministic five-tone chord used by the JS, Kotlin and GPU checks.
/// One generator avoids subtly different signals in live and offscreen modes.
pub struct TestSignal {
    sample_rate: u32,
    pub written: u64,
}
impl TestSignal {
    pub fn new(sample_rate: u32) -> Self {
        assert!(sample_rate > 0);
        Self {
            sample_rate,
            written: 0,
        }
    }
    pub fn sample_at(sample: u64, sample_rate: u32) -> f32 {
        let t = sample as f64 / sample_rate as f64;
        [110.0, 164.813778, 195.997718, 246.94165, 261.62556]
            .into_iter()
            .map(|f| 0.09 * (std::f64::consts::TAU * f * t).sin())
            .sum::<f64>() as f32
    }
    pub fn fill(&mut self, output: &mut [f32]) {
        for value in output {
            *value = Self::sample_at(self.written, self.sample_rate);
            self.written += 1;
        }
    }
}

/// Adaptive and fixed render scale controller matching `web/src/render-scale.js` (plus manual `0.5`).
#[derive(Clone, Debug)]
pub struct RenderScale {
    mode: String,
    index: usize,
    scale: f64,
    slow: f64,
    fast: f64,
    avg: f64,
    trial: Option<RenderTrial>,
    hold: f64,
    hold_next: f64,
    penalty: [f64; 3],
}

#[derive(Clone, Debug)]
struct RenderTrial {
    lowered: bool,
    from: usize,
    before: f64,
    time: f64,
    sum: f64,
    n: usize,
}

impl RenderScale {
    pub const AUTO_SCALES: [f64; 3] = [1.0, 0.85, 0.7];
    pub const SLOW_DT: f64 = 1.0 / 48.0;
    pub const FAST_DT: f64 = 1.0 / 55.0;
    pub const TRIAL_S: f64 = 5.0;
    pub const UP_TRIAL_S: f64 = 7.0;
    pub const BUDGET_MS: f64 = 13.0;

    /// Builds a scale controller from `"auto"`, `"1"`, `"0.85"`, `"0.7"`, or `"0.5"`.
    pub fn new(mode: &str) -> Self {
        let mut out = Self {
            mode: "1".into(),
            index: 0,
            scale: 1.0,
            slow: 0.0,
            fast: 0.0,
            avg: 1.0 / 60.0,
            trial: None,
            hold: 0.0,
            hold_next: 60.0,
            penalty: [1.0; 3],
        };
        out.set_mode(mode);
        out
    }

    pub fn is_valid_mode(mode: &str) -> bool {
        matches!(mode, "auto" | "1" | "0.85" | "0.7" | "0.5")
    }

    pub fn set_mode(&mut self, mode: &str) {
        self.mode = if Self::is_valid_mode(mode) {
            mode.into()
        } else {
            "1".into()
        };
        self.index = 0;
        self.scale = match self.mode.as_str() {
            "auto" | "1" => 1.0,
            "0.85" => 0.85,
            "0.7" => 0.7,
            "0.5" => 0.5,
            _ => 1.0,
        };
        self.slow = 0.0;
        self.fast = 0.0;
        self.avg = 1.0 / 60.0;
        self.trial = None;
        self.hold = 0.0;
        self.hold_next = 60.0;
        self.penalty = [1.0; 3];
    }

    pub fn mode(&self) -> &str {
        &self.mode
    }

    pub fn is_auto(&self) -> bool {
        self.mode == "auto"
    }

    pub fn scale(&self) -> f32 {
        self.scale as f32
    }

    pub fn scale_f64(&self) -> f64 {
        self.scale
    }

    fn set_index(&mut self, index: usize) {
        self.index = index;
        self.scale = Self::AUTO_SCALES[index];
        self.slow = 0.0;
        self.fast = 0.0;
    }

    /// Updates automatic scaling from frame `dt` (seconds) and optional GPU time `gpu_ms`.
    /// Returns `true` when the active scale changed.
    pub fn update(&mut self, dt: f64, gpu_ms: Option<f64>, active: bool) -> bool {
        if self.mode != "auto" || !active || !dt.is_finite() || dt <= 0.0 || dt > 0.1 {
            self.slow = 0.0;
            self.fast = 0.0;
            return false;
        }
        self.avg += (dt - self.avg) * dt.min(1.0);
        self.hold = (self.hold - dt).max(0.0);
        let gpu_bound = gpu_ms.map_or(true, |ms| ms >= dt * 1000.0 * 0.5);
        let slow = dt > Self::SLOW_DT && gpu_bound;
        let up = if self.index > 0 {
            let ratio = Self::AUTO_SCALES[self.index - 1] / Self::AUTO_SCALES[self.index];
            ratio * ratio
        } else {
            0.0
        };
        let fast = dt < Self::FAST_DT && gpu_ms.map_or(true, |ms| ms * up < Self::BUDGET_MS);
        self.slow = if slow { self.slow + dt } else { 0.0 };
        self.fast = if fast { self.fast + dt } else { 0.0 };

        if let Some(t) = self.trial.as_mut() {
            t.time += dt;
            if t.lowered {
                t.sum += dt;
                t.n += 1;
                if t.time < Self::TRIAL_S {
                    return false;
                }
                let mean = t.sum / t.n as f64;
                let before = t.before;
                let from = t.from;
                self.trial = None;
                if mean > Self::SLOW_DT && mean > before * 0.9 {
                    self.set_index(from);
                    self.hold = self.hold_next;
                    self.hold_next = (self.hold_next * 2.0).min(600.0);
                    return true;
                }
            } else if t.time >= Self::UP_TRIAL_S {
                self.trial = None;
            }
        }

        if self.slow >= 2.0 && self.index < Self::AUTO_SCALES.len() - 1 && self.hold == 0.0 {
            if self.trial.is_some() {
                self.penalty[self.index] = (self.penalty[self.index] * 2.0).min(32.0);
            }
            let from = self.index;
            self.set_index(from + 1);
            self.trial = Some(RenderTrial {
                lowered: true,
                from,
                before: self.avg,
                time: 0.0,
                sum: 0.0,
                n: 0,
            });
            return true;
        }
        if self.trial.is_none() && self.index > 0 {
            let wait = if gpu_ms.is_none() { 30.0 } else { 10.0 } * self.penalty[self.index - 1];
            if self.fast >= wait {
                self.set_index(self.index - 1);
                self.trial = Some(RenderTrial {
                    lowered: false,
                    from: 0,
                    before: 0.0,
                    time: 0.0,
                    sum: 0.0,
                    n: 0,
                });
                return true;
            }
        }
        false
    }
}

pub const CHROMA_WIDTH: usize = 8;
const DADASPEC_TILT_DB: [f64; 3] = [0.0, 5.5, 11.5];

/// Six-level tri-band perceptual chroma & onset flux pyramid for the Dadaspec style
/// (`web/styles/dadaspec/renderer.js`). Each level `l` occupies column `x = l` of an
/// `8 x 4096` RGBA8 ring texture.
#[derive(Clone, Debug)]
pub struct DadaspecChroma {
    pub count: usize,
    pub crossovers: [f64; 2],
    zone_map: Vec<u8>,
    zone_weight: Vec<f32>,
    zone_denom: [f64; 3],
    pub levels: [Vec<u8>; LEVELS],
    pub dirty: [usize; LEVELS],
    band_power_sums: [[f64; 3]; LEVELS],
    flux_peaks: [f32; LEVELS],
    band_ceil: [f32; 3],
    flux_env_db: f64,
    pub live_chroma: [f32; 4],
    power: [f32; 256],
}

impl DadaspecChroma {
    pub fn new(sub: usize, crossovers: [f64; 2]) -> Self {
        assert!(sub >= 1);
        let count = band_count(sub);
        let [lo_split, hi_split] = crossovers;
        let mut zone_map = Vec::with_capacity(count);
        let mut zone_weight = Vec::with_capacity(count);
        let mut zone_denom = [0.0f64; 3];
        for i in 0..count {
            let hz = band_hz(i as f64 / sub as f64);
            let z = if hz < lo_split {
                0
            } else if hz < hi_split {
                1
            } else {
                2
            };
            let w = if hz > 16500.0 { 0.30_f32 } else { 1.0_f32 };
            zone_map.push(z);
            zone_weight.push(w);
            zone_denom[z as usize] += w as f64;
        }
        for d in &mut zone_denom {
            *d = (*d).max(1e-6);
        }
        Self {
            count,
            crossovers,
            zone_map,
            zone_weight,
            zone_denom,
            levels: std::array::from_fn(|_| vec![0u8; HISTORY_ROWS * 4]),
            dirty: [0; LEVELS],
            band_power_sums: [[0.0; 3]; LEVELS],
            flux_peaks: [0.0; LEVELS],
            band_ceil: [-18.0, -23.5, -29.5],
            flux_env_db: -120.0,
            live_chroma: [0.0; 4],
            power: std::array::from_fn(|b| 10f64.powf((b as f64 / 2.0 - 120.0) / 10.0) as f32),
        }
    }

    pub fn clear(&mut self) {
        for l in 0..LEVELS {
            self.levels[l].fill(0);
            self.band_power_sums[l] = [0.0; 3];
            self.flux_peaks[l] = 0.0;
            self.dirty[l] = HISTORY_ROWS;
        }
        self.band_ceil = [-18.0, -23.5, -29.5];
        self.flux_env_db = -120.0;
        self.live_chroma = [0.0; 4];
    }

    pub fn measure_row_powers(&self, bytes: &[u8], offset: usize, stride: usize) -> [f64; 3] {
        let (mut p0, mut p1, mut p2) = (0.0f64, 0.0f64, 0.0f64);
        let stride = stride.max(1);
        let mut i = 0;
        while i < self.count {
            let p = self.power[bytes[offset + i] as usize] as f64 * self.zone_weight[i] as f64;
            match self.zone_map[i] {
                0 => p0 += p,
                1 => p1 += p,
                _ => p2 += p,
            }
            i += stride;
        }
        let scale = stride as f64;
        [
            p0 * scale / self.zone_denom[0],
            p1 * scale / self.zone_denom[1],
            p2 * scale / self.zone_denom[2],
        ]
    }

    pub fn compute_triad_rgb(
        &mut self,
        m: [f64; 3],
        floor: f64,
        advance_envelope: bool,
        dt_sec: f64,
    ) -> [f32; 3] {
        let dbs = [
            10.0 * (m[0] + 1e-30).log10(),
            10.0 * (m[1] + 1e-30).log10(),
            10.0 * (m[2] + 1e-30).log10(),
        ];
        let eff_floor = floor.min(-96.0);
        let gates = [eff_floor + 6.0, eff_floor + 4.0, eff_floor + 2.0];
        let min_span = 26.0;

        if advance_envelope {
            let attack = 1.0 - (-dt_sec / 0.035).exp();
            let release = 1.0 - (-dt_sec / 9.0).exp();
            for z in 0..3 {
                let db = dbs[z];
                if db > gates[z] + 2.0 {
                    let target = (gates[z] + min_span).max(db);
                    let ceil = self.band_ceil[z] as f64;
                    let rate = if target > ceil { attack } else { release };
                    self.band_ceil[z] = (ceil + (target - ceil) * rate) as f32;
                }
            }
        }

        let ref_peak = (self.band_ceil[0] as f64 + DADASPEC_TILT_DB[0])
            .max(self.band_ceil[1] as f64 + DADASPEC_TILT_DB[1])
            .max(self.band_ceil[2] as f64 + DADASPEC_TILT_DB[2]);
        let live_peak = (dbs[0] + DADASPEC_TILT_DB[0])
            .max(dbs[1] + DADASPEC_TILT_DB[1])
            .max(dbs[2] + DADASPEC_TILT_DB[2]);
        let active_ref = if live_peak < ref_peak - 32.0 {
            live_peak + 4.0
        } else {
            ref_peak
        };

        let mut n = [0.0f64; 3];
        for z in 0..3 {
            let coupled_ceil = (gates[z] + 18.0)
                .max((self.band_ceil[z] as f64).min(active_ref - DADASPEC_TILT_DB[z]))
                .max(active_ref - DADASPEC_TILT_DB[z] - 10.0);
            let band_floor = gates[z].max(coupled_ceil - 36.0);
            let span = 18.0f64.max(coupled_ceil - band_floor);
            n[z] = ((dbs[z] - band_floor) / span).clamp(0.0, 1.0);
        }
        let mx = n[0].max(n[1]).max(n[2]);
        if mx <= 1e-3 {
            [0.0; 3]
        } else {
            [(n[0] / mx) as f32, (n[1] / mx) as f32, (n[2] / mx) as f32]
        }
    }

    fn total_db(&self, m: [f64; 3]) -> f64 {
        let denom_sum = self.zone_denom[0] + self.zone_denom[1] + self.zone_denom[2];
        let total_mean =
            (m[0] * self.zone_denom[0] + m[1] * self.zone_denom[1] + m[2] * self.zone_denom[2])
                / denom_sum;
        10.0 * (total_mean + 1e-30).log10()
    }

    pub fn compute_onset_flux(&mut self, total_db: f64, dt_sec: f64) -> f32 {
        if self.flux_env_db <= -118.0 {
            self.flux_env_db = total_db;
            return if total_db > -85.0 { 0.65 } else { 0.0 };
        }
        let rise = total_db - self.flux_env_db;
        let flux = ((rise - 1.6) / 11.0).clamp(0.0, 1.0) as f32;
        let tau = if total_db > self.flux_env_db {
            0.042
        } else {
            0.095
        };
        self.flux_env_db += (total_db - self.flux_env_db) * (1.0 - (-dt_sec / tau).exp());
        flux
    }

    fn encode_byte(v: f32) -> u8 {
        js_round((v as f64).clamp(0.0, 1.0) * 255.0).clamp(0.0, 255.0) as u8
    }

    /// Updates all 6 chroma pyramid levels for a row inserted at `row_index` (the value of
    /// `history.written` immediately before `history.push(bytes)`).
    pub fn push_row(&mut self, row_index: usize, bytes: &[u8], row_rate: u32, floor: f64) {
        assert_eq!(bytes.len(), self.count);
        let m = self.measure_row_powers(bytes, 0, 1);
        let rate = if row_rate == 0 { 60 } else { row_rate.max(30) };
        let dt_sec = 1.0 / rate as f64;
        let [r0, g0, b0] = self.compute_triad_rgb(m, floor, true, dt_sec);
        let total_db = self.total_db(m);
        let flux = self.compute_onset_flux(total_db, dt_sec);

        let base0 = (row_index % HISTORY_ROWS) * 4;
        self.levels[0][base0] = Self::encode_byte(r0);
        self.levels[0][base0 + 1] = Self::encode_byte(g0);
        self.levels[0][base0 + 2] = Self::encode_byte(b0);
        self.levels[0][base0 + 3] = Self::encode_byte(flux);
        self.dirty[0] += 1;
        self.live_chroma = [r0, g0, b0, flux];

        for l in 1..LEVELS {
            let span = 1usize << l;
            for z in 0..3 {
                self.band_power_sums[l][z] += m[z];
            }
            if flux > self.flux_peaks[l] {
                self.flux_peaks[l] = flux;
            }
            if (row_index + 1) % span != 0 {
                continue;
            }
            let sums = self.band_power_sums[l];
            let avg = [sums[0] / span as f64, sums[1] / span as f64, sums[2] / span as f64];
            let [rl, gl, bl] = self.compute_triad_rgb(avg, floor, false, 0.0);
            let basel = ((row_index / span) % HISTORY_ROWS) * 4;
            let flux_peak = self.flux_peaks[l];
            self.levels[l][basel] = Self::encode_byte(rl);
            self.levels[l][basel + 1] = Self::encode_byte(gl);
            self.levels[l][basel + 2] = Self::encode_byte(bl);
            self.levels[l][basel + 3] = Self::encode_byte(flux_peak);
            self.band_power_sums[l] = [0.0; 3];
            self.flux_peaks[l] = 0.0;
            self.dirty[l] += 1;
        }
    }

    /// Rebuilds all 6 chroma pyramid levels from an existing CPU `History` ring (e.g. on style switch).
    pub fn sync_from_history(&mut self, history: &History, sub: usize, row_rate: u32, floor: f64) {
        assert_eq!(history.count, self.count);
        self.clear();
        if history.written == 0 {
            return;
        }
        let stride = sub.max(1);
        let rate = if row_rate == 0 { 60 } else { row_rate.max(30) };
        let base_dt = 1.0 / rate as f64;

        for l in (0..LEVELS).rev() {
            let span = 1usize << l;
            let total_blocks = history.written / span;
            let valid_blocks = total_blocks.min(HISTORY_ROWS).min(history.rows);
            if valid_blocks == 0 {
                continue;
            }
            let dt_block = base_dt * span as f64;
            let start_block = total_blocks - valid_blocks;
            let mut local_env_db = -120.0f64;
            for b in start_block..total_blocks {
                let ring_row = b % history.rows;
                let chroma_row = b % HISTORY_ROWS;
                let m = self.measure_row_powers(
                    &history.levels[l],
                    ring_row * history.count,
                    stride,
                );
                let [r, g, bl] = self.compute_triad_rgb(m, floor, l == 0, dt_block);
                let total_db = self.total_db(m);
                let flux = if b == start_block {
                    local_env_db = total_db;
                    0.0
                } else {
                    let f = ((total_db - local_env_db - 1.6) / 11.0).clamp(0.0, 1.0) as f32;
                    let tau = if total_db > local_env_db {
                        0.042
                    } else {
                        0.095
                    };
                    local_env_db +=
                        (total_db - local_env_db) * (1.0 - (-dt_block / tau).exp());
                    f
                };
                let o = chroma_row * 4;
                self.levels[l][o] = Self::encode_byte(r);
                self.levels[l][o + 1] = Self::encode_byte(g);
                self.levels[l][o + 2] = Self::encode_byte(bl);
                self.levels[l][o + 3] = Self::encode_byte(flux);

                if l == 0 && b == total_blocks - 1 {
                    self.flux_env_db = local_env_db;
                    self.live_chroma = [r, g, bl, flux];
                }
            }
            self.dirty[l] = HISTORY_ROWS;
        }

        for l in 1..LEVELS {
            let span = 1usize << l;
            let rem = history.written % span;
            if rem == 0 {
                continue;
            }
            for r in (history.written - rem)..history.written {
                let ring_row = r % history.rows;
                let m = self.measure_row_powers(
                    &history.levels[0],
                    ring_row * history.count,
                    stride,
                );
                for z in 0..3 {
                    self.band_power_sums[l][z] += m[z];
                }
            }
        }
    }
}

#[cfg(test)]
mod display_test {
    use super::*;
    #[test]
    fn attack_release_peak_and_maximum_follow_audio_time() {
        let mut display = DisplaySpectrum::new(1);
        display.update(&[-20.0], 1.0 / 60.0);
        assert_eq!(display.shown[0], -20.0);
        for _ in 0..60 {
            display.update(&[-120.0], 1.0 / 60.0);
        }
        assert!((display.shown[0] + 56.0).abs() < 0.001);
        assert_eq!(display.peak[0], -20.0);
        for _ in 0..60 {
            display.update(&[-120.0], 1.0 / 60.0);
        }
        assert!(display.peak[0] < -28.0 && display.peak[0] > -30.0);
        assert_eq!(display.maximum[0], -20.0);
        display.clear_maximum();
        assert_eq!(display.maximum[0], -120.0);
    }
    #[test]
    fn mel_axis_keeps_the_ends_and_inverts() {
        // The constants web/shaders/common.glsl carries for the same mapping.
        assert!((mel_of(band_hz(0.0)) - 0.0230908).abs() < 1e-6);
        assert!((mel_of(band_hz(126.0)) - 3.5504234).abs() < 1e-6);
        for mel in [0.0, 0.3, 1.0] {
            assert!(band_to_u(0.0, mel).abs() < 1e-12);
            assert!((band_to_u(126.0, mel) - 1.0).abs() < 1e-12);
            for band in [0.0, 12.5, 57.0, 71.2131, 126.0] {
                assert!((u_to_band(band_to_u(band, mel), mel) - band).abs() < 1e-5);
            }
        }
        assert_eq!(band_to_u(63.0, 0.0), 0.5);
        // 1 kHz sits in the left quarter of the mel axis.
        assert!(band_to_u(hz_to_band(1000.0), 1.0) < 0.26);
    }
    #[test]
    fn envelope_keeps_peaks_and_fills_dips() {
        let mut src = vec![-90.0f32; 41];
        src[20] = -12.0;
        src[24] = -30.0;
        let (mut dst, mut scratch) = (vec![0.0; 41], vec![0.0; 41]);
        envelope(&src, &mut dst, &mut scratch, 2);
        assert_eq!(dst[20], -12.0, "a peak keeps its exact level");
        assert!(dst[22] > -40.0, "the dip between two partials fills in");
        assert_eq!(dst[5], -90.0);
        envelope(&src, &mut dst, &mut scratch, 0);
        assert_eq!(dst, src);
        // Half a semitone each side: none at one band per semitone.
        for (sub, radius) in [(1, 0), (2, 1), (4, 2), (8, 4)] {
            assert_eq!(DisplaySpectrum::new(band_count(sub)).smooth, radius);
        }
        let mut display = DisplaySpectrum::new(band_count(4));
        let mut row = vec![-90.0; band_count(4)];
        row[200] = -10.0;
        display.update(&row, 1.0 / 60.0);
        assert_eq!(display.raw[201], -90.0, "raw stays raw for the history");
        assert_eq!(display.shown[200], -10.0);
        assert!(display.shown[201] > -20.0 && display.maximum[201] == display.shown[201]);
    }
    #[test]
    fn peaks_are_tracked_and_labelled() {
        let mut shown = vec![-90.0f32; 127];
        shown[57] = -20.0;
        shown[56] = -40.0;
        shown[58] = -40.0;
        shown[69] = -32.0;
        let mut tracker = PeakTracker::default();
        for _ in 0..20 {
            tracker.update(&shown, &shown, 1.0 / 60.0, |_| None, 1);
        }
        assert_eq!(tracker.tracks.len(), 2);
        let main = &tracker.tracks[0];
        assert!((main.band - 57.0).abs() < 1e-9 && main.alpha == 1.0);
        assert_eq!(
            main.text.as_ref().unwrap(),
            &["A4".to_owned(), "±0¢".into(), "440\u{202f}Hz  −20\u{202f}dB".into()]
        );
        let quiet = vec![-120.0f32; 127];
        for _ in 0..20 {
            tracker.update(&quiet, &quiet, 1.0 / 60.0, |_| None, 1);
        }
        assert!(tracker.tracks.is_empty(), "tracks fade out and are dropped");
        assert_eq!(note_name(452.0), ("A4".into(), 47));
    }
    #[test]
    fn sample_clock_has_no_display_dependency_or_chunk_drift() {
        for rate in [60, 120, 240, 480] {
            let mut clock = RowClock::new(48000, rate);
            let mut count = 0;
            for written in 1..=48000 {
                for end in clock.consume_samples(written) {
                    count += 1;
                    assert_eq!(end, count * 48000 / rate as usize);
                }
            }
            assert_eq!(count, rate as usize);
            assert_eq!(clock.sample_phase(48000), 0.0);
        }
    }
    #[test]
    fn skipped_ffts_preserve_rows_and_refresh_due_analysers() {
        let mut analysis = Analysis::new(48000, 480, 1);
        let mut signal = TestSignal::new(48000);
        let mut block = [0.0; 100];
        for row in 0..960 {
            signal.fill(&mut block);
            analysis.receive(&block);
            analysis.analyse_with_refresh(analysis.written, row % 2 == 1);
        }
        assert_eq!(analysis.row_index, 960);
        assert!((analysis.peak_hz(33.0).unwrap() - 110.0).abs() < 1.0);
        assert!(analysis.bands.iter().copied().fold(-120.0f32, f32::max) > -30.0);
    }
    #[test]
    fn test_signal_is_chunk_independent() {
        let mut signal = TestSignal::new(48000);
        let mut block = [0.0; 513];
        signal.fill(&mut block[..128]);
        signal.fill(&mut block[128..]);
        for (i, &sample) in block.iter().enumerate() {
            assert_eq!(sample, TestSignal::sample_at(i as u64, 48000));
        }
    }
    #[test]
    fn render_scale_supports_modes_and_auto_trials() {
        for (mode, expected) in [
            ("auto", 1.0),
            ("1", 1.0),
            ("0.85", 0.85),
            ("0.7", 0.7),
            ("0.5", 0.5),
            ("bad", 1.0),
        ] {
            let mut rs = RenderScale::new(mode);
            assert_eq!(rs.scale_f64(), expected);
            assert!(!rs.update(0.03, Some(25.0), true) || mode == "auto");
        }
        let mut manual = RenderScale::new("0.5");
        assert_eq!(manual.mode(), "0.5");
        assert_eq!(manual.scale(), 0.5);
        for _ in 0..300 {
            assert!(!manual.update(0.03, Some(25.0), true));
        }
        assert_eq!(manual.scale(), 0.5);

        let mut rs = RenderScale::new("auto");
        // Slow frames that stay slow after lowering are undone and held off for 60 s.
        for _ in 0..81 {
            rs.update(0.025, Some(20.0), true);
        }
        assert_eq!(rs.scale_f64(), 0.85);
        for _ in 0..201 {
            rs.update(0.025, Some(20.0), true);
        }
        assert_eq!(rs.scale_f64(), 1.0);
        for _ in 0..100 {
            assert!(!rs.update(0.025, Some(20.0), true));
        }
    }
    #[test]
    fn dadaspec_chroma_tracks_bands_flux_pyramid_and_history_sync() {
        let sub = 2;
        let count = band_count(sub);
        let mut h = History::new(count, HISTORY_ROWS);
        let mut chroma = DadaspecChroma::new(sub, [200.0, 1500.0]);

        let low_band = hz_to_band(100.0).round() as usize * sub;
        let mid_band = hz_to_band(600.0).round() as usize * sub;
        let high_band = hz_to_band(4000.0).round() as usize * sub;

        let mut row = vec![0u8; count];
        for b in 0..sub {
            row[low_band + b] = 220;
        }
        for _ in 0..32 {
            let idx = h.written;
            h.push(&row);
            chroma.push_row(idx, &row, 60, -96.0);
        }
        // Row 31 closes a block on levels 1..5 (32 rows pushed).
        assert_eq!(chroma.dirty, [32, 16, 8, 4, 2, 1]);
        assert_eq!(chroma.levels[0][31 * 4], 255);
        assert_eq!(chroma.levels[0][31 * 4 + 1], 0);
        assert_eq!(chroma.levels[0][31 * 4 + 2], 0);

        // Loud broadband transient produces strong onset flux and moves dominant zone cleanly.
        row.fill(0);
        for b in 0..sub {
            row[mid_band + b] = 230;
        }
        let idx = h.written;
        h.push(&row);
        chroma.push_row(idx, &row, 60, -96.0);
        assert!(chroma.levels[0][idx * 4 + 1] > chroma.levels[0][idx * 4]);
        assert!(chroma.levels[0][idx * 4 + 3] > 50);

        // Sync at row 33 (with partial remainder on levels 1..5), then push row 33 (high band)
        // to close level-1 block 16 (mid_band + high_band) cleanly.
        let mut synced = DadaspecChroma::new(sub, [200.0, 1500.0]);
        synced.sync_from_history(&h, sub, 60, -96.0);
        assert_eq!(synced.dirty, [HISTORY_ROWS; LEVELS]);
        assert!(
            (synced.levels[0][idx * 4 + 1] as i32 - chroma.levels[0][idx * 4 + 1] as i32).abs()
                <= 2
        );
        row.fill(0);
        for b in 0..sub {
            row[high_band + b] = 235;
        }
        let idx = h.written;
        h.push(&row);
        chroma.push_row(idx, &row, 60, -96.0);
        assert!(chroma.live_chroma[2] > chroma.live_chroma[0]);
        synced.push_row(idx, &row, 60, -96.0);
        assert!(synced.levels[1][(idx / 2) * 4 + 2] > 200);
        assert!(
            (synced.levels[1][(idx / 2) * 4 + 1] as i32
                - chroma.levels[1][(idx / 2) * 4 + 1] as i32)
                .abs()
                <= 15
        );

        synced.clear();
        assert_eq!(synced.dirty, [HISTORY_ROWS; LEVELS]);
        assert_eq!(synced.live_chroma, [0.0; 4]);
    }
}
