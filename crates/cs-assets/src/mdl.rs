//! GoldSrc StudioModel v10 (`IDST`) parser, RLE animation decoder, skeletal skinning, and hitboxes.

use cs_vfs::Vfs;
use glam::{Mat4, Quat, Vec3};
use thiserror::Error;

use crate::wad::read_cstr;

pub const MDL_MAGIC: &[u8; 4] = b"IDST";
pub const MDL_VERSION: i32 = 10;

pub const STUDIO_NF_CHROME: i32 = 0x0002;
pub const STUDIO_NF_ADDITIVE: i32 = 0x0020;
pub const STUDIO_NF_MASKED: i32 = 0x0040;

#[derive(Debug, Error)]
pub enum MdlError {
    #[error("invalid StudioModel header or version (expected IDST v10)")]
    InvalidHeader,
    #[error("truncated MDL data: {0}")]
    Truncated(&'static str),
    #[error("VFS error: {0}")]
    Vfs(#[from] cs_vfs::VfsError),
}

#[derive(Debug, Clone)]
pub struct MdlBone {
    pub name: String,
    pub parent: i32,
    pub default_pos: Vec3,
    pub default_rot: Vec3,
    pub pos_scale: Vec3,
    pub rot_scale: Vec3,
}

#[derive(Debug, Clone, Copy)]
pub struct MdlHitbox {
    pub bone: usize,
    /// GoldSrc hitgroup: 0=generic, 1=head, 2=chest, 3=stomach, 4=left_arm, 5=right_arm, 6=left_leg, 7=right_leg.
    pub group: i32,
    pub bbmin: Vec3,
    pub bbmax: Vec3,
}

#[derive(Debug, Clone)]
pub struct MdlBoneAnim {
    /// Per-frame `[dx, dy, dz, rx, ry, rz]` decoded from RLE `mstudioanimvalue_t` (len == sequence.num_frames).
    pub frames: Vec<[f32; 6]>,
}

#[derive(Debug, Clone)]
pub struct MdlSequence {
    pub label: String,
    pub fps: f32,
    pub flags: i32,
    pub num_frames: usize,
    /// Per-bone animation channels (empty if sequence lives in an external seqgroup).
    pub bone_anims: Vec<MdlBoneAnim>,
}

#[derive(Debug, Clone)]
pub struct MdlTexture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub flags: i32,
    pub rgba: Vec<u8>,
}

/// Unskinned mesh triangle vertex referencing bone indices.
#[derive(Debug, Clone, Copy)]
pub struct MdlVertexRef {
    pub pos: Vec3,
    pub normal: Vec3,
    pub bone: u8,
    pub uv: [f32; 2],
}

#[derive(Debug, Clone)]
pub struct MdlMesh {
    pub texture_index: usize,
    pub vertices: Vec<MdlVertexRef>,
}

#[derive(Debug, Clone)]
pub struct MdlSubModel {
    pub name: String,
    pub meshes: Vec<MdlMesh>,
}

#[derive(Debug, Clone)]
pub struct MdlBodyPart {
    pub name: String,
    pub submodels: Vec<MdlSubModel>,
}

/// Complete parsed GoldSrc StudioModel v10.
#[derive(Debug, Clone)]
pub struct StudioModel {
    pub name: String,
    pub bones: Vec<MdlBone>,
    pub hitboxes: Vec<MdlHitbox>,
    pub sequences: Vec<MdlSequence>,
    pub textures: Vec<MdlTexture>,
    pub bodyparts: Vec<MdlBodyPart>,
}

impl StudioModel {
    /// Load a `.mdl` file from VFS (automatically loading companion `*T.mdl` textures if needed).
    pub fn load(vfs: &Vfs, rel_path: &str) -> Result<Self, MdlError> {
        let data = vfs.read_bytes(rel_path)?;
        let mut model = Self::parse(&data)?;

        // Some GoldSrc models split textures into `<name>T.mdl`
        if model.textures.is_empty()
            && let Some(stem) = rel_path.strip_suffix(".mdl")
        {
            let tex_path = format!("{stem}t.mdl");
            if let Ok(tex_bytes) = vfs.read_bytes(&tex_path)
                && let Ok(tex_model) = Self::parse_textures_only(&tex_bytes)
            {
                model.textures = tex_model.0;
                // Remap raw skinref indices in meshes using the companion skin table
                remap_mesh_textures(&mut model.bodyparts, &tex_model.1, &model.textures);
            }
        }

        Ok(model)
    }

