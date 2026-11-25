//! GoldSrc StudioModel v10 (`.mdl`) renderer:
//! - Bone controllers, 1D/9-way/2D multi-blending, `"Bip01 Spine"` lower/upper gait split,
//!   sequence crossfade interpolation, external `*01.mdl` sequence groups, and attachments
//! - All 6 `EntRenderMode`s, `RenderFx` (`GlowShell`, `Explode`, `LightMultiplier`),
//!   `STUDIO_NF_{FLATSHADE, CHROME, ADDITIVE, MASKED}`, `body` bodygroups, and `skin` families
//! - Floor `R_LightPoint` + `DLightPool`/flashlight spotlights, `GL_EXP2` fog, GoldSrc
//!   `gamma`/`texgamma`/`brightness`/`r_software_mode`, POT mipmaps + `gl_texturemode`/`gl_ansio`,
//!   and configurable MSAA (`1 | 4`).

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use cs_assets::gamma::{GammaTables, GoldSrcGammaConfig};
use cs_assets::mdl::{
    BoneEvalParams, MdlSeqEvent, STUDIO_NF_ADDITIVE, STUDIO_NF_CHROME, STUDIO_NF_FLATSHADE,
    STUDIO_NF_MASKED, StudioModel,
};
use cs_vfs::Vfs;
use glam::{Mat4, Quat, Vec3};
use tracing::warn;
use wgpu::util::DeviceExt;

use crate::render::lighting::{
    GpuLightsUniform, TextureFilterMode, TextureUploadConfig, create_sampler,
    upload_rgba_texture_mipmapped_with_config,
};
use crate::render::scene::{
    EntRenderMode, RENDERFX_EXPLODE, RENDERFX_GLOW_SHELL, RENDERFX_LIGHT_MULTIPLIER,
    StudioShadow,
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MdlShadowVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ShadowUniforms {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    fog_color_density: [f32; 4],
    gamma_params: [f32; 4],
}

const SHADOW_SHADER_WGSL: &str = r#"
struct ShadowUniforms {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    fog_color_density: vec4<f32>,
    gamma_params: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> u: ShadowUniforms;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) world_pos: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_pos = u.view_proj * vec4<f32>(in.position, 1.0);
    out.color = in.color;
    out.world_pos = in.position;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var rgb = in.color.rgb;
    if u.fog_color_density.w > 0.0 {
        let cam_dist = length(in.world_pos - u.camera_pos.xyz);
        let fd = cam_dist * u.fog_color_density.w;
        let f = exp(-(fd * fd));
        rgb = mix(u.fog_color_density.rgb, rgb, clamp(f, 0.0, 1.0));
    }
    return vec4<f32>(rgb, in.color.a);
}
"#;

/// Build a feathered octagonal translucent disc (`8` triangles = `24` vertices) on the floor
/// plane orthogonal to `shadow.normal`, centered at `shadow.origin` (`alpha = shadow.alpha` at
/// center fading to `0.0` at `shadow.radius`).
pub fn build_shadow_disc_vertices(shadow: &StudioShadow) -> Vec<MdlShadowVertex> {
    const SIDES: usize = 8;
    let normal = shadow.normal.normalize_or_zero();
    if normal.length_squared() < 1e-4 || shadow.alpha <= 0.0 || shadow.radius <= 0.0 {
        return Vec::new();
    }
    let ref_up = if normal.z.abs() < 0.99 { Vec3::Z } else { Vec3::Y };
    let u = normal.cross(ref_up).normalize_or_zero();
    let v = normal.cross(u).normalize_or_zero();
    let center = shadow.origin;
    let center_col = [0.0, 0.0, 0.0, shadow.alpha];
    let rim_col = [0.0, 0.0, 0.0, 0.0];

    let mut ring = [Vec3::ZERO; SIDES];
    for (i, slot) in ring.iter_mut().enumerate() {
        let ang = (i as f32) * (std::f32::consts::TAU / SIDES as f32);
        let (s, c) = ang.sin_cos();
        *slot = center + (u * c + v * s) * shadow.radius;
    }

    let mut verts = Vec::with_capacity(SIDES * 3);
    for i in 0..SIDES {
        let next = (i + 1) % SIDES;
        verts.push(MdlShadowVertex {
            position: center.to_array(),
            color: center_col,
        });
        verts.push(MdlShadowVertex {
            position: ring[i].to_array(),
            color: rim_col,
        });
        verts.push(MdlShadowVertex {
            position: ring[next].to_array(),
            color: rim_col,
        });
    }
    verts
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MdlGpuVertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    color: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MdlUniforms {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    cam_right: [f32; 4],
    cam_up: [f32; 4],
    fog_color_density: [f32; 4],
    gamma_params: [f32; 4],
    light_color: [f32; 4],
    render_color: [f32; 4],
    flags_params: [f32; 4],
}

/// Shared camera, lighting, fog, and gamma parameters for a [`MdlRenderer::prepare_studio`] pass.
#[derive(Debug, Clone, Copy)]
pub struct MdlPassParams {
    pub view_proj: Mat4,
    pub camera_pos: Vec3,
    pub cam_right: Vec3,
    pub cam_up: Vec3,
    pub gamma: f32,
    pub texgamma: f32,
    pub lightgamma: f32,
    pub brightness: f32,
    pub direct: f32,
    pub gl_picmip: i32,
    pub gl_max_size: u32,
    pub gl_round_down: i32,
    pub software_mode: bool,
    pub fog: Option<([f32; 3], f32)>,
    pub lights: GpuLightsUniform,
}

impl Default for MdlPassParams {
    fn default() -> Self {
        Self {
            view_proj: Mat4::IDENTITY,
            camera_pos: Vec3::ZERO,
            cam_right: Vec3::Y,
            cam_up: Vec3::Z,
            gamma: 2.5,
            texgamma: 2.0,
            lightgamma: 2.5,
            brightness: 0.0,
            direct: 0.9,
            gl_picmip: 0,
            gl_max_size: 512,
            gl_round_down: 3,
            software_mode: false,
            fog: None,
            lights: GpuLightsUniform::default(),
        }
    }
}

/// A studio entity of the scene API.
pub struct StudioInstance<'a> {
    pub model_path: &'a str,
    pub transform: Mat4,
    pub sequence: i32,
    /// `entity_state_t::frame`.
    pub frame: f32,
    pub animtime: f32,
    pub framerate: f32,
    /// Client time in seconds.
    pub now: f32,
    pub body: i32,
    pub skin: i32,
    pub controllers: [u8; 4],
    pub mouth: u8,
    pub blending: [u8; 2],
    /// Lower-body gait sequence and current gait frame `(gait_seq, gait_frame)`.
    pub gait: Option<(i32, f32)>,
    /// Previous sequence crossfade `(prev_seq, prev_frame, weight_1_to_0)`.
    pub prev_sequence: Option<(i32, f32, f32)>,
    pub light_rgb: Vec3,
    pub rendermode: EntRenderMode,
    pub renderfx: u8,
    pub rendercolor: [f32; 3],
    pub renderamt: f32,
    pub blend: f32,
    /// Mirror the model (the left-handed view model, `cl_righthand 0`).
    pub flip_winding: bool,
    /// A studio model merged onto this one's bones (`EF_BONEMERGE`).
    pub attached_model: Option<&'a str>,
}

