//! Audio capture probe for test runs: opens one input device through the app's own analysis
//! worker, drains rows for a while, and prints one JSON line. It records only counts, rates
//! and whether the signal was non-zero, never the audio itself.
//!
//!     cargo run --manifest-path rust/Cargo.toml --release -p spectrum-wgpu --example audio-probe -- \
//!         [DEVICE] [ROWS_PER_SECOND] [SECONDS] [BANDS_PER_SEMITONE] [--input-buffer fixed|default]
//!
//! DEVICE is an index from `--list-inputs` (all digits) or part of a device name; without it
//! the system default input is used. Defaults: 60 rows/s, 10 s, 4 bands per semitone.
use spectrum_wgpu::audio::{input_devices, AnalysisWorker, InputBuffer};
use std::{
    thread,
    time::{Duration, Instant},
};

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = Vec::new();
    let mut input_buffer = InputBuffer::default();
    let mut cli = std::env::args().skip(1);
    while let Some(arg) = cli.next() {
        if arg == "--input-buffer" {
            input_buffer = cli
                .next()
                .ok_or("--input-buffer requires a value")?
                .parse()?;
        } else if arg.starts_with("--") {
            return Err(format!("Unknown argument {arg}"));
        } else {
            args.push(arg);
        }
    }
    let number = |i: usize, default: u64| -> Result<u64, String> {
        args.get(i).map_or(Ok(default), |a| {
            a.parse().map_err(|_| format!("not a number: {a}"))
        })
    };
    let rate = number(1, 60)? as u32;
    let seconds = number(2, 10)?;
    let sub = number(3, 4)? as usize;
    let devices = input_devices()?;
    let device = match args.first().map(String::as_str) {
        None => devices.iter().find(|d| d.is_default).or(devices.first()),
        Some(n) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {
            let index: usize = n.parse().map_err(|_| format!("bad index: {n}"))?;
            devices.iter().find(|d| d.index == index)
        }
        Some(needle) => devices.iter().find(|d| d.name.contains(needle)),
    }
    .ok_or_else(|| {
        let names: Vec<_> = devices
            .iter()
            .map(|d| format!("{}: {}", d.index, d.name))
            .collect();
        format!("input device not found; available:\n{}", names.join("\n"))
    })?;
    let mut worker = AnalysisWorker::microphone_target_with_buffer(
        Some(&device.id),
        None,
        rate,
        sub,
        input_buffer,
    )?;
    let start = Instant::now();
    let (mut rows, mut nonzero, mut max_byte) = (0u64, 0u64, 0u8);
    while start.elapsed() < Duration::from_secs(seconds) {
        for row in worker.drain() {
            rows += 1;
            let peak = row.bytes.iter().copied().max().unwrap_or(0);
            nonzero += u64::from(peak > 0);
            max_byte = max_byte.max(peak);
        }
        thread::sleep(Duration::from_millis(8));
    }
    let s = worker.stats();
    let line = serde_json::json!({
        "device": device.name,
        "index": device.index,
        "durationSeconds": start.elapsed().as_secs_f64(),
        "requestedRowRate": rate,
        "sub": sub,
        "inputBufferRequested": input_buffer.as_str(),
        "inputBuffer": worker.input_buffer().map(|b| b.as_str()),
        "callbackFrames": s.callback_frames,
        "callbackMinFrames": s.callback_min_frames,
        "callbackMaxFrames": s.callback_max_frames,
        "callbackCount": s.callback_count,
        "callbackMs": s.callback_frames.filter(|_| s.source_sample_rate > 0).map(|n| n as f64 * 1000.0 / s.source_sample_rate as f64),
        "sourceSampleRate": s.source_sample_rate,
        "channels": s.channels,
        "rowsPerSecond": s.rows_per_second,
        "fftPerSecond": s.fft_rows_per_second,
        "analysisMs": s.analysis_ms,
        "samples": s.samples,
        "inputFramesDropped": s.input_frames_dropped,
        "displayRowsDropped": s.display_rows_dropped,
        "rowsDrained": rows,
        "nonzeroRows": nonzero,
        "maxByte": max_byte,
        "error": worker.error().unwrap_or_default(),
    });
    println!("{line}");
    worker.stop();
    Ok(())
}