    pub fn parse(data: &[u8]) -> Result<Self, MdlError> {
        if data.len() < 244 || &data[0..4] != MDL_MAGIC {
            return Err(MdlError::InvalidHeader);
        }
        let version = read_i32(data, 4)?;
        if version != MDL_VERSION {
            return Err(MdlError::InvalidHeader);
        }

        let name = read_cstr(&data[8..72]);
        let num_bones = read_i32(data, 140)?.max(0) as usize;
        let bone_index = read_i32(data, 144)?.max(0) as usize;
        let num_hitboxes = read_i32(data, 156)?.max(0) as usize;
        let hitbox_index = read_i32(data, 160)?.max(0) as usize;
        let num_seq = read_i32(data, 164)?.max(0) as usize;
        let seq_index = read_i32(data, 168)?.max(0) as usize;
        let num_textures = read_i32(data, 180)?.max(0) as usize;
        let texture_index = read_i32(data, 184)?.max(0) as usize;
        let num_skinref = read_i32(data, 192)?.max(0) as usize;
        let skin_index = read_i32(data, 200)?.max(0) as usize;
        let num_bodyparts = read_i32(data, 204)?.max(0) as usize;
        let bodypart_index = read_i32(data, 208)?.max(0) as usize;

        // 1. Bones
        let mut bones = Vec::with_capacity(num_bones);
        for i in 0..num_bones {
            let base = bone_index + i * 112;
            if base + 112 > data.len() {
                return Err(MdlError::Truncated("bones"));
            }
            bones.push(MdlBone {
                name: read_cstr(&data[base..base + 32]),
                parent: read_i32(data, base + 32)?,
                default_pos: Vec3::new(
                    read_f32(data, base + 64)?,
                    read_f32(data, base + 68)?,
                    read_f32(data, base + 72)?,
                ),
                default_rot: Vec3::new(
                    read_f32(data, base + 76)?,
                    read_f32(data, base + 80)?,
                    read_f32(data, base + 84)?,
                ),
                pos_scale: Vec3::new(
                    read_f32(data, base + 88)?,
                    read_f32(data, base + 92)?,
                    read_f32(data, base + 96)?,
                ),
                rot_scale: Vec3::new(
                    read_f32(data, base + 100)?,
                    read_f32(data, base + 104)?,
                    read_f32(data, base + 108)?,
                ),
            });
        }

        // 2. Hitboxes
        let mut hitboxes = Vec::with_capacity(num_hitboxes);
        for i in 0..num_hitboxes {
            let base = hitbox_index + i * 32;
            if base + 32 > data.len() {
                return Err(MdlError::Truncated("hitboxes"));
            }
            let bone = read_i32(data, base)?.max(0) as usize;
            let group = read_i32(data, base + 4)?;
            let bbmin = Vec3::new(
                read_f32(data, base + 8)?,
                read_f32(data, base + 12)?,
                read_f32(data, base + 16)?,
            );
            let bbmax = Vec3::new(
                read_f32(data, base + 20)?,
                read_f32(data, base + 24)?,
                read_f32(data, base + 28)?,
            );
            hitboxes.push(MdlHitbox {
                bone: bone.min(bones.len().saturating_sub(1)),
                group,
                bbmin,
                bbmax,
            });
        }

        // 3. Textures & skin reference table
        let (textures, skin_table) = parse_textures_and_skins(
            data,
            num_textures,
            texture_index,
            num_skinref,
            skin_index,
        )?;

        // 4. Sequences & RLE animations
        let mut sequences = Vec::with_capacity(num_seq);
        for i in 0..num_seq {
            let base = seq_index + i * 176;
            if base + 176 > data.len() {
                return Err(MdlError::Truncated("sequences"));
            }
            let label = read_cstr(&data[base..base + 32]);
            let fps = read_f32(data, base + 32)?.max(1.0);
            let flags = read_i32(data, base + 36)?;
            let num_frames = read_i32(data, base + 56)?.max(1) as usize;
            let anim_index = read_i32(data, base + 124)?.max(0) as usize;
            let seqgroup = read_i32(data, base + 156)?;

            let mut bone_anims = Vec::new();
            if seqgroup == 0 && anim_index > 0 && anim_index + num_bones * 12 <= data.len() {
                bone_anims.reserve(num_bones);
                for b in 0..num_bones {
                    let anim_base = anim_index + b * 12;
                    let mut offsets = [0usize; 6];
                    for (ch, slot) in offsets.iter_mut().enumerate() {
                        *slot = read_u16(data, anim_base + ch * 2)? as usize;
                    }
                    let mut frames = vec![[0.0f32; 6]; num_frames];
                    for (f, frame_vals) in frames.iter_mut().enumerate() {
                        for (ch, &rel_off) in offsets.iter().enumerate() {
                            if rel_off != 0 {
                                frame_vals[ch] = decode_anim_value(data, anim_base + rel_off, f);
                            }
                        }
                    }
                    bone_anims.push(MdlBoneAnim { frames });
                }
            }

            sequences.push(MdlSequence {
                label,
                fps,
                flags,
                num_frames,
                bone_anims,
            });
        }

        // 5. Bodyparts -> Submodels -> Meshes (triangle strips & fans)
        let mut bodyparts = Vec::with_capacity(num_bodyparts);
        for bp in 0..num_bodyparts {
            let bp_base = bodypart_index + bp * 76;
            if bp_base + 76 > data.len() {
                return Err(MdlError::Truncated("bodyparts"));
            }
            let bp_name = read_cstr(&data[bp_base..bp_base + 64]);
            let num_models = read_i32(data, bp_base + 64)?.max(0) as usize;
            let model_index = read_i32(data, bp_base + 72)?.max(0) as usize;

            let mut submodels = Vec::with_capacity(num_models);
            for m in 0..num_models {
                let m_base = model_index + m * 112;
                if m_base + 112 > data.len() {
                    return Err(MdlError::Truncated("submodel"));
                }
                let m_name = read_cstr(&data[m_base..m_base + 64]);
                let num_mesh = read_i32(data, m_base + 72)?.max(0) as usize;
                let mesh_index = read_i32(data, m_base + 76)?.max(0) as usize;
                let num_verts = read_i32(data, m_base + 80)?.max(0) as usize;
                let vert_info_idx = read_i32(data, m_base + 84)?.max(0) as usize;
                let vert_idx = read_i32(data, m_base + 88)?.max(0) as usize;
                let num_norms = read_i32(data, m_base + 92)?.max(0) as usize;
                let norm_idx = read_i32(data, m_base + 100)?.max(0) as usize;

                let mut verts = Vec::with_capacity(num_verts);
                for v in 0..num_verts {
                    let off = vert_idx + v * 12;
                    if off + 12 > data.len() {
                        return Err(MdlError::Truncated("model vertices"));
                    }
                    verts.push(Vec3::new(
                        read_f32(data, off)?,
                        read_f32(data, off + 4)?,
                        read_f32(data, off + 8)?,
                    ));
                }

                let mut vert_bones = Vec::with_capacity(num_verts);
                for v in 0..num_verts {
                    vert_bones.push(data.get(vert_info_idx + v).copied().unwrap_or(0));
                }

                let mut norms = Vec::with_capacity(num_norms);
                for n in 0..num_norms {
                    let off = norm_idx + n * 12;
                    if off + 12 > data.len() {
                        return Err(MdlError::Truncated("model normals"));
                    }
                    norms.push(Vec3::new(
                        read_f32(data, off)?,
                        read_f32(data, off + 4)?,
                        read_f32(data, off + 8)?,
                    ));
                }

                let mut meshes = Vec::with_capacity(num_mesh);
                for mi in 0..num_mesh {
                    let me_base = mesh_index + mi * 20;
                    if me_base + 20 > data.len() {
                        return Err(MdlError::Truncated("mesh header"));
                    }
                    let tri_index = read_i32(data, me_base + 4)?.max(0) as usize;
                    let skinref = read_i32(data, me_base + 8)?.max(0) as usize;
                    let tex_idx = skin_table.get(skinref).copied().unwrap_or(skinref);
                    let (inv_w, inv_h) = textures
                        .get(tex_idx)
                        .map(|t| (1.0 / t.width.max(1) as f32, 1.0 / t.height.max(1) as f32))
                        .unwrap_or((1.0 / 64.0, 1.0 / 64.0));

                    let tri_verts = decode_mesh_tristrips(
                        data,
                        tri_index,
                        &verts,
                        &vert_bones,
                        &norms,
                        inv_w,
                        inv_h,
                    );
                    meshes.push(MdlMesh {
                        texture_index: tex_idx,
                        vertices: tri_verts,
                    });
                }

                submodels.push(MdlSubModel {
                    name: m_name,
                    meshes,
                });
            }

            bodyparts.push(MdlBodyPart {
                name: bp_name,
                submodels,
            });
        }

        Ok(Self {
            name,
            bones,
            hitboxes,
            sequences,
            textures,
            bodyparts,
        })
    }

