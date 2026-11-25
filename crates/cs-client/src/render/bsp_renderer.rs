//! GoldSrc BSP v30 GPU renderer supporting dynamic lightmaps & lightstyles, PVS leaf culling,
//! animated/toggleable/scrolling textures, water ripples, distance fog, dynamic lights/flashlight,
//! MSAA, software-mode dithering, and live `gl_texturemode` + `gl_ansio` switching.

use bytemuck::{Pod, Zeroable};
use cs_assets::bsp::{
    BspDlight, BspMap, FaceDlightInfo, Leaf, LightmapAtlas, Node, Plane, RenderBatch, RenderMode,
    TextureAnimTable, WorldFaceSpan,
};
use cs_assets::gamma::{GammaTables, GoldSrcGammaConfig};
use cs_assets::wad::DecodedTexture;
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::render::lighting::{
    GpuLightsUniform, TextureFilterMode, TextureUploadConfig, create_sampler,
    upload_rgba_texture_mipmapped_with_config,
};
use crate::render::scene::{BrushDraw, EntRenderMode};

/// GoldSrc / Xash3D FWGS `TURBSCALE` (`ref/gl/gl_rsurf.c`, GPL-3.0-or-later, read 2026-11-25).
pub const TURBSCALE: f32 = 256.0 / std::f32::consts::TAU;

/// 256-entry `r_turbsin` table (`4.0 * sin(i * 2*pi / 256)`), ported from Xash3D FWGS
/// `engine/warpsin.h` (GPL-3.0-or-later, read 2026-11-25).
#[rustfmt::skip]
#[allow(clippy::excessive_precision)]
pub const R_TURBSIN: [f32; 256] = [
    0.000000, 0.098165, 0.196270, 0.294259, 0.392069, 0.489643, 0.586920, 0.683850,
    0.780360, 0.876405, 0.971920, 1.066850, 1.161140, 1.254725, 1.347560, 1.439580,
    1.530735, 1.620965, 1.710220, 1.798445, 1.885585, 1.971595, 2.056410, 2.139990,
    2.222280, 2.303235, 2.382795, 2.460925, 2.537575, 2.612690, 2.686235, 2.758160,
    2.828425, 2.896990, 2.963805, 3.028835, 3.092040, 3.153385, 3.212830, 3.270340,
    3.325880, 3.379415, 3.430915, 3.480350, 3.527685, 3.572895, 3.615955, 3.656840,
    3.695520, 3.731970, 3.766175, 3.798115, 3.827760, 3.855105, 3.880125, 3.902810,
    3.923140, 3.941110, 3.956705, 3.969920, 3.980740, 3.989160, 3.995180, 3.998795,
    4.000000, 3.998795, 3.995180, 3.989160, 3.980740, 3.969920, 3.956705, 3.941110,
    3.923140, 3.902810, 3.880125, 3.855105, 3.827760, 3.798115, 3.766175, 3.731970,
    3.695520, 3.656840, 3.615955, 3.572895, 3.527685, 3.480350, 3.430915, 3.379415,
    3.325880, 3.270340, 3.212830, 3.153385, 3.092040, 3.028835, 2.963805, 2.896990,
    2.828425, 2.758160, 2.686235, 2.612690, 2.537575, 2.460925, 2.382795, 2.303235,
    2.222280, 2.139990, 2.056410, 1.971595, 1.885585, 1.798445, 1.710220, 1.620965,
    1.530735, 1.439580, 1.347560, 1.254725, 1.161140, 1.066850, 0.971920, 0.876405,
    0.780360, 0.683850, 0.586920, 0.489643, 0.392069, 0.294259, 0.196270, 0.098165,
    0.000000, -0.098165, -0.196270, -0.294259, -0.392069, -0.489643, -0.586920, -0.683850,
    -0.780360, -0.876405, -0.971920, -1.066850, -1.161140, -1.254725, -1.347560, -1.439580,
    -1.530735, -1.620965, -1.710220, -1.798445, -1.885585, -1.971595, -2.056410, -2.139990,
    -2.222280, -2.303235, -2.382795, -2.460925, -2.537575, -2.612690, -2.686235, -2.758160,
    -2.828425, -2.896990, -2.963805, -3.028835, -3.092040, -3.153385, -3.212830, -3.270340,
    -3.325880, -3.379415, -3.430915, -3.480350, -3.527685, -3.572895, -3.615955, -3.656840,
    -3.695520, -3.731970, -3.766175, -3.798115, -3.827760, -3.855105, -3.880125, -3.902810,
    -3.923140, -3.941110, -3.956705, -3.969920, -3.980740, -3.989160, -3.995180, -3.998795,
    -4.000000, -3.998795, -3.995180, -3.989160, -3.980740, -3.969920, -3.956705, -3.941110,
    -3.923140, -3.902810, -3.880125, -3.855105, -3.827760, -3.798115, -3.766175, -3.731970,
    -3.695520, -3.656840, -3.615955, -3.572895, -3.527685, -3.480350, -3.430915, -3.379415,
    -3.325880, -3.270340, -3.212830, -3.153385, -3.092040, -3.028835, -2.963805, -2.896990,
    -2.828425, -2.758160, -2.686235, -2.612690, -2.537575, -2.460925, -2.382795, -2.303235,
    -2.222280, -2.139990, -2.056410, -1.971595, -1.885585, -1.798445, -1.710220, -1.620965,
    -1.530735, -1.439580, -1.347560, -1.254725, -1.161140, -1.066850, -0.971920, -0.876405,
    -0.780360, -0.683850, -0.586920, -0.489643, -0.392069, -0.294259, -0.196270, -0.098165,
];

