//! GoldSrc BSP v30 GPU renderer supporting lightmaps, alpha-test grates, transparent/additive brushes,
//! and live `gl_texturemode` switching (`GL_LINEAR_MIPMAP_LINEAR` vs `GL_NEAREST`).

use bytemuck::{Pod, Zeroable};
use cs_assets::bsp::{BspMap, RenderBatch, RenderMode};
use glam::Mat4;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuBspVertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    lm_uv: [f32; 2],
    alpha: f32,
    flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BspUniforms {
    view_proj: [[f32; 4]; 4],
    params: [f32; 4], // gamma, brightness, time, debug_mode
}

pub struct BspRenderStats {
    pub draw_calls: usize,
    pub triangles: usize,
}

struct UploadedMap {
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    batches: Vec<RenderBatch>,
    global_bg: wgpu::BindGroup,
    tex_bgs_linear: Vec<wgpu::BindGroup>,
    tex_bgs_nearest: Vec<wgpu::BindGroup>,
    total_triangles: usize,
}

pub struct BspRenderer {
    opaque_pipeline: wgpu::RenderPipeline,
    alphatest_pipeline: wgpu::RenderPipeline,
    transparent_pipeline: wgpu::RenderPipeline,
    additive_pipeline: wgpu::RenderPipeline,
    global_bgl: wgpu::BindGroupLayout,
    tex_bgl: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    lightmap_sampler: wgpu::Sampler,
    linear_sampler: wgpu::Sampler,
    nearest_sampler: wgpu::Sampler,
    uploaded: Option<UploadedMap>,
}

