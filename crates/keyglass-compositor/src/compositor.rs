//! Unified cross-platform `wgpu` layered GPU compositor.
//!
//! Renders the complete 7-stage Keyglass v2 pipeline identically on macOS (Metal),
//! Windows (DX12/Vulkan), Linux desktop (Vulkan/GLES), and RK3566 Mali-G52 (Vulkan/GLES):
//! 0. Background (`NV12` hardware video, `RGBA` image, or custom WGSL/GLSL shader module)
//! 1. Distortion Simulation (2D wave heightfield ping-pong in `Rgba16Float`)
//! 2. 3D Refraction & Specular Composite (bends live background video/shader via wave gradients)
//! 3. Under-Key GPU Effects (instanced Analog Depth/Velocity glows, classic rings/ripples, SDF bouncing/refracting particles)
//! 4. Key Legends & Keycap Rim Layer
//! 5. Top-Bar Module Tiles (4 slots)
//! 6. World Overlay (roaming companions like the Idle Pet)

use crate::canvas2d::{Canvas2D, DEFAULT_ANSI_LABELS};
use crate::particles::{KeySdfGrid, ParticleSystem};
use crate::shaders::{
    BLIT_RGBA_WGSL, DEFAULT_BG_WGSL, FS_TRIANGLE_VERT, KEY_GLOW_WGSL, NV12_TO_RGBA_WGSL,
    REFRACT_COMPOSITE_WGSL,
};
use crate::wave2d::{WaveField2D, WAVE2D_H, WAVE2D_W};
use bytemuck::{Pod, Zeroable};
use keyglass_bus::{KeyboardGeometry, DISPLAY_HEIGHT_PX, DISPLAY_WIDTH_PX};
use keyglass_modules::{FrameScene, KeyGlowShape};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Instant, SystemTime};

#[derive(Debug, Clone, Copy, Default)]
pub struct FrameTimings {
    pub physics_ms: f32,
    pub canvas2d_ms: f32,
    pub gpu_submit_ms: f32,
    pub total_ms: f32,
    pub particle_count: usize,
    pub glow_count: usize,
    pub wave_enabled: bool,
}

