//! The wgpu renderer.
//!
//! Two pipelines, two atlases, one gradient ramp texture. A frame is drawn by
//! translating the scene's primitives into instance buffers and issuing one
//! draw call per batch.
//!
//! The surface is deliberately configured without an sRGB format. Colors are
//! authored in sRGB, gradients interpolate in sRGB, and blending happens in
//! sRGB, which is what every other interface toolkit does and what a stylesheet
//! author expects. Letting the hardware convert would silently darken every
//! blend.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use guirs_core::{Bounds, Gradient, Paint, Px, ScaleFactor};
use guirs_text::{GlyphContent, GlyphKey, TextSystem};

use crate::atlas::{Atlas, AtlasSlot};
use crate::primitives::{
    FrameUniforms, GradientRef, IconSprite, Image, ImageId, QuadInstance, SpriteInstance,
    MAX_ROUNDED_CLIPS, MAX_TRANSFORMS, NO_CLIP, NO_TRANSFORM,
    SpriteKind,
};
use crate::scene::{Batch, BatchKind, QuadItem, Scene, SpriteItem};

/// Edge length of one page of the single channel atlas, in texels.
///
/// Glyphs and icons are small, so one page of this size holds a few thousand of
/// them at the sizes an interface actually uses.
pub const MONO_ATLAS_SIZE: u32 = 1024;
/// Edge length of one page of the color atlas.
///
/// Smaller, because its contents are emoji and images: rare in most interfaces,
/// and it grows when they do appear.
pub const COLOR_ATLAS_SIZE: u32 = 512;

/// A page of the picture atlas.
///
/// Larger than the others because pictures are: a photograph does not fit in
/// the page emoji use, and a page large enough for one would waste most of
/// itself holding emoji. Pages are created when the first picture is drawn, so
/// an application showing none pays nothing for this.
///
/// Four megabytes a page, which is the balance struck here. It holds any
/// picture an interface normally shows at the size it shows it, and anything
/// larger is shrunk to fit rather than refused. Doubling it would hold a
/// photograph at full resolution and cost sixteen megabytes a page to do it,
/// for detail no box on screen is large enough to display.
pub const IMAGE_ATLAS_SIZE: u32 = 1024;
/// How many pages an atlas may grow to.
pub const MAX_ATLAS_LAYERS: u32 = 8;

/// Kept for callers that referred to the old single size.
pub const ATLAS_SIZE: u32 = MONO_ATLAS_SIZE;
/// Kept for callers that referred to the old fixed layer count.
pub const ATLAS_LAYERS: u32 = MAX_ATLAS_LAYERS;
/// Samples across one gradient ramp.
pub const RAMP_WIDTH: u32 = 256;
/// Distinct gradients that can be live at once.
pub const RAMP_ROWS: u32 = 64;

/// What can go wrong bringing the renderer up.
#[derive(Debug)]
pub enum RendererError {
    NoAdapter,
    Surface(wgpu::CreateSurfaceError),
    Device(wgpu::RequestDeviceError),
    /// The surface reported no usable texture format.
    NoSurfaceFormat,
}

impl std::fmt::Display for RendererError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RendererError::NoAdapter => {
                f.write_str("no graphics adapter supports the required backends")
            }
            RendererError::Surface(e) => write!(f, "could not create a surface: {e}"),
            RendererError::Device(e) => write!(f, "could not create a device: {e}"),
            RendererError::NoSurfaceFormat => f.write_str("the surface exposes no usable format"),
        }
    }
}

impl std::error::Error for RendererError {}

/// Where the time inside a render call went.
///
/// "Render" is three very different things wearing one name: turning the scene
/// into instance buffers, waiting for the presentation engine to hand back an
/// image, and recording and submitting the commands. They fail for unrelated
/// reasons, so they are measured separately.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RenderTimings {
    /// Building instance buffers, rasterizing new glyphs, uploading.
    pub prepare_ms: f32,
    /// Blocking until the swap chain has an image free.
    pub acquire_ms: f32,
    /// Recording the pass, submitting, and presenting.
    pub submit_ms: f32,
    /// Glyphs rasterized this frame. In steady state this should be zero.
    pub rasterized: u32,
}

/// Anything that lives in the single channel atlas.
///
/// Glyphs and icons share one texture, so they must share one allocator. Two
/// allocators over one texture hand out the same rectangles and quietly
/// overwrite each other, which shows up as text with a few wrong letters in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum MonoKey {
    Glyph(GlyphKey),
    Icon(IconKey),
}

/// Anything that lives in the color atlas.
///
/// Only color glyphs now: pictures moved to an atlas of their own, because one
/// page size cannot suit both an emoji and a photograph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ColorKey {
    /// A color glyph, as emoji fonts provide.
    Glyph(GlyphKey),
}

/// Identifies one rasterized icon mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct IconKey {
    /// Hash of the path data and its view box.
    path: u64,
    /// Size in device pixels, which is what the mask was rasterized at.
    width: u16,
    height: u16,
}

/// Which graphics API to draw through.
///
/// This matters more than it looks. Asking for every backend makes the driver
/// stack of every vendor on the machine load into the process, which on a
/// desktop with two graphics cards is several hundred megabytes of DLLs mapped
/// in order to pick one adapter. One backend loads one driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GraphicsBackend {
    /// The platform's native API: Direct3D 12 on Windows, Metal on macOS,
    /// Vulkan elsewhere. Falls back to anything available if that fails.
    #[default]
    Native,
    Dx12,
    Vulkan,
    Metal,
    OpenGl,
    /// Every backend the build supports. Slowest to start and heaviest in
    /// memory; useful when diagnosing a driver problem.
    Any,
}

impl GraphicsBackend {
    fn to_wgpu(self) -> wgpu::Backends {
        match self {
            GraphicsBackend::Native => {
                #[cfg(target_os = "windows")]
                {
                    wgpu::Backends::DX12
                }
                #[cfg(target_os = "macos")]
                {
                    wgpu::Backends::METAL
                }
                #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                {
                    wgpu::Backends::VULKAN
                }
            }
            GraphicsBackend::Dx12 => wgpu::Backends::DX12,
            GraphicsBackend::Vulkan => wgpu::Backends::VULKAN,
            GraphicsBackend::Metal => wgpu::Backends::METAL,
            GraphicsBackend::OpenGl => wgpu::Backends::GL,
            GraphicsBackend::Any => wgpu::Backends::all(),
        }
    }
}