/// Sample [`R_TURBSIN`] with C `(int)(v) & 255` two's-complement wrap semantics.
#[inline]
pub fn turbsin_sample(index_expr: f32) -> f32 {
    let idx = (index_expr as i32 & 255) as usize;
    R_TURBSIN[idx]
}

/// CPU twin of `EmitWaterPolys` (`ref/gl/gl_rsurf.c`, read 2026-11-25) vertex Z wave offset.
/// Negates `wave_height` when the water vertex is at or above `camera_z` (underwater view).
pub fn water_wave_offset(local_pos: [f32; 3], time_sec: f32, wave_height: f32, camera_z: f32) -> f32 {
    if wave_height == 0.0 {
        return 0.0;
    }
    let wh = if local_pos[2] >= camera_z {
        -wave_height
    } else {
        wave_height
    };
    let nv1 = turbsin_sample(time_sec * 160.0 + local_pos[1] + local_pos[0]) + 8.0;
    let nv2 = (turbsin_sample(local_pos[0] * 5.0 + time_sec * 171.0 - local_pos[1]) + 8.0) * 0.8 + nv1;
    nv2 * wh
}

/// CPU twin of `EmitWaterPolys` (`ref/gl/gl_rsurf.c`, read 2026-11-25) UV warp.
pub fn water_warp_uv(uv: [f32; 2], tex_size: [f32; 2], time_sec: f32) -> [f32; 2] {
    let os = uv[0] * tex_size[0];
    let ot = uv[1] * tex_size[1];
    let s = os + turbsin_sample((ot * 0.125 + time_sec) * TURBSCALE);
    let t = ot + turbsin_sample((os * 0.125 + time_sec) * TURBSCALE);
    [s / tex_size[0].max(1.0), t / tex_size[1].max(1.0)]
}

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
    camera_pos: [f32; 4],
    fog_color_density: [f32; 4],
    gamma_params: [f32; 4], // gamma, texgamma, brightness, software_mode
    misc_params: [f32; 4],  // time_sec, debug_mode, wave_height, _pad
}

/// Parameters passed to [`BspRenderer::draw`] for world and brush entity shading.
#[derive(Debug, Clone, Copy)]
pub struct BspDrawParams {
    pub view_proj: Mat4,
    pub camera_pos: Vec3,
    pub gamma: f32,
    pub texgamma: f32,
    pub lightgamma: f32,
    pub brightness: f32,
    pub gl_overbright: bool,
    pub gl_picmip: i32,
    pub gl_max_size: u32,
    pub gl_round_down: i32,
    pub software_mode: bool,
    pub time_sec: f32,
    pub debug_mode: f32,
    pub wave_height: f32,
    pub fog: Option<([f32; 3], f32)>,
    pub r_vis: bool,
    pub lights: GpuLightsUniform,
}

/// Per-entity uniforms of a brush submodel draw (bind group 2, dynamic offset). Slot 0 is the
/// identity transform used by the world pass.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct EntityUniforms {
    model: [[f32; 4]; 4],
    /// rgb = rendercolor, a = blend (0..1).
    color: [f32; 4],
    /// x = shader mode, y = UV U-scroll offset.
    params: [f32; 4],
}

const ENT_MODE_WORLD: f32 = 0.0;
const ENT_MODE_TRANS_COLOR: f32 = 1.0;
const ENT_MODE_ADD: f32 = 2.0;
const ENT_MODE_TRANS_ALPHA: f32 = 3.0;
const ENT_MODE_BLEND: f32 = 4.0;

/// One brush entity after `prepare_entities`, ready for `draw_entities`.
pub struct PreparedBrushEntity {
    slot: u32,
    submodel: usize,
    mode: EntRenderMode,
    solid: bool,
    frame: f32,
}

impl PreparedBrushEntity {
    pub fn is_opaque(&self) -> bool {
        self.solid
    }
}

pub(crate) fn group_batches_by_model(batches: &[RenderBatch]) -> Vec<Vec<usize>> {
    let max = batches.iter().map(|b| b.model_index).max().unwrap_or(0);
    let mut groups = vec![Vec::new(); max + 1];
    for (i, b) in batches.iter().enumerate() {
        groups[b.model_index].push(i);
    }
    groups
}

fn entity_shader_params(
    mode: EntRenderMode,
    blend: f32,
    amt: f32,
    color: [f32; 3],
) -> ([f32; 4], f32) {
    match mode {
        EntRenderMode::Normal => ([1.0; 4], ENT_MODE_WORLD),
        EntRenderMode::TransColor => ([color[0], color[1], color[2], amt], ENT_MODE_TRANS_COLOR),
        EntRenderMode::TransAdd => ([1.0, 1.0, 1.0, blend], ENT_MODE_ADD),
        EntRenderMode::TransAlpha => ([1.0; 4], ENT_MODE_TRANS_ALPHA),
        EntRenderMode::TransTexture | EntRenderMode::Glow => ([1.0, 1.0, 1.0, blend], ENT_MODE_BLEND),
    }
}

