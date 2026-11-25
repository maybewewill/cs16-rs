//! Interpolation of the entities of the newest frame and the list of what the renderer draws.
//!
//! Ported from Xash3D FWGS (GPL-3.0-or-later; read 2026-11-23): `engine/client/cl_frame.c`
//! (`CL_FindInterpolationUpdates`, `CL_PureOrigin`, `CL_InterpolateModel`,
//! `CL_ComputePlayerOrigin`, `CL_ParametricMove`, `CL_LinkPlayers`, `CL_LinkPacketEntities`,
//! `CL_LinkCustomEntity`, `CL_AddVisibleEntity`) and `engine/client/cl_main.c`
//! (`CL_ComputeClientInterpolationAmount`: the bounds of `ex_interp`); the quaternion helpers
//! come from `public/xash3d_mathlib.c` (`QuaternionSlerp`) and `public/xash3d_mathlib.h`
//! (`AngleQuaternion`, `QuaternionAngle`), `public/matrixlib.c` (`Matrix3x4_FromOriginQuat`,
//! `Matrix3x4_AnglesFromMatrix`), `VectorAngles`.
//!
//! The render time of an entity is `cl.time - ex_interp`. Players other than the local one are
//! interpolated between the two position-history entries that bracket it and extrapolated up
//! to 1.2 intervals past the newest (`CL_PureOrigin`); other entities are interpolated and
//! never extrapolated (`CL_InterpolateModel`). The local player is not interpolated: its
//! origin is the predicted one the caller passes in [`EmitInputs::simorg`].
//!
//! Host features. Xash3D asks the client library for `ENGINE_COMPUTE_STUDIO_LERP` and
//! `ENGINE_STEP_POSHISTORY_LERP`; a GoldSrc client library gets both (the source comments the
//! second one "in GoldSrc this is done for cstrike and czero"), which is this crate's default
//! (assumption; the host-feature table was not fetched). With the first one the studio
//! renderer moves a model between its latched and current state itself (`R_StudioLerpMovement`,
//! in the renderer: [`VisibleEntity::studio_lerp`] marks the entities it applies to).
//! `CL_AddEntityEffects` / `CL_AddModelEffects` (dynamic lights, particle effects of the effect
//! bits) are not here: they belong to the effects code.

use cs_proto48::delta::GoldSrcEntityState;
use cs_proto48::messages::ENTITY_NORMAL;
use glam::Vec3;

use crate::cvars::ClientCvars;
use crate::entities::{
    ClientEntity, EFLAG_SLERP, EntityStore, HISTORY_MASK, ModelKind, PositionHistory, ef,
    entity_custom_lerp, movetype,
};
use crate::frames::{ClientClock, Frame};

/// `EQUAL_EPSILON`.
const EQUAL_EPSILON: f64 = 0.001;
/// `MIN_UPDATERATE`, `MAX_UPDATERATE`, `DEFAULT_UPDATERATE`, `MAX_EX_INTERP*` (`hw.dll:0x101a3c79`).
const MIN_UPDATERATE: f32 = 10.0;
const MAX_UPDATERATE: f32 = 102.0;
const DEFAULT_UPDATERATE: f32 = 20.0;
const MAX_EX_INTERP: f32 = 0.1;
const MAX_EX_INTERP_SPECTATOR: f32 = 0.2;

/// The switches the interpolation reads.
#[derive(Debug, Clone, PartialEq)]
pub struct InterpSettings {
    /// `ex_interp` after `CL_ComputeClientInterpolationAmount` bounded it: seconds in the
    /// past entities are drawn at.
    pub interp: f32,
    /// `cl_nointerp`.
    pub nointerp: bool,
    /// `cl_bmodelinterp` (default 1).
    pub bmodel_interp: bool,
    /// `cl_fixmodelinterpolationartifacts` (default 1).
    pub fix_model_interp_artifacts: bool,
    /// `ENGINE_COMPUTE_STUDIO_LERP`.
    pub compute_studio_lerp: bool,
    /// `ENGINE_STEP_POSHISTORY_LERP`.
    pub step_poshistory_lerp: bool,
    /// The server is on this machine (`NET_IsLocalAddress`): `MOVETYPE_STEP` entities are
    /// then not lerped from the position history.
    pub remote_is_local: bool,
    /// Spectator or HLTV playback: raises upper `ex_interp` limit to 200 ms (`hw.dll:0x101a3c79`).
    pub spectator_or_hltv: bool,
}