/// What the renderer is holding, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RendererMemory {
    /// Atlas and ramp textures. These live in graphics memory, which is
    /// separate from the process heap on a discrete card and shared with it on
    /// an integrated one.
    pub textures: usize,
    /// Instance buffers, which grow to the busiest frame and stay there.
    pub buffers: usize,
    /// Glyphs, icons and images currently packed.
    pub atlas_entries: usize,
    /// Bookkeeping kept alongside the atlases.
    pub caches: usize,
}

impl RendererMemory {
    pub fn total(&self) -> usize {
        self.textures + self.buffers + self.caches
    }
}

/// Whether a frame actually reached the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    Presented,
    /// The surface was not ready. The caller should try again next frame.
    Skipped,
}

/// A vertex buffer that grows to fit and never shrinks.
struct InstanceBuffer {
    buffer: wgpu::Buffer,
    capacity: u64,
    label: &'static str,
}

impl InstanceBuffer {
    fn new(device: &wgpu::Device, label: &'static str, capacity: u64) -> Self {
        InstanceBuffer {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: capacity.max(1),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            capacity: capacity.max(1),
            label,
        }
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if bytes.len() as u64 > self.capacity {
            // Grow generously so a steadily busier frame does not reallocate
            // every time it adds a primitive.
            let capacity = (bytes.len() as u64).next_power_of_two();
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: capacity,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.capacity = capacity;
        }
        queue.write_buffer(&self.buffer, 0, bytes);
    }
}

struct ImageMeta {
    slot: AtlasSlot,
}

/// Maps gradients to rows of the ramp texture.
#[derive(Default)]
struct GradientCache {
    rows: HashMap<Arc<Gradient>, u32>,
    next_row: u32,
    /// Rows written since the last upload.
    pending: Vec<(u32, Vec<u8>)>,
}

impl GradientCache {
    fn resolve(&mut self, gradient: &Arc<Gradient>) -> Option<GradientRef> {
        if let Some(row) = self.rows.get(gradient) {
            return Some(GradientRef { row: *row });
        }
        if self.next_row >= RAMP_ROWS {
            // Out of rows. Start over rather than refusing to draw; the ramps
            // are rebuilt from the gradients this frame actually uses.
            self.rows.clear();
            self.next_row = 0;
        }
        let row = self.next_row;
        self.next_row += 1;
        self.rows.insert(gradient.clone(), row);
        self.pending.push((row, gradient.to_ramp()));
        Some(GradientRef { row })
    }
}

/// Draws scenes.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    scale_factor: ScaleFactor,

    quad_pipeline: wgpu::RenderPipeline,
    sprite_pipeline: wgpu::RenderPipeline,

    uniform_buffer: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    atlas_bind_group: wgpu::BindGroup,

    quad_instances: InstanceBuffer,
    sprite_instances: InstanceBuffer,
    quad_scratch: Vec<QuadInstance>,
    sprite_scratch: Vec<SpriteInstance>,

    /// One allocator per texture, covering everything that texture holds.
    mono_atlas: Atlas<MonoKey>,
    color_atlas: Atlas<ColorKey>,
    /// Pictures, kept apart from the emoji so each gets a page size that fits.
    image_atlas: Atlas<ImageId>,
    /// Bitmap offsets recorded when each glyph was rasterized. Needed again
    /// every frame to place the sprite relative to the pen position.
    glyph_placements: HashMap<GlyphKey, (i32, i32)>,
    /// Glyphs that live in the color atlas rather than the mono one.
    color_glyphs: HashSet<GlyphKey>,
    /// Where each rasterized icon's mask sits inside the box it was asked for.
    icon_placements: HashMap<IconKey, (i32, i32)>,
    /// Icons whose path produced no coverage at all.
    blank_icons: HashSet<IconKey>,
    /// Glyphs the rasterizer produced no bitmap for: spaces, control
    /// characters, anything with no ink.
    ///
    /// These never reach the atlas, so without remembering them there is
    /// nothing to hit on the next frame and they are rasterized again, forever.
    /// A screen of text contains a lot of spaces, and this was costing more
    /// than everything else in the frame put together.
    blank_glyphs: HashSet<GlyphKey>,
    mono_texture: wgpu::Texture,
    color_texture: wgpu::Texture,
    image_texture: wgpu::Texture,
    ramp_texture: wgpu::Texture,
    /// Atlases start at a single page and gain more only when the packer
    /// actually spills. Allocating for the worst case up front costs tens of
    /// megabytes that a typical interface never touches.
    mono_layers: u32,
    color_layers: u32,
    /// Zero until the first picture is drawn, when the texture is created.
    image_layers: u32,
    /// Held so the atlas bind group can be rebuilt when a texture grows.
    atlas_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,

    gradients: GradientCache,
    images: HashMap<ImageId, ImageMeta>,
    next_image_id: u32,

    frame_index: u64,
    timings: RenderTimings,
}

