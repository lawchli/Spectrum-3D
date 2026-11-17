//! Real output-clock validation; muted unless --listen is explicitly supplied.
//! cargo run --release --example playback-probe -- 300 [--listen] [FILE]
use spectrum_wgpu::audio::{decode, AnalysisWorker};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let seconds = args
        .first()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(300);
    let listen = args.iter().any(|s| s == "--listen");
    let pcm = if let Some(path) = args.iter().skip(1).find(|s| !s.starts_with("--")) {
        decode::file(std::path::Path::new(path)).unwrap()
    } else {
        decode::test_track(0).unwrap()
    };
    let worker = AnalysisWorker::track(Arc::new(pcm), 240, 4).unwrap();
    worker.set_listen(listen);
    let start = Instant::now();
    let mut next = 5.;
    while start.elapsed().as_secs_f64() < seconds as f64 {
        std::thread::sleep(Duration::from_millis(10));
        let rows = worker.drain();
        if start.elapsed().as_secs_f64() >= next || start.elapsed().as_secs_f64() >= seconds as f64
        {
            let a = worker.stats();
            let playback = a.playback_frames as f64 / a.output_rate.max(1) as f64;
            let analysis = a.samples as f64 / 48000.;
            println!(
                "{}",
                serde_json::json!({"elapsed":start.elapsed().as_secs_f64(),
                "playbackSeconds":playback,"analysisSeconds":analysis,"differenceMs":(playback-analysis)*1000.,
                "rowSeconds":rows.last().map(|r| r.sample_end as f64/48000.),
                "outputFrames":a.output_frames,"rate":a.output_rate,"peak":a.output_peak,
                "underruns":a.output_underruns,"drops":a.input_frames_dropped,"error":worker.output_error()})
            );
            next += 30.;
        }
    }
}
