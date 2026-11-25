//! Studio model hitboxes for traces: the pose of a model and the hull of each of its hitboxes,
//! as the original server clips a bullet against a player.
//!
//! Ported from ReHLDS `rehlds/engine/r_studio.cpp` (`SV_StudioSetupBones`, `R_StudioHull`,
//! `SV_SetStudioHullPlane`, `R_StudioCalcBoneAdj`, `R_StudioCalcBoneQuaterion`,
//! `R_StudioCalcBonePosition`, `R_StudioSlerpBones`, `R_StudioPlayerBlend`,
//! `SV_StudioPlayerTracePose`, `SV_HullForStudioModel`, `R_CheckStudioCache`,
//! `R_AddToStudioCache`) and `rehlds/engine/mathlib.cpp` (`AngleMatrix`, `R_ConcatTransforms`,
//! `VectorCompare`), read 2026-11-22. The multi-hull clip itself is
//! `SV_SingleClipMoveToEntity` in `trace_ents.rs`.
//!
//! Choices, all of them named where they are made:
//! - the `REHLDS_FIXES` branches are taken, as in `physics.rs` (the `long double` arithmetic of
//!   `R_StudioCalcBonePosition`, shield hitbox 20);
//! - animations of another sequence group (`seqgroup != 0`, a `*01.mdl` file) are not read, the
//!   model is then traced as a box (the CS player models keep theirs in the file);
//! - the cache of the original (16 poses, kept between frames) lives for one game call here:
//!   the pellets of one shot, which share a pose, still cost one bone setup, and the result of
//!   a pose does not depend on the cache.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use cs_assets::bsp::Plane;
use cs_assets::mdl::{RawBone, RawSequence, STUDIO_TRACE_HITBOX, StudioPoseData};
use glam::Vec3;

use crate::collision::BoxHull;
use crate::edict::{EdictId, EntVars};
use crate::game::EngineApi;
use crate::world::consts::FL_CLIENT;

/// A pose: 3x4 matrices, `[row][column]`, as `bonetransform` is.
type Mat34 = [[f32; 4]; 3];

const STUDIO_MOUTH: i32 = 4;
const STUDIO_X: i32 = 0x0001;
const STUDIO_Y: i32 = 0x0002;
const STUDIO_Z: i32 = 0x0004;
const STUDIO_XR: i32 = 0x0008;
const STUDIO_YR: i32 = 0x0010;
const STUDIO_ZR: i32 = 0x0020;
const STUDIO_TYPES: i32 = 0x7FFF;
const STUDIO_RLOOP: i32 = 0x8000;
/// `STUDIO_CACHE_SIZE`.
const STUDIO_CACHE_SIZE: usize = 16;
/// `SV_StudioShieldHitbox` with `REHLDS_FIXES` (21 without; the same hulls result for the
/// models of CS, which have 21 hitboxes at most).
const SHIELD_HITBOX: usize = 20;
/// `plane->type` of a studio plane: not axial.
const PLANE_NOT_AXIAL: i32 = 5;

/// One hitbox as a clipping hull (`studio_hull[i]`) with its hitgroup
/// (`studio_hull_hitgroup[i]`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct StudioHull {
    pub(crate) hull: BoxHull,
    pub(crate) group: i32,
}

struct CacheEntry {
    model: u16,
    frame: f32,
    sequence: i32,
    angles: Vec3,
    origin: Vec3,
    size: Vec3,
    controller: [u8; 4],
    blending: [u8; 2],
    /// Not in the key of the original's cache, which then serves a pose with the shield for
    /// one without (and the other way round) until it is flushed; here it is in the key.
    skip_shield: bool,
    /// `None`: the pose could not be made (also remembered, so a model that cannot be posed
    /// costs one attempt per pose and call).
    hulls: Option<Rc<[StudioHull]>>,
}

/// `rgStudioCache`.
#[derive(Default)]
pub(crate) struct StudioCache {
    entries: Vec<CacheEntry>,
    /// Slot the next entry goes to once the cache is full.
    next: usize,
    /// Bone setups done (the cache misses): the work the traces cost.
    setups: usize,
}