    fn parse_textures_only(data: &[u8]) -> Result<(Vec<MdlTexture>, Vec<usize>), MdlError> {
        if data.len() < 244 || &data[0..4] != MDL_MAGIC {
            return Err(MdlError::InvalidHeader);
        }
        let num_textures = read_i32(data, 180)?.max(0) as usize;
        let texture_index = read_i32(data, 184)?.max(0) as usize;
        let num_skinref = read_i32(data, 192)?.max(0) as usize;
        let skin_index = read_i32(data, 200)?.max(0) as usize;
        parse_textures_and_skins(data, num_textures, texture_index, num_skinref, skin_index)
    }

    /// Find sequence index by case-insensitive substring (e.g. `"idle"`, `"shoot"`, `"reload"`, `"draw"`).
    pub fn find_sequence(&self, needle: &str) -> Option<usize> {
        let lower = needle.to_ascii_lowercase();
        self.sequences
            .iter()
            .position(|s| s.label.to_ascii_lowercase() == lower)
            .or_else(|| {
                self.sequences
                    .iter()
                    .position(|s| s.label.to_ascii_lowercase().contains(&lower))
            })
    }

    /// Evaluate hierarchical bone matrices in model space for `(sequence_idx, frame)` with linear interpolation.
    pub fn evaluate_bones(&self, seq_idx: usize, frame_time: f32) -> Vec<Mat4> {
        self.evaluate_bones_ex(seq_idx, frame_time, true)
    }

