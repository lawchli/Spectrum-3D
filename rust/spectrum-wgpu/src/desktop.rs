mod panel;
use crate::{apply_options, assets, monitor, Options};
use spectrum_wgpu::{
    audio::{decode, input_devices, AnalysisWorker, InputDevice, InputStatus},
    gpu::{
        scene::{PeakLabel, Scene, DEFAULT_SUB},
        Gpu,
    },
};
use std::{
    fs,
    path::PathBuf,
    sync::{mpsc, Arc},
    time::Instant,
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

/// The smallest window, in logical pixels: the panel and a usable picture beside it.
const MIN_SIZE: [f64; 2] = [640., 420.];

/// When a setting the command line replaced goes back to the file (F8).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Release {
    /// Until the user changes it: compared with its value as the run began.
    Changed,
    /// Until the user picks another source or device. These settle after start (a file
    /// decodes, a device resolves its id), so comparing values would not tell.
    Source,
    /// The camera goes with the view.
    WithView,
}
/// A saved setting the command line replaced for this run only.
#[derive(Clone, Debug)]
struct Kept {
    top: &'static str,
    sub: Option<&'static str>,
    saved: serde_json::Value,
    release: Release,
}
impl Kept {
    fn at<'a>(&self, v: &'a serde_json::Value) -> &'a serde_json::Value {
        match self.sub {
            Some(sub) => &v[self.top][sub],
            None => &v[self.top],
        }
    }
}
/// Command-line settings hold for this run only: puts back what the file had for each one
/// the user has not changed since the run began.
fn keep_saved(
    value: &mut serde_json::Value,
    start: &serde_json::Value,
    kept: &[Kept],
    source: bool,
) {
    let view_kept = kept
        .iter()
        .any(|k| k.sub == Some("view") && k.at(value) == k.at(start));
    for k in kept {
        let keep = match k.release {
            Release::Changed => k.at(value) == k.at(start),
            Release::Source => source,
            Release::WithView => view_kept,
        };
        if keep {
            match k.sub {
                Some(sub) => value[k.top][sub] = k.saved.clone(),
                None => value[k.top] = k.saved.clone(),
            }
        }
    }
}
/// The settings the command line replaces, from what the file had before they applied.
fn kept_settings(o: &Options, saved: &serde_json::Value) -> Vec<Kept> {
    let mut kept = Vec::new();
    let mut keep = |top, sub, release| {
        let mut k = Kept {
            top,
            sub,
            saved: serde_json::Value::Null,
            release,
        };
        k.saved = k.at(saved).clone();
        kept.push(k);
    };
    for (given, key) in [
        (o.view.is_some(), "view"),
        (o.palette.is_some(), "palette"),
        // A colour style brings its own plot style.
        (o.style.is_some() || o.palette.is_some(), "style"),
        (o.axis.is_some(), "axis"),
        (o.sub.is_some(), "sub"),
        (o.tilt, "tilt"),
        (o.keys, "keys"),
        (o.peaks, "peaks"),
    ] {
        if given {
            keep("settings", Some(key), Release::Changed);
        }
    }
    if o.palette.is_some() {
        keep("settings", Some("styleOptions"), Release::Changed);
    }
    if o.view.is_some() {
        keep("camera", None, Release::WithView);
    }
    if o.scale.is_some() {
        keep("renderScale", None, Release::Changed);
    }
    if o.msaa.is_some() {
        keep("msaa", None, Release::Changed);
    }
    if o.source.is_some() || o.file.is_some() || o.device.is_some() {
        for key in ["source", "device", "deviceId", "deviceName"] {
            keep(key, None, Release::Source);
        }
    }
    kept
}

/// The first window, in logical pixels: most of the screen, so the monitor fits in one row
/// beside the open panel, within a desktop-sized cap. Without a screen to go by (Wayland
/// gives none) it is 1280 × 800.
fn window_size(screen: Option<[f64; 2]>) -> [f64; 2] {
    let Some([w, h]) = screen.filter(|[w, h]| *w > 0. && *h > 0.) else {
        return [1280., 800.];
    };
    [
        (w * 0.9).min(1680.).max(MIN_SIZE[0].min(w)).round(),
        (h * 0.85).min(1050.).max(MIN_SIZE[1].min(h)).round(),
    ]
}