impl Default for InterpSettings {
    fn default() -> Self {
        Self {
            interp: 0.1,
            nointerp: false,
            bmodel_interp: true,
            fix_model_interp_artifacts: true,
            compute_studio_lerp: true,
            step_poshistory_lerp: true,
            remote_is_local: false,
            spectator_or_hltv: false,
        }
    }
}

impl InterpSettings {
    /// Reads `ex_interp`, `cl_nointerp` and `cl_updaterate` and bounds the interpolation time
    /// matching `hw.dll:FUN_101a3a40` (`0x101a3c79`, `DAT_102b6df8`):
    /// resets `cl_updaterate < 10.0` to `20.0`, quantizes `min_ms = floor(1000.0 / updaterate).max(1.0)`,
    /// and clamps `ex_interp` to `[min_ms, max_ms]` (`200 ms` spectator/HLTV, `100 ms` otherwise)
    /// with `1 ms` tolerance.
    pub fn apply_cvars(&mut self, cvars: &ClientCvars) {
        let mut updaterate = cvars.float("cl_updaterate");
        if !updaterate.is_finite() || updaterate < MIN_UPDATERATE {
            // `Cvar_Reset`: back to retail `"20"`.
            updaterate = cvars
                .find("cl_updaterate")
                .map_or(DEFAULT_UPDATERATE, |c| crate::cvars::parse_float(&c.default));
            if !updaterate.is_finite() || updaterate < MIN_UPDATERATE {
                updaterate = DEFAULT_UPDATERATE;
            }
        }
        updaterate = updaterate.min(MAX_UPDATERATE);
        let min_ms = (1000.0 / updaterate).floor().max(1.0);
        let max_ms = if self.spectator_or_hltv {
            MAX_EX_INTERP_SPECTATOR * 1000.0
        } else {
            MAX_EX_INTERP * 1000.0
        };
        let interp = cvars.float("ex_interp");
        let interp_ms = interp * 1000.0;
        self.interp = if !interp.is_finite() {
            max_ms / 1000.0
        } else if interp_ms < min_ms - 1.0 {
            min_ms / 1000.0
        } else if interp_ms > max_ms + 1.0 {
            max_ms / 1000.0
        } else {
            interp
        };
        self.nointerp = cvars.float("cl_nointerp") > 0.0;
    }
}

/// The renderer's entity kinds (`ET_*`) the frame produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    /// `ET_NORMAL`.
    Normal,
    /// `ET_PLAYER`.
    Player,
    /// `ET_BEAM`.
    Beam,
}

/// What the renderer needs of one entity of the frame.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibleEntity {
    pub number: u16,
    pub kind: EntityKind,
    pub model: ModelKind,
    pub modelindex: u16,
    /// Where to draw it, after interpolation.
    pub origin: Vec3,
    pub angles: Vec3,
    pub frame: f32,
    pub sequence: u16,
    pub rendermode: u8,
    pub renderamt: u8,
    pub renderfx: u8,
    pub rendercolor: [u8; 3],
    /// The entity is the local player: a first-person view does not draw it.
    pub local_player: bool,
    /// The studio renderer lerps this model's movement itself (`R_StudioLerpMovement`) from
    /// `latched` and `state`.
    pub studio_lerp: bool,
    pub latched: crate::entities::Latched,
    /// The current state with the fix-ups the link pass applies (render colour, follow).
    pub state: GoldSrcEntityState,
}

