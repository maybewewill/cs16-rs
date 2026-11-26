//! GoldSrc StudioModel v10 (`.mdl`) renderer:
//! - GPU skeletal bone skinning (`<= 128` 3x4 affine bone matrices per posed instance, `6,144 B`
//!   uniform buffer strictly within WebGL2's `16,384 B` limit) over static rest-pose per-submodel
//!   vertex buffers, with cached `GammaTables` and GPU `lightgammatable` LUT shading
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
    BoneEvalParams, MAXSTUDIOBONES, MdlBone, MdlBoneAnim, MdlSeqEvent, STUDIO_NF_ADDITIVE,
    STUDIO_NF_CHROME, STUDIO_NF_FLATSHADE, STUDIO_NF_MASKED, STUDIO_RLOOP, StudioModel,
    goldsrc_angle_quat,
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

/// Maximum posed bones uploaded to the GPU vertex shader per studio instance (`MAXSTUDIOBONES`).
/// `128 * 3 * 16 = 6,144` bytes — strictly inside WebGL2's `16,384`-byte uniform buffer cap.
pub const MAX_GPU_BONES: usize = MAXSTUDIOBONES;

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

/// Append a feathered octagonal translucent disc (`8` triangles = `24` vertices) on the floor plane
/// orthogonal to `shadow.normal`, centered at `shadow.origin` (`alpha = shadow.alpha` at center
/// fading to `0.0` at `shadow.radius`).
fn push_shadow_disc_vertices(shadow: &StudioShadow, verts: &mut Vec<MdlShadowVertex>) {
    const SIDES: usize = 8;
    let normal = shadow.normal.normalize_or_zero();
    if normal.length_squared() < 1e-4 || shadow.alpha <= 0.0 || shadow.radius <= 0.0 {
        return;
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
}

/// Build a feathered octagonal translucent disc (`8` triangles = `24` vertices) on the floor
/// plane orthogonal to `shadow.normal`, centered at `shadow.origin` (`alpha = shadow.alpha` at
/// center fading to `0.0` at `shadow.radius`).
pub fn build_shadow_disc_vertices(shadow: &StudioShadow) -> Vec<MdlShadowVertex> {
    let mut verts = Vec::with_capacity(24);
    push_shadow_disc_vertices(shadow, &mut verts);
    verts
}

/// Static rest-pose GPU vertex uploaded once per `(bodypart, submodel)` at model load time.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MdlStaticVertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    pos_bone: u32,
    norm_bone: u32,
}

/// Per-draw studio uniforms (224 bytes — fits inside one 256-byte uniform alignment slot).
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
    studio_light_vec: [f32; 4],
    studio_light_params: [f32; 4],
}

/// Packed 1024-entry `GammaTables::lightgammatable` (`2 x u16` per `u32`, `2,048` bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LightGammaUniform {
    entries: [[u32; 4]; 128],
}

fn pack_light_gamma_uniform(tables: &GammaTables) -> LightGammaUniform {
    let mut entries = [[0u32; 4]; 128];
    for (i, vec) in entries.iter_mut().enumerate() {
        for (c, slot) in vec.iter_mut().enumerate() {
            let base = i * 8 + c * 2;
            let lo = u32::from(tables.lightgammatable[base]);
            let hi = u32::from(tables.lightgammatable[base + 1]);
            *slot = lo | (hi << 16);
        }
    }
    LightGammaUniform { entries }
}

/// Posed bone palette (`<= 128` 3x4 row-major affine matrices = `384 vec4<f32>` = `6,144` bytes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MdlBonesUniform {
    rows: [[f32; 4]; MAX_GPU_BONES * 3],
}

const BONE_MATRIX_BYTES: usize = 48; // 3 * vec4<f32>
const BONE_UNIFORM_BINDING_SIZE: u64 = std::mem::size_of::<MdlBonesUniform>() as u64;

