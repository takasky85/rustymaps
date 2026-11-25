//! GPU side of the 3D aircraft: baked meshes (loaded from the data folder), one offscreen pass with
//! a shared depth buffer that draws every visible plane, and the composite that lays the result over
//! the map. It is its own paint callback so it can sit *above* the route lines and station markers.

use eframe::egui;
use eframe::egui_wgpu;
use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;

use crate::data::{ModelData, MODEL_HEADER, VERTEX_SIZE};

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// One aircraft to draw this frame.
#[derive(Clone, Copy)]
pub struct PlaneDraw {
    /// Index of the uploaded model.
    pub model: usize,
    pub mvp: [f32; 16],
    pub nrm: [f32; 16],
}

struct ModelGpu {
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    index_count: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Uniform {
    mvp: [f32; 16],
    nrm: [f32; 16],
    flags: [f32; 4],
}

struct Target {
    size: (u32, u32),
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

pub struct PlaneGpu {
    pipeline: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    uniform_bgl: wgpu::BindGroupLayout,
    /// One uniform buffer + bind group per plane drawn in a frame (grown on demand).
    uniforms: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    composite_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    models: Vec<Option<ModelGpu>>,
    format: wgpu::TextureFormat,
    target: Option<Target>,
    draws: Vec<PlaneDraw>,
    active: bool,
}

impl PlaneGpu {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("plane shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("plane.wgsl").into()),
        });

        let ubgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("plane uniform bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Snorm8x4, 2 => Unorm8x4];
        let playout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("plane layout"),
            bind_group_layouts: &[&ubgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("plane pipeline"),
            layout: Some(&playout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_plane"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: VERTEX_SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_plane"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let composite_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("plane composite bgl"),
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
        let clayout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("plane composite layout"),
            bind_group_layouts: &[&composite_bgl],
            push_constant_ranges: &[],
        });
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("plane composite"),
            layout: Some(&clayout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_full"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("plane sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        PlaneGpu {
            pipeline,
            composite,
            uniform_bgl: ubgl,
            uniforms: Vec::new(),
            composite_bgl,
            sampler,
            models: Vec::new(),
            format,
            target: None,
            draws: Vec::new(),
            active: false,
        }
    }

    /// Uploads a baked model as slot `index`.
    pub fn add_model(&mut self, device: &wgpu::Device, index: usize, m: &ModelData) {
        let vend = MODEL_HEADER + m.vertices * VERTEX_SIZE;
        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("plane vertices"),
            contents: &m.bytes[MODEL_HEADER..vend],
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("plane indices"),
            contents: &m.bytes[vend..vend + m.indices * 4],
            usage: wgpu::BufferUsages::INDEX,
        });
        if self.models.len() <= index {
            self.models.resize_with(index + 1, || None);
        }
        self.models[index] = Some(ModelGpu { vb, ib, index_count: m.indices as u32 });
    }

    fn ensure_uniforms(&mut self, device: &wgpu::Device, n: usize) {
        while self.uniforms.len() < n {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("plane uniform"),
                size: std::mem::size_of::<Uniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("plane uniform bg"),
                layout: &self.uniform_bgl,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }],
            });
            self.uniforms.push((buf, bg));
        }
    }

    fn make_target(&self, device: &wgpu::Device, size: (u32, u32)) -> Target {
        let extent = wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 };
        let color_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("plane color"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("plane depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let color = color_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("plane composite bg"),
            layout: &self.composite_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&color) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        Target { size, color, depth, bind_group }
    }

    /// Draws all planes into the offscreen texture (or marks them as not shown).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        draws: &[PlaneDraw],
        srgb: bool,
        viewport_px: [u32; 2],
    ) {
        self.active = false;
        self.draws.clear();
        self.draws.extend(draws.iter().copied().filter(|d| matches!(self.models.get(d.model), Some(Some(_)))));
        if self.draws.is_empty() || viewport_px[0] == 0 || viewport_px[1] == 0 {
            return;
        }
        // Supersample 2x for smooth edges unless the screen is already huge.
        let ss = if (viewport_px[0] as u64) * (viewport_px[1] as u64) <= 2_600_000 { 2 } else { 1 };
        let limit = device.limits().max_texture_dimension_2d;
        let size = ((viewport_px[0] * ss).min(limit), (viewport_px[1] * ss).min(limit));
        if self.target.as_ref().map_or(true, |t| t.size != size) {
            self.target = Some(self.make_target(device, size));
        }
        self.ensure_uniforms(device, self.draws.len());
        for (i, d) in self.draws.iter().enumerate() {
            let u = Uniform { mvp: d.mvp, nrm: d.nrm, flags: [if srgb { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0] };
            // SAFETY: `Uniform` is repr(C) and made only of f32, so it has no padding.
            let bytes =
                unsafe { std::slice::from_raw_parts(&u as *const Uniform as *const u8, std::mem::size_of::<Uniform>()) };
            queue.write_buffer(&self.uniforms[i].0, 0, bytes);
        }

        let target = self.target.as_ref().unwrap();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("plane pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.color,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        for (i, d) in self.draws.iter().enumerate() {
            let Some(Some(m)) = self.models.get(d.model) else { continue };
            pass.set_bind_group(0, &self.uniforms[i].1, &[]);
            pass.set_vertex_buffer(0, m.vb.slice(..));
            pass.set_index_buffer(m.ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..m.index_count, 0, 0..1);
        }
        drop(pass);
        self.active = true;
    }

    /// Lays the rendered plane over the map (inside egui's pass, viewport = the map rect).
    pub fn paint(&self, pass: &mut wgpu::RenderPass<'static>) {
        if !self.active {
            return;
        }
        if let Some(t) = &self.target {
            pass.set_pipeline(&self.composite);
            pass.set_bind_group(0, &t.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

pub fn insert(rs: &egui_wgpu::RenderState) {
    rs.renderer.write().callback_resources.insert(PlaneGpu::new(&rs.device, rs.target_format));
}

/// Uploads a model into slot `index` of the shared renderer.
pub fn upload(rs: &egui_wgpu::RenderState, index: usize, model: &ModelData) {
    if let Some(gpu) = rs.renderer.write().callback_resources.get_mut::<PlaneGpu>() {
        gpu.add_model(&rs.device, index, model);
    }
}

/// Paint callback for all aircraft of one frame; add it after everything the planes must cover.
pub struct PlaneCallback {
    pub draws: Vec<PlaneDraw>,
    pub srgb: bool,
    /// Size of the map rectangle in physical pixels.
    pub viewport_px: [u32; 2],
}

impl egui_wgpu::CallbackTrait for PlaneCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(gpu) = resources.get_mut::<PlaneGpu>() {
            gpu.prepare(device, queue, encoder, &self.draws, self.srgb, self.viewport_px);
        }
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        if let Some(gpu) = resources.get::<PlaneGpu>() {
            gpu.paint(pass);
        }
    }
}