impl Renderer {
    /// Bring up a device and swap chain for a window.
    pub fn new<W: Clone + Into<wgpu::SurfaceTarget<'static>>>(
        target: W,
        width: u32,
        height: u32,
        scale_factor: ScaleFactor,
        vsync: bool,
        high_performance: bool,
        backend: GraphicsBackend,
    ) -> Result<Self, RendererError> {
        pollster::block_on(Renderer::new_async(
            target,
            width,
            height,
            scale_factor,
            vsync,
            high_performance,
            backend,
        ))
    }

    pub async fn new_async<W: Clone + Into<wgpu::SurfaceTarget<'static>>>(
        target: W,
        width: u32,
        height: u32,
        scale_factor: ScaleFactor,
        vsync: bool,
        high_performance: bool,
        backend: GraphicsBackend,
    ) -> Result<Self, RendererError> {
        let preferred = backend.to_wgpu();
        let power = if high_performance {
            wgpu::PowerPreference::HighPerformance
        } else {
            wgpu::PowerPreference::LowPower
        };

        // Try the one backend first. Only if it turns up no adapter does the
        // rest of the driver stack get loaded.
        let opened = open_backend(preferred, target.clone(), power).await;
        let (instance, surface, adapter) = match opened {
            Ok(triple) => triple,
            Err(error) if preferred != wgpu::Backends::all() => {
                log::warn!("{preferred:?} gave no adapter ({error}), trying the rest");
                open_backend(wgpu::Backends::all(), target, power).await?
            }
            Err(error) => return Err(error),
        };
        let _ = &instance;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("guirs device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(RendererError::Device)?;

        log::info!("adapter: {:?}", adapter.get_info());
        let capabilities = surface.get_capabilities(&adapter);
        // Prefer a linear format so that authored sRGB values reach the display
        // untouched. Fall back to whatever exists rather than failing.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .ok_or(RendererError::NoSurfaceFormat)?;

        let mut config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
            .ok_or(RendererError::NoSurfaceFormat)?;
        config.format = format;
        config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        // With vsync on, acquiring the next image blocks until the display is
        // ready, which paces the whole loop to the refresh rate. Turning it off
        // is the only way to see what a frame actually costs.
        let preferred: &[wgpu::PresentMode] = if vsync {
            &[wgpu::PresentMode::Fifo]
        } else {
            &[
                wgpu::PresentMode::Mailbox,
                wgpu::PresentMode::Immediate,
                wgpu::PresentMode::Fifo,
            ]
        };
        config.present_mode = preferred
            .iter()
            .copied()
            .find(|mode| capabilities.present_modes.contains(mode))
            .unwrap_or(capabilities.present_modes[0]);
        log::info!("present mode: {:?}", config.present_mode);
        surface.configure(&device, &config);

        let mono_texture = create_atlas_texture(
            &device,
            "guirs mono atlas",
            wgpu::TextureFormat::R8Unorm,
            MONO_ATLAS_SIZE,
            1,
        );
        let color_texture = create_atlas_texture(
            &device,
            "guirs color atlas",
            wgpu::TextureFormat::Rgba8Unorm,
            COLOR_ATLAS_SIZE,
            1,
        );
        // One texel until a picture is actually drawn. The binding has to
        // exist for the pipeline to be valid, but a page of the real size is
        // sixteen megabytes, and most applications show no pictures at all.
        let image_texture = create_atlas_texture(
            &device,
            "guirs image atlas placeholder",
            wgpu::TextureFormat::Rgba8Unorm,
            1,
            1,
        );
        let ramp_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("guirs gradient ramps"),
            size: wgpu::Extent3d {
                width: RAMP_WIDTH,
                height: RAMP_ROWS,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("guirs sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("guirs frame uniforms"),
            size: std::mem::size_of::<FrameUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("guirs frame layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("guirs atlas layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let ramp_view = ramp_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let frame_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("guirs frame bind group"),
            layout: &frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&ramp_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let mono_view = mono_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let color_view = color_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let image_view = image_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let atlas_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("guirs atlas bind group"),
            layout: &atlas_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&mono_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&image_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let blend = wgpu::BlendState {
            // Instance colors leave the fragment shader premultiplied, so the
            // source factor is one rather than src_alpha.
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };

        let quad_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("guirs quad shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/quad.wgsl").into()),
        });
        let sprite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("guirs sprite shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sprite.wgsl").into()),
        });

        let quad_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("guirs quad pipeline layout"),
                bind_group_layouts: &[Some(&frame_layout)],
                immediate_size: 0,
            });
        let sprite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("guirs sprite pipeline layout"),
                bind_group_layouts: &[Some(&frame_layout), Some(&atlas_layout)],
                immediate_size: 0,
            });

        let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("guirs quad pipeline"),
            layout: Some(&quad_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &quad_shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(QuadInstance::layout())],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &quad_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sprite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("guirs sprite pipeline"),
            layout: Some(&sprite_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &sprite_shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(SpriteInstance::layout())],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &sprite_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Ok(Renderer {
            quad_instances: InstanceBuffer::new(&device, "guirs quad instances", 64 * 1024),
            sprite_instances: InstanceBuffer::new(&device, "guirs sprite instances", 64 * 1024),
            device,
            queue,
            surface,
            config,
            scale_factor,
            quad_pipeline,
            sprite_pipeline,
            uniform_buffer,
            frame_bind_group,
            atlas_bind_group,
            quad_scratch: Vec::new(),
            sprite_scratch: Vec::new(),
            mono_atlas: Atlas::new(MONO_ATLAS_SIZE, MAX_ATLAS_LAYERS as usize),
            color_atlas: Atlas::new(COLOR_ATLAS_SIZE, MAX_ATLAS_LAYERS as usize),
            image_atlas: Atlas::new(IMAGE_ATLAS_SIZE, MAX_ATLAS_LAYERS as usize),
            glyph_placements: HashMap::new(),
            color_glyphs: HashSet::new(),
            blank_glyphs: HashSet::new(),
            icon_placements: HashMap::new(),
            blank_icons: HashSet::new(),
            mono_texture,
            color_texture,
            image_texture,
            ramp_texture,
            mono_layers: 1,
            color_layers: 1,
            image_layers: 0,
            atlas_layout,
            sampler,
            gradients: GradientCache::default(),
            images: HashMap::new(),
            next_image_id: 1,
            frame_index: 0,
            timings: RenderTimings::default(),
        })
    }

    /// Surface size in logical pixels.
    pub fn size(&self) -> (f32, f32) {
        (
            self.config.width as f32 / self.scale_factor.0,
            self.config.height as f32 / self.scale_factor.0,
        )
    }

    pub fn scale_factor(&self) -> ScaleFactor {
        self.scale_factor
    }

    /// Reconfigure after the window changed size or moved to another display.
    pub fn resize(&mut self, width: u32, height: u32, scale_factor: ScaleFactor) {
        if width == 0 || height == 0 {
            return;
        }
        let scale_changed = (scale_factor.0 - self.scale_factor.0).abs() > f32::EPSILON;
        self.config.width = width;
        self.config.height = height;
        self.scale_factor = scale_factor;
        self.surface.configure(&self.device, &self.config);

        if scale_changed {
            // Every cached glyph was rasterized for the old scale, so they are
            // all the wrong size now.
            self.mono_atlas.reset();
            self.glyph_placements.clear();
            self.color_glyphs.clear();
            self.blank_glyphs.clear();
            self.icon_placements.clear();
            self.blank_icons.clear();
        }
    }

    /// Upload an RGBA image and return a handle for drawing it.
    ///
    /// Returns `None` when the image will not fit in the atlas.
    pub fn add_image(&mut self, width: u32, height: u32, rgba: &[u8]) -> Option<ImageId> {
        if width == 0 || height == 0 {
            return None;
        }
        if rgba.len() < (width * height * 4) as usize {
            return None;
        }

        if width > IMAGE_ATLAS_SIZE || height > IMAGE_ATLAS_SIZE {
            return None;
        }

        let id = ImageId(self.next_image_id);
        let slot = self.image_atlas.insert(id, width, height)?;
        self.next_image_id += 1;
        self.ensure_image_layers(slot.page as u32 + 1);

        write_layer(&self.queue, &self.image_texture, slot, rgba, 4);
        self.images.insert(id, ImageMeta { slot });
        Some(id)
    }

    /// Size of a previously added image, in texels.
    pub fn image_size(&self, id: ImageId) -> Option<(u32, u32)> {
        self.images.get(&id).map(|m| (m.slot.width, m.slot.height))
    }

    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// Glyphs held in the atlas, and glyphs known to have no ink.
    pub fn glyph_cache_size(&self) -> (usize, usize) {
        (self.mono_atlas.entry_count(), self.blank_glyphs.len())
    }

    /// Make sure the single channel atlas has room for `layers` pages.
    fn ensure_mono_layers(&mut self, layers: u32) {
        if layers <= self.mono_layers {
            return;
        }
        let layers = layers.min(MAX_ATLAS_LAYERS);
        let grown = create_atlas_texture(
            &self.device,
            "guirs mono atlas",
            wgpu::TextureFormat::R8Unorm,
            MONO_ATLAS_SIZE,
            layers,
        );
        copy_layers(
            &self.device,
            &self.queue,
            &self.mono_texture,
            &grown,
            MONO_ATLAS_SIZE,
            self.mono_layers,
        );
        self.mono_texture = grown;
        self.mono_layers = layers;
        self.rebuild_atlas_bind_group();
        log::debug!("mono atlas grew to {layers} pages");
    }

    /// Make sure the color atlas has room for `layers` pages.
    fn ensure_color_layers(&mut self, layers: u32) {
        if layers <= self.color_layers {
            return;
        }
        let layers = layers.min(MAX_ATLAS_LAYERS);
        let grown = create_atlas_texture(
            &self.device,
            "guirs color atlas",
            wgpu::TextureFormat::Rgba8Unorm,
            COLOR_ATLAS_SIZE,
            layers,
        );
        copy_layers(
            &self.device,
            &self.queue,
            &self.color_texture,
            &grown,
            COLOR_ATLAS_SIZE,
            self.color_layers,
        );
        self.color_texture = grown;
        self.color_layers = layers;
        self.rebuild_atlas_bind_group();
        log::debug!("color atlas grew to {layers} pages");
    }

    /// Create or grow the picture atlas.
    ///
    /// The first call replaces the one texel placeholder with a real page,
    /// which is why an application showing no pictures never allocates one.
    fn ensure_image_layers(&mut self, layers: u32) {
        if layers <= self.image_layers {
            return;
        }
        let layers = layers.min(MAX_ATLAS_LAYERS);
        let grown = create_atlas_texture(
            &self.device,
            "guirs image atlas",
            wgpu::TextureFormat::Rgba8Unorm,
            IMAGE_ATLAS_SIZE,
            layers,
        );
        // Nothing to carry over on the first call: the placeholder is a
        // different size, and holds nothing anybody can see.
        if self.image_layers > 0 {
            copy_layers(
                &self.device,
                &self.queue,
                &self.image_texture,
                &grown,
                IMAGE_ATLAS_SIZE,
                self.image_layers,
            );
        }
        self.image_texture = grown;
        self.image_layers = layers;
        self.rebuild_atlas_bind_group();
        log::debug!("image atlas grew to {layers} pages");
    }

    fn rebuild_atlas_bind_group(&mut self) {
        let mono_view = self.mono_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let color_view = self.color_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let image_view = self.image_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        self.atlas_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("guirs atlas bind group"),
            layout: &self.atlas_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&mono_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&image_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
    }

    /// What the renderer is holding on to.
    pub fn memory(&self) -> RendererMemory {
        let mono = (MONO_ATLAS_SIZE * MONO_ATLAS_SIZE) as usize * self.mono_layers as usize;
        let color =
            (COLOR_ATLAS_SIZE * COLOR_ATLAS_SIZE) as usize * self.color_layers as usize * 4;
        let images =
            (IMAGE_ATLAS_SIZE * IMAGE_ATLAS_SIZE) as usize * self.image_layers as usize * 4;
        RendererMemory {
            textures: mono + color + images + (RAMP_WIDTH * RAMP_ROWS * 4) as usize,
            buffers: (self.quad_instances.capacity + self.sprite_instances.capacity) as usize,
            atlas_entries: self.mono_atlas.entry_count()
                + self.color_atlas.entry_count()
                + self.image_atlas.entry_count(),
            caches: self.glyph_placements.len()
                * (std::mem::size_of::<GlyphKey>() + std::mem::size_of::<(i32, i32)>())
                + self.icon_placements.len()
                    * (std::mem::size_of::<IconKey>() + std::mem::size_of::<(i32, i32)>())
                + (self.blank_glyphs.len() + self.color_glyphs.len())
                    * std::mem::size_of::<GlyphKey>(),
        }
    }

    /// Where the last frame's render time went.
    pub fn timings(&self) -> RenderTimings {
        self.timings
    }

    /// Draw one scene.
    pub fn render(&mut self, scene: &Scene, text: &mut TextSystem) -> FrameOutcome {
        self.frame_index += 1;
        let prepare_started = std::time::Instant::now();
        self.timings.rasterized = 0;

        // Translate the scene before acquiring the swap chain image, not after.
        // Acquiring blocks until the presentation engine hands one back, so
        // doing it first means the CPU sits idle during the wait and then does
        // its work while the GPU is free. This way the two overlap.
        self.prepare(scene, text);
        self.timings.prepare_ms = prepare_started.elapsed().as_secs_f32() * 1000.0;

        let acquire_started = std::time::Instant::now();
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            // Nothing is visible, so there is nothing worth drawing.
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return FrameOutcome::Skipped
            }
            // The swap chain went stale, usually because the window was resized
            // between configure and acquire. Rebuild and skip this frame; the
            // next one will be correct.
            _ => {
                self.surface.configure(&self.device, &self.config);
                return FrameOutcome::Skipped;
            }
        };

        self.timings.acquire_ms = acquire_started.elapsed().as_secs_f32() * 1000.0;
        let submit_started = std::time::Instant::now();

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("guirs frame"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("guirs main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: scene.background.r as f64,
                            g: scene.background.g as f64,
                            b: scene.background.b as f64,
                            a: scene.background.a as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            pass.set_bind_group(0, &self.frame_bind_group, &[]);

            for batch in scene.ordered_batches() {
                if !self.apply_scissor(&mut pass, batch) {
                    continue;
                }
                match batch.kind {
                    BatchKind::Quad => {
                        pass.set_pipeline(&self.quad_pipeline);
                        pass.set_vertex_buffer(0, self.quad_instances.buffer.slice(..));
                        pass.draw(0..6, batch.range.clone());
                    }
                    BatchKind::Sprite => {
                        pass.set_pipeline(&self.sprite_pipeline);
                        pass.set_bind_group(1, &self.atlas_bind_group, &[]);
                        pass.set_vertex_buffer(0, self.sprite_instances.buffer.slice(..));
                        pass.draw(0..6, batch.range.clone());
                    }
                }
            }
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        self.timings.submit_ms = submit_started.elapsed().as_secs_f32() * 1000.0;
        FrameOutcome::Presented
    }

    /// Set the scissor for a batch. Returns false when nothing would be drawn.
    fn apply_scissor(&self, pass: &mut wgpu::RenderPass<'_>, batch: &Batch) -> bool {
        let (width, height) = (self.config.width, self.config.height);
        let Some(clip) = batch.clip else {
            pass.set_scissor_rect(0, 0, width, height);
            return true;
        };

        let scale = self.scale_factor.0;
        let left = (clip.left().0 * scale).floor().max(0.0) as u32;
        let top = (clip.top().0 * scale).floor().max(0.0) as u32;
        let right = ((clip.right().0 * scale).ceil().max(0.0) as u32).min(width);
        let bottom = ((clip.bottom().0 * scale).ceil().max(0.0) as u32).min(height);

        if left >= right || top >= bottom {
            return false;
        }
        pass.set_scissor_rect(left, top, right - left, bottom - top);
        true
    }

    /// Translate the scene into instance buffers, filling atlases as needed.
    fn prepare(&mut self, scene: &Scene, text: &mut TextSystem) {
        let (logical_width, logical_height) = self.size();
        let clips = collect_rounded_clips(scene);
        let transforms = collect_transforms(scene);
        let mut transform_table = [[0.0f32; 4]; MAX_TRANSFORMS * 2];
        for (index, transform) in transforms.iter().enumerate() {
            transform_table[index * 2] = [transform.a, transform.b, transform.c, transform.d];
            transform_table[index * 2 + 1] = [
                transform.tx,
                transform.ty,
                // The coverage ramp is a device pixel wide, so a scaled shape
                // needs its distances divided by how much it grew or its edge
                // goes hard.
                transform.average_scale(),
                0.0,
            ];
        }

        let mut table = [[0.0f32; 4]; MAX_ROUNDED_CLIPS * 2];
        for (index, clip) in clips.iter().enumerate() {
            table[index * 2] = [
                clip.bounds.origin.x.0,
                clip.bounds.origin.y.0,
                clip.bounds.size.width.0,
                clip.bounds.size.height.0,
            ];
            let radii = clip.radii.clamp_to(clip.bounds.size);
            table[index * 2 + 1] = [
                radii.top_left.0,
                radii.top_right.0,
                radii.bottom_right.0,
                radii.bottom_left.0,
            ];
        }

        self.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&FrameUniforms {
                viewport: [logical_width, logical_height],
                scale_factor: self.scale_factor.0,
                _padding: 0.0,
                atlas_sizes: [
                    ATLAS_SIZE as f32,
                    ATLAS_SIZE as f32,
                    RAMP_ROWS as f32,
                    0.0,
                ],
                clips: table,
                transforms: transform_table,
            }),
        );

        // Quads, resolving gradients into ramp rows.
        self.quad_scratch.clear();
        self.quad_scratch.reserve(scene.quads().len());
        for item in scene.quads() {
            let instance = match item {
                QuadItem::Box(quad) => {
                    let gradient = match &quad.background {
                        Paint::Gradient(g) => self.gradients.resolve(g),
                        _ => None,
                    };
                    QuadInstance::from_quad(quad, gradient)
                }
                QuadItem::Shadow(shadow) => QuadInstance::from_shadow(shadow),
            };
            self.quad_scratch.push(instance);
        }

        for (row, ramp) in std::mem::take(&mut self.gradients.pending) {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.ramp_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: row, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &ramp,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(RAMP_WIDTH * 4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: RAMP_WIDTH,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }

        // Sprites, rasterizing any glyph the atlas has not seen.
        self.sprite_scratch.clear();
        self.sprite_scratch.reserve(scene.sprites().len());
        for item in scene.sprites() {
            let instance = match item {
                SpriteItem::Glyph(glyph) => self.prepare_glyph(glyph, text),
                SpriteItem::Image(image) => self.prepare_image(image),
                SpriteItem::Icon(icon) => self.prepare_icon(icon),
            };
            // Every scene primitive keeps its slot even when it cannot be
            // drawn, because batch ranges index this list positionally.
            self.sprite_scratch.push(instance.unwrap_or_default());
        }

        stamp_batch_slots(
            scene,
            &clips,
            &transforms,
            &mut self.quad_scratch,
            &mut self.sprite_scratch,
        );

        let quads = std::mem::take(&mut self.quad_scratch);
        self.quad_instances
            .upload(&self.device, &self.queue, bytemuck::cast_slice(&quads));
        self.quad_scratch = quads;

        let sprites = std::mem::take(&mut self.sprite_scratch);
        self.sprite_instances
            .upload(&self.device, &self.queue, bytemuck::cast_slice(&sprites));
        self.sprite_scratch = sprites;
    }

    fn prepare_glyph(
        &mut self,
        glyph: &crate::primitives::Glyph,
        text: &mut TextSystem,
    ) -> Option<SpriteInstance> {
        let key = glyph.key;
        // Which texture a glyph lives in is decided when it is first
        // rasterized, so the lookup has to know that before it can find it.
        let cached = if self.color_glyphs.contains(&key) {
            self.color_atlas
                .get(&ColorKey::Glyph(key))
                .map(|slot| (slot, true))
        } else {
            self.mono_atlas
                .get(&MonoKey::Glyph(key))
                .map(|slot| (slot, false))
        };

        let (slot, is_color) = match cached {
            Some(hit) => hit,
            None => {
                if self.blank_glyphs.contains(&key) {
                    return None;
                }
                self.timings.rasterized += 1;
                let Some(raster) = text.rasterizer.rasterize(&text.fonts, key) else {
                    // Nothing to draw. Remember it, or every space on screen is
                    // rasterized again on the next frame.
                    self.blank_glyphs.insert(key);
                    return None;
                };

                let is_color = raster.content == GlyphContent::Color;
                // A failure to find room is not the same as having no ink: the
                // atlas may have space for it later, so it is not remembered.
                let slot = if is_color {
                    let slot =
                        self.color_atlas
                            .insert(ColorKey::Glyph(key), raster.width, raster.height)?;
                    // The packer may have opened a page the texture does not
                    // have yet, so grow before writing into it.
                    self.ensure_color_layers(slot.page as u32 + 1);
                    write_layer(&self.queue, &self.color_texture, slot, &raster.data, 4);
                    self.color_glyphs.insert(key);
                    slot
                } else {
                    let slot =
                        self.mono_atlas
                            .insert(MonoKey::Glyph(key), raster.width, raster.height)?;
                    self.ensure_mono_layers(slot.page as u32 + 1);
                    write_layer(&self.queue, &self.mono_texture, slot, &raster.data, 1);
                    slot
                };
                self.glyph_placements.insert(key, (raster.left, raster.top));
                (slot, is_color)
            }
        };

        let (left, top) = *self.glyph_placements.get(&key)?;
        let scale = self.scale_factor.0;
        let bounds = crate::scene::glyph_bounds(
            glyph.position,
            left,
            top,
            slot.width,
            slot.height,
            scale,
        );

        Some(SpriteInstance {
            bounds: [
                bounds.origin.x.0,
                bounds.origin.y.0,
                bounds.size.width.0,
                bounds.size.height.0,
            ],
            uv: slot.uv(if is_color { COLOR_ATLAS_SIZE } else { MONO_ATLAS_SIZE }),
            color: glyph.color.to_array(),
            params: [
                if is_color {
                    SpriteKind::ColorGlyph as u32 as f32
                } else {
                    SpriteKind::AlphaGlyph as u32 as f32
                },
                0.0,
                1.0,
                slot.page as f32,
            ],
            // Slots are stamped on later, once the batches are known.
            ..SpriteInstance::default()
        })
    }

    /// Rasterize an icon's path into the mono atlas, or reuse what is there.
    ///
    /// The mask is filled at the exact device size the icon will occupy, which
    /// is why an icon stays sharp at any scale factor: it is not a bitmap being
    /// stretched, it is the path being filled again at the size it is needed.
    fn prepare_icon(&mut self, sprite: &IconSprite) -> Option<SpriteInstance> {
        let scale = self.scale_factor.0;
        let device_width = (sprite.bounds.size.width.0 * scale).round().max(1.0) as u16;
        let device_height = (sprite.bounds.size.height.0 * scale).round().max(1.0) as u16;

        let key = IconKey {
            path: sprite.icon.key() ^ ((sprite.icon.view_box() as u64) << 48),
            width: device_width,
            height: device_height,
        };

        let slot = match self.mono_atlas.get(&MonoKey::Icon(key)) {
            Some(slot) => slot,
            None => {
                if self.blank_icons.contains(&key) {
                    return None;
                }
                let (mask, left, top, width, height) =
                    rasterize_icon(&sprite.icon, device_width, device_height)?;
                if width == 0 || height == 0 {
                    self.blank_icons.insert(key);
                    return None;
                }
                let slot = self.mono_atlas.insert(MonoKey::Icon(key), width, height)?;
                self.ensure_mono_layers(slot.page as u32 + 1);
                write_layer(&self.queue, &self.mono_texture, slot, &mask, 1);
                self.icon_placements.insert(key, (left, top));
                slot
            }
        };

        let (left, top) = *self.icon_placements.get(&key)?;
        // The mask covers only the inked part of the box, so place it where the
        // rasterizer said it belongs rather than at the box origin.
        let bounds = Bounds::from_xywh(
            sprite.bounds.origin.x + Px(left as f32 / scale),
            sprite.bounds.origin.y + Px(top as f32 / scale),
            Px(slot.width as f32 / scale),
            Px(slot.height as f32 / scale),
        );

        Some(SpriteInstance {
            bounds: [
                bounds.origin.x.0,
                bounds.origin.y.0,
                bounds.size.width.0,
                bounds.size.height.0,
            ],
            uv: slot.uv(MONO_ATLAS_SIZE),
            color: sprite.color.to_array(),
            params: [
                SpriteKind::AlphaGlyph as u32 as f32,
                0.0,
                1.0,
                slot.page as f32,
            ],
            // Slots are stamped on later, once the batches are known.
            ..SpriteInstance::default()
        })
    }

    fn prepare_image(&mut self, image: &Image) -> Option<SpriteInstance> {
        let meta = self.images.get(&image.id)?;
        let slot = meta.slot;
        Some(SpriteInstance {
            bounds: [
                image.bounds.origin.x.0,
                image.bounds.origin.y.0,
                image.bounds.size.width.0,
                image.bounds.size.height.0,
            ],
            uv: crop_uv(slot.uv(IMAGE_ATLAS_SIZE), image.crop),
            color: image.tint.to_array(),
            params: [
                SpriteKind::Image as u32 as f32,
                image
                    .corner_radii
                    .clamp_to(image.bounds.size)
                    .max()
                    .0,
                1.0,
                slot.page as f32,
            ],
            // Slots are stamped on later, once the batches are known.
            ..SpriteInstance::default()
        })
    }
}