#[inline]
fn write_bone_3x4(dst: &mut [u8], m: Mat4) {
    let rows: [[f32; 4]; 3] = [
        [m.x_axis.x, m.y_axis.x, m.z_axis.x, m.w_axis.x],
        [m.x_axis.y, m.y_axis.y, m.z_axis.y, m.w_axis.y],
        [m.x_axis.z, m.y_axis.z, m.z_axis.z, m.w_axis.z],
    ];
    dst[..BONE_MATRIX_BYTES].copy_from_slice(bytemuck::bytes_of(&rows));
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
    /// Team-colored X-Ray silhouette (`spec_xray 1`) rendered through walls in a second pass.
    pub xray_color: Option<[f32; 3]>,
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
            xray_color: None,
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

/// Submodel index within body part `index` selected by the entity's `body` value.
fn bodypart_submodel_index(
    parts: &[cs_assets::mdl::MdlBodyPart],
    index: usize,
    body: i32,
) -> Option<usize> {
    let part = parts.get(index)?;
    let count = part.submodels.len() as i64;
    if count == 0 {
        return None;
    }
    let base: i64 = parts[..index]
        .iter()
        .try_fold(1i64, |acc, p| acc.checked_mul((p.submodels.len() as i64).max(1)))?;
    let pick = (i64::from(body.max(0)) / base) % count;
    let idx = pick as usize;
    (idx < part.submodels.len()).then_some(idx)
}

/// Submodel of body part `index` selected by the entity's `body` value.
pub fn bodypart_submodel(
    parts: &[cs_assets::mdl::MdlBodyPart],
    index: usize,
    body: i32,
) -> Option<&cs_assets::mdl::MdlSubModel> {
    let idx = bodypart_submodel_index(parts, index, body)?;
    parts.get(index)?.submodels.get(idx)
}

struct CachedGpuTexture {
    view: wgpu::TextureView,
    bg: wgpu::BindGroup,
    flags: i32,
}

struct CachedSubModelMesh {
    skinref: usize,
    first_vertex: u32,
    vertex_count: u32,
}

struct CachedSubModelGpu {
    vertex_buf: Option<wgpu::Buffer>,
    meshes: Vec<CachedSubModelMesh>,
}

struct CachedBodyPartGpu {
    submodels: Vec<CachedSubModelGpu>,
}

struct CachedGpuModel {
    mdl: StudioModel,
    textures: Vec<CachedGpuTexture>,
    bodyparts: Vec<CachedBodyPartGpu>,
    model_bone_count: usize,
    gpu_bone_slots: usize,
    fallback_bone_slot: Option<u32>,
    upper_body_mask: [bool; MAX_GPU_BONES],
}

fn compute_upper_body_mask(mdl: &StudioModel) -> [bool; MAX_GPU_BONES] {
    let n = mdl.bones.len().min(MAX_GPU_BONES);
    let mut mask = [false; MAX_GPU_BONES];
    let Some(spine_idx) = mdl.bones[..n]
        .iter()
        .position(|b| b.name.eq_ignore_ascii_case("Bip01 Spine"))
    else {
        mask[..n].fill(true);
        return mask;
    };
    mask[spine_idx] = true;
    for i in (spine_idx + 1)..n {
        let p = mdl.bones[i].parent;
        if p >= 0 && (p as usize) < i && mask[p as usize] {
            mask[i] = true;
        }
    }
    mask
}

fn build_cached_gpu_model(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mdl: StudioModel,
    tables: &GammaTables,
    tex_cfg: TextureUploadConfig,
    tex_bgl: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
) -> CachedGpuModel {
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
            layout: tex_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        textures.push(CachedGpuTexture {
            view,
            bg,
            flags: tex.flags,
        });
    }

    let model_bone_count = mdl.bones.len().min(MAX_GPU_BONES);
    let mut has_oob_bone = false;
    'scan: for bp in &mdl.bodyparts {
        for sub in &bp.submodels {
            for mesh in &sub.meshes {
                for v in &mesh.vertices {
                    if usize::from(v.bone) >= model_bone_count {
                        has_oob_bone = true;
                        break 'scan;
                    }
                }
            }
        }
    }
    let fallback_bone_slot = if model_bone_count == 0 {
        Some(0u32)
    } else if has_oob_bone {
        Some(model_bone_count.min(MAX_GPU_BONES - 1) as u32)
    } else {
        None
    };
    let gpu_bone_slots = match fallback_bone_slot {
        Some(fb) => (fb as usize + 1).max(model_bone_count).min(MAX_GPU_BONES),
        None => model_bone_count.max(1),
    };

    let mut bodyparts = Vec::with_capacity(mdl.bodyparts.len());
    let mut sub_verts: Vec<MdlStaticVertex> = Vec::new();
    for bp in &mdl.bodyparts {
        let mut submodels = Vec::with_capacity(bp.submodels.len());
        for sub in &bp.submodels {
            sub_verts.clear();
            let mut meshes = Vec::with_capacity(sub.meshes.len());
            for mesh in &sub.meshes {
                if mesh.vertices.is_empty() {
                    continue;
                }
                let first_vertex = sub_verts.len() as u32;
                sub_verts.reserve(mesh.vertices.len());
                for v in &mesh.vertices {
                    let bone_idx = if usize::from(v.bone) < model_bone_count {
                        u32::from(v.bone)
                    } else {
                        fallback_bone_slot.unwrap_or(0)
                    };
                    sub_verts.push(MdlStaticVertex {
                        position: v.pos.to_array(),
                        normal: v.normal.to_array(),
                        uv: v.uv,
                        pos_bone: bone_idx,
                        norm_bone: bone_idx,
                    });
                }
                meshes.push(CachedSubModelMesh {
                    skinref: mesh.skinref,
                    first_vertex,
                    vertex_count: (sub_verts.len() as u32) - first_vertex,
                });
            }
            let vertex_buf = (!sub_verts.is_empty()).then(|| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("mdl_static_submodel_vbuf"),
                    contents: bytemuck::cast_slice(&sub_verts),
                    usage: wgpu::BufferUsages::VERTEX,
                })
            });
            submodels.push(CachedSubModelGpu { vertex_buf, meshes });
        }
        bodyparts.push(CachedBodyPartGpu { submodels });
    }

    let upper_body_mask = compute_upper_body_mask(&mdl);
    CachedGpuModel {
        mdl,
        textures,
        bodyparts,
        model_bone_count,
        gpu_bone_slots,
        fallback_bone_slot,
        upper_body_mask,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MdlPipelineKind {
    Opaque,
    Alpha,
    Additive,
    XRay,
}

#[derive(Clone, Copy)]
struct PreparedDraw {
    model_id: u32,
    bodypart_idx: u16,
    submodel_idx: u16,
    tex_idx: u16,
    first_vertex: u32,
    vertex_count: u32,
    uniform_slot: u32,
    bone_byte_offset: u32,
    pipe_kind: MdlPipelineKind,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PreparedMdlPass {
    total_draws: usize,
    xray_draws: usize,
}

impl PreparedMdlPass {
    /// Total prepared draw calls across all studio pipelines.
    pub fn total_draws(&self) -> usize {
        self.total_draws
    }

    /// Number of wall-occluded X-Ray silhouette draw calls (`MdlPipelineKind::XRay`).
    pub fn xray_draw_count(&self) -> usize {
        self.xray_draws
    }
}

struct BoneEvalScratch {
    bones: Box<[Mat4; MAX_GPU_BONES]>,
    merged_bones: Box<[Mat4; MAX_GPU_BONES]>,
    pose_upper: Box<[(Vec3, Quat); MAX_GPU_BONES]>,
    pose_prev: Box<[(Vec3, Quat); MAX_GPU_BONES]>,
    pose_gait: Box<[(Vec3, Quat); MAX_GPU_BONES]>,
}

impl BoneEvalScratch {
    fn new() -> Self {
        Self {
            bones: Box::new([Mat4::IDENTITY; MAX_GPU_BONES]),
            merged_bones: Box::new([Mat4::IDENTITY; MAX_GPU_BONES]),
            pose_upper: Box::new([(Vec3::ZERO, Quat::IDENTITY); MAX_GPU_BONES]),
            pose_prev: Box::new([(Vec3::ZERO, Quat::IDENTITY); MAX_GPU_BONES]),
            pose_gait: Box::new([(Vec3::ZERO, Quat::IDENTITY); MAX_GPU_BONES]),
        }
    }
}

fn compute_controller_adj(mdl: &StudioModel, controller: &[u8; 4], mouth: u8) -> [f32; 8] {
    let mut adj = [0.0f32; 8];
    for ctrl in &mdl.controllers {
        let idx = ctrl.index;
        if !(0..8).contains(&idx) {
            continue;
        }
        let raw = if idx == 4 {
            f32::from(mouth) / 64.0
        } else if let Some(&v) = controller.get(idx as usize) {
            f32::from(v) / 255.0
        } else {
            continue;
        };
        let span = ctrl.end - ctrl.start;
        let value = if ctrl.kind & STUDIO_RLOOP != 0 {
            ctrl.start + raw * span
        } else {
            ctrl.start + raw.clamp(0.0, 1.0) * span
        };
        adj[idx as usize] = value;
    }
    adj
}

fn eval_sequence_pose_into(
    mdl: &StudioModel,
    seq_idx: usize,
    frame_time: f32,
    loop_anim: bool,
    blending: [u8; 2],
    adj: &[f32; 8],
    out: &mut [(Vec3, Quat); MAX_GPU_BONES],
) {
    let n = mdl.bones.len().min(MAX_GPU_BONES);
    let seq = mdl.sequences.get(seq_idx);
    let (f0, f1, frac) = if let Some(s) = seq {
        let nf = s.num_frames.max(1);
        if nf <= 1 {
            (0, 0, 0.0)
        } else if loop_anim {
            let period = (nf - 1) as f32;
            let f_wrapped = frame_time.rem_euclid(period);
            let i0 = (f_wrapped.floor() as usize).min(nf - 2);
            (i0, i0 + 1, f_wrapped - f_wrapped.floor())
        } else {
            let f_clamped = frame_time.clamp(0.0, (nf - 1) as f32);
            let i0 = (f_clamped.floor() as usize).min(nf - 1);
            let i1 = (i0 + 1).min(nf - 1);
            (i0, i1, f_clamped - f_clamped.floor())
        }
    } else {
        (0, 0, 0.0)
    };

    let sample_blend = |blend: &[MdlBoneAnim], i: usize, bone: &MdlBone| -> (Vec3, Quat) {
        if let Some(anim) = blend.get(i)
            && !anim.frames.is_empty()
        {
            let mut ch0 = anim.frames[f0.min(anim.frames.len() - 1)];
            let mut ch1 = anim.frames[f1.min(anim.frames.len() - 1)];
            for c in 0..6 {
                let ci = bone.bonecontroller[c];
                if (0..8).contains(&ci) {
                    ch0[c] += adj[ci as usize];
                    ch1[c] += adj[ci as usize];
                }
            }
            let p0 = bone.default_pos + Vec3::new(ch0[0], ch0[1], ch0[2]) * bone.pos_scale;
            let p1 = bone.default_pos + Vec3::new(ch1[0], ch1[1], ch1[2]) * bone.pos_scale;
            let r0 = bone.default_rot + Vec3::new(ch0[3], ch0[4], ch0[5]) * bone.rot_scale;
            let r1 = bone.default_rot + Vec3::new(ch1[3], ch1[4], ch1[5]) * bone.rot_scale;
            (
                p0.lerp(p1, frac),
                goldsrc_angle_quat(r0).slerp(goldsrc_angle_quat(r1), frac),
            )
        } else {
            (bone.default_pos, goldsrc_angle_quat(bone.default_rot))
        }
    };

    let blends = seq.map(|s| s.blends.as_slice()).unwrap_or(&[]);
    let nb = blends.len();
    if nb <= 1 {
        let b0 = blends.first().map(Vec::as_slice).unwrap_or(&[]);
        for (i, bone) in mdl.bones.iter().take(n).enumerate() {
            out[i] = sample_blend(b0, i, bone);
        }
        return;
    }
    let blend_type1 = seq.map_or(0, |s| s.blend_type[1]);
    if blend_type1 == 0 || nb < 4 {
        let x = (f32::from(blending[0]) / 255.0) * ((nb - 1) as f32);
        let i0 = (x.floor() as usize).min(nb - 1);
        let i1 = (i0 + 1).min(nb - 1);
        let t = (x - i0 as f32).clamp(0.0, 1.0);
        for (i, bone) in mdl.bones.iter().take(n).enumerate() {
            let (p0, q0) = sample_blend(&blends[i0], i, bone);
            let (p1, q1) = sample_blend(&blends[i1], i, bone);
            out[i] = (p0.lerp(p1, t), q0.slerp(q1, t));
        }
    } else {
        let cols = (nb as f32).sqrt().round().max(1.0) as usize;
        let rows = nb / cols;
        let fx = (f32::from(blending[0]) / 255.0) * (cols.saturating_sub(1) as f32);
        let fy = (f32::from(blending[1]) / 255.0) * (rows.saturating_sub(1) as f32);
        let x0 = (fx.floor() as usize).min(cols - 1);
        let x1 = (x0 + 1).min(cols - 1);
        let y0 = (fy.floor() as usize).min(rows.saturating_sub(1));
        let y1 = (y0 + 1).min(rows.saturating_sub(1));
        let tx = (fx - x0 as f32).clamp(0.0, 1.0);
        let ty = (fy - y0 as f32).clamp(0.0, 1.0);
        for (i, bone) in mdl.bones.iter().take(n).enumerate() {
            let (p00, q00) = sample_blend(&blends[(y0 * cols + x0).min(nb - 1)], i, bone);
            let (p10, q10) = sample_blend(&blends[(y0 * cols + x1).min(nb - 1)], i, bone);
            let (p01, q01) = sample_blend(&blends[(y1 * cols + x0).min(nb - 1)], i, bone);
            let (p11, q11) = sample_blend(&blends[(y1 * cols + x1).min(nb - 1)], i, bone);
            let pa = p00.lerp(p10, tx);
            let pb = p01.lerp(p11, tx);
            let qa = q00.slerp(q10, tx);
            let qb = q01.slerp(q11, tx);
            out[i] = (pa.lerp(pb, ty), qa.slerp(qb, ty));
        }
    }
}

/// Zero-allocation hierarchical bone evaluation matching [`StudioModel::evaluate_bones_full`].
fn eval_bones_into(
    mdl: &StudioModel,
    upper_body_mask: &[bool; MAX_GPU_BONES],
    params: &BoneEvalParams,
    scratch: &mut BoneEvalScratch,
) -> usize {
    let n = mdl.bones.len().min(MAX_GPU_BONES);
    if n == 0 {
        return 0;
    }
    let adj = compute_controller_adj(mdl, &params.controller, params.mouth);
    eval_sequence_pose_into(
        mdl,
        params.sequence,
        params.frame,
        params.loop_anim,
        params.blending,
        &adj,
        &mut scratch.pose_upper,
    );
    let prev_weight = params.prev_sequence.map(|(p_seq, p_frame, p_loop, weight)| {
        eval_sequence_pose_into(
            mdl,
            p_seq,
            p_frame,
            p_loop,
            params.blending,
            &adj,
            &mut scratch.pose_prev,
        );
        weight.clamp(0.0, 1.0)
    });
    let has_gait = if let Some((g_seq, g_frame)) = params.gait {
        eval_sequence_pose_into(
            mdl,
            g_seq,
            g_frame,
            true,
            [0, 0],
            &adj,
            &mut scratch.pose_gait,
        );
        true
    } else {
        false
    };

    for (i, bone) in mdl.bones.iter().take(n).enumerate() {
        let (mut pos, mut quat) = if has_gait {
            if upper_body_mask[i] {
                scratch.pose_upper[i]
            } else {
                scratch.pose_gait[i]
            }
        } else {
            scratch.pose_upper[i]
        };
        if let Some(weight) = prev_weight
            && weight > 0.0
        {
            let (p_pos, p_quat) = scratch.pose_prev[i];
            let t = 1.0 - weight;
            pos = p_pos.lerp(pos, t);
            quat = p_quat.slerp(quat, t);
        }
        let local = Mat4::from_rotation_translation(quat, pos);
        scratch.bones[i] = if bone.parent >= 0 && (bone.parent as usize) < i {
            scratch.bones[bone.parent as usize] * local
        } else {
            local
        };
    }
    n
}

/// Zero-allocation `EF_BONEMERGE` evaluation matching [`StudioModel::evaluate_bones_merged`].
fn eval_bones_merged_into(
    child: &StudioModel,
    parent: &StudioModel,
    parent_bones: &[Mat4],
    out: &mut [Mat4; MAX_GPU_BONES],
) -> usize {
    let n = child.bones.len().min(MAX_GPU_BONES);
    for (i, bone) in child.bones.iter().take(n).enumerate() {
        if let Some(p_idx) = parent
            .bones
            .iter()
            .position(|pb| pb.name.eq_ignore_ascii_case(&bone.name))
            && let Some(&p_mat) = parent_bones.get(p_idx)
        {
            out[i] = p_mat;
            continue;
        }
        let local = Mat4::from_rotation_translation(
            goldsrc_angle_quat(bone.default_rot),
            bone.default_pos,
        );
        out[i] = if bone.parent >= 0 && (bone.parent as usize) < i {
            out[bone.parent as usize] * local
        } else {
            local
        };
    }
    n
}

pub struct MdlRenderer {
    color_format: wgpu::TextureFormat,
    msaa_samples: u32,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    opaque_pipeline: wgpu::RenderPipeline,
    alpha_pipeline: wgpu::RenderPipeline,
    additive_pipeline: wgpu::RenderPipeline,
    xray_pipeline: wgpu::RenderPipeline,
    uniform_bgl: wgpu::BindGroupLayout,
    tex_bgl: wgpu::BindGroupLayout,
    bone_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    filter_mode: TextureFilterMode,
    anisotropy: u16,
    models: Vec<Option<CachedGpuModel>>,
    exact_path_to_id: HashMap<String, u32>,
    lower_path_to_id: HashMap<String, u32>,
    uniform_buf: wgpu::Buffer,
    uniform_cap: usize,
    uniform_stride: u64,
    light_gamma_buf: wgpu::Buffer,
    uniform_bg: wgpu::BindGroup,
    bone_buf: wgpu::Buffer,
    bone_buf_cap: usize,
    bone_align: usize,
    bone_bg: wgpu::BindGroup,
    shadow_shader: wgpu::ShaderModule,
    shadow_pipeline_layout: wgpu::PipelineLayout,
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_uniform_buf: wgpu::Buffer,
    shadow_uniform_bg: wgpu::BindGroup,
    shadow_vbuf: wgpu::Buffer,
    shadow_vbuf_cap: usize,
    shadow_vertex_count: u32,
    cached_gamma: GoldSrcGammaConfig,
    cached_gamma_tables: GammaTables,
    cached_tex_cfg: TextureUploadConfig,
    prepared_draws: Vec<PreparedDraw>,
    scratch_uniform_bytes: Vec<u8>,
    scratch_bone_bytes: Vec<u8>,
    scratch_shadow_verts: Vec<MdlShadowVertex>,
    bone_scratch: BoneEvalScratch,
    frame_upload_bytes: u64,
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
    let make_pipe = |label: &str,
                     blend: Option<wgpu::BlendState>,
                     depth_write: bool,
                     depth_compare: wgpu::CompareFunction| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MdlStaticVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,
                        1 => Float32x3,
                        2 => Float32x2,
                        3 => Uint32,
                        4 => Uint32,
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
                depth_compare: Some(depth_compare),
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
        make_pipe("mdl_opaque", None, true, wgpu::CompareFunction::LessEqual),
        make_pipe(
            "mdl_alpha",
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
            wgpu::CompareFunction::LessEqual,
        ),
        make_pipe("mdl_additive", add_blend, false, wgpu::CompareFunction::LessEqual),
        make_pipe(
            "mdl_xray",
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
            wgpu::CompareFunction::Greater,
        ),
    )
}

fn create_mdl_uniform_bg(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buf: &wgpu::Buffer,
    raw_size: u64,
    light_gamma_buf: &wgpu::Buffer,
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
                resource: light_gamma_buf.as_entire_binding(),
            },
        ],
    })
}

