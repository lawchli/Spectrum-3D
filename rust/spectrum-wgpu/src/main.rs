//! Standalone native spectrum application. Canonical shaders and look are exported from web/.
// A release build on Windows opens no console window when double-clicked; command-line use
// reattaches to the parent terminal in `main`.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
use spectrum_wgpu::audio::InputBuffer;
use spectrum_wgpu::gpu::Gpu;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
mod desktop;
mod fonts;
mod monitor;

#[derive(Clone, Debug)]
pub(crate) struct Options {
    root: PathBuf,
    headless: bool,
    frames: Option<u32>,
    width: u32,
    height: u32,
    /// `--width` or `--height` was given; otherwise the window sizes itself to the screen.
    size_given: bool,
    view: Option<String>,
    palette: Option<String>,
    style: Option<String>,
    axis: Option<String>,
    sub: Option<usize>,
    tilt: bool,
    keys: bool,
    peaks: bool,
    scale: Option<String>,
    msaa: Option<u32>,
    frame_latency: u32,
    input_buffer: InputBuffer,
    /// Yaw and pitch in degrees, replacing the preset's once it has settled (offscreen only).
    orbit: Option<(f64, f64)>,
    source: Option<String>,
    device: Option<usize>,
    file: Option<PathBuf>,
    listen: bool,
    output: Option<PathBuf>,
    settings: Option<PathBuf>,
    date: String,
    no_ui: bool,
    benchmark: bool,
    snapshot: bool,
    list_inputs: bool,
}
impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        // Packaged layouts: the macOS .app and the Linux folder keep Resources beside the
        // program's own folder; the Windows folder keeps it next to the exe.
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(PathBuf::from));
        let bundled = exe_dir
            .iter()
            .flat_map(|dir| [dir.parent().map(|p| p.join("Resources")), Some(dir.join("Resources"))])
            .flatten()
            .find(|p| p.join("toolchains/spectrum-assets/look.json").exists());
        let mut o = Self {
            root: bundled
                .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")),
            headless: false,
            frames: None,
            width: 1176,
            height: 800,
            size_given: false,
            view: None,
            palette: None,
            style: None,
            axis: None,
            sub: None,
            tilt: false,
            keys: false,
            peaks: false,
            scale: None,
            msaa: None,
            frame_latency: 1,
            input_buffer: InputBuffer::default(),
            orbit: None,
            source: None,
            device: None,
            file: None,
            listen: false,
            output: None,
            settings: None,
            date: today(),
            no_ui: false,
            benchmark: false,
            snapshot: false,
            list_inputs: false,
        };
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{arg} requires a value"));
            match arg.as_str() {
                "--file"=>o.file=Some(value()?.into()),
                "--listen"=>o.listen=true,
                "--window"=>o.headless=false,"--headless"=>o.headless=true,
                "--snapshot"=>{o.snapshot=true;o.headless=true;},
                "--benchmark"=>o.benchmark=true,"--no-ui"=>o.no_ui=true,"--list-inputs"=>o.list_inputs=true,
                "--frames"=>o.frames=Some(value()?.parse().map_err(|_|"invalid frame count")?),
                "--width"=>{o.width=value()?.parse().map_err(|_|"invalid width")?;o.size_given=true}
                "--height"=>{o.height=value()?.parse().map_err(|_|"invalid height")?;o.size_given=true}
                "--view"=>o.view=Some(value()?),"--palette"=>o.palette=Some(value()?),
                "--style"=>o.style=Some(value()?),"--source"=>o.source=Some(value()?),
                "--axis"=>o.axis=Some(value()?),"--tilt"=>o.tilt=true,"--keys"=>o.keys=true,"--peaks"=>o.peaks=true,
                "--scale"=>o.scale=Some(value()?),
                "--msaa"=>o.msaa=Some(value()?.parse::<u32>().ok().filter(|n|[0,1,4].contains(n)).map(spectrum_wgpu::gpu::normalize_msaa).ok_or("--msaa must be 0, 1 or 4")?),
                "--frame-latency"=>o.frame_latency=value()?.parse().ok().filter(|n|[1,2].contains(n)).ok_or("--frame-latency must be 1 or 2")?,
                "--input-buffer"=>o.input_buffer=value()?.parse()?,
                "--sub"=>o.sub=Some(value()?.parse().ok().filter(|n|[1,2,4,8].contains(n)).ok_or("--sub must be 1, 2, 4 or 8")?),
                "--orbit"=>{let v=value()?;let mut p=v.split(',').map(|n|n.parse::<f64>().ok().filter(|n|n.is_finite()));
                    match (p.next().flatten(),p.next().flatten(),p.next()) {(Some(yaw),Some(pitch),None)=>o.orbit=Some((yaw,pitch.clamp(-3.0,89.9))),_=>return Err("--orbit takes YAW,PITCH in degrees".into())}},
                "--device"=>o.device=Some(value()?.parse().map_err(|_|"invalid device index")?),
                "--output"=>o.output=Some(PathBuf::from(value()?)),
                "--settings"=>o.settings=Some(PathBuf::from(value()?)),
                "--assets-root"=>o.root=PathBuf::from(value()?),"--date"=>o.date=value()?,
                "--help"|"-h"=>return Err("Usage: spectrum-wgpu [--window | --headless | --snapshot] [--frames N] [--view front|top|perspective] [--palette tungsten|mel|green|dadaspec] [--style sculpt|contours|lattice|pure] [--axis log|mel] [--sub 1|2|4|8] [--tilt] [--keys] [--peaks] [--scale auto|1|0.85|0.7|0.5] [--msaa 0|1|4] [--frame-latency 1|2] [--input-buffer fixed|default] [--orbit YAW,PITCH] [--source test|test1|test2|microphone|off] [--file PATH] [--listen] [--device INDEX] [--list-inputs] [--width PX --height PX] [--output PATH.ppm] [--date YYYY-MM-DD] [--settings PATH] [--assets-root DIR] [--no-ui] [--benchmark]".into()),
                _=>return Err(format!("Unknown argument {arg}")),
            }
        }
        if o.width < 64 || o.height < 64 || o.width > 8192 || o.height > 8192 {
            return Err("window size must be 64..8192".into());
        }
        if o.frames == Some(0) {
            return Err("--frames must be positive".into());
        }
        if o.output.is_some() && !o.headless && o.frames.is_none() {
            o.frames = Some(120);
        }
        if o.date.len() != 10
            || o.date.chars().enumerate().any(|(i, c)| {
                if i == 4 || i == 7 {
                    c != '-'
                } else {
                    !c.is_ascii_digit()
                }
            })
        {
            return Err("--date must be YYYY-MM-DD".into());
        }
        for (value, valid) in [
            (&o.view, &["front", "top", "perspective"][..]),
            (&o.palette, &["tungsten", "mel", "green", "dadaspec"][..]),
            (&o.style, &["sculpt", "contours", "lattice", "pure"][..]),
            (
                &o.source,
                &["test", "test1", "test2", "microphone", "off"][..],
            ),
            (&o.axis, &["log", "mel"][..]),
            (&o.scale, &["auto", "1", "0.85", "0.7", "0.5"][..]),
        ] {
            if let Some(v) = value {
                if !valid.contains(&v.as_str()) {
                    return Err(format!("Unsupported value {v}"));
                }
            }
        }
        Ok(o)
    }
}
fn today() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 86400;
    let z = days as i64 + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    y += i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}