impl<'a> Default for StudioInstance<'a> {
    fn default() -> Self {
        Self {
            model_path: "",
            transform: Mat4::IDENTITY,
            sequence: 0,
            frame: 0.0,
            animtime: 0.0,
            framerate: 1.0,
            now: 0.0,
            body: 0,
            skin: 0,
            controllers: [127, 127, 127, 127],
            mouth: 0,
            blending: [0, 0],
            gait: None,
            prev_sequence: None,
            light_rgb: Vec3::splat(0.8),
            rendermode: EntRenderMode::Normal,
            renderfx: 0,
            rendercolor: [1.0, 1.0, 1.0],
            renderamt: 1.0,
            blend: 1.0,
            flip_winding: false,
            attached_model: None,
        }
    }
}

/// `STUDIO_LOOPING` sequence flag.
pub const STUDIO_LOOPING: i32 = 1;

/// Animation frame of a studio entity: port of `R_StudioEstimateFrame` (Xash3D FWGS
/// `ref/gl/gl_studio.c:565-590`, read 2026-11-23, interpolation on).
pub fn studio_frame(
    frame: f32,
    animtime: f32,
    framerate: f32,
    now: f32,
    fps: f32,
    numframes: usize,
    looping: bool,
) -> f32 {
    let dfdt = if now < animtime {
        0.0
    } else {
        f64::from(now - animtime) * f64::from(framerate) * f64::from(fps)
    };
    let span = numframes.saturating_sub(1) as f64;
    let mut f = if numframes <= 1 {
        0.0
    } else {
        f64::from(frame * span as f32 / 256.0)
    };
    f += dfdt;
    if looping {
        if numframes > 1 {
            f -= ((f / span) as i64) as f64 * span;
        }
        if f < 0.0 {
            f += span;
        }
    } else {
        let max_f = (span - 0.001).max(0.0);
        if f >= max_f {
            f = max_f;
        }
        if f < 0.0 {
            f = 0.0;
        }
    }
    f as f32
}

/// Submodel of body part `index` selected by the entity's `body` value.
pub fn bodypart_submodel(
    parts: &[cs_assets::mdl::MdlBodyPart],
    index: usize,
    body: i32,
) -> Option<&cs_assets::mdl::MdlSubModel> {
    let part = parts.get(index)?;
    let count = part.submodels.len() as i64;
    if count == 0 {
        return None;
    }
    let base: i64 = parts[..index]
        .iter()
        .try_fold(1i64, |acc, p| acc.checked_mul((p.submodels.len() as i64).max(1)))?;
    let pick = (i64::from(body.max(0)) / base) % count;
    part.submodels.get(pick as usize)
}

struct CachedGpuTexture {
    view: wgpu::TextureView,
    bg: wgpu::BindGroup,
    flags: i32,
}

