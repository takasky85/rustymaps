//! wgpu resources and the egui paint callback that draws the globe.
//!
//! The render thread only ever *draws* data that is already on the GPU: tile
//! meshes are built on worker threads and uploaded once when they arrive.

use std::collections::HashMap;

use eframe::wgpu::util::DeviceExt;
use eframe::{egui, egui_wgpu, wgpu};

use crate::camera::lonlat_to_vec;
use crate::mesh::{self, FillVertex, LineInst, TileMesh};
use crate::style;
use crate::tile_id::TileId;

/// How many times the map image has been redrawn (shown in the top bar for diagnosis).
pub static MAP_REDRAWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Reinterprets a slice of plain `repr(C)` data (no padding) as bytes.
fn as_bytes<T: Copy>(s: &[T]) -> &[u8] {
    // SAFETY: only used with padding-free repr(C) types made of f32/u32.
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}

/// Per-frame uniforms. Layout must match `Globals` in shader.wgsl (1216 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Globals {
    /// Absolute world -> clip; only the base sphere uses it.
    pub view_proj: [f32; 16],
    /// Target-relative (east, north, radial) -> clip; tiles use it.
    pub proj: [f32; 16],
    /// xyz = camera position (planet radii), w = 1.0 when the target is sRGB.
    pub cam_pos: [f32; 4],
    /// x = cos(target latitude), y = sin(target latitude), z = alt*cos(pitch), w = alt*sin(pitch).
    pub cam_rel: [f32; 4],
    /// x = sin(heading), y = cos(heading).
    pub head: [f32; 4],
    /// xy = viewport size in physical pixels.
    pub screen: [f32; 4],
    pub styles: [[f32; 8]; style::TABLE_SIZE],
}

const FILL_ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Uint32];
const LINE_ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x4, 1 => Uint32];
const BASE_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];

struct TileGpu {
    /// Updated every frame with the tile's offset from the camera.
    uniform: wgpu::Buffer,
    origin: (f64, f64),
    unit: f64,
    bind_group: wgpu::BindGroup,
    fill: Option<wgpu::Buffer>,
    fill_count: u32,
    line: Option<wgpu::Buffer>,
    line_count: u32,
}

/// Everything the paint callback needs; lives in egui's callback resources.
pub struct MapGpu {
    base_pipeline: wgpu::RenderPipeline,
    fill_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    tile_bgl: wgpu::BindGroupLayout,
    base_vb: wgpu::Buffer,
    base_ib: wgpu::Buffer,
    base_icount: u32,
    tiles: HashMap<TileId, TileGpu>,
    /// Bumped whenever a tile is added or removed (part of the cache key).
    generation: u64,
    format: wgpu::TextureFormat,
    blit: wgpu::RenderPipeline,
    blit_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The map as last rendered; it is only redrawn when `key` changes.
    cache: Option<Cache>,
    key: Vec<u8>,
    key_draw: Vec<TileId>,
}