pub(crate) fn assets(root: &Path, snapshot: bool) -> Result<HashMap<String, Vec<u8>>, String> {
    let mut data = HashMap::new();
    let generated = root.join("toolchains/spectrum-assets");
    let mut dirs = vec![generated, root.join("toolchains/shader-lab/tungsten")];
    if snapshot {
        dirs.push(root.join("toolchains/wgpu-snapshot"));
    }
    for dir in dirs {
        for entry in fs::read_dir(&dir).map_err(|e|format!("{}: {e}. Run node scripts/export-shared.mjs and node scripts/shader-migrate.mjs first.",dir.display()))? {
            let e=entry.map_err(|e|e.to_string())?;
            if e.path().is_file(){data.insert(e.file_name().to_string_lossy().into_owned(),fs::read(e.path()).map_err(|e|e.to_string())?);}
        }
    }
    let dadaspec = root.join("toolchains/shader-lab/dadaspec");
    if !snapshot || dadaspec.exists() {
        for entry in fs::read_dir(&dadaspec).map_err(|e| format!("{}: {e}. Run node scripts/export-shared.mjs and node scripts/shader-migrate.mjs first.", dadaspec.display()))? {
            let e = entry.map_err(|e| e.to_string())?;
            if e.path().is_file() {
                data.insert(
                    format!("dadaspec/{}", e.file_name().to_string_lossy()),
                    fs::read(e.path()).map_err(|e| e.to_string())?,
                );
            }
        }
    }
    Ok(data)
}
pub(crate) fn apply_options(gpu: &mut Gpu, o: &Options) {
    if let Some(scene) = gpu.scene.as_mut() {
        if let Some(v) = &o.view {
            scene.settings.view = v.clone();
        }
        if let Some(v) = &o.palette {
            scene.set_palette(v);
        }
        if let Some(v) = &o.style {
            scene.settings.style = v.clone();
        }
        if let Some(v) = &o.axis {
            scene.settings.axis = v.clone();
        }
        if let Some(v) = o.sub {
            scene.settings.sub = v;
        }
        scene.settings.tilt |= o.tilt;
        scene.settings.keys |= o.keys;
        scene.settings.peaks |= o.peaks;
    }
}
fn headless(o: &Options) -> Result<(), String> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let data = assets(&o.root, o.snapshot)?;
    let mut g = if o.snapshot {
        pollster::block_on(Gpu::new(
            &instance,
            None,
            wgpu::TextureFormat::Rgba8Unorm,
            &data,
        ))
    } else {
        pollster::block_on(Gpu::new_live(
            &instance,
            None,
            wgpu::TextureFormat::Rgba8Unorm,
            &data,
            o.width,
            o.height,
        ))
    };
    apply_options(&mut g, o);
    if let Some(msaa) = o.msaa {
        g.set_msaa(msaa);
    }
    if let Some(scale) = &o.scale {
        g.set_render_scale(spectrum_core::RenderScale::new(scale).scale());
    }
    g.set_timing_enabled(o.benchmark);
    if !o.snapshot {
        for _ in 0..240 {
            g.advance(1.0 / 60.0);
        }
        if let (Some((yaw, pitch)), Some(scene)) = (o.orbit, g.scene.as_mut()) {
            scene.camera.target[0] = yaw;
            scene.camera.target[1] = pitch;
            scene.camera.settle();
            g.advance(0.0);
        }
    }
    let texture = g.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen"),
        size: wgpu::Extent3d {
            width: g.width,
            height: g.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target = texture.create_view(&Default::default());
    let start = Instant::now();
    let frames = o.frames.unwrap_or(1);
    let samples: Vec<_> = (0..frames).filter_map(|_| g.render(&target)).collect();
    let output = o
        .output
        .clone()
        .unwrap_or_else(|| o.root.join(format!("previews/{}-wgpu-native.ppm", o.date)));
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    g.save(&texture, &output);
    let report = serde_json::json!({"adapter":g.adapter,"frames":frames,"canvas":[g.width,g.height],"renderPixels":g.render_dimensions(),"renderScale":g.render_scale(),"msaa":g.msaa_samples(),"elapsedSeconds":start.elapsed().as_secs_f64(),"gpuMs":samples,"snapshot":o.snapshot,"view":o.view,"palette":o.palette,"style":o.style,"output":output,"method":"deterministic four-second test signal; native camera, uniforms, mesh and shared GLSL; offscreen readback is not display FPS"});
    let path = output.with_extension("json");
    fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).map_err(|e| e.to_string())?;
    println!("{report}");
    Ok(())
}
fn run() -> Result<(), String> {
    let o = Options::parse(std::env::args().skip(1))?;
    if o.list_inputs {
        for d in spectrum_wgpu::audio::input_devices()? {
            println!(
                "{}\t{}{}",
                d.index,
                d.name,
                if d.is_default { " (default)" } else { "" }
            );
        }
        return Ok(());
    }
    if o.headless {
        headless(&o)
    } else {
        desktop::run(o)
    }
}
/// A GUI-subsystem program starts without standard handles. Started from a terminal, print
/// to it: `--list-inputs`, `--headless` and the exit JSON stay readable. Redirected handles
/// (a pipe or a file) are already valid and left alone.
#[cfg(all(windows, not(debug_assertions)))]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
        fn GetStdHandle(std_handle: u32) -> isize;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    // SAFETY: plain Win32 calls with constant arguments and no pointers.
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        if out == 0 || out == -1 {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}
fn main() {
    #[cfg(all(windows, not(debug_assertions)))]
    attach_parent_console();
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_cli() {
        assert!(
            Options::parse(
                ["--headless", "--view", "front", "--frames", "1"]
                    .into_iter()
                    .map(str::to_owned)
            )
            .unwrap()
            .headless
        );
        assert!(Options::parse(["--date", "../oops"].into_iter().map(str::to_owned)).is_err());
        assert!(Options::parse(["--width", "0"].into_iter().map(str::to_owned)).is_err());
        assert!(Options::parse(["--palette", "unknown"].into_iter().map(str::to_owned)).is_err());
        let o = Options::parse(
            ["--axis", "mel", "--sub", "4", "--tilt", "--orbit", "160,15"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap();
        assert_eq!(
            (o.axis.as_deref(), o.sub, o.tilt, o.orbit),
            (Some("mel"), Some(4), true, Some((160.0, 15.0)))
        );
        assert!(Options::parse(["--sub", "3"].into_iter().map(str::to_owned)).is_err());
        assert!(Options::parse(["--axis", "bark"].into_iter().map(str::to_owned)).is_err());
        assert!(Options::parse(["--orbit", "10"].into_iter().map(str::to_owned)).is_err());
        let scale_msaa = Options::parse(
            ["--scale", "0.5", "--msaa", "0"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap();
        assert_eq!((scale_msaa.scale.as_deref(), scale_msaa.msaa), (Some("0.5"), Some(1)));
        assert_eq!(
            Options::parse(["--msaa", "4"].into_iter().map(str::to_owned))
                .unwrap()
                .msaa,
            Some(4)
        );
        assert!(Options::parse(["--scale", "0.3"].into_iter().map(str::to_owned)).is_err());
        assert!(Options::parse(["--msaa", "2"].into_iter().map(str::to_owned)).is_err());
        let defaults = Options::parse(std::iter::empty()).unwrap();
        assert_eq!(
            (defaults.frame_latency, defaults.input_buffer),
            (1, InputBuffer::Fixed)
        );
        for latency in ["1", "2"] {
            for buffer in ["fixed", "default"] {
                let o = Options::parse(
                    ["--frame-latency", latency, "--input-buffer", buffer]
                        .into_iter()
                        .map(str::to_owned),
                )
                .unwrap();
                assert_eq!(o.frame_latency, latency.parse::<u32>().unwrap());
                assert_eq!(o.input_buffer.as_str(), buffer);
            }
        }
        for args in [
            vec!["--frame-latency", "0"],
            vec!["--frame-latency", "3"],
            vec!["--frame-latency", "-1"],
            vec!["--frame-latency", "1.5"],
            vec!["--frame-latency", "bad"],
            vec!["--frame-latency"],
            vec!["--input-buffer", "5"],
            vec!["--input-buffer", "Fixed"],
            vec!["--input-buffer", ""],
            vec!["--input-buffer"],
        ] {
            assert!(Options::parse(args.into_iter().map(str::to_owned)).is_err());
        }
        let usage = Options::parse(["--help"].into_iter().map(str::to_owned)).unwrap_err();
        assert!(usage.contains("--frame-latency 1|2"));
        assert!(usage.contains("--input-buffer fixed|default"));
        assert_eq!(
            Options::parse(["--palette", "dadaspec"].into_iter().map(str::to_owned))
                .unwrap()
                .palette
                .as_deref(),
            Some("dadaspec")
        );
    }
}
