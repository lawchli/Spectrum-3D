//! The host-side behaviour that lives outside the shared shaders and look.json is written
//! twice, in web/src and in Rust. These tests run the web modules in Node and compare, so a
//! change on the web side fails here until the Rust side follows.
use serde_json::Value;
use spectrum_core::{
    band_count, band_hz, band_to_u, envelope, u_to_band, DadaspecChroma, DisplaySpectrum, History,
    PeakTracker, RenderScale, HISTORY_ROWS, LEVELS,
};
use spectrum_wgpu::gpu::scene::front_facing;

const SCRIPT: &str = r#"
import { readFileSync } from 'node:fs';
import { bandToU, uToBand, bandHz } from './web/src/bands.js';
import { frontFacing } from './web/src/labels.js';
import { PeakTracker } from './web/src/peaks.js';
// main.js is the page itself and cannot be imported here; take the one function.
const source = readFileSync('./web/src/main.js', 'utf8').match(/function envelope\([^]*?\r?\n}\r?\n/);
if (!source) throw Error('envelope() not found in web/src/main.js');
const envelope = new Function(`${source[0]}; return envelope;`)();
const radiusSource = readFileSync('./web/src/main.js', 'utf8').match(/function envelopeRadius\([^]*?\r?\n}\r?\n/);
if (!radiusSource) throw Error('envelopeRadius() not found in web/src/main.js');
const envelopeRadius = new Function(`${radiusSource[0]}; return envelopeRadius;`)();
const radii = [1, 2, 4, 8].map((sub) => ({ sub, r: envelopeRadius(sub) }));
const mels = [0, 0.25, 0.5, 1];
const axis = [];
for (const mel of mels) {
  for (const band of [0, 0.5, 12, 33.3, 57, 71.2131, 100, 126]) axis.push({ mel, band, u: bandToU(band, mel) });
}
const inverse = [];
for (const mel of mels) for (const u of [0, 0.1, 0.25, 0.5, 0.9, 1]) inverse.push({ mel, u, band: uToBand(u, mel) });
const facing = [[0, 0, 1], [0.3, 0.2, 0.1], [0, 0.5, -0.05], [0, 0.2, -0.11], [0.5, 0.1, -0.15], [0, 0, -1]]
  .map((back) => ({ back, value: frontFacing(back) }));
let seed = 12345;
const random = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648;
const src = Float32Array.from({ length: 505 }, () => -100 + 70 * random());
const envelopes = [0, 1, 2, 4].map((r) => {
  const dst = new Float32Array(505), scratch = new Float32Array(505);
  envelope(src, dst, scratch, r);
  return { r, dst: [...dst] };
});
// Two peaks, one drifting and one that stops sounding, over a slowly moving floor.
const tracker = new PeakTracker(), frames = [];
for (let f = 0; f < 48; f++) {
  const shown = new Float32Array(505), raw = new Float32Array(505);
  for (let k = 0; k < 505; k++) {
    const band = k / 4;
    const a = -18 - 3 * ((band - (57.1 + f * 0.01)) / 0.35) ** 2;
    const b = (f < 30 ? -31 : -31 - (f - 30) * 6) - 3 * ((band - 88.6) / 0.4) ** 2;
    const c = -52 - 3 * ((band - 24.4) / 0.5) ** 2;
    raw[k] = Math.max(-96 + 2 * Math.sin(k * 0.7 + f), a, f < 30 ? b : -120, c);
    shown[k] = Math.max(-96 + 2 * Math.sin(k * 0.7 + f), a, b, c);
  }
  tracker.update(shown, raw, 1 / 60, (band) => bandHz(band) * 1.002, 4);
  frames.push({ shown: [...shown], raw: [...raw], tracks: tracker.tracks.map((t) => ({ band: t.band, db: t.db, hz: t.hz, alpha: t.alpha, text: t.text })) });
}
console.log(JSON.stringify({ axis, inverse, facing, src: [...src], envelopes, radii, frames }));
"#;

