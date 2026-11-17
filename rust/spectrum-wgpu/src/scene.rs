//! Native scene construction. The only visual specification is exported look.json;
//! uniform offsets are read by field name from the shader migration manifest.
#[path = "camera.rs"]
pub mod camera;
use camera::{transform, Camera, Matrices};
use serde_json::{json, Value};
use spectrum_core::{
    band_count, band_to_u, hz_to_band, PeakTrack, HISTORY_ROWS, LEVELS, PEAK_LABELS,
};
use std::collections::HashMap;
pub type Assets = HashMap<String, Vec<u8>>;
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub view: String,
    pub palette: String,
    pub style: String,
    pub seconds: f64,
    pub row_rate: u32,
    pub sub: usize,
    pub waterfall: bool,
    pub spectrum: bool,
    pub low_level: bool,
    pub max_hold: bool,
    pub keys: bool,
    pub peaks: bool,
    pub hold: bool,
    /// Frequency axis: "log" (every semitone the same width) or "mel".
    pub axis: String,
    /// Draw the front spectrum 4.5 dB/oct steeper about 1 kHz, in the front view only.
    pub tilt: bool,
    pub dpr: f32,
    /// The plot style last used with each colour style, as the web's `styleOptions`: switching
    /// from Tungsten on Mesh to Dadaspec and back gives Mesh again.
    pub style_options: std::collections::BTreeMap<String, String>,
}
/// Bands per semitone until the user picks, as on a desktop-sized web window (settings.js).
pub const DEFAULT_SUB: usize = 4;
/// The front view's optional tilt in dB per octave (web/src/main.js), about the pivot
/// front.vert uses: 1 kHz in semitones from C0.
pub const TILT_DB_PER_OCTAVE: f32 = 4.5;
pub const TILT_PIVOT_BAND: f64 = 71.2131;
impl Settings {
    pub fn from_look(look: &Value) -> Self {
        Self {
            view: "perspective".into(),
            palette: look["palette"].as_str().unwrap().into(),
            style: look["style"].as_str().unwrap().into(),
            seconds: 4.0,
            row_rate: 60,
            sub: DEFAULT_SUB,
            waterfall: true,
            spectrum: true,
            low_level: false,
            max_hold: false,
            keys: false,
            peaks: false,
            hold: false,
            axis: "log".into(),
            tilt: false,
            dpr: 1.0,
            style_options: Default::default(),
        }
    }
    pub fn to_json(&self) -> Value {
        json!({"view":self.view,"palette":self.palette,"style":self.style,"seconds":self.seconds,"row_rate":self.row_rate,"sub":self.sub,"waterfall":self.waterfall,"spectrum":self.spectrum,"low_level":self.low_level,"max_hold":self.max_hold,"keys":self.keys,"peaks":self.peaks,"hold":self.hold,"axis":self.axis,"tilt":self.tilt,"styleOptions":self.style_options.iter().map(|(k, v)| (k.clone(), json!({"style": v}))).collect::<serde_json::Map<_, _>>()})
    }
    pub fn from_json(look: &Value, v: &Value) -> Self {
        let mut s = Self::from_look(look);
        for (key, dest) in [
            ("view", &mut s.view),
            ("palette", &mut s.palette),
            ("style", &mut s.style),
            ("axis", &mut s.axis),
        ] {
            if let Some(v) = v[key].as_str() {
                *dest = v.into();
            }
        }
        if !["log", "mel"].contains(&s.axis.as_str()) {
            s.axis = "log".into();
        }
        if !look["views"][&s.view].is_object() {
            s.view = "perspective".into();
        }
        if !look["palettes"][&s.palette].is_array() {
            s.palette = look["palette"].as_str().unwrap().into();
        }
        if !look["styles"][&s.style].is_object() {
            s.style = look["style"].as_str().unwrap().into();
        }
        if let Some(n) = v["seconds"].as_f64() {
            if n.is_finite() {
                s.seconds = n.clamp(1.0, 32.0);
            }
        }
        if let Some(n) = v["row_rate"].as_u64() {
            if [60, 120, 240, 480].contains(&n) {
                s.row_rate = n as u32;
            }
        }
        if let Some(n) = v["sub"].as_u64() {
            if [1, 2, 4, 8].contains(&n) {
                s.sub = n as usize;
            }
        }
        for (key, dest) in [
            ("waterfall", &mut s.waterfall),
            ("spectrum", &mut s.spectrum),
            ("low_level", &mut s.low_level),
            ("max_hold", &mut s.max_hold),
            ("keys", &mut s.keys),
            ("peaks", &mut s.peaks),
            ("hold", &mut s.hold),
            ("tilt", &mut s.tilt),
        ] {
            if let Some(b) = v[key].as_bool() {
                *dest = b;
            }
        }
        if !s.spectrum && !s.waterfall {
            s.waterfall = true;
        }
        if let Some(options) = v["styleOptions"].as_object() {
            for (palette, item) in options {
                if let Some(style) = item["style"].as_str() {
                    s.style_options.insert(palette.clone(), style.into());
                }
            }
        }
        s
    }
}
#[derive(Clone, Debug)]
pub struct Label {
    pub text: String,
    pub kind: &'static str,
    pub x: f32,
    pub y: f32,
    pub alpha: f32,
}
/// One main-peak label, anchored where its peak sits on the drawn curve.
#[derive(Clone, Debug)]
pub struct PeakLabel {
    /// Note name, signed cents, then frequency and level.
    pub text: [String; 3],
    pub x: f32,
    pub y: f32,
    pub alpha: f32,
    /// The loudest one.
    pub main: bool,
    pub band: f64,
}
#[derive(Clone, Debug)]
pub struct Guide {
    pub name: &'static str,
    pub first: u32,
    pub count: u32,
}
pub struct Scene {
    pub settings: Settings,
    pub camera: Camera,
    pub look: Value,
    layout: Value,
    base_look: Value,
    base_layout: Value,
    dadaspec_look: Option<Value>,
    dadaspec_layout: Option<Value>,
    pub live_chroma: [f32; 4],
    pub mesh: Vec<f32>,
    pub indices: Vec<u32>,
    pub guides: Vec<f32>,
    pub slices: Vec<Guide>,
    pub keys: Vec<f32>,
    /// Where the frequency axis is between semitone (0) and mel (1); glides toward
    /// `settings.axis`.
    pub mel: f32,
    /// The guides were rebuilt on their own (the axis is gliding); the mesh was not.
    pub guides_dirty: bool,
    /// Tracked main peaks of the live spectrum, set by whoever runs the analysis.
    pub tracks: Vec<PeakTrack>,
    pub cols: usize,
    pub matrices: Matrices,
    pub width: u32,
    pub height: u32,
    last_view: String,
    geometry_key: (usize, u32, u64, bool),
    flat: bool,
    view_before_flat: String,
    camera_before_flat: Option<(String, [f64; 7])>,
}
fn n(v: &Value) -> f32 {
    v.as_f64().expect("numeric look value") as f32
}
fn rgb(v: &Value) -> [f32; 3] {
    let s = v.as_str().unwrap_or("#000000").trim_start_matches('#');
    if s.len() != 6 {
        return [0.0; 3];
    }
    std::array::from_fn(|i| {
        u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap_or(0) as f32 / 255.0
    })
}
fn floats(v: &Value) -> Vec<f32> {
    v.as_array()
        .expect("numeric look array")
        .iter()
        .map(n)
        .collect()
}
fn mix_rgb(a: &Value, b: &Value, white: &Value, chroma_sat: f32) -> [f32; 3] {
    let ca = rgb(a);
    let cb = rgb(b);
    let w = rgb(white);
    let m = [ca[0] + cb[0], ca[1] + cb[1], ca[2] + cb[2]];
    let pk = m[0].max(m[1]).max(m[2]).max(1e-4);
    std::array::from_fn(|i| {
        let norm = m[i] / pk;
        let ch = ((w[i] * (1.0 - chroma_sat * 0.75) + norm * chroma_sat * 0.75)
            .clamp(0.0, 1.0)
            * 255.0)
            .round();
        ch / 255.0
    })
}
fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
/// 1 while the camera looks at the front edge from its own side (or straight down), falling
/// to 0 once it has gone round behind, where the terrain stands between it and everything
/// drawn on that edge (web/src/labels.js `frontFacing`). `back`: from the centre to the camera.
pub fn front_facing(back: [f64; 3]) -> f32 {
    let t = ((-back[2] - 0.02) / 0.18).clamp(0.0, 1.0);
    (1.0 - t * t * (3.0 - 2.0 * t)) as f32
}
impl Scene {
    pub fn new(assets: &Assets, width: u32, height: u32) -> Self {
        let look: Value =
            serde_json::from_slice(&assets["look.json"]).expect("valid exported look.json");
        let layout: Value =
            serde_json::from_slice(&assets["uniforms.json"]).expect("valid migrated uniforms.json");
        let dadaspec_look = assets
            .get("look-dadaspec.json")
            .and_then(|b| serde_json::from_slice::<Value>(b).ok());
        let dadaspec_layout = assets
            .get("dadaspec/uniforms.json")
            .and_then(|b| serde_json::from_slice::<Value>(b).ok());
        let camera = Camera::new(&look);
        let matrices = camera.matrices(width as f64 / height.max(1) as f64);
        let settings = Settings::from_look(&look);
        let mut scene = Self {
            last_view: settings.view.clone(),
            settings,
            camera,
            base_look: look.clone(),
            base_layout: layout.clone(),
            dadaspec_look,
            dadaspec_layout,
            live_chroma: [0.0; 4],
            look,
            layout,
            mesh: vec![],
            indices: vec![],
            guides: vec![],
            slices: vec![],
            keys: vec![],
            mel: 0.0,
            guides_dirty: false,
            tracks: vec![],
            cols: 0,
            matrices,
            width: width.max(1),
            height: height.max(1),
            geometry_key: (0, 0, 0, false),
            flat: false,
            view_before_flat: "perspective".into(),
            camera_before_flat: None,
        };
        scene.build_geometry();
        scene
    }
    pub fn has_dadaspec(&self) -> bool {
        self.dadaspec_look.is_some()
    }
    pub fn is_dadaspec(&self) -> bool {
        self.settings.palette == "dadaspec" && self.dadaspec_look.is_some()
    }
    pub fn crossovers(&self) -> [f64; 2] {
        let c = &self.look["crossovers"];
        [
            c[0].as_f64().unwrap_or(200.0),
            c[1].as_f64().unwrap_or(1500.0),
        ]
    }
    pub fn set_palette(&mut self, name: &str) {
        let (from, from_style) = (self.settings.palette.clone(), self.settings.style.clone());
        self.switch_palette(name);
        if self.settings.palette != from {
            // As on the web: the style's remembered plot style, else that style's own default
            // (Green's variant, else the look's), never the one carried over from the last.
            self.settings.style_options.insert(from, from_style);
            let remembered = self.settings.style_options.get(name).cloned();
            let fallback = self.look["variants"][name]["style"]
                .as_str()
                .or(self.look["style"].as_str())
                .map(str::to_owned);
            if let Some(style) = [remembered, fallback]
                .into_iter()
                .flatten()
                .find(|s| self.look["styles"][s].is_object())
            {
                self.settings.style = style;
            }
        }
    }
    fn switch_palette(&mut self, name: &str) {
        if name == "dadaspec" {
            let Some(dl) = self.dadaspec_look.clone() else {
                return;
            };
            let was = self.is_dadaspec();
            self.look = dl;
            if let Some(dlay) = &self.dadaspec_layout {
                self.layout = dlay.clone();
            }
            self.camera.set_look(&self.look);
            self.settings.palette = "dadaspec".into();
            if !was {
                self.settings.style = self.look["style"].as_str().unwrap_or("sculpt").into();
                self.build_guides();
                self.guides_dirty = true;
            }
        } else if self.base_look["palettes"][name].is_array() {
            let was = self.is_dadaspec();
            self.settings.palette = name.into();
            if was {
                self.look = self.base_look.clone();
                self.layout = self.base_layout.clone();
                self.camera.set_look(&self.look);
                self.build_guides();
                self.guides_dirty = true;
            }
            if let Some(style) = self.look["variants"][name]["style"].as_str() {
                self.settings.style = style.into();
            }
        }
    }
    pub fn apply_settings(&mut self, v: &Value) {
        let want_dadaspec =
            v["palette"].as_str() == Some("dadaspec") && self.dadaspec_look.is_some();
        let (look, layout) = if want_dadaspec {
            (
                self.dadaspec_look.clone().unwrap(),
                self.dadaspec_layout
                    .clone()
                    .unwrap_or_else(|| self.base_layout.clone()),
            )
        } else {
            (self.base_look.clone(), self.base_layout.clone())
        };
        let was_dadaspec = self.is_dadaspec();
        self.settings = Settings::from_json(&look, v);
        self.look = look;
        self.layout = layout;
        self.camera.set_look(&self.look);
        if was_dadaspec != self.is_dadaspec() {
            self.build_guides();
            self.guides_dirty = true;
        }
    }
    /// The parameters the current mesh was built from (`sub`, `row_rate`, `seconds`,
    /// `low_level`). The renderer compares it with the key of its own buffers.
    pub fn geometry_key(&self) -> (usize, u32, u64, bool) {
        self.geometry_key
    }
    pub fn update(&mut self, dt: f64) -> bool {
        if !self.settings.spectrum && !self.settings.waterfall {
            self.settings.waterfall = true;
        }
        let flat = self.settings.spectrum && !self.settings.waterfall;
        if flat && !self.flat {
            self.view_before_flat = self.settings.view.clone();
            self.camera_before_flat = Some((self.camera.view.clone(), self.camera.target));
            self.camera.set_view("front");
        } else if !flat && self.flat {
            self.settings.view = self.view_before_flat.clone();
            if let Some((view, target)) = self.camera_before_flat.take() {
                self.camera.restore(&view, target);
            } else {
                self.camera.set_view(&self.settings.view);
            }
            self.last_view = self.settings.view.clone();
        }
        self.flat = flat;
        if self.settings.view != self.last_view {
            if !flat {
                self.camera.set_view(&self.settings.view);
            }
            self.last_view = self.settings.view.clone();
        }
        self.camera.update(dt);
        self.matrices = self.camera.matrices(self.width as f64 / self.height as f64);
        let key = (
            self.settings.sub,
            self.settings.row_rate,
            self.settings.seconds.to_bits(),
            self.settings.low_level,
        );
        // The frequency axis glides between semitone and mel (main.js: 0.08 s time constant,
        // about 0.3 s to settle); the guides follow it, the mesh does not depend on it.
        let target = if self.settings.axis == "mel" {
            1.0
        } else {
            0.0
        };
        let mut glided = false;
        if self.mel != target && dt > 0.0 {
            let m = self.mel + (target - self.mel) * (1.0 - (-dt / 0.08).exp()) as f32;
            self.mel = if (target - m).abs() < 1e-3 { target } else { m };
            glided = true;
        }
        if key != self.geometry_key {
            self.build_geometry();
            true
        } else {
            if glided {
                self.build_guides();
                self.guides_dirty = true;
            }
            false
        }
    }
    /// Jump to the chosen axis without the glide, for a first frame or a still image.
    pub fn settle_axis(&mut self) {
        let target = if self.settings.axis == "mel" {
            1.0
        } else {
            0.0
        };
        if self.mel != target {
            self.mel = target;
            self.build_guides();
            self.guides_dirty = true;
        }
    }
    /// The look's Hz ticks, or its mel list once the axis is more mel than not
    /// (bands.js `hzTicks`).
    fn hz_ticks(&self, compact: bool) -> Vec<f32> {
        let labels = &self.look["labels"];
        let mel = self.mel > 0.5 && labels["hzMel"].is_array();
        let full = &labels[if mel { "hzMel" } else { "hz" }];
        let short = &labels[if mel { "hzMelCompact" } else { "hzCompact" }];
        floats(if compact && short.is_array() {
            short
        } else {
            full
        })
    }
    fn band_x(&self, band: f64, width: f32) -> f32 {
        (band_to_u(band, self.mel as f64) as f32 - 0.5) * width
    }
    fn tilt(&self) -> f32 {
        if self.settings.tilt {
            TILT_DB_PER_OCTAVE * self.metrics().1
        } else {
            0.0
        }
    }
    pub fn db_floor(&self) -> f32 {
        if self.settings.low_level {
            -120.0
        } else {
            n(&self.look["db"]["floor"])
        }
    }
    pub fn front(&self, per_band: usize, db: &[f32]) -> Vec<f32> {
        let count = band_count(self.settings.sub);
        let mut data = Vec::with_capacity(count * per_band * 3);
        for k in 0..count {
            for j in 0..per_band {
                data.extend_from_slice(&[
                    k as f32 / self.settings.sub as f32,
                    *db.get(k).unwrap_or(&-120.0),
                    if per_band == 2 { (1 - j) as f32 } else { 1.0 },
                ]);
            }
        }
        data
    }
    pub fn draw_names(&self) -> Vec<String> {
        self.slices
            .iter()
            .map(|g| g.name.into())
            .chain(
                ["curtain", "maximum", "peak", "trace", "keys"]
                    .into_iter()
                    .map(String::from),
            )
            .collect()
    }
    pub fn draw_visible(&self, name: &str) -> bool {
        let (top, front, fade, time) = self.metrics();
        match name {
            "terrain" => self.settings.waterfall,
            "floorGrid" | "sideRails" => self.settings.waterfall,
            "xoverFloorLow" | "xoverFloorHigh" => self.is_dadaspec() && self.settings.waterfall,
            "xoverLow" | "xoverHigh" => self.is_dadaspec(),
            "timeTicks" => self.settings.waterfall && time > 0.002,
            "dbSpine" | "dbAccents" => fade > 0.002,
            "frontGrid" | "headroomLine" => front > 0.002,
            "xoverFrontLow" | "xoverFrontHigh" => self.is_dadaspec() && front > 0.002,
            "curtain" | "trace" => self.settings.spectrum,
            "maximum" => self.settings.spectrum && self.settings.max_hold && top < 0.98,
            "peak" => self.settings.spectrum && top < 0.98,
            "keys" => self.settings.keys,
            _ => true,
        }
    }
    fn metrics(&self) -> (f32, f32, f32, f32) {
        let b = self.matrices.back;
        let pitch = b[1].clamp(-1.0, 1.0).asin();
        let top = smooth(62.0, 86.0, pitch.to_degrees() as f32);
        let flat = 1.0 - smooth(3.0, 18.0, self.camera.current[2].exp() as f32);
        let front = smooth(0.95, 0.998, b[2] as f32) * (0.3 + 0.7 * flat);
        let fade = (pitch.cos() as f32 * 1.75 - 0.35).clamp(0.0, 1.0);
        let time = ((b[0].hypot(b[1]) as f32 - 0.12) * 3.2).clamp(0.0, 1.0);
        (top, front, fade, time)
    }
    fn write(&self, bytes: &mut [u8], name: &str, values: &[f32]) {
        let Some(field) = self.layout["fields"].get(name) else {
            return;
        };
        let offset = field["offset"].as_u64().expect("uniform offset") as usize;
        for (i, value) in values.iter().enumerate() {
            bytes[offset + i * 4..offset + i * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    pub fn uniforms(&self, name: &str, written: usize, phase: f64) -> Vec<u8> {
        self.uniforms_scaled(name, written, phase, [self.width, self.height], 1.0)
    }

    /// Writes shader uniforms for an offscreen render size while keeping UI-facing
    /// stage metrics (`heightPxSmall`, `project_labels`) anchored at window pixels.
    pub fn uniforms_scaled(
        &self,
        name: &str,
        written: usize,
        phase: f64,
        viewport: [u32; 2],
        render_scale: f32,
    ) -> Vec<u8> {
        let mut b = vec![0; self.layout["size"].as_u64().unwrap() as usize];
        let l = &self.look;
        let s = &self.settings;
        let size = &l["size"];
        let style = &l["styles"][&s.style];
        let (top, front, fade, time) = self.metrics();
        let scale = if render_scale.is_finite() && render_scale > 0.0 && render_scale < 0.999 {
            render_scale
        } else {
            1.0
        };
        let eff_dpr = s.dpr * scale;
        let mut set = |key: &str, v: &[f32]| self.write(&mut b, key, v);
        for (key, v) in [
            ("u_rows", HISTORY_ROWS as f32),
            ("u_sub", s.sub as f32),
            ("u_rowScale", s.row_rate as f32 / 60.0),
            (
                "u_newest",
                ((written + HISTORY_ROWS * 32 - 1) % (HISTORY_ROWS * 32)) as f32,
            ),
            ("u_phase", phase as f32),
            (
                "u_spanRows",
                s.seconds as f32 * s.row_rate as f32 * n(&size["tail"]),
            ),
            ("u_tail", n(&size["tail"])),
            ("u_fogStart", n(&l["fog"]["start"])),
            ("u_fogPower", n(&l["fog"]["power"])),
            ("u_stepU", 1.0 / (self.cols - 1) as f32),
            (
                "u_lift",
                if ["curtain", "maximum", "peak", "trace"].contains(&name) {
                    n(&size["height"]) * 0.0025
                } else {
                    0.0
                },
            ),
            ("u_ambient", n(&l["surface"]["ambient"])),
            ("u_exposure", n(&l["surface"]["exposure"])),
            (
                "u_registerTint",
                l.get("paletteRegister")
                    .and_then(|p| p.get(&s.palette))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0) as f32,
            ),
            ("u_topness", top),
            ("u_frontness", front),
            ("u_dpr", eff_dpr),
            ("u_keyGapPx", n(&l["keys"]["gapPx"]) * eff_dpr),
            (
                "u_keyHeightPx",
                n(&l["keys"][if self.height as f32 / s.dpr < 300.0 {
                    "heightPxSmall"
                } else {
                    "heightPx"
                }]) * eff_dpr,
            ),
            ("u_hoverBand", -10.0),
            ("u_melAxis", self.mel),
            ("u_tilt", self.tilt()),
        ] {
            set(key, &[v]);
        }
        set("u_dbRange", &[self.db_floor(), n(&l["db"]["ceiling"])]);
        set(
            "u_size",
            &[n(&size["width"]), n(&size["height"]), n(&size["depth"])],
        );
        set("u_viewProj", &self.matrices.view_proj);
        set("u_invViewProj", &self.matrices.inverse);
        set(
            "u_viewport",
            &[viewport[0].max(1) as f32, viewport[1].max(1) as f32],
        );
        set("u_background", &rgb(&l["background"]));
        set("u_keyLight", &floats(&l["surface"]["keyLight"]));
        set("u_rimLight", &floats(&l["surface"]["rimLight"]));
        for field in [
            "surfaceMix",
            "sliceStrength",
            "sliceEvery",
            "ribStrength",
            "ribEvery",
            "contourStrength",
            "octaveStrength",
            "lineFloor",
        ] {
            set(&format!("u_{field}"), &[n(&style[field])]);
        }
        for (field, default) in [
            ("channelEtch", 0.14f32),
            ("seamBoost", 0.55),
            ("crossoverStrength", 0.46),
        ] {
            let v = style
                .get(field)
                .and_then(Value::as_f64)
                .map_or(default, |x| x as f32);
            set(&format!("u_{field}"), &[v]);
        }
        if let Some(triad) = l.get("triad").filter(|v| v.is_object()) {
            set("u_lowColor", &rgb(&triad["low"]));
            set("u_midColor", &rgb(&triad["mid"]));
            set("u_highColor", &rgb(&triad["high"]));
            set("u_whiteColor", &rgb(&triad["white"]));
            set(
                "u_chromaSat",
                &[triad
                    .get("chromaSat")
                    .and_then(Value::as_f64)
                    .unwrap_or(1.0) as f32],
            );
            set(
                "u_dispersion",
                &[triad
                    .get("dispersion")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.34) as f32],
            );
            set("u_liveChroma", &self.live_chroma);
        }
        let f = &l["front"];
        let variant = &l["variants"][&s.palette]["front"];
        for (field, uniform) in [
            ("trace", "u_traceColor"),
            ("peak", "u_peakColor"),
            ("headroom", "u_headroomColor"),
            ("maxHold", "u_maxColor"),
        ] {
            set(uniform, &rgb(variant.get(field).unwrap_or(&f[field])));
        }
        for (field, uniform) in [
            ("white", "u_whiteKey"),
            ("black", "u_blackKey"),
            ("lit", "u_litKey"),
            ("hover", "u_hoverKey"),
            ("octave", "u_octaveEdge"),
        ] {
            set(uniform, &rgb(&l["keys"][field]));
        }
        let g = &l["guides"];
        let floor_alpha = (n(&g["floorAlpha3d"])
            + (n(&g["floorAlphaTop"]) - n(&g["floorAlpha3d"])) * top)
            * (1.0 - front);
        let triad_opt = l.get("triad").filter(|v| v.is_object());
        let triad_sat = triad_opt
            .and_then(|t| t.get("chromaSat"))
            .and_then(Value::as_f64)
            .unwrap_or(1.0) as f32;
        let xover_low_mid = || {
            triad_opt.map_or([0.0; 3], |t| {
                mix_rgb(&t["low"], &t["mid"], &t["white"], triad_sat)
            })
        };
        let xover_mid_high = || {
            triad_opt.map_or([0.0; 3], |t| {
                mix_rgb(&t["mid"], &t["high"], &t["white"], triad_sat)
            })
        };
        let (color_rgb, alpha) = match name {
            "floorGrid" => (rgb(&g["floorColor"]), floor_alpha),
            "xoverFloorLow" => (xover_low_mid(), floor_alpha * 1.65),
            "xoverFloorHigh" => (xover_mid_high(), floor_alpha * 1.65),
            "sideRails" | "rails" => (rgb(&g["railColor"]), n(&g["railAlpha"])),
            "xoverLow" => (
                xover_low_mid(),
                g.get("crossoverTickAlpha")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.50) as f32,
            ),
            "xoverHigh" => (
                xover_mid_high(),
                g.get("crossoverTickAlpha")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.50) as f32,
            ),
            "timeTicks" => (rgb(&g["railColor"]), n(&g["timeTickAlpha"]) * time),
            "a4Tick" => (rgb(&g["accentColor"]), n(&g["accentAlpha"])),
            "dbSpine" => (rgb(&g["spineColor"]), n(&g["spineAlpha"]) * fade),
            "dbAccents" => (rgb(&g["accentColor"]), n(&g["accentAlpha"]) * fade),
            "frontGrid" => (rgb(&g["frontGridColor"]), n(&g["frontGridAlpha"]) * front),
            "xoverFrontLow" => (
                xover_low_mid(),
                g.get("crossoverLineAlpha")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.11) as f32
                    * front,
            ),
            "xoverFrontHigh" => (
                xover_mid_high(),
                g.get("crossoverLineAlpha")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.11) as f32
                    * front,
            ),
            "headroomLine" => (rgb(&g["accentColor"]), n(&g["headroomLineAlpha"]) * front),
            _ => (rgb(&g["railColor"]), n(&l["keys"]["alpha"])),
        };
        set("u_color", &color_rgb);
        set("u_alpha", &[alpha]);
        set(
            "u_mode",
            &[match name {
                "maximum" => 3.0,
                "peak" => 1.0,
                "trace" => 2.0,
                _ => 0.0,
            }],
        );
        b
    }
    pub fn asset_scene(&self, source: &Assets) -> Assets {
        let mut assets = source.clone();
        let mut draws = Vec::new();
        let blank = vec![-120.0; band_count(self.settings.sub)];
        for g in &self.slices {
            draws.push(
                json!({"name":"lines","tag":g.name,"mode":1,"first":g.first,"count":g.count}),
            );
        }
        for (name, mode, mul) in [
            ("curtain", 5, 2),
            ("maximum", 3, 1),
            ("peak", 3, 1),
            ("trace", 3, 1),
        ] {
            draws.push(
                json!({"name":"front","tag":name,"mode":mode,"first":0,"count":blank.len()*mul}),
            );
        }
        draws.push(json!({"name":"keys","tag":"keys","mode":4,"first":0,"count":127*6}));
        assets.insert("scene.json".into(),serde_json::to_vec(&json!({"native":true,"width":self.width,"height":self.height,"count":blank.len(),"sub":self.settings.sub,"rate":self.settings.row_rate,"rows":0,"indexCount":self.indices.len(),"draws":draws})).unwrap());
        for (name, data) in [
            ("mesh", &self.mesh),
            ("guides", &self.guides),
            ("keys", &self.keys),
        ] {
            assets.insert(format!("{name}.bin"), bytemuck::cast_slice(data).to_vec());
        }
        assets.insert(
            "indices.bin".into(),
            bytemuck::cast_slice(&self.indices).to_vec(),
        );
        assets.insert(
            "front.bin".into(),
            bytemuck::cast_slice(&self.front(1, &blank)).to_vec(),
        );
        assets.insert(
            "curtain.bin".into(),
            bytemuck::cast_slice(&self.front(2, &blank)).to_vec(),
        );
        assets.insert(
            "history.bin".into(),
            vec![0; blank.len() * LEVELS * HISTORY_ROWS],
        );
        assets.insert(
            "palette.bin".into(),
            source[&format!("palette-{}.rgba", self.settings.palette)].clone(),
        );
        assets.insert("surface.uniforms".into(), self.uniforms("terrain", 0, 0.0));
        for (i, name) in self.draw_names().iter().enumerate() {
            assets.insert(format!("draw-{i}.uniforms"), self.uniforms(name, 0, 0.0));
        }
        assets
    }
    pub fn build_geometry(&mut self) {
        self.geometry_key = (
            self.settings.sub,
            self.settings.row_rate,
            self.settings.seconds.to_bits(),
            self.settings.low_level,
        );
        self.cols = band_count(self.settings.sub).max(253);
        let near_rows = self.settings.seconds * self.settings.row_rate as f64;
        let span_rows = near_rows * self.look["size"]["tail"].as_f64().unwrap();
        let mut near = 1;
        while near < 32 && near_rows / near as f64 > 360.0 {
            near *= 2;
        }
        let step = |age: f64| {
            if age < near_rows {
                near
            } else {
                ((near as f64) * 2f64.powf((age / near_rows).log2().floor() + 1.0)).min(32.0)
                    as usize
            }
        };
        let mut ages = vec![0usize];
        while (*ages.last().unwrap() as f64) < span_rows {
            let a = *ages.last().unwrap();
            ages.push(a + step(a as f64));
        }
        self.mesh.clear();
        self.indices.clear();
        for age in &ages {
            for c in 0..self.cols {
                self.mesh.extend_from_slice(&[
                    c as f32 / (self.cols - 1) as f32,
                    *age as f32,
                    step(*age as f64) as f32,
                ]);
            }
        }
        for r in 0..ages.len() - 1 {
            for c in 0..self.cols - 1 {
                let i = (r * self.cols + c) as u32;
                let cols = self.cols as u32;
                self.indices.extend_from_slice(&[
                    i,
                    i + 1,
                    i + cols,
                    i + 1,
                    i + cols + 1,
                    i + cols,
                ]);
            }
        }
        self.keys.clear();
        for k in 0..127 {
            for (edge, side) in [
                (-0.5, 0.0),
                (0.5, 0.0),
                (-0.5, 1.0),
                (0.5, 0.0),
                (0.5, 1.0),
                (-0.5, 1.0),
            ] {
                self.keys
                    .extend_from_slice(&[k as f32 + edge, side, k as f32, 0.0]);
            }
        }
        self.build_guides();
        self.guides_dirty = false;
    }
    /// Axis lines and ticks. They depend on the frequency axis, so they are rebuilt alone
    /// while it glides.
    pub fn build_guides(&mut self) {
        let size = &self.look["size"];
        let (w, h, d, tail) = (
            n(&size["width"]),
            n(&size["height"]),
            n(&size["depth"]),
            n(&size["tail"]),
        );
        let (x0, x1, zf, zend) = (-w / 2.0, w / 2.0, d / 2.0, d / 2.0 - d * tail);
        let floor = self.db_floor();
        let ceiling = n(&self.look["db"]["ceiling"]);
        let ydb = |db: f32| (db - floor) / (ceiling - floor) * h;
        let mel = self.mel as f64;
        let xhz = |hz: f64| (band_to_u(hz_to_band(hz), mel) as f32 - 0.5) * w;
        // Tick positions first: the closures below borrow the vertex lists.
        let dada = self.is_dadaspec();
        let [lo_hz, hi_hz] = self.crossovers();
        let raw_hz = self.hz_ticks(false);
        let hz: Vec<f32> = raw_hz
            .iter()
            .copied()
            .map(|f| xhz(f as f64))
            .filter(|x| *x >= x0 && *x <= x1)
            .collect();
        let octaves: Vec<f32> = (12..=120)
            .step_by(12)
            .map(|band| self.band_x(band as f64, w))
            .collect();
        let a4 = xhz(440.0);
        let x_lo = xhz(lo_hz);
        let x_hi = xhz(hi_hz);
        let every = n(&self.look["labels"]["seconds"]);
        let seconds = self.settings.seconds as f32;
        let hr = n(&self.look["labels"]["headroomDb"]);
        self.guides.clear();
        self.slices.clear();
        let mut group = |name, segments: Vec<[f32; 6]>| {
            let first = self.guides.len() as u32 / 3;
            for seg in segments {
                self.guides.extend_from_slice(&seg);
            }
            self.slices.push(Guide {
                name,
                first,
                count: self.guides.len() as u32 / 3 - first,
            });
        };
        let mut seg = Vec::new();
        for &freq in &raw_hz {
            if dada
                && ((freq as f64 - lo_hz).abs() < 1e-3 || (freq as f64 - hi_hz).abs() < 1e-3)
            {
                continue;
            }
            let x = xhz(freq as f64);
            if x >= x0 && x <= x1 {
                seg.push([x, h * 0.0006, zf, x, h * 0.0006, zend]);
            }
        }
        let mut sec = every;
        while sec <= seconds * tail + 1e-6 {
            let z = zf - sec / seconds * d;
            seg.push([x0, h * 0.0006, z, x1, h * 0.0006, z]);
            sec += every;
        }
        group("floorGrid", seg);
        group(
            "xoverFloorLow",
            if dada {
                vec![[x_lo, h * 0.0006, zf, x_lo, h * 0.0006, zend]]
            } else {
                vec![]
            },
        );
        group(
            "xoverFloorHigh",
            if dada {
                vec![[x_hi, h * 0.0006, zf, x_hi, h * 0.0006, zend]]
            } else {
                vec![]
            },
        );
        group(
            "sideRails",
            vec![[x0, 0.0, zf, x0, 0.0, zend], [x1, 0.0, zf, x1, 0.0, zend]],
        );
        let mut seg = vec![[x0, 0.0, zf, x1, 0.0, zf]];
        for &x in &hz {
            seg.push([x, 0.0, zf, x, 0.0, zf + d * 0.022]);
        }
        for x in octaves {
            seg.push([x, 0.0, zf, x, 0.0, zf + d * 0.01]);
        }
        group("rails", seg);
        group(
            "xoverLow",
            if dada {
                vec![
                    [x_lo, 0.0, zf - d * 0.012, x_lo, 0.0, zf + d * 0.026],
                    [x_lo, 0.0, zf, x_lo, h * 0.032, zf],
                ]
            } else {
                vec![]
            },
        );
        group(
            "xoverHigh",
            if dada {
                vec![
                    [x_hi, 0.0, zf - d * 0.012, x_hi, 0.0, zf + d * 0.026],
                    [x_hi, 0.0, zf, x_hi, h * 0.032, zf],
                ]
            } else {
                vec![]
            },
        );
        let mut seg = vec![];
        let mut sec = 0.0;
        while sec <= seconds + 1e-6 {
            let z = zf - sec / seconds * d;
            seg.push([x1, 0.0, z, x1 + w * 0.014, 0.0, z]);
            sec += every;
        }
        group("timeTicks", seg);
        group("a4Tick", vec![[a4, 0.0, zf, a4, 0.0, zf + d * 0.018]]);
        let mut seg = vec![
            [x0, 0.0, zf, x0, h, zf],
            [x1, h, zf, x1 - w * 0.014, h, zf],
            [x1, h, zf, x1, h - h * 0.045, zf],
        ];
        let mut db = 0.0;
        while db >= floor - 1e-6 {
            if db != hr {
                let y = ydb(db);
                let major = db % 24.0 == 0.0;
                seg.push([x0, y, zf, x0 - w * if major { 0.015 } else { 0.008 }, y, zf]);
            }
            db -= 12.0;
        }
        group("dbSpine", seg);
        let y = ydb(hr);
        group(
            "dbAccents",
            vec![
                [x0, y, zf, x0 - w * 0.016, y, zf],
                [x1, y, zf, x1 + w * 0.01, y, zf],
            ],
        );
        let mut seg = vec![];
        let mut db = 0.0;
        while db > floor + 1e-6 {
            let y = ydb(db);
            if y >= 0.0 && y <= h {
                seg.push([x0, y, zf, x1, y, zf]);
            }
            db -= 24.0;
        }
        group("frontGrid", seg);
        group(
            "xoverFrontLow",
            if dada {
                vec![[x_lo, 0.0, zf, x_lo, h, zf]]
            } else {
                vec![]
            },
        );
        group(
            "xoverFrontHigh",
            if dada {
                vec![[x_hi, 0.0, zf, x_hi, h, zf]]
            } else {
                vec![]
            },
        );
        group(
            "headroomLine",
            if y >= 0.0 && y <= h {
                vec![[x0, y, zf, x1, y, zf]]
            } else {
                vec![]
            },
        );
    }
    pub fn project_labels(&self, width: f32, height: f32) -> Vec<Label> {
        let l = &self.look;
        let size = &l["size"];
        let (w, h, d) = (n(&size["width"]), n(&size["height"]), n(&size["depth"]));
        let (_, _, fade_db, fade_time) = self.metrics();
        // Everything on the front edge gives way once the camera is behind the terrain.
        let facing = front_facing(self.matrices.back);
        let mut labels = vec![];
        let dpr = self.settings.dpr.max(0.1);
        let logical_height = height / dpr;
        let key_height = n(&l["keys"][if logical_height < 300.0 {
            "heightPxSmall"
        } else {
            "heightPx"
        }]);
        // An octave name is left out while it would sit within 22 px of the one before it:
        // every other C on a narrow log axis, most of the low octaves on the mel axis.
        let mut last_key_x = f32::NEG_INFINITY;
        let mut add = |kind, text, pos, alpha| {
            let p = transform(&self.matrices.view_proj, pos);
            if p[3] > 0.0 && kind == "key" {
                let x = (p[0] / p[3] * 0.5 + 0.5) * width / dpr;
                if (x - last_key_x).abs() < 22.0 {
                    return;
                }
                last_key_x = x;
            }
            if p[3] > 0.0 && alpha > 0.002 {
                labels.push(Label {
                    text,
                    kind,
                    x: (p[0] / p[3] * 0.5 + 0.5) * width
                        + match kind {
                            "db" => -10.0 * dpr,
                            "time" => 18.0 * dpr,
                            _ => 0.0,
                        },
                    y: (0.5 - p[1] / p[3] * 0.5) * height
                        + match kind {
                            "hz" => {
                                (if self.settings.keys {
                                    n(&l["keys"]["gapPx"]) + key_height + 4.0
                                } else {
                                    5.0
                                }) * dpr
                            }
                            "key" => (n(&l["keys"]["gapPx"]) + key_height / 2.0) * dpr,
                            "time" => -8.0 * dpr,
                            _ => 0.0,
                        },
                    alpha,
                });
            }
        };
        for hz in self.hz_ticks(width / dpr < 430.0) {
            let u = band_to_u(hz_to_band(hz as f64), self.mel as f64) as f32;
            if (-0.001..=1.001).contains(&u) {
                let text = if hz >= 1000.0 {
                    format!("{}k", hz / 1000.0)
                } else {
                    format!("{hz:.0}")
                };
                add(
                    "hz",
                    text,
                    [(u - 0.5) * w, 0.0, d / 2.0 + d * 0.026],
                    facing,
                );
            }
        }
        let db_key = if height / dpr < 260.0 {
            "dbCompact"
        } else {
            "db"
        };
        let mut dbs = floats(&l["labels"][db_key]);
        let mut low = dbs.iter().copied().fold(0.0, f32::min) - 24.0;
        while low >= self.db_floor() {
            dbs.push(low);
            low -= 24.0;
        }
        for db in dbs {
            let level = (db - self.db_floor()) / (n(&l["db"]["ceiling"]) - self.db_floor());
            if (0.0..=1.0).contains(&level) {
                add(
                    "db",
                    if db == 0.0 {
                        "0 dB".into()
                    } else {
                        format!("−{:.0}", -db)
                    },
                    [-w / 2.0 - w * 0.018, level * h, d / 2.0],
                    fade_db * facing,
                );
            }
        }
        if self.settings.waterfall {
            let mut sec = 0.0;
            let every = n(&l["labels"]["seconds"]);
            while sec <= self.settings.seconds as f32 + 1e-6 {
                if !(self.settings.seconds >= 8.0 && height / dpr < 440.0 && sec % 2.0 != 0.0) {
                    add(
                        "time",
                        if sec == 0.0 {
                            "0 s".into()
                        } else {
                            format!("−{sec} s")
                        },
                        [
                            w / 2.0 + w * 0.017,
                            0.0,
                            d / 2.0 - sec / self.settings.seconds as f32 * d,
                        ],
                        fade_time,
                    );
                }
                sec += every;
            }
        }
        if self.settings.keys {
            for band in (0..127).step_by(12) {
                add(
                    "key",
                    format!("C{}", band / 12),
                    [self.band_x((band as f64 - 0.5).max(0.0), w), 0.0, d / 2.0],
                    facing,
                );
            }
        }
        // Labels are drawn centred on their anchor. One that sits on an edge (the lowest Hz
        // tick and C0 at the bottom left in a short perspective frame) moves in far enough to
        // be read whole; one well outside the frame stays there and is clipped away. The
        // octave names keep their distance above the Hz row, so the two never overlap.
        let half_h = 7.0 * dpr;
        let key_lift = if self.settings.keys {
            (key_height / 2.0 + 4.0) * dpr
        } else {
            0.0
        };
        let nudge = |v: f32, low: f32, high: f32, reach: f32| {
            if v > low - reach && v < high + reach && high > low {
                v.clamp(low, high)
            } else {
                v
            }
        };
        for label in &mut labels {
            let half_w = (label.text.chars().count() as f32 * 3.3 + 2.0) * dpr;
            label.x = nudge(label.x, half_w, width - half_w, 2.0 * half_w);
            let lift = if label.kind == "key" { key_lift } else { 0.0 };
            label.y = nudge(label.y, half_h, height - half_h - lift, 2.0 * half_h + lift);
        }
        labels
    }
    /// Labels of the main peaks, loudest first, in the same pixels as `project_labels`
    /// (web/src/peaks.js `PeakLabels.update`). They sit on the drawn curve, so they follow
    /// the tilt and the frequency axis; they fade toward the top view and behind the terrain.
    pub fn peak_labels(&self) -> Vec<PeakLabel> {
        let s = &self.settings;
        let topness = ((self.camera.current[1] as f32 - 62.0) / 24.0).clamp(0.0, 1.0);
        let fade = if s.spectrum && s.peaks {
            (1.0 - topness) * front_facing(self.matrices.back)
        } else {
            0.0
        };
        if fade <= 0.01 {
            return vec![];
        }
        let size = &self.look["size"];
        let (w, h, d) = (n(&size["width"]), n(&size["height"]), n(&size["depth"]));
        let (floor, ceiling) = (self.db_floor(), n(&self.look["db"]["ceiling"]));
        let dpr = s.dpr.max(0.1);
        let (css_w, css_h) = (self.width as f32 / dpr, self.height as f32 / dpr);
        let tilt = self.tilt() as f64;
        let mut tracks: Vec<&PeakTrack> = self.tracks.iter().collect();
        tracks.sort_by(|p, q| q.db.total_cmp(&p.db));
        let mut shown: Vec<(f32, f32, &PeakTrack)> = vec![];
        for t in tracks {
            let Some(text) = &t.text else { continue };
            if shown.len() >= PEAK_LABELS || t.alpha <= 0.0 || text[0].is_empty() {
                continue;
            }
            let drawn = t.db + tilt * (t.band - TILT_PIVOT_BAND) / 12.0;
            let level = ((drawn as f32 - floor) / (ceiling - floor)).clamp(0.0, 1.0);
            let p = transform(
                &self.matrices.view_proj,
                [self.band_x(t.band, w), level * h, d / 2.0],
            );
            if p[3] <= 0.0 {
                continue;
            }
            let sx = (p[0] / p[3] * 0.5 + 0.5) * css_w;
            let sy = (1.0 - (p[1] / p[3] * 0.5 + 0.5)) * css_h;
            if shown
                .iter()
                .any(|(x, y, _)| (x - sx).abs() < 88.0 && (y - sy).abs() < 34.0)
            {
                continue;
            }
            shown.push((sx, sy, t));
        }
        shown
            .into_iter()
            .enumerate()
            .map(|(i, (sx, sy, t))| PeakLabel {
                text: t.text.clone().unwrap(),
                x: sx.clamp(48.0, (css_w - 48.0).max(48.0)) * dpr,
                y: sy.max(38.0) * dpr,
                alpha: t.alpha as f32 * fade,
                main: i == 0,
                band: t.band,
            })
            .collect()
    }
    /// Lights the piano keys under the labelled peaks, the loudest fully and the others
    /// dimmer (main.js). True when a key changed and the strip has to be uploaded again.
    pub fn light_keys(&mut self) -> bool {
        let mut lit = [0.0f32; 127];
        if self.settings.keys {
            for p in self.peak_labels() {
                let k = ((p.band + 0.5).floor() as usize).min(126);
                lit[k] = lit[k].max(p.alpha * if p.main { 1.0 } else { 0.6 });
            }
        }
        let mut changed = false;
        for vertex in self.keys.chunks_exact_mut(4) {
            let value = lit[vertex[2] as usize];
            if vertex[3] != value {
                vertex[3] = value;
                changed = true;
            }
        }
        changed
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    fn reference() -> Value {
        let output = std::process::Command::new("node")
            .args(["--input-type=module", "-e", "import {LOOK} from './web/src/look.js'; import {OrbitCamera} from './web/src/camera.js';const cameras=[];for(const view of ['front','top','perspective'])for(const aspect of [16/9,9/16,1,3]){const camera=new OrbitCamera(LOOK);camera.setView(view);camera.current={...camera.target};const m=camera.matrices(aspect);cameras.push({view,aspect,matrix:[...m.viewProj]});} console.log(JSON.stringify({look:LOOK,cameras}));"])
            .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .output().expect("Node is needed for canonical JS camera reference");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn assets(look: &Value) -> Assets {
        let mut assets = Assets::new();
        assets.insert("look.json".into(), serde_json::to_vec(look).unwrap());
        assets.insert("uniforms.json".into(), br#"{"size":20,"fields":{"u_newest":{"offset":0},"u_phase":{"offset":4},"u_sub":{"offset":8},"u_melAxis":{"offset":12},"u_tilt":{"offset":16}}}"#.to_vec());
        assets
    }
    #[test]
    fn camera_matches_javascript_all_presets_and_aspects() {
        let r = reference();
        for test in r["cameras"].as_array().unwrap() {
            let mut camera = Camera::new(&r["look"]);
            camera.set_view(test["view"].as_str().unwrap());
            camera.settle();
            let matrix = camera.matrices(test["aspect"].as_f64().unwrap()).view_proj;
            for (rust, js) in matrix.iter().zip(test["matrix"].as_array().unwrap()) {
                let expected = js.as_f64().unwrap() as f32;
                assert!(
                    (rust - expected).abs() < 1e-5 * expected.abs().max(1.0),
                    "{} aspect {}: {rust} != {expected}",
                    test["view"],
                    test["aspect"]
                );
            }
            let identity = camera::multiply(
                &matrix,
                &camera.matrices(test["aspect"].as_f64().unwrap()).inverse,
            );
            for (i, value) in identity.iter().enumerate() {
                assert!((value - if i % 5 == 0 { 1.0 } else { 0.0 }).abs() < 0.005);
            }
        }
    }
    #[test]
    fn each_colour_style_keeps_its_own_plot_style() {
        let r = reference();
        let assets = assets(&r["look"]);
        let mut scene = Scene::new(&assets, 1176, 800);
        assert_eq!(scene.settings.palette, "tungsten");
        scene.settings.style = "lattice".into();
        // Green has its own default the first time, then Tungsten comes back on Mesh.
        scene.set_palette("green");
        assert_eq!(scene.settings.style, "contours");
        scene.settings.style = "pure".into();
        scene.set_palette("tungsten");
        assert_eq!(scene.settings.style, "lattice");
        scene.set_palette("mel");
        assert_eq!(scene.settings.style, "sculpt");
        scene.set_palette("green");
        assert_eq!(scene.settings.style, "pure");
        // The memory survives a save and a restart, in the web's format.
        let saved = scene.settings.to_json();
        assert_eq!(saved["styleOptions"]["tungsten"]["style"], "lattice");
        let mut again = Scene::new(&assets, 1176, 800);
        again.apply_settings(&saved);
        assert_eq!(again.settings.style, "pure");
        again.set_palette("tungsten");
        assert_eq!(again.settings.style, "lattice");
        // A remembered style the look does not have is ignored.
        again
            .settings
            .style_options
            .insert("mel".into(), "missing".into());
        again.set_palette("mel");
        assert_eq!(again.settings.style, "sculpt");
    }
    #[test]
    fn multiresolution_geometry_uniform_lookup_and_setting_roundtrip() {
        let r = reference();
        let assets = assets(&r["look"]);
        let mut scene = Scene::new(&assets, 1176, 800);
        for sub in [1, 2, 4, 8] {
            for rate in [60, 120, 240, 480] {
                scene.settings.sub = sub;
                scene.settings.row_rate = rate;
                scene.settings.seconds = 32.0;
                assert!(scene.update(0.1) || sub == 1 && rate == 60);
                let vertices = scene.mesh.len() / 3;
                assert_eq!(scene.mesh.len() % 3, 0);
                assert!(scene.indices.iter().all(|&i| (i as usize) < vertices));
                assert!(scene.mesh.chunks_exact(3).all(|v| v[0] >= 0.0
                    && v[0] <= 1.0
                    && [1.0, 2.0, 4.0, 8.0, 16.0, 32.0].contains(&v[2])));
                assert!(
                    scene.mesh.len() / 3 / scene.cols < 2200,
                    "bounded tail rows"
                );
                let bytes = scene.uniforms("terrain", 131073, 0.375);
                assert_eq!(f32::from_le_bytes(bytes[0..4].try_into().unwrap()), 0.0);
                assert_eq!(f32::from_le_bytes(bytes[4..8].try_into().unwrap()), 0.375);
                assert_eq!(
                    f32::from_le_bytes(bytes[8..12].try_into().unwrap()),
                    sub as f32
                );
            }
        }
        scene.settings.low_level = true;
        scene.settings.keys = true;
        scene.settings.tilt = true;
        scene.settings.axis = "mel".into();
        scene.settings.palette = "mel".into();
        scene.update(0.1);
        let copy = Settings::from_json(&scene.look, &scene.settings.to_json());
        assert_eq!(copy, scene.settings);
        let bad = Settings::from_json(
            &scene.look,
            &json!({"sub":99,"row_rate":7,"palette":"missing","seconds":-4,"spectrum":false,"waterfall":false,"axis":"bark"}),
        );
        assert_eq!(bad.sub, DEFAULT_SUB);
        assert_eq!(bad.axis, "log");
        assert_eq!(bad.row_rate, 60);
        assert_eq!(bad.seconds, 1.0);
        assert!(bad.waterfall);
        scene.settings.spectrum = true;
        scene.settings.waterfall = false;
        scene.update(0.1);
        assert_eq!(scene.camera.view, "front");
        scene.settings.waterfall = true;
        scene.update(0.1);
        assert_eq!(scene.camera.view, "perspective");
        scene.camera.orbit(20.0, 5.0);
        scene.camera.zoom_by(0.7);
        scene.camera.pan_by(4.0, 2.0, 800.0);
        let target = scene.camera.target;
        scene.settings.waterfall = false;
        scene.update(0.0);
        scene.settings.waterfall = true;
        scene.update(0.0);
        assert_eq!(scene.camera.view, "free");
        assert_eq!(
            scene.camera.target, target,
            "restore free camera after spectrum-only view"
        );
        scene.set_palette("green");
        assert_eq!(scene.settings.style, "contours");
        scene.settings.style = "sculpt".into();
        scene.update(0.1);
        assert_eq!(
            scene.settings.style, "sculpt",
            "explicit mode wins over palette default"
        );
    }
    fn float(bytes: &[u8], offset: usize) -> f32 {
        f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }
    fn settled(view: &str) -> Scene {
        let r = reference();
        let mut scene = Scene::new(&assets(&r["look"]), 1176, 800);
        scene.settings.view = view.into();
        scene.update(0.0);
        scene.camera.settle();
        scene.update(0.0);
        scene
    }
    #[test]
    fn mel_axis_glides_and_moves_guides_and_labels_only() {
        let mut scene = settled("front");
        let log_guides = scene.guides.clone();
        let log_labels = scene.project_labels(1176.0, 800.0);
        let mesh = scene.mesh.clone();
        scene.settings.axis = "mel".into();
        assert!(
            !scene.update(0.0) && !scene.guides_dirty,
            "no time, no glide"
        );
        let mut steps = 0;
        while scene.mel != 1.0 {
            assert!(!scene.update(1.0 / 60.0), "the mesh is not rebuilt");
            assert!(scene.guides_dirty);
            assert!(scene.mel > 0.0 && scene.mel <= 1.0);
            steps += 1;
            assert!(steps < 60, "settles within a second");
        }
        assert!(steps > 10, "a glide, not a jump");
        assert_eq!(float(&scene.uniforms("terrain", 0, 0.0), 12), 1.0);
        assert_eq!(scene.mesh, mesh);
        assert_ne!(scene.guides, log_guides);
        // The mel axis has its own Hz ticks: 1 kHz moves into the left quarter, 20 Hz is gone.
        let mel_labels = scene.project_labels(1176.0, 800.0);
        let x = |labels: &[Label], text: &str| {
            labels
                .iter()
                .find(|l| l.kind == "hz" && l.text == text)
                .map(|l| l.x)
        };
        assert!(x(&log_labels, "20").is_some() && x(&mel_labels, "20").is_none());
        assert!(x(&mel_labels, "1k").unwrap() < x(&log_labels, "1k").unwrap() - 200.0);
        // Octave names closer than 22 px to the one before are left out.
        scene.settings.keys = true;
        let keys: Vec<f32> = scene
            .project_labels(1176.0, 800.0)
            .iter()
            .filter(|l| l.kind == "key")
            .map(|l| l.x)
            .collect();
        assert!(keys.len() < 11 && keys.windows(2).all(|w| w[1] - w[0] >= 22.0));
        scene.settings.axis = "log".into();
        scene.settle_axis();
        assert_eq!(scene.guides, log_guides);
        let keys = scene.project_labels(1176.0, 800.0);
        assert_eq!(keys.iter().filter(|l| l.kind == "key").count(), 11);
    }
    #[test]
    fn tilt_is_front_only_and_peak_labels_follow_it() {
        let track = |band: f64, db: f64| {
            let mut tracker = spectrum_core::PeakTracker::default();
            let mut shown = vec![-90.0f32; 127];
            shown[band as usize] = db as f32;
            for _ in 0..20 {
                tracker.update(&shown, &shown, 1.0 / 60.0, |_| None, 1);
            }
            tracker.tracks
        };
        let mut scene = settled("front");
        scene.tracks = track(95.0, -30.0);
        scene.tracks.extend(track(40.0, -40.0));
        assert!(scene.peak_labels().is_empty(), "peak readout is off");
        scene.settings.peaks = true;
        scene.settings.keys = true;
        let flat = scene.peak_labels();
        assert_eq!(flat.len(), 2);
        assert!(flat[0].main && flat[0].band == 95.0 && flat[0].alpha == 1.0);
        assert_eq!(float(&scene.uniforms("trace", 0, 0.0), 16), 0.0);
        assert!(scene.light_keys() && !scene.light_keys());
        let lit = |scene: &Scene, key: usize| scene.keys[key * 24 + 3];
        assert_eq!(
            (lit(&scene, 95), lit(&scene, 40), lit(&scene, 41)),
            (1.0, 0.6, 0.0)
        );
        scene.settings.tilt = true;
        assert_eq!(
            float(&scene.uniforms("trace", 0, 0.0), 16),
            TILT_DB_PER_OCTAVE
        );
        let tilted = scene.peak_labels();
        // Above 1 kHz the drawn curve is higher, below it lower; x does not move.
        let range = n(&scene.look["db"]["ceiling"]) - scene.db_floor();
        let per_db = (flat[1].y - flat[0].y) / 10.0;
        assert!(per_db > 0.0 && range > 0.0);
        let expected = |band: f64| (4.5 * (band - TILT_PIVOT_BAND) / 12.0) as f32 * per_db;
        assert!((flat[0].y - tilted[0].y - expected(95.0)).abs() < 0.05);
        assert!((flat[1].y - tilted[1].y - expected(40.0)).abs() < 0.05);
        assert_eq!(flat[0].x, tilted[0].x);
        // The tilt belongs to the flat front view.
        let mut scene = settled("perspective");
        scene.settings.tilt = true;
        assert_eq!(float(&scene.uniforms("trace", 0, 0.0), 16), 0.0);
    }
    #[test]
    fn front_edge_labels_fade_behind_the_terrain() {
        assert_eq!(front_facing([0.0, 0.0, 1.0]), 1.0);
        assert_eq!(
            front_facing([0.0, 1.0, 0.0]),
            1.0,
            "straight down still reads"
        );
        assert_eq!(front_facing([0.0, 0.0, -0.2]), 0.0);
        let half = front_facing([0.0, 0.0, -0.11]);
        assert!((half - 0.5).abs() < 1e-6);
        let mut scene = settled("perspective");
        scene.settings.keys = true;
        scene.settings.peaks = true;
        let count = |scene: &Scene, kind: &str| {
            scene
                .project_labels(1176.0, 800.0)
                .iter()
                .filter(|l| l.kind == kind)
                .count()
        };
        assert!(count(&scene, "hz") > 5 && count(&scene, "db") > 2 && count(&scene, "key") > 5);
        let time = count(&scene, "time");
        // Round to the back: yaw 180 degrees.
        scene.camera.target[0] += 180.0;
        scene.camera.settle();
        scene.update(0.0);
        assert_eq!(
            (
                count(&scene, "hz"),
                count(&scene, "db"),
                count(&scene, "key")
            ),
            (0, 0, 0)
        );
        assert_eq!(
            count(&scene, "time"),
            time,
            "time labels are not on the front edge"
        );
        assert!(scene.peak_labels().is_empty());
    }
    #[test]
    fn scaled_uniforms_keep_window_labels_fixed_and_scale_gpu_viewport() {
        let r = reference();
        let mut a = assets(&r["look"]);
        a.insert(
            "uniforms.json".into(),
            br#"{"size":20,"fields":{"u_viewport":{"offset":0},"u_dpr":{"offset":8},"u_keyGapPx":{"offset":12},"u_keyHeightPx":{"offset":16}}}"#.to_vec(),
        );
        let mut scene = Scene::new(&a, 1176, 800);
        scene.settings.view = "front".into();
        scene.camera.set_view("front");
        scene.camera.settle();
        scene.update(0.0);
        scene.settings.dpr = 2.0;
        let full = scene.uniforms("keys", 0, 0.0);
        let scaled = scene.uniforms_scaled("keys", 0, 0.0, [588, 400], 0.5);
        let read_vec2 = |bytes: &[u8], off: usize| {
            [
                f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()),
                f32::from_le_bytes(bytes[off + 4..off + 8].try_into().unwrap()),
            ]
        };
        let read_f32 = |bytes: &[u8], off: usize| {
            f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap())
        };
        assert_eq!(read_vec2(&full, 0), [1176.0, 800.0]);
        assert_eq!(read_vec2(&scaled, 0), [588.0, 400.0]);
        assert_eq!(read_f32(&scaled, 8), 1.0);
        assert!((read_f32(&scaled, 16) - read_f32(&full, 16) * 0.5).abs() < 1e-5);
    }
    #[test]
    fn dadaspec_look_crossovers_and_uniform_layout_match_spec() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut a = Assets::new();
        for (key, rel) in [
            ("look.json", "toolchains/spectrum-assets/look.json"),
            ("look-dadaspec.json", "toolchains/spectrum-assets/look-dadaspec.json"),
            ("uniforms.json", "toolchains/shader-lab/tungsten/uniforms.json"),
            ("dadaspec/uniforms.json", "toolchains/shader-lab/dadaspec/uniforms.json"),
        ] {
            a.insert(key.into(), std::fs::read(root.join(rel)).unwrap());
        }
        let mut scene = Scene::new(&a, 1176, 800);
        assert!(scene.has_dadaspec() && !scene.is_dadaspec());
        let tungsten_floor = scene
            .slices
            .iter()
            .find(|g| g.name == "floorGrid")
            .unwrap()
            .count;
        assert_eq!(
            scene.slices.iter().find(|g| g.name == "xoverLow").unwrap().count,
            0
        );
        assert!(!scene.draw_visible("xoverLow"));

        scene.set_palette("dadaspec");
        assert!(scene.is_dadaspec());
        assert_eq!(scene.settings.style, "sculpt");
        assert_eq!(scene.crossovers(), [200.0, 1500.0]);
        let dada_floor = scene
            .slices
            .iter()
            .find(|g| g.name == "floorGrid")
            .unwrap()
            .count;
        // 200 Hz (1 line) skipped in floorGrid; 1500 Hz isn't in look.labels.hz.
        assert_eq!(tungsten_floor - dada_floor, 2);
        assert_eq!(
            scene.slices.iter().find(|g| g.name == "xoverLow").unwrap().count,
            4
        );
        assert_eq!(
            scene.slices.iter().find(|g| g.name == "xoverHigh").unwrap().count,
            4
        );
        assert_eq!(
            scene
                .slices
                .iter()
                .find(|g| g.name == "xoverFloorLow")
                .unwrap()
                .count,
            2
        );
        assert_eq!(
            scene
                .slices
                .iter()
                .find(|g| g.name == "xoverFrontHigh")
                .unwrap()
                .count,
            2
        );
        assert!(scene.draw_visible("xoverLow"));

        scene.live_chroma = [0.8, 0.4, 0.2, 0.65];
        scene.settings.style = "contours".into();
        let saved = scene.settings.to_json();
        let bytes = scene.uniforms("terrain", 12, 0.5);
        assert_eq!(bytes.len(), 592);
        let dlayout: Value =
            serde_json::from_slice(&a["dadaspec/uniforms.json"]).unwrap();
        let off = |k: &str| dlayout["fields"][k]["offset"].as_u64().unwrap() as usize;
        assert_eq!(float(&bytes, off("u_seamBoost")), 0.76);
        assert_eq!(float(&bytes, off("u_crossoverStrength")), 0.40);
        assert_eq!(float(&bytes, off("u_chromaSat")), 1.0);
        assert_eq!(float(&bytes, off("u_liveChroma")), 0.8);
        assert_eq!(float(&bytes, off("u_liveChroma") + 12), 0.65);

        let xover = scene.uniforms("xoverLow", 12, 0.5);
        assert!((float(&xover, off("u_alpha")) - 0.52).abs() < 1e-6);

        // Reload persisted settings from a fresh Scene instance.
        let mut reloaded = Scene::new(&a, 1176, 800);
        reloaded.apply_settings(&saved);
        assert!(reloaded.is_dadaspec());
        assert_eq!(reloaded.settings.style, "contours");
        reloaded.set_palette("tungsten");
        assert!(!reloaded.is_dadaspec());
        assert_eq!(
            reloaded.slices.iter().find(|g| g.name == "floorGrid").unwrap().count,
            tungsten_floor
        );
        assert!(!reloaded.draw_visible("xoverLow"));
    }
}