fn create_mdl_bone_bg(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    bone_buf: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mdl_bone_bg"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: bone_buf,
                offset: 0,
                size: wgpu::BufferSize::new(BONE_UNIFORM_BINDING_SIZE),
            }),
        }],
    })
}

impl MdlRenderer {
    pub fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mdl.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mdl.wgsl").into()),
        });

        let align = device.limits().min_uniform_buffer_offset_alignment.max(1) as u64;
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
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<LightGammaUniform>() as u64,
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

        let bone_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mdl_bone_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(BONE_UNIFORM_BINDING_SIZE),
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mdl_pipeline_layout"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&tex_bgl), Some(&bone_bgl)],
            immediate_size: 0,
        });

        let msaa_samples = 1;
        let (opaque_pipeline, alpha_pipeline, additive_pipeline, xray_pipeline) =
            create_mdl_pipelines(device, &shader, &pipeline_layout, color_format, msaa_samples);

        let filter_mode = TextureFilterMode::LinearMipmapLinear;
        let anisotropy = 1;
        let sampler = create_sampler(device, "mdl_sampler", filter_mode, anisotropy, false);

        let uniform_cap = 256;
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mdl_dynamic_ubuf"),
            size: (uniform_cap as u64) * uniform_stride,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let cached_gamma = GoldSrcGammaConfig::default().validated();
        let cached_gamma_tables = GammaTables::build(cached_gamma);
        let light_gamma_init = pack_light_gamma_uniform(&cached_gamma_tables);
        let light_gamma_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mdl_light_gamma_lut"),
            contents: bytemuck::bytes_of(&light_gamma_init),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bg = create_mdl_uniform_bg(
            device,
            &uniform_bgl,
            &uniform_buf,
            raw_size,
            &light_gamma_buf,
        );

        let bone_buf_cap = 64 * 1024;
        let bone_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mdl_bone_palette_ubuf"),
            size: bone_buf_cap as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bone_bg = create_mdl_bone_bg(device, &bone_bgl, &bone_buf);

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
            xray_pipeline,
            uniform_bgl,
            tex_bgl,
            bone_bgl,
            sampler,
            filter_mode,
            anisotropy,
            models: Vec::new(),
            exact_path_to_id: HashMap::new(),
            lower_path_to_id: HashMap::new(),
            uniform_buf,
            uniform_cap,
            uniform_stride,
            light_gamma_buf,
            uniform_bg,
            bone_buf,
            bone_buf_cap,
            bone_align: align as usize,
            bone_bg,
            shadow_shader,
            shadow_pipeline_layout,
            shadow_pipeline,
            shadow_uniform_buf,
            shadow_uniform_bg,
            shadow_vbuf,
            shadow_vbuf_cap,
            shadow_vertex_count: 0,
            cached_gamma,
            cached_gamma_tables,
            cached_tex_cfg: TextureUploadConfig::default(),
            prepared_draws: Vec::new(),
            scratch_uniform_bytes: Vec::new(),
            scratch_bone_bytes: Vec::new(),
            scratch_shadow_verts: Vec::new(),
            bone_scratch: BoneEvalScratch::new(),
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
        let (opaque, alpha, additive, xray) = create_mdl_pipelines(
            device,
            &self.shader,
            &self.pipeline_layout,
            self.color_format,
            samples,
        );
        self.opaque_pipeline = opaque;
        self.alpha_pipeline = alpha;
        self.additive_pipeline = additive;
        self.xray_pipeline = xray;
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
        for cached in self.models.iter_mut().flatten() {
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
        self.models.clear();
        self.exact_path_to_id.clear();
        self.lower_path_to_id.clear();
        self.prepared_draws.clear();
    }

    fn lookup_model_id(&self, model_path: &str) -> Option<u32> {
        if let Some(&id) = self.exact_path_to_id.get(model_path) {
            return Some(id);
        }
        let lower = model_path.to_ascii_lowercase();
        self.lower_path_to_id.get(&lower).copied()
    }

    fn ensure_loaded(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        rel_path: &str,
        tex_cfg: TextureUploadConfig,
    ) -> Option<u32> {
        if let Some(&id) = self.exact_path_to_id.get(rel_path) {
            return self
                .models
                .get(id as usize)
                .and_then(|o| o.as_ref())
                .map(|_| id);
        }

        let lower = rel_path.to_ascii_lowercase();
        if let Some(&id) = self.lower_path_to_id.get(&lower) {
            self.exact_path_to_id.insert(rel_path.to_string(), id);
            return self
                .models
                .get(id as usize)
                .and_then(|o| o.as_ref())
                .map(|_| id);
        }

        let loaded = match StudioModel::load(vfs, &lower) {
            Ok(mdl) => Some(build_cached_gpu_model(
                device,
                queue,
                mdl,
                &self.cached_gamma_tables,
                tex_cfg,
                &self.tex_bgl,
                &self.sampler,
            )),
            Err(err) => {
                warn!(model = %lower, %err, "Could not load StudioModel");
                None
            }
        };
        let is_some = loaded.is_some();
        let id = self.models.len() as u32;
        self.models.push(loaded);
        self.lower_path_to_id.insert(lower, id);
        self.exact_path_to_id.insert(rel_path.to_string(), id);
        is_some.then_some(id)
    }

    /// Evaluate the 4 world-space attachment points of a studio model from cached GPU data.
    pub fn query_attachments(
        &self,
        model_path: &str,
        params: &BoneEvalParams,
        entity_transform: Mat4,
    ) -> Option<[Vec3; 4]> {
        let id = self.lookup_model_id(model_path)?;
        let cached = self.models.get(id as usize)?.as_ref()?;
        let bones = cached.mdl.evaluate_bones_full(params);
        Some(cached.mdl.evaluate_attachments(entity_transform, &bones))
    }

    /// Inspect sequence animation events for a cached studio model.
    pub fn sequence_events(&self, model_path: &str, sequence: usize) -> &[MdlSeqEvent] {
        self.lookup_model_id(model_path)
            .and_then(|id| self.models.get(id as usize))
            .and_then(|o| o.as_ref())
            .and_then(|c| c.mdl.sequences.get(sequence))
            .map(|s| s.events.as_slice())
            .unwrap_or(&[])
    }

    /// Pose studio model bone hierarchies on CPU and stage GPU skinning palettes & draw uniforms
    /// with full GoldSrc animation, blending, render modes, lighting, fog, and gamma.
    pub fn prepare_studio(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vfs: &Vfs,
        instances: &[StudioInstance<'_>],
        params: &MdlPassParams,
    ) -> PreparedMdlPass {
        self.prepared_draws.clear();
        self.scratch_uniform_bytes.clear();
        self.scratch_bone_bytes.clear();
        let mut uniform_slot: u32 = 0;
        let mut xray_draws: usize = 0;

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
        let gamma_changed = self.cached_gamma != gamma_cfg;
        if gamma_changed {
            self.cached_gamma = gamma_cfg;
            self.cached_gamma_tables = GammaTables::build(gamma_cfg);
            let packed = pack_light_gamma_uniform(&self.cached_gamma_tables);
            queue.write_buffer(&self.light_gamma_buf, 0, bytemuck::bytes_of(&packed));
            self.frame_upload_bytes += std::mem::size_of::<LightGammaUniform>() as u64;
        }
        if gamma_changed || self.cached_tex_cfg != tex_cfg {
            self.clear_cache();
            self.cached_tex_cfg = tex_cfg;
        }

        let uniform_stride = self.uniform_stride as usize;
        let bone_align = self.bone_align;

        for inst in instances {
            let Some(model_id) = self.ensure_loaded(device, queue, vfs, inst.model_path, tex_cfg)
            else {
                continue;
            };
            let attached_id = inst
                .attached_model
                .and_then(|p| self.ensure_loaded(device, queue, vfs, p, tex_cfg));

            let Some(Some(cached)) = self.models.get(model_id as usize) else {
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
            let bone_count = eval_bones_into(
                &cached.mdl,
                &cached.upper_body_mask,
                &eval_params,
                &mut self.bone_scratch,
            );

            let explode_scale = if inst.renderfx == RENDERFX_EXPLODE {
                Vec3::splat(1.12)
            } else {
                Vec3::ONE
            };
            let base_xf = inst.transform * Mat4::from_scale(explode_scale);
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

            let mut push_posed_model = |m_id: u32,
                                        cached_m: &CachedGpuModel,
                                        bone_byte_offset: u32,
                                        shell_only: bool| {
                for bi in 0..cached_m.mdl.bodyparts.len() {
                    let Some(sub_idx) = bodypart_submodel_index(&cached_m.mdl.bodyparts, bi, inst.body)
                    else {
                        continue;
                    };
                    let Some(sub_gpu) = cached_m
                        .bodyparts
                        .get(bi)
                        .and_then(|bp| bp.submodels.get(sub_idx))
                    else {
                        continue;
                    };
                    if sub_gpu.vertex_buf.is_none() || sub_gpu.meshes.is_empty() {
                        continue;
                    }
                    for mesh in &sub_gpu.meshes {
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
                                light_color.x,
                                light_color.y,
                                light_color.z,
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
                                (tex_flags
                                    & (STUDIO_NF_FLATSHADE
                                        | STUDIO_NF_CHROME
                                        | STUDIO_NF_ADDITIVE
                                        | STUDIO_NF_MASKED)) as f32,
                                shell_offset,
                                light_mul,
                            ],
                            studio_light_vec: [
                                plight_vec.x,
                                plight_vec.y,
                                plight_vec.z,
                                ambient_light,
                            ],
                            studio_light_params: [shade_light, 0.0, 0.0, 0.0],
                        };
                        let start = self.scratch_uniform_bytes.len();
                        self.scratch_uniform_bytes.resize(start + uniform_stride, 0);
                        let raw = bytemuck::bytes_of(&u);
                        self.scratch_uniform_bytes[start..start + raw.len()].copy_from_slice(raw);

                        self.prepared_draws.push(PreparedDraw {
                            model_id: m_id,
                            bodypart_idx: bi as u16,
                            submodel_idx: sub_idx as u16,
                            tex_idx: tex_idx as u16,
                            first_vertex: mesh.first_vertex,
                            vertex_count: mesh.vertex_count,
                            uniform_slot,
                            bone_byte_offset,
                            pipe_kind,
                        });
                        uniform_slot += 1;

                        if !shell_only
                            && let Some(xray_rgb) = inst.xray_color
                        {
                            let xray_u = MdlUniforms {
                                render_color: [xray_rgb[0], xray_rgb[1], xray_rgb[2], 0.65],
                                flags_params: [6.0, 0.0, 0.0, 1.0],
                                ..u
                            };
                            let x_start = self.scratch_uniform_bytes.len();
                            self.scratch_uniform_bytes.resize(x_start + uniform_stride, 0);
                            let x_raw = bytemuck::bytes_of(&xray_u);
                            self.scratch_uniform_bytes[x_start..x_start + x_raw.len()]
                                .copy_from_slice(x_raw);

                            self.prepared_draws.push(PreparedDraw {
                                model_id: m_id,
                                bodypart_idx: bi as u16,
                                submodel_idx: sub_idx as u16,
                                tex_idx: tex_idx as u16,
                                first_vertex: mesh.first_vertex,
                                vertex_count: mesh.vertex_count,
                                uniform_slot,
                                bone_byte_offset,
                                pipe_kind: MdlPipelineKind::XRay,
                            });
                            uniform_slot += 1;
                            xray_draws += 1;
                        }
                    }
                }
            };

            let main_bone_offset = stage_bone_palette(
                &mut self.scratch_bone_bytes,
                bone_align,
                cached,
                base_xf,
                &self.bone_scratch.bones[..bone_count],
            );
            push_posed_model(model_id, cached, main_bone_offset, false);
            if inst.renderfx == RENDERFX_GLOW_SHELL {
                push_posed_model(model_id, cached, main_bone_offset, true);
            }

            if let Some(att_id) = attached_id
                && let Some(Some(att_cached)) = self.models.get(att_id as usize)
            {
                let merged_count = eval_bones_merged_into(
                    &att_cached.mdl,
                    &cached.mdl,
                    &self.bone_scratch.bones[..bone_count],
                    &mut self.bone_scratch.merged_bones,
                );
                let att_bone_offset = stage_bone_palette(
                    &mut self.scratch_bone_bytes,
                    bone_align,
                    att_cached,
                    base_xf,
                    &self.bone_scratch.merged_bones[..merged_count],
                );
                push_posed_model(att_id, att_cached, att_bone_offset, false);
                if inst.renderfx == RENDERFX_GLOW_SHELL {
                    push_posed_model(att_id, att_cached, att_bone_offset, true);
                }
            }
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
                &self.light_gamma_buf,
            );
        }
        if !self.scratch_uniform_bytes.is_empty() {
            queue.write_buffer(&self.uniform_buf, 0, &self.scratch_uniform_bytes);
            self.frame_upload_bytes += self.scratch_uniform_bytes.len() as u64;
        }

        // Ensure every dynamic offset + BONE_UNIFORM_BINDING_SIZE (6,144 B) stays strictly in-bounds
        // of `bone_buf` for both WebGPU and WebGL2 validation.
        let required_bone_cap = self
            .scratch_bone_bytes
            .len()
            .saturating_add(BONE_UNIFORM_BINDING_SIZE as usize);
        if required_bone_cap > self.bone_buf_cap {
            self.bone_buf_cap = required_bone_cap.next_power_of_two();
            self.bone_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mdl_bone_palette_ubuf"),
                size: self.bone_buf_cap as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.bone_bg = create_mdl_bone_bg(device, &self.bone_bgl, &self.bone_buf);
        }
        if !self.scratch_bone_bytes.is_empty() {
            queue.write_buffer(&self.bone_buf, 0, &self.scratch_bone_bytes);
            self.frame_upload_bytes += self.scratch_bone_bytes.len() as u64;
        }

        PreparedMdlPass {
            total_draws: self.prepared_draws.len(),
            xray_draws,
        }
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, prepared: &PreparedMdlPass) {
        let _ = self.draw_counted(pass, prepared);
    }

    pub(crate) fn draw_counted<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        prepared: &PreparedMdlPass,
    ) -> usize {
        self.draw_filtered(pass, prepared, |_| true)
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
        self.frame_upload_bytes += std::mem::size_of::<ShadowUniforms>() as u64;
        if shadows.is_empty() {
            self.shadow_vertex_count = 0;
            return;
        }
        self.scratch_shadow_verts.clear();
        for s in shadows {
            push_shadow_disc_vertices(s, &mut self.scratch_shadow_verts);
        }
        self.shadow_vertex_count = self.scratch_shadow_verts.len() as u32;
        if self.scratch_shadow_verts.len() > self.shadow_vbuf_cap {
            self.shadow_vbuf_cap = self.scratch_shadow_verts.len().next_power_of_two();
            self.shadow_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mdl_shadow_vbuf"),
                size: (self.shadow_vbuf_cap * std::mem::size_of::<MdlShadowVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !self.scratch_shadow_verts.is_empty() {
            queue.write_buffer(
                &self.shadow_vbuf,
                0,
                bytemuck::cast_slice(&self.scratch_shadow_verts),
            );
            self.frame_upload_bytes +=
                (self.scratch_shadow_verts.len() * std::mem::size_of::<MdlShadowVertex>()) as u64;
        }
    }

    /// Draw prepared studio blob shadows (depth-read `LessEqual`, no depth-write, alpha-blended).
    pub fn draw_shadows<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        let _ = self.draw_shadows_counted(pass);
    }

    pub(crate) fn draw_shadows_counted<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) -> usize {
        if self.shadow_vertex_count == 0 {
            return 0;
        }
        pass.set_pipeline(&self.shadow_pipeline);
        pass.set_bind_group(0, &self.shadow_uniform_bg, &[]);
        pass.set_vertex_buffer(0, self.shadow_vbuf.slice(..));
        pass.draw(0..self.shadow_vertex_count, 0..1);
        1
    }

    pub fn draw_opaque<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, prepared: &PreparedMdlPass) {
        let _ = self.draw_opaque_counted(pass, prepared);
    }

    pub(crate) fn draw_opaque_counted<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        prepared: &PreparedMdlPass,
    ) -> usize {
        self.draw_filtered(pass, prepared, |k| k == MdlPipelineKind::Opaque)
    }

    pub fn draw_translucent<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        prepared: &PreparedMdlPass,
    ) {
        let _ = self.draw_translucent_counted(pass, prepared);
    }

    pub(crate) fn draw_translucent_counted<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        prepared: &PreparedMdlPass,
    ) -> usize {
        self.draw_filtered(
            pass,
            prepared,
            |k| matches!(k, MdlPipelineKind::Alpha | MdlPipelineKind::Additive),
        )
    }

    /// Draw wall-occluded X-Ray player silhouettes (`CompareFunction::Greater`, no depth write).
    pub fn draw_xray<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, prepared: &PreparedMdlPass) {
        let _ = self.draw_xray_counted(pass, prepared);
    }

    pub(crate) fn draw_xray_counted<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        prepared: &PreparedMdlPass,
    ) -> usize {
        if prepared.xray_draws == 0 {
            return 0;
        }
        self.draw_filtered(pass, prepared, |k| k == MdlPipelineKind::XRay)
    }

    fn draw_filtered<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        prepared: &PreparedMdlPass,
        pred: fn(MdlPipelineKind) -> bool,
    ) -> usize {
        if prepared.total_draws == 0 || self.prepared_draws.is_empty() {
            return 0;
        }
        let mut draw_calls = 0usize;
        let mut last_pipe: Option<MdlPipelineKind> = None;
        let mut last_vbuf: Option<(u32, u16, u16)> = None;
        let mut last_bone_offset: Option<u32> = None;
        let mut last_tex: Option<(u32, u16)> = None;

        for d in &self.prepared_draws {
            if !pred(d.pipe_kind) {
                continue;
            }
            let Some(Some(cached)) = self.models.get(d.model_id as usize) else {
                continue;
            };
            let Some(tex) = cached.textures.get(d.tex_idx as usize) else {
                continue;
            };
            let Some(sub_gpu) = cached
                .bodyparts
                .get(d.bodypart_idx as usize)
                .and_then(|bp| bp.submodels.get(d.submodel_idx as usize))
            else {
                continue;
            };
            let Some(ref vbuf) = sub_gpu.vertex_buf else {
                continue;
            };

            if last_pipe != Some(d.pipe_kind) {
                let pipe = match d.pipe_kind {
                    MdlPipelineKind::Opaque => &self.opaque_pipeline,
                    MdlPipelineKind::Alpha => &self.alpha_pipeline,
                    MdlPipelineKind::Additive => &self.additive_pipeline,
                    MdlPipelineKind::XRay => &self.xray_pipeline,
                };
                pass.set_pipeline(pipe);
                last_pipe = Some(d.pipe_kind);
            }
            let vbuf_key = (d.model_id, d.bodypart_idx, d.submodel_idx);
            if last_vbuf != Some(vbuf_key) {
                pass.set_vertex_buffer(0, vbuf.slice(..));
                last_vbuf = Some(vbuf_key);
            }
            let u_offset = d.uniform_slot * (self.uniform_stride as u32);
            pass.set_bind_group(0, &self.uniform_bg, &[u_offset]);
            let tex_key = (d.model_id, d.tex_idx);
            if last_tex != Some(tex_key) {
                pass.set_bind_group(1, &tex.bg, &[]);
                last_tex = Some(tex_key);
            }
            if last_bone_offset != Some(d.bone_byte_offset) {
                pass.set_bind_group(2, &self.bone_bg, &[d.bone_byte_offset]);
                last_bone_offset = Some(d.bone_byte_offset);
            }
            pass.draw(d.first_vertex..d.first_vertex + d.vertex_count, 0..1);
            draw_calls += 1;
        }
        draw_calls
    }
}

