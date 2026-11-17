#[cfg(not(target_arch = "wasm32"))]
pub mod audio;
pub mod gpu;
#[cfg(target_arch = "wasm32")]
mod web {
    use super::gpu::Gpu;
    use std::collections::HashMap;
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    pub struct WebRenderer {
        gpu: Gpu,
        surface: wgpu::Surface<'static>,
        canvas: web_sys::HtmlCanvasElement,
        config: wgpu::SurfaceConfiguration,
    }
    impl WebRenderer {
        async fn build(
            canvas: web_sys::HtmlCanvasElement,
            files: js_sys::Map,
            backend: String,
            snapshot: bool,
        ) -> Result<Self, JsValue> {
            let mut assets = HashMap::new();
            for item in files.entries() {
                let pair = js_sys::Array::from(&item?);
                let name = pair
                    .get(0)
                    .as_string()
                    .ok_or_else(|| JsValue::from_str("asset key must be a string"))?;
                assets.insert(name, js_sys::Uint8Array::new(&pair.get(1)).to_vec());
            }
            for name in if snapshot {
                &["scene.json", "history.bin", "uniforms.json"][..]
            } else {
                &[
                    "look.json",
                    "uniforms.json",
                    "terrain.vert.wgsl",
                    "surface.frag.wgsl",
                    "keys.vert.wgsl",
                ][..]
            } {
                if !assets.contains_key(*name) {
                    return Err(JsValue::from_str(&format!(
                        "Missing generated asset {name}; run scripts/wgpu-build.sh"
                    )));
                }
            }
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends: if backend == "webgl" {
                    wgpu::Backends::GL
                } else {
                    wgpu::Backends::BROWSER_WEBGPU
                },
                ..Default::default()
            });
            let surface = instance
                .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    compatible_surface: Some(&surface),
                    ..Default::default()
                })
                .await
                .map_err(|e| JsValue::from_str(&e.to_string()))?;
            let formats = surface.get_capabilities(&adapter).formats;
            let format = formats
                .iter()
                .copied()
                .find(|format| {
                    matches!(
                        format,
                        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                    )
                })
                .or_else(|| formats.first().copied())
                .ok_or_else(|| JsValue::from_str("No supported canvas surface format"))?;
            let mut gpu = if snapshot {
                Gpu::new(&instance, Some(&surface), format, &assets).await
            } else {
                Gpu::new_live(
                    &instance,
                    Some(&surface),
                    format,
                    &assets,
                    canvas.width().max(64),
                    canvas.height().max(64),
                )
                .await
            };
            if !snapshot {
                // A deterministic reference image is generated in Rust, without a
                // browser-exported scene, while the live button starts audio time.
                for _ in 0..240 {
                    gpu.advance(1.0 / 60.0);
                }
            }
            canvas.set_width(gpu.width);
            canvas.set_height(gpu.height);
            let config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: gpu.width,
                height: gpu.height,
                present_mode: wgpu::PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
                alpha_mode: wgpu::CompositeAlphaMode::Opaque,
                view_formats: vec![],
            };
            surface.configure(&gpu.device, &config);
            Ok(Self {
                gpu,
                surface,
                canvas,
                config,
            })
        }
    }
    #[wasm_bindgen]
    impl WebRenderer {
        /// Default entry point: Rust owns scene, camera, geometry and uniforms.
        #[wasm_bindgen(js_name=create)]
        pub async fn create(
            canvas: web_sys::HtmlCanvasElement,
            files: js_sys::Map,
            backend: String,
        ) -> Result<WebRenderer, JsValue> {
            Self::build(canvas, files, backend, false).await
        }
        /// Optional legacy capture, packaged only with `wgpu-build.sh --snapshot`.
        #[wasm_bindgen(js_name=createSnapshot)]
        pub async fn create_snapshot(
            canvas: web_sys::HtmlCanvasElement,
            files: js_sys::Map,
            backend: String,
        ) -> Result<WebRenderer, JsValue> {
            Self::build(canvas, files, backend, true).await
        }
        pub fn advance(&mut self, dt: f64) {
            if dt.is_finite() {
                self.gpu.advance(dt.max(0.0));
            }
        }
        pub fn render(&mut self) -> Result<(), JsValue> {
            let frame = match self.surface.get_current_texture() {
                Ok(frame) => frame,
                Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                    self.surface.configure(&self.gpu.device, &self.config);
                    self.surface
                        .get_current_texture()
                        .map_err(|e| JsValue::from_str(&e.to_string()))?
                }
                Err(error) => return Err(JsValue::from_str(&error.to_string())),
            };
            self.gpu
                .render(&frame.texture.create_view(&Default::default()));
            frame.present();
            Ok(())
        }
        pub fn resize(&mut self, width: u32, height: u32) {
            if width == 0 || height == 0 || width > 8192 || height > 8192 {
                return;
            }
            self.gpu.resize(width, height);
            self.canvas.set_width(width);
            self.canvas.set_height(height);
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.gpu.device, &self.config);
        }
        pub fn set_scale(&mut self, scale: f32) {
            if scale.is_finite() && scale > 0.0 {
                if let Some(scene) = self.gpu.scene.as_mut() {
                    scene.settings.dpr = scale.clamp(0.25, 4.0);
                }
            }
        }
        pub fn settings(&self) -> String {
            self.gpu
                .scene
                .as_ref()
                .map(|s| s.settings.to_json().to_string())
                .unwrap_or_else(|| "{}".into())
        }
        pub fn configure(&mut self, json: &str) -> Result<(), JsValue> {
            let patch: serde_json::Value =
                serde_json::from_str(json).map_err(|e| JsValue::from_str(&e.to_string()))?;
            let patch = patch
                .as_object()
                .ok_or_else(|| JsValue::from_str("Settings must be a JSON object"))?;
            if let Some(scene) = self.gpu.scene.as_mut() {
                let mut merged = scene.settings.to_json();
                for (key, value) in patch {
                    merged[key] = value.clone();
                }
                let scale = scene.settings.dpr;
                scene.apply_settings(&merged);
                scene.settings.dpr = scale;
            }
            Ok(())
        }
        pub fn set_view(&mut self, name: &str) {
            if let Some(scene) = self.gpu.scene.as_mut() {
                if scene.look["views"][name].is_object() {
                    scene.settings.view = name.into();
                }
            }
        }
        pub fn set_palette(&mut self, name: &str) {
            if let Some(scene) = self.gpu.scene.as_mut() {
                scene.set_palette(name);
            }
        }
        pub fn orbit(&mut self, dx: f64, dy: f64) {
            if dx.is_finite() && dy.is_finite() {
                if let Some(scene) = self.gpu.scene.as_mut() {
                    scene.camera.orbit(dx, dy);
                }
            }
        }
        pub fn pan(&mut self, dx: f64, dy: f64) {
            if dx.is_finite() && dy.is_finite() {
                if let Some(scene) = self.gpu.scene.as_mut() {
                    scene.camera.pan_by(dx, dy, self.gpu.height as f64);
                }
            }
        }
        pub fn zoom(&mut self, factor: f64) {
            if let Some(scene) = self.gpu.scene.as_mut() {
                scene.camera.zoom_by(factor);
            }
        }
        pub fn clear_maximum(&mut self) {
            self.gpu.clear_maximum();
        }
        pub fn labels(&self) -> String {
            self.gpu.scene.as_ref().map(|s| serde_json::Value::Array(s.project_labels(self.gpu.width as f32, self.gpu.height as f32)
                .into_iter().map(|l| serde_json::json!({"text":l.text,"kind":l.kind,"x":l.x,"y":l.y,"alpha":l.alpha})).collect()).to_string()).unwrap_or_else(|| "[]".into())
        }
        pub fn adapter(&self) -> String {
            self.gpu.adapter.clone()
        }
    }
}