struct CachedGpuModel {
    mdl: StudioModel,
    textures: Vec<CachedGpuTexture>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MdlPipelineKind {
    Opaque,
    Alpha,
    Additive,
}

pub struct MdlRenderer {
    color_format: wgpu::TextureFormat,
    msaa_samples: u32,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    opaque_pipeline: wgpu::RenderPipeline,
    alpha_pipeline: wgpu::RenderPipeline,
    additive_pipeline: wgpu::RenderPipeline,
    uniform_bgl: wgpu::BindGroupLayout,
    tex_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    filter_mode: TextureFilterMode,
    anisotropy: u16,
    cache: HashMap<String, Option<CachedGpuModel>>,
    vertex_buf: wgpu::Buffer,
    vertex_cap: usize,
    uniform_buf: wgpu::Buffer,
    uniform_cap: usize,
    uniform_stride: u64,
    lights_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    shadow_shader: wgpu::ShaderModule,
    shadow_pipeline_layout: wgpu::PipelineLayout,
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_uniform_buf: wgpu::Buffer,
    shadow_uniform_bg: wgpu::BindGroup,
    shadow_vbuf: wgpu::Buffer,
    shadow_vbuf_cap: usize,
    shadow_vertex_count: u32,
    cached_gamma: GoldSrcGammaConfig,
    cached_tex_cfg: TextureUploadConfig,
}

fn create_shadow_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    color_format: wgpu::TextureFormat,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("mdl_shadow_pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<MdlShadowVertex>() as wgpu::BufferAddress,
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
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
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
}

struct PreparedDraw {
    model_key: String,
    tex_idx: usize,
    first_vertex: u32,
    vertex_count: u32,
    uniform_slot: u32,
    pipe_kind: MdlPipelineKind,
}

pub struct PreparedMdlPass {
    draws: Vec<PreparedDraw>,
}

fn create_mdl_pipelines(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    color_format: wgpu::TextureFormat,
    sample_count: u32,
) -> (
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
) {
    let add_blend = Some(wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    });
    let make_pipe = |label: &str, blend: Option<wgpu::BlendState>, depth_write: bool| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MdlGpuVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,
                        1 => Float32x3,
                        2 => Float32x2,
                        3 => Float32x3,
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
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(depth_write),
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
        make_pipe("mdl_opaque", None, true),
        make_pipe("mdl_alpha", Some(wgpu::BlendState::ALPHA_BLENDING), false),
        make_pipe("mdl_additive", add_blend, false),
    )
}

fn create_mdl_uniform_bg(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buf: &wgpu::Buffer,
    raw_size: u64,
    lights_buf: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mdl_uniform_bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: uniform_buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(raw_size),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: lights_buf.as_entire_binding(),
            },
        ],
    })
}