    /// Evaluate hierarchical bone matrices with optional clamping (`loop_anim = false` for death corpses).
    pub fn evaluate_bones_ex(&self, seq_idx: usize, frame_time: f32, loop_anim: bool) -> Vec<Mat4> {
        let n = self.bones.len();
        let mut world = vec![Mat4::IDENTITY; n];
        let seq = self.sequences.get(seq_idx);

        let (f0, f1, frac) = if let Some(s) = seq {
            let nf = s.num_frames.max(1);
            if nf <= 1 {
                (0, 0, 0.0)
            } else if loop_anim {
                let f_wrapped = frame_time.rem_euclid(nf as f32);
                let i0 = (f_wrapped.floor() as usize).min(nf - 1);
                let i1 = if i0 + 1 < nf { i0 + 1 } else { 0 };
                (i0, i1, f_wrapped - f_wrapped.floor())
            } else {
                let f_clamped = frame_time.clamp(0.0, (nf - 1) as f32);
                let i0 = (f_clamped.floor() as usize).min(nf - 1);
                let i1 = (i0 + 1).min(nf - 1);
                (i0, i1, f_clamped - f_clamped.floor())
            }
        } else {
            (0, 0, 0.0)
        };

        for (i, bone) in self.bones.iter().enumerate() {
            let (pos, quat) = if let Some(anim) = seq.and_then(|s| s.bone_anims.get(i))
                && !anim.frames.is_empty()
            {
                let v0 = anim.frames[f0.min(anim.frames.len() - 1)];
                let v1 = anim.frames[f1.min(anim.frames.len() - 1)];

                let p0 = bone.default_pos + Vec3::new(v0[0], v0[1], v0[2]) * bone.pos_scale;
                let p1 = bone.default_pos + Vec3::new(v1[0], v1[1], v1[2]) * bone.pos_scale;
                let pos = p0.lerp(p1, frac);

                let r0 = bone.default_rot + Vec3::new(v0[3], v0[4], v0[5]) * bone.rot_scale;
                let r1 = bone.default_rot + Vec3::new(v1[3], v1[4], v1[5]) * bone.rot_scale;
                let q0 = goldsrc_angle_quat(r0);
                let q1 = goldsrc_angle_quat(r1);
                let quat = q0.slerp(q1, frac);
                (pos, quat)
            } else {
                (bone.default_pos, goldsrc_angle_quat(bone.default_rot))
            };

            let local = Mat4::from_rotation_translation(quat, pos);
            world[i] = if bone.parent >= 0 && (bone.parent as usize) < i {
                world[bone.parent as usize] * local
            } else {
                local
            };
        }

        world
    }