fn stage_bone_palette(
    dst: &mut Vec<u8>,
    bone_align: usize,
    cached_m: &CachedGpuModel,
    base_xf: Mat4,
    bones: &[Mat4],
) -> u32 {
    let raw_bytes = cached_m.gpu_bone_slots * BONE_MATRIX_BYTES;
    let aligned_bytes = raw_bytes.div_ceil(bone_align) * bone_align;
    let offset = dst.len();
    dst.resize(offset + aligned_bytes, 0);
    for b in 0..cached_m.model_bone_count {
        let bm = bones.get(b).copied().unwrap_or(Mat4::IDENTITY);
        write_bone_3x4(&mut dst[offset + b * BONE_MATRIX_BYTES..], base_xf * bm);
    }
    if let Some(fb) = cached_m.fallback_bone_slot {
        write_bone_3x4(
            &mut dst[offset + (fb as usize) * BONE_MATRIX_BYTES..],
            base_xf,
        );
    }
    offset as u32
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

/// CPU reference port of `R_StudioLighting` (`ref/gl/gl_studio.c:1324-1370`,
/// `SHADE_LAMBERT = 1.4953241`, read 2026-11-24). Kept for GPU vertex-shader parity verification.
#[cfg(test)]
fn r_studio_lighting_cpu_ref(
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
    let clamped = illum.clamp(0.0, 255.0);
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
    use cs_assets::mdl::{MdlBodyPart, MdlMesh, MdlSubModel, MdlTexture, MdlVertexRef};

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

    fn wgsl_lookup_light_gamma(lut: &LightGammaUniform, b_10bit: u32) -> f32 {
        let idx = b_10bit.min(1023);
        let v = lut.entries[(idx >> 3) as usize];
        let packed = v[((idx >> 1) & 3) as usize];
        let val = if (idx & 1) != 0 {
            packed >> 16
        } else {
            packed & 0xFFFF
        };
        (val as f32) / 1023.0
    }

    #[allow(clippy::too_many_arguments)]
    fn wgsl_skin_and_shade(
        bones_buf: &[u8],
        lut: &LightGammaUniform,
        pos: Vec3,
        normal: Vec3,
        uv: [f32; 2],
        bone_idx: u32,
        tex_flags: u32,
        ambientlight: f32,
        shadelight: f32,
        plightvec: Vec3,
        light_color: Vec3,
        cam_right: Vec3,
        cam_up: Vec3,
    ) -> (Vec3, Vec3, [f32; 2], Vec3) {
        let base = (bone_idx.min(127) as usize) * BONE_MATRIX_BYTES;
        let rows: [[f32; 4]; 3] = *bytemuck::from_bytes(&bones_buf[base..base + BONE_MATRIX_BYTES]);
        let r0 = glam::Vec4::from_array(rows[0]);
        let r1 = glam::Vec4::from_array(rows[1]);
        let r2 = glam::Vec4::from_array(rows[2]);
        let p4 = pos.extend(1.0);
        let wp = Vec3::new(r0.dot(p4), r1.dot(p4), r2.dot(p4));
        let n_raw = Vec3::new(
            r0.truncate().dot(normal),
            r1.truncate().dot(normal),
            r2.truncate().dot(normal),
        );
        let n = if n_raw.length_squared() > 1e-12 {
            n_raw / n_raw.length()
        } else {
            Vec3::ZERO
        };

        const TEX_FLAG_FLATSHADE: u32 = 1;
        const TEX_FLAG_CHROME: u32 = 2;
        const TEX_FLAG_FULLBRIGHT: u32 = 64;
        const SHADE_LAMBERT: f32 = 1.4953241;

        let lv = if (tex_flags & TEX_FLAG_FULLBRIGHT) != 0 {
            1.0
        } else {
            let illum = if (tex_flags & TEX_FLAG_FLATSHADE) != 0 {
                ambientlight + shadelight * 0.8
            } else {
                let mut lum = ambientlight + shadelight;
                let lightcos = n.dot(plightvec).min(1.0);
                let adj = (lightcos + (SHADE_LAMBERT - 1.0)) / SHADE_LAMBERT;
                if adj > 0.0 {
                    lum -= shadelight * adj;
                }
                lum.max(0.0)
            };
            let clamped = illum.clamp(0.0, 255.0);
            wgsl_lookup_light_gamma(lut, (clamped * 4.0) as u32)
        };
        let out_uv = if (tex_flags & TEX_FLAG_CHROME) != 0 {
            [n.dot(cam_right) * 0.5 + 0.5, -n.dot(cam_up) * 0.5 + 0.5]
        } else {
            uv
        };
        (wp, n, out_uv, light_color * lv)
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

    #[test]
    fn uniform_buffers_fit_webgl2_limit_and_light_gamma_lut_is_bit_exact() {
        let webgl2_limit = wgpu::Limits::downlevel_webgl2_defaults().max_uniform_buffer_binding_size as usize;
        assert_eq!(std::mem::size_of::<MdlBonesUniform>(), 6144);
        assert_eq!(std::mem::size_of::<LightGammaUniform>(), 2048);
        assert!(std::mem::size_of::<MdlUniforms>() <= webgl2_limit);
        assert!(std::mem::size_of::<MdlBonesUniform>() <= webgl2_limit);
        assert!(std::mem::size_of::<LightGammaUniform>() <= webgl2_limit);

        for cfg in [
            GoldSrcGammaConfig::default(),
            GoldSrcGammaConfig {
                gamma: 2.5,
                brightness: 1.5,
                texgamma: 2.0,
                lightgamma: 2.2,
            },
            GoldSrcGammaConfig {
                gamma: 1.8,
                brightness: 0.0,
                texgamma: 2.8,
                lightgamma: 3.0,
            },
        ] {
            let tables = GammaTables::build(cfg);
            let lut = pack_light_gamma_uniform(&tables);
            for i in 0..1024u32 {
                assert_eq!(
                    wgsl_lookup_light_gamma(&lut, i),
                    tables.light_to_tex_gamma_unit(i),
                    "mismatch at entry {i} for {cfg:?}"
                );
            }
        }
    }

    #[test]
    fn gpu_skinning_matches_cpu_reference_on_synthetic_chrome_and_righthand() {
        let mdl = StudioModel {
            name: "synth.mdl".into(),
            bones: vec![
                MdlBone {
                    name: "root".into(),
                    parent: -1,
                    bonecontroller: [-1; 6],
                    default_pos: Vec3::new(1.0, -2.0, 3.0),
                    default_rot: Vec3::new(0.2, -0.3, 0.5),
                    pos_scale: Vec3::ONE,
                    rot_scale: Vec3::ONE,
                },
                MdlBone {
                    name: "Bip01 Spine".into(),
                    parent: 0,
                    bonecontroller: [-1; 6],
                    default_pos: Vec3::new(0.0, 4.0, 8.0),
                    default_rot: Vec3::new(-0.15, 0.45, -0.25),
                    pos_scale: Vec3::ONE,
                    rot_scale: Vec3::ONE,
                },
            ],
            controllers: Vec::new(),
            attachments: Vec::new(),
            hitboxes: Vec::new(),
            sequences: Vec::new(),
            seq_groups: Vec::new(),
            textures: vec![
                MdlTexture {
                    name: "chrome".into(),
                    width: 4,
                    height: 4,
                    flags: STUDIO_NF_CHROME,
                    rgba: vec![200; 64],
                },
                MdlTexture {
                    name: "flat".into(),
                    width: 4,
                    height: 4,
                    flags: STUDIO_NF_FLATSHADE,
                    rgba: vec![180; 64],
                },
                MdlTexture {
                    name: "masked".into(),
                    width: 4,
                    height: 4,
                    flags: STUDIO_NF_MASKED,
                    rgba: vec![255; 64],
                },
            ],
            skin_families: vec![vec![0, 1, 2]],
            bodyparts: vec![MdlBodyPart {
                name: "body".into(),
                submodels: vec![MdlSubModel {
                    name: "s0".into(),
                    meshes: vec![
                        MdlMesh {
                            texture_index: 0,
                            skinref: 0,
                            vertices: vec![MdlVertexRef {
                                pos: Vec3::new(3.5, -1.25, 7.0),
                                normal: Vec3::new(0.4, -0.5, 0.7681146).normalize(),
                                bone: 1,
                                uv: [0.25, 0.75],
                            }],
                        },
                        MdlMesh {
                            texture_index: 1,
                            skinref: 1,
                            vertices: vec![MdlVertexRef {
                                pos: Vec3::new(-2.0, 5.0, 1.0),
                                normal: Vec3::new(-0.6, 0.2, 0.7745967).normalize(),
                                bone: 0,
                                uv: [0.5, 0.5],
                            }],
                        },
                        MdlMesh {
                            texture_index: 2,
                            skinref: 2,
                            vertices: vec![MdlVertexRef {
                                pos: Vec3::new(0.5, 0.5, -2.0),
                                normal: Vec3::Y,
                                bone: 1,
                                uv: [0.1, 0.9],
                            }],
                        },
                    ],
                }],
            }],
        };

        let tables = GammaTables::build(GoldSrcGammaConfig {
            gamma: 2.5,
            brightness: 0.75,
            texgamma: 2.0,
            lightgamma: 2.5,
        });
        let lut = pack_light_gamma_uniform(&tables);
        let mut lights = GpuLightsUniform::default();
        lights.counts[0] = 1;
        lights.dlight_pos_rad[0] = [40.0, -15.0, 25.0, 200.0];
        lights.dlight_col[0] = [1.0, 0.7, 0.3, 1.0];

        let cam_right = Vec3::new(0.0, -1.0, 0.0);
        let cam_up = Vec3::Z;
        let eval_params = BoneEvalParams::default();
        let ref_bones = mdl.evaluate_bones_full(&eval_params);
        let mask = compute_upper_body_mask(&mdl);
        let mut scratch = BoneEvalScratch::new();
        let n = eval_bones_into(&mdl, &mask, &eval_params, &mut scratch);
        assert_eq!(n, ref_bones.len());
        for i in 0..n {
            assert!((scratch.bones[i] - ref_bones[i]).abs_diff_eq(Mat4::ZERO, 1e-6));
        }

        for right_handed in [false, true] {
            let xf = viewmodel_transform_with_angles(
                Vec3::new(10.0, 20.0, 30.0),
                35.0,
                -12.0,
                4.0,
                Vec3::new(1.0, 0.0, -0.5),
                right_handed,
            );
            let (amb, shade, lcol, pvec) =
                compute_entity_lighting(xf.w_axis.truncate(), Vec3::new(0.6, 0.7, 0.8), &lights, 0.9);
            let mut bone_bytes = vec![0u8; mdl.bones.len() * BONE_MATRIX_BYTES];
            for (b, &bm) in ref_bones.iter().enumerate() {
                write_bone_3x4(&mut bone_bytes[b * BONE_MATRIX_BYTES..], xf * bm);
            }
            for mesh in &mdl.bodyparts[0].submodels[0].meshes {
                let flags = mdl.textures[mesh.skinref].flags;
                for v in &mesh.vertices {
                    let m = xf * ref_bones[usize::from(v.bone)];
                    let cpu_pos = m.transform_point3(v.pos);
                    let cpu_norm = m.transform_vector3(v.normal).normalize_or_zero();
                    let cpu_lv = r_studio_lighting_cpu_ref(&tables, flags, cpu_norm, amb, shade, pvec);
                    let cpu_col = lcol * cpu_lv;
                    let cpu_uv = if (flags & STUDIO_NF_CHROME) != 0 {
                        [
                            cpu_norm.dot(cam_right) * 0.5 + 0.5,
                            -cpu_norm.dot(cam_up) * 0.5 + 0.5,
                        ]
                    } else {
                        v.uv
                    };
                    let masked_flags = (flags
                        & (STUDIO_NF_FLATSHADE | STUDIO_NF_CHROME | STUDIO_NF_ADDITIVE | STUDIO_NF_MASKED))
                        as u32;
                    let (gpu_pos, gpu_norm, gpu_uv, gpu_col) = wgsl_skin_and_shade(
                        &bone_bytes,
                        &lut,
                        v.pos,
                        v.normal,
                        v.uv,
                        u32::from(v.bone),
                        masked_flags,
                        amb,
                        shade,
                        pvec,
                        lcol,
                        cam_right,
                        cam_up,
                    );
                    assert!((gpu_pos - cpu_pos).length() < 1e-4, "pos mismatch");
                    assert!((gpu_norm - cpu_norm).length() < 1e-4, "norm mismatch");
                    assert!((gpu_uv[0] - cpu_uv[0]).abs() < 1e-5 && (gpu_uv[1] - cpu_uv[1]).abs() < 1e-5);
                    assert!(
                        (gpu_col - cpu_col).abs().max_element() < (1.0 / 255.0),
                        "color diff {:?} vs {:?}",
                        gpu_col,
                        cpu_col
                    );
                }
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn gpu_skinning_parity_urban_and_v_ak47_with_chrome_and_righthand_if_installed() {
        let Ok(gamedir) = std::env::var("CS16_GAMEDIR") else {
            return;
        };
        let Ok(vfs) = Vfs::open(std::path::Path::new(&gamedir), cs_vfs::VfsConfig::default()) else {
            return;
        };
        let Ok(urban) = StudioModel::load(&vfs, "models/player/urban/urban.mdl") else {
            return;
        };
        let Ok(ak47) = StudioModel::load(&vfs, "models/v_ak47.mdl") else {
            return;
        };

        let tables = GammaTables::build(GoldSrcGammaConfig {
            gamma: 2.5,
            brightness: 0.0,
            texgamma: 2.0,
            lightgamma: 2.5,
        });
        let lut = pack_light_gamma_uniform(&tables);
        let mut lights = GpuLightsUniform::default();
        lights.counts[0] = 1;
        lights.dlight_pos_rad[0] = [64.0, 32.0, 48.0, 220.0];
        lights.dlight_col[0] = [1.0, 0.85, 0.6, 1.0];
        let cam_right = Vec3::new(0.0, -1.0, 0.0);
        let cam_up = Vec3::Z;

        // 1. Verify every vertex of urban.mdl (with gait + blending) and v_ak47.mdl (cl_righthand 0 & 1 + chrome!).
        let urban_params = BoneEvalParams {
            sequence: 4.min(urban.sequences.len().saturating_sub(1)),
            frame: 5.5,
            loop_anim: false,
            blending: [160, 0],
            controller: [127, 127, 127, 127],
            mouth: 0,
            prev_sequence: Some((0, 1.0, true, 0.35)),
            gait: Some((1.min(urban.sequences.len().saturating_sub(1)), 3.25)),
        };
        let ak_params = BoneEvalParams {
            sequence: 0,
            frame: 2.5,
            loop_anim: false,
            ..BoneEvalParams::default()
        };

        let verify_model = |label: &str,
                            mdl: &StudioModel,
                            eval_params: &BoneEvalParams,
                            xf: Mat4| -> (usize, usize) {
            let ref_bones = mdl.evaluate_bones_full(eval_params);
            let mask = compute_upper_body_mask(mdl);
            let mut scratch = BoneEvalScratch::new();
            let n = eval_bones_into(mdl, &mask, eval_params, &mut scratch);
            assert_eq!(n, ref_bones.len(), "{label}: bone count");
            for i in 0..n {
                assert!(
                    (scratch.bones[i] - ref_bones[i]).abs_diff_eq(Mat4::ZERO, 1e-5),
                    "{label}: zero-alloc bone {i} drifted"
                );
            }

            let mut bone_bytes = vec![0u8; ref_bones.len().max(1) * BONE_MATRIX_BYTES];
            for (b, &bm) in ref_bones.iter().enumerate() {
                write_bone_3x4(&mut bone_bytes[b * BONE_MATRIX_BYTES..], xf * bm);
            }
            let (amb, shade, lcol, pvec) =
                compute_entity_lighting(xf.w_axis.truncate(), Vec3::splat(0.75), &lights, 0.9);

            let mut verts_checked = 0usize;
            let mut chrome_verts = 0usize;
            for bp in &mdl.bodyparts {
                let Some(sub) = bp.submodels.first() else {
                    continue;
                };
                for mesh in &sub.meshes {
                    let tex_idx = mdl
                        .resolve_skin_texture(0, mesh.skinref)
                        .min(mdl.textures.len().saturating_sub(1));
                    let flags = mdl.textures.get(tex_idx).map(|t| t.flags).unwrap_or(0);
                    let masked_flags = (flags
                        & (STUDIO_NF_FLATSHADE | STUDIO_NF_CHROME | STUDIO_NF_ADDITIVE | STUDIO_NF_MASKED))
                        as u32;
                    for v in &mesh.vertices {
                        let bm = ref_bones.get(usize::from(v.bone)).copied().unwrap_or(Mat4::IDENTITY);
                        let m = xf * bm;
                        let cpu_pos = m.transform_point3(v.pos);
                        let cpu_norm = m.transform_vector3(v.normal).normalize_or_zero();
                        let cpu_lv = r_studio_lighting_cpu_ref(&tables, flags, cpu_norm, amb, shade, pvec);
                        let cpu_col = lcol * cpu_lv;
                        let cpu_uv = if (flags & STUDIO_NF_CHROME) != 0 {
                            chrome_verts += 1;
                            [
                                cpu_norm.dot(cam_right) * 0.5 + 0.5,
                                -cpu_norm.dot(cam_up) * 0.5 + 0.5,
                            ]
                        } else {
                            v.uv
                        };
                        let (gpu_pos, gpu_norm, gpu_uv, gpu_col) = wgsl_skin_and_shade(
                            &bone_bytes,
                            &lut,
                            v.pos,
                            v.normal,
                            v.uv,
                            u32::from(v.bone),
                            masked_flags,
                            amb,
                            shade,
                            pvec,
                            lcol,
                            cam_right,
                            cam_up,
                        );
                        assert!(
                            (gpu_pos - cpu_pos).length() < 1e-3,
                            "{label}: pos diff {:?}",
                            (gpu_pos - cpu_pos).length()
                        );
                        assert!(
                            (gpu_norm - cpu_norm).length() < 1e-4,
                            "{label}: norm diff {:?}",
                            (gpu_norm - cpu_norm).length()
                        );
                        assert!(
                            (gpu_uv[0] - cpu_uv[0]).abs() < 1e-4
                                && (gpu_uv[1] - cpu_uv[1]).abs() < 1e-4,
                            "{label}: uv diff {:?} vs {:?}",
                            gpu_uv,
                            cpu_uv
                        );
                        assert!(
                            (gpu_col - cpu_col).abs().max_element() < (1.0 / 255.0),
                            "{label}: color diff {:?} vs {:?}",
                            gpu_col,
                            cpu_col
                        );
                        verts_checked += 1;
                    }
                }
            }
            (verts_checked, chrome_verts)
        };

        let (urban_verts, _) = verify_model(
            "urban.mdl",
            &urban,
            &urban_params,
            Mat4::from_rotation_translation(Quat::from_rotation_z(0.6), Vec3::new(90.0, 0.0, -32.0)),
        );
        assert!(urban_verts > 500, "urban.mdl should have >500 verts, got {urban_verts}");

        for right_handed in [false, true] {
            let vm_xf = viewmodel_transform_with_angles(
                Vec3::ZERO,
                0.0,
                0.0,
                0.0,
                Vec3::ZERO,
                right_handed,
            );
            let (ak_verts, ak_chrome) = verify_model(
                if right_handed { "v_ak47.mdl (righthand 1)" } else { "v_ak47.mdl (righthand 0)" },
                &ak47,
                &ak_params,
                vm_xf,
            );
            assert!(ak_verts > 500, "v_ak47.mdl should have >500 verts, got {ak_verts}");
            // v_ak47.mdl has chrome metallic parts if textures carry STUDIO_NF_CHROME; check non-negative.
            let has_chrome_tex = ak47.textures.iter().any(|t| (t.flags & STUDIO_NF_CHROME) != 0);
            if has_chrome_tex {
                assert!(ak_chrome > 0, "v_ak47.mdl chrome vertices must be exercised");
            }
        }

        // Also check bonemerge parity if p_ak47.mdl is present.
        if let Ok(p_ak) = StudioModel::load(&vfs, "models/p_ak47.mdl") {
            let ref_parent = urban.evaluate_bones_full(&urban_params);
            let ref_merged = p_ak.evaluate_bones_merged(&urban, &ref_parent);
            let mut merged_out = [Mat4::IDENTITY; MAX_GPU_BONES];
            let mn = eval_bones_merged_into(&p_ak, &urban, &ref_parent, &mut merged_out);
            assert_eq!(mn, ref_merged.len());
            for i in 0..mn {
                assert!((merged_out[i] - ref_merged[i]).abs_diff_eq(Mat4::ZERO, 1e-6));
            }
        }

        // 2. End-to-end headless GPU render check for urban.mdl (normal + X-Ray) and v_ak47.mdl (righthand 1)
        // under WebGL2 downlevel uniform limits!
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Some(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .ok() else {
            return;
        };
        let limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
        let Ok((device, queue)) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("mdl-parity-webgl2-device"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            ..Default::default()
        })) else {
            return;
        };

        let w = 128u32;
        let h = 128u32;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let color_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mdl_parity_color"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mdl_parity_depth"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let color_view = color_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let mut mdl_r = MdlRenderer::new(&device, format);
        let cam = crate::camera::Camera {
            position: Vec3::ZERO,
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            roll_deg: 0.0,
        };
        let vp = cam.proj_matrix(w as f32, h as f32, 90.0, "widescreen") * cam.view_matrix();
        let pass_params = MdlPassParams {
            view_proj: vp,
            camera_pos: cam.position,
            cam_right: cam.right(),
            cam_up: Vec3::Z,
            lights,
            ..MdlPassParams::default()
        };
        let urban_inst = StudioInstance {
            model_path: "models/player/urban/urban.mdl",
            transform: Mat4::from_rotation_translation(
                Quat::from_rotation_z(std::f32::consts::PI),
                Vec3::new(96.0, 0.0, -32.0),
            ),
            sequence: 4,
            gait: Some((1, 2.0)),
            attached_model: vfs.exists("models/p_ak47.mdl").then_some("models/p_ak47.mdl"),
            xray_color: Some([0.25, 0.55, 1.0]),
            ..StudioInstance::default()
        };
        let ak_inst = StudioInstance {
            model_path: "models/v_ak47.mdl",
            transform: viewmodel_transform_with_angles(Vec3::ZERO, 0.0, 0.0, 0.0, Vec3::ZERO, true),
            sequence: 0,
            flip_winding: false,
            ..StudioInstance::default()
        };
        let prep = mdl_r.prepare_studio(&device, &queue, &vfs, &[urban_inst, ak_inst], &pass_params);
        assert!(prep.total_draws() >= 2, "expected studio draws, got {}", prep.total_draws());
        assert!(prep.xray_draw_count() > 0, "expected spec_xray draws for urban.mdl");

        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("mdl_parity_enc"),
        });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mdl_parity_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            let opaque_calls = mdl_r.draw_opaque_counted(&mut pass, &prep);
            let xray_calls = mdl_r.draw_xray_counted(&mut pass, &prep);
            assert!(opaque_calls >= 2);
            assert_eq!(xray_calls, prep.xray_draw_count());
        }

        let bpr = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mdl_parity_readback"),
            size: u64::from(bpr) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &color_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit(Some(enc.finish()));

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().unwrap().unwrap();
        let mapped = slice.get_mapped_range().unwrap();
        let mut lit_pixels = 0usize;
        for y in 0..h {
            let row = &mapped[(y * bpr) as usize..(y * bpr + w * 4) as usize];
            for px in row.as_chunks::<4>().0 {
                if px[0] > 5 || px[1] > 5 || px[2] > 5 {
                    lit_pixels += 1;
                }
            }
        }
        drop(mapped);
        staging.unmap();
        assert!(lit_pixels > 200, "GPU-skinned urban.mdl + v_ak47.mdl rendered too few pixels: {lit_pixels}");
    }
}