/// What the caller knows that the frame does not.
#[derive(Debug, Clone, Default)]
pub struct EmitInputs {
    /// `cl.simorg`: where the prediction puts the local player. `None`: the origin of the
    /// server's state (assumption: Xash3D's value without prediction was not read).
    pub simorg: Option<Vec3>,
    /// `cl.local.moving && cl.local.onground`: the entity the local player stands on, which is
    /// not interpolated (the player rides it, the prediction moves it).
    pub standing_on: Option<u16>,
}

/// What [`emit_entities`] produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmitResult {
    /// Players first, then the other entities, each by number (`CL_LinkPlayers`,
    /// `CL_LinkPacketEntities`).
    pub entities: Vec<VisibleEntity>,
    /// The local player has `EF_MUZZLEFLASH`: the view model flashes too.
    pub view_muzzleflash: bool,
}

// ---- quaternions ------------------------------------------------------------------------

type Quat = [f32; 4];

/// `AngleQuaternion( angles, q, false )`.
fn angle_quaternion(angles: Vec3) -> Quat {
    let rad = std::f32::consts::PI / 180.0;
    let (sy, cy) = (angles.y * rad * 0.5).sin_cos();
    let (sp, cp) = (angles.x * rad * 0.5).sin_cos();
    let (sr, cr) = (angles.z * rad * 0.5).sin_cos();
    [
        sr * cp * cy - cr * sp * sy,
        cr * sp * cy + sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
        cr * cp * cy + sr * sp * sy,
    ]
}

/// `QuaternionAlign` then `QuaternionSlerpNoAlign`: 0 gives `p`, 1 gives `q`.
fn quaternion_slerp(p: Quat, q: Quat, t: f32) -> Quat {
    // Make sure the quaternions are within 180 degrees of one another.
    let (mut a, mut b) = (0.0f32, 0.0f32);
    for i in 0..4 {
        a += (p[i] - q[i]) * (p[i] - q[i]);
        b += (p[i] + q[i]) * (p[i] + q[i]);
    }
    let q2: Quat = if a > b { q.map(|v| -v) } else { q };

    let cosom = p[0] * q2[0] + p[1] * q2[1] + p[2] * q2[2] + p[3] * q2[3];
    let mut qt = [0.0f32; 4];
    if (1.0 + cosom) > 0.000001 {
        let (sclp, sclq);
        if (1.0 - cosom) > 0.000001 {
            let omega = cosom.acos();
            let sinom = omega.sin();
            sclp = ((1.0 - t) * omega).sin() / sinom;
            sclq = (t * omega).sin() / sinom;
        } else {
            sclp = 1.0 - t;
            sclq = t;
        }
        for i in 0..4 {
            qt[i] = sclp * p[i] + sclq * q2[i];
        }
    } else {
        qt[0] = -q2[1];
        qt[1] = q2[0];
        qt[2] = -q2[3];
        qt[3] = q2[2];
        let half_pi = 0.5 * std::f32::consts::PI;
        let sclp = ((1.0 - t) * half_pi).sin();
        let sclq = (t * half_pi).sin();
        for i in 0..3 {
            qt[i] = sclp * p[i] + sclq * qt[i];
        }
    }
    qt
}

