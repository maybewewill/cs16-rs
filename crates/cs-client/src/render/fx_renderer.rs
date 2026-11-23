//! Lightweight flat-colour 3D FX renderer for the beams and tracers of `TE_*` messages and for
//! point glows (dynamic lights, particles). The temp entities that carry a sprite or a studio
//! model go through the scene API instead; textured beams and decals are not drawn yet.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FxVertex {
    position: [f32; 3],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FxUniforms {
    view_proj: [[f32; 4]; 4],
}

/// A ribbon between two points, facing the camera.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxBeam {
    pub start: Vec3,
    pub end: Vec3,
    /// Full width in world units.
    pub width: f32,
    /// Straight (not premultiplied) RGBA.
    pub color: [f32; 4],
}

/// A camera-facing square.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxPoint {
    pub origin: Vec3,
    /// Half the side in world units.
    pub size: f32,
    pub color: [f32; 4],
}

pub struct FxRenderer {
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    vertex_buf: wgpu::Buffer,
    vertex_cap: usize,
    vertex_count: u32,
}

impl FxRenderer {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fx.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/fx.wgsl").into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx_bgl"),
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

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fx_layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fx_pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<FxVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fx_uniform_buf"),
            contents: bytemuck::bytes_of(&FxUniforms {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fx_uniform_bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let vertex_cap = 8192;
        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fx_vbuf"),
            size: (vertex_cap * std::mem::size_of::<FxVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            uniform_buf,
            uniform_bg,
            vertex_buf,
            vertex_cap,
            vertex_count: 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view_proj: Mat4,
        cam_pos: Vec3,
        cam_right: Vec3,
        cam_up: Vec3,
        beams: &[FxBeam],
        points: &[FxPoint],
    ) {
        let mut verts: Vec<FxVertex> = Vec::new();

        for b in beams {
            let dir = (b.end - b.start).normalize_or_zero();
            let to_cam = (cam_pos - b.start).normalize_or_zero();
            let side = dir.cross(to_cam).normalize_or_zero() * (b.width * 0.5);
            push_raw_quad(
                &mut verts,
                b.start - side,
                b.start + side,
                b.end + side,
                b.end - side,
                b.color,
            );
        }

        for p in points {
            push_quad(
                &mut verts,
                p.origin,
                cam_right * p.size,
                cam_up * p.size,
                p.color,
            );
        }

        queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::bytes_of(&FxUniforms {
                view_proj: view_proj.to_cols_array_2d(),
            }),
        );

        if verts.len() > self.vertex_cap {
            self.vertex_cap = verts.len().next_power_of_two();
            self.vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fx_vbuf"),
                size: (self.vertex_cap * std::mem::size_of::<FxVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !verts.is_empty() {
            queue.write_buffer(&self.vertex_buf, 0, bytemuck::cast_slice(&verts));
        }
        self.vertex_count = verts.len() as u32;
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.vertex_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniform_bg, &[]);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }
}

fn push_quad(verts: &mut Vec<FxVertex>, center: Vec3, right: Vec3, up: Vec3, color: [f32; 4]) {
    push_raw_quad(
        verts,
        center - right - up,
        center + right - up,
        center + right + up,
        center - right + up,
        color,
    );
}

fn push_raw_quad(
    verts: &mut Vec<FxVertex>,
    p0: Vec3,
    p1: Vec3,
    p2: Vec3,
    p3: Vec3,
    color: [f32; 4],
) {
    verts.push(FxVertex { position: p0.to_array(), color });
    verts.push(FxVertex { position: p1.to_array(), color });
    verts.push(FxVertex { position: p2.to_array(), color });
    verts.push(FxVertex { position: p0.to_array(), color });
    verts.push(FxVertex { position: p2.to_array(), color });
    verts.push(FxVertex { position: p3.to_array(), color });
}
