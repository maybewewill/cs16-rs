//! The intro cameras: while a joining player reads the MOTD and picks a team it looks at the map
//! through a `trigger_camera` and moves to the next camera every [`INTRO_CAM_INTERVAL`] seconds.
//! Ported from ReGameDLL_CS (`client.cpp` `ClientPutInServer`, `player.cpp` `JoiningThink`,
//! `util.cpp` `UTIL_FindEntityByTargetname`/`UTIL_VecToAngles`) and ReHLDS (`mathlib.cpp`
//! `VectorAngles`, `pr_cmds.cpp` `PF_find_Shared`), all read 2026-11-22, the vanilla (not
//! `REGAMEDLL_FIXES`) branches.
//!
//! A camera is a `trigger_camera` entity ([`crate::world::TriggerCamera`]); the view it gives is
//! at its origin, looking at the entity its `target` names. A camera with no such target gives no
//! view.
//!
//! Gap: `CSGameRules()->m_bMapHasCameras` (written at join in vanilla, read only under
//! `REGAMEDLL_FIXES`) is not kept.

use cs_engine::{EdictId, EngineApi};
use glam::Vec3;

use crate::GameState;

/// The classname of an intro camera.
const CAMERA_CLASS: &str = "trigger_camera";

/// `gpGlobals->time + 6` (`ClientPutInServer`, `JoiningThink`): how long a camera holds the view.
pub(crate) const INTRO_CAM_INTERVAL: f64 = 6.0;

/// `VectorAngles` (ReHLDS `mathlib.cpp`, read 2026-11-22), which `UTIL_VecToAngles` calls
/// through `VEC_TO_ANGLES`: the pitch (0 to 360, up is positive, as the engine reports it) and
/// the yaw of a direction. The arc tangents are computed in `double` and stored in `float`.
pub(crate) fn vec_to_angles(forward: Vec3) -> Vec3 {
    let (yaw, pitch);
    if forward.y == 0.0 && forward.x == 0.0 {
        yaw = 0.0;
        pitch = if forward.z > 0.0 { 90.0 } else { 270.0 };
    } else {
        let mut y = f64::from(forward.y).atan2(f64::from(forward.x)) * 180.0 / std::f64::consts::PI;
        if y < 0.0 {
            y += 360.0;
        }
        // `Q_sqrt((double)(forward[0] * forward[0] + forward[1] * forward[1]))`: the sum is a
        // `float`.
        let length = f64::from(forward.x * forward.x + forward.y * forward.y).sqrt();
        let mut p = f64::from(forward.z).atan2(length) * 180.0 / std::f64::consts::PI;
        if p < 0.0 {
            p += 360.0;
        }
        yaw = y as f32;
        pitch = p as f32;
    }
    Vec3::new(pitch, yaw, 0.0)
}

/// `Vector::Normalize` (`vector.h`): the unit vector, or `(0, 0, 1)` for a vector of no length.
fn normalized(v: Vec3) -> Vec3 {
    let length = v.length();
    if length == 0.0 {
        return Vec3::Z;
    }
    let scale = 1.0 / length;
    Vec3::new(v.x * scale, v.y * scale, v.z * scale)
}

/// `UTIL_FindEntityByTargetname(after, name)`: the first live entity after `after` (from the
/// first for `None`) whose `targetname` is `name`. An empty name or an entity with no name never
/// matches (`PF_find_Shared` skips an empty field).
pub(crate) fn find_by_targetname(
    eng: &EngineApi<'_>,
    after: Option<EdictId>,
    name: &str,
) -> Option<EdictId> {
    if name.is_empty() {
        return None;
    }
    let first = after.map_or(0, |id| usize::from(id.index) + 1);
    eng.entity_ids()
        .into_iter()
        .filter(|id| usize::from(id.index) >= first)
        .find(|&id| eng.ent(id).is_some_and(|v| v.targetname == name))
}

/// The view a camera gives: its origin and the angles that look at its target (`UTIL_VecToAngles`
/// of the normalised direction, the pitch negated), or `None` for a camera that is gone or whose
/// `target` names no entity.
pub(crate) fn camera_view(eng: &EngineApi<'_>, camera: EdictId) -> Option<(Vec3, Vec3)> {
    let cam = eng.ent(camera)?;
    let (origin, target) = (cam.origin, cam.target.clone());
    let at = eng.ent(find_by_targetname(eng, None, &target)?)?.origin;
    let mut angles = vec_to_angles(normalized(at - origin));
    angles.x = -angles.x;
    Some((origin, angles))
}