struct App {
    options: Options,
    window: Option<Arc<Window>>,
    surface: Option<wgpu::Surface<'static>>,
    gpu: Option<Gpu>,
    config: Option<wgpu::SurfaceConfiguration>,
    ctx: egui::Context,
    input: Option<egui_winit::State>,
    painter: Option<egui_wgpu::Renderer>,
    worker: Option<AnalysisWorker>,
    devices: Vec<InputDevice>,
    device_enumeration: Option<mpsc::Receiver<Result<Vec<InputDevice>, String>>>,
    devices_requested: bool,
    last_devices: Instant,
    source: String,
    device: Option<usize>,
    device_id: Option<String>,
    device_name: Option<String>,
    worker_config: (String, Option<usize>, u32, usize),
    strings: Arc<serde_json::Value>,
    lang: String,
    error: Option<String>,
    decoded: Option<Arc<Vec<f32>>>,
    loading: Option<mpsc::Receiver<Result<Option<(String, Arc<Vec<f32>>)>, String>>>,
    file_name: Option<String>,
    listen: bool,
    last_frame: Instant,
    frames: u32,
    /// The performance panel (`M`), remembered with the settings.
    monitor: bool,
    mon_active: bool,
    render_scale: spectrum_core::RenderScale,
    msaa: u32,
    /// Panels hidden with `H`, remembered with the settings. `--no-ui` hides them for this
    /// run only and is not saved.
    hide_ui: bool,
    /// The docks folded to their rail or head, the monitor columns shown and the analysis
    /// drawer, remembered with the settings as on the web.
    panel_collapsed: bool,
    hud_collapsed: bool,
    monitor_parts: [bool; 5],
    advanced_open: bool,
    /// Height of the panel's lower part last frame, to keep it at the bottom.
    aux_height: f32,
    mon: monitor::Monitor,
    started: std::time::Instant,
    cursor: (f64, f64),
    drag: Option<MouseButton>,
    shift: bool,
    settings_path: PathBuf,
    persisted: String,
    /// Settings the command line replaced for this run, the settings as the run began, and
    /// whether the source and device are still the command line's (F8).
    cli_kept: Vec<Kept>,
    cli_start: serde_json::Value,
    cli_source: bool,
}
fn settings_path() -> PathBuf {
    if let Some(p) = std::env::var_os("SPECTRUM_SETTINGS_PATH") {
        return p.into();
    }
    #[cfg(target_os = "macos")]
    let base = PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support");
    #[cfg(target_os = "windows")]
    let base = PathBuf::from(std::env::var_os("APPDATA").unwrap_or_else(|| ".".into()));
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    let path = base.join("spectrum-3d/settings.json");
    // Builds before 0.3.0 kept their settings in a folder named spectrum-visualizer: carry them over once.
    let old = base.join("spectrum-visualizer/settings.json");
    if !path.exists() && old.exists() {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::copy(&old, &path);
    }
    path
}
fn text(strings: &serde_json::Value, lang: &str, key: &str) -> String {
    strings[lang][key]
        .as_str()
        .or_else(|| strings["en"][key].as_str())
        .unwrap_or(key)
        .to_owned()
}
/// Web label colours for the peak labels (web/style.css), with the label's own fade.
fn tint(rgb: [u8; 3], opacity: f32, alpha: f32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        rgb[0],
        rgb[1],
        rgb[2],
        (opacity * alpha * 255.0).round() as u8,
    )
}
/// One main-peak label: note and cents, frequency and level, in a small box above the peak
/// with a tick down to it. The loudest is amber. Layout follows `.peak` in web/style.css.
fn paint_peak(painter: &egui::Painter, peak: &PeakLabel, scale: f32) {
    const INK_1: [u8; 3] = [234, 237, 242];
    const INK_2: [u8; 3] = [142, 151, 164];
    const ACCENT: [u8; 3] = [232, 162, 70];
    const ACCENT_INK: [u8; 3] = [245, 207, 150];
    let a = peak.alpha;
    // The fonts at hand have no narrow no-break space (the web puts one before each unit).
    let galley = |text: &str, size: f32, color| {
        painter.layout_no_wrap(
            text.replace('\u{202f}', " "),
            egui::FontId::proportional(size),
            color,
        )
    };
    let note = galley(
        &peak.text[0],
        13.,
        tint(if peak.main { ACCENT_INK } else { INK_1 }, 1., a),
    );
    let cents = galley(
        &peak.text[1],
        11.,
        tint(if peak.main { INK_1 } else { INK_2 }, 1., a),
    );
    let meta = galley(&peak.text[2], 11., tint(INK_2, 1., a));
    let width = (note.size().x + 6. + cents.size().x).max(meta.size().x);
    let height = note.size().y + 1. + meta.size().y;
    let anchor = egui::pos2(peak.x / scale, peak.y / scale);
    let rect = egui::Rect::from_min_size(
        egui::pos2(anchor.x - width / 2. - 6., anchor.y - 8. - height - 6.),
        egui::vec2(width + 12., height + 6.),
    );
    painter.rect_filled(rect, 2., tint([10, 12, 16], 0.92, a));
    painter.rect_stroke(
        rect,
        2.,
        egui::Stroke::new(
            1.,
            if peak.main {
                tint(ACCENT, 0.38, a)
            } else {
                tint([255, 255, 255], 0.14, a)
            },
        ),
        egui::StrokeKind::Inside,
    );
    let tick = if peak.main {
        tint(ACCENT, 1., a)
    } else {
        tint(INK_1, 0.55, a)
    };
    if peak.main {
        painter.line_segment(
            [rect.left_top(), rect.left_bottom()],
            egui::Stroke::new(2., tick),
        );
    }
    painter.line_segment(
        [
            egui::pos2(anchor.x, rect.bottom()),
            egui::pos2(anchor.x, rect.bottom() + 6.),
        ],
        egui::Stroke::new(1., tick),
    );
    let left = rect.left() + 6.;
    let top = rect.top() + 3.;
    let baseline = top + note.size().y;
    painter.galley(egui::pos2(left, top), note.clone(), egui::Color32::WHITE);
    painter.galley(
        egui::pos2(left + width - cents.size().x, baseline - cents.size().y),
        cents,
        egui::Color32::WHITE,
    );
    painter.galley(egui::pos2(left, baseline + 1.), meta, egui::Color32::WHITE);
}
impl App {
    fn choose_file(&mut self, path: Option<PathBuf>) {
        let (tx, rx) = mpsc::channel();
        self.loading = Some(rx);
        self.error = None;
        std::thread::spawn(move || {
            let path = path.or_else(|| {
                rfd::FileDialog::new()
                    .add_filter("Audio", &["wav", "mp3", "flac", "aac", "m4a", "ogg", "oga"])
                    .pick_file()
            });
            let result = path
                .map(|path| {
                    decode::file(&path).map(|pcm| {
                        (
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                            Arc::new(pcm),
                        )
                    })
                })
                .transpose();
            let _ = tx.send(result);
        });
    }
    fn poll_file(&mut self) {
        let result = self.loading.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(result) = result {
            self.loading = None;
            match result {
                Ok(Some((name, pcm))) => {
                    self.decoded = Some(pcm);
                    self.file_name = Some(name);
                    self.source = "file".into();
                    self.worker_config.0.clear();
                }
                Ok(None) => {}
                Err(e) => self.error = Some(e),
            }
        }
    }
    fn poll_devices(&mut self) {
        // Enumeration itself can instantiate a platform audio unit. Keep it lazy and
        // off the event loop so an unresponsive device cannot prevent the window opening.
        if self.source == "microphone"
            && self.device_enumeration.is_none()
            && (!self.devices_requested || self.last_devices.elapsed().as_secs_f64() >= 1.)
        {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(input_devices());
            });
            self.device_enumeration = Some(rx);
            self.devices_requested = true;
            self.last_devices = Instant::now();
        }
        let result = self
            .device_enumeration
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        if let Some(result) = result {
            self.device_enumeration = None;
            match result {
                Ok(devices) => {
                    self.devices = devices;
                    if self.source == "microphone" && self.worker_config.0 == self.source {
                        if let Some(worker) = &self.worker {
                            if let Some(info) = worker.input_device() {
                                if worker.input_status() == Some(InputStatus::Running)
                                    && !self.devices.iter().any(|d| d.id == info.id)
                                {
                                    worker.input_disconnected();
                                }
                                self.device_id = Some(info.id);
                                self.device_name = Some(info.raw_name);
                            }
                        }
                        let selected = self
                            .device_id
                            .as_ref()
                            .and_then(|id| self.devices.iter().find(|d| &d.id == id));
                        self.device = selected.map(|info| info.index);
                        // Enumeration index changes are not a user device switch.
                        self.worker_config.1 = self.device;
                        if let Some(info) = selected {
                            self.device_name = Some(info.raw_name.clone());
                        }
                    }
                }
                Err(error) => self.error = Some(error),
            }
        }
    }
    fn settings_value(&self, scene: &Scene) -> serde_json::Value {
        let monitor_parts: serde_json::Map<_, _> = monitor::Part::ALL
            .iter()
            .zip(self.monitor_parts)
            .map(|(p, on)| (p.key().to_owned(), on.into()))
            .collect();
        serde_json::json!({"version":2,"settings":scene.settings.to_json(),"camera":scene.camera.target,"lang":self.lang,"source":self.source,"device":self.device,"deviceId":self.device_id,"deviceName":self.device_name,"hideUi":self.hide_ui,"monitor":self.monitor,"panelCollapsed":self.panel_collapsed,"hudCollapsed":self.hud_collapsed,"advancedOpen":self.advanced_open,"monitorParts":monitor_parts,"renderScale":self.render_scale.mode(),"msaa":self.msaa})
    }
    fn save(&mut self) {
        let Some(scene) = self.gpu.as_ref().and_then(|g| g.scene.as_ref()) else {
            return;
        };
        let mut value = self.settings_value(scene);
        keep_saved(&mut value, &self.cli_start, &self.cli_kept, self.cli_source);
        let data = value.to_string();
        if data == self.persisted {
            return;
        }
        let result = (|| -> std::io::Result<()> {
            if let Some(p) = self.settings_path.parent() {
                fs::create_dir_all(p)?;
            }
            let tmp = self.settings_path.with_extension("tmp");
            fs::write(&tmp, &data)?;
            fs::rename(tmp, &self.settings_path)
        })();
        match result {
            Ok(()) => self.persisted = data,
            Err(e) => self.error = Some(format!("Settings: {e}")),
        };
    }
    fn sync_worker(&mut self) {
        self.poll_file();
        let Some(scene) = self.gpu.as_ref().and_then(|g| g.scene.as_ref()) else {
            return;
        };
        let next = (
            self.source.clone(),
            self.device,
            scene.settings.row_rate,
            scene.settings.sub,
        );
        let hold = scene.settings.hold;
        if next != self.worker_config {
            self.worker = None;
            let result = match self.source.as_str() {
                "test1" | "test2" => {
                    AnalysisWorker::test_track(usize::from(self.source == "test2"), next.2, next.3)
                }
                "file" => self
                    .decoded
                    .clone()
                    .ok_or_else(|| "No audio file".to_string())
                    .and_then(|pcm| AnalysisWorker::track(pcm, next.2, next.3)),
                "microphone" => {
                    if let Some(id) = &self.device_id {
                        AnalysisWorker::microphone_target_with_buffer(
                            Some(id),
                            self.device_name.as_deref(),
                            next.2,
                            next.3,
                            self.options.input_buffer,
                        )
                    } else {
                        AnalysisWorker::microphone_with_buffer(
                            next.1,
                            next.2,
                            next.3,
                            self.options.input_buffer,
                        )
                    }
                }
                _ => {
                    self.worker_config = next;
                    return;
                }
            };
            match result {
                Ok(w) => {
                    self.worker = Some(w);
                    self.error = None;
                }
                Err(e) => {
                    self.error = Some(e);
                    self.source = "off".into();
                }
            }
            self.worker_config = next;
        }
        if let Some(worker) = &self.worker {
            worker.set_frozen(hold);
            worker.set_listen(self.listen);
        }
    }
    fn draw_ui(&mut self) -> egui::FullOutput {
        let window = self.window.clone().unwrap();
        let raw = self.input.as_mut().unwrap().take_egui_input(&window);
        let ctx = self.ctx.clone();
        let strings = self.strings.clone();
        let lang = self.lang.clone();
        let t = |key: &str| text(&strings, &lang, key);
        // The renderer is lent out for the frame so the panel can change it and the app.
        let mut g = self.gpu.take().unwrap();
        let monitor_view = (self.monitor && !self.options.no_ui).then(|| {
            let audio = self.worker.as_ref().map(|w| w.stats());
            let hold = g.scene.as_ref().is_some_and(|s| s.settings.hold);
            self.mon.view(
                t,
                g.gpu_ms(),
                g.timing_available(),
                &g.adapter,
                g.draw_stats(),
                g.render_dimensions(),
                g.msaa_samples(),
                g.render_scale(),
                self.render_scale.mode(),
                audio.as_ref(),
                hold,
            )
        });
        let output = ctx.run(raw, |ctx| {
            self.docks(ctx, &mut g, monitor_view.as_ref());
            let stage = ctx.available_rect();
            let scale = ctx.pixels_per_point();
            // The picture takes what the docks leave, in whole pixels. It is set before the
            // labels are placed so both use this frame's layout: a resize resets the stage to
            // the whole window, and labels placed before the docks would land on them.
            let x = (stage.min.x * scale).round().max(0.) as u32;
            let y = (stage.min.y * scale).round().max(0.) as u32;
            let right = ((stage.max.x * scale).round() as u32).min(g.width);
            let bottom = ((stage.max.y * scale).round() as u32).min(g.height);
            g.set_stage(x, y, right.saturating_sub(x), bottom.saturating_sub(y));
            let (labels, peaks) = {
                let [sx, sy, sw, sh] = g.stage();
                let scene = g.scene.as_ref().unwrap();
                let offset = |x: f32, y: f32| (x + sx as f32, y + sy as f32);
                let mut labels = scene.project_labels(sw as f32, sh as f32);
                for l in &mut labels {
                    (l.x, l.y) = offset(l.x, l.y);
                }
                let mut peaks = scene.peak_labels();
                for p in &mut peaks {
                    (p.x, p.y) = offset(p.x, p.y);
                }
                (labels, peaks)
            };
            if self.source == "microphone" {
                if let Some(key) = self.worker.as_ref().and_then(|w| w.input_notice()) {
                    egui::Area::new(egui::Id::new("input-notice"))
                        .order(egui::Order::Foreground)
                        .fixed_pos(egui::pos2(stage.center().x, stage.top() + 18.))
                        .pivot(egui::Align2::CENTER_TOP)
                        .show(ctx, |ui| {
                            egui::Frame::popup(ui.style()).show(ui, |ui| {
                                ui.set_max_width((stage.width() - 32.).max(80.));
                                ui.label(t(key));
                            });
                        });
                }
            }
            let painter = ctx
                .layer_painter(egui::LayerId::new(
                    egui::Order::Background,
                    egui::Id::new("axis-labels"),
                ))
                .with_clip_rect(stage);
            for label in &labels {
                if label.alpha > 0.01 {
                    painter.text(
                        egui::pos2(label.x / scale, label.y / scale),
                        egui::Align2::CENTER_CENTER,
                        &label.text,
                        egui::FontId::proportional(11.),
                        egui::Color32::from_white_alpha((label.alpha * 180.) as u8),
                    );
                }
            }
            for peak in peaks.iter().rev() {
                paint_peak(&painter, peak, scale);
            }
        });
        self.gpu = Some(g);
        output
    }
    fn redraw(&mut self, event: &ActiveEventLoop) {
        let size = self.window.as_ref().unwrap().inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.poll_devices();
        self.sync_worker();
        let interval = self.last_frame.elapsed().as_secs_f64();
        let dt = interval.min(0.25);
        self.last_frame = Instant::now();
        let g = self.gpu.as_mut().unwrap();
        // Keep GPU timestamp queries active while either the monitor or auto scale needs them.
        let want_timing = self.monitor || self.render_scale.is_auto();
        if g.async_timing() != want_timing {
            g.set_async_timing(want_timing);
        }
        let toggled = self.monitor != self.mon_active;
        if toggled {
            self.mon_active = self.monitor;
            if self.monitor {
                self.mon.reset();
            }
        }
        let active = g.scene.as_ref().is_none_or(|s| !s.settings.hold);
        if self.render_scale.update(interval, g.gpu_ms(), active) {
            g.set_render_scale(self.render_scale.scale());
        }
        if self.monitor && (toggled || self.frames % 240 == 0) {
            self.mon.set_refresh(
                self.window
                    .as_ref()
                    .and_then(|w| w.current_monitor())
                    .and_then(|m| m.refresh_rate_millihertz())
                    .map(|mhz| mhz as f32 / 1000.),
            );
        }
        if let Some(scene) = g.scene.as_mut() {
            scene.tracks = self.worker.as_ref().map(|w| w.tracks()).unwrap_or_default();
        }
        g.advance(dt);
        let upload_from = Instant::now();
        let mut captured = None;
        if let Some(worker) = self.worker.as_mut() {
            let rows = worker.drain();
            if let Some(last) = rows.last() {
                captured = last.captured_at.map(|at| (worker.session_id(), at));
                let bytes: Vec<_> = rows.iter().map(|r| r.bytes.clone()).collect();
                g.submit_rows(
                    &bytes,
                    &last.shown,
                    &last.peaks,
                    &last.maxima,
                    worker.phase(),
                );
            }
            g.set_phase(worker.phase());
            if let Some(error) = worker.error() {
                self.error = Some(if error == "inputOpenTimeout" {
                    text(&self.strings, &self.lang, &error)
                } else {
                    error
                });
                if self.source != "microphone" {
                    self.source = "off".into();
                    self.worker = None;
                }
            }
        }
        let upload_ms = upload_from.elapsed().as_secs_f64() * 1000.;
        let output = self.draw_ui();
        let window = self.window.as_ref().unwrap();
        self.input
            .as_mut()
            .unwrap()
            .handle_platform_output(window, output.platform_output);
        let g = self.gpu.as_mut().unwrap();
        let frame = match self.surface.as_ref().unwrap().get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface
                    .as_ref()
                    .unwrap()
                    .configure(&g.device, self.config.as_ref().unwrap());
                window.request_redraw();
                return;
            }
            Err(wgpu::SurfaceError::Timeout) => {
                window.request_redraw();
                return;
            }
            Err(e) => {
                self.error = Some(e.to_string());
                event.exit();
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let draw_from = Instant::now();
        g.render(&view);
        // render() has just submitted the spectrum pass, before UI work and presentation.
        let submitted = g.submitted_at().unwrap();
        if let (Some((session, at)), Some(worker)) = (captured, self.worker.as_ref()) {
            if session == worker.session_id() {
                worker.record_submission(at, submitted);
            }
        }
        let painter = self.painter.as_mut().unwrap();
        for (id, delta) in &output.textures_delta.set {
            painter.update_texture(&g.device, &g.queue, *id, delta);
        }
        let jobs = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [g.width, g.height],
            pixels_per_point: output.pixels_per_point,
        };
        let mut encoder = g.device.create_command_encoder(&Default::default());
        let callbacks = painter.update_buffers(&g.device, &g.queue, &mut encoder, &jobs, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("native interface"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            painter.render(&mut pass, &jobs, &screen);
        }
        g.queue.submit(
            callbacks
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        for id in &output.textures_delta.free {
            painter.free_texture(id);
        }
        let draw_ms = draw_from.elapsed().as_secs_f64() * 1000.;
        if self.options.output.is_some()
            && self
                .options
                .frames
                .is_some_and(|limit| self.frames + 1 >= limit)
            && self
                .config
                .as_ref()
                .unwrap()
                .usage
                .contains(wgpu::TextureUsages::COPY_SRC)
        {
            let output = self.options.output.as_ref().unwrap();
            if let Some(parent) = output.parent() {
                let _ = fs::create_dir_all(parent);
            }
            g.save(&frame.texture, output);
        }
        frame.present();
        if self.monitor {
            self.mon.frame(interval, upload_ms, draw_ms);
        }
        self.frames += 1;
        // The task bar button does not take an icon set while the window is being created, so
        // set it again once the window has been up for a moment (see `set_windows_icon`).
        #[cfg(windows)]
        if self.frames == 60 {
            if let Some(window) = &self.window {
                set_windows_icon(window);
            }
        }
        if self.frames % 30 == 0 {
            self.save();
        }
        if self
            .options
            .frames
            .is_some_and(|limit| self.frames >= limit)
        {
            self.save();
            let g = self.gpu.as_ref().unwrap();
            let (vertices, draws) = g.draw_stats();
            // Frame and GPU figures are only collected while the monitor is shown.
            let monitor = self.monitor.then(|| {
                let mut m = self.mon.summary();
                m["gpuMs"] = serde_json::json!(g.gpu_ms());
                m
            });
            let audio = self.worker.as_ref().map(|w| w.stats()).map(|a| {
                serde_json::json!({
                    "captureToSubmitMs": a.capture_to_submit_ms,
                    "captureToSubmitMeanMs": a.capture_to_submit_mean_ms,
                    "captureToSubmitMaxMs": a.capture_to_submit_max_ms,
                    "captureToSubmitCount": a.capture_to_submit_count,
                    "callbackCount": a.callback_count, "callbackFrames": a.callback_frames,
                    "callbackMinFrames": a.callback_min_frames, "callbackMaxFrames": a.callback_max_frames,
                    "callbackMs": a.callback_frames.filter(|_| a.source_sample_rate > 0).map(|n| n as f64 * 1000.0 / a.source_sample_rate as f64),
                    "inputBuffer": self.worker.as_ref().and_then(|w| w.input_buffer()).map(|b| b.as_str()),
                    "sourceSampleRate": a.source_sample_rate, "channels": a.channels,
                    "rowsPerSecond": a.rows_per_second, "fftPerSecond": a.fft_rows_per_second,
                    "analysisMs": a.analysis_ms, "analysisStride": a.analysis_stride,
                    "inputFramesDropped": a.input_frames_dropped,
                    "displayRowsDropped": a.display_rows_dropped, "samples": a.samples,
                    "playbackFrames": a.playback_frames, "outputFrames": a.output_frames,
                    "outputRate": a.output_rate, "outputPeak": a.output_peak, "outputUnderruns": a.output_underruns,
                    "outputOpenMs": a.output_open_ms,
                    "outputDevice": a.output_device, "outputReopens": a.output_reopens,
                    "clockDifferenceMs": (a.playback_frames > 0).then(|| a.playback_frames as f64 / a.output_rate.max(1) as f64 * 1000. - a.samples as f64 / 48.),
                    "outputError": self.worker.as_ref().and_then(|w| w.output_error()),
                })
            });
            println!(
                "{}",
                serde_json::json!({"frames":self.frames,"adapter":g.adapter,"window":true,"source":self.source,"error":self.error,"elapsedSeconds":self.started.elapsed().as_secs_f64(),"vertices":vertices,"draws":draws,"windowPixels":[g.width,g.height],"stagePixels":g.stage(),"renderPixels":g.render_dimensions(),"renderScale":g.render_scale(),"renderScaleMode":self.render_scale.mode(),"msaa":g.msaa_samples(),"frameLatency":self.config.as_ref().map(|c| c.desired_maximum_frame_latency),"inputBuffer":self.worker.as_ref().and_then(|w| w.input_buffer()).unwrap_or(self.options.input_buffer).as_str(),"inputBufferRequested":self.options.input_buffer.as_str(),"monitor":monitor,"audio":audio})
            );
            event.exit();
        } else {
            self.window.as_ref().unwrap().request_redraw();
        }
    }
}
/// Windows: the exe carries the icon as resource 1 (build.rs). Without this the title bar and the
/// task bar show the generic window icon even though the file has an icon. Each call loads a fresh
/// icon handle: the task bar ignored the same handle sent again.
#[cfg(windows)]
fn set_windows_icon(window: &Window) {
    use winit::platform::windows::{IconExtWindows, WindowExtWindows};
    if let Ok(icon) = winit::window::Icon::from_resource(1, None) {
        window.set_window_icon(Some(icon));
    }
    if let Ok(icon) = winit::window::Icon::from_resource(1, None) {
        window.set_taskbar_icon(Some(icon));
    }
}