/// `QuaternionAngle`: `Matrix3x4_FromOriginQuat` then `Matrix3x4_AnglesFromMatrix`.
fn quaternion_angle(q: Quat) -> Vec3 {
    let m00 = 1.0 - 2.0 * q[1] * q[1] - 2.0 * q[2] * q[2];
    let m10 = 2.0 * q[0] * q[1] + 2.0 * q[3] * q[2];
    let m20 = 2.0 * q[0] * q[2] - 2.0 * q[3] * q[1];
    let m01 = 2.0 * q[0] * q[1] - 2.0 * q[3] * q[2];
    let m11 = 1.0 - 2.0 * q[0] * q[0] - 2.0 * q[2] * q[2];
    let m21 = 2.0 * q[1] * q[2] + 2.0 * q[3] * q[0];
    let m22 = 1.0 - 2.0 * q[0] * q[0] - 2.0 * q[1] * q[1];

    let deg = 180.0 / std::f32::consts::PI;
    let xy_dist = (m00 * m00 + m10 * m10).sqrt();
    if xy_dist > 0.001 {
        Vec3::new(
            (-m20).atan2(xy_dist) * deg,
            m10.atan2(m00) * deg,
            m21.atan2(m22) * deg,
        )
    } else {
        // Forward is mostly Z: gimbal lock.
        Vec3::new((-m20).atan2(xy_dist) * deg, (-m01).atan2(m11) * deg, 0.0)
    }
}

/// `QuaternionSlerp( AngleQuaternion(from), AngleQuaternion(to), frac )` as angles.
fn slerp_angles(from: Vec3, to: Vec3, frac: f64) -> Vec3 {
    quaternion_angle(quaternion_slerp(angle_quaternion(from), angle_quaternion(to), frac as f32))
}

/// `VectorMA( base, frac, delta, out )` with a `double` fraction: computed in double, stored
/// as float.
fn vector_ma(base: Vec3, frac: f64, delta: Vec3) -> Vec3 {
    Vec3::new(
        (f64::from(base.x) + frac * f64::from(delta.x)) as f32,
        (f64::from(base.y) + frac * f64::from(delta.y)) as f32,
        (f64::from(base.z) + frac * f64::from(delta.z)) as f32,
    )
}

/// `Q_equal( a, b )`.
fn q_equal(a: f64, b: f64) -> bool {
    a >= b - EQUAL_EPSILON && a <= b + EQUAL_EPSILON
}

/// `VectorAngles`.
fn vector_angles(forward: Vec3) -> Vec3 {
    let (yaw, pitch);
    if forward.y == 0.0 && forward.x == 0.0 {
        yaw = 0.0;
        pitch = if forward.z > 0.0 { 90.0 } else { 270.0 };
    } else {
        let mut y = forward.y.atan2(forward.x) * 180.0 / std::f32::consts::PI;
        if y < 0.0 {
            y += 360.0;
        }
        let tmp = (forward.x * forward.x + forward.y * forward.y).sqrt();
        let mut p = forward.z.atan2(tmp) * 180.0 / std::f32::consts::PI;
        if p < 0.0 {
            p += 360.0;
        }
        yaw = y;
        pitch = p;
    }
    Vec3::new(pitch, yaw, 0.0)
}

// ---- interpolation ----------------------------------------------------------------------

/// `CL_FindInterpolationUpdates`: the two history entries around `targettime`, newer first.
/// The flag is `true` when `targettime` is newer than everything remembered (extrapolation).
///
/// Quirk kept: the search starts one entry behind the newest, so a target that is newer than
/// the newest entry still finds the pair (newest, one before) and the fraction runs past 1.
fn find_interpolation_updates(
    ent: &ClientEntity,
    targettime: f64,
) -> (PositionHistory, PositionHistory, bool) {
    let imod = ent.current_position;
    let mut i0 = imod & HISTORY_MASK;
    let mut i1 = imod.wrapping_sub(1) & HISTORY_MASK;
    let mut extrapolate = true;

    for i in 1..(crate::entities::HISTORY_MAX as u32 - 1) {
        let at = f64::from(ent.ph[(imod.wrapping_sub(i) & HISTORY_MASK) as usize].animtime);
        if at == 0.0 {
            break;
        }
        if targettime > at {
            i0 = imod.wrapping_sub(i).wrapping_add(1) & HISTORY_MASK;
            i1 = imod.wrapping_sub(i) & HISTORY_MASK;
            extrapolate = false;
            break;
        }
    }
    (ent.ph[i0 as usize], ent.ph[i1 as usize], extrapolate)
}