impl MdlRenderer {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mdl.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mdl.wgsl").into()),
        });

        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let raw_size = std::mem::size_of::<MdlUniforms>() as u64;
        let uniform_stride = raw_size.div_ceil(align) * align;

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mdl_uniform_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(raw_size),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<GpuLightsUniform>() as u64,
                        ),
                    },
                    count: None,
                },
            ],
        });

        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mdl_tex_bgl"),
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
            label: Some("mdl_pipeline_layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&tex_bgl)],
            immediate_size: 0,
        });

        let msaa_samples = 1;
        let (opaque_pipeline, alpha_pipeline, additive_pipeline) =
            create_mdl_pipelines(device, &shader, &pipeline_layout, color_format, msaa_samples);

        let filter_mode = TextureFilterMode::LinearMipmapLinear;
        let anisotropy = 1;
        let sampler = create_sampler(device, "mdl_sampler", filter_mode, anisotropy, false);

        let vertex_cap = 16384;
        let vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mdl_dynamic_vbuf"),
            size: (vertex_cap * std::mem::size_of::<MdlGpuVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_cap = 256;
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mdl_dynamic_ubuf"),
            size: (uniform_cap as u64) * uniform_stride,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let lights_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mdl_lights_buf"),
            contents: bytemuck::bytes_of(&GpuLightsUniform::default()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bg = create_mdl_uniform_bg(
            device,
            &uniform_bgl,
            &uniform_buf,
            raw_size,
            &lights_buf,
        );

        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mdl_shadow.wgsl"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER_WGSL.into()),
        });
        let shadow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mdl_shadow_bgl"),
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
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("mdl_shadow_pipeline_layout"),
                bind_group_layouts: &[Some(&shadow_bgl)],
                immediate_size: 0,
            });
        let shadow_pipeline = create_shadow_pipeline(
            device,
            &shadow_shader,
            &shadow_pipeline_layout,
            color_format,
            msaa_samples,
        );
        let shadow_uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mdl_shadow_ubuf"),
            contents: bytemuck::bytes_of(&ShadowUniforms {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                camera_pos: [0.0, 0.0, 0.0, 1.0],
                fog_color_density: [0.0, 0.0, 0.0, 0.0],
                gamma_params: [2.5, 2.0, 0.0, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let shadow_uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mdl_shadow_ubg"),
            layout: &shadow_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: shadow_uniform_buf.as_entire_binding(),
            }],
        });
        let shadow_vbuf_cap = 1024;
        let shadow_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mdl_shadow_vbuf"),
            size: (shadow_vbuf_cap * std::mem::size_of::<MdlShadowVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            color_format,
            msaa_samples,
            shader,
            pipeline_layout,
            opaque_pipeline,
            alpha_pipeline,
            additive_pipeline,
            uniform_bgl,
            tex_bgl,
            sampler,
            filter_mode,
            anisotropy,
            cache: HashMap::new(),
            vertex_buf,
            vertex_cap,
            uniform_buf,
            uniform_cap,
            uniform_stride,
            lights_buf,
            uniform_bg,
            shadow_shader,
            shadow_pipeline_layout,
            shadow_pipeline,
            shadow_uniform_buf,
            shadow_uniform_bg,
            shadow_vbuf,
            shadow_vbuf_cap,
            shadow_vertex_count: 0,
            cached_gamma: GoldSrcGammaConfig::default(),
            cached_tex_cfg: TextureUploadConfig::default(),
        }
    }

    pub fn rebuild_pipelines(&mut self, device: &wgpu::Device, msaa_samples: u32) {
        let samples = msaa_samples.max(1);
        if self.msaa_samples == samples {
            return;
        }
        self.msaa_samples = samples;
        let (opaque, alpha, additive) = create_mdl_pipelines(
            device,
            &self.shader,
            &self.pipeline_layout,
            self.color_format,
            samples,
        );
        self.opaque_pipeline = opaque;
        self.alpha_pipeline = alpha;
        self.additive_pipeline = additive;
        self.shadow_pipeline = create_shadow_pipeline(
            device,
            &self.shadow_shader,
            &self.shadow_pipeline_layout,
            self.color_format,
            samples,
        );
    }

    pub fn set_texture_filter(
        &mut self,
        device: &wgpu::Device,
        mode: TextureFilterMode,
        anisotropy: u16,
    ) {
        let clamped_aniso = anisotropy.clamp(1, 16);
        if self.filter_mode == mode && self.anisotropy == clamped_aniso {
            return;
        }
        self.filter_mode = mode;
        self.anisotropy = clamped_aniso;
        self.sampler = create_sampler(device, "mdl_sampler", mode, clamped_aniso, false);
        for cached in self.cache.values_mut().flatten() {
            for (idx, tex) in cached.textures.iter_mut().enumerate() {
                tex.bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("mdl_tex_bg_{idx}")),
                    layout: &self.tex_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&tex.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
            }
        }
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    fn ensure_loaded(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        rel_path: &str,
        tables: &GammaTables,
        tex_cfg: TextureUploadConfig,
    ) -> Option<String> {
        let key = rel_path.to_ascii_lowercase();
        if !self.cache.contains_key(&key) {
            let loaded = match StudioModel::load(vfs, &key) {
                Ok(mdl) => {
                    let mut textures = Vec::with_capacity(mdl.textures.len());
                    for tex in &mdl.textures {
                        let cfg = TextureUploadConfig {
                            apply_texgamma: (tex.flags & STUDIO_NF_MASKED) == 0,
                            ..tex_cfg
                        };
                        let view = upload_rgba_texture_mipmapped_with_config(
                            device,
                            queue,
                            &tex.name,
                            tex.width.max(1),
                            tex.height.max(1),
                            &tex.rgba,
                            Some(tables),
                            cfg,
                        );
                        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some(&tex.name),
                            layout: &self.tex_bgl,
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
                        textures.push(CachedGpuTexture {
                            view,
                            bg,
                            flags: tex.flags,
                        });
                    }
                    Some(CachedGpuModel { mdl, textures })
                }
                Err(err) => {
                    warn!(model = %key, %err, "Could not load StudioModel");
                    None
                }
            };
            self.cache.insert(key.clone(), loaded);
        }

        if self.cache.get(&key).and_then(|o| o.as_ref()).is_some() {
            Some(key)
        } else {
            None
        }
    }

    /// Evaluate the 4 world-space attachment points of a studio model from cached GPU data.
    pub fn query_attachments(
        &self,
        model_path: &str,
        params: &BoneEvalParams,
        entity_transform: Mat4,
    ) -> Option<[Vec3; 4]> {
        let key = model_path.to_ascii_lowercase();
        let cached = self.cache.get(&key)?.as_ref()?;
        let bones = cached.mdl.evaluate_bones_full(params);
        Some(cached.mdl.evaluate_attachments(entity_transform, &bones))
    }

    /// Inspect sequence animation events for a cached studio model.
    pub fn sequence_events(&self, model_path: &str, sequence: usize) -> &[MdlSeqEvent] {
        let key = model_path.to_ascii_lowercase();
        self.cache
            .get(&key)
            .and_then(|o| o.as_ref())
            .and_then(|c| c.mdl.sequences.get(sequence))
            .map(|s| s.events.as_slice())
            .unwrap_or(&[])
    }

    /// Skin and upload studio model entities with full GoldSrc animation, blending, render modes,
    /// lighting, fog, and gamma.
    pub fn prepare_studio(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        instances: &[StudioInstance<'_>],
        params: &MdlPassParams,
    ) -> PreparedMdlPass {
        queue.write_buffer(&self.lights_buf, 0, bytemuck::bytes_of(&params.lights));

        let mut all_verts: Vec<MdlGpuVertex> = Vec::new();
        let mut draws: Vec<PreparedDraw> = Vec::new();
        let mut uniform_bytes: Vec<u8> = Vec::new();
        let mut uniform_slot: u32 = 0;

        let (fog_rgb, fog_density) = params.fog.unwrap_or(([0.0, 0.0, 0.0], 0.0));
        let gamma_vec = [
            params.gamma,
            params.texgamma,
            params.brightness,
            if params.software_mode { 1.0 } else { 0.0 },
        ];

        let gamma_cfg = GoldSrcGammaConfig {
            gamma: params.gamma,
            brightness: params.brightness,
            texgamma: params.texgamma,
            lightgamma: params.lightgamma,
        }
        .validated();
        let tex_cfg = TextureUploadConfig {
            gl_round_down: params.gl_round_down,
            gl_picmip: params.gl_picmip,
            gl_max_size: params.gl_max_size,
            apply_texgamma: true,
        };
        if self.cached_gamma != gamma_cfg || self.cached_tex_cfg != tex_cfg {
            self.cache.clear();
            self.cached_gamma = gamma_cfg;
            self.cached_tex_cfg = tex_cfg;
        }
        let tables = GammaTables::build(gamma_cfg);

        for inst in instances {
            let Some(key) =
                self.ensure_loaded(device, queue, vfs, inst.model_path, &tables, tex_cfg)
            else {
                continue;
            };
            let attached_key = inst
                .attached_model
                .and_then(|p| self.ensure_loaded(device, queue, vfs, p, &tables, tex_cfg));
            let Some(Some(cached)) = self.cache.get(&key) else {
                continue;
            };

            let seq_idx = if (inst.sequence.max(0) as usize) < cached.mdl.sequences.len() {
                inst.sequence.max(0) as usize
            } else {
                0
            };
            let (fps, numframes, looping) = cached
                .mdl
                .sequences
                .get(seq_idx)
                .map(|s| (s.fps, s.num_frames, s.flags & STUDIO_LOOPING != 0))
                .unwrap_or((30.0, 1, false));
            let frame = studio_frame(
                inst.frame,
                inst.animtime,
                inst.framerate,
                inst.now,
                fps,
                numframes,
                looping,
            );
            let prev_sequence = inst.prev_sequence.map(|(ps, pf, w)| {
                let ps_idx = (ps.max(0) as usize).min(cached.mdl.sequences.len().saturating_sub(1));
                let p_loop = cached
                    .mdl
                    .sequences
                    .get(ps_idx)
                    .is_some_and(|s| s.flags & STUDIO_LOOPING != 0);
                (ps_idx, pf, p_loop, w)
            });
            let gait = inst.gait.map(|(gs, gf)| {
                let gs_idx = (gs.max(0) as usize).min(cached.mdl.sequences.len().saturating_sub(1));
                (gs_idx, gf)
            });
            let eval_params = BoneEvalParams {
                sequence: seq_idx,
                frame,
                loop_anim: false,
                blending: inst.blending,
                controller: inst.controllers,
                mouth: inst.mouth,
                prev_sequence,
                gait,
            };
            let bones = cached.mdl.evaluate_bones_full(&eval_params);

            let explode_scale = if inst.renderfx == RENDERFX_EXPLODE {
                Vec3::splat(1.12)
            } else {
                Vec3::ONE
            };
            let light_mul = if inst.renderfx == RENDERFX_LIGHT_MULTIPLIER {
                1.5
            } else {
                1.0
            };
            let (ambient_light, shade_light, light_color, plight_vec) = compute_entity_lighting(
                inst.transform.w_axis.truncate(),
                inst.light_rgb * light_mul,
                &params.lights,
                params.direct,
            );

            let mut push_model =
                |m_key: &str, cached_m: &CachedGpuModel, m_bones: &[Mat4], shell_only: bool| {
                    for bi in 0..cached_m.mdl.bodyparts.len() {
                        let Some(sub) = bodypart_submodel(&cached_m.mdl.bodyparts, bi, inst.body)
                        else {
                            continue;
                        };
                        for mesh in &sub.meshes {
                            if mesh.vertices.is_empty() {
                                continue;
                            }
                            let tex_idx = cached_m
                                .mdl
                                .resolve_skin_texture(inst.skin, mesh.skinref)
                                .min(cached_m.textures.len().saturating_sub(1));
                            let tex_flags = cached_m
                                .textures
                                .get(tex_idx)
                                .map(|t| t.flags)
                                .unwrap_or(0);
                            let alpha_test = (tex_flags & STUDIO_NF_MASKED) != 0;

                            let first_vertex = all_verts.len() as u32;
                            for v in &mesh.vertices {
                                let bm = m_bones
                                    .get(v.bone as usize)
                                    .copied()
                                    .unwrap_or(Mat4::IDENTITY);
                                let m = inst.transform * Mat4::from_scale(explode_scale) * bm;
                                let pos = m.transform_point3(v.pos);
                                let norm = m.transform_vector3(v.normal).normalize_or_zero();
                                let lv = r_studio_lighting(
                                    &tables,
                                    tex_flags,
                                    norm,
                                    ambient_light,
                                    shade_light,
                                    plight_vec,
                                );
                                let vcol = light_color * lv;
                                all_verts.push(MdlGpuVertex {
                                    position: pos.to_array(),
                                    normal: norm.to_array(),
                                    uv: v.uv,
                                    color: vcol.to_array(),
                                });
                            }
                            let vertex_count = (all_verts.len() as u32) - first_vertex;

                            let (shader_mode, shell_offset, pipe_kind, alpha_val) = if shell_only {
                                let thickness = (inst.renderamt * 255.0 * 0.05).clamp(0.4, 3.0);
                                (5.0, thickness, MdlPipelineKind::Additive, 0.75)
                            } else {
                                let is_add_tex = (tex_flags & STUDIO_NF_ADDITIVE) != 0;
                                match inst.rendermode {
                                    EntRenderMode::Normal => {
                                        if is_add_tex {
                                            (2.0, 0.0, MdlPipelineKind::Additive, 1.0)
                                        } else {
                                            (0.0, 0.0, MdlPipelineKind::Opaque, 1.0)
                                        }
                                    }
                                    EntRenderMode::TransColor => {
                                        (1.0, 0.0, MdlPipelineKind::Alpha, inst.renderamt)
                                    }
                                    EntRenderMode::TransTexture | EntRenderMode::Glow => {
                                        (4.0, 0.0, MdlPipelineKind::Alpha, inst.blend)
                                    }
                                    EntRenderMode::TransAlpha => {
                                        (3.0, 0.0, MdlPipelineKind::Alpha, inst.blend)
                                    }
                                    EntRenderMode::TransAdd => {
                                        (2.0, 0.0, MdlPipelineKind::Additive, inst.blend)
                                    }
                                }
                            };

                            let u = MdlUniforms {
                                view_proj: params.view_proj.to_cols_array_2d(),
                                camera_pos: [
                                    params.camera_pos.x,
                                    params.camera_pos.y,
                                    params.camera_pos.z,
                                    1.0,
                                ],
                                cam_right: [
                                    params.cam_right.x,
                                    params.cam_right.y,
                                    params.cam_right.z,
                                    0.0,
                                ],
                                cam_up: [params.cam_up.x, params.cam_up.y, params.cam_up.z, 0.0],
                                fog_color_density: [fog_rgb[0], fog_rgb[1], fog_rgb[2], fog_density],
                                gamma_params: gamma_vec,
                                light_color: [
                                    inst.light_rgb.x,
                                    inst.light_rgb.y,
                                    inst.light_rgb.z,
                                    if alpha_test { 1.0 } else { 0.0 },
                                ],
                                render_color: [
                                    inst.rendercolor[0],
                                    inst.rendercolor[1],
                                    inst.rendercolor[2],
                                    alpha_val,
                                ],
                                flags_params: [
                                    shader_mode,
                                    (tex_flags & (STUDIO_NF_FLATSHADE | STUDIO_NF_CHROME | STUDIO_NF_ADDITIVE)) as f32,
                                    shell_offset,
                                    light_mul,
                                ],
                            };
                            let start = uniform_bytes.len();
                            uniform_bytes.resize(start + self.uniform_stride as usize, 0);
                            let raw = bytemuck::bytes_of(&u);
                            uniform_bytes[start..start + raw.len()].copy_from_slice(raw);

                            draws.push(PreparedDraw {
                                model_key: m_key.to_string(),
                                tex_idx,
                                first_vertex,
                                vertex_count,
                                uniform_slot,
                                pipe_kind,
                            });
                            uniform_slot += 1;
                        }
                    }
                };

            push_model(&key, cached, &bones, false);
            if inst.renderfx == RENDERFX_GLOW_SHELL {
                push_model(&key, cached, &bones, true);
            }

            if let Some(ref att_key) = attached_key
                && let Some(Some(att_cached)) = self.cache.get(att_key)
            {
                let merged_bones = att_cached.mdl.evaluate_bones_merged(&cached.mdl, &bones);
                push_model(att_key, att_cached, &merged_bones, false);
                if inst.renderfx == RENDERFX_GLOW_SHELL {
                    push_model(att_key, att_cached, &merged_bones, true);
                }
            }
        }

        if all_verts.len() > self.vertex_cap {
            self.vertex_cap = all_verts.len().next_power_of_two();
            self.vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mdl_dynamic_vbuf"),
                size: (self.vertex_cap * std::mem::size_of::<MdlGpuVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !all_verts.is_empty() {
            queue.write_buffer(&self.vertex_buf, 0, bytemuck::cast_slice(&all_verts));
        }

        if (uniform_slot as usize) > self.uniform_cap {
            self.uniform_cap = (uniform_slot as usize).next_power_of_two();
            self.uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mdl_dynamic_ubuf"),
                size: (self.uniform_cap as u64) * self.uniform_stride,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let raw_size = std::mem::size_of::<MdlUniforms>() as u64;
            self.uniform_bg = create_mdl_uniform_bg(
                device,
                &self.uniform_bgl,
                &self.uniform_buf,
                raw_size,
                &self.lights_buf,
            );
        }
        if !uniform_bytes.is_empty() {
            queue.write_buffer(&self.uniform_buf, 0, &uniform_bytes);
        }

        PreparedMdlPass { draws }
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, prepared: &PreparedMdlPass) {
        self.draw_filtered(pass, prepared, |_| true);
    }

    /// Upload feathered octagonal studio blob shadows (`cl_shadows 1`) for the current frame.
    pub fn prepare_shadows(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shadows: &[StudioShadow],
        params: &MdlPassParams,
    ) {
        let (fog_rgb, fog_density) = params.fog.unwrap_or(([0.0, 0.0, 0.0], 0.0));
        queue.write_buffer(
            &self.shadow_uniform_buf,
            0,
            bytemuck::bytes_of(&ShadowUniforms {
                view_proj: params.view_proj.to_cols_array_2d(),
                camera_pos: [params.camera_pos.x, params.camera_pos.y, params.camera_pos.z, 1.0],
                fog_color_density: [fog_rgb[0], fog_rgb[1], fog_rgb[2], fog_density],
                gamma_params: [
                    params.gamma,
                    params.texgamma,
                    params.brightness,
                    if params.software_mode { 1.0 } else { 0.0 },
                ],
            }),
        );
        if shadows.is_empty() {
            self.shadow_vertex_count = 0;
            return;
        }
        let mut verts = Vec::with_capacity(shadows.len() * 24);
        for s in shadows {
            verts.extend(build_shadow_disc_vertices(s));
        }
        self.shadow_vertex_count = verts.len() as u32;
        if verts.len() > self.shadow_vbuf_cap {
            self.shadow_vbuf_cap = verts.len().next_power_of_two();
            self.shadow_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mdl_shadow_vbuf"),
                size: (self.shadow_vbuf_cap * std::mem::size_of::<MdlShadowVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !verts.is_empty() {
            queue.write_buffer(&self.shadow_vbuf, 0, bytemuck::cast_slice(&verts));
        }
    }

    /// Draw prepared studio blob shadows (depth-read `LessEqual`, no depth-write, alpha-blended).
    pub fn draw_shadows<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.shadow_vertex_count == 0 {
            return;
        }
        pass.set_pipeline(&self.shadow_pipeline);
        pass.set_bind_group(0, &self.shadow_uniform_bg, &[]);
        pass.set_vertex_buffer(0, self.shadow_vbuf.slice(..));
        pass.draw(0..self.shadow_vertex_count, 0..1);
    }

    pub fn draw_opaque<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, prepared: &PreparedMdlPass) {
        self.draw_filtered(pass, prepared, |k| k == MdlPipelineKind::Opaque);
    }

    pub fn draw_translucent<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        prepared: &PreparedMdlPass,
    ) {
        self.draw_filtered(pass, prepared, |k| k != MdlPipelineKind::Opaque);
    }

    fn draw_filtered<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        prepared: &PreparedMdlPass,
        pred: fn(MdlPipelineKind) -> bool,
    ) {
        if prepared.draws.is_empty() {
            return;
        }
        pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
        for d in &prepared.draws {
            if !pred(d.pipe_kind) {
                continue;
            }
            let Some(Some(cached)) = self.cache.get(&d.model_key) else {
                continue;
            };
            let Some(tex) = cached.textures.get(d.tex_idx) else {
                continue;
            };
            let pipe = match d.pipe_kind {
                MdlPipelineKind::Opaque => &self.opaque_pipeline,
                MdlPipelineKind::Alpha => &self.alpha_pipeline,
                MdlPipelineKind::Additive => &self.additive_pipeline,
            };
            pass.set_pipeline(pipe);
            let offset = d.uniform_slot * (self.uniform_stride as u32);
            pass.set_bind_group(0, &self.uniform_bg, &[offset]);
            pass.set_bind_group(1, &tex.bg, &[]);
            pass.draw(d.first_vertex..d.first_vertex + d.vertex_count, 0..1);
        }
    }
}

