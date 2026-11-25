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

/// Reinterprets a slice of plain `repr(C)` data (no padding) as bytes.
fn as_bytes<T: Copy>(s: &[T]) -> &[u8] {
    // SAFETY: only used with padding-free repr(C) types made of f32/u32.
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}

/// Per-frame uniforms. Layout must match `Globals` in shader.wgsl (1120 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Globals {
    pub view_proj: [f32; 16],
    /// xyz = camera position (planet radii), w = 1.0 when the target is sRGB.
    pub cam_pos: [f32; 4],
    /// xy = viewport size in physical pixels.
    pub screen: [f32; 4],
    pub styles: [[f32; 8]; style::TABLE_SIZE],
}

const FILL_ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Uint32];
const LINE_ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x4, 1 => Uint32];
const BASE_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];

struct TileGpu {
    _uniform: wgpu::Buffer,
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
        }
    }

    /// Uploads a freshly built tile. Cheap: three small buffers and a bind group.
    pub fn add_tile(&mut self, device: &wgpu::Device, id: TileId, mesh: &TileMesh) {
        let (ox, oy, size) = id.origin_size();
        let uniform_data: [f32; 4] = [ox as f32, oy as f32, size as f32, mesh::EXTENT];
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tile uniform"),
            contents: as_bytes(&uniform_data),
            usage: wgpu::BufferUsages::UNIFORM,
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
        self.tiles.insert(
            id,
            TileGpu {
                _uniform: uniform,
                bind_group,
                fill,
                fill_count: mesh.fills.len() as u32,
                line,
                line_count: mesh.lines.len() as u32,
            },
        );
    }

    pub fn remove_tile(&mut self, id: &TileId) {
        self.tiles.remove(id);
    }
}

/// One frame's worth of drawing: camera uniforms plus the tiles to draw,
/// ordered from lowest zoom to highest so detailed tiles paint over coarse ones.
pub struct MapCallback {
    pub globals: Globals,
    pub draw: Vec<TileId>,
}

impl egui_wgpu::CallbackTrait for MapCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(gpu) = callback_resources.get_mut::<MapGpu>() {
            queue.write_buffer(&gpu.globals_buf, 0, as_bytes(std::slice::from_ref(&self.globals)));
        }
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

        // 1. Land-coloured planet.
        render_pass.set_pipeline(&gpu.base_pipeline);
        render_pass.set_bind_group(0, &gpu.globals_bg, &[]);
        render_pass.set_vertex_buffer(0, gpu.base_vb.slice(..));
        render_pass.set_index_buffer(gpu.base_ib.slice(..), wgpu::IndexFormat::Uint32);
        render_pass.draw_indexed(0..gpu.base_icount, 0, 0..1);

        // 2. All polygon fills (coarse tiles first).
        render_pass.set_pipeline(&gpu.fill_pipeline);
        render_pass.set_bind_group(0, &gpu.globals_bg, &[]);
        for id in &self.draw {
            if let Some(t) = gpu.tiles.get(id) {
                if let Some(vb) = &t.fill {
                    render_pass.set_bind_group(1, &t.bind_group, &[]);
                    render_pass.set_vertex_buffer(0, vb.slice(..));
                    render_pass.draw(0..t.fill_count, 0..1);
                }
            }
        }

        // 3. All lines on top.
        render_pass.set_pipeline(&gpu.line_pipeline);
        render_pass.set_bind_group(0, &gpu.globals_bg, &[]);
        for id in &self.draw {
            if let Some(t) = gpu.tiles.get(id) {
                if let Some(vb) = &t.line {
                    render_pass.set_bind_group(1, &t.bind_group, &[]);
                    render_pass.set_vertex_buffer(0, vb.slice(..));
                    render_pass.draw(0..6, 0..t.line_count);
                }
            }
        }
    }
}