fn create_entity_buffer(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    slots: usize,
    stride: u64,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let identity = EntityUniforms {
        model: Mat4::IDENTITY.to_cols_array_2d(),
        color: [1.0; 4],
        params: [ENT_MODE_WORLD, 0.0, 0.0, 0.0],
    };
    let mut init = vec![0u8; slots * stride as usize];
    let raw = bytemuck::bytes_of(&identity);
    init[..raw.len()].copy_from_slice(raw);
    let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("bsp_entity_ubuf"),
        contents: &init,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bsp_entity_bg"),
        layout: bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buf,
                offset: 0,
                size: wgpu::BufferSize::new(std::mem::size_of::<EntityUniforms>() as u64),
            }),
        }],
    });
    (buf, bg)
}

pub struct BspRenderStats {
    pub draw_calls: usize,
    pub triangles: usize,
}

struct BspPvsData {
    head_node: i32,
    planes: Vec<Plane>,
    nodes: Vec<Node>,
    leafs: Vec<Leaf>,
    visibility: Vec<u8>,
    mark_surfaces: Vec<u16>,
    vis_leafs: usize,
}

impl BspPvsData {
    fn from_map(map: &BspMap) -> Self {
        let head_node = map.models.first().map_or(0, |m| m.head_nodes[0]);
        let vis_leafs = map
            .models
            .first()
            .map_or(0, |m| m.vis_leafs.max(0) as usize)
            .min(map.leafs.len().saturating_sub(1));
        Self {
            head_node,
            planes: map.planes.clone(),
            nodes: map.nodes.clone(),
            leafs: map.leafs.clone(),
            visibility: map.visibility.clone(),
            mark_surfaces: map.mark_surfaces.clone(),
            vis_leafs,
        }
    }

    fn find_leaf(&self, point: Vec3) -> usize {
        if self.nodes.is_empty() || self.leafs.is_empty() {
            return 0;
        }
        let mut node_idx = self.head_node;
        let mut guard = self.nodes.len() + self.leafs.len();
        while node_idx >= 0 {
            if guard == 0 {
                return 0;
            }
            guard -= 1;
            let Some(node) = self.nodes.get(node_idx as usize) else {
                return 0;
            };
            let Some(plane) = self.planes.get(node.plane as usize) else {
                return 0;
            };
            let d = plane.normal.dot(point) - plane.dist;
            node_idx = if d >= 0.0 {
                i32::from(node.children[0])
            } else {
                i32::from(node.children[1])
            };
        }
        ((-node_idx - 1) as usize).min(self.leafs.len().saturating_sub(1))
    }

    fn visible_faces(&self, cam_leaf: usize, face_count: usize) -> Option<Vec<bool>> {
        if cam_leaf == 0 || self.visibility.is_empty() || self.vis_leafs == 0 {
            return None;
        }
        let leaf = self.leafs.get(cam_leaf)?;
        if leaf.vis_offset < 0 {
            return None;
        }
        let mut pvs = vec![false; self.vis_leafs];
        let mut in_idx = leaf.vis_offset as usize;
        let mut out_leaf = 0usize;
        while out_leaf < self.vis_leafs && in_idx < self.visibility.len() {
            let byte = self.visibility[in_idx];
            in_idx += 1;
            if byte == 0 {
                let Some(&run) = self.visibility.get(in_idx) else {
                    break;
                };
                in_idx += 1;
                out_leaf += usize::from(run) * 8;
            } else {
                for bit in 0..8 {
                    if out_leaf + bit < self.vis_leafs && (byte & (1 << bit)) != 0 {
                        pvs[out_leaf + bit] = true;
                    }
                }
                out_leaf += 8;
            }
        }
        let mut mask = vec![false; face_count];
        for (v, &vis) in pvs.iter().enumerate() {
            if !vis {
                continue;
            }
            let Some(l) = self.leafs.get(v + 1) else {
                continue;
            };
            let first = usize::from(l.first_mark_surface);
            let end = first
                .saturating_add(usize::from(l.num_mark_surfaces))
                .min(self.mark_surfaces.len());
            for &fi in &self.mark_surfaces[first..end] {
                if let Some(slot) = mask.get_mut(usize::from(fi)) {
                    *slot = true;
                }
            }
        }
        Some(mask)
    }
}

struct UploadedMap {
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    batches: Vec<RenderBatch>,
    global_bg: wgpu::BindGroup,
    lightmap_tex: wgpu::Texture,
    lightmap_atlas: LightmapAtlas,
    dlight_infos: Vec<FaceDlightInfo>,
    prev_dlight_faces: Vec<bool>,
    lighting: Vec<u8>,
    last_style_values: [f32; 64],
    cached_gamma: GoldSrcGammaConfig,
    cached_tex_cfg: TextureUploadConfig,
    cached_overbright: bool,
    textures_raw: Vec<DecodedTexture>,
    tex_anim: TextureAnimTable,
    tex_views: Vec<wgpu::TextureView>,
    tex_bgs: Vec<wgpu::BindGroup>,
    total_triangles: usize,
    model_batches: Vec<Vec<usize>>,
    pvs_data: BspPvsData,
    world_vbuf: wgpu::Buffer,
    world_raw_indices: Vec<u32>,
    world_spans: Vec<Option<WorldFaceSpan>>,
    pvs_ibuf: wgpu::Buffer,
    pvs_ibuf_cap: usize,
    pvs_cached_leaf: usize,
    pvs_batches: Vec<(RenderMode, usize, u32, u32)>,
    pvs_triangles: usize,
}

