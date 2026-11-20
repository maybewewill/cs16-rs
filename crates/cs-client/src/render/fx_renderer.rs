//! Lightweight 3D FX renderer for bullet decals, tracers, smoke clouds, muzzleflashes, and blinking C4 LED.

use bytemuck::{Pod, Zeroable};
use cs_sim::grenades::{ActiveGrenade, GrenadeKind, SmokeCloud};
use cs_sim::rules::{BulletTracer, ImpactDecal, PlantedC4};
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
        decals: &[ImpactDecal],
        tracers: &[BulletTracer],
        grenades: &[ActiveGrenade],
        smokes: &[SmokeCloud],
        planted_c4: Option<&PlantedC4>,
        muzzle_flashes: &[Vec3],
        time_sec: f32,
    ) {
        let mut verts: Vec<FxVertex> = Vec::new();

        // 1. Bullet impact decals, blood splatters, and classic orange CS 1.6 Lambda sprays!
        for d in decals {
            let n = d.normal.normalize_or_zero();
            let ref_up = if n.z.abs() > 0.9 { Vec3::X } else { Vec3::Z };
            let t1 = n.cross(ref_up).normalize();
            let t2 = n.cross(t1).normalize();
            if d.is_spray {
                // Iconic CS 1.6 orange Lambda spray logo (`impulse 201`) on wall
                let bg = [0.06, 0.05, 0.04, 0.78];
                let orange = [1.0, 0.56, 0.04, 0.96];
                push_quad(&mut verts, d.position, t1 * 22.0, t2 * 22.0, bg);
                // Frame bars
                push_quad(&mut verts, d.position + n * 0.1 + t2 * 19.5, t1 * 21.0, t2 * 1.5, orange);
                push_quad(&mut verts, d.position + n * 0.1 - t2 * 19.5, t1 * 21.0, t2 * 1.5, orange);
                push_quad(&mut verts, d.position + n * 0.1 + t1 * 19.5, t1 * 1.5, t2 * 21.0, orange);
                push_quad(&mut verts, d.position + n * 0.1 - t1 * 19.5, t1 * 1.5, t2 * 21.0, orange);
                // Geometric Lambda (λ) legs
                let leg_r = (t1 * 0.55 - t2 * 0.85).normalize();
                let leg_r_perp = n.cross(leg_r);
                push_quad(&mut verts, d.position + n * 0.15 + t1 * 2.0, leg_r * 13.0, leg_r_perp * 2.2, orange);
                let leg_l = (-t1 * 0.55 - t2 * 0.85).normalize();
                let leg_l_perp = n.cross(leg_l);
                push_quad(&mut verts, d.position + n * 0.15 - t1 * 4.0 - t2 * 4.0, leg_l * 8.5, leg_l_perp * 2.0, orange);
            } else {
                let half = if d.is_blood { 5.5 } else { 2.4 };
                let color = if d.is_blood {
                    [0.45, 0.04, 0.04, 0.85]
                } else {
                    [0.06, 0.06, 0.06, 0.88]
                };
                push_quad(&mut verts, d.position, t1 * half, t2 * half, color);
            }
        }

        // 2. Bullet tracers (thin bright ribbons)
        for tr in tracers {
            let dir = (tr.end - tr.start).normalize_or_zero();
            let to_cam = (cam_pos - tr.start).normalize_or_zero();
            let side = dir.cross(to_cam).normalize_or_zero() * 0.65;
            let a = (tr.remaining / 0.06).clamp(0.0, 1.0) * 0.75;
            let col = [1.0, 0.88, 0.45, a];
            let p0 = tr.start - side;
            let p1 = tr.start + side;
            let p2 = tr.end + side;
            let p3 = tr.end - side;
            push_raw_quad(&mut verts, p0, p1, p2, p3, col);
        }

        // 3. Flying grenades (compact billboarded indicators)
        for g in grenades {
            let col = match g.kind {
                GrenadeKind::He => [0.28, 0.36, 0.22, 1.0],
                GrenadeKind::Flashbang => [0.75, 0.78, 0.82, 1.0],
                GrenadeKind::Smoke => [0.35, 0.42, 0.35, 1.0],
            };
            push_quad(&mut verts, g.position, cam_right * 5.0, cam_up * 5.0, col);
        }

        // 4. Smoke grenade volumetric puff clouds
        for s in smokes {
            let fade = (s.remaining / 3.0).clamp(0.0, 1.0);
            let offsets = [
                Vec3::new(0.0, 0.0, 10.0),
                Vec3::new(42.0, 20.0, 28.0),
                Vec3::new(-40.0, 25.0, 24.0),
                Vec3::new(22.0, -38.0, 18.0),
                Vec3::new(-28.0, -32.0, 32.0),
                Vec3::new(0.0, 0.0, 56.0),
            ];
            for (idx, off) in offsets.iter().enumerate() {
                let drift = ((time_sec * 0.6 + idx as f32).sin()) * 6.0;
                let center = s.position + *off + Vec3::new(drift, -drift * 0.5, 0.0);
                let r = s.radius * 0.52;
                let col = [0.76, 0.78, 0.76, 0.62 * fade];
                push_quad(&mut verts, center, cam_right * r, cam_up * r, col);
            }
        }

        // 5. Planted C4 blinking red LED
        if let Some(c4) = planted_c4 {
            let blink_hz = if c4.timer < 5.0 { 8.0 } else if c4.timer < 15.0 { 3.5 } else { 1.4 };
            let blink = ((time_sec * blink_hz * std::f32::consts::TAU).sin() > 0.2) as u8;
            // Base package block
            push_quad(
                &mut verts,
                c4.position + Vec3::Z * 4.0,
                cam_right * 8.0,
                cam_up * 6.0,
                [0.45, 0.43, 0.32, 0.95],
            );
            if blink != 0 {
                push_quad(
                    &mut verts,
                    c4.position + Vec3::Z * 8.0,
                    cam_right * 7.5,
                    cam_up * 7.5,
                    [1.0, 0.12, 0.08, 0.95],
                );
            }
        }

        // 6. Muzzle flashes
        for &mf in muzzle_flashes {
            push_quad(
                &mut verts,
                mf,
                cam_right * 11.0,
                cam_up * 11.0,
                [1.0, 0.82, 0.32, 0.85],
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