/// `CL_PureOrigin`: where a player is at time `t`: interpolated, and extrapolated up to 1.2
/// intervals.
fn pure_origin(ent: &ClientEntity, t: f64) -> (Vec3, Vec3) {
    // ph0 is the next, ph1 the previous.
    let (ph0, ph1, _) = find_interpolation_updates(ent, t);
    let t0 = f64::from(ph0.animtime);
    let t1 = f64::from(ph1.animtime);

    if t0 != 0.0 {
        let delta = ph0.origin - ph1.origin;
        let mut frac = if !q_equal(t0, t1) { (t - t1) / (t0 - t1) } else { 1.0 };
        // `bound( 0.0, frac, 1.2 )`
        frac = if frac >= 0.0 {
            if frac < 1.2 { frac } else { 1.2 }
        } else {
            0.0
        };
        (vector_ma(ph1.origin, frac, delta), slerp_angles(ph1.angles, ph0.angles, frac))
    } else {
        // No backup found.
        (ph1.origin, ph1.angles)
    }
}

/// `CL_ComputePlayerOrigin`: interpolates a player that is not the local one.
// Task 5 hook: `pub(crate)` so the predictor can pose the other players.
pub(crate) fn compute_player_origin(ent: &mut ClientEntity, time: f64, settings: &InterpSettings) {
    if !ent.player {
        return;
    }
    if settings.nointerp {
        ent.angles = ent.curstate.angles;
        ent.origin = ent.curstate.origin;
        return;
    }
    let targettime = time - f64::from(settings.interp);
    let (origin, angles) = pure_origin(ent, targettime);
    ent.angles = angles;
    ent.origin = origin;
}

/// `CL_InterpolateModel`: interpolates any other entity; `false` when it is not drawn this
/// frame (no history that reaches back to the render time).
fn interpolate_model(
    ent: &mut ClientEntity,
    time: f64,
    max_clients: u8,
    settings: &InterpSettings,
    standing_on: Option<u16>,
) -> bool {
    ent.origin = ent.curstate.origin;
    ent.angles = ent.curstate.angles;

    // `!e->model`; the timedemo case does not exist here.
    if ent.model.is_none() {
        return true;
    }
    if max_clients <= 1 {
        return true;
    }
    if ent.model == Some(ModelKind::Brush) && !settings.bmodel_interp {
        return true;
    }
    if standing_on == Some(ent.index) {
        return true;
    }

    let t = time - f64::from(settings.interp);
    let (ph0, ph1, _) = find_interpolation_updates(ent, t);
    let t1 = f64::from(ph1.animtime);
    let t2 = f64::from(ph0.animtime);

    if t - t1 < 0.0 {
        return false;
    }
    if t1 == 0.0 {
        ent.origin = ph0.origin;
        ent.angles = ph0.angles;
        return false;
    }
    if q_equal(t2, t1) {
        ent.origin = ph0.origin;
        ent.angles = ph0.angles;
        return true;
    }

    let delta = ph0.origin - ph1.origin;
    let mut frac = (t - t1) / (t2 - t1);
    if frac < 0.0 {
        return false;
    }
    if frac > 1.0 {
        frac = 1.0;
    }
    ent.origin = vector_ma(ph1.origin, frac, delta);
    ent.angles = slerp_angles(ph1.angles, ph0.angles, frac);
    true
}

/// `CL_ParametricMove`: an entity that flies from `startpos` to `endpos` between two times.
fn parametric_move(ent: &mut ClientEntity, time: f64) {
    let delta = ent.curstate.endpos - ent.curstate.startpos;
    let dt = ent.curstate.impacttime - ent.curstate.starttime;
    if dt != 0.0 {
        let t = if f64::from(ent.lastmove) > time { f64::from(ent.lastmove) } else { time };
        let frac = ((t - f64::from(ent.curstate.starttime)) / f64::from(dt)) as f32;
        let frac = frac.clamp(0.0, 1.0);
        ent.curstate.origin = ent.curstate.startpos + delta * frac;
        ent.lastmove = t as f32;
    }
    // Re-aim the projectile.
    let len = delta.length();
    if len > 0.0 {
        ent.curstate.angles = vector_angles(delta / len);
    }
}