impl StudioCache {
    /// `R_CheckStudioCache`: the keys are compared exactly (`VectorCompare`, `memcmp`).
    #[allow(clippy::too_many_arguments)]
    fn check(
        &self,
        model: u16,
        frame: f32,
        sequence: i32,
        angles: Vec3,
        origin: Vec3,
        size: Vec3,
        controller: [u8; 4],
        blending: [u8; 2],
        skip_shield: bool,
    ) -> Option<Option<Rc<[StudioHull]>>> {
        self.entries
            .iter()
            .find(|c| {
                c.skip_shield == skip_shield
                    && c.model == model
                    && c.frame == frame
                    && c.sequence == sequence
                    && c.angles == angles
                    && c.origin == origin
                    && c.size == size
                    && c.controller == controller
                    && c.blending == blending
            })
            .map(|c| c.hulls.clone())
    }

    /// `R_AddToStudioCache`.
    fn add(&mut self, entry: CacheEntry) {
        if self.entries.len() < STUDIO_CACHE_SIZE {
            self.entries.push(entry);
        } else {
            self.entries[self.next] = entry;
            self.next = (self.next + 1) % STUDIO_CACHE_SIZE;
        }
    }
}

/// `AngleMatrix`. Mind the source's names: its `sy`/`cy` come from the roll, `sp`/`cp` from
/// the yaw and `sr`/`cr` from the pitch.
fn angle_matrix(angles: Vec3, origin: Vec3) -> Mat34 {
    let sc = |degrees: f32| {
        let angle = (f64::from(degrees) * (std::f64::consts::PI * 2.0 / 360.0)) as f32;
        let angle = f64::from(angle);
        (angle.sin() as f32, angle.cos() as f32)
    };
    let (sy, cy) = sc(angles.z);
    let (sp, cp) = sc(angles.y);
    let (sr, cr) = sc(angles.x);
    [
        [
            cr * cp,
            sy * sr * cp - cy * sp,
            cy * sr * cp + sy * sp,
            origin.x,
        ],
        [
            cr * sp,
            sy * sr * sp + cy * cp,
            cy * sr * sp - sy * cp,
            origin.y,
        ],
        [-sr, sy * cr, cy * cr, origin.z],
    ]
}

/// `R_ConcatTransforms`.
fn concat_transforms(a: &Mat34, b: &Mat34) -> Mat34 {
    let mut out = [[0.0f32; 4]; 3];
    for r in 0..3 {
        for c in 0..3 {
            out[r][c] = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
        }
        out[r][3] = a[r][0] * b[0][3] + a[r][1] * b[1][3] + a[r][2] * b[2][3] + a[r][3];
    }
    out
}

/// `AngleQuaternion`; the sines are taken in double and stored as floats, as `sin(float)` of
/// the original does (assumption: its `sin` is the double one).
fn angle_quaternion(angles: [f32; 3]) -> [f32; 4] {
    let sc = |a: f32| {
        let angle = f64::from(a * 0.5);
        (angle.sin() as f32, angle.cos() as f32)
    };
    let (sy, cy) = sc(angles[2]);
    let (sp, cp) = sc(angles[1]);
    let (sr, cr) = sc(angles[0]);
    [
        sr * cp * cy - cr * sp * sy,
        cr * sp * cy + sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
        cr * cp * cy + sr * sp * sy,
    ]
}

