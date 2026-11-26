//! Flat-colour 3D FX renderer supporting alpha & additive particles, tracers, beams, rings,
//! lightning, weather, `GL_EXP2` fog, gamma/software-mode dithering, and MSAA (`1 | 4`).

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
    camera_pos: [f32; 4],
    fog_color_density: [f32; 4],
    gamma_params: [f32; 4],
}

/// Parameters for a [`FxRenderer::prepare`] frame.
#[derive(Debug, Clone, Copy)]
pub struct FxDrawParams {
    pub view_proj: Mat4,
    pub cam_pos: Vec3,
    pub cam_right: Vec3,
    pub cam_up: Vec3,
    pub gamma: f32,
    pub brightness: f32,
    pub software_mode: bool,
    pub fog: Option<([f32; 3], f32)>,
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
    pub additive: bool,
}

impl Default for FxBeam {
    fn default() -> Self {
        Self {
            start: Vec3::ZERO,
            end: Vec3::X,
            width: 1.0,
            color: [1.0; 4],
            additive: true,
        }
    }
}

/// A camera-facing square.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxPoint {
    pub origin: Vec3,
    /// Half the side in world units.
    pub size: f32,
    pub color: [f32; 4],
    pub additive: bool,
}

impl Default for FxPoint {
    fn default() -> Self {
        Self {
            origin: Vec3::ZERO,
            size: 1.0,
            color: [1.0; 4],
            additive: false,
        }
    }
}

pub struct FxRenderer {
    color_format: wgpu::TextureFormat,
    msaa_samples: u32,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    alpha_pipeline: wgpu::RenderPipeline,
    additive_pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    vertex_buf: wgpu::Buffer,
    vertex_cap: usize,
    alpha_count: u32,
    additive_count: u32,
    scratch_alpha_verts: Vec<FxVertex>,
    scratch_add_verts: Vec<FxVertex>,
    frame_upload_bytes: u64,
}

fn create_fx_pipelines(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    color_format: wgpu::TextureFormat,
    sample_count: u32,
) -> (wgpu::RenderPipeline, wgpu::RenderPipeline) {
    let add_blend = Some(wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    });
    let make = |label: &str, blend: Option<wgpu::BlendState>| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<FxVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend,
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
            multisample: wgpu::MultisampleState {
                count: sample_count.max(1),
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        })
    };
    (
        make("fx_alpha_pipeline", Some(wgpu::BlendState::ALPHA_BLENDING)),
        make("fx_additive_pipeline", add_blend),
    )
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
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fx_layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let msaa_samples = 1;
        let (alpha_pipeline, additive_pipeline) =
            create_fx_pipelines(device, &shader, &pipeline_layout, color_format, msaa_samples);

        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fx_uniform_buf"),
            contents: bytemuck::bytes_of(&FxUniforms {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                camera_pos: [0.0, 0.0, 0.0, 1.0],
                fog_color_density: [0.0, 0.0, 0.0, 0.0],
                gamma_params: [2.5, 2.0, 1.0, 0.0],
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
            color_format,
            msaa_samples,
            shader,
            pipeline_layout,
            alpha_pipeline,
            additive_pipeline,
            uniform_buf,
            uniform_bg,
            vertex_buf,
            vertex_cap,
            alpha_count: 0,
            additive_count: 0,
            scratch_alpha_verts: Vec::new(),
            scratch_add_verts: Vec::new(),
            frame_upload_bytes: 0,
        }
    }

    pub(crate) fn take_upload_bytes(&mut self) -> u64 {
        std::mem::take(&mut self.frame_upload_bytes)
    }

    pub fn rebuild_pipelines(&mut self, device: &wgpu::Device, msaa_samples: u32) {
        let samples = msaa_samples.max(1);
        if self.msaa_samples == samples {
            return;
        }
        self.msaa_samples = samples;
        let (alpha, additive) = create_fx_pipelines(
            device,
            &self.shader,
            &self.pipeline_layout,
            self.color_format,
            samples,
        );
        self.alpha_pipeline = alpha;
        self.additive_pipeline = additive;
    }

    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        params: &FxDrawParams,
        beams: &[FxBeam],
        points: &[FxPoint],
    ) {
        self.scratch_alpha_verts.clear();
        self.scratch_add_verts.clear();

        for b in beams {
            let dir = (b.end - b.start).normalize_or_zero();
            let to_cam = (params.cam_pos - b.start).normalize_or_zero();
            let side = dir.cross(to_cam).normalize_or_zero() * (b.width * 0.5);
            let dst = if b.additive {
                &mut self.scratch_add_verts
            } else {
                &mut self.scratch_alpha_verts
            };
            push_raw_quad(
                dst,
                b.start - side,
                b.start + side,
                b.end + side,
                b.end - side,
                b.color,
            );
        }

        for p in points {
            let dst = if p.additive {
                &mut self.scratch_add_verts
            } else {
                &mut self.scratch_alpha_verts
            };
            push_quad(
                dst,
                p.origin,
                params.cam_right * p.size,
                params.cam_up * p.size,
                p.color,
            );
        }

        let (fog_rgb, fog_density) = params.fog.unwrap_or(([0.0, 0.0, 0.0], 0.0));
        queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::bytes_of(&FxUniforms {
                view_proj: params.view_proj.to_cols_array_2d(),
                camera_pos: [params.cam_pos.x, params.cam_pos.y, params.cam_pos.z, 1.0],
                fog_color_density: [fog_rgb[0], fog_rgb[1], fog_rgb[2], fog_density],
                gamma_params: [
                    params.gamma,
                    2.0,
                    params.brightness,
                    if params.software_mode { 1.0 } else { 0.0 },
                ],
            }),
        );
        self.frame_upload_bytes += std::mem::size_of::<FxUniforms>() as u64;

        self.alpha_count = self.scratch_alpha_verts.len() as u32;
        self.additive_count = self.scratch_add_verts.len() as u32;
        self.scratch_alpha_verts
            .extend_from_slice(&self.scratch_add_verts);

        if self.scratch_alpha_verts.len() > self.vertex_cap {
            self.vertex_cap = self.scratch_alpha_verts.len().next_power_of_two();
            self.vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fx_vbuf"),
                size: (self.vertex_cap * std::mem::size_of::<FxVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !self.scratch_alpha_verts.is_empty() {
            queue.write_buffer(
                &self.vertex_buf,
                0,
                bytemuck::cast_slice(&self.scratch_alpha_verts),
            );
            self.frame_upload_bytes +=
                (self.scratch_alpha_verts.len() * std::mem::size_of::<FxVertex>()) as u64;
        }
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        let _ = self.draw_counted(pass);
    }

    pub(crate) fn draw_counted<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) -> usize {
        if self.alpha_count == 0 && self.additive_count == 0 {
            return 0;
        }
        let mut draw_calls = 0usize;
        pass.set_bind_group(0, &self.uniform_bg, &[]);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        if self.alpha_count > 0 {
            pass.set_pipeline(&self.alpha_pipeline);
            pass.draw(0..self.alpha_count, 0..1);
            draw_calls += 1;
        }
        if self.additive_count > 0 {
            pass.set_pipeline(&self.additive_pipeline);
            pass.draw(self.alpha_count..self.alpha_count + self.additive_count, 0..1);
            draw_calls += 1;
        }
        draw_calls
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
