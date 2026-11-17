//! Background file decoding. Gapless packets remove the LAME delay/padding in MP3.
use super::*;
use symphonia::core::{
    audio::SampleBuffer,
    codecs::DecoderOptions,
    errors::Error,
    formats::FormatOptions,
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
    probe::Hint,
};

pub fn decode(source: Box<dyn MediaSource>, extension: Option<&str>) -> Result<Vec<f32>, String> {
    decode_counted(source, extension).map(|(pcm, _)| pcm)
}
/// Also returns how many damaged packets were skipped. As symphonia intends, a packet that
/// does not decode is dropped and the rest of the file plays (downloaded MP3s sometimes
/// carry a bad frame); only a file that yields nothing at all is an error.
fn decode_counted(
    source: Box<dyn MediaSource>,
    extension: Option<&str>,
) -> Result<(Vec<f32>, usize), String> {
    let mut hint = Hint::new();
    if let Some(ext) = extension {
        hint.with_extension(ext);
    }
    let stream = MediaSourceStream::new(source, Default::default());
    let mut format = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|e| e.to_string())?
        .format;
    let track = format.default_track().ok_or("No audio track")?;
    let id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| e.to_string())?;
    let mut mono = Vec::new();
    let mut rate = None;
    let mut skipped = 0;
    let mut first_error = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.to_string()),
        };
        if packet.track_id() != id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(Error::DecodeError(e)) => {
                skipped += 1;
                first_error.get_or_insert(e);
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        let spec = *decoded.spec();
        if rate.replace(spec.rate).is_some_and(|r| r != spec.rate) {
            return Err("Changing sample rate inside a file is unsupported".into());
        }
        let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        mono.extend(
            buffer
                .samples()
                .chunks_exact(spec.channels.count())
                .map(|frame| {
                    let v = frame.iter().sum::<f32>() / frame.len() as f32;
                    if v.is_finite() {
                        v
                    } else {
                        0.0
                    }
                }),
        );
    }
    let Some(rate) = rate.filter(|_| !mono.is_empty()) else {
        return Err(first_error.map_or("Audio file is empty".into(), |e| e.to_string()));
    };
    if skipped > 0 {
        eprintln!(
            "Skipped {skipped} damaged packets ({})",
            first_error.unwrap_or_default()
        );
    }
    if rate == ANALYSIS_SAMPLE_RATE {
        return Ok((mono, skipped));
    }
    // Flush the sinc filter, then keep exactly the duration of the input.
    let count = (mono.len() as u64 * ANALYSIS_SAMPLE_RATE as u64 / rate as u64) as usize;
    let mut converter = Resampler::new(rate, ANALYSIS_SAMPLE_RATE);
    let mut converted = Vec::with_capacity(count + 32);
    converter.process(&mono, &mut converted);
    converter.process(&[0.; 32], &mut converted);
    converted.truncate(count);
    Ok((converted, skipped))
}
pub fn file(path: &std::path::Path) -> Result<Vec<f32>, String> {
    decode(
        Box::new(std::fs::File::open(path).map_err(|e| e.to_string())?),
        path.extension().and_then(|e| e.to_str()),
    )
}
pub fn test_track(index: usize) -> Result<Vec<f32>, String> {
    let bytes: &'static [u8] = match index {
        0 => include_bytes!("../../assets/test-audio/test-1.mp3"),
        _ => include_bytes!("../../assets/test-audio/test-2.mp3"),
    };
    decode(Box::new(std::io::Cursor::new(bytes)), Some("mp3"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_mp3_is_current_and_gapless_length_matches_rendered_wav() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        assert!(std::process::Command::new("node")
            .arg(root.join("scripts/check-test-audio.mjs"))
            .status()
            .expect("Node 22 is required for shared asset checks")
            .success());
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/test-audio/manifest.json")).unwrap();
        for i in 0..2 {
            let samples = test_track(i).unwrap();
            assert_eq!(
                samples.len() as u64,
                manifest["tracks"][i]["samples"].as_u64().unwrap()
            );
            assert!(samples.iter().all(|s| s.is_finite()));
            let peak = samples.iter().fold(0f32, |p, s| p.max(s.abs()));
            assert!(peak > 0.1 && peak < 1.0, "track {i}: {peak}");
            println!(
                "track {}: {} samples, peak {}, wrap step {}",
                i + 1,
                samples.len(),
                peak,
                (samples[0] - samples[samples.len() - 1]).abs()
            );
        }
    }
    #[test]
    fn damaged_packets_are_skipped() {
        let mut bytes = include_bytes!("../../assets/test-audio/test-1.mp3").to_vec();
        let whole = test_track(0).unwrap();
        // Garble a stretch inside several frames, well past the header.
        for i in 1..=8 {
            let at = bytes.len() * i / 10;
            for (j, b) in bytes[at..at + 400].iter_mut().enumerate() {
                *b = (j * 37 % 251) as u8;
            }
        }
        let (pcm, skipped) =
            decode_counted(Box::new(std::io::Cursor::new(bytes)), Some("mp3")).unwrap();
        assert!(skipped > 0, "the damage reached the decoder");
        assert!(pcm.iter().all(|s| s.is_finite()));
        // Each skipped MP3 frame is 1152 samples; the rest of the song is all there.
        let lost = whole.len() as f64 - pcm.len() as f64;
        assert!(
            lost >= 0. && lost <= (skipped * 1152 * 2) as f64,
            "{lost} for {skipped}"
        );
        println!("skipped {skipped} packets, {lost} samples shorter");
    }
    #[test]
    fn invalid_files_report_errors() {
        assert!(decode(
            Box::new(std::io::Cursor::new(b"invalid".to_vec())),
            Some("mp3")
        )
        .is_err());
    }
}