/// Narrow a slot's texture coordinates to the part of the image being drawn.
///
/// The crop is in the image's own space, so it has to be mapped into the
/// slot the image was packed into rather than used directly.
fn crop_uv(slot: [f32; 4], crop: [f32; 4]) -> [f32; 4] {
    let width = slot[2] - slot[0];
    let height = slot[3] - slot[1];
    [
        slot[0] + width * crop[0],
        slot[1] + height * crop[1],
        slot[0] + width * crop[2],
        slot[1] + height * crop[3],
    ]
}

/// Fill an icon's path into an 8 bit coverage mask.
///
/// Returns the mask, where it sits inside the requested box, and its size.
fn rasterize_icon(
    icon: &guirs_core::Icon,
    width: u16,
    height: u16,
) -> Option<(Vec<u8>, i32, i32, u32, u32)> {
    // Icons are authored in a square, so a non square target scales by the
    // smaller side and centers, rather than distorting the artwork.
    let scale = (width as f32 / icon.view_box() as f32)
        .min(height as f32 / icon.view_box() as f32)
        .max(0.0001);
    let drawn = icon.view_box() as f32 * scale;
    let offset_x = ((width as f32 - drawn) * 0.5).max(0.0);
    let offset_y = ((height as f32 - drawn) * 0.5).max(0.0);

    // The stroke width is authored in view box units, so it scales with the
    // artwork and an icon keeps its weight at any rendered size.
    // The stroke has to outlive the borrow the mask takes of it.
    let stroke = match icon.style() {
        guirs_core::IconStyle::Stroke(width) => Some(
            *zeno::Stroke::new(width * scale)
                .cap(zeno::Cap::Round)
                .join(zeno::Join::Round),
        ),
        guirs_core::IconStyle::Fill => None,
    };
    let style: zeno::Style = match &stroke {
        Some(stroke) => (*stroke).into(),
        None => zeno::Fill::NonZero.into(),
    };

    let (mask, placement) = zeno::Mask::new(icon.path())
        .style(style)
        .transform(Some(
            zeno::Transform::scale(scale, scale).then_translate(offset_x, offset_y),
        ))
        .format(zeno::Format::Alpha)
        .origin(zeno::Origin::TopLeft)
        .render();

    if placement.width == 0 || placement.height == 0 {
        return Some((Vec::new(), 0, 0, 0, 0));
    }
    Some((
        mask,
        placement.left,
        placement.top,
        placement.width,
        placement.height,
    ))
}