/// `QuaternionSlerp`. The source flips `q` in place when it is the far one; `q` is not used
/// after, so the flip stays local here.
fn quaternion_slerp(p: [f32; 4], q: [f32; 4], t: f32) -> [f32; 4] {
    let mut q = q;
    let (mut a, mut b) = (0.0f32, 0.0f32);
    for i in 0..4 {
        a += (p[i] - q[i]) * (p[i] - q[i]);
        b += (p[i] + q[i]) * (p[i] + q[i]);
    }
    if a > b {
        for v in &mut q {
            *v = -*v;
        }
    }
    let cosom = p[0] * q[0] + p[1] * q[1] + p[2] * q[2] + p[3] * q[3];
    let mut qt = [0.0f32; 4];
    if (1.0 + f64::from(cosom)) > 0.000001 {
        let (sclp, sclq);
        if (1.0 - f64::from(cosom)) > 0.000001 {
            let omega = f64::from(cosom).acos() as f32;
            let sinom = f64::from(omega).sin() as f32;
            sclp = (((1.0 - f64::from(t)) * f64::from(omega)).sin() / f64::from(sinom)) as f32;
            sclq = ((f64::from(t) * f64::from(omega)).sin() / f64::from(sinom)) as f32;
        } else {
            sclp = 1.0 - t;
            sclq = t;
        }
        for i in 0..4 {
            qt[i] = sclp * p[i] + sclq * q[i];
        }
    } else {
        // The source's quirk: qt[3] keeps p[2] and is not blended.
        qt[0] = -p[1];
        qt[1] = p[0];
        qt[2] = -p[3];
        qt[3] = p[2];
        let sclp = ((1.0 - f64::from(t)) * 0.5 * std::f64::consts::PI).sin() as f32;
        let sclq = (f64::from(t) * 0.5 * std::f64::consts::PI).sin() as f32;
        for i in 0..3 {
            qt[i] = sclp * p[i] + sclq * qt[i];
        }
    }
    qt
}

/// `QuaternionMatrix` (the rotation part of a bone matrix).
fn quaternion_matrix(q: [f32; 4]) -> Mat34 {
    [
        [
            1.0 - 2.0 * q[1] * q[1] - 2.0 * q[2] * q[2],
            2.0 * q[0] * q[1] - 2.0 * q[3] * q[2],
            2.0 * q[0] * q[2] + 2.0 * q[3] * q[1],
            0.0,
        ],
        [
            2.0 * q[0] * q[1] + 2.0 * q[3] * q[2],
            1.0 - 2.0 * q[0] * q[0] - 2.0 * q[2] * q[2],
            2.0 * q[1] * q[2] - 2.0 * q[3] * q[0],
            0.0,
        ],
        [
            2.0 * q[0] * q[2] - 2.0 * q[3] * q[1],
            2.0 * q[1] * q[2] + 2.0 * q[3] * q[0],
            1.0 - 2.0 * q[0] * q[0] - 2.0 * q[1] * q[1],
            0.0,
        ],
    ]
}

/// `R_StudioCalcBoneAdj` with `dadt` 0, both controller arrays the same and a closed mouth
/// (the one call of `SV_StudioSetupBones`). `adj` holds what the controllers do to the bones.
fn calc_bone_adj(model: &StudioPoseData, controller: [u8; 4]) -> [f32; 8] {
    let dadt = 0.0f32;
    let mut adj = [0.0f32; 8];
    for (j, bc) in model.controllers.iter().enumerate() {
        let i = bc.index;
        let value: f32;
        if i >= STUDIO_MOUTH {
            // The mouth is hardcoded at controller 4; `mouthopen` is 0 here.
            let mut v = (0.0f64 / 64.0) as f32;
            if f64::from(v) > 1.0 {
                v = 1.0;
            }
            value =
                ((1.0 - f64::from(v)) * f64::from(bc.start) + f64::from(v) * f64::from(bc.end))
                    as f32;
        } else {
            let c = |k: usize| f32::from(controller.get(k).copied().unwrap_or(0));
            let i = i as usize;
            if bc.kind & STUDIO_RLOOP != 0 {
                // The source wraps the 360 degrees when its two controller arrays differ by
                // more than 128 (and then indexes them by `j`, not `i`); both are the same
                // array here, so that branch is dead and left out.
                value = ((f64::from(c(i) * dadt) + f64::from(c(i)) * (1.0 - f64::from(dadt)))
                    * (360.0 / 256.0)
                    + f64::from(bc.start)) as f32;
            } else {
                let mut v = ((f64::from(c(i) * dadt) + f64::from(c(i)) * (1.0 - f64::from(dadt)))
                    / 255.0) as f32;
                v = v.clamp(0.0, 1.0);
                value = ((1.0 - f64::from(v)) * f64::from(bc.start)
                    + f64::from(v) * f64::from(bc.end)) as f32;
            }
        }
        match bc.kind & STUDIO_TYPES {
            STUDIO_XR | STUDIO_YR | STUDIO_ZR => {
                adj[j] = (f64::from(value) * (std::f64::consts::PI / 180.0)) as f32;
            }
            STUDIO_X | STUDIO_Y | STUDIO_Z => adj[j] = value,
            _ => {}
        }
    }
    adj
}

