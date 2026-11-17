//! Native audio capture and analysis. The device callback only mixes and queues
//! fixed-size packets; resampling, FFTs and display dynamics run on one worker.
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use spectrum_core::{
    band_count, band_hz, db_to_byte, Analysis, DisplaySpectrum, PeakTrack, PeakTracker, RowClock,
    TestSignal,
};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub mod decode;
mod input;
pub use input::InputStatus;
mod playback;

pub const ANALYSIS_SAMPLE_RATE: u32 = 48_000;
const PACKET_FRAMES: usize = 2048;
const MIN_INPUT_PACKETS: usize = 32;
const OUTPUT_ROWS: usize = 240;

/// Per-run input buffer policy. Fixed requests about 5 ms, with a default-buffer fallback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputBuffer {
    #[default]
    Fixed,
    Default,
}
impl InputBuffer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Default => "default",
        }
    }
    fn target_frames(self, supported: &cpal::SupportedBufferSize, sample_rate: u32) -> Option<u32> {
        match self {
            Self::Fixed => preferred_input_buffer_size(supported, sample_rate),
            Self::Default => None,
        }
    }
}
impl std::str::FromStr for InputBuffer {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "fixed" => Ok(Self::Fixed),
            "default" => Ok(Self::Default),
            _ => Err("--input-buffer must be fixed or default".into()),
        }
    }
}

/// Return the successfully opened policy, including a fixed-to-default fallback.
fn build_input_stream<T>(
    config: &mut cpal::StreamConfig,
    target_frames: Option<u32>,
    mut build: impl FnMut(&cpal::StreamConfig) -> Result<T, String>,
) -> Result<(T, InputBuffer), String> {
    if let Some(frames) = target_frames {
        config.buffer_size = cpal::BufferSize::Fixed(frames);
        if let Ok(stream) = build(config) {
            return Ok((stream, InputBuffer::Fixed));
        }
    }
    config.buffer_size = cpal::BufferSize::Default;
    build(config).map(|stream| (stream, InputBuffer::Default))
}

fn preferred_input_buffer_size(
    supported: &cpal::SupportedBufferSize,
    sample_rate: u32,
) -> Option<u32> {
    match supported {
        cpal::SupportedBufferSize::Range { min, max } if *max > 0 && *min <= *max => {
            let step = if sample_rate >= 96_000 { 128 } else { 64 };
            let raw = (sample_rate.max(1) as u64 * 5).div_ceil(1000);
            let aligned = ((raw + step / 2) / step * step).max(step) as u32;
            Some(aligned.clamp(*min, *max))
        }
        _ => None,
    }
}

pub(super) fn packet_queue_capacity(sample_rate: u32, buffer_frames: Option<u32>) -> usize {
    let frames_200ms = (sample_rate.max(1) as usize).div_ceil(5);
    let chunk_frames = buffer_frames
        .map_or(256, |b| (b as usize).min(256))
        .clamp(64, PACKET_FRAMES);
    frames_200ms.div_ceil(chunk_frames).max(MIN_INPUT_PACKETS)
}

#[derive(Clone, Debug)]
pub struct InputDevice {
    pub index: usize,
    /// Stable CPAL device ID, suitable for saving across application launches.
    pub id: String,
    pub name: String,
    /// Unadorned system name, retained for reconnection when the ID changes.
    pub raw_name: String,
    pub is_default: bool,
}
pub fn input_devices() -> Result<Vec<InputDevice>, String> {
    input::devices()
}