/// The distinct rounded clips a frame uses, in first use order.
///
/// There are only ever a handful: a clip needs both a corner radius and
/// children to clip. Anything past the table's size keeps its scissor and loses
/// its corners, which is a squared off corner rather than a wrong picture.
fn collect_rounded_clips(scene: &Scene) -> Vec<crate::scene::RoundedClip> {
    let mut clips: Vec<crate::scene::RoundedClip> = Vec::new();
    for batch in scene.batches() {
        let Some(rounded) = batch.rounded_clip else {
            continue;
        };
        if clips.contains(&rounded) {
            continue;
        }
        if clips.len() == MAX_ROUNDED_CLIPS {
            log::warn!("more than {MAX_ROUNDED_CLIPS} rounded clips in one frame, squaring the rest");
            break;
        }
        clips.push(rounded);
    }
    clips
}

/// The distinct transforms a frame uses, in first use order.
fn collect_transforms(scene: &Scene) -> Vec<guirs_core::Affine> {
    let mut transforms: Vec<guirs_core::Affine> = Vec::new();
    for batch in scene.batches() {
        let Some(transform) = batch.transform else {
            continue;
        };
        if transforms.contains(&transform) {
            continue;
        }
        if transforms.len() == MAX_TRANSFORMS {
            log::warn!("more than {MAX_TRANSFORMS} transforms in one frame, drawing the rest in place");
            break;
        }
        transforms.push(transform);
    }
    transforms
}