    /// Authentic GoldSrc `EF_BONEMERGE`: attach a `p_*.mdl` weapon directly to matching bone names
    /// (e.g. `Bip01 R Hand`, `Bip01 L Hand`) on the parent player model's already-evaluated skeleton!
    pub fn evaluate_bones_merged(
        &self,
        parent: &StudioModel,
        parent_bones: &[Mat4],
    ) -> Vec<Mat4> {
        let n = self.bones.len();
        let mut world = vec![Mat4::IDENTITY; n];
        for (i, bone) in self.bones.iter().enumerate() {
            if let Some(p_idx) = parent
                .bones
                .iter()
                .position(|pb| pb.name.eq_ignore_ascii_case(&bone.name))
                && let Some(&p_mat) = parent_bones.get(p_idx)
            {
                world[i] = p_mat;
                continue;
            }
            let local = Mat4::from_rotation_translation(
                goldsrc_angle_quat(bone.default_rot),
                bone.default_pos,
            );
            world[i] = if bone.parent >= 0 && (bone.parent as usize) < i {
                world[bone.parent as usize] * local
            } else {
                local
            };
        }
        world
    }

    /// Compute the skinned model-space `(min, max)` AABB at `(seq_idx, frame_time)`.
    pub fn skinned_bounds(&self, seq_idx: usize, frame_time: f32) -> (Vec3, Vec3) {
        let bones = self.evaluate_bones(seq_idx, frame_time);
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for bp in &self.bodyparts {
            let Some(sub) = bp.submodels.first() else {
                continue;
            };
            for mesh in &sub.meshes {
                for v in &mesh.vertices {
                    let b = bones
                        .get(v.bone as usize)
                        .copied()
                        .unwrap_or(Mat4::IDENTITY);
                    let p = b.transform_point3(v.pos);
                    min = min.min(p);
                    max = max.max(p);
                }
            }
        }
        (min, max)
    }
}

/// Exact Half-Life SDK `AngleQuaternion(rx, ry, rz)` (`Z * Y * X` Euler order).
pub fn goldsrc_angle_quat(rot: Vec3) -> Quat {
    Quat::from_rotation_z(rot.z) * Quat::from_rotation_y(rot.y) * Quat::from_rotation_x(rot.x)
}

fn parse_textures_and_skins(
    data: &[u8],
    num_textures: usize,
    texture_index: usize,
    num_skinref: usize,
    skin_index: usize,
) -> Result<(Vec<MdlTexture>, Vec<usize>), MdlError> {
    let mut textures = Vec::with_capacity(num_textures);
    for i in 0..num_textures {
        let base = texture_index + i * 80;
        if base + 80 > data.len() {
            return Err(MdlError::Truncated("texture headers"));
        }
        let name = read_cstr(&data[base..base + 64]);
        let flags = read_i32(data, base + 64)?;
        let width = read_i32(data, base + 68)?.max(1) as u32;
        let height = read_i32(data, base + 72)?.max(1) as u32;
        let pix_off = read_i32(data, base + 76)?.max(0) as usize;

        let pix_count = (width as usize) * (height as usize);
        let pal_off = pix_off + pix_count;
        if pal_off + 256 * 3 > data.len() {
            return Err(MdlError::Truncated("texture pixels/palette"));
        }
        let indices = &data[pix_off..pal_off];
        let palette = &data[pal_off..pal_off + 256 * 3];
        let masked = (flags & STUDIO_NF_MASKED) != 0;

        let mut rgba = Vec::with_capacity(pix_count * 4);
        for &idx in indices {
            if masked && idx == 255 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let p = (idx as usize) * 3;
                rgba.extend_from_slice(&[palette[p], palette[p + 1], palette[p + 2], 255]);
            }
        }

        textures.push(MdlTexture {
            name,
            width,
            height,
            flags,
            rgba,
        });
    }

    let mut skin_table = Vec::with_capacity(num_skinref);
    for i in 0..num_skinref {
        let off = skin_index + i * 2;
        if off + 2 <= data.len() {
            skin_table.push(read_u16(data, off)? as usize);
        }
    }

    Ok((textures, skin_table))
}

