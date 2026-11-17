//! Orbit camera ported from web/src/camera.js. All visual constants come from look.json.
use serde_json::Value;
#[derive(Clone, Debug)]
pub struct Camera {
    pub current: [f64; 7],
    pub target: [f64; 7],
    velocity: [f64; 7],
    smooth_time: f64,
    pub view: String,
    look: Value,
}
#[derive(Clone, Debug)]
pub struct Matrices {
    pub view_proj: [f32; 16],
    pub inverse: [f32; 16],
    pub back: [f64; 3],
}
fn number(v: &Value) -> f64 {
    v.as_f64().expect("numeric camera look value")
}
impl Camera {
    pub fn new(look: &Value) -> Self {
        let state = Self::from_view(&look["views"]["perspective"]);
        Self {
            current: state,
            target: state,
            velocity: [0.0; 7],
            smooth_time: number(&look["motion"]["presetSeconds"]),
            view: "perspective".into(),
            look: look.clone(),
        }
    }
    fn from_view(v: &Value) -> [f64; 7] {
        [
            number(&v["yaw"]),
            number(&v["pitch"]),
            number(&v["fov"]).ln(),
            number(&v["zoom"]).ln(),
            number(&v["pan"][0]),
            number(&v["pan"][1]),
            number(&v["pan"][2]),
        ]
    }
    pub fn set_look(&mut self, look: &Value) {
        self.look = look.clone();
    }
    pub fn set_view(&mut self, name: &str) {
        if !self.look["views"][name].is_object() {
            return;
        }
        self.target = Self::from_view(&self.look["views"][name]);
        self.target[0] += 360.0 * ((self.current[0] - self.target[0]) / 360.0).round();
        self.smooth_time = number(&self.look["motion"]["presetSeconds"]);
        self.view = name.into();
    }
    pub fn restore(&mut self, view: &str, mut target: [f64; 7]) {
        target[0] += 360.0 * ((self.current[0] - target[0]) / 360.0).round();
        self.target = target;
        self.view = view.into();
        self.smooth_time = number(&self.look["motion"]["presetSeconds"]);
    }
    pub fn settle(&mut self) {
        self.current = self.target;
        self.velocity.fill(0.0);
    }
    pub fn orbit(&mut self, dx: f64, dy: f64) {
        if self.target[2].exp() < 10.0 {
            self.target[2] = number(&self.look["freeFov"]).ln();
        }
        self.target[0] -= dx * 0.3;
        self.target[1] = (self.target[1] + dy * 0.3).clamp(
            number(&self.look["motion"]["pitchMin"]),
            number(&self.look["motion"]["pitchMax"]),
        );
        self.smooth_time = number(&self.look["motion"]["dragSeconds"]);
        self.view = "free".into();
    }
    pub fn zoom_by(&mut self, factor: f64) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        self.target[3] = (self.target[3] + factor.ln()).clamp(0.25f64.ln(), 3.0f64.ln());
        self.smooth_time = number(&self.look["motion"]["dragSeconds"]) * 2.0;
    }
    pub fn pan_by(&mut self, dx: f64, dy: f64, viewport_h: f64) {
        let (_, right, up, dist) = self.basis(1.0);
        let s = 2.0 * dist * (self.current[2].exp().to_radians() / 2.0).tan() / viewport_h.max(1.0);
        for i in 0..3 {
            self.target[4 + i] -= (right[i] * dx - up[i] * dy) * s;
        }
        self.smooth_time = number(&self.look["motion"]["dragSeconds"]);
    }
    pub fn update(&mut self, dt: f64) {
        let dt = dt.clamp(0.0, 0.1);
        let omega = 2.0 / self.smooth_time.max(1e-4);
        let x = omega * dt;
        let exp = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
        for i in 0..7 {
            let change = self.current[i] - self.target[i];
            let temp = (self.velocity[i] + omega * change) * dt;
            self.velocity[i] = (self.velocity[i] - omega * temp) * exp;
            self.current[i] = self.target[i] + (change + temp) * exp;
        }
    }
    fn basis(&self, aspect: f64) -> ([f64; 3], [f64; 3], [f64; 3], f64) {
        let yaw = self.current[0].to_radians();
        let pitch = self.current[1].to_radians();
        let back = [
            pitch.cos() * yaw.sin(),
            pitch.sin(),
            pitch.cos() * yaw.cos(),
        ];
        let right = [yaw.cos(), 0.0, -yaw.sin()];
        let up = cross(back, right);
        let size = &self.look["size"];
        let (w, h, d) = (
            number(&size["width"]),
            number(&size["height"]),
            number(&size["depth"]),
        );
        let tan_v = (self.current[2].exp().to_radians() / 2.0).tan();
        let tan_h = tan_v * aspect.max(1e-6);
        let mut distance: f64 = 0.0;
        for x in [-w / 2.0, w / 2.0] {
            for y in [-h / 2.0, h / 2.0] {
                for z in [-d / 2.0, d / 2.0] {
                    let p = [x, y, z];
                    let toward = dot(p, back);
                    distance = distance
                        .max(dot(p, right).abs() / tan_h + toward)
                        .max(dot(p, up).abs() / tan_v + toward);
                }
            }
        }
        (back, right, up, distance * 1.06 * self.current[3].exp())
    }
    pub fn matrices(&self, aspect: f64) -> Matrices {
        let (back, _, _, dist) = self.basis(aspect);
        let s = &self.look["size"];
        let (w, h, d) = (
            number(&s["width"]),
            number(&s["height"]),
            number(&s["depth"]),
        );
        let center = [self.current[4], h / 2.0 + self.current[5], self.current[6]];
        let eye = std::array::from_fn(|i| center[i] + back[i] * dist);
        let radius = (w * w + h * h + d * d).sqrt() / 2.0;
        let near = (dist * 0.01).max(dist - radius * 2.5);
        let far = dist + radius * 2.5 + d * number(&s["tail"]);
        let yaw = self.current[0].to_radians();
        let up = if self.current[1] > 89.0 {
            [-yaw.sin(), 0.0, -yaw.cos()]
        } else {
            [0.0, 1.0, 0.0]
        };
        let z = normalize(std::array::from_fn(|i| eye[i] - center[i]));
        let x = normalize(cross(up, z));
        let y = cross(z, x);
        let view = [
            x[0] as f32,
            y[0] as f32,
            z[0] as f32,
            0.0,
            x[1] as f32,
            y[1] as f32,
            z[1] as f32,
            0.0,
            x[2] as f32,
            y[2] as f32,
            z[2] as f32,
            0.0,
            -dot(x, eye) as f32,
            -dot(y, eye) as f32,
            -dot(z, eye) as f32,
            1.0,
        ];
        let f = 1.0 / (self.current[2].exp().to_radians() / 2.0).tan();
        let nf = 1.0 / (near - far);
        let proj = [
            (f / aspect.max(1e-6)) as f32,
            0.0,
            0.0,
            0.0,
            0.0,
            f as f32,
            0.0,
            0.0,
            0.0,
            0.0,
            ((far + near) * nf) as f32,
            -1.0,
            0.0,
            0.0,
            (2.0 * far * near * nf) as f32,
            0.0,
        ];
        let vp = multiply(&proj, &view);
        Matrices {
            view_proj: vp,
            inverse: invert(&vp).expect("invertible camera"),
            back,
        }
    }
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(a: [f64; 3]) -> [f64; 3] {
    let n = dot(a, a).sqrt().max(1e-12);
    a.map(|v| v / n)
}
pub fn multiply(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    std::array::from_fn(|i| {
        let (c, r) = (i / 4, i % 4);
        (0..4)
            .map(|k| a[k * 4 + r] as f64 * b[c * 4 + k] as f64)
            .sum::<f64>() as f32
    })
}
pub fn transform(m: &[f32; 16], p: [f32; 3]) -> [f32; 4] {
    std::array::from_fn(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
}
pub fn invert(m: &[f32; 16]) -> Option<[f32; 16]> {
    let mut a = [[0.0f64; 8]; 4];
    for r in 0..4 {
        for c in 0..4 {
            a[r][c] = m[c * 4 + r] as f64;
        }
        a[r][4 + r] = 1.0;
    }
    for c in 0..4 {
        let pivot = (c..4).max_by(|&r, &s| a[r][c].abs().total_cmp(&a[s][c].abs()))?;
        if a[pivot][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, pivot);
        let d = a[c][c];
        for j in 0..8 {
            a[c][j] /= d;
        }
        for r in 0..4 {
            if r != c {
                let factor = a[r][c];
                for j in 0..8 {
                    a[r][j] -= factor * a[c][j];
                }
            }
        }
    }
    Some(std::array::from_fn(|i| a[i % 4][4 + i / 4] as f32))
}