/// The animation value `n` entries after `pos`.
fn value_at(model: &StudioPoseData, pos: usize, n: i32) -> Option<f32> {
    let at = pos.checked_add(usize::try_from(n).ok()?.checked_mul(2)?)?;
    model.anim_value(at).map(|(_, _, v)| f32::from(v))
}

/// Finds the run of animation values that holds frame `frame` of the channel at `start`:
/// the position of its header and the frame's index in it (the `while` of the source).
/// `None` when the data ends first, or at a run of no frames (a real run has at least one):
/// every step then takes a frame off `frame` or is refused, so the walk is bounded by the
/// frame number and the sizes of the runs, not by the file.
fn find_run(model: &StudioPoseData, start: usize, frame: i32) -> Option<(usize, i32, i32, i32)> {
    let mut pos = start;
    let mut k = frame;
    loop {
        let (valid, total, _) = model.anim_value(pos)?;
        if total == 0 {
            return None;
        }
        if i32::from(total) <= k {
            k -= i32::from(total);
            pos = pos.checked_add((usize::from(valid) + 1) * 2)?;
        } else {
            return Some((pos, k, i32::from(valid), i32::from(total)));
        }
    }
}

/// `R_StudioCalcBoneQuaterion`. `None` when the animation data runs out.
fn calc_bone_quaternion(
    model: &StudioPoseData,
    frame: i32,
    s: f32,
    bone: &RawBone,
    anim: (usize, [u16; 6]),
    adj: &[f32; 8],
) -> Option<[f32; 4]> {
    let (base, offsets) = anim;
    let mut angle1 = [0.0f32; 3];
    let mut angle2 = [0.0f32; 3];
    for j in 0..3 {
        if offsets[j + 3] == 0 {
            angle1[j] = bone.value[j + 3];
            angle2[j] = bone.value[j + 3];
        } else {
            let (pos, k, valid, total) = find_run(model, base + usize::from(offsets[j + 3]), frame)?;
            let (a1, a2);
            // "Bah, missing blend!": the frames of a run are not blended here, the two angles
            // are the neighbours and `s` blends the quaternions below.
            if valid > k {
                a1 = value_at(model, pos, k + 1)?;
                if valid > k + 1 {
                    a2 = value_at(model, pos, k + 2)?;
                } else if total > k + 1 {
                    a2 = a1;
                } else {
                    a2 = value_at(model, pos, valid + 2)?;
                }
            } else {
                a1 = value_at(model, pos, valid)?;
                if total > k + 1 {
                    a2 = a1;
                } else {
                    a2 = value_at(model, pos, valid + 2)?;
                }
            }
            angle1[j] = bone.value[j + 3] + a1 * bone.scale[j + 3];
            angle2[j] = bone.value[j + 3] + a2 * bone.scale[j + 3];
        }
        let controller = bone.bonecontroller[j + 3];
        if controller != -1 {
            let a = adj[controller as usize];
            angle1[j] += a;
            angle2[j] += a;
        }
    }
    if angle1 != angle2 {
        Some(quaternion_slerp(
            angle_quaternion(angle1),
            angle_quaternion(angle2),
            s,
        ))
    } else {
        Some(angle_quaternion(angle1))
    }
}

