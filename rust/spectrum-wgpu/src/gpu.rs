#[path = "scene.rs"]
pub mod scene;
use serde_json::Value;
use std::{fs, path::Path};
use wgpu::util::DeviceExt;
#[derive(Clone, Copy)]
enum DrawKind {
    Keys,
    Front,
    Line,
}

struct Draw {
    tag: String,
    uniform_index: usize,
    front_data: Option<Vec<f32>>,
    per_band: usize,
    kind: DrawKind,
    topology: wgpu::PrimitiveTopology,
    blend: wgpu::BlendState,
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    dadaspec_pipeline: Option<wgpu::RenderPipeline>,
    dadaspec_bind: Option<wgpu::BindGroup>,
    buffer: wgpu::Buffer,
    first: u32,
    count: u32,
}

struct PipelineShaders {
    terrain_vs: wgpu::ShaderModule,
    terrain_fs: wgpu::ShaderModule,
    keys_vs: wgpu::ShaderModule,
    keys_fs: wgpu::ShaderModule,
    front_vs: wgpu::ShaderModule,
    front_fs: wgpu::ShaderModule,
    line_vs: wgpu::ShaderModule,
    line_fs: wgpu::ShaderModule,
}

struct DadaspecShaders {
    terrain_vs: wgpu::ShaderModule,
    terrain_fs: wgpu::ShaderModule,
    keys_vs: wgpu::ShaderModule,
    keys_fs: wgpu::ShaderModule,
    front_vs: wgpu::ShaderModule,
    front_fs: wgpu::ShaderModule,
    line_vs: wgpu::ShaderModule,
    line_fs: wgpu::ShaderModule,
}

struct ScaledTarget {
    width: u32,
    height: u32,
    msaa_samples: u32,
    color: wgpu::TextureView,
    msaa: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
    blit_bind: wgpu::BindGroup,
}

struct SnapshotScaleOffsets {
    viewport: usize,
    dpr: usize,
    key_gap: usize,
    key_height: usize,
    base_dpr: f32,
    base_gap: f32,
    base_key_height: f32,
}

const BLIT_WGSL: &str = r#"
struct BlitOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn blit_vs(@builtin(vertex_index) idx: u32) -> BlitOut {
    let x = f32(i32(idx & 1u) * 4 - 1);
    let y = f32(i32(idx >> 1u) * 4 - 1);
    var out: BlitOut;
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5 + 0.5, 0.5 - y * 0.5);
    return out;
}

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_samp: sampler;

@fragment
fn blit_fs(in: BlitOut) -> @location(0) vec4<f32> {
    return textureSample(src_tex, src_samp, in.uv);
}
"#;

/// Normalizes MSAA sample counts to supported desktop values (`1` for off, `4` for 4x).
pub fn normalize_msaa(samples: u32) -> u32 {
    match samples {
        0 | 1 => 1,
        _ => 4,
    }
}

/// Clamps and snaps render scale near 1.0 to full native pass resolution.
pub fn normalize_render_scale(scale: f32) -> f32 {
    if !scale.is_finite() || scale >= 0.999 {
        1.0
    } else {
        scale.clamp(0.25, 1.0)
    }
}

/// Computes offscreen render pixel dimensions `[rw, rh]` for a given stage size and scale.
pub fn scaled_dimensions(stage_w: u32, stage_h: u32, render_scale: f32) -> [u32; 2] {
    let w = stage_w.max(1);
    let h = stage_h.max(1);
    let scale = normalize_render_scale(render_scale);
    if scale >= 0.999 {
        [w, h]
    } else {
        [
            ((w as f32 * scale).round() as u32).max(1),
            ((h as f32 * scale).round() as u32).max(1),
        ]
    }
}

fn create_attachment(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    sample_count: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

#[allow(clippy::too_many_arguments)]
fn build_spectrum_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    sample_count: u32,
    name: &str,
    vs: &wgpu::ShaderModule,
    fs: &wgpu::ShaderModule,
    topology: wgpu::PrimitiveTopology,
    blend: Option<wgpu::BlendState>,
    write: bool,
    kind: Option<DrawKind>,
) -> wgpu::RenderPipeline {
    let keys_attrs = wgpu::vertex_attr_array![0=>Float32,1=>Float32,2=>Float32,3=>Float32];
    let front_attrs = wgpu::vertex_attr_array![0=>Float32,1=>Float32,2=>Float32];
    let vec3_attrs = wgpu::vertex_attr_array![0=>Float32x3];
    let (array_stride, attributes): (u64, &[wgpu::VertexAttribute]) = match kind {
        Some(DrawKind::Keys) => (16, &keys_attrs),
        Some(DrawKind::Front) => (12, &front_attrs),
        None | Some(DrawKind::Line) => (12, &vec3_attrs),
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(name),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: vs,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology,
            strip_index_format: None,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth24Plus,
            depth_write_enabled: write,
            // The piano strip is depth-tested too, without writing depth: from the
            // front nothing of the box sits there, from behind the terrain hides it.
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: Default::default(),
            bias: if write {
                wgpu::DepthBiasState {
                    constant: 1,
                    slope_scale: 1.0,
                    clamp: 0.0,
                }
            } else {
                Default::default()
            },
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: fs,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    })
}