/// Port of `R_EntityDynamicLight` (`ref/common/ref_light.c:520-685`, read 2026-11-24).
fn compute_entity_lighting(
    ent_origin: Vec3,
    floor_light_rgb: Vec3,
    lights: &GpuLightsUniform,
    direct: f32,
) -> (f32, f32, Vec3, Vec3) {
    let light = floor_light_rgb * 255.0;
    let mut final_light = light;
    let mut total = light.max_element();
    if total == 0.0 {
        total = 1.0;
    }
    let mut light_dir = Vec3::new(0.0, 0.0, -1.0) * total;

    let nd = (lights.counts[0] as usize).min(lights.dlight_pos_rad.len());
    for i in 0..nd {
        let pr = lights.dlight_pos_rad[i];
        let mut dist_v = ent_origin - Vec3::new(pr[0], pr[1], pr[2]);
        let radius = dist_v.length();
        let add = pr[3] - radius;
        if add > 0.0 {
            total += add;
            if radius > 1.0 {
                dist_v *= add / radius;
            } else {
                dist_v *= add;
            }
            light_dir += dist_v;
            let col = lights.dlight_col[i];
            final_light += Vec3::new(col[0], col[1], col[2]) * (255.0 * (add / 256.0));
        }
    }

    let scale = direct.clamp(0.75, 1.0);
    light_dir *= scale;
    let mut shadelight = light_dir.length();
    let ambientlight = (total - shadelight).min(128.0);
    if ambientlight + shadelight > 255.0 {
        shadelight = 255.0 - ambientlight;
    }
    let col_max = final_light.max_element();
    let lightcolor = if col_max > 0.0 {
        final_light / col_max
    } else {
        Vec3::ONE
    };
    let plightvec = light_dir.normalize_or_zero();
    (ambientlight, shadelight, lightcolor, plightvec)
}