// ---- linking ----------------------------------------------------------------------------

fn visible(ent: &ClientEntity, kind: EntityKind, model: ModelKind, local_player: bool, studio_lerp: bool) -> VisibleEntity {
    VisibleEntity {
        number: ent.index,
        kind,
        model,
        modelindex: ent.curstate.modelindex,
        origin: ent.origin,
        angles: ent.angles,
        frame: ent.curstate.frame,
        sequence: ent.curstate.sequence,
        rendermode: ent.curstate.rendermode,
        renderamt: ent.curstate.renderamt,
        renderfx: ent.curstate.renderfx,
        rendercolor: ent.curstate.rendercolor,
        local_player,
        studio_lerp,
        latched: ent.latched.clone(),
        state: ent.curstate.clone(),
    }
}

/// `CL_LinkPlayers`.
fn link_players(
    store: &mut EntityStore,
    frame: &Frame,
    clock: &ClientClock,
    settings: &InterpSettings,
    inputs: &EmitInputs,
    out: &mut EmitResult,
) {
    let local_number = store.local_entity_number();
    let max_velocity = store.max_velocity;

    // Apply the muzzle flash to the weapon model.
    if let Some(local) = store.ents.get(&local_number)
        && local.curstate.effects & ef::MUZZLEFLASH != 0
    {
        out.view_muzzleflash = true;
    }

    for number in 1..=u16::from(store.max_clients) {
        // Not present this frame.
        let Some(fe) = frame.entity(number) else {
            continue;
        };
        let state = &fe.state;
        if state.modelindex == 0 || state.effects & ef::NODRAW != 0 {
            continue;
        }
        let Some(ent) = store.ents.get_mut(&number) else {
            continue;
        };
        ent.index = number;
        let is_local = number == local_number;

        if is_local {
            ent.origin = state.origin;
            ent.prevstate.origin = state.origin;
            ent.curstate.origin = state.origin;
            ent.angles = ent.curstate.angles;
        }

        if ent.curstate.effects & ef::NOINTERP != 0 {
            ent.reset_latched_vars(false, clock.mtime[0]);
        }
        if ent.teleported(max_velocity) {
            ent.latched.prevorigin = ent.curstate.origin;
            ent.latched.prevangles = ent.curstate.angles;
            ent.reset_positions();
        }

        if is_local {
            // Interpolation is used for the local player's angles only.
            compute_player_origin(ent, clock.time, settings);
            ent.origin = inputs.simorg.unwrap_or(state.origin);
        } else {
            ent.origin = ent.curstate.origin;
            ent.angles = ent.curstate.angles;
            compute_player_origin(ent, clock.time, settings);
        }
        ent.attachment = [ent.origin; 4];

        // `CL_AddVisibleEntity`: no model, nothing to draw.
        if let Some(model) = ent.model {
            out.entities
                .push(visible(ent, EntityKind::Player, model, is_local, false));
        }
    }
}