impl BspRenderer {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bsp.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/bsp.wgsl").into()),
        });

        let global_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bsp_global_bgl"),
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

        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bsp_tex_bgl"),
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

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bsp_pipeline_layout"),
            bind_group_layouts: &[Some(&global_bgl), Some(&tex_bgl)],
            immediate_size: 0,
        });

        let opaque_pipeline = build_pipeline(
            device,
            &layout,
            &shader,
            color_format,
            "bsp_opaque",
            Some(wgpu::Face::Back),
            None,
            true,
        );
        let alphatest_pipeline = build_pipeline(
            device,
            &layout,
            &shader,
            color_format,
            "bsp_alphatest",
            None,
            None,
            true,
        );
        let transparent_pipeline = build_pipeline(
            device,
            &layout,
            &shader,
            color_format,
            "bsp_transparent",
            None,
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
        );
        let additive_pipeline = build_pipeline(
            device,
            &layout,
            &shader,
            color_format,
            "bsp_additive",
            None,
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            }),
            false,
        );

        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bsp_uniform_buf"),
            contents: bytemuck::bytes_of(&BspUniforms {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                params: [2.5, 1.0, 0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let lightmap_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bsp_lightmap_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bsp_linear_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });

        let nearest_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("bsp_nearest_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        Self {
            opaque_pipeline,
            alphatest_pipeline,
            transparent_pipeline,
            additive_pipeline,
            global_bgl,
            tex_bgl,
            uniform_buf,
            lightmap_sampler,
            linear_sampler,
            nearest_sampler,
            uploaded: None,
        }
    }

    pub fn clear_map(&mut self) {
        self.uploaded = None;
    }

    pub fn has_map(&self) -> bool {
        self.uploaded.is_some()
    }

    pub fn upload_map(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, map: &BspMap) {
        let gpu_verts: Vec<GpuBspVertex> = map
            .render_vertices
            .iter()
            .map(|v| GpuBspVertex {
                position: v.position,
                normal: v.normal,
                uv: v.uv,
                lm_uv: v.lm_uv,
                alpha: v.alpha,
                flags: v.flags,
            })
            .collect();

        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bsp_vertex_buf"),
            contents: bytemuck::cast_slice(&gpu_verts),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bsp_index_buf"),
            contents: bytemuck::cast_slice(&map.render_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Upload lightmap atlas
        let lm = &map.lightmap_atlas;
        let lm_view = upload_rgba_texture(
            device,
            queue,
            "bsp_lightmap_atlas",
            lm.width,
            lm.height,
            &lm.rgba,
        );

        let global_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bsp_global_bg"),
            layout: &self.global_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&lm_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.lightmap_sampler),
                },
            ],
        });

        // Upload all map diffuse textures and create both Linear and Nearest bind groups
        let mut tex_bgs_linear = Vec::with_capacity(map.textures.len());
        let mut tex_bgs_nearest = Vec::with_capacity(map.textures.len());
        for tex in &map.textures {
            let view = upload_rgba_texture(
                device,
                queue,
                &tex.name,
                tex.width.max(1),
                tex.height.max(1),
                &tex.rgba,
            );
            let bg_lin = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&tex.name),
                layout: &self.tex_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                    },
                ],
            });
            let bg_near = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&tex.name),
                layout: &self.tex_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.nearest_sampler),
                    },
                ],
            });
            tex_bgs_linear.push(bg_lin);
            tex_bgs_nearest.push(bg_near);
        }

        self.uploaded = Some(UploadedMap {
            vertex_buf,
            index_buf,
            batches: map.batches.clone(),
            global_bg,
            tex_bgs_linear,
            tex_bgs_nearest,
            total_triangles: map.render_indices.len() / 3,
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw<'a>(
        &'a self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        view_proj: Mat4,
        gamma: f32,
        brightness: f32,
        time_sec: f32,
        debug_mode: f32,
        use_nearest: bool,
        hidden_models: &[usize],
    ) -> BspRenderStats {
        let Some(map) = &self.uploaded else {
            return BspRenderStats {
                draw_calls: 0,
                triangles: 0,
            };
        };

        let uniforms = BspUniforms {
            view_proj: view_proj.to_cols_array_2d(),
            params: [gamma, brightness, time_sec, debug_mode],
        };
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));

        pass.set_bind_group(0, &map.global_bg, &[]);
        pass.set_vertex_buffer(0, map.vertex_buf.slice(..));
        pass.set_index_buffer(map.index_buf.slice(..), wgpu::IndexFormat::Uint32);

        let tex_bgs = if use_nearest {
            &map.tex_bgs_nearest
        } else {
            &map.tex_bgs_linear
        };

        let mut last_mode: Option<RenderMode> = None;
        for batch in &map.batches {
            if batch.model_index != 0 && hidden_models.contains(&batch.model_index) {
                continue;
            }
            if last_mode != Some(batch.render_mode) {
                let pipe = match batch.render_mode {
                    RenderMode::Normal => &self.opaque_pipeline,
                    RenderMode::AlphaTest => &self.alphatest_pipeline,
                    RenderMode::Transparent => &self.transparent_pipeline,
                    RenderMode::Additive => &self.additive_pipeline,
                };
                pass.set_pipeline(pipe);
                last_mode = Some(batch.render_mode);
            }
            if let Some(bg) = tex_bgs.get(batch.texture_index) {
                pass.set_bind_group(1, bg, &[]);
                pass.draw_indexed(batch.first_index..batch.first_index + batch.index_count, 0, 0..1);
            }
        }

        BspRenderStats {
            draw_calls: map.batches.len(),
            triangles: map.total_triangles,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    color_format: wgpu::TextureFormat,
    label: &str,
    cull_mode: Option<wgpu::Face>,
    blend: Option<wgpu::BlendState>,
    depth_write: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GpuBspVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x3,
                    1 => Float32x3,
                    2 => Float32x2,
                    3 => Float32x2,
                    4 => Float32,
                    5 => Uint32,
                ],
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
            front_face: wgpu::FrontFace::Ccw,
            cull_mode,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn upload_rgba_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> wgpu::TextureView {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
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
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        size,
    );
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}