/// Port of `R_StudioLighting` (`ref/gl/gl_studio.c:1324-1370`, `SHADE_LAMBERT = 1.4953241`,
/// read 2026-11-24).
fn r_studio_lighting(
    tables: &GammaTables,
    tex_flags: i32,
    normal: Vec3,
    ambientlight: f32,
    shadelight: f32,
    plightvec: Vec3,
) -> f32 {
    const STUDIO_NF_FULLBRIGHT: i32 = 0x0040;
    const SHADE_LAMBERT: f32 = 1.4953241;
    if (tex_flags & STUDIO_NF_FULLBRIGHT) != 0 {
        return 1.0;
    }
    let illum = if (tex_flags & STUDIO_NF_FLATSHADE) != 0 {
        ambientlight + shadelight * 0.8
    } else {
        let mut lum = ambientlight + shadelight;
        let lightcos = normal.dot(plightvec).min(1.0);
        let adj = (lightcos + (SHADE_LAMBERT - 1.0)) / SHADE_LAMBERT;
        if adj > 0.0 {
            lum -= shadelight * adj;
        }
        lum.max(0.0)
    };
    let clamped = illum.min(255.0);
    tables.light_to_tex_gamma_unit((clamped * 4.0) as u32)
}

/// Build the authentic GoldSrc first-person viewmodel matrix anchored to the camera eye
/// with yaw, pitch, and roll (`client.dll` `0x1006b52f..1006b7a0`).
pub fn viewmodel_transform_with_angles(
    eye: Vec3,
    yaw_deg: f32,
    pitch_deg: f32,
    roll_deg: f32,
    bob_offset: Vec3,
    right_handed: bool,
) -> Mat4 {
    let rot = Quat::from_rotation_z(yaw_deg.to_radians())
        * Quat::from_rotation_y(pitch_deg.to_radians())
        * Quat::from_rotation_x(roll_deg.to_radians());
    let scale = if right_handed {
        Vec3::new(1.0, -1.0, 1.0)
    } else {
        Vec3::ONE
    };
    let pos = eye + rot * bob_offset;
    Mat4::from_scale_rotation_translation(scale, rot, pos)
}