/// `R_StudioCalcBonePosition` (the `REHLDS_FIXES` branches: `long double` is taken as `f64`).
fn calc_bone_position(
    model: &StudioPoseData,
    frame: i32,
    s: f32,
    bone: &RawBone,
    anim: (usize, [u16; 6]),
    adj: &[f32; 8],
) -> Option<[f32; 3]> {
    let (base, offsets) = anim;
    let mut pos = [0.0f32; 3];
    for j in 0..3 {
        pos[j] = bone.value[j];
        if offsets[j] != 0 {
            let (at, k, valid, total) = find_run(model, base + usize::from(offsets[j]), frame)?;
            let scale = f64::from(bone.scale[j]);
            let blend = |v1: f32, v2: f32| {
                (f64::from(v1) * f64::from(1.0 - s) + f64::from(s) * f64::from(v2)) * scale
            };
            let delta = if valid > k {
                if valid > k + 1 {
                    blend(value_at(model, at, k + 1)?, value_at(model, at, k + 2)?)
                } else {
                    f64::from(value_at(model, at, k + 1)?) * scale
                }
            } else if total <= k + 1 {
                blend(value_at(model, at, valid)?, value_at(model, at, valid + 2)?)
            } else {
                f64::from(value_at(model, at, valid)?) * scale
            };
            pos[j] = (f64::from(pos[j]) + delta) as f32;
        }
        let controller = bone.bonecontroller[j];
        if controller != -1 {
            pos[j] += adj[controller as usize];
        }
    }
    Some(pos)
}

/// `SV_StudioSetupBones` for the whole model (`iBone` -1): the transform of every bone.
/// `None` for a model without sequences, one whose sequence keeps its animations in another
/// file, or a frame or animation data that is out of range (the original would read memory
/// there). A sequence out of range is sequence 0, as in the source.
pub(crate) fn setup_bones(
    model: &StudioPoseData,
    frame: f32,
    sequence: i32,
    angles: Vec3,
    origin: Vec3,
    controller: [u8; 4],
    blending: [u8; 2],
) -> Option<Vec<Mat34>> {
    let sequence = usize::try_from(sequence)
        .ok()
        .filter(|&s| s < model.sequences.len())
        .unwrap_or(0);
    let seq = model.sequences.get(sequence)?;
    if seq.seqgroup != 0 || !frame.is_finite() {
        return None;
    }
    let num_bones = model.bones.len();

    let f = if seq.numframes > 1 {
        (seq.numframes - 1) as f32 * frame / 256.0
    } else {
        0.0
    };
    let adj = calc_bone_adj(model, controller);
    let whole = f as i32;
    if whole < 0 {
        return None;
    }
    let s = f - whole as f32;

    let mut q1 = vec![[0.0f32; 4]; num_bones];
    let mut pos1 = vec![[0.0f32; 3]; num_bones];
    for (bone, raw) in model.bones.iter().enumerate() {
        let anim = model.anim_entry(seq.animindex, bone)?;
        q1[bone] = calc_bone_quaternion(model, whole, s, raw, anim, &adj)?;
        pos1[bone] = calc_bone_position(model, whole, s, raw, anim, &adj)?;
    }

    if seq.numblends > 1 {
        let second = seq.animindex.checked_add(num_bones.checked_mul(12)?)?;
        let mut q2 = vec![[0.0f32; 4]; num_bones];
        let mut pos2 = vec![[0.0f32; 3]; num_bones];
        for (bone, raw) in model.bones.iter().enumerate() {
            let anim = model.anim_entry(second, bone)?;
            q2[bone] = calc_bone_quaternion(model, whole, s, raw, anim, &adj)?;
            pos2[bone] = calc_bone_position(model, whole, s, raw, anim, &adj)?;
        }
        // `R_StudioSlerpBones` with `*pblending / 255.0f`.
        let w = (f32::from(blending[0]) / 255.0).clamp(0.0, 1.0);
        let w1 = 1.0 - w;
        for i in 0..num_bones {
            q1[i] = quaternion_slerp(q1[i], q2[i], w);
            for c in 0..3 {
                pos1[i][c] = pos1[i][c] * w1 + pos2[i][c] * w;
            }
        }
    }

    let rotation = angle_matrix(angles, origin);
    let mut transforms: Vec<Mat34> = Vec::with_capacity(num_bones);
    for (bone, raw) in model.bones.iter().enumerate() {
        let mut m = quaternion_matrix(q1[bone]);
        m[0][3] = pos1[bone][0];
        m[1][3] = pos1[bone][1];
        m[2][3] = pos1[bone][2];
        let parent = usize::try_from(raw.parent).ok();
        let out = match parent {
            None => concat_transforms(&rotation, &m),
            // Parents come first (checked when the model was read).
            Some(p) => concat_transforms(transforms.get(p)?, &m),
        };
        transforms.push(out);
    }
    Some(transforms)
}