const WAVE_W: u32 = WAVE2D_W as u32;
const WAVE_H: u32 = WAVE2D_H as u32;
const MAX_GLOW_INSTANCES: usize = 2048;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CustomShaderUniforms {
    resolution: [f32; 2],
    time: f32,
    dt: f32,
    params: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RefractUniforms {
    texel: [f32; 2],
    refraction_px: f32,
    specular_strength: f32,
    resolution: [f32; 2],
    enabled: f32,
    bg_dimming: f32,
    tint: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ScreenUniform {
    resolution: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlowInstanceGpu {
    rect: [f32; 4],
    params: [f32; 4],
    color: [f32; 4],
}

struct CachedCustomPipeline {
    mtime: Option<SystemTime>,
    pipeline: wgpu::RenderPipeline,
    uses_scene_tex: bool,
}

#[derive(Debug, Clone)]
pub struct CompositorOptions {
    pub width: u32,
    pub height: u32,
    pub draw_default_legends: bool,
}

impl Default for CompositorOptions {
    fn default() -> Self {
        Self {
            width: DISPLAY_WIDTH_PX,
            height: DISPLAY_HEIGHT_PX,
            draw_default_legends: true,
        }
    }
}

pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_info: wgpu::AdapterInfo,
    width: u32,
    height: u32,
    draw_default_legends: bool,

    geometry: KeyboardGeometry,
    sdf_grid: KeySdfGrid,
    particles: ParticleSystem,
    wave: WaveField2D,

    linear_sampler: wgpu::Sampler,

    // Offscreen render targets (ping-pong pair for arbitrary chained module distortion shaders!)
    _scene_bg_tex: wgpu::Texture,
    scene_bg_view: wgpu::TextureView,
    _scene_alt_tex: wgpu::Texture,
    scene_alt_view: wgpu::TextureView,
    output_tex: wgpu::Texture,
    output_view: wgpu::TextureView,

    // Single-crest analytical 3D wave normal/height texture (384x216 RGBA8)
    wave_tex: wgpu::Texture,
    wave_view: wgpu::TextureView,

    // Background image / NV12 textures (tracks mtime so live `bg-stream` frames hot-reload!)
    bg_rgba_tex: Option<(String, Option<SystemTime>, wgpu::Texture, wgpu::TextureView)>,
    nv12_y_tex: Option<(u32, u32, wgpu::Texture, wgpu::TextureView)>,
    nv12_uv_tex: Option<(u32, u32, wgpu::Texture, wgpu::TextureView)>,

    // Overlay 2D RGBA texture (under-key 2D, legends, tiles, world overlay)
    overlay_tex: wgpu::Texture,
    overlay_view: wgpu::TextureView,
    canvas: Canvas2D,
    cached_under_key_draws: Vec<keyglass_modules::DrawCommand>,

    // Uniforms & instance storage
    custom_uniform_buf: wgpu::Buffer,
    custom_bgl: wgpu::BindGroupLayout,
    custom_stage_bgl: wgpu::BindGroupLayout,
    custom_bg: wgpu::BindGroup,
    custom_pipelines: HashMap<(String, bool), CachedCustomPipeline>,

    default_bg_pipeline: wgpu::RenderPipeline,
    blit_bgl: wgpu::BindGroupLayout,
    blit_pipeline: wgpu::RenderPipeline,
    alpha_blit_pipeline: wgpu::RenderPipeline,
    nv12_bgl: wgpu::BindGroupLayout,
    nv12_pipeline: wgpu::RenderPipeline,

    refract_uniform_buf: wgpu::Buffer,
    refract_bgl: wgpu::BindGroupLayout,
    refract_pipeline: wgpu::RenderPipeline,

    glow_screen_buf: wgpu::Buffer,
    glow_instance_buf: wgpu::Buffer,
    glow_bg: wgpu::BindGroup,
    glow_pipeline: wgpu::RenderPipeline,

    // Staging readback buffer for headless PNG/RGBA capture
    readback_buf: wgpu::Buffer,
    padded_bytes_per_row: u32,
}

impl Compositor {
    pub fn new_headless(
        geometry: KeyboardGeometry,
        font_bytes: Option<&[u8]>,
        opts: CompositorOptions,
    ) -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .or_else(|| {
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: true,
            }))
        })
        .ok_or_else(|| "no suitable wgpu GPU adapter found".to_string())?;

        let adapter_info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("keyglass-compositor"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .map_err(|e| format!("wgpu request_device failed: {e}"))?;

        Self::init_with_device(device, queue, adapter_info, geometry, font_bytes, opts)
    }

    pub fn adapter_summary(&self) -> String {
        format!(
            "{} ({:?}, {:?})",
            self.adapter_info.name, self.adapter_info.backend, self.adapter_info.device_type
        )
    }

    fn init_with_device(
        device: wgpu::Device,
        queue: wgpu::Queue,
        adapter_info: wgpu::AdapterInfo,
        geometry: KeyboardGeometry,
        font_bytes: Option<&[u8]>,
        opts: CompositorOptions,
    ) -> Result<Self, String> {
        let width = opts.width.max(64);
        let height = opts.height.max(64);
        let sdf_grid = KeySdfGrid::build(&geometry);
        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let make_rt = |label: &str, w: u32, h: u32, format: wgpu::TextureFormat, copy_src: bool| {
            let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST;
            if copy_src {
                usage |= wgpu::TextureUsages::COPY_SRC;
            }
            let tex = device.create_texture(&wgpu::TextureDescriptor {
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
                usage,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            (tex, view)
        };

        let (scene_bg_tex, scene_bg_view) = make_rt(
            "scene-bg",
            width,
            height,
            wgpu::TextureFormat::Rgba8Unorm,
            false,
        );
        let (scene_alt_tex, scene_alt_view) = make_rt(
            "scene-alt",
            width,
            height,
            wgpu::TextureFormat::Rgba8Unorm,
            false,
        );
        let (output_tex, output_view) = make_rt(
            "output",
            width,
            height,
            wgpu::TextureFormat::Rgba8Unorm,
            true,
        );
        let (wave_tex, wave_view) = make_rt(
            "wave-2d",
            WAVE_W,
            WAVE_H,
            wgpu::TextureFormat::Rgba8Unorm,
            false,
        );
        let (overlay_tex, overlay_view) = make_rt(
            "overlay-2d",
            DISPLAY_WIDTH_PX,
            DISPLAY_HEIGHT_PX,
            wgpu::TextureFormat::Rgba8Unorm,
            false,
        );

        let fs_vert_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fs-vert"),
            source: wgpu::ShaderSource::Wgsl(FS_TRIANGLE_VERT.into()),
        });

        // Custom / Default background shader pipeline
        let custom_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("custom-uniforms"),
            size: std::mem::size_of::<CustomShaderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let custom_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("custom-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let custom_stage_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("custom-stage-bgl"),
            entries: &[
                uniform_entry(0),
                tex_entry(1, true),
                sampler_entry(2),
            ],
        });
        let custom_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("custom-bg"),
            layout: &custom_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: custom_uniform_buf.as_entire_binding(),
            }],
        });
        let default_bg_pipeline = build_fs_pipeline(
            &device,
            "default-bg",
            &fs_vert_mod,
            DEFAULT_BG_WGSL,
            &[&custom_bgl],
            wgpu::TextureFormat::Rgba8Unorm,
            None,
        );

        // Blit pipelines (opaque & alpha-blended)
        let blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit-bgl"),
            entries: &[
                tex_entry(0, true),
                sampler_entry(1),
            ],
        });
        let blit_pipeline = build_fs_pipeline(
            &device,
            "blit-opaque",
            &fs_vert_mod,
            BLIT_RGBA_WGSL,
            &[&blit_bgl],
            wgpu::TextureFormat::Rgba8Unorm,
            None,
        );
        let alpha_blit_pipeline = build_fs_pipeline(
            &device,
            "blit-alpha",
            &fs_vert_mod,
            BLIT_RGBA_WGSL,
            &[&blit_bgl],
            wgpu::TextureFormat::Rgba8Unorm,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );

        // NV12 hardware video pipeline
        let nv12_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nv12-bgl"),
            entries: &[
                tex_entry(0, true),
                tex_entry(1, true),
                sampler_entry(2),
            ],
        });
        let nv12_pipeline = build_fs_pipeline(
            &device,
            "nv12-bg",
            &fs_vert_mod,
            NV12_TO_RGBA_WGSL,
            &[&nv12_bgl],
            wgpu::TextureFormat::Rgba8Unorm,
            None,
        );

        // Refraction composite pipeline
        let refract_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("refract-uniforms"),
            size: std::mem::size_of::<RefractUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let refract_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("refract-bgl"),
            entries: &[
                tex_entry(0, true),
                tex_entry(1, true),
                sampler_entry(2),
                uniform_entry(3),
            ],
        });
        let refract_pipeline = build_fs_pipeline(
            &device,
            "refract-composite",
            &fs_vert_mod,
            REFRACT_COMPOSITE_WGSL,
            &[&refract_bgl],
            wgpu::TextureFormat::Rgba8Unorm,
            None,
        );

        // Instanced Key Glow + Particle pipeline
        let glow_screen_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glow-screen"),
            size: std::mem::size_of::<ScreenUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let glow_instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glow-instances"),
            size: (std::mem::size_of::<GlowInstanceGpu>() * MAX_GLOW_INSTANCES) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let glow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glow-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let glow_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glow-bg"),
            layout: &glow_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: glow_screen_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: glow_instance_buf.as_entire_binding(),
                },
            ],
        });
        let glow_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("key-glow"),
            source: wgpu::ShaderSource::Wgsl(KEY_GLOW_WGSL.into()),
        });
        let glow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glow-layout"),
            bind_group_layouts: &[&glow_bgl],
            push_constant_ranges: &[],
        });
        let glow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glow-pipeline"),
            layout: Some(&glow_layout),
            vertex: wgpu::VertexState {
                module: &glow_mod,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &glow_mod,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Readback buffer aligned to 256-byte row pitch
        let unpadded = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded.div_ceil(align) * align;
        let readback_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded_bytes_per_row as u64) * (height as u64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            adapter_info,
            width,
            height,
            draw_default_legends: opts.draw_default_legends,
            geometry,
            sdf_grid,
            particles: ParticleSystem::default(),
            wave: WaveField2D::new(),
            linear_sampler,
            _scene_bg_tex: scene_bg_tex,
            scene_bg_view,
            _scene_alt_tex: scene_alt_tex,
            scene_alt_view,
            output_tex,
            output_view,
            wave_tex,
            wave_view,
            bg_rgba_tex: None,
            nv12_y_tex: None,
            nv12_uv_tex: None,
            overlay_tex,
            overlay_view,
            canvas: Canvas2D::new(DISPLAY_WIDTH_PX, DISPLAY_HEIGHT_PX, font_bytes),
            cached_under_key_draws: Vec::new(),
            custom_uniform_buf,
            custom_bgl,
            custom_stage_bgl,
            custom_bg,
            custom_pipelines: HashMap::new(),
            default_bg_pipeline,
            blit_bgl,
            blit_pipeline,
            alpha_blit_pipeline,
            nv12_bgl,
            nv12_pipeline,
            refract_uniform_buf,
            refract_bgl,
            refract_pipeline,
            glow_screen_buf,
            glow_instance_buf,
            glow_bg,
            glow_pipeline,
            readback_buf,
            padded_bytes_per_row,
        })
    }

    /// Uploads a hardware-decoded NV12 video frame (`Y` plane + interleaved `UV` plane) directly
    /// into GPU textures so all downstream distortion/refraction modules bend the live video!
    pub fn upload_nv12_background(
        &mut self,
        video_w: u32,
        video_h: u32,
        y_plane: &[u8],
        uv_plane: &[u8],
    ) {
        let recreate_y = self
            .nv12_y_tex
            .as_ref()
            .is_none_or(|(w, h, _, _)| *w != video_w || *h != video_h);
        if recreate_y {
            let y_tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("nv12-y"),
                size: wgpu::Extent3d {
                    width: video_w,
                    height: video_h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let y_view = y_tex.create_view(&Default::default());
            self.nv12_y_tex = Some((video_w, video_h, y_tex, y_view));

            let uv_w = (video_w / 2).max(1);
            let uv_h = (video_h / 2).max(1);
            let uv_tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("nv12-uv"),
                size: wgpu::Extent3d {
                    width: uv_w,
                    height: uv_h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rg8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let uv_view = uv_tex.create_view(&Default::default());
            self.nv12_uv_tex = Some((uv_w, uv_h, uv_tex, uv_view));
        }

        if let Some((_, _, ref y_tex, _)) = self.nv12_y_tex {
            self.queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: y_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                y_plane,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(video_w),
                    rows_per_image: Some(video_h),
                },
                wgpu::Extent3d {
                    width: video_w,
                    height: video_h,
                    depth_or_array_layers: 1,
                },
            );
        }
        if let Some((uv_w, uv_h, ref uv_tex, _)) = self.nv12_uv_tex {
            self.queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: uv_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                uv_plane,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(uv_w * 2),
                    rows_per_image: Some(uv_h),
                },
                wgpu::Extent3d {
                    width: uv_w,
                    height: uv_h,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    fn ensure_bg_image(&mut self, path: &str) {
        let lower = path.to_ascii_lowercase();
        let is_video = lower.ends_with(".mp4")
            || lower.ends_with(".mov")
            || lower.ends_with(".mkv")
            || lower.ends_with(".webm");
        let resolved_path: std::path::PathBuf = if is_video {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            path.hash(&mut h);
            let thumb = std::env::temp_dir().join(format!("keyglass_vid_poster_{:016x}.jpg", h.finish()));
            if !thumb.is_file() {
                let ff = ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "ffmpeg"]
                    .into_iter()
                    .find(|c| *c == "ffmpeg" || Path::new(c).is_file())
                    .unwrap_or("ffmpeg");
                let _ = std::process::Command::new(ff)
                    .args([
                        "-hide_banner",
                        "-loglevel",
                        "error",
                        "-ss",
                        "1.5",
                        "-i",
                        path,
                        "-frames:v",
                        "1",
                        "-q:v",
                        "3",
                        "-y",
                    ])
                    .arg(&thumb)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
            if thumb.is_file() {
                thumb
            } else {
                Path::new(path).to_path_buf()
            }
        } else {
            Path::new(path).to_path_buf()
        };

        let mtime = std::fs::metadata(&resolved_path).ok().and_then(|m| m.modified().ok());
        if self
            .bg_rgba_tex
            .as_ref()
            .is_some_and(|(cached, cached_mt, _, _)| cached == path && *cached_mt == mtime)
        {
            return;
        }
        let Ok(img) = image::open(&resolved_path) else {
            return;
        };
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        // Reuse existing texture if dimensions match (zero GPU allocation per streamed frame!)
        if let Some((ref mut cpath, ref mut cmt, ref tex, _)) = self.bg_rgba_tex {
            if tex.width() == w && tex.height() == h {
                self.queue.write_texture(
                    wgpu::ImageCopyTexture {
                        texture: tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &rgba,
                    wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(w * 4),
                        rows_per_image: Some(h),
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
                *cpath = path.to_string();
                *cmt = mtime;
                return;
            }
        }
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bg-image"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let view = tex.create_view(&Default::default());
        self.bg_rgba_tex = Some((path.to_string(), mtime, tex, view));
    }

    fn get_or_compile_custom_shader(
        &mut self,
        shader_path: &str,
        alpha_blend: bool,
    ) -> Option<&CachedCustomPipeline> {
        let key = (shader_path.to_string(), alpha_blend);
        let mtime = std::fs::metadata(shader_path).ok().and_then(|m| m.modified().ok());
        let needs_build = match self.custom_pipelines.get(&key) {
            None => true,
            Some(c) => c.mtime != mtime,
        };
        if needs_build {
            let source = std::fs::read_to_string(shader_path).ok()?;
            let lower = shader_path.to_ascii_lowercase();
            let is_glsl = lower.ends_with(".glsl") || lower.ends_with(".frag");

            // Universal Keyglass GLSL shaders implement `vec4 effect_main(vec2 uv)` and can
            // call `sample_scene(vec2 uv)` identically on Metal/Vulkan/DX12 AND RK3566 GLES2!
            let (wrapped_source, uses_scene_tex) = if is_glsl && source.contains("effect_main") {
                let wrapped = format!(
                    "#version 450\n\
                     layout(location = 0) in vec2 v_uv;\n\
                     layout(location = 0) out vec4 out_color;\n\
                     layout(set = 0, binding = 0) uniform KeyglassUniforms {{\n\
                         vec2 u_resolution;\n\
                         float u_time;\n\
                         float u_dt;\n\
                         vec4 u_params[4];\n\
                     }};\n\
                     layout(set = 0, binding = 1) uniform texture2D u_scene_tex;\n\
                     layout(set = 0, binding = 2) uniform sampler u_scene_samp;\n\
                     vec4 sample_scene(vec2 uv) {{\n\
                         return texture(sampler2D(u_scene_tex, u_scene_samp), clamp(uv, vec2(0.001), vec2(0.999)));\n\
                     }}\n\
                     {source}\n\
                     void main() {{\n\
                         out_color = effect_main(v_uv);\n\
                     }}\n"
                );
                (wrapped, true)
            } else {
                (source, false)
            };

            self.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let fs_vert = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("custom-vs"),
                source: wgpu::ShaderSource::Wgsl(FS_TRIANGLE_VERT.into()),
            });
            let fs_mod = if is_glsl {
                self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(shader_path),
                    source: wgpu::ShaderSource::Glsl {
                        shader: wrapped_source.into(),
                        stage: wgpu::naga::ShaderStage::Fragment,
                        defines: Default::default(),
                    },
                })
            } else {
                self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(shader_path),
                    source: wgpu::ShaderSource::Wgsl(wrapped_source.into()),
                })
            };
            let entry = if is_glsl { "main" } else { "fs_main" };
            let bgl = if uses_scene_tex {
                &self.custom_stage_bgl
            } else {
                &self.custom_bgl
            };
            let layout = self
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("custom-layout"),
                    bind_group_layouts: &[bgl],
                    push_constant_ranges: &[],
                });
            let pipeline = self
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(shader_path),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &fs_vert,
                        entry_point: Some("vs_main"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &fs_mod,
                        entry_point: Some(entry),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            blend: if alpha_blend {
                                Some(wgpu::BlendState::ALPHA_BLENDING)
                            } else {
                                None
                            },
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview: None,
                    cache: None,
                });
            let err = pollster::block_on(self.device.pop_error_scope());
            if let Some(e) = err {
                eprintln!("[keyglass-compositor] shader compile error for {shader_path}: {e}");
                return None;
            }
            self.custom_pipelines.insert(
                key.clone(),
                CachedCustomPipeline {
                    mtime,
                    pipeline,
                    uses_scene_tex,
                },
            );
        }
        self.custom_pipelines.get(&key)
    }

    fn make_custom_pass_uniform_buf(&self, scene: &FrameScene, params: [[f32; 4]; 4]) -> wgpu::Buffer {
        let u = CustomShaderUniforms {
            resolution: [self.width as f32, self.height as f32],
            time: scene.time_sec,
            dt: scene.dt_sec.max(0.001),
            params,
        };
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("custom-pass-u"),
            size: std::mem::size_of::<CustomShaderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&buf, 0, bytemuck::bytes_of(&u));
        buf
    }

    /// Executes the complete 7-stage GPU pipeline for `scene` and updates `output_tex`.
    pub fn render_scene(&mut self, scene: &FrameScene) -> FrameTimings {
        let t_start = Instant::now();

        // Spawn & step SDF-aware + particle-colliding physics.
        let t_phys = Instant::now();
        for sp in &scene.particle_spawns {
            self.particles.spawn(sp, &self.sdf_grid);
        }
        self.particles.step(scene.dt_sec.max(0.001), &self.sdf_grid, &self.geometry);
        let physics_ms = t_phys.elapsed().as_secs_f32() * 1000.0;

        // Prepare background media if requested (clear stale image if no media active).
        if let Some(ref bg_path) = scene.background_media {
            self.ensure_bg_image(bg_path);
        } else {
            self.bg_rgba_tex = None;
        }

        // Precompile ALL active module shaders across background, distortion, and under_key stages!
        for s in &scene.background_shaders {
            let _ = self.get_or_compile_custom_shader(&s.shader_path, false);
        }
        for s in &scene.distortion_shaders {
            let _ = self.get_or_compile_custom_shader(&s.shader_path, false);
        }
        for s in &scene.under_key_shaders {
            let _ = self.get_or_compile_custom_shader(&s.shader_path, true);
        }

        // Prepare default background uniforms
        let bg_uniforms = CustomShaderUniforms {
            resolution: [self.width as f32, self.height as f32],
            time: scene.time_sec,
            dt: scene.dt_sec.max(0.001),
            params: scene
                .background_shaders
                .first()
                .map(|s| s.params)
                .unwrap_or([[0.0; 4]; 4]),
        };
        self.queue
            .write_buffer(&self.custom_uniform_buf, 0, bytemuck::bytes_of(&bg_uniforms));

        // Step single-crest analytical 3D outgoing ripples & upload 384x216 normal/height map
        if scene.distortion_enabled {
            for imp in &scene.wave_impulses {
                self.wave.apply_impulse(imp);
            }
            self.wave.step_dt(scene.dt_sec.max(0.001), &scene.wave_params);
            self.queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &self.wave_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &self.wave.rgba,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(WAVE_W * 4),
                    rows_per_image: Some(WAVE_H),
                },
                wgpu::Extent3d {
                    width: WAVE_W,
                    height: WAVE_H,
                    depth_or_array_layers: 1,
                },
            );
        }

        let refract_u = RefractUniforms {
            texel: [1.0 / WAVE_W as f32, 1.0 / WAVE_H as f32],
            refraction_px: scene.wave_params.refraction_strength,
            specular_strength: scene.wave_params.specular_strength,
            resolution: [DISPLAY_WIDTH_PX as f32, DISPLAY_HEIGHT_PX as f32],
            enabled: if scene.distortion_enabled { 1.0 } else { 0.0 },
            bg_dimming: scene.bg_dimming,
            tint: [
                scene.wave_params.tint[0],
                scene.wave_params.tint[1],
                scene.wave_params.tint[2],
                scene.top_fade.clamp(0.0, 0.85),
            ],
        };
        self.queue
            .write_buffer(&self.refract_uniform_buf, 0, bytemuck::bytes_of(&refract_u));

        // Build instanced GPU glows + SDF particles
        let mut instances = Vec::with_capacity(
            (scene.key_glows.len() + self.particles.particles.len()).min(MAX_GLOW_INSTANCES),
        );
        for g in &scene.key_glows {
            if instances.len() >= MAX_GLOW_INSTANCES {
                break;
            }
            let shape_id = match g.shape {
                KeyGlowShape::Glow => 0.0,
                KeyGlowShape::Ring => 1.0,
                KeyGlowShape::Ripple => 2.0,
                KeyGlowShape::Solid => 3.0,
            };
            instances.push(GlowInstanceGpu {
                rect: [g.cx, g.cy, g.w, g.h],
                params: [shape_id, g.radius_px, g.thickness_px, g.intensity],
                color: g.color,
            });
        }
        for p in &self.particles.particles {
            if instances.len() >= MAX_GLOW_INSTANCES {
                break;
            }
            let dot = p.dot();
            instances.push(GlowInstanceGpu {
                rect: [dot.center_px[0], dot.center_px[1], 0.0, 0.0],
                params: [4.0, dot.radius_px, 0.0, 1.0],
                color: dot.color,
            });
        }
        let screen_u = ScreenUniform {
            resolution: [DISPLAY_WIDTH_PX as f32, DISPLAY_HEIGHT_PX as f32],
            _pad: [0.0; 2],
        };
        self.queue
            .write_buffer(&self.glow_screen_buf, 0, bytemuck::bytes_of(&screen_u));
        if !instances.is_empty() {
            self.queue.write_buffer(
                &self.glow_instance_buf,
                0,
                bytemuck::cast_slice(&instances),
            );
        }

        // Rasterize 2D overlay stages (reusing cached under_key_draws when `keys-renderer` calls `ctx:keep_layer()`!)
        let t_c2d = Instant::now();
        if !scene.keep_under_key_layer {
            self.cached_under_key_draws = scene.under_key_draws.clone();
        }
        self.canvas.clear();
        let active_under = if scene.keep_under_key_layer && !self.cached_under_key_draws.is_empty() {
            &self.cached_under_key_draws
        } else {
            &scene.under_key_draws
        };
        if !active_under.is_empty() {
            self.canvas.execute(active_under);
        }
        if scene.has_custom_keys_renderer {
            if !scene.has_custom_dock {
                self.canvas.draw_top_bar_dividers(&self.geometry);
            }
        } else if self.draw_default_legends {
            self.canvas
                .blit_cached_default_key_legends(&self.geometry, DEFAULT_ANSI_LABELS);
        }
        if !scene.tile_draws.is_empty() {
            self.canvas.execute(&scene.tile_draws);
        }
        if !scene.world_overlay_draws.is_empty() {
            self.canvas.execute(&scene.world_overlay_draws);
        }
        let canvas2d_ms = t_c2d.elapsed().as_secs_f32() * 1000.0;
        let t_gpu = Instant::now();

        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.overlay_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.canvas.rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(DISPLAY_WIDTH_PX * 4),
                rows_per_image: Some(DISPLAY_HEIGHT_PX),
            },
            wgpu::Extent3d {
                width: DISPLAY_WIDTH_PX,
                height: DISPLAY_HEIGHT_PX,
                depth_or_array_layers: 1,
            },
        );

        // Encode GPU passes
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("keyglass-frame"),
            });

        // Ping-pong index: `cur_is_alt == false` means live scene sits in `scene_bg_view`,
        // `cur_is_alt == true` means live scene sits in `scene_alt_view`.
        let mut cur_is_alt = false;

        // Stage 0: Background (NV12 video / RGBA image / fallback) -> scene_bg_view
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("stage-0-bg"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.scene_bg_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });

            if let (Some((_, _, _, ref yv)), Some((_, _, _, ref uvv))) =
                (&self.nv12_y_tex, &self.nv12_uv_tex)
            {
                let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("nv12-bg"),
                    layout: &self.nv12_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(yv),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(uvv),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                        },
                    ],
                });
                pass.set_pipeline(&self.nv12_pipeline);
                pass.set_bind_group(0, &bg, &[]);
                pass.draw(0..3, 0..1);
            } else if let Some((_, _, _, ref view)) = self.bg_rgba_tex {
                let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("bg-img"),
                    layout: &self.blit_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                        },
                    ],
                });
                pass.set_pipeline(&self.blit_pipeline);
                pass.set_bind_group(0, &bg, &[]);
                pass.draw(0..3, 0..1);
            } else {
                pass.set_pipeline(&self.default_bg_pipeline);
                pass.set_bind_group(0, &self.custom_bg, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        // Stage 0b: Execute ALL custom module `background` shaders in order (ping-ponged so later
        // shaders can even sample earlier background layers via `sample_scene(uv)`!)
        let has_media_bg = self.nv12_y_tex.is_some() || self.bg_rgba_tex.is_some();
        if !has_media_bg {
            for s in &scene.background_shaders {
                let key = (s.shader_path.clone(), false);
                let Some(cached) = self.custom_pipelines.get(&key) else { continue };
                let u_buf = self.make_custom_pass_uniform_buf(scene, s.params);
                let (src_view, dst_view) = if cur_is_alt {
                    (&self.scene_alt_view, &self.scene_bg_view)
                } else {
                    (&self.scene_bg_view, &self.scene_alt_view)
                };
                let bg = if cached.uses_scene_tex {
                    self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("custom-bg-stage"),
                        layout: &self.custom_stage_bgl,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: u_buf.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(src_view) },
                            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.linear_sampler) },
                        ],
                    })
                } else {
                    self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("custom-bg-legacy"),
                        layout: &self.custom_bgl,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: u_buf.as_entire_binding() },
                        ],
                    })
                };
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("stage-0b-custom-bg"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: dst_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&cached.pipeline);
                pass.set_bind_group(0, &bg, &[]);
                pass.draw(0..3, 0..1);
                cur_is_alt = !cur_is_alt;
            }
        }

        // Stage 1b: Execute ALL custom module `distortion` shaders in sequence (ping-ponging live video/bg!)
        for s in &scene.distortion_shaders {
            let key = (s.shader_path.clone(), false);
            let Some(cached) = self.custom_pipelines.get(&key) else { continue };
            let u_buf = self.make_custom_pass_uniform_buf(scene, s.params);
            let (src_view, dst_view) = if cur_is_alt {
                (&self.scene_alt_view, &self.scene_bg_view)
            } else {
                (&self.scene_bg_view, &self.scene_alt_view)
            };
            let bg = if cached.uses_scene_tex {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("custom-dist-stage"),
                    layout: &self.custom_stage_bgl,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: u_buf.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(src_view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.linear_sampler) },
                    ],
                })
            } else {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("custom-dist-legacy"),
                    layout: &self.custom_bgl,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: u_buf.as_entire_binding() },
                    ],
                })
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("stage-1b-custom-distortion"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dst_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&cached.pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
            cur_is_alt = !cur_is_alt;
        }

        // Stage 2 + 3 + 4..6: Water Refraction (with bg_dimming + top_fade) -> Custom Under-Key Shaders -> Glows & Particles -> 2D Overlay
        let final_scene_view = if cur_is_alt {
            &self.scene_alt_view
        } else {
            &self.scene_bg_view
        };
        let refract_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("refract-bg"),
            layout: &self.refract_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(final_scene_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.wave_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.refract_uniform_buf.as_entire_binding(),
                },
            ],
        });
        let overlay_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay-bg"),
            layout: &self.blit_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.overlay_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
            ],
        });

        // Prepare bind groups for custom `under_key` shaders before starting the final render pass
        let mut under_key_binds: Vec<((String, bool), wgpu::BindGroup)> = Vec::new();
        for s in &scene.under_key_shaders {
            let key = (s.shader_path.clone(), true);
            let Some(cached) = self.custom_pipelines.get(&key) else { continue };
            let u_buf = self.make_custom_pass_uniform_buf(scene, s.params);
            let bg = if cached.uses_scene_tex {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("custom-under-stage"),
                    layout: &self.custom_stage_bgl,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: u_buf.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(final_scene_view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.linear_sampler) },
                    ],
                })
            } else {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("custom-under-legacy"),
                    layout: &self.custom_bgl,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: u_buf.as_entire_binding() },
                    ],
                })
            };
            under_key_binds.push((key, bg));
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("stage-2-to-6-final"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.output_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });

            // Stage 2: Refracted & specular-lit background (plus bg_dimming & top-bar shade)
            pass.set_pipeline(&self.refract_pipeline);
            pass.set_bind_group(0, &refract_bg, &[]);
            pass.draw(0..3, 0..1);

            // Stage 2b: Custom module `under_key` shaders (alpha-blended cleanly under keys & glows!)
            for (key, bg) in &under_key_binds {
                if let Some(cached) = self.custom_pipelines.get(key) {
                    pass.set_pipeline(&cached.pipeline);
                    pass.set_bind_group(0, bg, &[]);
                    pass.draw(0..3, 0..1);
                }
            }

            // Stage 3: Instanced Key Glows + SDF Bouncing/Refracting Particles
            if !instances.is_empty() {
                pass.set_pipeline(&self.glow_pipeline);
                pass.set_bind_group(0, &self.glow_bg, &[]);
                pass.draw(0..6, 0..(instances.len() as u32));
            }

            // Stages 4..6: Key Legends, Top-Bar Tiles, World Overlay (Idle Pet)
            pass.set_pipeline(&self.alpha_blit_pipeline);
            pass.set_bind_group(0, &overlay_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        self.queue.submit(Some(encoder.finish()));
        let gpu_submit_ms = t_gpu.elapsed().as_secs_f32() * 1000.0;
        FrameTimings {
            physics_ms,
            canvas2d_ms,
            gpu_submit_ms,
            total_ms: t_start.elapsed().as_secs_f32() * 1000.0,
            particle_count: self.particles.particles.len(),
            glow_count: scene.key_glows.len(),
            wave_enabled: scene.distortion_enabled,
        }
    }

    /// Exposes the crisp 1920x1080 transparent 2D overlay buffer (key legends, top bar widgets,
    /// and OSD toasts) so host WebGL2 previews can composite it cleanly over live 60 FPS video!
    pub fn overlay_rgba(&self) -> (u32, u32, &[u8]) {
        (self.canvas.width, self.canvas.height, &self.canvas.rgba)
    }

    /// Reads back the last rendered frame as tightly-packed RGBA8 (`width * height * 4`).
    pub fn readback_rgba(&self) -> Result<Vec<u8>, String> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("readback-copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &self.output_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &self.readback_buf,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = self.readback_buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|e| format!("readback channel error: {e}"))?
            .map_err(|e| format!("readback map failed: {e:?}"))?;

        let mapped = slice.get_mapped_range();
        let row_bytes = (self.width * 4) as usize;
        let stride = self.padded_bytes_per_row as usize;
        let mut out = vec![0u8; row_bytes * self.height as usize];
        for y in 0..self.height as usize {
            let src = &mapped[y * stride..y * stride + row_bytes];
            let dst = &mut out[y * row_bytes..(y + 1) * row_bytes];
            dst.copy_from_slice(src);
        }
        drop(mapped);
        self.readback_buf.unmap();
        Ok(out)
    }

    pub fn save_png(&self, path: &Path) -> Result<(), String> {
        let rgba = self.readback_rgba()?;
        let img = image::RgbaImage::from_raw(self.width, self.height, rgba)
            .ok_or_else(|| "invalid RGBA buffer dimensions".to_string())?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        img.save(path).map_err(|e| format!("save png {}: {e}", path.display()))
    }
}

fn tex_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn build_fs_pipeline(
    device: &wgpu::Device,
    label: &str,
    vert_mod: &wgpu::ShaderModule,
    frag_wgsl: &str,
    bgls: &[&wgpu::BindGroupLayout],
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let frag_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(frag_wgsl.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: bgls,
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: vert_mod,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &frag_mod,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}