/// Decodes a mono 48 kHz PCM WAV of 16 or 24 bits, which is what
/// scripts/render-test-audio.mjs writes for the two test tracks.
pub fn decode_wav(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("Not a WAV file".into());
    }
    let u16le = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32le = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let (mut at, mut bits) = (12, 0);
    while at + 8 <= bytes.len() {
        let size = u32le(at + 4) as usize;
        let body = at + 8;
        let end = body.saturating_add(size).min(bytes.len());
        match &bytes[at..at + 4] {
            b"fmt " if end - body >= 16 => {
                if u16le(body) != 1 || u16le(body + 2) != 1 || u32le(body + 4) != 48000 {
                    return Err("Test audio must be mono 48 kHz PCM".into());
                }
                bits = u16le(body + 14);
            }
            b"data" => {
                let data = &bytes[body..end];
                return match bits {
                    16 => Ok(data
                        .chunks_exact(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                        .collect()),
                    24 => Ok(data
                        .chunks_exact(3)
                        .map(|b| {
                            (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8388608.0
                        })
                        .collect()),
                    _ => Err("Test audio must be 16 or 24 bit".into()),
                };
            }
            _ => {}
        }
        at = body + size + (size & 1);
    }
    Err("WAV file has no audio data".into())
}

#[derive(Clone, Debug)]
pub struct AnalysisRow {
    pub sample_end: u64,
    /// Callback entry that supplied the samples completing this row (input only).
    pub captured_at: Option<Instant>,
    pub bands: Vec<f32>,
    pub bytes: Vec<u8>,
    pub shown: Vec<f32>,
    pub peaks: Vec<f32>,
    pub maxima: Vec<f32>,
    pub peak_hz: Option<f64>,
    /// Position in semitones, not the data band's subdivision index.
    pub peak_band: Option<f64>,
}
#[derive(Clone, Debug, Default)]
pub struct AudioStats {
    pub capture_to_submit_ms: Option<f64>,
    pub capture_to_submit_mean_ms: Option<f64>,
    pub capture_to_submit_max_ms: Option<f64>,
    pub capture_to_submit_count: u64,
    pub callback_count: u64,
    pub callback_frames: Option<u64>,
    pub callback_min_frames: Option<u64>,
    pub callback_max_frames: Option<u64>,
    pub source_sample_rate: u32,
    pub channels: u16,
    pub rows_per_second: f64,
    pub fft_rows_per_second: f64,
    pub analysis_ms: f64,
    pub analysis_stride: usize,
    pub input_frames_dropped: u64,
    pub display_rows_dropped: u64,
    pub samples: u64,
    pub playback_frames: u64,
    pub output_frames: u64,
    pub output_peak: f32,
    pub output_underruns: u64,
    pub output_rate: u32,
    /// Time the latest microphone-listening output took to open (also when it failed).
    pub output_open_ms: Option<f64>,
    /// The output device last opened, and how many times the output moved to another
    /// device or came back after failing (F4).
    pub output_device: Option<String>,
    pub output_reopens: u64,
}
impl AudioStats {
    fn record_submission(&mut self, captured: Instant, submitted: Instant) {
        let Some(elapsed) = submitted.checked_duration_since(captured) else {
            return;
        };
        let ms = elapsed.as_secs_f64() * 1000.0;
        self.capture_to_submit_count += 1;
        let mean = self.capture_to_submit_mean_ms.unwrap_or(0.0);
        self.capture_to_submit_mean_ms =
            Some(mean + (ms - mean) / self.capture_to_submit_count as f64);
        self.capture_to_submit_max_ms = Some(self.capture_to_submit_max_ms.unwrap_or(0.0).max(ms));
        self.capture_to_submit_ms = Some(ms);
    }
}
#[derive(Default)]
struct Output {
    pending: VecDeque<AnalysisRow>,
    phase: f64,
    stats: AudioStats,
    error: Option<String>,
    tracks: Vec<PeakTrack>,
    output_error: Option<String>,
    input_status: Option<InputStatus>,
    input_device: Option<InputDevice>,
    input_buffer: Option<InputBuffer>,
    recovered_at: Option<Instant>,
}
impl Output {
    fn input_lost(&mut self) {
        if self.input_status == Some(InputStatus::Running) {
            self.input_status = Some(InputStatus::Waiting);
            self.pending.clear();
            self.tracks.clear();
        }
    }
}
struct Controls {
    running: AtomicBool,
    frozen: AtomicBool,
    input_failed: AtomicBool,
    clear_maximum: AtomicBool,
    dropped: AtomicU64,
    listen: AtomicBool,
    callback_count: AtomicU64,
    callback_frames: AtomicU64,
    callback_min_frames: AtomicU64,
    callback_max_frames: AtomicU64,
    playback: Arc<Mutex<playback::State>>,
}
impl Controls {
    fn record_callback(&self, frames: u64) {
        if frames == 0 {
            return;
        }
        self.callback_frames.store(frames, Ordering::Relaxed);
        self.callback_min_frames
            .fetch_min(frames, Ordering::Relaxed);
        self.callback_max_frames
            .fetch_max(frames, Ordering::Relaxed);
        self.callback_count.fetch_add(1, Ordering::Relaxed);
    }
    fn new() -> Self {
        Self {
            running: AtomicBool::new(true),
            frozen: AtomicBool::new(false),
            input_failed: AtomicBool::new(false),
            clear_maximum: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
            listen: AtomicBool::new(false),
            callback_count: AtomicU64::new(0),
            callback_frames: AtomicU64::new(0),
            callback_min_frames: AtomicU64::new(u64::MAX),
            callback_max_frames: AtomicU64::new(0),
            playback: Arc::new(Mutex::new(playback::State::default())),
        }
    }
}
struct Packet {
    captured_at: Option<Instant>,
    first: u64,
    len: usize,
    data: [f32; PACKET_FRAMES],
}

pub struct AnalysisWorker {
    controls: Arc<Controls>,
    output: Arc<Mutex<Output>>,
    worker: Option<JoinHandle<()>>,
}
impl AnalysisWorker {
    /// The deterministic five-tone chord.
    pub fn test(row_rate: u32, sub: usize) -> Result<Self, String> {
        Self::generated(row_rate, sub, None)
    }
    pub fn test_track(index: usize, row_rate: u32, sub: usize) -> Result<Self, String> {
        Self::validate(row_rate, sub)?;
        let controls = Arc::new(Controls::new());
        let output = Arc::new(Mutex::new(Output::default()));
        let (control, out) = (controls.clone(), output.clone());
        let worker = thread::Builder::new()
            .name("spectrum-track-decode".into())
            .spawn(move || match decode::test_track(index) {
                Ok(pcm) => playback::run_track(Arc::new(pcm), row_rate, sub, control, out),
                Err(e) => out.lock().unwrap().error = Some(e),
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            controls,
            output,
            worker: Some(worker),
        })
    }
    /// A decoded mono 48 kHz file, looped on the output clock.
    pub fn track(samples: Arc<Vec<f32>>, row_rate: u32, sub: usize) -> Result<Self, String> {
        if samples.is_empty() {
            return Err("Test audio is empty".into());
        }
        Self::generated(row_rate, sub, Some(samples))
    }
    fn generated(row_rate: u32, sub: usize, track: Option<Arc<Vec<f32>>>) -> Result<Self, String> {
        Self::validate(row_rate, sub)?;
        let controls = Arc::new(Controls::new());
        let output = Arc::new(Mutex::new(Output::default()));
        let (control, out) = (controls.clone(), output.clone());
        let worker = thread::Builder::new()
            .name("spectrum-analysis".into())
            .spawn(move || {
                if let Some(track) = track {
                    playback::run_track(track, row_rate, sub, control, out);
                    return;
                }
                let mut processor =
                    Processor::new(ANALYSIS_SAMPLE_RATE, row_rate, sub, control.clone(), out);
                let mut signal = TestSignal::new(ANALYSIS_SAMPLE_RATE);
                let mut block = [0.0; 256];
                let start = Instant::now();
                while control.running.load(Ordering::Acquire) {
                    signal.fill(&mut block);
                    processor.receive(&block);
                    let due = Duration::from_secs_f64(
                        signal.written as f64 / ANALYSIS_SAMPLE_RATE as f64,
                    );
                    if let Some(wait) = due.checked_sub(start.elapsed()) {
                        thread::sleep(wait);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            controls,
            output,
            worker: Some(worker),
        })
    }
    /// Input device discovery/configuration may block in an operating-system driver.
    /// Keep that work and the stream on the worker, never on the window thread.
    pub fn microphone(
        device_index: Option<usize>,
        row_rate: u32,
        sub: usize,
    ) -> Result<Self, String> {
        Self::microphone_with_buffer(device_index, row_rate, sub, InputBuffer::default())
    }
    pub fn microphone_with_buffer(
        device_index: Option<usize>,
        row_rate: u32,
        sub: usize,
        input_buffer: InputBuffer,
    ) -> Result<Self, String> {
        let mut target = input::Target::new(device_index, None, None);
        Self::open_input(row_rate, sub, input_buffer, move || target.choose())
    }
    /// Reconnect by ID first, then by the saved unadorned system name.
    pub fn microphone_id(
        device_id: Option<&str>,
        row_rate: u32,
        sub: usize,
    ) -> Result<Self, String> {
        Self::microphone_target(device_id, None, row_rate, sub)
    }
    pub fn microphone_target(
        device_id: Option<&str>,
        device_name: Option<&str>,
        row_rate: u32,
        sub: usize,
    ) -> Result<Self, String> {
        Self::microphone_target_with_buffer(
            device_id,
            device_name,
            row_rate,
            sub,
            InputBuffer::default(),
        )
    }
    pub fn microphone_target_with_buffer(
        device_id: Option<&str>,
        device_name: Option<&str>,
        row_rate: u32,
        sub: usize,
        input_buffer: InputBuffer,
    ) -> Result<Self, String> {
        let mut target = input::Target::new(None, device_id, device_name);
        Self::open_input(row_rate, sub, input_buffer, move || target.choose())
    }
    fn open_input(
        row_rate: u32,
        sub: usize,
        input_buffer: InputBuffer,
        choose: impl FnMut() -> Result<(cpal::Device, InputDevice), String> + Send + 'static,
    ) -> Result<Self, String> {
        Self::open_input_with(
            row_rate,
            sub,
            input_buffer,
            choose,
            Arc::new(open_listen_output),
        )
    }
    fn open_input_with(
        row_rate: u32,
        sub: usize,
        input_buffer: InputBuffer,
        mut choose: impl FnMut() -> Result<(cpal::Device, InputDevice), String> + Send + 'static,
        open_output: OutputOpener,
    ) -> Result<Self, String> {
        Self::validate(row_rate, sub)?;
        let controls = Arc::new(Controls::new());
        let output = Arc::new(Mutex::new(Output {
            input_status: Some(InputStatus::Opening),
            ..Output::default()
        }));
        let (control, out) = (controls.clone(), output.clone());
        let worker = thread::Builder::new()
            .name("spectrum-input-analysis".into())
            .spawn(move || {
                let mut retry = input::Retry::new(Instant::now());
                while control.running.load(Ordering::Acquire) {
                    control.input_failed.store(false, Ordering::Release);
                    let result = choose().and_then(|(device, info)| {
                        if !control.running.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        out.lock().unwrap().input_device = Some(info);
                        Self::run_device(
                            device,
                            row_rate,
                            sub,
                            input_buffer,
                            control.clone(),
                            out.clone(),
                            open_output.clone(),
                        )
                    });
                    if !control.running.load(Ordering::Acquire) {
                        break;
                    }
                    let mut output = out.lock().unwrap();
                    // Only a callback proves that opening actually succeeded.
                    if control.callback_count.load(Ordering::Relaxed) > 0 {
                        retry.connected();
                    }
                    let error = result
                        .err()
                        .or_else(|| output.error.take())
                        .unwrap_or_else(|| "Audio input stopped responding".into());
                    if !retry.retry(Instant::now()) {
                        output.input_status = Some(InputStatus::Failed);
                        output.error = Some(error);
                        break;
                    }
                    output.input_status = Some(if retry.was_connected() {
                        InputStatus::Waiting
                    } else {
                        InputStatus::Opening
                    });
                    output.error = None;
                    output.pending.clear();
                    output.tracks.clear();
                    drop(output);
                    // Cancellation is checked frequently, including while waiting to retry.
                    let until = Instant::now() + Duration::from_secs(1);
                    while control.running.load(Ordering::Acquire) && Instant::now() < until {
                        thread::sleep(Duration::from_millis(25));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        // A driver may block inside discovery/open. Bound the visible initial
        // attempt as well; cancellation fences any late result without joining it.
        let (watch_control, watch_output) = (controls.clone(), output.clone());
        thread::spawn(move || {
            let started = Instant::now();
            while watch_control.running.load(Ordering::Acquire) {
                let mut out = watch_output.lock().unwrap();
                if out.input_status != Some(InputStatus::Opening) {
                    break;
                }
                if started.elapsed() >= Duration::from_secs(10) {
                    out.input_status = Some(InputStatus::Failed);
                    out.error = Some("inputOpenTimeout".into());
                    watch_control.running.store(false, Ordering::Release);
                    break;
                }
                drop(out);
                thread::sleep(Duration::from_millis(25));
            }
        });
        Ok(Self {
            controls,
            output,
            worker: Some(worker),
        })
    }
    pub fn input_disconnected(&self) {
        self.output.lock().unwrap().input_lost();
        self.controls.input_failed.store(true, Ordering::Release);
    }
    pub fn input_status(&self) -> Option<InputStatus> {
        self.output.lock().unwrap().input_status
    }
    pub fn input_device(&self) -> Option<InputDevice> {
        self.output.lock().unwrap().input_device.clone()
    }
    /// Last successfully started input stream's policy; None before opening or for playback.
    pub fn input_buffer(&self) -> Option<InputBuffer> {
        self.output.lock().unwrap().input_buffer
    }
    pub fn input_notice(&self) -> Option<&'static str> {
        let out = self.output.lock().unwrap();
        if out.input_status == Some(InputStatus::Waiting) {
            Some("deviceLost")
        } else if out
            .recovered_at
            .is_some_and(|at| at.elapsed() < Duration::from_secs(3))
        {
            Some("deviceBack")
        } else {
            None
        }
    }
    fn validate(row_rate: u32, sub: usize) -> Result<(), String> {
        if ![60, 120, 240, 480].contains(&row_rate) || ![1, 2, 4, 8].contains(&sub) {
            return Err("Unsupported row rate or band subdivision".into());
        }
        Ok(())
    }
    fn run_device(
        device: cpal::Device,
        row_rate: u32,
        sub: usize,
        input_buffer: InputBuffer,
        controls: Arc<Controls>,
        output: Arc<Mutex<Output>>,
        open_output: OutputOpener,
    ) -> Result<(), String> {
        // Prefer the device's native format; arbitrary source rates are resampled
        // on the worker instead of imposing 48 kHz on USB interfaces.
        let supported = device.default_input_config().map_err(|e| e.to_string())?;
        let supported_buffer = *supported.buffer_size();
        let format = supported.sample_format();
        let mut config: cpal::StreamConfig = supported.into();
        if config.channels == 0 || config.sample_rate == 0 {
            return Err("Invalid device audio configuration".into());
        }
        let target_frames = input_buffer.target_frames(&supported_buffer, config.sample_rate);
        let (tx, rx) = mpsc::sync_channel(packet_queue_capacity(config.sample_rate, target_frames));
        let build = |cfg: &cpal::StreamConfig| {
            let sender = tx.clone();
            match format {
                cpal::SampleFormat::I8 => {
                    build_stream::<i8>(&device, cfg, sender, controls.clone(), output.clone())
                }
                cpal::SampleFormat::I16 => {
                    build_stream::<i16>(&device, cfg, sender, controls.clone(), output.clone())
                }
                cpal::SampleFormat::I24 => build_stream::<cpal::I24>(
                    &device,
                    cfg,
                    sender,
                    controls.clone(),
                    output.clone(),
                ),
                cpal::SampleFormat::I32 => {
                    build_stream::<i32>(&device, cfg, sender, controls.clone(), output.clone())
                }
                cpal::SampleFormat::I64 => {
                    build_stream::<i64>(&device, cfg, sender, controls.clone(), output.clone())
                }
                cpal::SampleFormat::U8 => {
                    build_stream::<u8>(&device, cfg, sender, controls.clone(), output.clone())
                }
                cpal::SampleFormat::U16 => {
                    build_stream::<u16>(&device, cfg, sender, controls.clone(), output.clone())
                }
                cpal::SampleFormat::U24 => build_stream::<cpal::U24>(
                    &device,
                    cfg,
                    sender,
                    controls.clone(),
                    output.clone(),
                ),
                cpal::SampleFormat::U32 => {
                    build_stream::<u32>(&device, cfg, sender, controls.clone(), output.clone())
                }
                cpal::SampleFormat::U64 => {
                    build_stream::<u64>(&device, cfg, sender, controls.clone(), output.clone())
                }
                cpal::SampleFormat::F32 => {
                    build_stream::<f32>(&device, cfg, sender, controls.clone(), output.clone())
                }
                cpal::SampleFormat::F64 => {
                    build_stream::<f64>(&device, cfg, sender, controls.clone(), output.clone())
                }
                other => Err(format!("Unsupported input sample format: {other}")),
            }
        };
        let (stream, actual_buffer) = build_input_stream(&mut config, target_frames, build)?;
        drop(tx);
        if !controls.running.load(Ordering::Acquire) {
            return Ok(());
        }
        // Everything that allocates or may block is done before the callback starts:
        // once `play` returns, the queue only has ~200 ms of headroom.
        let mut processor =
            Processor::new(config.sample_rate, row_rate, sub, controls.clone(), output);
        processor.set_channels(config.channels);
        let listener = Listener::spawn(&controls, &processor.output, open_output);
        // Recorded before `play` so nothing takes a lock between the first callback and draining.
        let previous_buffer = processor
            .output
            .lock()
            .unwrap()
            .input_buffer
            .replace(actual_buffer);
        if let Err(e) = stream.play() {
            processor.output.lock().unwrap().input_buffer = previous_buffer;
            return Err(e.to_string());
        }
        process_packets(rx, &mut processor, &controls);
        drop(listener);
        // Report the loss before dropping a stream: a driver can block in close too.
        if controls.running.load(Ordering::Acquire) {
            processor.output.lock().unwrap().input_lost();
        }
        drop(stream);
        Ok(())
    }
    /// Draining never runs analysis and is safe to call at any display frame rate.
    pub fn drain(&self) -> Vec<AnalysisRow> {
        self.output.lock().unwrap().pending.drain(..).collect()
    }
    pub fn phase(&self) -> f64 {
        self.output.lock().unwrap().phase
    }
    pub fn stats(&self) -> AudioStats {
        let mut stats = self.output.lock().unwrap().stats.clone();
        let count = self.controls.callback_count.load(Ordering::Relaxed);
        stats.callback_count = count;
        if count > 0 {
            stats.callback_frames = Some(self.controls.callback_frames.load(Ordering::Relaxed));
            stats.callback_min_frames =
                Some(self.controls.callback_min_frames.load(Ordering::Relaxed));
            stats.callback_max_frames =
                Some(self.controls.callback_max_frames.load(Ordering::Relaxed));
        }
        let play = self.controls.playback.lock().unwrap();
        stats.playback_frames = play.frames;
        stats.output_frames = play.output_frames;
        stats.output_peak = play.peak;
        stats.output_underruns = play.underruns;
        stats.output_rate = play.rate;
        stats.input_frames_dropped = self.controls.dropped.load(Ordering::Relaxed);
        stats
    }
    pub fn session_id(&self) -> usize {
        Arc::as_ptr(&self.output) as usize
    }
    /// Called once per submitted frame with its newest input row, even with the monitor hidden.
    pub fn record_submission(&self, captured: Instant, submitted: Instant) {
        self.output
            .lock()
            .unwrap()
            .stats
            .record_submission(captured, submitted);
    }
    pub fn error(&self) -> Option<String> {
        let out = self.output.lock().unwrap();
        if out
            .input_status
            .is_some_and(|status| status != InputStatus::Failed)
        {
            None
        } else {
            out.error.clone()
        }
    }
    /// The tracked main peaks, for the labels on the front spectrum.
    pub fn tracks(&self) -> Vec<PeakTrack> {
        self.output.lock().unwrap().tracks.clone()
    }
    pub fn set_frozen(&self, frozen: bool) {
        self.controls.frozen.store(frozen, Ordering::Release);
        if frozen {
            self.output.lock().unwrap().pending.clear();
        }
    }
    pub fn set_listen(&self, listen: bool) {
        self.controls.listen.store(listen, Ordering::Release);
    }
    pub fn output_error(&self) -> Option<String> {
        self.output.lock().unwrap().output_error.clone()
    }
    pub fn clear_maximum(&self) {
        self.controls.clear_maximum.store(true, Ordering::Release);
    }
    pub fn stop(&mut self) {
        self.controls.running.store(false, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            // A faulty driver may never return from open/close. Cancellation still fences this
            // session, and detaching that OS call keeps source switches and window close usable.
            let deadline = Instant::now() + Duration::from_millis(25);
            while !worker.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}
impl Drop for AnalysisWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Keeps the output stream alive; a `cpal::Stream` is not always `Send`, so it never
/// leaves the thread that built it.
type OutputGuard = Box<dyn std::any::Any>;
/// Opens and starts the output used to hear a microphone. Replaceable for tests.
type OutputOpener =
    Arc<dyn Fn(&Arc<Controls>, &Arc<Mutex<Output>>) -> Result<OutputGuard, String> + Send + Sync>;
/// How long listening must stay off before the output closes, so the 20 ms fade ends first.
const LISTEN_CLOSE_DELAY: Duration = Duration::from_millis(250);

fn open_listen_output(
    controls: &Arc<Controls>,
    output: &Arc<Mutex<Output>>,
) -> Result<OutputGuard, String> {
    let (stream, _, _) = playback::open(None, controls.clone(), output.clone(), 0)?;
    if let Err(e) = stream.play() {
        let e = e.to_string();
        output.lock().unwrap().output_error = Some(e.clone());
        return Err(e);
    }
    Ok(Box::new(stream))
}

/// The microphone's listening output lives on its own thread for one input session:
/// opening a device can take any amount of time and must never delay draining the
/// input queue. The output is opened only while listening is on.
struct Listener {
    session: Arc<AtomicBool>,
}
impl Listener {
    fn spawn(controls: &Arc<Controls>, output: &Arc<Mutex<Output>>, open: OutputOpener) -> Self {
        let session = Arc::new(AtomicBool::new(true));
        output.lock().unwrap().output_error = None;
        let (c, o, s) = (controls.clone(), output.clone(), session.clone());
        if let Err(e) = thread::Builder::new()
            .name("spectrum-listen-output".into())
            .spawn(move || listen_output(c, o, s, open))
        {
            output.lock().unwrap().output_error = Some(e.to_string());
        }
        Self { session }
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        // Never joined: closing a device may block too. The thread notices within 10 ms.
        self.session.store(false, Ordering::Release);
    }
}
fn listen_output(
    controls: Arc<Controls>,
    output: Arc<Mutex<Output>>,
    session: Arc<AtomicBool>,
    open: OutputOpener,
) {
    let alive = || controls.running.load(Ordering::Acquire) && session.load(Ordering::Acquire);
    let mut stream: Option<OutputGuard> = None;
    // A failed open is retried after listening is switched off and on again, or once the
    // system default output changes. An open output follows the default too (F4).
    let mut failed = false;
    let mut off_since: Option<Instant> = None;
    // The default output's id first seen after the latest open (or failed open); looked up
    // off the opening path, since asking the system can itself take a while.
    let mut device: Option<Option<String>> = None;
    let mut checked = Instant::now();
    while alive() {
        let listening = controls.listen.load(Ordering::Acquire);
        if listening && (stream.is_some() || failed) && checked.elapsed() >= Duration::from_secs(1)
        {
            checked = Instant::now();
            let now = playback::default_output_id();
            let broken = stream.is_some() && output.lock().unwrap().output_error.is_some();
            match &device {
                None => device = Some(now),
                Some(seen) if broken || now.is_some() && &now != seen => {
                    drop(stream.take());
                    failed = false;
                    output.lock().unwrap().stats.output_reopens += 1;
                }
                Some(_) => {}
            }
        }
        if listening {
            off_since = None;
            if stream.is_none() && !failed {
                device = None;
                let started = Instant::now();
                let result = open(&controls, &output);
                let ms = started.elapsed().as_secs_f64() * 1000.0;
                output.lock().unwrap().stats.output_open_ms = Some(ms);
                match result {
                    Ok(s) => {
                        // Drop what queued up while the device was opening.
                        controls.playback.lock().unwrap().queue.clear();
                        output.lock().unwrap().output_error = None;
                        stream = Some(s);
                    }
                    Err(_) => failed = true,
                }
                continue;
            }
        } else if failed {
            failed = false;
            output.lock().unwrap().output_error = None;
        } else if stream.is_some()
            && off_since.get_or_insert_with(Instant::now).elapsed() >= LISTEN_CLOSE_DELAY
        {
            drop(stream.take());
            off_since = None;
            output.lock().unwrap().output_error = None;
        }
        thread::sleep(Duration::from_millis(10));
    }
    drop(stream);
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    tx: SyncSender<Packet>,
    controls: Arc<Controls>,
    output: Arc<Mutex<Output>>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = config.channels as usize;
    let (control, failures) = (controls.clone(), controls.clone());
    let mut frame = 0u64;
    device
        .build_input_stream(
            config,
            move |input: &[T], _| {
                let captured_at = Instant::now();
                if !control.running.load(Ordering::Acquire) {
                    return;
                }
                let frames = (input.len() / channels) as u64;
                if frames == 0 {
                    return;
                }
                control.record_callback(frames);
                for chunk in input.chunks(channels * PACKET_FRAMES) {
                    let mut packet = Packet {
                        captured_at: Some(captured_at),
                        first: frame,
                        len: chunk.len() / channels,
                        data: [0.0; PACKET_FRAMES],
                    };
                    for (value, samples) in packet.data.iter_mut().zip(chunk.chunks_exact(channels))
                    {
                        *value = samples
                            .iter()
                            .map(|&v| <f32 as cpal::FromSample<T>>::from_sample_(v))
                            .sum::<f32>()
                            / channels as f32;
                        if !value.is_finite() {
                            *value = 0.0;
                        }
                    }
                    frame += packet.len as u64;
                    if let Err(TrySendError::Full(packet)) = tx.try_send(packet) {
                        control
                            .dropped
                            .fetch_add(packet.len as u64, Ordering::Relaxed);
                    }
                }
            },
            move |error| {
                failures.input_failed.store(true, Ordering::Release);
                let mut out = output.lock().unwrap();
                out.error = Some(error.to_string());
                out.input_lost();
            },
            None,
        )
        .map_err(|e| e.to_string())
}

fn process_packets(rx: Receiver<Packet>, processor: &mut Processor, control: &Controls) {
    let mut expected = 0u64;
    let mut last_packet = Instant::now();
    let silence = [0.0; PACKET_FRAMES];
    while control.running.load(Ordering::Acquire) && !control.input_failed.load(Ordering::Acquire) {
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(packet) => {
                last_packet = Instant::now();
                {
                    let mut out = processor.output.lock().unwrap();
                    if !control.running.load(Ordering::Acquire) {
                        break;
                    }
                    if out.input_status.is_some() && out.input_status != Some(InputStatus::Running)
                    {
                        if out.input_status == Some(InputStatus::Waiting) {
                            out.recovered_at = Some(Instant::now());
                        }
                        out.input_status = Some(InputStatus::Running);
                        out.error = None;
                    }
                }
                // A callback overrun must not compress the audio time axis. Replace
                // dropped frames with silence, while exposing their count in stats.
                while expected < packet.first && control.running.load(Ordering::Acquire) {
                    let count = (packet.first - expected).min(PACKET_FRAMES as u64) as usize;
                    processor.captured_at = None;
                    processor.receive(&silence[..count]);
                    expected += count as u64;
                }
                processor.captured_at = packet.captured_at;
                processor.receive(&packet.data[..packet.len]);
                expected = packet.first + packet.len as u64;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Some backends stop callbacks without delivering a stream error.
                if processor.output.lock().unwrap().input_status.is_some()
                    && last_packet.elapsed() >= Duration::from_secs(3)
                {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

struct Processor {
    captured_at: Option<Instant>,
    analysis: Analysis,
    display: DisplaySpectrum,
    clock: RowClock,
    resampler: Resampler,
    resampled: Vec<f32>,
    controls: Arc<Controls>,
    output: Arc<Mutex<Output>>,
    tracker: PeakTracker,
    tracker_rows: usize,
    stride: usize,
    skipped: usize,
    busy: Duration,
    window_rows: usize,
    rows: usize,
    fft_rows: usize,
    measured: Instant,
    /// Samples analysed before this processor took over a session (on another clock).
    samples_before: u64,
}
impl Processor {
    fn new(
        source_rate: u32,
        row_rate: u32,
        sub: usize,
        controls: Arc<Controls>,
        output: Arc<Mutex<Output>>,
    ) -> Self {
        Self::start(source_rate, row_rate, sub, controls, output, true)
    }
    /// Carries a session on to another clock (a new output device or the fallback clock):
    /// the counters and the output's identity go on from where they were.
    fn continuing(
        source_rate: u32,
        row_rate: u32,
        sub: usize,
        controls: Arc<Controls>,
        output: Arc<Mutex<Output>>,
    ) -> Self {
        Self::start(source_rate, row_rate, sub, controls, output, false)
    }
    fn start(
        source_rate: u32,
        row_rate: u32,
        sub: usize,
        controls: Arc<Controls>,
        output: Arc<Mutex<Output>>,
        fresh: bool,
    ) -> Self {
        let samples_before = {
            let mut out = output.lock().unwrap();
            if fresh {
                out.stats = AudioStats {
                    output_device: out.stats.output_device.take(),
                    ..Default::default()
                };
            }
            out.stats.source_sample_rate = source_rate;
            out.stats.channels = 1;
            out.stats.analysis_stride = 1;
            out.stats.samples
        };
        Self {
            samples_before,
            captured_at: None,
            analysis: Analysis::new(ANALYSIS_SAMPLE_RATE, row_rate, sub),
            display: DisplaySpectrum::new(band_count(sub)),
            clock: RowClock::new(ANALYSIS_SAMPLE_RATE, row_rate),
            resampler: Resampler::new(source_rate, ANALYSIS_SAMPLE_RATE),
            resampled: Vec::with_capacity(4096),
            controls,
            output,
            tracker: PeakTracker::default(),
            tracker_rows: 0,
            stride: 1,
            skipped: 0,
            busy: Duration::ZERO,
            window_rows: 0,
            rows: 0,
            fft_rows: 0,
            measured: Instant::now(),
        }
    }
    fn set_channels(&mut self, channels: u16) {
        self.output.lock().unwrap().stats.channels = channels;
    }
    fn receive(&mut self, input: &[f32]) {
        self.resampled.clear();
        self.resampler.process(input, &mut self.resampled);
        if self.controls.listen.load(Ordering::Relaxed) {
            let mut play = self.controls.playback.lock().unwrap();
            if play.microphone {
                play.queue.extend(self.resampled.iter().copied());
                while play.queue.len() > 9600 {
                    play.queue.pop_front();
                }
            }
        }
        let mut offset = 0;
        while offset < self.resampled.len() {
            let end = self.clock.next_sample();
            let take = (end - self.analysis.written).min(self.resampled.len() - offset);
            self.analysis
                .receive(&self.resampled[offset..offset + take]);
            offset += take;
            if self.analysis.written == end {
                self.clock.consume_samples(end as u64);
                self.emit_row(end);
            }
        }
        let mut output = self.output.lock().unwrap();
        output.phase = if self.controls.frozen.load(Ordering::Acquire) {
            0.0
        } else {
            self.clock.sample_phase(self.analysis.written as u64)
        };
        output.stats.samples = self.samples_before + self.analysis.written as u64;
        let elapsed = self.measured.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            output.stats.rows_per_second = self.rows as f64 / elapsed;
            output.stats.fft_rows_per_second = self.fft_rows as f64 / elapsed;
            self.rows = 0;
            self.fft_rows = 0;
            self.measured = Instant::now();
        }
    }
    fn emit_row(&mut self, end: usize) {
        let refresh = self.skipped + 1 >= self.stride;
        let started = Instant::now();
        self.analysis.analyse_with_refresh(end, refresh);
        let spent = started.elapsed();
        self.busy += spent;
        self.window_rows += 1;
        self.rows += 1;
        if refresh {
            self.skipped = 0;
            self.fft_rows += 1;
        } else {
            self.skipped += 1;
        }
        if self.window_rows >= (self.analysis.row_rate as usize / 2).max(8) {
            let load =
                self.busy.as_secs_f64() / (self.window_rows as f64 / self.analysis.row_rate as f64);
            if load > 0.75 && self.stride < 8 {
                self.stride += 1;
            } else if self.stride > 1
                && load * (self.stride as f64) / ((self.stride - 1) as f64) < 0.5
            {
                self.stride -= 1;
            }
            self.busy = Duration::ZERO;
            self.window_rows = 0;
        }
        if self.controls.clear_maximum.swap(false, Ordering::AcqRel) {
            self.display.clear_maximum();
        }
        if self.controls.frozen.load(Ordering::Acquire) {
            return;
        }
        self.display
            .update(&self.analysis.bands, 1.0 / self.analysis.row_rate as f64);
        // Main peaks at 60 steps a second of audio time whatever the row rate: the tracker's
        // smoothing is per step, and the web runs it once a display frame.
        self.tracker_rows += 1;
        let tracked = self.tracker_rows >= (self.analysis.row_rate as usize / 60).max(1);
        if tracked {
            self.tracker_rows = 0;
            self.tracker.update(
                &self.display.shown,
                &self.display.live,
                1.0 / 60.0,
                |band| self.analysis.peak_hz(band),
                self.analysis.sub,
            );
        }
        let loudest = self
            .analysis
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1));
        let peak_band = loudest
            .filter(|(_, db)| **db > -120.0)
            .map(|(k, _)| k as f64 / self.analysis.sub as f64);
        let peak_hz =
            peak_band.map(|band| self.analysis.peak_hz(band).unwrap_or_else(|| band_hz(band)));
        let row = AnalysisRow {
            captured_at: self.captured_at,
            sample_end: end as u64,
            bytes: self
                .analysis
                .bands
                .iter()
                .map(|db| db_to_byte(*db as f64))
                .collect(),
            bands: self.analysis.bands.clone(),
            shown: self.display.shown.clone(),
            peaks: self.display.peak.clone(),
            maxima: self.display.maximum.clone(),
            peak_hz,
            peak_band,
        };
        let mut output = self.output.lock().unwrap();
        if tracked {
            output.tracks.clone_from(&self.tracker.tracks);
        }
        if output.pending.len() == OUTPUT_ROWS {
            output.pending.pop_front();
            output.stats.display_rows_dropped += 1;
        }
        output.pending.push_back(row);
        output.stats.analysis_stride = self.stride;
        output.stats.analysis_ms =
            output.stats.analysis_ms * 0.95 + spent.as_secs_f64() * 1000.0 * 0.05;
    }
}

/// Streaming windowed-sinc sample-rate conversion, with an antialias low-pass
/// when reducing the device rate. Phase and filter history survive packet edges.
/// It buffers 16 source frames (0.36 ms at 44.1 kHz); 48 kHz input is passed through.
struct Resampler {
    input_rate: u32,
    output_rate: u32,
    written: u64,
    next: u64,
    base: i64,
    input: VecDeque<f32>,
    kernels: Vec<[f32; 32]>,
}
impl Resampler {
    fn new(input_rate: u32, output_rate: u32) -> Self {
        assert!(input_rate > 0 && output_rate > 0);
        let cutoff = (output_rate as f64 / input_rate as f64).min(1.0) * 0.94;
        let kernels = (0..256)
            .map(|phase| {
                let fraction = phase as f64 / 256.0;
                let mut taps = [0.0; 32];
                let mut sum = 0.0;
                for (i, tap) in taps.iter_mut().enumerate() {
                    let distance = i as f64 - 15.0 - fraction;
                    let x = std::f64::consts::PI * cutoff * distance;
                    let sinc = if x.abs() < 1e-12 { 1.0 } else { x.sin() / x };
                    let window = 0.5 + 0.5 * (std::f64::consts::PI * distance / 16.0).cos();
                    *tap = (cutoff * sinc * window) as f32;
                    sum += *tap as f64;
                }
                for tap in &mut taps {
                    *tap /= sum as f32;
                }
                taps
            })
            .collect();
        Self {
            input_rate,
            output_rate,
            written: 0,
            next: 0,
            base: -15,
            input: VecDeque::from(vec![0.0; 15]),
            kernels,
        }
    }
    fn process(&mut self, samples: &[f32], output: &mut Vec<f32>) {
        if self.input_rate == self.output_rate {
            output.extend_from_slice(samples);
            return;
        }
        self.input.extend(samples.iter().copied());
        self.written += samples.len() as u64;
        loop {
            let numerator = self.next * self.input_rate as u64;
            let centre = (numerator / self.output_rate as u64) as i64;
            if centre + 16 >= self.written as i64 {
                break;
            }
            let phase =
                ((numerator % self.output_rate as u64) * 256 / self.output_rate as u64) as usize;
            let offset = (centre - 15 - self.base) as usize;
            let mut value = 0.0;
            for (i, coefficient) in self.kernels[phase].iter().enumerate() {
                value += self.input[offset + i] * coefficient;
            }
            output.push(value);
            self.next += 1;
            let following =
                (self.next * self.input_rate as u64 / self.output_rate as u64) as i64 - 15;
            while self.base < following {
                self.input.pop_front();
                self.base += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_sizes_count_source_frames_before_packet_splitting() {
        let controls = Arc::new(Controls::new());
        let worker = AnalysisWorker {
            controls: controls.clone(),
            output: Arc::new(Mutex::new(Output::default())),
            worker: None,
        };
        assert_eq!(worker.stats().callback_frames, None);
        for frames in [0, 128, 8192, 512] {
            controls.record_callback(frames);
        }
        let stats = worker.stats();
        assert_eq!(stats.callback_count, 3);
        assert_eq!(stats.callback_frames, Some(512));
        assert_eq!(stats.callback_min_frames, Some(128));
        assert_eq!(stats.callback_max_frames, Some(8192));
    }
    #[test]
    fn capture_timestamp_survives_packet_queue_resampling_and_row_drain() {
        // A whole callback may exceed a packet, and many rows share its timestamp.
        // Test 48 kHz pass-through and 192 kHz resampling without an actual device.
        for rate in [48000, 192000] {
            let controls = Arc::new(Controls::new());
            let output = Arc::new(Mutex::new(Output::default()));
            let mut processor = Processor::new(rate, 480, 1, controls.clone(), output.clone());
            let (tx, rx) = mpsc::sync_channel(4);
            let at = Instant::now() - Duration::from_millis(30);
            for i in 0..2 {
                tx.send(Packet {
                    captured_at: Some(at),
                    first: (i * PACKET_FRAMES) as u64,
                    len: PACKET_FRAMES,
                    data: [0.1; PACKET_FRAMES],
                })
                .unwrap();
            }
            drop(tx);
            process_packets(rx, &mut processor, &controls);
            let worker = AnalysisWorker {
                controls,
                output,
                worker: None,
            };
            let rows = worker.drain();
            assert!(!rows.is_empty());
            assert!(rows.iter().all(|r| r.captured_at == Some(at)));
            assert!(worker.stats().capture_to_submit_ms.is_none());
            // Submission is measured once for the newest row, not once per history row.
            let submitted = at + Duration::from_millis(40);
            worker.record_submission(rows.last().unwrap().captured_at.unwrap(), submitted);
            let stats = worker.stats();
            assert_eq!(stats.capture_to_submit_count, 1);
            assert_eq!(stats.capture_to_submit_ms, Some(40.0));
            assert!(worker.drain().is_empty());
        }
    }
    #[test]
    fn submission_summary_is_absent_until_measured_and_counts_mean_and_max() {
        let mut stats = AudioStats::default();
        assert!(stats.capture_to_submit_ms.is_none());
        assert!(stats.callback_frames.is_none());
        let at = Instant::now();
        stats.record_submission(at, at + Duration::from_millis(10));
        stats.record_submission(at, at + Duration::from_millis(30));
        stats.record_submission(at, at - Duration::from_millis(1));
        assert_eq!(stats.capture_to_submit_count, 2);
        assert_eq!(stats.capture_to_submit_ms, Some(30.0));
        assert_eq!(stats.capture_to_submit_mean_ms, Some(20.0));
        assert_eq!(stats.capture_to_submit_max_ms, Some(30.0));
    }
    #[test]
    fn stalled_device_initialization_can_be_cancelled_without_blocking_the_window() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let mut worker = AnalysisWorker::open_input(60, 1, InputBuffer::Fixed, move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            finished_tx.send(()).unwrap();
            Err("cancelled driver initialization".into())
        })
        .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let start = Instant::now();
        worker.stop();
        assert!(start.elapsed() < Duration::from_millis(250));
        assert!(!worker.controls.running.load(Ordering::Acquire));
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    }
    #[test]
    fn initial_driver_hang_reports_timeout_without_waiting_for_driver() {
        let (release_tx, release_rx) = mpsc::channel();
        let worker = AnalysisWorker::open_input(60, 1, InputBuffer::Fixed, move || {
            release_rx.recv().unwrap();
            Err("late result".into())
        })
        .unwrap();
        let start = Instant::now();
        while worker.input_status() != Some(InputStatus::Failed)
            && start.elapsed() < Duration::from_secs(12)
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(worker.input_status(), Some(InputStatus::Failed));
        assert_eq!(worker.error().as_deref(), Some("inputOpenTimeout"));
        release_tx.send(()).unwrap();
        assert!(!worker.controls.running.load(Ordering::Acquire));
    }
    #[test]
    fn cancelling_a_retry_prevents_any_later_open() {
        let calls = Arc::new(AtomicU64::new(0));
        let count = calls.clone();
        let mut worker = AnalysisWorker::open_input(60, 1, InputBuffer::Fixed, move || {
            count.fetch_add(1, Ordering::Relaxed);
            Err("device busy".into())
        })
        .unwrap();
        let start = Instant::now();
        while calls.load(Ordering::Relaxed) == 0 && start.elapsed() < Duration::from_secs(1) {
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(worker.input_status(), Some(InputStatus::Opening));
        assert!(worker.error().is_none());
        worker.stop();
        thread::sleep(Duration::from_millis(1100));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
    #[test]
    fn recovered_input_requires_pcm_and_notice_expires() {
        let controls = Arc::new(Controls::new());
        let output = Arc::new(Mutex::new(Output {
            input_status: Some(InputStatus::Waiting),
            ..Default::default()
        }));
        let worker = AnalysisWorker {
            controls: controls.clone(),
            output: output.clone(),
            worker: None,
        };
        assert_eq!(worker.input_notice(), Some("deviceLost"));
        let mut processor = Processor::new(48000, 60, 1, controls.clone(), output.clone());
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(Packet {
            captured_at: Some(Instant::now()),
            first: 0,
            len: 800,
            data: [0.1; PACKET_FRAMES],
        })
        .unwrap();
        drop(tx);
        process_packets(rx, &mut processor, &controls);
        assert_eq!(worker.input_status(), Some(InputStatus::Running));
        assert_eq!(worker.input_notice(), Some("deviceBack"));
        assert!(!worker.drain().is_empty());
        output.lock().unwrap().recovered_at = Some(Instant::now() - Duration::from_secs(4));
        assert_eq!(worker.input_notice(), None);
        worker.input_disconnected();
        assert!(controls.running.load(Ordering::Acquire));
        assert!(controls.input_failed.load(Ordering::Acquire));
    }
    /// Feeds 256-frame packets in real time the way the input callback does,
    /// counting a full queue as dropped input. Returns the packets sent.
    fn feed_like_a_callback(
        tx: SyncSender<Packet>,
        controls: Arc<Controls>,
        duration: Duration,
    ) -> JoinHandle<u64> {
        thread::spawn(move || {
            let frames = 256;
            let start = Instant::now();
            let mut sent = 0u64;
            while start.elapsed() < duration {
                let mut packet = Packet {
                    captured_at: Some(Instant::now()),
                    first: sent * frames as u64,
                    len: frames,
                    data: [0.0; PACKET_FRAMES],
                };
                packet.data[..frames].fill(0.1);
                if let Err(TrySendError::Full(p)) = tx.try_send(packet) {
                    controls.dropped.fetch_add(p.len as u64, Ordering::Relaxed);
                }
                sent += 1;
                let due = Duration::from_secs_f64((sent * frames as u64) as f64 / 48_000.0);
                if let Some(wait) = due.checked_sub(start.elapsed()) {
                    thread::sleep(wait);
                }
            }
            sent
        })
    }
    struct OpenedOutput(mpsc::Sender<&'static str>);
    impl Drop for OpenedOutput {
        fn drop(&mut self) {
            let _ = self.0.send("closed");
        }
    }
    /// An output that takes `delay` to open and reports opening and closing.
    fn slow_output(delay: Duration, events: mpsc::Sender<&'static str>) -> OutputOpener {
        let events = Mutex::new(events);
        Arc::new(move |_: &Arc<Controls>, _: &Arc<Mutex<Output>>| {
            let events = events.lock().unwrap().clone();
            events.send("opening").unwrap();
            thread::sleep(delay);
            events.send("opened").unwrap();
            Ok(Box::new(OpenedOutput(events)) as OutputGuard)
        })
    }
    #[test]
    fn slow_listening_output_does_not_stall_input() {
        let queue = packet_queue_capacity(48_000, Some(256));
        let run = |inline: bool| {
            let controls = Arc::new(Controls::new());
            controls.listen.store(true, Ordering::Release);
            let output = Arc::new(Mutex::new(Output {
                input_status: Some(InputStatus::Opening),
                ..Output::default()
            }));
            let mut processor = Processor::new(48_000, 60, 1, controls.clone(), output.clone());
            let (events_tx, events) = mpsc::channel();
            let open = slow_output(Duration::from_millis(500), events_tx);
            let (tx, rx) = mpsc::sync_channel(queue);
            // Same order as run_device: listener first, then the callback starts, then drain.
            let listener = (!inline).then(|| Listener::spawn(&controls, &output, open.clone()));
            let feeder = feed_like_a_callback(tx, controls.clone(), Duration::from_millis(900));
            if inline {
                // The order before the fix: the worker opened the output before draining.
                let _guard = open(&controls, &output).unwrap();
            }
            process_packets(rx, &mut processor, &controls);
            let sent = feeder.join().unwrap();
            assert_eq!(events.recv_timeout(Duration::from_secs(2)), Ok("opening"));
            assert_eq!(events.recv_timeout(Duration::from_secs(2)), Ok("opened"));
            drop(listener);
            let out = output.lock().unwrap();
            (controls.dropped.load(Ordering::Relaxed), sent, out.stats.clone())
        };
        let (dropped, sent, stats) = run(false);
        assert_eq!(dropped, 0);
        assert_eq!(stats.samples, sent * 256);
        assert!(stats.output_open_ms.is_some_and(|ms| ms >= 500.0));
        // The test can tell: opening inline, as before, overflows the same queue.
        let (dropped, _, _) = run(true);
        assert!(dropped > 0);
    }
    #[test]
    fn listening_output_opens_only_while_listening_and_on_its_own_thread() {
        let controls = Arc::new(Controls::new());
        let output = Arc::new(Mutex::new(Output::default()));
        let (events_tx, events) = mpsc::channel();
        let open = slow_output(Duration::from_millis(50), events_tx);
        let listener = Listener::spawn(&controls, &output, open);
        assert!(events.recv_timeout(Duration::from_millis(200)).is_err());
        assert!(output.lock().unwrap().stats.output_open_ms.is_none());
        controls.listen.store(true, Ordering::Release);
        assert_eq!(events.recv_timeout(Duration::from_secs(1)), Ok("opening"));
        assert_eq!(events.recv_timeout(Duration::from_secs(1)), Ok("opened"));
        controls.listen.store(false, Ordering::Release);
        // Held through the fade-out, then closed by the thread that opened it.
        assert!(events.recv_timeout(LISTEN_CLOSE_DELAY / 2).is_err());
        assert_eq!(events.recv_timeout(Duration::from_secs(1)), Ok("closed"));
        assert!(output.lock().unwrap().stats.output_open_ms.is_some());
        controls.listen.store(true, Ordering::Release);
        assert_eq!(events.recv_timeout(Duration::from_secs(1)), Ok("opening"));
        assert_eq!(events.recv_timeout(Duration::from_secs(1)), Ok("opened"));
        // The input session ending closes it even while listening is on.
        drop(listener);
        assert_eq!(events.recv_timeout(Duration::from_secs(1)), Ok("closed"));
    }
    #[test]
    fn failed_listening_output_waits_for_the_switch_before_retrying() {
        let controls = Arc::new(Controls::new());
        let output = Arc::new(Mutex::new(Output::default()));
        let calls = Arc::new(AtomicU64::new(0));
        let count = calls.clone();
        let open: OutputOpener = Arc::new(move |_: &Arc<Controls>, out: &Arc<Mutex<Output>>| {
            count.fetch_add(1, Ordering::Relaxed);
            out.lock().unwrap().output_error = Some("no output".into());
            Err("no output".into())
        });
        controls.listen.store(true, Ordering::Release);
        let _listener = Listener::spawn(&controls, &output, open);
        thread::sleep(Duration::from_millis(150));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(output.lock().unwrap().output_error.is_some());
        controls.listen.store(false, Ordering::Release);
        thread::sleep(Duration::from_millis(60));
        assert!(output.lock().unwrap().output_error.is_none());
        controls.listen.store(true, Ordering::Release);
        thread::sleep(Duration::from_millis(60));
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }
    #[test]
    fn counters_carry_on_when_a_session_moves_to_another_clock() {
        let controls = Arc::new(Controls::new());
        let output = Arc::new(Mutex::new(Output::default()));
        let mut first = Processor::new(48_000, 60, 1, controls.clone(), output.clone());
        first.receive(&vec![0.1; 9600]);
        let before = output.lock().unwrap().stats.samples;
        assert!(before > 0);
        // A new output at another rate carries on from the same count.
        let mut next = Processor::continuing(44_100, 60, 1, controls.clone(), output.clone());
        next.receive(&vec![0.1; 8820]);
        let after = output.lock().unwrap().stats.samples;
        assert!(after > before + 9000 && after <= before + 9600, "{before} then {after}");
        assert_eq!(output.lock().unwrap().stats.source_sample_rate, 44_100);
    }
    fn sine(rate: u32, hz: f64, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin() as f32)
            .collect()
    }
    #[test]
    fn conversion_preserves_audio_clock_and_packet_independence() {
        for rate in [44100, 48000, 96000] {
            let input = sine(rate, 1000.0, rate as usize);
            let (mut a, mut b) = (Resampler::new(rate, 48000), Resampler::new(rate, 48000));
            let (mut entire, mut chunks) = (Vec::new(), Vec::new());
            a.process(&input, &mut entire);
            for block in input.chunks(173) {
                b.process(block, &mut chunks);
            }
            assert_eq!(entire, chunks);
            assert!((entire.len() as i64 - 48000).abs() <= 20);
            let error = entire
                .iter()
                .enumerate()
                .skip(64)
                .map(|(i, &v)| {
                    let expected = (std::f64::consts::TAU * 1000.0 * i as f64 / 48000.0).sin();
                    (v as f64 - expected).powi(2)
                })
                .sum::<f64>()
                / entire.len() as f64;
            assert!(error.sqrt() < 0.002, "rate {rate}: {}", error.sqrt());
        }
    }
    #[test]
    fn decodes_test_audio_wav_and_tracks_its_peak() {
        let mut wav = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\xbb\0\0\x80\x32\x02\0\x03\0\x18\0LIST\x03\0\0\0abc\0data".to_vec();
        let tone = sine(48000, 440.0, 48000);
        wav.extend_from_slice(&(tone.len() as u32 * 3).to_le_bytes());
        for v in &tone {
            wav.extend_from_slice(&((v * 8388607.0).round() as i32).to_le_bytes()[..3]);
        }
        let decoded = decode_wav(&wav).unwrap();
        assert_eq!(decoded.len(), tone.len());
        assert!(decoded.iter().zip(&tone).all(|(a, b)| (a - b).abs() < 1e-6));
        assert!(decode_wav(b"RIFF....WAVE").is_err());
        let controls = Arc::new(Controls::new());
        let output = Arc::new(Mutex::new(Output::default()));
        let mut processor = Processor::new(48000, 240, 4, controls, output.clone());
        processor.receive(&decoded);
        let tracks = output.lock().unwrap().tracks.clone();
        assert_eq!(tracks.len(), 1);
        assert!((tracks[0].hz - 440.0).abs() < 0.5 && tracks[0].alpha == 1.0);
        assert_eq!(tracks[0].text.as_ref().unwrap()[0], "A4");
    }
    #[test]
    fn downsampling_rejects_above_nyquist_energy() {
        let mut converter = Resampler::new(96000, 48000);
        let mut out = Vec::new();
        converter.process(&sine(96000, 36000.0, 96000), &mut out);
        let rms = (out
            .iter()
            .skip(64)
            .map(|v| (*v as f64).powi(2))
            .sum::<f64>()
            / (out.len() - 64) as f64)
            .sqrt();
        assert!(rms < 0.003, "aliased RMS {rms}");
    }
    #[test]
    fn analysis_rows_are_exact_and_freeze_does_not_queue_history() {
        let controls = Arc::new(Controls::new());
        let output = Arc::new(Mutex::new(Output::default()));
        let mut processor = Processor::new(48000, 480, 1, controls.clone(), output.clone());
        let mut signal = TestSignal::new(48000);
        let mut block = [0.0; 173];
        for _ in 0..100 {
            signal.fill(&mut block);
            processor.receive(&block);
        }
        let rows: Vec<_> = output.lock().unwrap().pending.drain(..).collect();
        assert_eq!(rows.len(), 173);
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row.sample_end, (i + 1) as u64 * 100);
        }
        controls.frozen.store(true, Ordering::Release);
        for _ in 0..10 {
            processor.receive(&block);
        }
        assert!(output.lock().unwrap().pending.is_empty());
        controls.frozen.store(false, Ordering::Release);
        processor.receive(&block);
        let first = output.lock().unwrap().pending.front().unwrap().sample_end;
        assert!(first > 19000);
    }
    #[test]
    fn input_buffer_policy_selects_fixed_default_and_reports_fallback() {
        let supported = cpal::SupportedBufferSize::Range { min: 64, max: 2048 };
        let mut config = cpal::StreamConfig {
            channels: 2,
            sample_rate: 48_000,
            buffer_size: cpal::BufferSize::Default,
        };
        let fixed_frames = InputBuffer::Fixed.target_frames(&supported, config.sample_rate);
        assert_eq!(fixed_frames, Some(256));
        assert_eq!(InputBuffer::Default.target_frames(&supported, 48_000), None);
        assert_eq!(
            InputBuffer::Fixed.target_frames(&cpal::SupportedBufferSize::Unknown, 48_000),
            None
        );

        let mut calls = Vec::new();
        let (_, actual) = build_input_stream(&mut config, fixed_frames, |cfg| {
            calls.push(cfg.buffer_size);
            Ok(())
        })
        .unwrap();
        assert!(matches!(calls.as_slice(), [cpal::BufferSize::Fixed(256)]));
        assert_eq!(actual, InputBuffer::Fixed);

        calls.clear();
        let (_, actual) = build_input_stream(&mut config, fixed_frames, |cfg| {
            calls.push(cfg.buffer_size);
            match cfg.buffer_size {
                cpal::BufferSize::Fixed(_) => Err("fixed unsupported".into()),
                cpal::BufferSize::Default => Ok(()),
            }
        })
        .unwrap();
        assert!(matches!(
            calls.as_slice(),
            [cpal::BufferSize::Fixed(256), cpal::BufferSize::Default]
        ));
        assert_eq!(actual, InputBuffer::Default);

        calls.clear();
        let (_, actual) = build_input_stream(&mut config, None, |cfg| {
            calls.push(cfg.buffer_size);
            Ok(())
        })
        .unwrap();
        assert!(matches!(calls.as_slice(), [cpal::BufferSize::Default]));
        assert_eq!(actual, InputBuffer::Default);
        assert_eq!(
            build_input_stream::<()>(&mut config, fixed_frames, |_| Err("open failed".into())),
            Err("open failed".into())
        );
    }
    #[test]
    fn small_fixed_buffer_and_200ms_queue_capacity_cover_192khz() {
        assert_eq!(
            preferred_input_buffer_size(&cpal::SupportedBufferSize::Range { min: 64, max: 2048 }, 48_000),
            Some(256)
        );
        assert_eq!(
            preferred_input_buffer_size(&cpal::SupportedBufferSize::Range { min: 64, max: 4096 }, 192_000),
            Some(1024)
        );
        assert_eq!(
            preferred_input_buffer_size(&cpal::SupportedBufferSize::Range { min: 512, max: 2048 }, 48_000),
            Some(512)
        );
        assert_eq!(
            preferred_input_buffer_size(&cpal::SupportedBufferSize::Unknown, 48_000),
            None
        );
        let q48 = packet_queue_capacity(48_000, Some(256));
        assert!(q48 * 256 >= 9_600 && q48 >= MIN_INPUT_PACKETS);
        let q192 = packet_queue_capacity(192_000, Some(1024));
        assert!(q192 * 256 >= 38_400 && q192 >= 150);
    }
}