struct Cache {
    size: (u32, u32),
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

/// A coarse lat/lon sphere in the land colour, painted under all tile data.
fn base_sphere() -> (Vec<[f32; 3]>, Vec<u32>) {
    const NLAT: u32 = 90;
    const NLON: u32 = 180;
    let mut verts = Vec::new();
    for i in 0..=NLAT {
        let lat = (-90.0 + 180.0 * i as f64 / NLAT as f64).to_radians();
        for j in 0..=NLON {
            let lon = (-180.0 + 360.0 * j as f64 / NLON as f64).to_radians();
            let p = lonlat_to_vec(lon, lat);
            verts.push([p[0] as f32, p[1] as f32, p[2] as f32]);
        }
    }
    let mut idx = Vec::new();
    for i in 0..NLAT {
        for j in 0..NLON {
            let a = i * (NLON + 1) + j;
            let b = a + 1;
            let c = a + (NLON + 1);
            let d = c + 1;
            idx.extend([a, c, b, b, c, d]);
        }
    }
    (verts, idx)
}

impl MapGpu {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("map shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals bgl"),
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
        let tile_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tile bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let base_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("base layout"),
            bind_group_layouts: &[&globals_bgl],
            push_constant_ranges: &[],
        });
        let tile_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tile layout"),
            bind_group_layouts: &[&globals_bgl, &tile_bgl],
            push_constant_ranges: &[],
        });

        let make = |label: &str,
                    layout: &wgpu::PipelineLayout,
                    vs: &str,
                    fs: &str,
                    buffers: &[wgpu::VertexBufferLayout]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                // egui's pass has no depth buffer unless asked for; we do not use one.
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };

        let base_pipeline = make(
            "base pipeline",
            &base_layout,
            "vs_base",
            "fs_base",
            &[wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &BASE_ATTRS,
            }],
        );
        let fill_pipeline = make(
            "fill pipeline",
            &tile_layout,
            "vs_fill",
            "fs_fill",
            &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<FillVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &FILL_ATTRS,
            }],
        );
        let line_pipeline = make(
            "line pipeline",
            &tile_layout,
            "vs_line",
            "fs_line",
            &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<LineInst>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &LINE_ATTRS,
            }],
        );

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as wgpu::BufferAddress,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals bg"),
            layout: &globals_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() }],
        });

        let (verts, indices) = base_sphere();
        let base_vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("base vertices"),
            contents: as_bytes(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let base_ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("base indices"),
            contents: as_bytes(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Cached-image blit.
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("map blit shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
        });
        let blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("map blit bgl"),
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
        let blit_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("map blit layout"),
            bind_group_layouts: &[&blit_bgl],
            push_constant_ranges: &[],
        });
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("map blit"),
            layout: Some(&blit_layout),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs_blit"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs_blit"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
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
            label: Some("map blit sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        MapGpu {
            base_pipeline,
            fill_pipeline,
            line_pipeline,
            globals_buf,
            globals_bg,
            tile_bgl,
            base_vb,
            base_ib,
            base_icount: indices.len() as u32,
            tiles: HashMap::new(),
            generation: 0,
            format: target_format,
            blit,
            blit_bgl,
            sampler,
            cache: None,
            key: Vec::new(),
            key_draw: Vec::new(),
        }
    }

    /// Uploads a freshly built tile. Cheap: three small buffers and a bind group.
    pub fn add_tile(&mut self, device: &wgpu::Device, id: TileId, mesh: &TileMesh) {
        let (ox, oy, size) = id.origin_size();
        let uniform_data: [f32; 4] = [0.0; 4];
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tile uniform"),
            contents: as_bytes(&uniform_data),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tile bg"),
            layout: &self.tile_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let fill = (!mesh.fills.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("tile fills"),
                contents: as_bytes(&mesh.fills),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        let line = (!mesh.lines.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("tile lines"),
                contents: as_bytes(&mesh.lines),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        self.generation += 1;
        self.tiles.insert(
            id,
            TileGpu {
                uniform,
                origin: (ox, oy),
                unit: size / mesh::EXTENT as f64,
                bind_group,
                fill,
                fill_count: mesh.fills.len() as u32,
                line,
                line_count: mesh.lines.len() as u32,
            },
        );
    }

    pub fn remove_tile(&mut self, id: &TileId) {
        if self.tiles.remove(id).is_some() {
            self.generation += 1;
        }
    }

    fn make_cache(&self, device: &wgpu::Device, size: (u32, u32)) -> Cache {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("map cache"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("map cache bg"),
            layout: &self.blit_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        Cache { size, view, bind_group }
    }
}

/// One frame's worth of drawing: camera uniforms plus the tiles to draw,
/// ordered from lowest zoom to highest so detailed tiles paint over coarse ones.
///
/// The map is drawn into an offscreen image that is kept between frames: when only the overlays
/// or the planes change (trains, aircraft, UI), the GPU just copies that image instead of
/// re-drawing every tile.
pub struct MapCallback {
    pub globals: Globals,
    /// Camera position in normalized mercator (0..1), full f64 precision.
    pub cam_merc: (f64, f64),
    pub draw: Vec<TileId>,
    /// Size of the map rectangle in physical pixels.
    pub viewport_px: [u32; 2],
}

impl MapCallback {
    /// Everything that changes the picture, as bytes.
    fn key(&self, generation: u64) -> Vec<u8> {
        let mut k = Vec::with_capacity(std::mem::size_of::<Globals>() + 40);
        k.extend_from_slice(as_bytes(std::slice::from_ref(&self.globals)));
        k.extend_from_slice(&self.cam_merc.0.to_le_bytes());
        k.extend_from_slice(&self.cam_merc.1.to_le_bytes());
        k.extend_from_slice(&generation.to_le_bytes());
        k.extend_from_slice(&self.viewport_px[0].to_le_bytes());
        k.extend_from_slice(&self.viewport_px[1].to_le_bytes());
        k
    }
}

impl egui_wgpu::CallbackTrait for MapCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(gpu) = callback_resources.get_mut::<MapGpu>() else {
            return Vec::new();
        };
        let limit = device.limits().max_texture_dimension_2d;
        let size = (self.viewport_px[0].clamp(1, limit), self.viewport_px[1].clamp(1, limit));
        let size_changed = gpu.cache.as_ref().map_or(true, |c| c.size != size);
        if size_changed {
            gpu.cache = Some(gpu.make_cache(device, size));
        }
        let key = self.key(gpu.generation);
        if !size_changed && key == gpu.key && self.draw == gpu.key_draw {
            return Vec::new(); // nothing moved: reuse the picture from last time
        }
        MAP_REDRAWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        gpu.key = key;
        gpu.key_draw.clear();
        gpu.key_draw.extend_from_slice(&self.draw);

        queue.write_buffer(&gpu.globals_buf, 0, as_bytes(std::slice::from_ref(&self.globals)));
        // Tile origin minus camera, subtracted in f64 so the f32 result is tiny and exact
        // however far we zoom in.
        let (cx, cy) = self.cam_merc;
        for id in &self.draw {
            if let Some(t) = gpu.tiles.get(id) {
                let mut dx = t.origin.0 - cx;
                dx -= dx.round(); // shortest way around the antimeridian
                let dy = t.origin.1 - cy;
                let data: [f32; 4] = [dx as f32, dy as f32, t.unit as f32, 0.0];
                queue.write_buffer(&t.uniform, 0, as_bytes(&data));
            }
        }

        let Some(cache) = gpu.cache.as_ref() else {
            return Vec::new();
        };
        let mut pass = egui_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("map pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &cache.view,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        // 1. Land-coloured planet.
        pass.set_pipeline(&gpu.base_pipeline);
        pass.set_bind_group(0, &gpu.globals_bg, &[]);
        pass.set_vertex_buffer(0, gpu.base_vb.slice(..));
        pass.set_index_buffer(gpu.base_ib.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..gpu.base_icount, 0, 0..1);

        // 2. All polygon fills (coarse tiles first).
        pass.set_pipeline(&gpu.fill_pipeline);
        pass.set_bind_group(0, &gpu.globals_bg, &[]);
        for id in &self.draw {
            if let Some(t) = gpu.tiles.get(id) {
                if let Some(vb) = &t.fill {
                    pass.set_bind_group(1, &t.bind_group, &[]);
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.draw(0..t.fill_count, 0..1);
                }
            }
        }

        // 3. All lines on top.
        pass.set_pipeline(&gpu.line_pipeline);
        pass.set_bind_group(0, &gpu.globals_bg, &[]);
        for id in &self.draw {
            if let Some(t) = gpu.tiles.get(id) {
                if let Some(vb) = &t.line {
                    pass.set_bind_group(1, &t.bind_group, &[]);
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.draw(0..6, 0..t.line_count);
                }
            }
        }
        drop(pass);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(gpu) = callback_resources.get::<MapGpu>() else {
            return;
        };
        if let Some(cache) = &gpu.cache {
            render_pass.set_pipeline(&gpu.blit);
            render_pass.set_bind_group(0, &cache.bind_group, &[]);
            render_pass.draw(0..3, 0..1);
        }
    }
}