fn reference() -> Value {
    let output = std::process::Command::new("node")
        .args(["--input-type=module", "-e", SCRIPT])
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .expect("Node is needed for the web reference");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn f(v: &Value) -> f64 {
    v.as_f64().expect("number")
}
fn floats(v: &Value) -> Vec<f32> {
    v.as_array().unwrap().iter().map(|x| f(x) as f32).collect()
}
fn close(a: f64, b: f64, what: &str) {
    assert!(
        (a - b).abs() <= 1e-9 * b.abs().max(1.0),
        "{what}: rust {a} web {b}"
    );
}

#[test]
fn frequency_axis_envelope_facing_and_peak_tracking_match_the_web() {
    let r = reference();
    for case in r["axis"].as_array().unwrap() {
        close(
            band_to_u(f(&case["band"]), f(&case["mel"])),
            f(&case["u"]),
            "bandToU",
        );
    }
    for case in r["inverse"].as_array().unwrap() {
        close(
            u_to_band(f(&case["u"]), f(&case["mel"])),
            f(&case["band"]),
            "uToBand",
        );
    }
    for case in r["facing"].as_array().unwrap() {
        let b = &case["back"];
        close(
            front_facing([f(&b[0]), f(&b[1]), f(&b[2])]) as f64,
            f(&case["value"]) as f32 as f64,
            "frontFacing",
        );
    }
    let src = floats(&r["src"]);
    for case in r["envelopes"].as_array().unwrap() {
        let (mut dst, mut scratch) = (vec![0.0; src.len()], vec![0.0; src.len()]);
        envelope(
            &src,
            &mut dst,
            &mut scratch,
            case["r"].as_u64().unwrap() as usize,
        );
        assert_eq!(dst, floats(&case["dst"]), "envelope r={}", case["r"]);
    }
    // The envelope's width per bands-per-semitone: none at one band per semitone.
    for case in r["radii"].as_array().unwrap() {
        let sub = case["sub"].as_u64().unwrap() as usize;
        assert_eq!(
            DisplaySpectrum::new(band_count(sub)).smooth,
            case["r"].as_u64().unwrap() as usize,
            "envelope radius at {sub} bands per semitone"
        );
    }
    let mut tracker = PeakTracker::default();
    let mut labelled = 0;
    for (index, frame) in r["frames"].as_array().unwrap().iter().enumerate() {
        tracker.update(
            &floats(&frame["shown"]),
            &floats(&frame["raw"]),
            1.0 / 60.0,
            |band| Some(band_hz(band) * 1.002),
            4,
        );
        let web = frame["tracks"].as_array().unwrap();
        assert_eq!(tracker.tracks.len(), web.len(), "frame {index}: track count");
        for (rust, web) in tracker.tracks.iter().zip(web) {
            let what = format!("frame {index}");
            close(rust.band, f(&web["band"]), &what);
            close(rust.db, f(&web["db"]), &what);
            close(rust.hz, f(&web["hz"]), &what);
            close(rust.alpha, f(&web["alpha"]), &what);
            // The web keeps note and cents in one string and splits it when it draws.
            let text = rust.text.as_ref().unwrap();
            assert_eq!(
                format!("{} {}", text[0], text[1]),
                web["text"][0].as_str().unwrap(),
                "{what}"
            );
            assert_eq!(text[2], web["text"][1].as_str().unwrap(), "{what}");
            labelled += 1;
        }
    }
    assert!(labelled > 100, "the sequence exercises the tracker");
}

#[test]
fn render_scale_matches_the_web() {
    const SCALE_SCRIPT: &str = r#"
import { RenderScale } from './web/src/render-scale.js';
const steps = [];
const push = (dt, gpuMs, active = true) => steps.push([dt, gpuMs, active]);
// 1. CPU-bound slow frames (gpuMs < 50% of frame) must NOT lower scale.
for (let i = 0; i < 120; i++) push(0.025, 5.0);
// 2. GPU-bound slow frames -> lower 1 -> 0.85, then improve (14 ms) -> keeps 0.85.
for (let i = 0; i < 90; i++) push(0.025, 20.0);
for (let i = 0; i < 400; i++) push(0.014, 9.0);
// 3. Raise 0.85 -> 1 after 10 s fast, then fail within 7 s -> drops to 0.85 with penalty x2.
for (let i = 0; i < 800; i++) push(0.012, 6.0);
for (let i = 0; i < 120; i++) push(0.025, 20.0);
// 4. Improve, then require 20 s (10 s * penalty 2) before raising back to 1.
for (let i = 0; i < 400; i++) push(0.014, 9.0);
for (let i = 0; i < 1600; i++) push(0.012, 6.0);
// 5. Lower to 0.85 then 0.7 where lowering 0.85 -> 0.7 does NOT help -> undo and hold 60s.
for (let i = 0; i < 90; i++) push(0.025, 20.0);
for (let i = 0; i < 400; i++) push(0.016, 10.0);
for (let i = 0; i < 90; i++) push(0.025, 20.0);
for (let i = 0; i < 250; i++) push(0.025, 20.0);
// 6. During 60 s hold, slow frames do NOT lower; then raise back to 1 without timer (30 s wait).
for (let i = 0; i < 120; i++) push(0.025, null);
// Also exercise inactive and dt > 0.1 stall rejection.
push(0.2, 20.0, true);
push(0.025, 20.0, false);
for (let i = 0; i < 3500; i++) push(0.012, null);

const rs = new RenderScale('auto');
const out = steps.map(([dt, gpuMs, active]) => ({
  changed: rs.update(dt, gpuMs, active),
  scale: rs.scale,
}));
console.log(JSON.stringify({ steps, out }));
"#;
    let output = std::process::Command::new("node")
        .args(["--input-type=module", "-e", SCALE_SCRIPT])
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .expect("Node is needed for the web reference");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let data: Value = serde_json::from_slice(&output.stdout).unwrap();
    let steps = data["steps"].as_array().unwrap();
    let out = data["out"].as_array().unwrap();
    let mut rs = RenderScale::new("auto");
    let mut changes = 0usize;
    for (idx, (step, web)) in steps.iter().zip(out.iter()).enumerate() {
        let arr = step.as_array().unwrap();
        let dt = arr[0].as_f64().unwrap();
        let gpu_ms = arr[1].as_f64();
        let active = arr[2].as_bool().unwrap();
        let changed = rs.update(dt, gpu_ms, active);
        assert_eq!(changed, web["changed"].as_bool().unwrap(), "step {idx}: changed");
        close(rs.scale_f64(), f(&web["scale"]), &format!("step {idx}: scale"));
        if changed {
            changes += 1;
        }
    }
    assert!(changes >= 6, "sequence exercises multiple transitions: {changes}");
}

#[test]
fn dadaspec_chroma_matches_the_web() {
    const CHROMA_SCRIPT: &str = r#"
import { Renderer } from './web/styles/dadaspec/renderer.js';
import { LOOK } from './web/styles/dadaspec/look.js';
import { bandCount, bandHz } from './web/src/bands.js';

const gl = new Proxy({}, {
  get(_, prop) {
    if (prop === 'COMPILE_STATUS' || prop === 'LINK_STATUS') return 1;
    if (prop === 'ACTIVE_UNIFORMS') return 2;
    if (prop === 'MAX_TEXTURE_SIZE') return 3;
    return (...args) => {
      if (prop === 'getParameter') {
        if (args[0] === 1) return true;
        if (args[0] === 2) return 0;
        if (args[0] === 3) return 8192;
      }
      if (prop === 'getShaderParameter' || prop === 'getProgramParameter') return true;
      return {};
    };
  },
});

const cases = [];
for (const [sub, rate, floor] of [[1, 60, -96], [2, 120, -96], [4, 240, -120]]) {
  const look = structuredClone(LOOK);
  look.db.floor = floor;
  const renderer = new Renderer(gl, look, {});
  renderer.setBands(sub);
  renderer.lastRowRate = rate;
  const count = bandCount(sub);
  const rows = [];

  for (let t = 0; t < 75; t++) {
    const bytes = new Uint8Array(count);
    for (let i = 0; i < count; i++) {
      const hz = bandHz(i / sub);
      let db = -112;
      if (t < 15) {
        if (hz < 180) db = -24 - Math.abs(hz - 90) * 0.15;
      } else if (t < 30) {
        db = -18 - ((i * 7 + t) % 13) * 1.4;
      } else if (t < 45) {
        if (hz > 1600) db = -20 - (hz > 16500 ? 6 : 0);
      } else if (t < 58) {
        db = -116;
      } else {
        if (hz >= 220 && hz <= 1300) db = -22 - Math.abs(hz - 650) * 0.02;
      }
      bytes[i] = Math.max(0, Math.min(255, Math.round((db + 120) * 2)));
    }
    renderer.pushRow(bytes);
    rows.push([...bytes]);
  }

  const snapLevels = (r) => r.chromaLevels.map((arr, l) => {
    const blocks = Math.floor(r.historyStore.rowsPushed / (1 << l));
    return [...arr.subarray(0, blocks * 4)];
  });
  const pushLevels = snapLevels(renderer);
  const pushLive = [...renderer.liveChroma];

  // Now force a full CPU history rebuild via syncFromHistoryStore() and push 5 more rows
  // so partial remainder accumulation is also verified.
  renderer.syncFromHistoryStore();
  const syncLevels = snapLevels(renderer);
  const syncLive = [...renderer.liveChroma];

  const extraRows = [];
  for (let t = 0; t < 5; t++) {
    const bytes = new Uint8Array(count).fill(t % 2 === 0 ? 185 : 210);
    renderer.pushRow(bytes);
    extraRows.push([...bytes]);
  }
  const afterExtraLevels = snapLevels(renderer);
  const afterExtraLive = [...renderer.liveChroma];

  cases.push({
    sub,
    rate,
    floor,
    rows,
    extraRows,
    pushLevels,
    pushLive,
    syncLevels,
    syncLive,
    afterExtraLevels,
    afterExtraLive,
  });
}
console.log(JSON.stringify(cases));
"#;
    let output = std::process::Command::new("node")
        .args(["--input-type=module", "-e", CHROMA_SCRIPT])
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .expect("Node is needed for the web reference");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cases: Value = serde_json::from_slice(&output.stdout).unwrap();
    for case in cases.as_array().unwrap() {
        let sub = case["sub"].as_u64().unwrap() as usize;
        let rate = case["rate"].as_u64().unwrap() as u32;
        let floor = case["floor"].as_f64().unwrap();
        let count = band_count(sub);
        let mut history = History::new(count, HISTORY_ROWS);
        let mut chroma = DadaspecChroma::new(sub, [200.0, 1500.0]);

        for row in case["rows"].as_array().unwrap() {
            let bytes: Vec<u8> = row.as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u8).collect();
            let idx = history.written;
            history.push(&bytes);
            chroma.push_row(idx, &bytes, rate, floor);
        }

        let check_levels = |chroma: &DadaspecChroma, web_levels: &Value, total_rows: usize, phase: &str| {
            for (l, web_arr) in web_levels.as_array().unwrap().iter().enumerate() {
                let blocks = total_rows / (1 << l);
                let web_bytes = web_arr.as_array().unwrap();
                assert_eq!(web_bytes.len(), blocks * 4);
                for (i, w) in web_bytes.iter().enumerate() {
                    let rb = chroma.levels[l][i] as i32;
                    let wb = w.as_u64().unwrap() as i32;
                    assert!(
                        (rb - wb).abs() <= 1,
                        "sub={sub} rate={rate} {phase} level {l} byte {i}: rust {rb} vs web {wb}"
                    );
                }
            }
        };
        let check_live = |chroma: &DadaspecChroma, web_live: &Value, phase: &str| {
            for (c, w) in web_live.as_array().unwrap().iter().enumerate() {
                let rl = chroma.live_chroma[c] as f64;
                let wl = w.as_f64().unwrap();
                assert!(
                    (rl - wl).abs() <= 1e-4,
                    "sub={sub} rate={rate} {phase} live_chroma[{c}]: rust {rl} vs web {wl}"
                );
            }
        };

        let rows_len = case["rows"].as_array().unwrap().len();
        check_levels(&chroma, &case["pushLevels"], rows_len, "push");
        check_live(&chroma, &case["pushLive"], "push");

        let mut synced = DadaspecChroma::new(sub, [200.0, 1500.0]);
        synced.sync_from_history(&history, sub, rate, floor);
        assert!(LEVELS == 6);
        check_levels(&synced, &case["syncLevels"], rows_len, "sync");
        check_live(&synced, &case["syncLive"], "sync");

        for row in case["extraRows"].as_array().unwrap() {
            let bytes: Vec<u8> = row.as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u8).collect();
            let idx = history.written;
            history.push(&bytes);
            synced.push_row(idx, &bytes, rate, floor);
        }
        let extra_len = rows_len + case["extraRows"].as_array().unwrap().len();
        check_levels(&synced, &case["afterExtraLevels"], extra_len, "afterExtra");
        check_live(&synced, &case["afterExtraLive"], "afterExtra");
    }
}