fn build_blit_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("spectrum-blit"),
        source: wgpu::ShaderSource::Wgsl(BLIT_WGSL.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("spectrum-blit-layout"),
        bind_group_layouts: &[layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("spectrum-blit"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("blit_vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("blit_fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    })
}

pub struct Gpu {
    history_texture: wgpu::Texture,
    chroma_texture: wgpu::Texture,
    uniform_buffers: Vec<(wgpu::Buffer, Vec<u8>)>,
    newest_offset: usize,
    phase_offset: usize,
    snapshot_scale: Option<SnapshotScaleOffsets>,
    live: spectrum_core::Analysis,
    history: spectrum_core::History,
    chroma: spectrum_core::DadaspecChroma,
    row_clock: spectrum_core::RowClock,
    signal: spectrum_core::TestSignal,
    display: spectrum_core::DisplaySpectrum,
    sample_clock: f64,
    phase: f64,
    pub scene: Option<scene::Scene>,
    external_input: bool,
    tracker: spectrum_core::PeakTracker,
    format: wgpu::TextureFormat,
    msaa_samples: u32,
    render_scale: f32,
    palette_texture: wgpu::Texture,
    current_palette: String,
    /// The scene geometry key the mesh and overlay buffers were built from. Compared with
    /// the scene on every sync, so a rebuild cannot be lost to whoever calls `update` first.
    built_geometry: (usize, u32, u64, bool),
    palettes: std::collections::HashMap<String, Vec<u8>>,
    bind_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    shaders: PipelineShaders,
    dadaspec_shaders: Option<DadaspecShaders>,
    blit_layout: wgpu::BindGroupLayout,
    blit_sampler: wgpu::Sampler,
    blit_pipeline: wgpu::RenderPipeline,
    history_sampler: wgpu::Sampler,
    palette_sampler: wgpu::Sampler,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    mesh: wgpu::Buffer,
    indices: wgpu::Buffer,
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    dadaspec_pipeline: Option<wgpu::RenderPipeline>,
    dadaspec_bind: Option<wgpu::BindGroup>,
    draws: Vec<Draw>,
    count: u32,
    pub width: u32,
    pub height: u32,
    /// The part of the target the spectrum is drawn in, in pixels (x, y, width, height):
    /// the window beside the docked panels. `None` is the whole target.
    stage: Option<[u32; 4]>,
    /// Window-sized depth and MSAA targets for drawing at full scale; dropped while the
    /// spectrum renders offscreen at a lower scale (F7).
    depth: Option<wgpu::TextureView>,
    msaa: Option<wgpu::TextureView>,
    scaled: Option<ScaledTarget>,
    queries: Option<wgpu::QuerySet>,
    resolve: Option<wgpu::Buffer>,
    read: Option<wgpu::Buffer>,
    /// The window's monitor reads the pass time without waiting for the GPU: one readback
    /// at a time, picked up a frame or two later.
    timing_async: bool,
    timing_pending: std::cell::Cell<bool>,
    timing_ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
    gpu_ms: std::cell::Cell<Option<f64>>,
    #[cfg(not(target_arch = "wasm32"))]
    submitted_at: std::cell::Cell<Option<std::time::Instant>>,
    pub adapter: String,
}
impl Gpu {
    pub async fn new(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        format: wgpu::TextureFormat,
        assets: &std::collections::HashMap<String, Vec<u8>>,
    ) -> Self {
        let scene: Value = serde_json::from_slice(&assets["scene.json"]).unwrap();
        let width = scene["width"].as_u64().unwrap() as u32;
        let height = scene["height"].as_u64().unwrap() as u32;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: surface,
                ..Default::default()
            })
            .await
            .unwrap();
        let timestamps_supported = !cfg!(target_arch = "wasm32")
            && adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY);
        let timestamp = timestamps_supported && scene["native"] != true;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("spectrum"),
                required_limits: if cfg!(target_arch = "wasm32") {
                    wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())
                } else {
                    wgpu::Limits::default()
                },
                required_features: if timestamps_supported {
                    wgpu::Features::TIMESTAMP_QUERY
                } else {
                    wgpu::Features::empty()
                },
                ..Default::default()
            })
            .await
            .unwrap();
        let info = adapter.get_info();
        let adapter_name = format!("{} / {:?}", info.name, info.backend);
        let module = |name: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(
                    String::from_utf8(assets[&format!("{name}.wgsl")].clone())
                        .unwrap()
                        .into(),
                ),
            })
        };
        let texture =
            |label: &str, w: u32, h: u32, format: wgpu::TextureFormat, data: &[u8], bpp: u32| {
                let t = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                queue.write_texture(
                    t.as_image_copy(),
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w * bpp),
                        rows_per_image: Some(h),
                    },
                    t.size(),
                );
                t
            };
        let history_texture = texture(
            "history",
            scene["count"].as_u64().unwrap() as u32 * 6,
            4096,
            wgpu::TextureFormat::R8Unorm,
            &assets["history.bin"],
            1,
        );
        let history = history_texture.create_view(&Default::default());
        let chroma_init =
            vec![0u8; spectrum_core::CHROMA_WIDTH * spectrum_core::HISTORY_ROWS * 4];
        let chroma_texture = texture(
            "dadaspec chroma",
            spectrum_core::CHROMA_WIDTH as u32,
            spectrum_core::HISTORY_ROWS as u32,
            wgpu::TextureFormat::Rgba8Unorm,
            &chroma_init,
            4,
        );
        let chroma_view = chroma_texture.create_view(&Default::default());
        let palette_texture = texture(
            "palette",
            256,
            1,
            wgpu::TextureFormat::Rgba8Unorm,
            &assets["palette.bin"],
            4,
        );
        let palette = palette_texture.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear-history"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let palette_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear-palette"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let visibility = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("GLSL-adapter-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let has_dadaspec = [
            "dadaspec/terrain.vert.wgsl",
            "dadaspec/surface.frag.wgsl",
            "dadaspec/keys.vert.wgsl",
            "dadaspec/keys.frag.wgsl",
            "dadaspec/front.vert.wgsl",
            "dadaspec/front.frag.wgsl",
            "dadaspec/line.vert.wgsl",
            "dadaspec/line.frag.wgsl",
        ]
        .iter()
        .all(|k| assets.contains_key(*k));
        let dadaspec_uniform_size = assets
            .get("dadaspec/uniforms.json")
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
            .and_then(|v| v["size"].as_u64())
            .unwrap_or(0) as usize;
        let mut uniform_buffers = vec![];
        let mut bind = |name: &str| {
            let bytes = assets[&format!("{name}.uniforms")].clone();
            let mut init_bytes = bytes.clone();
            if init_bytes.len() < dadaspec_uniform_size {
                init_bytes.resize(dadaspec_uniform_size, 0);
            }
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(name),
                contents: &init_bytes,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(name),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&history),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&palette),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&palette_sampler),
                    },
                ],
            });
            let dadaspec_group = has_dadaspec.then(|| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("{name}-dadaspec")),
                    layout: &layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&history),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&chroma_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                    ],
                })
            });
            uniform_buffers.push((buffer, bytes));
            (group, dadaspec_group)
        };
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let msaa_samples = 4;
        let shaders = PipelineShaders {
            terrain_vs: module("terrain.vert"),
            terrain_fs: module("surface.frag"),
            keys_vs: module("keys.vert"),
            keys_fs: module("keys.frag"),
            front_vs: module("front.vert"),
            front_fs: module("front.frag"),
            line_vs: module("line.vert"),
            line_fs: module("line.frag"),
        };
        let dadaspec_shaders = has_dadaspec.then(|| DadaspecShaders {
            terrain_vs: module("dadaspec/terrain.vert"),
            terrain_fs: module("dadaspec/surface.frag"),
            keys_vs: module("dadaspec/keys.vert"),
            keys_fs: module("dadaspec/keys.frag"),
            front_vs: module("dadaspec/front.vert"),
            front_fs: module("dadaspec/front.frag"),
            line_vs: module("dadaspec/line.vert"),
            line_fs: module("dadaspec/line.frag"),
        });
        let pipeline = build_spectrum_pipeline(
            &device,
            &pipeline_layout,
            format,
            msaa_samples,
            "terrain",
            &shaders.terrain_vs,
            &shaders.terrain_fs,
            wgpu::PrimitiveTopology::TriangleList,
            None,
            true,
            None,
        );
        let dadaspec_pipeline = dadaspec_shaders.as_ref().map(|ds| {
            build_spectrum_pipeline(
                &device,
                &pipeline_layout,
                format,
                msaa_samples,
                "dadaspec-terrain",
                &ds.terrain_vs,
                &ds.terrain_fs,
                wgpu::PrimitiveTopology::TriangleList,
                None,
                true,
                None,
            )
        });
        let (surface_bind, dadaspec_bind) = bind("surface");
        let buffer = |file: &str, usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(file),
                contents: &assets[&format!("{file}.bin")],
                usage,
            })
        };
        let mesh = buffer("mesh", wgpu::BufferUsages::VERTEX);
        let indices = buffer("indices", wgpu::BufferUsages::INDEX);
        let mut draws = vec![];
        for (i, d) in scene["draws"].as_array().unwrap().iter().enumerate() {
            let front = d["name"] == "front";
            let keys = d["name"] == "keys";
            let mode = d["mode"].as_u64().unwrap();
            let topology = match mode {
                1 => wgpu::PrimitiveTopology::LineList,
                3 => wgpu::PrimitiveTopology::LineStrip,
                5 => wgpu::PrimitiveTopology::TriangleStrip,
                4 => wgpu::PrimitiveTopology::TriangleList,
                _ => panic!("unsupported primitive {mode}"),
            };
            let kind = if keys {
                DrawKind::Keys
            } else if front {
                DrawKind::Front
            } else {
                DrawKind::Line
            };
            let (vs, fs) = match kind {
                DrawKind::Keys => (&shaders.keys_vs, &shaders.keys_fs),
                DrawKind::Front => (&shaders.front_vs, &shaders.front_fs),
                DrawKind::Line => (&shaders.line_vs, &shaders.line_fs),
            };
            let blend = if front || keys {
                wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING
            } else {
                wgpu::BlendState::ALPHA_BLENDING
            };
            let p = build_spectrum_pipeline(
                &device,
                &pipeline_layout,
                format,
                msaa_samples,
                "overlay",
                vs,
                fs,
                topology,
                Some(blend),
                false,
                Some(kind),
            );
            let dada_p = dadaspec_shaders.as_ref().map(|ds| {
                let (dvs, dfs) = match kind {
                    DrawKind::Keys => (&ds.keys_vs, &ds.keys_fs),
                    DrawKind::Front => (&ds.front_vs, &ds.front_fs),
                    DrawKind::Line => (&ds.line_vs, &ds.line_fs),
                };
                build_spectrum_pipeline(
                    &device,
                    &pipeline_layout,
                    format,
                    msaa_samples,
                    "dadaspec-overlay",
                    dvs,
                    dfs,
                    topology,
                    Some(blend),
                    false,
                    Some(kind),
                )
            });
            let file = if keys {
                "keys"
            } else if front {
                if mode == 5 {
                    "curtain"
                } else {
                    "front"
                }
            } else {
                "guides"
            };
            let (draw_bind, dada_bind) = bind(&format!("draw-{i}"));
            draws.push(Draw {
                tag: d["tag"].as_str().unwrap_or("").into(),
                uniform_index: i + 1,
                kind,
                topology,
                blend,
                pipeline: p,
                bind: draw_bind,
                dadaspec_pipeline: dada_p,
                dadaspec_bind: dada_bind,
                buffer: buffer(
                    file,
                    wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                ),
                front_data: if front {
                    Some(
                        assets[&format!("{file}.bin")]
                            .chunks_exact(4)
                            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                            .collect(),
                    )
                } else {
                    None
                },
                per_band: if mode == 5 { 2 } else { 1 },
                first: d["first"].as_u64().unwrap() as u32,
                count: d["count"].as_u64().unwrap() as u32,
            });
        }
        let depth = create_attachment(
            &device,
            "spectrum depth attachment",
            width,
            height,
            msaa_samples,
            wgpu::TextureFormat::Depth24Plus,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let msaa = Some(create_attachment(
            &device,
            "spectrum msaa attachment",
            width,
            height,
            msaa_samples,
            format,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        ));
        let blit_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("spectrum-blit-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spectrum-blit-bind-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let blit_pipeline = build_blit_pipeline(&device, &blit_layout, format);
        let queries = timestamp.then(|| {
            device.create_query_set(&wgpu::QuerySetDescriptor {
                label: None,
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            })
        });
        let resolve = timestamp.then(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let read = timestamp.then(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 16,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        let uniform_layout: Value = serde_json::from_slice(&assets["uniforms.json"]).unwrap();
        let sub = scene["sub"].as_u64().unwrap() as usize;
        let rate = scene["rate"].as_u64().unwrap() as u32;
        let mut live = spectrum_core::Analysis::new(48000, rate, sub);
        let mut history_cpu = spectrum_core::History::new(spectrum_core::band_count(sub), 4096);
        let initial_rows = scene["rows"].as_u64().unwrap() as usize;
        let mut signal = spectrum_core::TestSignal::new(48000);
        let mut display = spectrum_core::DisplaySpectrum::new(spectrum_core::band_count(sub));
        for row in 0..initial_rows {
            let end = (row + 1) * 48000 / rate as usize;
            let mut pcm = vec![0.0; end - live.written];
            signal.fill(&mut pcm);
            live.receive(&pcm);
            let db = live.analyse(end);
            display.update(db, 1.0 / rate as f64);
            let bytes: Vec<_> = db
                .iter()
                .map(|v| spectrum_core::db_to_byte(*v as f64))
                .collect();
            history_cpu.push(&bytes);
        }
        let mut row_clock = spectrum_core::RowClock::new(48000, rate);
        row_clock.next = live.written as f64 + 48000.0 / rate as f64;
        row_clock.clock = live.written as f64;
        let sample_clock = live.written as f64;
        let read_base_f32 = |offset: usize| {
            f32::from_le_bytes(uniform_buffers[0].1[offset..offset + 4].try_into().unwrap())
        };
        let snapshot_scale = ["u_viewport", "u_dpr", "u_keyGapPx", "u_keyHeightPx"]
            .into_iter()
            .map(|k| uniform_layout["fields"].get(k)?.get("offset")?.as_u64().map(|v| v as usize))
            .collect::<Option<Vec<_>>>()
            .map(|offs| SnapshotScaleOffsets {
                viewport: offs[0],
                dpr: offs[1],
                key_gap: offs[2],
                key_height: offs[3],
                base_dpr: read_base_f32(offs[1]),
                base_gap: read_base_f32(offs[2]),
                base_key_height: read_base_f32(offs[3]),
            });
        let crossovers = assets
            .get("look-dadaspec.json")
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
            .map(|v| {
                [
                    v["crossovers"][0].as_f64().unwrap_or(200.0),
                    v["crossovers"][1].as_f64().unwrap_or(1500.0),
                ]
            })
            .unwrap_or([200.0, 1500.0]);
        let mut chroma = spectrum_core::DadaspecChroma::new(sub, crossovers);
        if initial_rows > 0 {
            chroma.sync_from_history(&history_cpu, sub, rate, -96.0);
        }
        Self {
            history_texture,
            chroma_texture,
            uniform_buffers,
            newest_offset: uniform_layout["fields"]["u_newest"]["offset"]
                .as_u64()
                .unwrap() as usize,
            phase_offset: uniform_layout["fields"]["u_phase"]["offset"]
                .as_u64()
                .unwrap() as usize,
            snapshot_scale,
            live,
            history: history_cpu,
            chroma,
            row_clock,
            signal,
            display,
            sample_clock,
            phase: 0.0,
            scene: None,
            external_input: false,
            tracker: Default::default(),
            format,
            msaa_samples,
            render_scale: 1.0,
            palette_texture,
            current_palette: String::new(),
            built_geometry: (0, 0, 0, false),
            palettes: assets
                .iter()
                .filter(|(k, _)| k.starts_with("palette-") && k.ends_with(".rgba"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            bind_layout: layout,
            pipeline_layout,
            shaders,
            dadaspec_shaders,
            blit_layout,
            blit_sampler,
            blit_pipeline,
            history_sampler: sampler,
            palette_sampler,
            device,
            queue,
            mesh,
            indices,
            pipeline,
            bind: surface_bind,
            dadaspec_pipeline,
            dadaspec_bind,
            draws,
            count: scene["indexCount"].as_u64().unwrap() as u32,
            width,
            height,
            stage: None,
            depth: Some(depth),
            msaa,
            scaled: None,
            queries,
            resolve,
            read,
            timing_async: false,
            timing_pending: std::cell::Cell::new(false),
            timing_ready: Default::default(),
            gpu_ms: std::cell::Cell::new(None),
            #[cfg(not(target_arch = "wasm32"))]
            submitted_at: std::cell::Cell::new(None),
            adapter: adapter_name,
        }
    }
    /// Builds its own camera, uniform blocks, meshes, guides and piano geometry.
    /// No browser snapshot or browser process is used.
    pub async fn new_live(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
        format: wgpu::TextureFormat,
        assets: &scene::Assets,
        width: u32,
        height: u32,
    ) -> Self {
        let scene = scene::Scene::new(assets, width, height);
        let generated = scene.asset_scene(assets);
        let mut gpu = Self::new(instance, surface, format, &generated).await;
        gpu.current_palette = scene.settings.palette.clone();
        gpu.built_geometry = scene.geometry_key();
        gpu.scene = Some(scene);
        gpu
    }
    pub fn set_external_input(&mut self, external: bool) {
        self.external_input = external;
    }
    /// Called on display frames without newly drained audio rows as well.
    pub fn set_phase(&mut self, phase: f64) {
        self.sync_scene(0.0);
        if !phase.is_finite() || self.scene.as_ref().is_some_and(|s| s.settings.hold) {
            return;
        }
        self.phase = phase.clamp(0.0, 1.0);
        self.update_uniforms();
    }
    /// Synchronous timestamp readback is deliberately restricted to explicit benchmarks.
    pub fn set_timing_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.queries = None;
            self.resolve = None;
            self.read = None;
            return;
        }
        if self.queries.is_some()
            || !self
                .device
                .features()
                .contains(wgpu::Features::TIMESTAMP_QUERY)
        {
            return;
        }
        self.queries = Some(self.device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("benchmark timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        }));
        self.resolve = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("benchmark timestamp resolve"),
            size: 16,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));
        self.read = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("benchmark timestamp readback"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));
    }
    /// Timestamps for a live readout: `render` then never blocks, and returns the most
    /// recent pass time it has. Off returns the device to no timestamp queries at all.
    pub fn set_async_timing(&mut self, enabled: bool) {
        self.set_timing_enabled(false);
        self.timing_async = enabled;
        self.timing_pending.set(false);
        self.timing_ready = Default::default();
        self.gpu_ms.set(None);
        if enabled {
            self.set_timing_enabled(true);
        }
    }
    pub fn async_timing(&self) -> bool {
        self.timing_async
    }
    /// The latest pass time the asynchronous readout has collected.
    pub fn gpu_ms(&self) -> Option<f64> {
        self.gpu_ms.get()
    }
    /// Whether this device can time a pass at all (the timestamp-query feature).
    pub fn timing_available(&self) -> bool {
        self.queries.is_some()
    }
    pub fn is_dadaspec(&self) -> bool {
        self.dadaspec_pipeline.is_some() && self.scene.as_ref().is_some_and(|s| s.is_dadaspec())
    }
    /// Vertices and draw calls of the spectrum pass as it is drawn now.
    pub fn draw_stats(&self) -> (u64, u32) {
        let (mut vertices, mut calls) = (0, 0);
        if self.scene.as_ref().is_none_or(|s| s.settings.waterfall) {
            vertices += self.mesh.size() / 12;
            calls += 1;
        }
        for d in &self.draws {
            if d.count == 0 || self.scene.as_ref().is_some_and(|s| !s.draw_visible(&d.tag)) {
                continue;
            }
            vertices += d.count as u64;
            calls += 1;
        }
        (vertices, calls)
    }
    pub fn clear_maximum(&mut self) {
        self.display.clear_maximum();
        self.refresh_front();
    }
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.width = width;
        self.height = height;
        self.stage = None;
        if let Some(scene) = &mut self.scene {
            scene.width = width;
            scene.height = height;
            scene.update(0.0);
        }
        self.rebuild_window_attachments();
        self.sync_scaled_target();
        self.update_uniforms();
    }
    /// Draws the spectrum into this part of the target only, in pixels. The camera's
    /// aspect, the key strip and the labels follow its size. A part that is the whole
    /// target, empty, or not inside it draws the whole target.
    pub fn set_stage(&mut self, x: u32, y: u32, width: u32, height: u32) {
        let whole = x == 0 && y == 0 && width == self.width && height == self.height;
        let fits = width > 0 && height > 0 && x + width <= self.width && y + height <= self.height;
        let stage = (fits && !whole).then_some([x, y, width, height]);
        if stage == self.stage {
            return;
        }
        self.stage = stage;
        let [_, _, w, h] = stage.unwrap_or([0, 0, self.width, self.height]);
        if let Some(scene) = &mut self.scene {
            scene.width = w;
            scene.height = h;
            scene.update(0.0);
        }
        self.sync_scaled_target();
        self.update_uniforms();
    }
    /// The part of the target the spectrum is drawn in: x, y, width, height in pixels.
    pub fn stage(&self) -> [u32; 4] {
        self.stage.unwrap_or([0, 0, self.width, self.height])
    }
    /// Active MSAA sample count (`1` when off, `4` when 4x MSAA).
    pub fn msaa_samples(&self) -> u32 {
        self.msaa_samples
    }
    /// Sets MSAA sample count (`0` or `1` -> `1`, `4` -> `4`) and rebuilds pipelines/attachments.
    pub fn set_msaa(&mut self, samples: u32) {
        let samples = normalize_msaa(samples);
        if samples == self.msaa_samples {
            return;
        }
        self.msaa_samples = samples;
        self.rebuild_pipelines();
        self.rebuild_window_attachments();
        self.sync_scaled_target();
    }
    /// Active offscreen render scale factor (`1.0` uses the direct target pass).
    pub fn render_scale(&self) -> f32 {
        self.render_scale
    }
    /// Sets offscreen render scale (`< 0.999` renders to a scaled offscreen target then linear-blits).
    pub fn set_render_scale(&mut self, scale: f32) {
        let scale = normalize_render_scale(scale);
        if (self.render_scale - scale).abs() < 1e-6 {
            return;
        }
        self.render_scale = scale;
        if (scale < 0.999) == self.depth.is_some() {
            self.rebuild_window_attachments();
        }
        self.sync_scaled_target();
        self.update_uniforms();
    }
    /// Pixel dimensions `[rw, rh]` used by the 3D spectrum pass (`scaled_color` when `< 1.0`).
    pub fn render_dimensions(&self) -> [u32; 2] {
        let [_, _, sw, sh] = self.stage();
        scaled_dimensions(sw, sh, self.render_scale)
    }
    /// Whether the window-sized depth and MSAA targets are held (only at full scale).
    pub fn holds_window_attachments(&self) -> bool {
        self.depth.is_some()
    }
    fn rebuild_window_attachments(&mut self) {
        // Offscreen scaling draws into its own smaller targets; holding window-sized ones as
        // well would cost about 265 MB at 4K with 4× MSAA, much for a shared-memory GPU.
        if self.render_scale < 0.999 {
            self.depth = None;
            self.msaa = None;
            return;
        }
        self.depth = Some(create_attachment(
            &self.device,
            "spectrum depth attachment",
            self.width,
            self.height,
            self.msaa_samples,
            wgpu::TextureFormat::Depth24Plus,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        ));
        self.msaa = (self.msaa_samples > 1).then(|| {
            create_attachment(
                &self.device,
                "spectrum msaa attachment",
                self.width,
                self.height,
                self.msaa_samples,
                self.format,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
        });
    }
    fn rebuild_pipelines(&mut self) {
        self.pipeline = build_spectrum_pipeline(
            &self.device,
            &self.pipeline_layout,
            self.format,
            self.msaa_samples,
            "terrain",
            &self.shaders.terrain_vs,
            &self.shaders.terrain_fs,
            wgpu::PrimitiveTopology::TriangleList,
            None,
            true,
            None,
        );
        self.dadaspec_pipeline = self.dadaspec_shaders.as_ref().map(|ds| {
            build_spectrum_pipeline(
                &self.device,
                &self.pipeline_layout,
                self.format,
                self.msaa_samples,
                "dadaspec-terrain",
                &ds.terrain_vs,
                &ds.terrain_fs,
                wgpu::PrimitiveTopology::TriangleList,
                None,
                true,
                None,
            )
        });
        for d in &mut self.draws {
            let (vs, fs) = match d.kind {
                DrawKind::Keys => (&self.shaders.keys_vs, &self.shaders.keys_fs),
                DrawKind::Front => (&self.shaders.front_vs, &self.shaders.front_fs),
                DrawKind::Line => (&self.shaders.line_vs, &self.shaders.line_fs),
            };
            d.pipeline = build_spectrum_pipeline(
                &self.device,
                &self.pipeline_layout,
                self.format,
                self.msaa_samples,
                "overlay",
                vs,
                fs,
                d.topology,
                Some(d.blend),
                false,
                Some(d.kind),
            );
            d.dadaspec_pipeline = self.dadaspec_shaders.as_ref().map(|ds| {
                let (dvs, dfs) = match d.kind {
                    DrawKind::Keys => (&ds.keys_vs, &ds.keys_fs),
                    DrawKind::Front => (&ds.front_vs, &ds.front_fs),
                    DrawKind::Line => (&ds.line_vs, &ds.line_fs),
                };
                build_spectrum_pipeline(
                    &self.device,
                    &self.pipeline_layout,
                    self.format,
                    self.msaa_samples,
                    "dadaspec-overlay",
                    dvs,
                    dfs,
                    d.topology,
                    Some(d.blend),
                    false,
                    Some(d.kind),
                )
            });
        }
    }
    fn sync_scaled_target(&mut self) {
        if self.render_scale >= 0.999 {
            self.scaled = None;
            return;
        }
        let [rw, rh] = self.render_dimensions();
        if let Some(s) = &self.scaled {
            if s.width == rw && s.height == rh && s.msaa_samples == self.msaa_samples {
                return;
            }
        }
        let color = create_attachment(
            &self.device,
            "spectrum scaled color",
            rw,
            rh,
            1,
            self.format,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let msaa = (self.msaa_samples > 1).then(|| {
            create_attachment(
                &self.device,
                "spectrum scaled msaa",
                rw,
                rh,
                self.msaa_samples,
                self.format,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
        });
        let depth = create_attachment(
            &self.device,
            "spectrum scaled depth",
            rw,
            rh,
            self.msaa_samples,
            wgpu::TextureFormat::Depth24Plus,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let blit_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spectrum-blit-bind"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.blit_sampler),
                },
            ],
        });
        self.scaled = Some(ScaledTarget {
            width: rw,
            height: rh,
            msaa_samples: self.msaa_samples,
            color,
            msaa,
            depth,
            blit_bind,
        });
    }
    fn rebind(&mut self) {
        let history = self.history_texture.create_view(&Default::default());
        let palette = self.palette_texture.create_view(&Default::default());
        let chroma = self.chroma_texture.create_view(&Default::default());
        let has_dadaspec = self.dadaspec_pipeline.is_some();
        let groups: Vec<_> = self
            .uniform_buffers
            .iter()
            .map(|(buffer, _)| {
                let g = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("live scene bindings"),
                    layout: &self.bind_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&history),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.history_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(&palette),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::Sampler(&self.palette_sampler),
                        },
                    ],
                });
                let dg = has_dadaspec.then(|| {
                    self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("live dadaspec bindings"),
                        layout: &self.bind_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: buffer.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(&history),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::Sampler(&self.history_sampler),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::TextureView(&chroma),
                            },
                            wgpu::BindGroupEntry {
                                binding: 4,
                                resource: wgpu::BindingResource::Sampler(&self.history_sampler),
                            },
                        ],
                    })
                });
                (g, dg)
            })
            .collect();
        self.bind = groups[0].0.clone();
        self.dadaspec_bind = groups[0].1.clone();
        for d in &mut self.draws {
            d.bind = groups[d.uniform_index].0.clone();
            d.dadaspec_bind = groups[d.uniform_index].1.clone();
        }
    }
    fn rebuild_scene(&mut self) {
        let Some(scene) = &self.scene else {
            return;
        };
        let sub = scene.settings.sub;
        let row_rate = scene.settings.row_rate;
        let crossovers = scene.crossovers();
        let floor = scene.db_floor() as f64;
        let is_dada = self.dadaspec_pipeline.is_some() && scene.is_dadaspec();
        let bands = spectrum_core::band_count(sub);
        let mut needs_rebind = false;
        let history_reset = self.live.sub != sub || self.live.row_rate != row_rate;
        if history_reset {
            self.live = spectrum_core::Analysis::new(48000, row_rate, sub);
            self.history = spectrum_core::History::new(bands, 4096);
            self.chroma = spectrum_core::DadaspecChroma::new(sub, crossovers);
            self.chroma.clear();
            self.display = spectrum_core::DisplaySpectrum::new(bands);
            self.tracker.clear();
            self.row_clock = spectrum_core::RowClock::new(48000, row_rate);
            self.sample_clock = 0.0;
            self.signal = spectrum_core::TestSignal::new(48000);
            self.phase = 0.0;
            self.history_texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("native history pyramid"),
                size: wgpu::Extent3d {
                    width: bands as u32 * 6,
                    height: 4096,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            needs_rebind = true;
        }
        let buffer = |label, data: &[u8], usage| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: data,
                    usage,
                })
        };
        self.mesh = buffer(
            "native terrain",
            bytemuck::cast_slice(&scene.mesh),
            wgpu::BufferUsages::VERTEX,
        );
        self.indices = buffer(
            "native indices",
            bytemuck::cast_slice(&scene.indices),
            wgpu::BufferUsages::INDEX,
        );
        self.count = scene.indices.len() as u32;
        for draw in &mut self.draws {
            let data = if draw.front_data.is_some() {
                let values = match draw.tag.as_str() {
                    "peak" => &self.display.peak,
                    "maximum" => &self.display.maximum,
                    _ => &self.display.shown,
                };
                scene.front(draw.per_band, values)
            } else if draw.tag == "keys" {
                scene.keys.clone()
            } else {
                scene.guides.clone()
            };
            draw.buffer = buffer(
                "native overlay",
                bytemuck::cast_slice(&data),
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            );
            if draw.front_data.is_some() {
                draw.front_data = Some(data);
                draw.first = 0;
                draw.count = (bands * draw.per_band) as u32;
            } else if draw.tag == "keys" {
                draw.first = 0;
                draw.count = 127 * 6;
            } else if let Some(g) = scene.slices.iter().find(|g| g.name == draw.tag) {
                draw.first = g.first;
                draw.count = g.count;
            }
        }

        if needs_rebind {
            self.rebind();
        }
        let mut live_chroma = [0.0f32; 4];
        let mut flush = history_reset;
        if !history_reset && is_dada {
            self.chroma.sync_from_history(&self.history, sub, row_rate, floor);
            live_chroma = self.chroma.live_chroma;
            flush = true;
        }
        if let Some(scene) = &mut self.scene {
            scene.live_chroma = live_chroma;
            scene.guides_dirty = false;
            self.built_geometry = scene.geometry_key();
        }
        if flush {
            self.flush_chroma();
        }
    }
    /// The frequency axis is gliding: only the guide lines moved, the mesh did not.
    fn rebuild_guides(&mut self) {
        let Some(scene) = &mut self.scene else {
            return;
        };
        scene.guides_dirty = false;
        for draw in &mut self.draws {
            if draw.front_data.is_some() || draw.tag == "keys" {
                continue;
            }
            draw.buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("native guides"),
                    contents: bytemuck::cast_slice(&scene.guides),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                });
            if let Some(g) = scene.slices.iter().find(|g| g.name == draw.tag) {
                draw.first = g.first;
                draw.count = g.count;
            }
        }
    }
    /// Piano keys under the labelled main peaks light up.
    fn update_keys(&mut self) {
        if let Some(scene) = &mut self.scene {
            if scene.light_keys() {
                if let Some(keys) = self.draws.iter().find(|d| d.tag == "keys") {
                    self.queue
                        .write_buffer(&keys.buffer, 0, bytemuck::cast_slice(&scene.keys));
                }
            }
        }
    }
    fn update_uniforms(&mut self) {
        let dims = self.render_dimensions();
        let scale = if self.render_scale < 0.999 {
            self.render_scale
        } else {
            1.0
        };
        if let Some(scene) = &self.scene {
            let names = std::iter::once("terrain".to_owned()).chain(scene.draw_names());
            for ((buffer, bytes), name) in self.uniform_buffers.iter_mut().zip(names) {
                *bytes = scene.uniforms_scaled(&name, self.history.written, self.phase, dims, scale);
                self.queue.write_buffer(buffer, 0, bytes);
            }
        } else {
            for (buffer, bytes) in &mut self.uniform_buffers {
                bytes[self.newest_offset..self.newest_offset + 4].copy_from_slice(
                    &(((self.history.written + 131071) % 131072) as f32).to_le_bytes(),
                );
                bytes[self.phase_offset..self.phase_offset + 4]
                    .copy_from_slice(&(self.phase as f32).to_le_bytes());
                if let Some(s) = &self.snapshot_scale {
                    bytes[s.viewport..s.viewport + 4]
                        .copy_from_slice(&(dims[0] as f32).to_le_bytes());
                    bytes[s.viewport + 4..s.viewport + 8]
                        .copy_from_slice(&(dims[1] as f32).to_le_bytes());
                    bytes[s.dpr..s.dpr + 4]
                        .copy_from_slice(&(s.base_dpr * scale).to_le_bytes());
                    bytes[s.key_gap..s.key_gap + 4]
                        .copy_from_slice(&(s.base_gap * scale).to_le_bytes());
                    bytes[s.key_height..s.key_height + 4]
                        .copy_from_slice(&(s.base_key_height * scale).to_le_bytes());
                }
                self.queue.write_buffer(buffer, 0, bytes);
            }
        }
    }
    fn flush_history(&mut self, before: usize) {
        for l in 0..6 {
            let count = (self.history.written / (1 << l) - before / (1 << l)).min(4096);
            if count == 0 {
                continue;
            }
            let end = self.history.written / (1 << l) % 4096;
            let start = (end + 4096 - count) % 4096;
            let first = count.min(4096 - start);
            for (row, n) in [(start, first), (0, count - first)] {
                if n == 0 {
                    continue;
                }
                let c = self.history.count;
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.history_texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: (l * c) as u32,
                            y: row as u32,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &self.history.levels[l][row * c..(row + n) * c],
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(c as u32),
                        rows_per_image: Some(n as u32),
                    },
                    wgpu::Extent3d {
                        width: c as u32,
                        height: n as u32,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
    }
    fn flush_chroma(&mut self) {
        if !self.chroma.dirty.iter().any(|&d| d > 0) {
            return;
        }
        let pushed = self.history.written;
        for l in 0..spectrum_core::LEVELS {
            let dirty = self.chroma.dirty[l].min(spectrum_core::HISTORY_ROWS);
            if dirty == 0 {
                continue;
            }
            let end = (pushed / (1usize << l)) % spectrum_core::HISTORY_ROWS;
            let start = (end + spectrum_core::HISTORY_ROWS - dirty) % spectrum_core::HISTORY_ROWS;
            let first = dirty.min(spectrum_core::HISTORY_ROWS - start);
            let buf = &self.chroma.levels[l];
            for (row, n) in [(start, first), (0, dirty - first)] {
                if n == 0 {
                    continue;
                }
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.chroma_texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: l as u32,
                            y: row as u32,
                            z: 0,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &buf[row * 4..(row + n) * 4],
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(4),
                        rows_per_image: Some(n as u32),
                    },
                    wgpu::Extent3d {
                        width: 1,
                        height: n as u32,
                        depth_or_array_layers: 1,
                    },
                );
            }
            self.chroma.dirty[l] = 0;
        }
    }
    fn update_front(&mut self, shown: &[f32], peaks: &[f32], maxima: &[f32]) {
        for draw in &mut self.draws {
            if let Some(data) = &mut draw.front_data {
                let db = match draw.tag.as_str() {
                    "peak" => peaks,
                    "maximum" => maxima,
                    _ => shown,
                };
                let mut changed = false;
                for (index, vertex) in data.chunks_exact_mut(3).enumerate() {
                    let value = *db.get(index / draw.per_band).unwrap_or(&-120.0);
                    if vertex[1] != value {
                        vertex[1] = value;
                        changed = true;
                    }
                }
                if changed {
                    self.queue
                        .write_buffer(&draw.buffer, 0, bytemuck::cast_slice(data));
                }
            }
        }
    }
    /// Accepts analysis rows from an audio worker. Display data is updated independently
    /// from history so the renderer never controls the analysis sample clock.
    pub fn submit_rows(
        &mut self,
        rows: &[Vec<u8>],
        shown: &[f32],
        peaks: &[f32],
        maxima: &[f32],
        phase: f64,
    ) {
        self.sync_scene(0.0);
        if self.scene.as_ref().is_some_and(|s| s.settings.hold) {
            return;
        }
        let before = self.history.written;
        let dada = self.is_dadaspec();
        let row_rate = self.live.row_rate;
        let floor = self.scene.as_ref().map_or(-96.0, |s| s.db_floor() as f64);
        for row in rows {
            if row.len() == self.history.count {
                if dada {
                    self.chroma
                        .push_row(self.history.written, row, row_rate, floor);
                }
                self.history.push(row);
            }
        }
        if phase.is_finite() {
            self.phase = phase.clamp(0.0, 1.0);
        }
        self.flush_history(before);
        if dada {
            if let Some(scene) = &mut self.scene {
                scene.live_chroma = self.chroma.live_chroma;
            }
            self.flush_chroma();
        }
        if shown.len() == self.history.count
            && peaks.len() == self.history.count
            && maxima.len() == self.history.count
        {
            self.display.shown.copy_from_slice(shown);
            self.display.peak.copy_from_slice(peaks);
            self.display.maximum.copy_from_slice(maxima);
            self.update_front(shown, peaks, maxima);
        }
        self.update_uniforms();
    }
    /// Advances the camera and, for standalone/WASM comparison use, a test signal.
    /// Native audio normally calls set_external_input(true) and submits worker rows.
    fn sync_scene(&mut self, dt: f64) {
        let (palette, geom_key, guides_dirty, sub, row_rate, floor, crossovers) = {
            let Some(scene) = self.scene.as_mut() else {
                return;
            };
            scene.update(dt);
            (
                scene.settings.palette.clone(),
                scene.geometry_key(),
                scene.guides_dirty,
                scene.settings.sub,
                scene.settings.row_rate,
                scene.db_floor() as f64,
                scene.crossovers(),
            )
        };
        let geom_changed = geom_key != self.built_geometry;
        if self.current_palette != palette {
            if let Some(bytes) = self.palettes.get(&format!("palette-{palette}.rgba")) {
                self.set_palette_bytes(bytes);
            }
            if palette == "dadaspec" && self.dadaspec_pipeline.is_some() && !geom_changed {
                if self.chroma.count != spectrum_core::band_count(sub)
                    || self.chroma.crossovers != crossovers
                {
                    self.chroma = spectrum_core::DadaspecChroma::new(sub, crossovers);
                }
                self.chroma
                    .sync_from_history(&self.history, sub, row_rate, floor);
                if let Some(scene) = &mut self.scene {
                    scene.live_chroma = self.chroma.live_chroma;
                }
                self.flush_chroma();
            }
            self.current_palette = palette;
        }
        if geom_changed {
            self.rebuild_scene();
            self.refresh_front();
        } else if guides_dirty {
            self.rebuild_guides();
        }
    }
    fn refresh_front(&mut self) {
        let shown = self.display.shown.clone();
        let peaks = self.display.peak.clone();
        let maxima = self.display.maximum.clone();
        self.update_front(&shown, &peaks, &maxima);
    }
    pub fn advance(&mut self, dt: f64) {
        let dt = if dt.is_finite() {
            dt.clamp(0.0, 0.1)
        } else {
            0.0
        };
        self.sync_scene(dt);
        if !self.external_input {
            let hold = self.scene.as_ref().is_some_and(|s| s.settings.hold);
            self.sample_clock += dt.clamp(0.0, 0.1) * 48000.0;
            let end = self.sample_clock.floor() as usize;
            let mut pcm = vec![0.0; end.saturating_sub(self.live.written)];
            self.signal.fill(&mut pcm);
            self.live.receive(&pcm);
            let ends = self.row_clock.consume_samples(self.live.written as u64);
            if !hold {
                self.phase = self.row_clock.sample_phase(self.live.written as u64);
            }
            let ends = if hold { Vec::new() } else { ends };
            let before = self.history.written;
            let row_dt = 1.0 / self.live.row_rate as f64;
            let dada = self.is_dadaspec();
            let row_rate = self.live.row_rate;
            let floor = self.scene.as_ref().map_or(-96.0, |s| s.db_floor() as f64);
            for end in ends {
                let db = self.live.analyse(end);
                self.display.update(db, row_dt);
                let bytes: Vec<_> = db
                    .iter()
                    .map(|v| spectrum_core::db_to_byte(*v as f64))
                    .collect();
                if dada {
                    self.chroma
                        .push_row(self.history.written, &bytes, row_rate, floor);
                }
                self.history.push(&bytes);
            }
            self.flush_history(before);
            if dada {
                if let Some(scene) = &mut self.scene {
                    scene.live_chroma = self.chroma.live_chroma;
                }
                self.flush_chroma();
            }
            let (shown, peak, maximum) = (
                self.display.shown.clone(),
                self.display.peak.clone(),
                self.display.maximum.clone(),
            );
            self.update_front(&shown, &peak, &maximum);
            if !hold && dt > 0.0 {
                // Once a display frame, as on the web: the tracker's smoothing is per call.
                self.tracker.update(
                    &self.display.shown,
                    &self.display.live,
                    dt,
                    |band| self.live.peak_hz(band),
                    self.live.sub,
                );
                if let Some(scene) = &mut self.scene {
                    scene.tracks = self.tracker.tracks.clone();
                }
            }
        }
        self.update_keys();
        self.update_uniforms();
    }
    pub fn set_palette_bytes(&self, bytes: &[u8]) {
        assert_eq!(bytes.len(), 256 * 4, "exported palette size");
        self.queue.write_texture(
            self.palette_texture.as_image_copy(),
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1024),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 256,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
    }
    /// CPU time immediately after the spectrum queue submission, before timer polling.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn submitted_at(&self) -> Option<std::time::Instant> {
        self.submitted_at.get()
    }
    fn draw_spectrum<'a>(&'a self, p: &mut wgpu::RenderPass<'a>) {
        let dada = self.is_dadaspec();
        if self.scene.as_ref().is_none_or(|s| s.settings.waterfall) {
            let (pipe, bind) = if dada {
                (
                    self.dadaspec_pipeline.as_ref().unwrap_or(&self.pipeline),
                    self.dadaspec_bind.as_ref().unwrap_or(&self.bind),
                )
            } else {
                (&self.pipeline, &self.bind)
            };
            p.set_pipeline(pipe);
            p.set_bind_group(0, bind, &[]);
            p.set_vertex_buffer(0, self.mesh.slice(..));
            p.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
            p.draw_indexed(0..self.count, 0, 0..1);
        }
        for d in &self.draws {
            if d.count == 0 || self.scene.as_ref().is_some_and(|s| !s.draw_visible(&d.tag)) {
                continue;
            }
            let (pipe, bind) = if dada {
                (
                    d.dadaspec_pipeline.as_ref().unwrap_or(&d.pipeline),
                    d.dadaspec_bind.as_ref().unwrap_or(&d.bind),
                )
            } else {
                (&d.pipeline, &d.bind)
            };
            p.set_pipeline(pipe);
            p.set_bind_group(0, bind, &[]);
            p.set_vertex_buffer(0, d.buffer.slice(..));
            p.draw(d.first..d.first + d.count, 0..1);
        }
    }
    pub fn render(&self, target: &wgpu::TextureView) -> Option<f64> {
        let mut e = self.device.create_command_encoder(&Default::default());
        let timestamp_writes = self.queries.as_ref().map(|q| wgpu::RenderPassTimestampWrites {
            query_set: q,
            beginning_of_pass_write_index: Some(0),
            end_of_pass_write_index: Some(1),
        });
        if let Some(scaled) = &self.scaled {
            {
                let (view, resolve_target, store) = match &scaled.msaa {
                    Some(msaa) => (msaa, Some(&scaled.color), wgpu::StoreOp::Discard),
                    None => (&scaled.color, None, wgpu::StoreOp::Store),
                };
                let mut p = e.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("spectrum-scaled"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &scaled.depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes,
                    occlusion_query_set: None,
                });
                self.draw_spectrum(&mut p);
            }
            {
                let mut p = e.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("spectrum-blit"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                if let Some([x, y, w, h]) = self.stage {
                    p.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
                }
                p.set_pipeline(&self.blit_pipeline);
                p.set_bind_group(0, &scaled.blit_bind, &[]);
                p.draw(0..3, 0..1);
            }
        } else {
            let (view, resolve_target, store) = match &self.msaa {
                Some(msaa) => (msaa, Some(target), wgpu::StoreOp::Discard),
                None => (target, None, wgpu::StoreOp::Store),
            };
            let mut p = e.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("spectrum"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: self
                        .depth
                        .as_ref()
                        .expect("window attachments are held at full scale"),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes,
                occlusion_query_set: None,
            });
            if let Some([x, y, w, h]) = self.stage {
                p.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
            }
            self.draw_spectrum(&mut p);
        }
        // The readback buffer cannot be written while a map of it is outstanding.
        let copy = !(self.timing_async && self.timing_pending.get());
        if let (Some(q), Some(resolve), Some(read), true) =
            (&self.queries, &self.resolve, &self.read, copy)
        {
            e.resolve_query_set(q, 0..2, resolve, 0);
            e.copy_buffer_to_buffer(resolve, 0, read, 0, 16);
        }
        self.queue.submit([e.finish()]);
        #[cfg(not(target_arch = "wasm32"))]
        self.submitted_at.set(Some(std::time::Instant::now()));
        if let (Some(read), true) = (&self.read, self.timing_async) {
            use std::sync::atomic::Ordering;
            if !self.timing_pending.get() {
                let ready = self.timing_ready.clone();
                read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                    if r.is_ok() {
                        ready.store(true, Ordering::Release);
                    }
                });
                self.timing_pending.set(true);
            } else {
                let _ = self.device.poll(wgpu::PollType::Poll);
                if self.timing_ready.swap(false, Ordering::AcqRel) {
                    let data = read.slice(..).get_mapped_range();
                    let a = u64::from_le_bytes(data[..8].try_into().unwrap());
                    let b = u64::from_le_bytes(data[8..16].try_into().unwrap());
                    drop(data);
                    read.unmap();
                    self.timing_pending.set(false);
                    self.gpu_ms.set(Some(
                        b.saturating_sub(a) as f64 * self.queue.get_timestamp_period() as f64 / 1e6,
                    ));
                }
            }
            return self.gpu_ms.get();
        }
        if let Some(read) = &self.read {
            let slice = read.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
            rx.recv().unwrap().unwrap();
            let data = slice.get_mapped_range();
            let a = u64::from_le_bytes(data[..8].try_into().unwrap());
            let b = u64::from_le_bytes(data[8..16].try_into().unwrap());
            drop(data);
            read.unmap();
            return Some((b - a) as f64 * self.queue.get_timestamp_period() as f64 / 1e6);
        }
        None
    }
    pub fn save(&self, texture: &wgpu::Texture, path: &Path) {
        let (width, height) = (texture.width(), texture.height());
        let stride = (width * 4).div_ceil(256) * 256;
        let read = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (stride * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut e = self.device.create_command_encoder(&Default::default());
        e.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &read,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(height),
                },
            },
            texture.size(),
        );
        self.queue.submit([e.finish()]);
        let slice = read.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
        rx.recv().unwrap().unwrap();
        let data = slice.get_mapped_range();
        let mut image = format!("P6\n{} {}\n255\n", width, height).into_bytes();
        for y in 0..height {
            for x in 0..width {
                let i = (y * stride + x * 4) as usize;
                if matches!(
                    texture.format(),
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
                ) {
                    image.extend_from_slice(&[data[i + 2], data[i + 1], data[i]]);
                } else {
                    image.extend_from_slice(&data[i..i + 3]);
                }
            }
        }
        fs::write(path, image).unwrap();
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    fn live_gpu(width: u32, height: u32) -> Gpu {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut assets = scene::Assets::new();
        for dir in [
            "toolchains/spectrum-assets",
            "toolchains/shader-lab/tungsten",
        ] {
            for entry in
                fs::read_dir(root.join(dir)).expect("run export-shared and shader-migrate first")
            {
                let path = entry.unwrap().path();
                if path.is_file() {
                    assets.insert(
                        path.file_name().unwrap().to_string_lossy().into_owned(),
                        fs::read(path).unwrap(),
                    );
                }
            }
        }
        for entry in fs::read_dir(root.join("toolchains/shader-lab/dadaspec"))
            .expect("run export-shared and shader-migrate first")
        {
            let path = entry.unwrap().path();
            if path.is_file() {
                assets.insert(
                    format!("dadaspec/{}", path.file_name().unwrap().to_string_lossy()),
                    fs::read(path).unwrap(),
                );
            }
        }
        let instance = wgpu::Instance::new(&Default::default());
        pollster::block_on(Gpu::new_live(
            &instance,
            None,
            wgpu::TextureFormat::Rgba8Unorm,
            &assets,
            width,
            height,
        ))
    }
    #[test]
    #[ignore = "Requires an actual graphics adapter and generated shared/shader assets; run locally with --ignored"]
    fn resize_before_first_frame_keeps_saved_resolution() {
        // Windows and Linux deliver a Resized event before the first frame. The resize used
        // to swallow the scene's one-shot rebuild notice, leaving the default mesh in place.
        let mut g = live_gpu(640, 360);
        g.set_external_input(true);
        {
            // The highest resolution as the Windows report ran it: 1009 bands, 480 rows/s,
            // piano keys on (127 × 6 of the vertices).
            let s = g.scene.as_mut().unwrap();
            s.settings.sub = 8;
            s.settings.row_rate = 480;
            s.settings.keys = true;
        }
        g.resize(800, 450);
        g.advance(1.0 / 60.0);
        assert_eq!(g.draw_stats().0, 520537);
        assert_eq!(g.history.count, spectrum_core::band_count(8));
    }
    #[test]
    #[ignore = "Requires an actual graphics adapter and generated shared/shader assets; run locally with --ignored"]
    fn live_reconfiguration_preserves_display_and_freeze_then_rebinds() {
        let mut g = live_gpu(640, 360);
        g.set_external_input(true);
        // The default is four bands per semitone.
        let bands = g.history.count;
        assert_eq!(bands, spectrum_core::band_count(scene::DEFAULT_SUB));
        let shown = vec![-30.0; bands];
        let peaks = vec![-20.0; bands];
        let maxima = vec![-10.0; bands];
        g.submit_rows(
            &[vec![180; bands], vec![181; bands]],
            &shown,
            &peaks,
            &maxima,
            0.375,
        );
        {
            let s = g.scene.as_mut().unwrap();
            s.settings.hold = true;
            s.settings.seconds = 8.0;
            s.settings.low_level = true;
        }
        g.advance(1.0 / 60.0);
        assert_eq!(g.history.written, 2);
        assert_eq!(g.phase, 0.375);
        for (tag, expected) in [("trace", -30.0), ("peak", -20.0), ("maximum", -10.0)] {
            let d = g.draws.iter().find(|d| d.tag == tag).unwrap();
            assert!(d
                .front_data
                .as_ref()
                .unwrap()
                .chunks_exact(3)
                .all(|v| v[1] == expected));
        }
        g.resize(320, 640);
        let s = g.scene.as_ref().unwrap();
        assert_eq!(s.matrices.view_proj, s.camera.matrices(0.5).view_proj);
        {
            let s = g.scene.as_mut().unwrap();
            s.settings.hold = false;
            s.settings.sub = 8;
            s.settings.row_rate = 480;
        }
        let values = vec![-25.0; 1009];
        g.submit_rows(&[vec![190; 1009]], &values, &values, &values, 0.25);
        assert_eq!(g.history.count, 1009);
        assert_eq!(g.history.written, 1);
        assert_eq!(
            g.draws.iter().find(|d| d.tag == "trace").unwrap().count,
            1009
        );
        g.submit_rows(&[], &[], &[], &[], 0.5);
        assert_eq!(g.phase, 0.5);
        assert_eq!(g.display.shown, values);
        g.set_phase(f64::NAN);
        assert_eq!(g.phase, 0.5);
        g.scene.as_mut().unwrap().settings.hold = true;
        g.set_external_input(false);
        g.advance(1.0 / 60.0);
        assert_eq!(g.phase, 0.5);
        assert_eq!(g.history.written, 1);
        g.scene.as_mut().unwrap().settings.hold = false;
        g.advance(1.0 / 60.0);
        assert_eq!(g.history.written, 9);
        let texture = g.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("reconfigured test target"),
            size: wgpu::Extent3d {
                width: g.width,
                height: g.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        g.render(&texture.create_view(&Default::default()));
        g.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let bgra = g.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("BGRA readback regression"),
            size: wgpu::Extent3d {
                width: 2,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        g.queue.write_texture(
            bgra.as_image_copy(),
            &[7, 11, 251, 255, 90, 20, 1, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8),
                rows_per_image: Some(1),
            },
            bgra.size(),
        );
        let output =
            std::env::temp_dir().join(format!("spectrum-bgra-test-{}.ppm", std::process::id()));
        g.save(&bgra, &output);
        let saved = fs::read(&output).unwrap();
        fs::remove_file(output).unwrap();
        assert_eq!(&saved[..11], b"P6\n2 1\n255\n");
        assert_eq!(&saved[11..], &[251, 11, 7, 1, 20, 90]);
    }
    #[test]
    fn render_dimensions_and_msaa_normalization_follow_bounds() {
        assert_eq!(normalize_msaa(0), 1);
        assert_eq!(normalize_msaa(1), 1);
        assert_eq!(normalize_msaa(4), 4);
        assert_eq!(scaled_dimensions(1472, 936, 1.0), [1472, 936]);
        assert_eq!(scaled_dimensions(1472, 936, 0.85), [1251, 796]);
        assert_eq!(scaled_dimensions(1472, 936, 0.7), [1030, 655]);
        assert_eq!(scaled_dimensions(1472, 936, 0.5), [736, 468]);
        assert_eq!(scaled_dimensions(1, 1, 0.5), [1, 1]);
    }

    #[test]
    #[ignore = "Requires an actual graphics adapter and generated shared/shader assets; run locally with --ignored"]
    fn style_scale_and_msaa_switches_preserve_history_and_render_inside_stage() {
        let mut g = live_gpu(640, 360);
        for _ in 0..240 {
            g.advance(1.0 / 60.0);
        }
        g.set_external_input(true);
        g.set_stage(80, 30, 480, 280);
        let written = g.history.written;
        let history = g.history.levels.clone();
        let shown = g.display.shown.clone();
        let target = g.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("style/scale/MSAA regression"),
            size: wgpu::Extent3d { width: 640, height: 360, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let path = std::env::temp_dir().join(format!("spectrum-scale-{}.ppm", std::process::id()));
        for palette in ["tungsten", "dadaspec", "green", "dadaspec", "tungsten"] {
            g.scene.as_mut().unwrap().set_palette(palette);
            g.advance(0.0);
            assert_eq!(g.history.written, written);
            assert_eq!(g.history.levels, history);
            assert_eq!(g.display.shown, shown);
            if palette == "dadaspec" {
                assert!(g.chroma.levels[0].iter().any(|&v| v > 0));
                assert_eq!(g.scene.as_ref().unwrap().live_chroma, g.chroma.live_chroma);
            }
            let labels = g.scene.as_ref().unwrap().project_labels(480.0, 280.0);
            for (scale, msaa) in [(1.0, 4), (0.85, 1), (0.7, 4), (0.5, 1), (1.0, 4)] {
                g.set_msaa(msaa);
                g.set_render_scale(scale);
                // Window-sized targets are only held while drawing at full scale (F7).
                assert_eq!(g.holds_window_attachments(), scale >= 0.999);
                g.resize(640, 360);
                g.set_stage(80, 30, 480, 280);
                assert_eq!(g.holds_window_attachments(), scale >= 0.999);
                let after = g.scene.as_ref().unwrap().project_labels(480.0, 280.0);
                assert_eq!(after.len(), labels.len());
                for (before, after) in labels.iter().zip(&after) {
                    assert_eq!((before.x, before.y), (after.x, after.y));
                }
                g.render(&target.create_view(&Default::default()));
                g.save(&target, &path);
                let image = fs::read(&path).unwrap();
                let rgb = &image[b"P6\n640 360\n255\n".len()..];
                let mut lit = 0;
                for (i, p) in rgb.chunks_exact(3).enumerate() {
                    let (x, y) = (i % 640, i / 640);
                    if !(80..560).contains(&x) || !(30..310).contains(&y) {
                        assert_eq!(p, [0, 0, 0], "{palette}/{scale}/{msaa}: outside stage");
                    } else if p.iter().any(|&c| c > 8) {
                        lit += 1;
                    }
                }
                assert!(lit > 1000, "{palette}/{scale}/{msaa}: empty spectrum ({lit})");
            }
        }
        fs::remove_file(path).unwrap();
    }

    // Manual experiment only: ordering is deliberately confined to this test, since
    // the acceptance threshold must be measured on UHD 730 before changing production.
    #[test]
    #[ignore = "Manual GPU timing experiment; run alone with --ignored --nocapture"]
    fn terrain_order_gpu_experiment() {
        let mut g = live_gpu(1920, 1280);
        g.set_timing_enabled(true);
        {
            let s = g.scene.as_mut().unwrap();
            s.settings.sub = 8;
            s.settings.row_rate = 480;
            s.settings.keys = true;
        }
        for _ in 0..240 {
            g.advance(1.0 / 60.0);
        }
        g.set_external_input(true);
        let s = g.scene.as_ref().unwrap();
        let original = s.indices.clone();
        let mut triangles: Vec<_> = original.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
        let w = s.look["size"]["width"].as_f64().unwrap();
        let d = s.look["size"]["depth"].as_f64().unwrap();
        let span = (s.settings.seconds * s.settings.row_rate as f64).max(1.0);
        let back = s.matrices.back;
        // Approximate depth on the base plane. This measures triangle ordering,
        // without changing vertices, winding, fragment work or depth state.
        let depth = |t: &[u32; 3]| {
            t.iter().map(|&i| {
                let v = &s.mesh[i as usize * 3..];
                let x = (v[0] as f64 - 0.5) * w;
                let z = d * 0.5 - v[1] as f64 / span * d;
                x * back[0] + z * back[2]
            }).sum::<f64>() / 3.0
        };
        triangles.sort_by(|a, b| depth(b).total_cmp(&depth(a)));
        let ordered: Vec<u32> = triangles.into_iter().flatten().collect();
        let target = g.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain order timing target"),
            size: wgpu::Extent3d { width: g.width, height: g.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut rounds = Vec::new();
        for sorted in [false, true, true, false] {
            let data = if sorted { &ordered } else { &original };
            g.indices = g.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("experimental terrain indices"),
                contents: bytemuck::cast_slice(data),
                usage: wgpu::BufferUsages::INDEX,
            });
            for _ in 0..20 { g.render(&view); }
            let mut samples = Vec::new();
            for _ in 0..90 {
                samples.push(g.render(&view).expect("GPU timestamps are required for this experiment"));
            }
            let mean = samples.iter().sum::<f64>() / samples.len() as f64;
            samples.sort_by(f64::total_cmp);
            rounds.push(serde_json::json!({"ordered":sorted,"meanMs":mean,"medianMs":samples[45],"samplesMs":samples}));
        }
        println!("terrain-order: {}", serde_json::json!({"adapter":g.adapter,"pixels":[1920,1280],"sub":8,"rowRate":480,"msaa":4,"vertices":g.draw_stats().0,"staticSignal":"four-second reference chord","method":"ABBA, 20 warmup + 90 timed frames per round, approximate base-plane front-to-back order, sorting CPU cost excluded","rounds":rounds}));
    }
}