/// `CL_LinkPacketEntities` (and `CL_LinkCustomEntity` for the beams among them).
fn link_packet_entities(
    store: &mut EntityStore,
    frame: &Frame,
    clock: &ClientClock,
    settings: &InterpSettings,
    inputs: &EmitInputs,
    out: &mut EmitResult,
) {
    let max_clients = store.max_clients;
    let max_velocity = store.max_velocity;

    for fe in &frame.entities {
        let state = &fe.state;
        // Clients are done in `link_players`.
        if state.number >= 1 && state.number <= u16::from(max_clients) {
            continue;
        }
        // Invisible.
        if state.modelindex == 0 || state.effects & ef::NODRAW != 0 {
            continue;
        }
        let Some(ent) = store.ents.get_mut(&state.number) else {
            continue;
        };

        // The animtime must keep an actual.
        ent.curstate.animtime = state.animtime;
        ent.curstate.frame = state.frame;
        let mut interpolate = false;

        let Some(model) = ent.model else {
            continue;
        };

        let parametric = ent.is_parametric();

        if !parametric && ent.curstate.movetype != movetype::COMPOUND {
            if ent.curstate.animtime == ent.prevstate.animtime
                && ent.curstate.origin != ent.prevstate.origin
            {
                ent.lastmove = (clock.time + 0.2) as f32;
            }

            if ent.curstate.eflags & EFLAG_SLERP != 0
                && ent.curstate.animtime != 0.0
                && model == ModelKind::Studio
            {
                // The branch without `STUDIO_INTERPOLATION_FIX`.
                if f64::from(ent.lastmove) >= clock.time {
                    let at = ent.curstate.animtime;
                    ent.reset_latched_vars(true, clock.mtime[0]);
                    if settings.fix_model_interp_artifacts {
                        ent.latched.prevanimtime = at;
                        ent.curstate.animtime = at;
                    }
                    ent.latched.prevorigin = ent.curstate.origin;
                    ent.latched.prevangles = ent.curstate.angles;
                    if !settings.compute_studio_lerp {
                        // Disable step interpolation in the client library.
                        ent.curstate.movetype = movetype::NONE;
                    }
                } else if settings.compute_studio_lerp {
                    interpolate = true;
                } else {
                    // Restore step interpolation in the client library.
                    ent.curstate.movetype = movetype::STEP;
                }
            }
        }

        if model == ModelKind::Brush {
            interpolate_model(ent, clock.time, max_clients, settings, inputs.standing_on);
        } else {
            if parametric {
                parametric_move(ent, clock.time);
                ent.origin = ent.curstate.origin;
                ent.angles = ent.curstate.angles;
            } else if entity_custom_lerp(ent)
                // In GoldSrc this is done for cstrike and czero (the source's comment).
                || (settings.step_poshistory_lerp
                    && ent.curstate.movetype == movetype::STEP
                    && !settings.remote_is_local)
            {
                if !interpolate_model(ent, clock.time, max_clients, settings, inputs.standing_on) {
                    continue;
                }
            } else {
                // No interpolation right now.
                ent.origin = ent.curstate.origin;
                ent.angles = ent.curstate.angles;
            }
        }
        let studio_lerp =
            model == ModelKind::Studio && interpolate && settings.compute_studio_lerp;

        if fe.state.entity_type & ENTITY_NORMAL == 0 {
            // `CL_LinkCustomEntity`. Deviation: the source stores the model index in
            // `movetype`, which is a byte here; the renderer reads `modelindex`.
            // The source warns "bad model on beam" for a model that is not a sprite and draws
            // it anyway; so does this.
            ent.latched.prevsequence = ent.curstate.sequence;
            ent.latched.prevorigin = ent.origin;
            ent.latched.prevangles = ent.angles;
            ent.prevstate = ent.curstate.clone();
            ent.prev_msg_time = ent.cur_msg_time;
            out.entities
                .push(visible(ent, EntityKind::Beam, model, false, false));
            continue;
        }

        if model != ModelKind::Brush {
            // "NOTE: never pass sprites with rendercolor '0 0 0' it's a stupid Valve Hammer
            // Editor bug"
            if ent.curstate.rendercolor == [0, 0, 0] {
                ent.curstate.rendercolor = [255, 255, 255];
            }
        }

        // "XASH SPECIFIC"
        if ent.curstate.rendermode == 0 && ent.curstate.renderfx == 0 {
            ent.curstate.renderamt = 255;
        }

        if ent.curstate.aiment != 0 && ent.curstate.movetype != movetype::COMPOUND {
            ent.curstate.movetype = movetype::FOLLOW;
        }

        if ent.curstate.effects & ef::NOINTERP != 0 {
            ent.reset_latched_vars(false, clock.mtime[0]);
        }
        if ent.teleported(max_velocity) {
            ent.latched.prevorigin = ent.curstate.origin;
            ent.latched.prevangles = ent.curstate.angles;
            ent.reset_positions();
        }
        ent.attachment = [ent.origin; 4];

        out.entities
            .push(visible(ent, EntityKind::Normal, model, false, studio_lerp));
    }
}

