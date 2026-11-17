//! Inspect decoded mono/48 kHz PCM without opening audio devices.
use spectrum_wgpu::audio::decode;
fn main() {
    for path in std::env::args().skip(1) {
        match decode::file(std::path::Path::new(&path)) {
            Ok(pcm) => println!(
                "{}",
                serde_json::json!({"file":path,"samples":pcm.len(),
                "peak":pcm.iter().fold(0f32,|p,s|p.max(s.abs())),
                "rms":(pcm.iter().map(|s|(*s as f64).powi(2)).sum::<f64>()/pcm.len() as f64).sqrt()})
            ),
            Err(e) => {
                eprintln!("{path}: {e}");
                std::process::exit(1);
            }
        }
    }
}
