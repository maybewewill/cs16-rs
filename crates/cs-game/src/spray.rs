//! The spray paint of `impulse 201`: `CBasePlayer::ImpulseCommands` case 201 and the entity it
//! makes, `CSprayCan` (`spraycan`). Ported from ReGameDLL_CS `player.cpp` (`ImpulseCommands`,
//! `CSprayCan::Spawn`, `CSprayCan::Think`) and `util.cpp` (`UTIL_DecalTrace`), read 2026-11-22,
//! the vanilla behaviour (the can starts at the player's origin plus 32 units up; the
//! `REGAMEDLL_FIXES` build starts at the eyes).
//!
//! A player may spray once every `decalfrequency` seconds, at a surface within 128 units of its
//! eyes. The can plays `player/sprayer.wav`, and a tenth of a second later paints the decal.
//!
//! Assumption: the player has no custom logo (`GetCustomDecalFrames() == -1`, which
//! `ClientPutInServer` sets: the customization files of the clients are not transferred), so
//! the can paints the stock `DECAL_LAMBDA6` and is gone; the animated custom logo branch
//! (`UTIL_PlayerDecalTrace`) is not ported.

use cs_engine::{EdictId, EngineApi, MoveFilter};
use cs_sim::pm::math::angle_vectors;
use glam::Vec3;

use crate::GameState;
use crate::bullets::{decal_number, decal_trace};
use crate::entity::{GameEntity, attach};
use crate::player::{ATTN_NORM, CHAN_VOICE, PITCH_NORM};

/// How far from the eyes (or the can) the spray reaches.
const SPRAY_RANGE: f32 = 128.0;

/// `CSprayCan`: waits a tenth of a second for its `Think`, which paints and removes it.
#[derive(Debug, Clone, Default)]
pub struct SprayCan;

/// Impulse 201 of `ImpulseCommands`, "paint decal": too early (before `m_flNextDecalTime`) does
/// nothing; the line from the eyes 128 units along `v_angle` (`ignore_monsters`) must hit
/// something, else nothing happens either; then the next spray is `decalfrequency` seconds off
/// and a can is made.
pub(crate) fn paint_decal(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    let now = eng.time() as f32;
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    if now < p.next_decal_time {
        // "too early!"
        return;
    }
    let Some(v) = eng.ent(id) else {
        return;
    };
    let (origin, view_ofs, v_angle) = (v.origin, v.view_ofs, v.v_angle);
    let (forward, _, _) = angle_vectors(v_angle);
    let eyes = origin + view_ofs;
    let tr = eng.trace_line(eyes, eyes + forward * SPRAY_RANGE, MoveFilter::NoMonsters, Some(id));
    if tr.trace.fraction == 1.0 {
        return;
    }
    // "line hit something, so paint a decal": `gpGlobals->time + CVAR_GET_FLOAT("decalfrequency")`.
    let frequency = eng.cvar_f32("decalfrequency");
    if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        p.next_decal_time = now + frequency;
    }
    spawn_can(eng, id, origin, v_angle);
}

/// `GetClassPtr<CCSSprayCan>` and `CSprayCan::Spawn(pev)`: the can at the owner's origin plus 32
/// units up, looking where the owner does, a tenth of a second from its think, with the
/// spray's sound.
fn spawn_can(eng: &mut EngineApi<'_>, owner: EdictId, owner_origin: Vec3, v_angle: Vec3) {
    let Some(can) = eng.create_entity("spraycan") else {
        return;
    };
    let now = eng.time() as f32;
    if let Some(v) = eng.ent_mut(can) {
        v.origin = owner_origin + Vec3::new(0.0, 0.0, 32.0);
        v.angles = v_angle;
        v.owner = Some(owner);
        v.frame = 0.0;
        v.nextthink = f64::from(now + 0.1);
    }
    attach(eng, can, Box::new(SprayCan));
    eng.emit_sound(can, CHAN_VOICE, "player/sprayer.wav", 1.0, ATTN_NORM, PITCH_NORM);
}

impl GameEntity for SprayCan {
    fn classname(&self) -> &'static str {
        "spraycan"
    }

    /// `CSprayCan::Think` with no custom logo: a line of 128 units from the can along its
    /// angles (`ignore_monsters`, the owner passed), `UTIL_DecalTrace(&tr, DECAL_LAMBDA6)`, and
    /// the can is removed.
    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some(v) = eng.ent(id) else {
            return;
        };
        let (origin, angles, owner) = (v.origin, v.angles, v.owner);
        let (forward, _, _) = angle_vectors(angles);
        let tr = eng.trace_line(origin, origin + forward * SPRAY_RANGE, MoveFilter::NoMonsters, owner);
        decal_trace(eng, &tr, decal_number::LAMBDA6);
        eng.remove_entity(id);
    }
}