pub struct BspRenderer {
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    opaque_pipeline: wgpu::RenderPipeline,
    alphatest_pipeline: wgpu::RenderPipeline,
    transparent_pipeline: wgpu::RenderPipeline,
    additive_pipeline: wgpu::RenderPipeline,
    add_one_pipeline: wgpu::RenderPipeline,
    color_format: wgpu::TextureFormat,
    msaa_samples: u32,
    ent_bgl: wgpu::BindGroupLayout,
    ent_buf: wgpu::Buffer,
    ent_cap: usize,
    ent_stride: u64,
    ent_bg: wgpu::BindGroup,
    skip_submodels: bool,
    global_bgl: wgpu::BindGroupLayout,
    tex_bgl: wgpu::BindGroupLayout,
    uniform_buf: wgpu::Buffer,
    lights_buf: wgpu::Buffer,
    lightmap_sampler: wgpu::Sampler,
    active_sampler: wgpu::Sampler,
    filter_mode: TextureFilterMode,
    anisotropy: u16,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
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

        let ent_raw_size = std::mem::size_of::<EntityUniforms>() as u64;
        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let ent_stride = ent_raw_size.div_ceil(align) * align;
        let ent_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bsp_entity_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(ent_raw_size),
                },
                count: None,
            }],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("bsp_pipeline_layout"),
            bind_group_layouts: &[Some(&global_bgl), Some(&tex_bgl), Some(&ent_bgl)],
            immediate_size: 0,
        });

        let msaa_samples = 1;
        let (opaque_pipeline, alphatest_pipeline, transparent_pipeline, additive_pipeline, add_one_pipeline) =
            build_all_pipelines(device, &layout, &shader, color_format, msaa_samples);

        let ent_cap = 64;
        let (ent_buf, ent_bg) = create_entity_buffer(device, &ent_bgl, ent_cap, ent_stride);

        let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bsp_uniform_buf"),
            contents: bytemuck::bytes_of(&BspUniforms {
                view_proj: Mat4::IDENTITY.to_cols_array_2d(),
                camera_pos: [0.0; 4],
                fog_color_density: [0.0; 4],
                gamma_params: [2.5, 2.0, 1.0, 0.0],
                misc_params: [0.0; 4],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let lights_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bsp_lights_buf"),
            contents: bytemuck::bytes_of(&GpuLightsUniform::default()),
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

        let filter_mode = TextureFilterMode::LinearMipmapLinear;
        let anisotropy = 1;
        let active_sampler = create_sampler(device, "bsp_tex_sampler", filter_mode, anisotropy, false);

        Self {
            shader,
            layout,
            opaque_pipeline,
            alphatest_pipeline,
            transparent_pipeline,
            additive_pipeline,
            add_one_pipeline,
            color_format,
            msaa_samples,
            ent_bgl,
            ent_buf,
            ent_cap,
            ent_stride,
            ent_bg,
            skip_submodels: false,
            global_bgl,
            tex_bgl,
            uniform_buf,
            lights_buf,
            lightmap_sampler,
            active_sampler,
            filter_mode,
            anisotropy,
            uploaded: None,
        }
    }

    pub fn lights_buffer(&self) -> &wgpu::Buffer {
        &self.lights_buf
    }

    pub fn rebuild_pipelines(&mut self, device: &wgpu::Device, msaa_samples: u32) {
        let samples = if msaa_samples >= 4 { 4 } else { 1 };
        if self.msaa_samples == samples {
            return;
        }
        self.msaa_samples = samples;
        let (o, a, t, add, add_one) =
            build_all_pipelines(device, &self.layout, &self.shader, self.color_format, samples);
        self.opaque_pipeline = o;
        self.alphatest_pipeline = a;
        self.transparent_pipeline = t;
        self.additive_pipeline = add;
        self.add_one_pipeline = add_one;
    }

    pub fn set_texture_filter(
        &mut self,
        device: &wgpu::Device,
        mode: TextureFilterMode,
        anisotropy: u16,
    ) {
        let aniso = anisotropy.clamp(1, 16);
        if self.filter_mode == mode && self.anisotropy == aniso {
            return;
        }
        self.filter_mode = mode;
        self.anisotropy = aniso;
        self.active_sampler = create_sampler(device, "bsp_tex_sampler", mode, aniso, false);
        if let Some(map) = &mut self.uploaded {
            map.tex_bgs = map
                .tex_views
                .iter()
                .enumerate()
                .map(|(i, view)| {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(&format!("bsp_tex_bg_{i}")),
                        layout: &self.tex_bgl,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&self.active_sampler),
                            },
                        ],
                    })
                })
                .collect();
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

        let lm = &map.lightmap_atlas;
        let (lightmap_tex, lm_view) = create_rgba_texture(
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.lights_buf.as_entire_binding(),
                },
            ],
        });

        let default_gamma = GoldSrcGammaConfig::default();
        let default_tables = GammaTables::build(default_gamma);
        let default_tex_cfg = TextureUploadConfig::default();

        let mut tex_views = Vec::with_capacity(map.textures.len());
        let mut tex_bgs = Vec::with_capacity(map.textures.len());
        for tex in &map.textures {
            let view = upload_rgba_texture_mipmapped_with_config(
                device,
                queue,
                &tex.name,
                tex.width.max(1),
                tex.height.max(1),
                &tex.rgba,
                Some(&default_tables),
                default_tex_cfg,
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
                        resource: wgpu::BindingResource::Sampler(&self.active_sampler),
                    },
                ],
            });
            tex_views.push(view);
            tex_bgs.push(bg);
        }

        let (wv, world_raw_indices, world_spans) = map.build_world_face_meshes();
        let gpu_wv: Vec<GpuBspVertex> = wv
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
        let fallback_vert = GpuBspVertex::zeroed();
        let world_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("bsp_world_pvs_vbuf"),
            contents: if gpu_wv.is_empty() {
                bytemuck::bytes_of(&fallback_vert)
            } else {
                bytemuck::cast_slice(&gpu_wv)
            },
            usage: wgpu::BufferUsages::VERTEX,
        });
        let pvs_ibuf_cap = world_raw_indices.len().max(64);
        let pvs_ibuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bsp_pvs_ibuf"),
            size: (pvs_ibuf_cap * std::mem::size_of::<u32>()) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let dlight_infos = map.build_face_dlight_infos();
        self.uploaded = Some(UploadedMap {
            vertex_buf,
            index_buf,
            batches: map.batches.clone(),
            global_bg,
            lightmap_tex,
            lightmap_atlas: lm.clone(),
            dlight_infos,
            prev_dlight_faces: Vec::new(),
            lighting: map.lighting.clone(),
            last_style_values: [1.0; 64],
            cached_gamma: default_gamma,
            cached_tex_cfg: default_tex_cfg,
            cached_overbright: false,
            textures_raw: map.textures.clone(),
            tex_anim: map.build_texture_anim_table(),
            tex_views,
            tex_bgs,
            total_triangles: map.render_indices.len() / 3,
            model_batches: group_batches_by_model(&map.batches),
            pvs_data: BspPvsData::from_map(map),
            world_vbuf,
            world_raw_indices,
            world_spans,
            pvs_ibuf,
            pvs_ibuf_cap,
            pvs_cached_leaf: usize::MAX,
            pvs_batches: Vec::new(),
            pvs_triangles: 0,
        });
    }

    pub fn update_lightstyles(&mut self, queue: &wgpu::Queue, style_values: &[f32; 64]) {
        let Some(map) = &mut self.uploaded else {
            return;
        };
        let tables = GammaTables::build(map.cached_gamma);
        let dirty = map.lightmap_atlas.rebuild_with_dlights(
            &map.lighting,
            &map.dlight_infos,
            style_values,
            &map.last_style_values,
            &[],
            &mut map.prev_dlight_faces,
            &tables,
            map.cached_overbright,
            false,
        );
        if dirty.is_empty() {
            return;
        }
        map.last_style_values = *style_values;
        upload_dirty_lightmap_rects(queue, &map.lightmap_tex, &map.lightmap_atlas, &dirty);
    }

    /// Synchronize texture `texgammatable` + POT mipmaps, `R_BuildLightMap` gamma LUTs,
    /// `gl_overbright`, animated lightstyles, and CPU `R_AddDynamicLights`.
    #[allow(clippy::too_many_arguments)]
    pub fn sync_gamma_and_lighting(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        style_values: &[f32; 64],
        gamma_cfg: GoldSrcGammaConfig,
        gl_overbright: bool,
        tex_cfg: TextureUploadConfig,
        dlights: &[BspDlight],
    ) {
        let Some(map) = &mut self.uploaded else {
            return;
        };
        let gamma_cfg = gamma_cfg.validated();
        let gamma_changed = map.cached_gamma != gamma_cfg;
        let tex_changed = gamma_changed || map.cached_tex_cfg != tex_cfg;
        let overbright_changed = map.cached_overbright != gl_overbright;
        let tables = GammaTables::build(gamma_cfg);

        if tex_changed {
            map.tex_views.clear();
            map.tex_bgs.clear();
            for tex in &map.textures_raw {
                let view = upload_rgba_texture_mipmapped_with_config(
                    device,
                    queue,
                    &tex.name,
                    tex.width.max(1),
                    tex.height.max(1),
                    &tex.rgba,
                    Some(&tables),
                    tex_cfg,
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
                            resource: wgpu::BindingResource::Sampler(&self.active_sampler),
                        },
                    ],
                });
                map.tex_views.push(view);
                map.tex_bgs.push(bg);
            }
            map.cached_tex_cfg = tex_cfg;
        }

        let force_all = gamma_changed || overbright_changed;
        let dirty = map.lightmap_atlas.rebuild_with_dlights(
            &map.lighting,
            &map.dlight_infos,
            style_values,
            &map.last_style_values,
            dlights,
            &mut map.prev_dlight_faces,
            &tables,
            gl_overbright,
            force_all,
        );
        map.last_style_values = *style_values;
        map.cached_gamma = gamma_cfg;
        map.cached_overbright = gl_overbright;
        if !dirty.is_empty() {
            upload_dirty_lightmap_rects(queue, &map.lightmap_tex, &map.lightmap_atlas, &dirty);
        }
    }

    pub fn set_skip_submodels(&mut self, skip: bool) {
        self.skip_submodels = skip;
    }

    pub fn prepare_entities(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        draws: &[&BrushDraw],
        time_sec: f32,
    ) -> Vec<PreparedBrushEntity> {
        let Some(map) = &self.uploaded else {
            return Vec::new();
        };
        let needed = draws.len() + 1;
        if needed > self.ent_cap {
            self.ent_cap = needed.next_power_of_two();
            let (buf, bg) =
                create_entity_buffer(device, &self.ent_bgl, self.ent_cap, self.ent_stride);
            self.ent_buf = buf;
            self.ent_bg = bg;
        }
        let mut bytes = vec![0u8; draws.len() * self.ent_stride as usize];
        let mut prepared = Vec::with_capacity(draws.len());
        for (i, d) in draws.iter().enumerate() {
            let submodel = d.submodel as usize;
            if map
                .model_batches
                .get(submodel)
                .is_none_or(|b| b.is_empty() || submodel == 0)
            {
                continue;
            }
            let (color, mode) = entity_shader_params(d.mode, d.blend, d.amt, d.color);
            let scroll = d.scroll_speed * time_sec;
            let u = EntityUniforms {
                model: d.matrix.to_cols_array_2d(),
                color,
                params: [mode, scroll, 0.0, 0.0],
            };
            let off = i * self.ent_stride as usize;
            let raw = bytemuck::bytes_of(&u);
            bytes[off..off + raw.len()].copy_from_slice(raw);
            prepared.push(PreparedBrushEntity {
                slot: i as u32 + 1,
                submodel,
                mode: d.mode,
                solid: d.solid,
                frame: d.frame,
            });
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.ent_buf, self.ent_stride, &bytes);
        }
        prepared
    }

    pub fn draw_entities<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        prepared: &[PreparedBrushEntity],
        time_sec: f32,
    ) {
        let Some(map) = &self.uploaded else {
            return;
        };
        pass.set_bind_group(0, &map.global_bg, &[]);
        pass.set_vertex_buffer(0, map.vertex_buf.slice(..));
        pass.set_index_buffer(map.index_buf.slice(..), wgpu::IndexFormat::Uint32);
        for p in prepared {
            let Some(batch_ids) = map.model_batches.get(p.submodel) else {
                continue;
            };
            pass.set_bind_group(2, &self.ent_bg, &[p.slot * self.ent_stride as u32]);
            for &bi in batch_ids {
                let batch = &map.batches[bi];
                let pipe = match p.mode {
                    EntRenderMode::Normal => match batch.render_mode {
                        RenderMode::Normal => &self.opaque_pipeline,
                        RenderMode::AlphaTest => &self.alphatest_pipeline,
                        RenderMode::Transparent => &self.transparent_pipeline,
                        RenderMode::Additive => &self.additive_pipeline,
                    },
                    EntRenderMode::TransAlpha => &self.alphatest_pipeline,
                    EntRenderMode::TransAdd => &self.add_one_pipeline,
                    EntRenderMode::TransColor | EntRenderMode::TransTexture | EntRenderMode::Glow => {
                        &self.transparent_pipeline
                    }
                };
                pass.set_pipeline(pipe);
                let resolved_tex = map.tex_anim.resolve(batch.texture_index, time_sec, p.frame);
                if let Some(bg) = map.tex_bgs.get(resolved_tex) {
                    pass.set_bind_group(1, bg, &[]);
                    pass.draw_indexed(
                        batch.first_index..batch.first_index + batch.index_count,
                        0,
                        0..1,
                    );
                }
            }
        }
    }

    pub fn prepare_world_pvs(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, camera_pos: Vec3, r_vis: bool) {
        let Some(map) = &mut self.uploaded else {
            return;
        };
        if !r_vis || map.world_raw_indices.is_empty() {
            return;
        }
        let leaf = map.pvs_data.find_leaf(camera_pos);
        if leaf == map.pvs_cached_leaf {
            return;
        }
        map.pvs_cached_leaf = leaf;
        map.pvs_batches.clear();
        map.pvs_triangles = 0;

        let Some(mask) = map.pvs_data.visible_faces(leaf, map.world_spans.len()) else {
            return;
        };
        let mut grouped: Vec<(u8, usize, RenderMode, &WorldFaceSpan)> = Vec::new();
        for (fi, span_opt) in map.world_spans.iter().enumerate() {
            if !mask.get(fi).copied().unwrap_or(false) {
                continue;
            }
            let Some(span) = span_opt else {
                continue;
            };
            let rank = match span.render_mode {
                RenderMode::Normal => 0,
                RenderMode::AlphaTest => 1,
                RenderMode::Transparent => 2,
                RenderMode::Additive => 3,
            };
            grouped.push((rank, span.texture_index, span.render_mode, span));
        }
        grouped.sort_unstable_by_key(|g| (g.0, g.1));

        let mut pvs_indices: Vec<u32> = Vec::with_capacity(map.world_raw_indices.len() / 2);
        for &(_, tex_idx, mode, span) in &grouped {
            let start = span.first_index as usize;
            let end = start + span.index_count as usize;
            let dst_first = pvs_indices.len() as u32;
            pvs_indices.extend_from_slice(&map.world_raw_indices[start..end]);
            let count = span.index_count;
            if let Some(last) = map.pvs_batches.last_mut()
                && last.0 == mode
                && last.1 == tex_idx
                && last.2 + last.3 == dst_first
            {
                last.3 += count;
            } else {
                map.pvs_batches.push((mode, tex_idx, dst_first, count));
            }
        }
        if pvs_indices.len() > map.pvs_ibuf_cap {
            map.pvs_ibuf_cap = pvs_indices.len().next_power_of_two();
            map.pvs_ibuf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("bsp_pvs_ibuf"),
                size: (map.pvs_ibuf_cap * std::mem::size_of::<u32>()) as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !pvs_indices.is_empty() {
            queue.write_buffer(&map.pvs_ibuf, 0, bytemuck::cast_slice(&pvs_indices));
        }
        map.pvs_triangles = pvs_indices.len() / 3;
    }

    pub fn draw<'a>(
        &'a self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'a>,
        params: &BspDrawParams,
        hidden_models: &[usize],
    ) -> BspRenderStats {
        let Some(map) = &self.uploaded else {
            return BspRenderStats {
                draw_calls: 0,
                triangles: 0,
            };
        };

        let (fog_rgb, fog_density) = params.fog.unwrap_or(([0.0, 0.0, 0.0], 0.0));
        let uniforms = BspUniforms {
            view_proj: params.view_proj.to_cols_array_2d(),
            camera_pos: [params.camera_pos.x, params.camera_pos.y, params.camera_pos.z, 1.0],
            fog_color_density: [fog_rgb[0], fog_rgb[1], fog_rgb[2], fog_density],
            gamma_params: [
                if params.gl_overbright { 1.0 } else { 0.0 },
                params.texgamma,
                params.brightness,
                if params.software_mode { 1.0 } else { 0.0 },
            ],
            misc_params: [params.time_sec, params.debug_mode, params.wave_height, 0.0],
        };
        queue.write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
        queue.write_buffer(&self.lights_buf, 0, bytemuck::bytes_of(&params.lights));

        pass.set_bind_group(0, &map.global_bg, &[]);
        pass.set_bind_group(2, &self.ent_bg, &[0]);

        if params.r_vis && !map.pvs_batches.is_empty() && self.skip_submodels {
            pass.set_vertex_buffer(0, map.world_vbuf.slice(..));
            pass.set_index_buffer(map.pvs_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            let mut last_mode: Option<RenderMode> = None;
            let mut calls = 0usize;
            for &(mode, tex_idx, first, count) in &map.pvs_batches {
                if last_mode != Some(mode) {
                    pass.set_pipeline(self.pipeline_for(mode));
                    last_mode = Some(mode);
                }
                let resolved_tex = map.tex_anim.resolve(tex_idx, params.time_sec, 0.0);
                if let Some(bg) = map.tex_bgs.get(resolved_tex) {
                    pass.set_bind_group(1, bg, &[]);
                    pass.draw_indexed(first..first + count, 0, 0..1);
                    calls += 1;
                }
            }
            return BspRenderStats {
                draw_calls: calls,
                triangles: map.pvs_triangles,
            };
        }

        pass.set_vertex_buffer(0, map.vertex_buf.slice(..));
        pass.set_index_buffer(map.index_buf.slice(..), wgpu::IndexFormat::Uint32);

        let mut last_mode: Option<RenderMode> = None;
        let mut calls = 0usize;
        for batch in &map.batches {
            if batch.model_index != 0
                && (self.skip_submodels || hidden_models.contains(&batch.model_index))
            {
                continue;
            }
            if last_mode != Some(batch.render_mode) {
                pass.set_pipeline(self.pipeline_for(batch.render_mode));
                last_mode = Some(batch.render_mode);
            }
            let resolved_tex = map.tex_anim.resolve(batch.texture_index, params.time_sec, 0.0);
            if let Some(bg) = map.tex_bgs.get(resolved_tex) {
                pass.set_bind_group(1, bg, &[]);
                pass.draw_indexed(batch.first_index..batch.first_index + batch.index_count, 0, 0..1);
                calls += 1;
            }
        }

        BspRenderStats {
            draw_calls: calls,
            triangles: map.total_triangles,
        }
    }

    fn pipeline_for(&self, mode: RenderMode) -> &wgpu::RenderPipeline {
        match mode {
            RenderMode::Normal => &self.opaque_pipeline,
            RenderMode::AlphaTest => &self.alphatest_pipeline,
            RenderMode::Transparent => &self.transparent_pipeline,
            RenderMode::Additive => &self.additive_pipeline,
        }
    }
}