/// `UTIL_FindEntityByClassname(nullptr, "trigger_camera")`: the first camera of the map.
pub(crate) fn first_camera(eng: &EngineApi<'_>) -> Option<EdictId> {
    eng.find_by_classname(None, CAMERA_CLASS)
}

/// Puts the player of `slot` at the view of `camera`: `UTIL_SetOrigin`, `angles` and `v_angle`
/// the view's, no velocity or punch, `fixangle` so the client takes the angles, no view offset.
/// The caller sets the time of the next camera.
fn apply_view(eng: &mut EngineApi<'_>, slot: u8, origin: Vec3, angles: Vec3) {
    let id = eng.client_edict(slot);
    if let Some(v) = eng.ent_mut(id) {
        v.angles = angles;
        v.v_angle = angles;
        v.velocity = Vec3::ZERO;
        v.punchangle = Vec3::ZERO;
        v.fixangle = 1;
        v.view_ofs = Vec3::ZERO;
    }
    eng.set_origin(id, origin);
}

/// The camera part of `CBasePlayer::JoiningThink` (`player.cpp`), which runs for a player that
/// is still joining and was not taken into the game by this very call: once the time of the
/// next camera has come (`m_fIntroCamTime`, 0 until a camera set it: a quirk of vanilla that
/// moves a player whose first camera has no target at its first think), the player moves to the
/// next `trigger_camera` after its own, the first if there is none after it, and looks at its
/// target. A camera with no target ends the tour (`m_pIntroCamera = nullptr`); the player stays
/// where it is.
pub(crate) fn joining_think(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let time = eng.time();
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    let Some(current) = p.intro_camera else {
        return;
    };
    if time < p.intro_cam_time {
        return;
    }
    // "Find the next another camera"; "could not find, go back to the start". A camera that
    // is gone has no successor: the vanilla code would read through a null pointer, which here
    // ends the tour.
    let next = eng
        .find_by_classname(Some(current), CAMERA_CLASS)
        .or_else(|| first_camera(eng));
    let view = next.and_then(|camera| camera_view(eng, camera));
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    p.intro_camera = next;
    match view {
        Some((origin, angles)) => {
            p.intro_cam_time = time + INTRO_CAM_INTERVAL;
            apply_view(eng, slot, origin, angles);
        }
        None => p.intro_camera = None,
    }
}

/// What `ClientPutInServer` does with the cameras: the player's first camera and, if it has a
/// target, the view at it. Returns the camera (set even when its target is missing, as in
/// vanilla) and the view.
pub(crate) fn join_view(eng: &EngineApi<'_>) -> (Option<EdictId>, Option<(Vec3, Vec3)>) {
    let camera = first_camera(eng);
    let view = camera.and_then(|c| camera_view(eng, c));
    (camera, view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec_to_angles_matches_vector_angles() {
        // Straight up and down are the special cases; yaw is 0 to 360, pitch 0 to 360.
        assert_eq!(vec_to_angles(Vec3::Z), Vec3::new(90.0, 0.0, 0.0));
        assert_eq!(vec_to_angles(-Vec3::Z), Vec3::new(270.0, 0.0, 0.0));
        assert_eq!(vec_to_angles(Vec3::X), Vec3::ZERO);
        assert_eq!(vec_to_angles(Vec3::Y), Vec3::new(0.0, 90.0, 0.0));
        assert_eq!(vec_to_angles(-Vec3::X), Vec3::new(0.0, 180.0, 0.0));
        assert_eq!(vec_to_angles(-Vec3::Y), Vec3::new(0.0, 270.0, 0.0));
        // Looking down at 45 degrees: pitch is `360 - 45`, and the intro negates it.
        let down = vec_to_angles(normalized(Vec3::new(1.0, 0.0, -1.0)));
        assert!((down.x - 315.0).abs() < 1e-3 && down.y == 0.0);
    }

    #[test]
    fn a_zero_direction_looks_up() {
        assert_eq!(normalized(Vec3::ZERO), Vec3::Z);
        assert_eq!(vec_to_angles(normalized(Vec3::ZERO)), Vec3::new(90.0, 0.0, 0.0));
    }
}