/// `SV_SetStudioHullPlane`.
fn hull_plane(bone: &Mat34, k: usize, dist: f32) -> Plane {
    let normal = Vec3::new(bone[0][k], bone[1][k], bone[2][k]);
    Plane {
        normal,
        dist: normal.z * bone[2][3] + normal.y * bone[1][3] + normal.x * bone[0][3] + dist,
        plane_type: PLANE_NOT_AXIAL,
    }
}

/// `R_StudioHull` without the cache: the hull of each hitbox (the shield's left out when
/// `skip_shield`), grown by `size`. `angles` are the entity's, whose pitch the bones take
/// the other way round. `None` when the pose cannot be made.
#[allow(clippy::too_many_arguments)]
fn build_hulls(
    model: &StudioPoseData,
    frame: f32,
    sequence: i32,
    angles: Vec3,
    origin: Vec3,
    size: Vec3,
    controller: [u8; 4],
    blending: [u8; 2],
    skip_shield: bool,
) -> Option<Vec<StudioHull>> {
    let angles2 = Vec3::new(-angles.x, angles.y, angles.z);
    let bones = setup_bones(model, frame, sequence, angles2, origin, controller, blending)?;
    let mut hulls = Vec::with_capacity(model.hitboxes.len());
    for (i, hitbox) in model.hitboxes.iter().enumerate() {
        if skip_shield && i == SHIELD_HITBOX {
            continue;
        }
        let bone = bones.get(hitbox.bone)?;
        let mut planes = [Plane {
            normal: Vec3::Z,
            dist: 0.0,
            plane_type: PLANE_NOT_AXIAL,
        }; 6];
        for j in 0..3 {
            let mut plane0 = hull_plane(bone, j, hitbox.bbmax[j]);
            let mut plane1 = hull_plane(bone, j, hitbox.bbmin[j]);
            plane0.dist += (plane0.normal.x * size.x).abs()
                + (plane0.normal.y * size.y).abs()
                + (plane0.normal.z * size.z).abs();
            plane1.dist -= (plane1.normal.x * size.x).abs()
                + (plane1.normal.y * size.y).abs()
                + (plane1.normal.z * size.z).abs();
            planes[j * 2] = plane0;
            planes[j * 2 + 1] = plane1;
        }
        hulls.push(StudioHull {
            hull: BoxHull::from_planes(planes),
            group: hitbox.group,
        });
    }
    Some(hulls)
}

/// `R_StudioPlayerBlend`: the blend weight (0..=255 for a pitch inside the sequence's range)
/// a pitch maps to, and the pitch left over for the bones.
fn player_blend(seq: &RawSequence, pitch: &mut f32) -> i32 {
    let start = seq.blendstart[0];
    let end = seq.blendend[0];
    let mut blend = (f64::from(*pitch) * 3.0) as i32;
    if (blend as f32) < start {
        blend = 0;
        *pitch -= (f64::from(start) / 3.0) as f32;
    } else if (blend as f32) <= end {
        let diff = end - start;
        if f64::from(diff) >= 0.1 {
            blend = ((blend as f32 - start) as f64 * 255.0 / f64::from(diff)) as i32;
            *pitch = 0.0;
        } else {
            blend = 127;
            *pitch = 0.0;
        }
    } else {
        blend = 255;
        *pitch = (f64::from(*pitch) - f64::from(end) / 3.0) as f32;
    }
    blend
}

/// `SV_StudioPlayerTracePose`: the angles, controllers and blending a player is traced in: the
/// pitch goes into the blend, the controllers stay centred. A sequence outside the model is
/// taken as 0, where the original reads past the table.
fn player_trace_pose(model: &StudioPoseData, v: &EntVars) -> (Vec3, [u8; 4], [u8; 2]) {
    let seq = model
        .sequences
        .get(usize::from(v.sequence))
        .or_else(|| model.sequences.first());
    let mut angles = v.angles;
    let blend = seq.map_or(0, |seq| player_blend(seq, &mut angles.x));
    (angles, [0x7F; 4], [blend as u8, 0])
}

