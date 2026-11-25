//! GoldSrc 6-face skybox renderer (`gfx/env/<skyname>{rt,bk,lf,ft,up,dn}.tga`).

use bytemuck::{Pod, Zeroable};
use cs_assets::scheme::SkyboxTextures;
use glam::Mat4;
use wgpu::util::DeviceExt;

use crate::render::lighting::{TextureFilterMode, create_sampler};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SkyVertex {
    position: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SkyUniforms {
    view_proj: [[f32; 4]; 4],
    params: [f32; 4],            // gamma, texgamma, brightness, software_mode
    fog_color_density: [f32; 4], // rgb, density
}

pub struct SkyboxRenderer {
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: wgpu::RenderPipeline,
    color_format: wgpu::TextureFormat,
    msaa_samples: u32,
    uniform_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    face_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    filter_mode: TextureFilterMode,
    vertex_buf: wgpu::Buffer,
    face_views: Vec<wgpu::TextureView>,
    face_bgs: Vec<wgpu::BindGroup>,
}

impl SkyboxRenderer {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("skybox.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/skybox.wgsl").into()),
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky_uniform_bgl"),
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

        let face_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky_face_bgl"),
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("skybox_pipeline_layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&face_bgl)],
            immediate_size: 0,
        });

        let msaa_samples = 1;
        let pipeline = build_sky_pipeline(device, &pipeline_layout, &shader, color_format, msaa_samples);

        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sky_uniform_buf"),
            contents: bytemuck::bytes_of(&SkyUniforms {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                params: [2.5, 2.0, 1.0, 0.0],
                fog_color_density: [0.0; 4],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky_uniform_bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });

        let filter_mode = TextureFilterMode::Linear;
        let sampler = create_sampler(device, "sky_sampler", filter_mode, 1, true);

        let verts = build_goldsrc_sky_cube();
        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sky_vertex_buf"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });

        Self {
            shader,
            pipeline_layout,
            pipeline,
            color_format,
            msaa_samples,
            uniform_buf,
            uniform_bg,
            face_bgl,
            sampler,
            filter_mode,
            vertex_buf,
            face_views: Vec::new(),
            face_bgs: Vec::new(),
        }
    }

    pub fn rebuild_pipelines(&mut self, device: &wgpu::Device, msaa_samples: u32) {
        let samples = if msaa_samples >= 4 { 4 } else { 1 };
        if self.msaa_samples == samples {
            return;
        }
        self.msaa_samples = samples;
        self.pipeline = build_sky_pipeline(
            device,
            &self.pipeline_layout,
            &self.shader,
            self.color_format,
            samples,
        );
    }

    pub fn set_texture_filter(&mut self, device: &wgpu::Device, mode: TextureFilterMode) {
        let mapped = match mode {
            TextureFilterMode::Nearest
            | TextureFilterMode::NearestMipmapNearest
            | TextureFilterMode::NearestMipmapLinear => TextureFilterMode::Nearest,
            _ => TextureFilterMode::Linear,
        };
        if self.filter_mode == mapped {
            return;
        }
        self.filter_mode = mapped;
        self.sampler = create_sampler(device, "sky_sampler", mapped, 1, true);
        self.face_bgs = self
            .face_views
            .iter()
            .enumerate()
            .map(|(idx, view)| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("sky_face_bg_{idx}")),
                    layout: &self.face_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                })
            })
            .collect();
    }

    pub fn upload_skybox(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, sky: &SkyboxTextures) {
        self.face_views.clear();
        self.face_bgs.clear();
        for (idx, face) in sky.faces.iter().enumerate() {
            let size = wgpu::Extent3d {
                width: face.width.max(1),
                height: face.height.max(1),
                depth_or_array_layers: 1,
            };
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(&format!("sky_face_{idx}")),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &face.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * face.width.max(1)),
                    rows_per_image: Some(face.height.max(1)),
                },
                size,
            );
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("sky_face_bg_{idx}")),
                layout: &self.face_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.face_views.push(view);
            self.face_bgs.push(bg);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw<'a>(
        &'a self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        sky_view_proj: Mat4,
        gamma: f32,
        texgamma: f32,
        brightness: f32,
        software_mode: bool,
        fog: Option<([f32; 3], f32)>,
    ) {
        let _ = self.draw_counted(
            queue,
            pass,
            sky_view_proj,
            gamma,
            texgamma,
            brightness,
            software_mode,
            fog,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_counted<'a>(
        &'a self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        sky_view_proj: Mat4,
        gamma: f32,
        texgamma: f32,
        brightness: f32,
        software_mode: bool,
        fog: Option<([f32; 3], f32)>,
    ) -> (usize, u64) {
        if self.face_bgs.len() != 6 {
            return (0, 0);
        }
        let (fog_rgb, fog_density) = fog.unwrap_or(([0.0, 0.0, 0.0], 0.0));
        let uniforms = SkyUniforms {
            view_proj: sky_view_proj.to_cols_array_2d(),
            params: [
                gamma,
                texgamma,
                brightness,
                if software_mode { 1.0 } else { 0.0 },
            ],
            fog_color_density: [fog_rgb[0], fog_rgb[1], fog_rgb[2], fog_density],
        };
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniform_bg, &[]);
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        for (i, bg) in self.face_bgs.iter().enumerate() {
            pass.set_bind_group(1, bg, &[]);
            let start = (i * 6) as u32;
            pass.draw(start..start + 6, 0..1);
        }
        (self.face_bgs.len(), std::mem::size_of::<SkyUniforms>() as u64)
    }
}

fn build_sky_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    color_format: wgpu::TextureFormat,
    msaa_samples: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("skybox_pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<SkyVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: None,
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
            count: msaa_samples,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview_mask: None,
        cache: None,
    })
}

fn build_goldsrc_sky_cube() -> Vec<SkyVertex> {
    let s = 100.0f32;
    let quad = |tl: [f32; 3], tr: [f32; 3], br: [f32; 3], bl: [f32; 3]| -> [SkyVertex; 6] {
        [
            SkyVertex { position: tl, uv: [0.0, 0.0] },
            SkyVertex { position: bl, uv: [0.0, 1.0] },
            SkyVertex { position: br, uv: [1.0, 1.0] },
            SkyVertex { position: tl, uv: [0.0, 0.0] },
            SkyVertex { position: br, uv: [1.0, 1.0] },
            SkyVertex { position: tr, uv: [1.0, 0.0] },
        ]
    };

    let mut verts = Vec::with_capacity(36);
    verts.extend_from_slice(&quad([s, s, s], [s, -s, s], [s, -s, -s], [s, s, -s]));
    verts.extend_from_slice(&quad([-s, s, s], [s, s, s], [s, s, -s], [-s, s, -s]));
    verts.extend_from_slice(&quad([-s, -s, s], [-s, s, s], [-s, s, -s], [-s, -s, -s]));
    verts.extend_from_slice(&quad([s, -s, s], [-s, -s, s], [-s, -s, -s], [s, -s, -s]));
    verts.extend_from_slice(&quad([-s, -s, s], [s, -s, s], [s, s, s], [-s, s, s]));
    verts.extend_from_slice(&quad([-s, s, -s], [s, s, -s], [s, -s, -s], [-s, -s, -s]));
    verts
}