/// Tell every instance which rounded clip and which transform apply to it.
///
/// Both belong to a whole batch, so they are stamped on afterwards rather than
/// threaded through every instance constructor. Batch ranges index the instance
/// lists positionally, which is why a primitive that could not be prepared
/// still keeps its slot in them.
fn stamp_batch_slots(
    scene: &Scene,
    clips: &[crate::scene::RoundedClip],
    transforms: &[guirs_core::Affine],
    quads: &mut [QuadInstance],
    sprites: &mut [SpriteInstance],
) {
    for batch in scene.batches() {
        // Anything past a table's size keeps its old behaviour: a clip loses
        // its corners, a transform draws where it was laid out.
        let clip_slot = batch
            .rounded_clip
            .and_then(|rounded| clips.iter().position(|clip| *clip == rounded))
            .map(|slot| slot as f32)
            .unwrap_or(NO_CLIP);
        let transform_slot = batch
            .transform
            .and_then(|moved| transforms.iter().position(|entry| *entry == moved))
            .map(|slot| slot as f32)
            .unwrap_or(NO_TRANSFORM);
        if clip_slot == NO_CLIP && transform_slot == NO_TRANSFORM {
            continue;
        }

        let range = batch.range.start as usize..batch.range.end as usize;
        match batch.kind {
            BatchKind::Quad => {
                for instance in &mut quads[range] {
                    instance.extra[2] = clip_slot;
                    instance.extra[3] = transform_slot;
                }
            }
            BatchKind::Sprite => {
                for instance in &mut sprites[range] {
                    instance.extra[0] = clip_slot;
                    instance.extra[1] = transform_slot;
                }
            }
        }
    }
}