impl EngineApi<'_> {
    /// `SV_HullForStudioModel`: the hitbox hulls an entity with studio model `model` is clipped
    /// against by a move of the box `mins..maxs`, or `None` when it is clipped as its box (a
    /// box move of a model without the `STUDIO_TRACE_HITBOX` flag, a client when
    /// `sv_clienttrace` is 0, a model that has no hitboxes or cannot be posed). `origin` is
    /// where the entity is taken to be.
    ///
    /// `FTRACE_SIMPLEBOX` is not supported: the engine has no `trace_flags` (vanilla never sets
    /// them).
    pub(crate) fn studio_hulls(
        &self,
        v: &EntVars,
        model: &StudioPoseData,
        origin: Vec3,
        mins: Vec3,
        maxs: Vec3,
    ) -> Option<Rc<[StudioHull]>> {
        let is_client = v.flags & FL_CLIENT != 0;
        let mut size = maxs - mins;
        let mut use_complex = false;
        let mut factor = 0.5f32;
        if size == Vec3::ZERO {
            use_complex = true;
            if is_client {
                // The cvar's default is 1 (`sv_clienttrace` of `sv_main.cpp`); a value that is
                // not a finite number counts as 0, as `cvar_f32` does.
                let clienttrace = self
                    .cvars
                    .get("sv_clienttrace")
                    .map_or(1.0, |c| c.as_f32());
                let clienttrace = if clienttrace.is_finite() {
                    clienttrace
                } else {
                    0.0
                };
                if clienttrace == 0.0 {
                    use_complex = false;
                } else {
                    size = Vec3::ONE;
                    factor = clienttrace * 0.5;
                }
            }
        }
        if model.flags & STUDIO_TRACE_HITBOX == 0 && !use_complex {
            return None;
        }
        if model.hitboxes.is_empty() {
            // Assumption: the original clips a model without hitboxes as a trace that never
            // leaves solid (an empty hull list); this clips it as a box instead.
            return None;
        }
        size *= factor;
        let skip_shield = v.gamestate == 1;
        let (angles, controller, blending) = if is_client {
            player_trace_pose(model, v)
        } else {
            (v.angles, v.controller, v.blending)
        };
        let sequence = i32::from(v.sequence);

        let cache_model = v.modelindex;
        if let Some(hit) = self.studio_cache.borrow().check(
            cache_model,
            v.frame,
            sequence,
            angles,
            origin,
            size,
            controller,
            blending,
            skip_shield,
        ) {
            return hit;
        }
        let hulls: Option<Rc<[StudioHull]>> = build_hulls(
            model, v.frame, sequence, angles, origin, size, controller, blending, skip_shield,
        )
        .map(Into::into);
        let mut cache = self.studio_cache.borrow_mut();
        cache.setups += 1;
        cache.add(CacheEntry {
            model: cache_model,
            frame: v.frame,
            sequence,
            angles,
            origin,
            size,
            controller,
            blending,
            skip_shield,
            hulls: hulls.clone(),
        });
        hulls
    }

    /// `GET_MODEL_PTR`: the studio model with model index `index` (the `modelindex` of an
    /// entity), `None` for model 0, a brush model, a file the loader did not find or one it could
    /// not parse. The game reads the sequences of a player model from it (`LookupSequence`,
    /// `LookupActivity`, `GetSequenceInfo`).
    pub fn studio_model(&self, index: u16) -> Option<Arc<StudioPoseData>> {
        self.precache.studio_model(index).cloned()
    }

    /// World-space oriented bounding boxes `(hitgroup, [8 corners])` for every hitbox of `id`'s
    /// posed studio model (`sv_showhitboxes`). Returns `None` if `id` is invalid or has no posed
    /// studio model with hitboxes.
    pub fn studio_hitbox_boxes(&self, id: EdictId) -> Option<Vec<(i32, [Vec3; 8])>> {
        let v = self.ent(id)?;
        let model = self.precache.studio_model(v.modelindex)?;
        if model.hitboxes.is_empty() {
            return None;
        }
        let is_client = v.flags & FL_CLIENT != 0;
        let skip_shield = v.gamestate == 1;
        let (angles, controller, blending) = if is_client {
            player_trace_pose(model, v)
        } else {
            (v.angles, v.controller, v.blending)
        };
        let angles2 = Vec3::new(-angles.x, angles.y, angles.z);
        let bones = setup_bones(
            model,
            v.frame,
            i32::from(v.sequence),
            angles2,
            v.origin,
            controller,
            blending,
        )?;
        let xform = |b: &Mat34, p: [f32; 3]| -> Vec3 {
            Vec3::new(
                b[0][0] * p[0] + b[0][1] * p[1] + b[0][2] * p[2] + b[0][3],
                b[1][0] * p[0] + b[1][1] * p[1] + b[1][2] * p[2] + b[1][3],
                b[2][0] * p[0] + b[2][1] * p[1] + b[2][2] * p[2] + b[2][3],
            )
        };
        let mut out = Vec::with_capacity(model.hitboxes.len());
        for (i, hb) in model.hitboxes.iter().enumerate() {
            if skip_shield && i == SHIELD_HITBOX {
                continue;
            }
            let Some(bone) = bones.get(hb.bone) else {
                continue;
            };
            let lo = hb.bbmin;
            let hi = hb.bbmax;
            let corners = [
                xform(bone, [lo[0], lo[1], lo[2]]),
                xform(bone, [hi[0], lo[1], lo[2]]),
                xform(bone, [hi[0], hi[1], lo[2]]),
                xform(bone, [lo[0], hi[1], lo[2]]),
                xform(bone, [lo[0], lo[1], hi[2]]),
                xform(bone, [hi[0], lo[1], hi[2]]),
                xform(bone, [hi[0], hi[1], hi[2]]),
                xform(bone, [lo[0], hi[1], hi[2]]),
            ];
            out.push((hb.group, corners));
        }
        Some(out)
    }

    /// Bone setups the traces of this call made so far (cache misses). For tests that count
    /// work instead of time. Test-only (`test-hooks` feature).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn studio_setups(&self) -> usize {
        self.studio_cache.borrow().setups
    }
}