/// Build the authentic GoldSrc first-person viewmodel matrix anchored to the camera eye.
pub fn viewmodel_transform(
    eye: Vec3,
    yaw_deg: f32,
    pitch_deg: f32,
    bob_offset: Vec3,
    right_handed: bool,
) -> Mat4 {
    viewmodel_transform_with_angles(eye, yaw_deg, pitch_deg, 0.0, bob_offset, right_handed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_assets::mdl::{MdlBodyPart, MdlSubModel};

    fn part(n: usize) -> MdlBodyPart {
        MdlBodyPart {
            name: String::new(),
            submodels: (0..n)
                .map(|i| MdlSubModel {
                    name: format!("s{i}"),
                    meshes: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn body_selects_submodels_per_part() {
        let parts = [part(3), part(2)];
        let name = |i, body| bodypart_submodel(&parts, i, body).map(|s| s.name.clone());
        assert_eq!(name(0, 0).as_deref(), Some("s0"));
        assert_eq!(name(0, 4).as_deref(), Some("s1"));
        assert_eq!(name(1, 0).as_deref(), Some("s0"));
        assert_eq!(name(1, 3).as_deref(), Some("s1"));
        assert_eq!(name(1, 5).as_deref(), Some("s1"));
        assert_eq!(name(0, -7).as_deref(), Some("s0"));
        assert!(bodypart_submodel(&parts, 2, 0).is_none());
        assert!(bodypart_submodel(&[part(0)], 0, 0).is_none());
    }

    #[test]
    fn studio_frame_follows_estimate_frame() {
        assert_eq!(studio_frame(128.0, 10.0, 1.0, 10.0, 30.0, 41, false), 20.0);
        assert_eq!(studio_frame(128.0, 10.0, 1.0, 11.0, 30.0, 41, true), 10.0);
        assert_eq!(
            studio_frame(128.0, 10.0, 1.0, 11.0, 30.0, 41, false),
            (40.0f64 - 0.001) as f32
        );
        assert_eq!(
            studio_frame(0.0, 10.0, 1.0, 11.0, 30.0, 2, false),
            (1.0f64 - 0.001) as f32
        );
        assert_eq!(studio_frame(64.0, 10.0, 0.0, 15.0, 30.0, 41, true), 10.0);
        assert_eq!(studio_frame(0.0, 12.0, 1.0, 11.0, 30.0, 41, true), 0.0);
        assert_eq!(studio_frame(200.0, 0.0, 1.0, 5.0, 30.0, 1, true), 150.0);
        assert_eq!(studio_frame(200.0, 0.0, 1.0, 5.0, 30.0, 1, false), 0.0);
    }

    #[test]
    fn viewmodel_transform_with_angles_applies_roll_and_righthand_mirror() {
        let xf_r = viewmodel_transform_with_angles(Vec3::new(1.0, 2.0, 3.0), 0.0, 0.0, 90.0, Vec3::ZERO, true);
        let up_col = xf_r.z_axis.truncate();
        assert!((up_col - Vec3::new(0.0, -1.0, 0.0)).length() < 1e-4, "got {up_col:?}");
        assert!((xf_r.y_axis.truncate().length() - 1.0).abs() < 1e-4);
        assert!(xf_r.determinant() < 0.0);

        let xf_l = viewmodel_transform_with_angles(Vec3::new(1.0, 2.0, 3.0), 0.0, 0.0, 90.0, Vec3::ZERO, false);
        assert!(xf_l.determinant() > 0.0);
    }

    #[test]
    fn build_shadow_disc_emits_feathered_octagon() {
        let shadow = StudioShadow {
            origin: Vec3::new(10.0, 20.0, 0.15),
            normal: Vec3::Z,
            radius: 18.0,
            alpha: 0.45,
        };
        let verts = build_shadow_disc_vertices(&shadow);
        assert_eq!(verts.len(), 24);
        for tri in verts.as_chunks::<3>().0 {
            assert_eq!(tri[0].position, shadow.origin.to_array());
            assert!((tri[0].color[3] - 0.45).abs() < 1e-6);
            assert_eq!(tri[1].color[3], 0.0);
            assert_eq!(tri[2].color[3], 0.0);
            let d1 = (Vec3::from_array(tri[1].position) - shadow.origin).length();
            assert!((d1 - 18.0).abs() < 1e-4);
        }
    }
}