/// Bring up an instance, a surface and an adapter on one set of backends.
async fn open_backend<W: Into<wgpu::SurfaceTarget<'static>>>(
    backends: wgpu::Backends,
    target: W,
    power: wgpu::PowerPreference,
) -> Result<(wgpu::Instance, wgpu::Surface<'static>, wgpu::Adapter), RendererError> {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = backends;
    // An explicit WGPU_BACKEND still wins; without one the choice above stands.
    let instance = wgpu::Instance::new(descriptor.with_env());

    let surface = instance
        .create_surface(target)
        .map_err(RendererError::Surface)?;

    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: power,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            ..Default::default()
        })
        .await
        .map_err(|_| RendererError::NoAdapter)?;

    Ok((instance, surface, adapter))
}

fn create_atlas_texture(
    device: &wgpu::Device,
    label: &str,
    format: wgpu::TextureFormat,
    size: u32,
    layers: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: layers.max(1),
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        // COPY_SRC so the existing pages survive a growth.
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// Copy the pages an atlas already had into its replacement.
fn copy_layers(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    from: &wgpu::Texture,
    to: &wgpu::Texture,
    size: u32,
    layers: u32,
) {
    if layers == 0 {
        return;
    }
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("guirs atlas growth"),
    });
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: from,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: to,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: layers,
        },
    );
    queue.submit(Some(encoder.finish()));
}