/// `CL_EmitEntities` after its guards: players first, then everything else.
pub(crate) fn emit_entities(
    store: &mut EntityStore,
    frame: &Frame,
    clock: &ClientClock,
    settings: &InterpSettings,
    inputs: &EmitInputs,
) -> EmitResult {
    let mut out = EmitResult::default();
    link_players(store, frame, clock, settings, inputs, &mut out);
    link_packet_entities(store, frame, clock, settings, inputs, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).abs().max_element() < 0.05
    }

    #[test]
    fn angles_survive_the_quaternion_round_trip() {
        for a in [
            Vec3::new(30.0, 45.0, 10.0),
            Vec3::new(-60.0, 170.0, -20.0),
            Vec3::new(10.0, -120.0, 80.0),
        ] {
            let back = quaternion_angle(angle_quaternion(a));
            assert!(close(back, a), "{a:?} -> {back:?}");
        }
    }

    #[test]
    fn pitch_and_roll_are_slerped() {
        let from = Vec3::new(0.0, 0.0, 0.0);
        let to = Vec3::new(40.0, 0.0, 20.0);
        let mid = slerp_angles(from, to, 0.5);
        assert!(close(mid, slerp_angles(to, from, 0.5)));
        assert!(mid.x > 15.0 && mid.x < 25.0 && mid.z > 5.0 && mid.z < 15.0, "{mid:?}");
        assert!(close(slerp_angles(from, to, 1.0), to));
    }

    #[test]
    fn straight_up_takes_the_gimbal_lock_branch() {
        let a = quaternion_angle(angle_quaternion(Vec3::new(-90.0, 30.0, 0.0)));
        assert!((a.x + 90.0).abs() < 0.1, "{a:?}");
        assert_eq!(a.z, 0.0);
    }

    #[test]
    fn apply_cvars_quantizes_min_ms_resets_low_updaterate_and_caps_spectator() {
        let mut cvars = ClientCvars::new();
        let mut s = InterpSettings::default();

        // cl_updaterate < 10.0 resets to 20.0 -> min_ms = floor(1000/20) = 50 ms.
        cvars.set("cl_updaterate", "5");
        cvars.set("ex_interp", "0.01");
        s.apply_cvars(&cvars);
        assert!((s.interp - 0.05).abs() < 1e-6, "{}", s.interp);

        // 1 ms tolerance below min_ms keeps 49.5 ms when min_ms is 50 ms.
        cvars.set("ex_interp", "0.0495");
        s.apply_cvars(&cvars);
        assert!((s.interp - 0.0495).abs() < 1e-6, "{}", s.interp);

        // Player caps at 100 ms; spectator/HLTV caps at 200 ms.
        cvars.set("cl_updaterate", "101");
        cvars.set("ex_interp", "0.15");
        s.apply_cvars(&cvars);
        assert!((s.interp - 0.1).abs() < 1e-6, "{}", s.interp);

        s.spectator_or_hltv = true;
        s.apply_cvars(&cvars);
        assert!((s.interp - 0.15).abs() < 1e-6, "{}", s.interp);

        cvars.set("ex_interp", "0.25");
        s.apply_cvars(&cvars);
        assert!((s.interp - 0.2).abs() < 1e-6, "{}", s.interp);
    }
}