/// What [`EngineApi`] holds for the cache.
pub(crate) type SharedStudioCache = RefCell<StudioCache>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slerp_of_equal_quaternions_is_that_quaternion() {
        let q = angle_quaternion([0.1, 0.2, 0.3]);
        let r = quaternion_slerp(q, q, 0.5);
        for i in 0..4 {
            assert!((r[i] - q[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn player_blend_follows_the_pitch_range() {
        let seq = RawSequence {
            label: String::new(),
            fps: 30.0,
            flags: 0,
            activity: 0,
            actweight: 0,
            linearmovement: [0.0; 3],
            numframes: 1,
            numblends: 2,
            blendstart: [-45.0, 0.0],
            blendend: [45.0, 0.0],
            seqgroup: 0,
            animindex: 0,
        };
        let mut pitch = 0.0;
        assert_eq!(player_blend(&seq, &mut pitch), 127);
        assert_eq!(pitch, 0.0);
        let mut pitch = -30.0;
        assert_eq!(player_blend(&seq, &mut pitch), 0);
        assert_eq!(pitch, -30.0 + 15.0);
        let mut pitch = 30.0;
        assert_eq!(player_blend(&seq, &mut pitch), 255);
        assert_eq!(pitch, 30.0 - 15.0);
    }

    #[test]
    fn angle_matrix_of_yaw_90_turns_x_into_y() {
        let m = angle_matrix(Vec3::new(0.0, 90.0, 0.0), Vec3::new(1.0, 2.0, 3.0));
        assert!((m[0][0]).abs() < 1e-6 && (m[1][0] - 1.0).abs() < 1e-6);
        assert_eq!((m[0][3], m[1][3], m[2][3]), (1.0, 2.0, 3.0));
    }
}