fn write_layer(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    slot: AtlasSlot,
    data: &[u8],
    bytes_per_pixel: u32,
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: slot.x,
                y: slot.y,
                z: slot.page as u32,
            },
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(slot.width * bytes_per_pixel),
            rows_per_image: Some(slot.height),
        },
        wgpu::Extent3d {
            width: slot.width,
            height: slot.height,
            depth_or_array_layers: 1,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use guirs_core::Rgba;

    #[test]
    fn the_clip_table_holds_each_distinct_clip_once() {
        use crate::scene::RoundedClip;
        use guirs_core::{Corners, Paint};

        let card = Bounds::from_xywh(Px(0.0), Px(0.0), Px(100.0), Px(100.0));
        let radii = Corners::all(Px(8.0));
        let quad = crate::primitives::Quad {
            bounds: Bounds::from_xywh(Px(1.0), Px(1.0), Px(4.0), Px(4.0)),
            background: Paint::Solid(guirs_core::Rgba::BLACK),
            ..crate::primitives::Quad::default()
        };

        let mut scene = Scene::new();
        // The same clip used twice, with an unclipped run in between.
        scene.push_rounded_clip(card, radii);
        scene.push_quad(quad.clone());
        scene.pop_clip();
        scene.push_quad(quad.clone());
        scene.push_rounded_clip(card, radii);
        scene.push_quad(quad.clone());
        scene.pop_clip();

        let clips = collect_rounded_clips(&scene);
        assert_eq!(clips, vec![RoundedClip { bounds: card, radii }]);
    }

    #[test]
    fn stamping_marks_only_the_instances_inside_the_clip() {
        use guirs_core::{Corners, Paint};

        let card = Bounds::from_xywh(Px(0.0), Px(0.0), Px(100.0), Px(100.0));
        let radii = Corners::all(Px(8.0));
        let quad = crate::primitives::Quad {
            bounds: Bounds::from_xywh(Px(1.0), Px(1.0), Px(4.0), Px(4.0)),
            background: Paint::Solid(guirs_core::Rgba::BLACK),
            ..crate::primitives::Quad::default()
        };

        let mut scene = Scene::new();
        scene.push_quad(quad.clone()); // 0: outside
        scene.push_rounded_clip(card, radii);
        scene.push_quad(quad.clone()); // 1: inside
        scene.push_quad(quad.clone()); // 2: inside
        scene.pop_clip();
        scene.push_quad(quad.clone()); // 3: outside

        let clips = collect_rounded_clips(&scene);
        let mut quads = vec![QuadInstance::default(); scene.quads().len()];
        let mut sprites: Vec<SpriteInstance> = Vec::new();
        stamp_batch_slots(&scene, &clips, &[], &mut quads, &mut sprites);

        let slots: Vec<f32> = quads.iter().map(|instance| instance.extra[2]).collect();
        assert_eq!(slots, vec![NO_CLIP, 0.0, 0.0, NO_CLIP]);
    }

    #[test]
    fn a_clip_past_the_table_keeps_its_scissor_and_loses_its_corners() {
        use guirs_core::{Corners, Paint};

        let quad = crate::primitives::Quad {
            bounds: Bounds::from_xywh(Px(1.0), Px(1.0), Px(4.0), Px(4.0)),
            background: Paint::Solid(guirs_core::Rgba::BLACK),
            ..crate::primitives::Quad::default()
        };

        let mut scene = Scene::new();
        // One more distinct clip than the table can hold. They differ by
        // radius rather than by position, so every one of them still contains
        // the quad and none is culled before it reaches a batch.
        for index in 0..(MAX_ROUNDED_CLIPS + 1) {
            scene.push_rounded_clip(
                Bounds::from_xywh(Px(0.0), Px(0.0), Px(100.0), Px(100.0)),
                Corners::all(Px(4.0 + index as f32)),
            );
            scene.push_quad(quad.clone());
            scene.pop_clip();
        }

        let clips = collect_rounded_clips(&scene);
        assert_eq!(clips.len(), MAX_ROUNDED_CLIPS);

        let mut quads = vec![QuadInstance::default(); scene.quads().len()];
        let mut sprites: Vec<SpriteInstance> = Vec::new();
        stamp_batch_slots(&scene, &clips, &[], &mut quads, &mut sprites);

        // Everything the table covered is stamped; the overflow is left square
        // rather than being pointed at the wrong clip.
        for instance in quads.iter().take(MAX_ROUNDED_CLIPS) {
            assert_ne!(instance.extra[2], NO_CLIP);
        }
        assert_eq!(quads.last().unwrap().extra[2], NO_CLIP);
    }

    #[test]
    fn atlas_constants_are_consistent() {
        assert!(ATLAS_SIZE.is_power_of_two());
        // Checked at compile time, because a zero layer atlas would not be a
        // runtime failure so much as a texture that cannot be created.
        const _: () = assert!(ATLAS_LAYERS >= 1);
        assert_eq!(RAMP_WIDTH, 256);
    }

    #[test]
    fn gradient_rows_are_reused_and_recycled() {
        let mut cache = GradientCache::default();
        let a = Arc::new(Gradient::vertical(Rgba::BLACK, Rgba::WHITE));
        let first = cache.resolve(&a).unwrap();
        let again = cache.resolve(&a).unwrap();
        assert_eq!(first.row, again.row);
        assert_eq!(cache.pending.len(), 1);

        // Distinct gradients take distinct rows.
        let b = Arc::new(Gradient::vertical(Rgba::WHITE, Rgba::BLACK));
        assert_ne!(cache.resolve(&b).unwrap().row, first.row);
    }

    #[test]
    fn running_out_of_rows_wraps_instead_of_failing() {
        let mut cache = GradientCache::default();
        for i in 0..(RAMP_ROWS + 5) {
            let shade = Rgba::rgb8(i as u8, 0, 0);
            let gradient = Arc::new(Gradient::vertical(shade, Rgba::WHITE));
            assert!(cache.resolve(&gradient).is_some(), "failed at {i}");
        }
        assert!(cache.next_row <= RAMP_ROWS);
    }
}