fn build_all_pipelines(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    color_format: wgpu::TextureFormat,
    msaa_samples: u32,
) -> (
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
) {
    let opaque_pipeline = build_pipeline(
        device,
        layout,
        shader,
        color_format,
        msaa_samples,
        "bsp_opaque",
        Some(wgpu::Face::Back),
        None,
        true,
    );
    let alphatest_pipeline = build_pipeline(
        device,
        layout,
        shader,
        color_format,
        msaa_samples,
        "bsp_alphatest",
        None,
        None,
        true,
    );
    let transparent_pipeline = build_pipeline(
        device,
        layout,
        shader,
        color_format,
        msaa_samples,
        "bsp_transparent",
        None,
        Some(wgpu::BlendState::ALPHA_BLENDING),
        false,
    );
    let additive_pipeline = build_pipeline(
        device,
        layout,
        shader,
        color_format,
        msaa_samples,
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
    let add_one_pipeline = build_pipeline(
        device,
        layout,
        shader,
        color_format,
        msaa_samples,
        "bsp_add_one",
        None,
        Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        }),
        false,
    );
    (
        opaque_pipeline,
        alphatest_pipeline,
        transparent_pipeline,
        additive_pipeline,
        add_one_pipeline,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    color_format: wgpu::TextureFormat,
    msaa_samples: u32,
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
        multisample: wgpu::MultisampleState {
            count: msaa_samples,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview_mask: None,
        cache: None,
    })
}

