//! The output callback is the clock for both file playback and analysis. Muting
//! changes only the gain; FFT work stays off the device callback.
use super::*;
#[derive(Default)]
pub(super) struct State {
    pub frames: u64,
    pub output_frames: u64,
    pub peak: f32,
    pub underruns: u64,
    pub rate: u32,
    pub microphone: bool,
    pub queue: VecDeque<f32>,
}

/// The system's default output and its id. Playback follows it (F4): the device is looked
/// up again when the default changes or the open one fails, as the web's audio does.
fn default_output() -> Option<(cpal::Device, String)> {
    let device = cpal::default_host().default_output_device()?;
    let id = device.id().ok()?.to_string();
    Some((device, id))
}
pub(super) fn default_output_id() -> Option<String> {
    default_output().map(|(_, id)| id)
}
/// How often the default output is checked while playing or waiting for one.
const FOLLOW_INTERVAL: Duration = Duration::from_secs(1);
/// How often an output that failed but is still the default is tried again.
const RETRY_INTERVAL: Duration = Duration::from_secs(3);

/// Opens the default output. A track starts `start` frames in (at the analysis rate), so a
/// new device continues the song where the last one stopped. Returns the device's id too.
pub(super) fn open(
    track: Option<Arc<Vec<f32>>>,
    controls: Arc<Controls>,
    output: Arc<Mutex<Output>>,
    start: u64,
) -> Result<(cpal::Stream, Option<Receiver<Packet>>, String), String> {
    let result = (|| {
        let (device, id) = default_output().ok_or("No output device")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let first = start * config.sample_rate as u64 / ANALYSIS_SAMPLE_RATE as u64;
        let (tx, rx) = if track.is_some() {
            let cap = packet_queue_capacity(config.sample_rate, None);
            let (tx, rx) = mpsc::sync_channel(cap);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        {
            let mut state = controls.playback.lock().unwrap();
            state.rate = config.sample_rate;
            state.microphone = track.is_none();
            if track.is_some() {
                state.frames = first;
            }
        }
        output.lock().unwrap().stats.output_device =
            device.description().ok().map(|d| d.name().to_owned());
        macro_rules! build {
            ($t:ty) => {
                build::<$t>(
                    &device,
                    &config,
                    first,
                    track,
                    tx,
                    controls.clone(),
                    output.clone(),
                )
            };
        }
        let stream = match format {
            cpal::SampleFormat::F32 => build!(f32),
            cpal::SampleFormat::F64 => build!(f64),
            cpal::SampleFormat::I8 => build!(i8),
            cpal::SampleFormat::I16 => build!(i16),
            cpal::SampleFormat::I24 => build!(cpal::I24),
            cpal::SampleFormat::I32 => build!(i32),
            cpal::SampleFormat::I64 => build!(i64),
            cpal::SampleFormat::U8 => build!(u8),
            cpal::SampleFormat::U16 => build!(u16),
            cpal::SampleFormat::U32 => build!(u32),
            cpal::SampleFormat::U64 => build!(u64),
            other => return Err(format!("Unsupported output sample format: {other}")),
        }?;
        Ok((stream, rx, id))
    })();
    if let Err(error) = &result {
        output.lock().unwrap().output_error = Some(error.clone());
    }
    result
}
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    first: u64,
    track: Option<Arc<Vec<f32>>>,
    tx: Option<SyncSender<Packet>>,
    controls: Arc<Controls>,
    output: Arc<Mutex<Output>>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let rate = config.sample_rate;
    // Resample once using the same sinc converter as capture, with periodic
    // history on both ends so conversion itself does not create a loop seam.
    let track = track.map(|samples| {
        if rate == ANALYSIS_SAMPLE_RATE {
            return samples;
        }
        let mut converter = Resampler::new(ANALYSIS_SAMPLE_RATE, rate);
        let mut converted = Vec::new();
        let prefix = 96;
        let before: Vec<_> = (0..prefix)
            .map(|i| samples[(samples.len() - prefix % samples.len() + i) % samples.len()])
            .collect();
        let after: Vec<_> = samples.iter().copied().cycle().take(prefix).collect();
        converter.process(&before, &mut converted);
        converter.process(&samples, &mut converted);
        converter.process(&after, &mut converted);
        let start = (prefix as u64 * rate as u64 / ANALYSIS_SAMPLE_RATE as u64) as usize;
        let len =
            ((samples.len() as u64 * rate as u64 / ANALYSIS_SAMPLE_RATE as u64) as usize).max(1);
        Arc::new(converted[start..start + len].to_vec())
    });
    let failures = output.clone();
    let mut frame = first;
    let mut gain = 0f32;
    let mut mic_phase = 0f64;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                let running = controls.running.load(Ordering::Acquire);
                let mut state = controls.playback.lock().unwrap();
                let target = if running && controls.listen.load(Ordering::Relaxed) {
                    1.0
                } else {
                    0.0
                };
                let smoothing = 1.0 - (-1.0 / (0.02 * rate as f32)).exp();
                let mut packet = Packet {
                    captured_at: None,
                    first: frame,
                    len: 0,
                    data: [0.; PACKET_FRAMES],
                };
                for out in data.chunks_mut(channels) {
                    let value = if !running {
                        0.0
                    } else if let Some(samples) = &track {
                        samples[(frame % samples.len() as u64) as usize]
                    } else if target > 0.0 || gain > 0.001 {
                        let a = state.queue.front().copied();
                        let b = state.queue.get(1).copied().or(a);
                        let v = a
                            .zip(b)
                            .map(|(a, b)| a + (b - a) * mic_phase as f32)
                            .unwrap_or_else(|| {
                                state.underruns += 1;
                                0.0
                            });
                        mic_phase += ANALYSIS_SAMPLE_RATE as f64 / rate as f64;
                        while mic_phase >= 1.0 {
                            state.queue.pop_front();
                            mic_phase -= 1.0;
                        }
                        v
                    } else {
                        state.queue.clear();
                        0.0
                    };
                    gain += (target - gain) * smoothing;
                    let audible = (value * gain).clamp(-1., 1.);
                    for sample in out {
                        *sample = T::from_sample_(audible);
                    }
                    state.peak = state.peak.max(audible.abs());
                    if running {
                        packet.data[packet.len] = value;
                        packet.len += 1;
                        frame += 1;
                        state.output_frames += 1;
                        if track.is_some() {
                            state.frames = frame;
                        }
                    }
                    if packet.len == PACKET_FRAMES {
                        if let Some(tx) = &tx {
                            send(tx, &controls, packet);
                        }
                        packet = Packet {
                            captured_at: None,
                            first: frame,
                            len: 0,
                            data: [0.; PACKET_FRAMES],
                        };
                    }
                }
                if packet.len > 0 {
                    if let Some(tx) = &tx {
                        send(tx, &controls, packet);
                    }
                }
            },
            move |e| {
                failures.lock().unwrap().output_error = Some(e.to_string());
            },
            None,
        )
        .map_err(|e| e.to_string())
}
fn send(tx: &SyncSender<Packet>, controls: &Controls, packet: Packet) {
    if let Err(TrySendError::Full(packet)) = tx.try_send(packet) {
        controls
            .dropped
            .fetch_add(packet.len as u64, Ordering::Relaxed);
    }
}
/// How one stretch of playback on an output ended.
enum Played {
    Stopped,
    /// The system default output changed; continue at this position on the new one.
    Changed(u64),
    /// The output could not be opened or stopped; this position, and the failed device.
    Failed(u64, Option<String>),
}
pub(super) fn run_track(
    track: Arc<Vec<f32>>,
    row_rate: u32,
    sub: usize,
    controls: Arc<Controls>,
    output: Arc<Mutex<Output>>,
) {
    // Position at the analysis rate, carried across devices and the fallback clock.
    let mut position = 0;
    let mut first = true;
    loop {
        let played = play(&track, row_rate, sub, &controls, &output, position, first);
        first = false;
        if matches!(played, Played::Changed(_)) {
            output.lock().unwrap().stats.output_reopens += 1;
        }
        position = match played {
            Played::Stopped => return,
            Played::Changed(at) => at,
            Played::Failed(at, failed) => {
                match fallback(&track, &controls, &output, at, row_rate, sub, Some(failed)) {
                    Some(at) => at,
                    None => return,
                }
            }
        };
    }
}
fn play(
    track: &Arc<Vec<f32>>,
    row_rate: u32,
    sub: usize,
    controls: &Arc<Controls>,
    output: &Arc<Mutex<Output>>,
    position: u64,
    fresh: bool,
) -> Played {
    if !controls.running.load(Ordering::Acquire) {
        return Played::Stopped;
    }
    let (stream, rx, id) = match open(
        Some(track.clone()),
        controls.clone(),
        output.clone(),
        position,
    ) {
        Ok((stream, Some(rx), id)) => (stream, rx, id),
        Ok(_) => unreachable!("a track always has a packet queue"),
        Err(_) => return Played::Failed(position, default_output_id()),
    };
    let rate = controls.playback.lock().unwrap().rate;
    let at =
        || controls.playback.lock().unwrap().frames * ANALYSIS_SAMPLE_RATE as u64 / rate as u64;
    if let Err(e) = stream.play() {
        output.lock().unwrap().output_error = Some(e.to_string());
        return Played::Failed(position, Some(id));
    }
    {
        let mut out = output.lock().unwrap();
        if out.output_error.take().is_some() {
            out.stats.output_reopens += 1;
        }
        // One line per output used, for following a change of device from the terminal.
        eprintln!(
            "Output: {} at {rate} Hz, from {:.2} s",
            out.stats.output_device.as_deref().unwrap_or("?"),
            position as f64 / ANALYSIS_SAMPLE_RATE as f64
        );
    }
    let mut processor = if fresh {
        Processor::new(rate, row_rate, sub, controls.clone(), output.clone())
    } else {
        Processor::continuing(rate, row_rate, sub, controls.clone(), output.clone())
    };
    // A failed or stalled output must not freeze the picture.
    let mut last = Instant::now();
    let mut checked = Instant::now();
    let mut expected = None;
    let silence = [0.; PACKET_FRAMES];
    while controls.running.load(Ordering::Acquire) {
        match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(packet) => {
                let expected = expected.get_or_insert(packet.first);
                while *expected < packet.first {
                    let n = (packet.first - *expected).min(PACKET_FRAMES as u64) as usize;
                    processor.receive(&silence[..n]);
                    *expected += n as u64;
                }
                processor.receive(&packet.data[..packet.len]);
                *expected = packet.first + packet.len as u64;
                last = Instant::now();
            }
            Err(_) if last.elapsed() > Duration::from_millis(500) => {
                output
                    .lock()
                    .unwrap()
                    .output_error
                    .get_or_insert("Output stopped responding".into());
            }
            Err(_) => {}
        }
        // Released before the stream is dropped: its error callback takes the same lock.
        let failed = output.lock().unwrap().output_error.clone();
        if let Some(error) = failed {
            eprintln!("Output failed: {error}");
            drop(stream);
            return Played::Failed(at(), Some(id));
        }
        if checked.elapsed() >= FOLLOW_INTERVAL {
            checked = Instant::now();
            if default_output_id().is_some_and(|now| now != id) {
                eprintln!("Output: the system default changed");
                drop(stream);
                return Played::Changed(at());
            }
        }
    }
    Played::Stopped
}
/// Keeps the picture moving on a monotonic clock while no output plays. With `retry`, the
/// output that failed (if any): returns the position to try an output again from as soon
/// as the default output changes, or every few seconds while the same one stays default.
/// Returns `None` once stopped.
fn fallback(
    track: &Arc<Vec<f32>>,
    controls: &Arc<Controls>,
    output: &Arc<Mutex<Output>>,
    from: u64,
    row_rate: u32,
    sub: usize,
    retry: Option<Option<String>>,
) -> Option<u64> {
    let mut written = from;
    let mut processor = Processor::continuing(
        ANALYSIS_SAMPLE_RATE,
        row_rate,
        sub,
        controls.clone(),
        output.clone(),
    );
    let start = Instant::now();
    let mut checked = start;
    let mut block = [0.; 256];
    while controls.running.load(Ordering::Acquire) {
        for v in &mut block {
            *v = track[(written % track.len() as u64) as usize];
            written += 1;
        }
        processor.receive(&block);
        if let Some(failed) = &retry {
            if checked.elapsed() >= FOLLOW_INTERVAL {
                checked = Instant::now();
                if let Some(now) = default_output_id() {
                    if Some(&now) != failed.as_ref() || start.elapsed() >= RETRY_INTERVAL {
                        return Some(written);
                    }
                }
            }
        }
        let due = Duration::from_secs_f64((written - from) as f64 / ANALYSIS_SAMPLE_RATE as f64);
        if let Some(wait) = due.checked_sub(start.elapsed()) {
            thread::sleep(wait);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_output_keeps_analysis_moving() {
        let controls = Arc::new(Controls::new());
        let output = Arc::new(Mutex::new(Output::default()));
        output.lock().unwrap().output_error = Some("simulated unavailable output".into());
        let (c, o) = (controls.clone(), output.clone());
        let thread = std::thread::spawn(move || {
            fallback(&Arc::new(vec![0.2; 48000]), &c, &o, 4800, 60, 1, None)
        });
        std::thread::sleep(Duration::from_millis(250));
        controls.running.store(false, Ordering::Release);
        assert_eq!(thread.join().unwrap(), None);
        let out = output.lock().unwrap();
        assert!(out.stats.samples > 0 && !out.pending.is_empty());
        assert!(out.error.is_none() && out.output_error.is_some());
    }
    #[test]
    fn waiting_without_output_tries_again_when_the_default_changes_or_later() {
        // Needs some output device to be the default; machines without one skip.
        let Some(default) = default_output_id() else {
            return;
        };
        let track = Arc::new(vec![0.2; 48000]);
        for (failed, wait) in [(Some("gone".to_owned()), FOLLOW_INTERVAL), (Some(default), RETRY_INTERVAL)] {
            let controls = Arc::new(Controls::new());
            let output = Arc::new(Mutex::new(Output::default()));
            let started = Instant::now();
            let at = fallback(&track, &controls, &output, 4800, 60, 1, Some(failed)).unwrap();
            let waited = started.elapsed();
            assert!(waited >= wait && waited < wait + Duration::from_millis(1500), "{waited:?}");
            // The clock kept the song going meanwhile, from where it was.
            let played = (at - 4800) as f64 / ANALYSIS_SAMPLE_RATE as f64;
            assert!((played - waited.as_secs_f64()).abs() < 0.1, "{played} vs {waited:?}");
            assert!(output.lock().unwrap().stats.samples > 0);
        }
    }
}