/// Linux: an ELF file cannot carry an icon. X11 takes the window icon from the program
/// (`_NET_WM_ICON`); Wayland ignores it and finds the icon through the `.desktop` file whose
/// name matches the app id. The raw 128x128 RGBA is `spectrum.png` resized (assets/icons).
#[cfg(all(unix, not(target_os = "macos")))]
fn linux_window_attributes(attributes: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    use winit::platform::{wayland::WindowAttributesExtWayland, x11::WindowAttributesExtX11};
    const APP_ID: &str = "spectrum-3d";
    let attributes = WindowAttributesExtX11::with_name(attributes, APP_ID, APP_ID);
    let attributes = WindowAttributesExtWayland::with_name(attributes, APP_ID, APP_ID);
    match winit::window::Icon::from_rgba(include_bytes!("../assets/icons/spectrum-128.rgba").to_vec(), 128, 128) {
        Ok(icon) => attributes.with_window_icon(Some(icon)),
        Err(_) => attributes,
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attributes = Window::default_attributes()
            .with_title("Spectrum 3D")
            .with_min_inner_size(winit::dpi::LogicalSize::new(MIN_SIZE[0], MIN_SIZE[1]));
        let screen = event.primary_monitor().map(|m| {
            let scale = m.scale_factor();
            let size = m.size().to_logical::<f64>(scale);
            let origin = m.position().to_logical::<f64>(scale);
            ([size.width, size.height], [origin.x, origin.y])
        });
        let size = if self.options.size_given {
            [self.options.width as f64, self.options.height as f64]
        } else {
            window_size(screen.map(|(size, _)| size))
        };
        attributes = attributes.with_inner_size(winit::dpi::LogicalSize::new(size[0], size[1]));
        if let (false, Some((screen, origin))) = (self.options.size_given, screen) {
            // Centred; the title bar and the menu bar or task bar take a little of the height.
            attributes = attributes.with_position(winit::dpi::LogicalPosition::new(
                origin[0] + ((screen[0] - size[0]) / 2.).max(0.),
                origin[1] + ((screen[1] - size[1]) / 2.).max(0.),
            ));
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            attributes = linux_window_attributes(attributes);
        }
        let window = Arc::new(event.create_window(attributes).unwrap());
        #[cfg(windows)]
        set_windows_icon(&window);
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window.clone()).unwrap();
        let data = match assets(&self.options.root, false) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("{e}");
                event.exit();
                return;
            }
        };
        self.strings = Arc::new(serde_json::from_slice(&data["strings.json"]).unwrap());
        let size = window.inner_size();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .unwrap();
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(capabilities.formats[0]);
        let mut gpu = pollster::block_on(Gpu::new_live(
            &instance,
            Some(&surface),
            format,
            &data,
            size.width,
            size.height,
        ));
        gpu.set_external_input(true);
        if let Ok(saved) = fs::read(&self.settings_path)
            .map(|b| serde_json::from_slice::<serde_json::Value>(&b).unwrap_or_default())
        {
            let scene = gpu.scene.as_mut().unwrap();
            scene.apply_settings(&saved["settings"]);
            // Version 1 always stored the bands per semitone, chosen or not, and its default
            // was 1. Treat it as not chosen once, so the finer default reaches old files.
            if saved["version"].as_u64().unwrap_or(1) < 2 {
                scene.settings.sub = DEFAULT_SUB;
            }
            if let Some(lang) = saved["lang"].as_str().filter(|l| ["zh", "en"].contains(l)) {
                self.lang = lang.into();
            }
            if let Some(source) = saved["source"]
                .as_str()
                .filter(|s| ["test", "test1", "test2", "off", "microphone"].contains(s))
            {
                self.source = source.into();
            }
            self.device_name = saved["deviceName"].as_str().map(str::to_owned);
            self.device = saved["device"].as_u64().map(|d| d as usize);
            if let Some(id) = saved["deviceId"].as_str() {
                self.device_id = Some(id.to_owned());
                self.device = None;
            }
            if let Some(hidden) = saved["hideUi"].as_bool() {
                self.hide_ui = hidden;
                self.options.no_ui |= hidden;
            }
            if let Some(shown) = saved["monitor"].as_bool() {
                self.monitor = shown;
            }
            for (field, key) in [
                (&mut self.panel_collapsed, "panelCollapsed"),
                (&mut self.hud_collapsed, "hudCollapsed"),
                (&mut self.advanced_open, "advancedOpen"),
            ] {
                if let Some(value) = saved[key].as_bool() {
                    *field = value;
                }
            }
            for (on, part) in self.monitor_parts.iter_mut().zip(monitor::Part::ALL) {
                if let Some(value) = saved["monitorParts"][part.key()].as_bool() {
                    *on = value;
                }
            }
            if let Some(mode) = saved["renderScale"]
                .as_str()
                .filter(|m| spectrum_core::RenderScale::is_valid_mode(m))
            {
                self.render_scale.set_mode(mode);
            }
            if let Some(samples) = saved["msaa"].as_u64() {
                self.msaa = spectrum_wgpu::gpu::normalize_msaa(samples as u32);
            }
            // This applies the saved view to the camera. The renderer notices the saved
            // resolution on its next frame by comparing geometry keys.
            scene.update(0.0);
            if let Some(values) = saved["camera"].as_array().filter(|v| v.len() == 7) {
                let mut target = [0.; 7];
                if values.iter().enumerate().all(|(i, v)| {
                    if let Some(n) = v.as_f64().filter(|n| n.is_finite()) {
                        target[i] = n;
                        true
                    } else {
                        false
                    }
                }) {
                    target[1] = target[1].clamp(-3., 89.9);
                    target[0] = target[0].clamp(-36000., 36000.);
                    target[2] = target[2].clamp(1f64.ln(), 90f64.ln());
                    target[3] = target[3].clamp(0.25f64.ln(), 3f64.ln());
                    for value in &mut target[4..] {
                        *value = value.clamp(-1000., 1000.);
                    }
                    scene.camera.target = target;
                    scene.camera.settle();
                }
            }
        }
        if self.source == "test" {
            self.source = "test1".into();
        }
        let before_options = self.settings_value(gpu.scene.as_ref().unwrap());
        apply_options(&mut gpu, &self.options);
        if let Some(mode) = &self.options.scale {
            self.render_scale.set_mode(mode);
        }
        if let Some(msaa) = self.options.msaa {
            self.msaa = msaa;
        }
        gpu.set_msaa(self.msaa);
        gpu.set_render_scale(self.render_scale.scale());
        gpu.scene.as_mut().unwrap().settings.dpr = window.scale_factor() as f32;
        if let Some(source) = &self.options.source {
            self.source = source.clone();
        }
        if self.source == "test" {
            self.source = "test1".into();
        }
        if let Some(path) = self.options.file.clone() {
            self.choose_file(Some(path));
        }
        self.listen = self.options.listen;
        // Start on the saved axis; the glide is for switching.
        gpu.scene.as_mut().unwrap().settle_axis();
        if self.options.device.is_some() {
            self.device = self.options.device;
            self.device_id = None;
            self.device_name = None;
        }
        self.cli_kept = kept_settings(&self.options, &before_options);
        self.cli_source = self.cli_kept.iter().any(|k| k.release == Release::Source);
        self.cli_start = self.settings_value(gpu.scene.as_ref().unwrap());
        let capture = self.options.output.is_some()
            && capabilities.usages.contains(wgpu::TextureUsages::COPY_SRC);
        if self.options.output.is_some() && !capture {
            self.error = Some("This surface does not support screenshot readback".into());
        }
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | if capture {
                    wgpu::TextureUsages::COPY_SRC
                } else {
                    wgpu::TextureUsages::empty()
                },
            format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: self.options.frame_latency,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
        };
        surface.configure(&gpu.device, &config);
        self.input = Some(egui_winit::State::new(
            self.ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        ));
        self.painter = Some(egui_wgpu::Renderer::new(
            &gpu.device,
            format,
            egui_wgpu::RendererOptions::default(),
        ));
        self.gpu = Some(gpu);
        self.surface = Some(surface);
        self.config = Some(config);
        self.window = Some(window);
        self.last_frame = Instant::now();
        self.window.as_ref().unwrap().request_redraw();
    }
    fn window_event(&mut self, event: &ActiveEventLoop, _: WindowId, message: WindowEvent) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let response = self
            .input
            .as_mut()
            .unwrap()
            .on_window_event(window, &message);
        match message {
            WindowEvent::CloseRequested => {
                self.save();
                self.worker = None;
                event.exit();
            }
            WindowEvent::RedrawRequested => self.redraw(event),
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    let g = self.gpu.as_mut().unwrap();
                    g.resize(size.width, size.height);
                    let c = self.config.as_mut().unwrap();
                    c.width = size.width;
                    c.height = size.height;
                    self.surface.as_ref().unwrap().configure(&g.device, c);
                    window.request_redraw();
                }
            }
            WindowEvent::DroppedFile(path) => {
                self.cli_source = false;
                self.choose_file(Some(path));
            }
            WindowEvent::ModifiersChanged(m) => self.shift = m.state().shift_key(),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.gpu
                    .as_mut()
                    .unwrap()
                    .scene
                    .as_mut()
                    .unwrap()
                    .settings
                    .dpr = scale_factor as f32;
                window.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Released {
                    self.drag = None;
                } else if !response.consumed {
                    self.drag = Some(button);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let dx = position.x - self.cursor.0;
                let dy = position.y - self.cursor.1;
                self.cursor = (position.x, position.y);
                if let Some(button) = self.drag {
                    let scene = self.gpu.as_mut().unwrap().scene.as_mut().unwrap();
                    if button == MouseButton::Right || self.shift {
                        scene
                            .camera
                            .pan_by(dx, dy, window.inner_size().height as f64);
                    } else if scene.settings.waterfall {
                        let scale = window.scale_factor();
                        scene.camera.orbit(dx / scale, dy / scale);
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } if !response.consumed => {
                let delta = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64 * 0.12,
                    MouseScrollDelta::PixelDelta(p) => p.y * 0.0015,
                };
                self.gpu
                    .as_mut()
                    .unwrap()
                    .scene
                    .as_mut()
                    .unwrap()
                    .camera
                    .zoom_by((-delta).exp());
            }
            WindowEvent::KeyboardInput { event: key, .. }
                if key.state == ElementState::Pressed && !key.repeat && !response.consumed =>
            {
                let scene = self.gpu.as_mut().unwrap().scene.as_mut().unwrap();
                match key.physical_key {
                    PhysicalKey::Code(KeyCode::Space) => scene.settings.hold = !scene.settings.hold,
                    PhysicalKey::Code(KeyCode::Digit1) => scene.settings.view = "front".into(),
                    PhysicalKey::Code(KeyCode::Digit2) => scene.settings.view = "top".into(),
                    PhysicalKey::Code(KeyCode::Digit3) => {
                        scene.settings.view = "perspective".into()
                    }
                    PhysicalKey::Code(KeyCode::KeyH) => {
                        self.options.no_ui = !self.options.no_ui;
                        self.hide_ui = self.options.no_ui;
                    }
                    PhysicalKey::Code(KeyCode::KeyM) => self.monitor = !self.monitor,
                    PhysicalKey::Code(KeyCode::KeyC) => {
                        if let Some(w) = &self.worker {
                            w.clear_maximum();
                        }
                        self.gpu.as_mut().unwrap().clear_maximum();
                    }
                    PhysicalKey::Code(KeyCode::KeyR) => {
                        scene.camera.set_view(&scene.settings.view);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.save();
        self.worker = None;
    }
}
pub(crate) fn run(options: Options) -> Result<(), String> {
    let ctx = egui::Context::default();
    ctx.set_visuals(egui::Visuals::dark());
    crate::fonts::install(&ctx);
    panel::apply_visuals(&ctx);
    let path = options.settings.clone().unwrap_or_else(settings_path);
    let mut app = App {
        options,
        window: None,
        surface: None,
        gpu: None,
        config: None,
        ctx,
        input: None,
        painter: None,
        worker: None,
        devices: Vec::new(),
        device_enumeration: None,
        devices_requested: false,
        last_devices: Instant::now(),
        source: "test".into(),
        device: None,
        device_id: None,
        device_name: None,
        worker_config: ("".into(), None, 0, 0),
        strings: Default::default(),
        lang: "zh".into(),
        error: None,
        decoded: None,
        loading: None,
        file_name: None,
        listen: false,
        last_frame: Instant::now(),
        frames: 0,
        monitor: false,
        mon_active: false,
        render_scale: spectrum_core::RenderScale::new("auto"),
        msaa: 4,
        hide_ui: false,
        panel_collapsed: false,
        hud_collapsed: false,
        monitor_parts: [true; 5],
        advanced_open: false,
        aux_height: 0.,
        mon: monitor::Monitor::new(),
        started: std::time::Instant::now(),
        cursor: (0., 0.),
        drag: None,
        shift: false,
        settings_path: path,
        persisted: String::new(),
        cli_kept: Vec::new(),
        cli_start: serde_json::Value::Null,
        cli_source: false,
    };
    EventLoop::new()
        .map_err(|e| e.to_string())?
        .run_app(&mut app)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_settings_are_for_this_run_only() {
        let o = Options::parse(
            [
                "--window",
                "--palette",
                "green",
                "--scale",
                "0.5",
                "--msaa",
                "0",
                "--view",
                "front",
                "--file",
                "song.wav",
                "--keys",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();
        let saved = serde_json::json!({"version":2,"settings":{"view":"perspective","palette":"tungsten","style":"lattice","keys":false,"sub":4,"styleOptions":{}},"camera":[1,2,3,4,5,6,7],"source":"microphone","device":null,"deviceId":"usb-1","deviceName":"Connex","renderScale":"auto","msaa":4,"lang":"zh"});
        let kept = kept_settings(&o, &saved);
        // As the run began: the command line's values, the file decoding, the camera moving.
        let mut start = saved.clone();
        for (key, v) in [
            ("palette", "green"),
            ("style", "contours"),
            ("view", "front"),
        ] {
            start["settings"][key] = v.into();
        }
        start["settings"]["keys"] = true.into();
        start["settings"]["styleOptions"] = serde_json::json!({"tungsten":{"style":"lattice"}});
        start["renderScale"] = "0.5".into();
        start["msaa"] = 0.into();
        let mut now = start.clone();
        now["source"] = "file".into();
        now["deviceId"] = serde_json::Value::Null;
        now["camera"] = serde_json::json!([9, 9, 9, 9, 9, 9, 9]);
        now["lang"] = "en".into();
        keep_saved(&mut now, &start, &kept, true);
        // Nothing the command line set reaches the file; what the user changed does.
        let mut expected = saved.clone();
        expected["lang"] = "en".into();
        assert_eq!(now, expected);

        // The user changes the plot style and picks a source: those are saved, the
        // rest still comes from the file.
        let mut now = start.clone();
        now["settings"]["style"] = "pure".into();
        now["settings"]["view"] = "top".into();
        now["camera"] = serde_json::json!([8, 8, 8, 8, 8, 8, 8]);
        now["source"] = "test2".into();
        keep_saved(&mut now, &start, &kept, false);
        assert_eq!(now["settings"]["style"], "pure");
        assert_eq!(now["settings"]["view"], "top");
        assert_eq!(now["camera"], serde_json::json!([8, 8, 8, 8, 8, 8, 8]));
        assert_eq!(now["source"], "test2");
        assert_eq!(now["settings"]["palette"], "tungsten");
        assert_eq!(now["renderScale"], "auto");
        assert_eq!(now["msaa"], 4);
        assert_eq!(now["settings"]["keys"], false);

        // Without options nothing is held back.
        let none = Options::parse(["--window"].into_iter().map(str::to_owned)).unwrap();
        assert!(kept_settings(&none, &saved).is_empty());
    }
    #[test]
    fn first_window_follows_the_screen() {
        // A 13-inch MacBook Air at its default scaling.
        assert_eq!(window_size(Some([1470., 956.])), [1323., 813.]);
        // A 1366 × 768 laptop and a large display stop at their bounds.
        assert_eq!(window_size(Some([1366., 768.])), [1229., 653.]);
        assert_eq!(window_size(Some([2560., 1440.])), [1680., 1050.]);
        // A screen smaller than the minimum window is filled rather than overflowed.
        assert_eq!(window_size(Some([600., 400.])), [600., 400.]);
        assert_eq!(window_size(None), [1280., 800.]);
        assert_eq!(window_size(Some([0., 0.])), [1280., 800.]);
    }
}