fn upload_dirty_lightmap_rects(
    queue: &wgpu::Queue,
    lightmap_tex: &wgpu::Texture,
    atlas: &LightmapAtlas,
    dirty: &[(u32, u32, u32, u32)],
) {
    let aw = atlas.width;
    let ah = atlas.height;
    for &(x, y, w, h) in dirty {
        let start = ((y * aw + x) * 4) as usize;
        if start >= atlas.rgba.len() {
            continue;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: lightmap_tex,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &atlas.rgba[start..],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * aw),
                rows_per_image: Some(ah),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }
}

fn create_rgba_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> (wgpu::Texture, wgpu::TextureView) {
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
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(model_index: usize) -> RenderBatch {
        RenderBatch {
            texture_index: 0,
            render_mode: RenderMode::Normal,
            model_index,
            first_index: 0,
            index_count: 3,
        }
    }

    #[test]
    fn batches_group_by_model_index() {
        let batches = vec![batch(0), batch(2), batch(0), batch(1), batch(2)];
        assert_eq!(
            group_batches_by_model(&batches),
            vec![vec![0, 2], vec![3], vec![1, 4]]
        );
        assert_eq!(group_batches_by_model(&[]), vec![Vec::<usize>::new()]);
    }

    #[test]
    fn entity_shader_params_follow_r_set_render_mode() {
        let (_, m) = entity_shader_params(EntRenderMode::Normal, 0.2, 0.2, [0.5; 3]);
        assert_eq!(m, ENT_MODE_WORLD);

        let (c, m) = entity_shader_params(EntRenderMode::TransColor, 0.9, 0.25, [0.2, 0.4, 0.6]);
        assert_eq!(m, ENT_MODE_TRANS_COLOR);
        assert_eq!(c, [0.2, 0.4, 0.6, 0.25]);

        let (c, m) = entity_shader_params(EntRenderMode::TransAdd, 0.6, 0.1, [1.0; 3]);
        assert_eq!(m, ENT_MODE_ADD);
        assert_eq!(c[3], 0.6);

        let (_, m) = entity_shader_params(EntRenderMode::TransAlpha, 0.5, 0.5, [1.0; 3]);
        assert_eq!(m, ENT_MODE_TRANS_ALPHA);

        for mode in [EntRenderMode::TransTexture, EntRenderMode::Glow] {
            let (c, m) = entity_shader_params(mode, 0.4, 0.8, [1.0; 3]);
            assert_eq!(m, ENT_MODE_BLEND);
            assert_eq!(c[3], 0.4);
        }
    }

    #[test]
    fn r_turbsin_water_warp_matches_xash_warpsin() {
        assert_eq!(R_TURBSIN[0], 0.0);
        assert_eq!(R_TURBSIN[64], 4.0);
        assert_eq!(R_TURBSIN[192], -4.0);
        assert!(R_TURBSIN.iter().all(|&v| (-4.0..=4.0).contains(&v)));
        // Negative index wraps identically to C `(int)(v) & 255`.
        assert_eq!(turbsin_sample(-1.0), R_TURBSIN[255]);
        assert_eq!(turbsin_sample(-64.9), R_TURBSIN[192]);

        let uv = water_warp_uv([0.5, 0.25], [64.0, 64.0], 1.25);
        assert!((uv[0] - 0.5).abs() <= 4.0 / 64.0 + 1e-6);
        assert!((uv[1] - 0.25).abs() <= 4.0 / 64.0 + 1e-6);
        assert_ne!(uv, [0.5, 0.25]);

        let above = water_wave_offset([10.0, 20.0, 0.0], 0.5, 2.0, 64.0);
        let below = water_wave_offset([10.0, 20.0, 0.0], 0.5, 2.0, -64.0);
        assert!((above + below).abs() < 1e-5 && above > 0.0);
    }
}