fn remap_mesh_textures(
    bodyparts: &mut [MdlBodyPart],
    skin_table: &[usize],
    textures: &[MdlTexture],
) {
    for bp in bodyparts {
        for sub in &mut bp.submodels {
            for mesh in &mut sub.meshes {
                let tex_idx = skin_table
                    .get(mesh.texture_index)
                    .copied()
                    .unwrap_or(mesh.texture_index);
                mesh.texture_index = tex_idx;
                if let Some(tex) = textures.get(tex_idx) {
                    // Rescale UVs if previously scaled with 64x64 placeholder
                    let scale_u = 64.0 / tex.width.max(1) as f32;
                    let scale_v = 64.0 / tex.height.max(1) as f32;
                    for v in &mut mesh.vertices {
                        v.uv[0] *= scale_u;
                        v.uv[1] *= scale_v;
                    }
                }
            }
        }
    }
}

fn decode_anim_value(data: &[u8], start_offset: usize, target_frame: usize) -> f32 {
    let mut ptr = start_offset;
    let mut k = target_frame;
    let mut guard = 0usize;
    while ptr + 2 <= data.len() && guard < 2048 {
        guard += 1;
        let valid = data[ptr] as usize;
        let total = data[ptr + 1] as usize;
        if total == 0 {
            return 0.0;
        }
        if k < total {
            let idx = if valid == 0 {
                0
            } else if k < valid {
                k
            } else {
                valid - 1
            };
            let val_off = ptr + 2 + idx * 2;
            return read_i16(data, val_off).unwrap_or(0) as f32;
        }
        k -= total;
        ptr += (valid + 1) * 2;
    }
    0.0
}

fn decode_mesh_tristrips(
    data: &[u8],
    mut offset: usize,
    verts: &[Vec3],
    vert_bones: &[u8],
    norms: &[Vec3],
    inv_w: f32,
    inv_h: f32,
) -> Vec<MdlVertexRef> {
    let mut out = Vec::new();

    while offset + 2 <= data.len() {
        let cmd = read_i16(data, offset).unwrap_or(0);
        offset += 2;
        if cmd == 0 {
            break;
        }
        let is_fan = cmd < 0;
        let count = cmd.unsigned_abs() as usize;
        if count < 3 || offset + count * 8 > data.len() {
            break;
        }

        let mut strip = Vec::with_capacity(count);
        for _ in 0..count {
            let v_idx = read_i16(data, offset).unwrap_or(0).max(0) as usize;
            let n_idx = read_i16(data, offset + 2).unwrap_or(0).max(0) as usize;
            let s = read_i16(data, offset + 4).unwrap_or(0) as f32;
            let t = read_i16(data, offset + 6).unwrap_or(0) as f32;
            offset += 8;

            strip.push(MdlVertexRef {
                pos: verts.get(v_idx).copied().unwrap_or(Vec3::ZERO),
                normal: norms.get(n_idx).copied().unwrap_or(Vec3::Z),
                bone: vert_bones.get(v_idx).copied().unwrap_or(0),
                uv: [s * inv_w, t * inv_h],
            });
        }

        if is_fan {
            for i in 1..(count - 1) {
                out.push(strip[0]);
                out.push(strip[i + 1]);
                out.push(strip[i]);
            }
        } else {
            for i in 0..(count - 2) {
                if i % 2 == 0 {
                    out.push(strip[i]);
                    out.push(strip[i + 2]);
                    out.push(strip[i + 1]);
                } else {
                    out.push(strip[i]);
                    out.push(strip[i + 1]);
                    out.push(strip[i + 2]);
                }
            }
        }
    }

    out
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, MdlError> {
    let arr: [u8; 2] = bytes
        .get(offset..offset + 2)
        .ok_or(MdlError::Truncated("u16"))?
        .try_into()
        .map_err(|_| MdlError::Truncated("u16"))?;
    Ok(u16::from_le_bytes(arr))
}

fn read_i16(bytes: &[u8], offset: usize) -> Result<i16, MdlError> {
    let arr: [u8; 2] = bytes
        .get(offset..offset + 2)
        .ok_or(MdlError::Truncated("i16"))?
        .try_into()
        .map_err(|_| MdlError::Truncated("i16"))?;
    Ok(i16::from_le_bytes(arr))
}

fn read_i32(bytes: &[u8], offset: usize) -> Result<i32, MdlError> {
    let arr: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or(MdlError::Truncated("i32"))?
        .try_into()
        .map_err(|_| MdlError::Truncated("i32"))?;
    Ok(i32::from_le_bytes(arr))
}

fn read_f32(bytes: &[u8], offset: usize) -> Result<f32, MdlError> {
    let arr: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or(MdlError::Truncated("f32"))?
        .try_into()
        .map_err(|_| MdlError::Truncated("f32"))?;
    Ok(f32::from_le_bytes(arr))
}
