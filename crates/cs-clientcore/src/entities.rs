//! The client's entity store: one [`ClientEntity`] per entity number with the current and the
//! previous state, the position history interpolation reads and the latched variables the
//! studio renderer lerps with.
//!
//! Ported from Xash3D FWGS `engine/client/cl_frame.c` (GPL-3.0-or-later; read 2026-11-23):
//! `CL_UpdatePositions`, `CL_ResetPositions`, `CL_EntityTeleported`, `CL_CompareTimestamps`,
//! `CL_EntityIgnoreLerp`, `CL_EntityCustomLerp`, `CL_UpdateLatchedVars`, `CL_ResetLatchedVars`,
//! `CL_ProcessEntityUpdate`, `CL_ResetLatchedState` and `CL_ProcessPacket`; the baseline part of
//! `CL_ParseBaseline` (`cl_parse.c`).
//!
//! What the engine does with a model and this store has no access to is taken from the model's
//! name instead: [`ModelKind::from_name`] tells brush, sprite and studio models apart the way
//! `Mod_*` does by the file's header; Xash3D's `mod_alias` (Quake models) does not exist in
//! GoldSrc and is left out.

use std::collections::BTreeMap;

use cs_proto48::delta::GoldSrcEntityState;
use cs_proto48::messages::ENTITY_NORMAL;
use glam::Vec3;

use crate::frames::Frame;
use crate::interp::InterpSettings;
use crate::precache::PrecacheTables;

/// `clgame.maxEntities` for GoldSrc: the entity number is 11 bits on the wire.
pub const MAX_ENTITIES: u16 = 2048;
/// `HISTORY_MAX` of `cl_entity.h`: the number of positions an entity remembers. Assumption:
/// the header was not fetched; 64 is the Half-Life SDK value.
pub const HISTORY_MAX: usize = 64;
pub const HISTORY_MASK: u32 = HISTORY_MAX as u32 - 1;
/// `GAME_FPS` (`common.h`): the update rate `CL_EntityTeleported` measures a jump against.
pub const GAME_FPS: f32 = 20.0;

/// `entity_state_t::effects` bits (`const.h`).
pub mod ef {
    pub const BRIGHTFIELD: u32 = 1;
    pub const MUZZLEFLASH: u32 = 2;
    pub const BRIGHTLIGHT: u32 = 4;
    pub const DIMLIGHT: u32 = 8;
    pub const INVLIGHT: u32 = 16;
    pub const NOINTERP: u32 = 32;
    pub const LIGHT: u32 = 64;
    pub const NODRAW: u32 = 128;
}

/// `MOVETYPE_*` (`const.h`; `MOVETYPE_COMPOUND` is Xash3D's addition). Assumption: the values
/// are the Half-Life SDK's and Xash3D's; the headers were not fetched.
pub mod movetype {
    pub const NONE: u8 = 0;
    pub const WALK: u8 = 3;
    pub const STEP: u8 = 4;
    pub const FLY: u8 = 5;
    pub const FOLLOW: u8 = 12;
    pub const COMPOUND: u8 = 14;
}

/// `EFLAG_SLERP` of `entity_state_t::eflags`.
pub const EFLAG_SLERP: u8 = 1;

/// What kind of model an entity shows (`model_t::type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    /// A map or a submodel (`maps/x.bsp`, `*3`): `mod_brush`.
    Brush,
    /// A sprite: `mod_sprite`.
    Sprite,
    /// A studio model: `mod_studio`.
    Studio,
}

impl ModelKind {
    /// The kind of the model the server calls `name` in its precache list. A name that is none
    /// of the three (an empty slot, a generic file) has no kind: the entity is not drawn, as
    /// Xash3D does for an entity whose `model` is `NULL`.
    pub fn from_name(name: &str) -> Option<Self> {
        if name.starts_with('*') {
            return Some(Self::Brush);
        }
        let ext = name.rsplit_once('.')?.1;
        if ext.eq_ignore_ascii_case("bsp") {
            Some(Self::Brush)
        } else if ext.eq_ignore_ascii_case("spr") {
            Some(Self::Sprite)
        } else if ext.eq_ignore_ascii_case("mdl") {
            Some(Self::Studio)
        } else {
            None
        }
    }
}

/// `position_history_t`: one remembered position of an entity.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PositionHistory {
    pub animtime: f32,
    pub origin: Vec3,
    pub angles: Vec3,
}

/// `cl_entity_t::latched`: the previous values the studio renderer interpolates from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Latched {
    pub prevorigin: Vec3,
    pub prevangles: Vec3,
    pub prevframe: f32,
    pub prevanimtime: f32,
    pub prevsequence: u16,
    pub sequencetime: f32,
    pub prevseqblending: [u8; 2],
    pub prevcontroller: [u8; 4],
    pub prevblending: [u8; 2],
    pub gaitsequence: i32,
    pub gaitframe: f32,
    pub gaityaw: f32,
    pub prevgaitorigin: Vec3,
}

/// `cl_entity_t`.
#[derive(Debug, Clone)]
pub struct ClientEntity {
    pub index: u16,
    /// `cl_entity_t::player`: the number is a player slot.
    pub player: bool,
    /// The model `curstate.modelindex` names; `None` when the index is unknown.
    pub model: Option<ModelKind>,
    pub baseline: GoldSrcEntityState,
    pub curstate: GoldSrcEntityState,
    pub prevstate: GoldSrcEntityState,
    /// `curstate.msg_time`: the server time of the packet `curstate` came in.
    pub cur_msg_time: f32,
    /// `prevstate.msg_time`.
    pub prev_msg_time: f32,
    /// `curstate.messagenum`: the sequence of the packet `curstate` came in (0: never).
    pub messagenum: u32,
    pub latched: Latched,
    pub ph: [PositionHistory; HISTORY_MAX],
    pub current_position: u32,
    /// `cl_entity_t::lastmove` (assumption: a float, as in the Xash3D header that was not
    /// fetched).
    pub lastmove: f32,
    /// The position the entity is drawn at after interpolation.
    pub origin: Vec3,
    pub angles: Vec3,
    pub attachment: [Vec3; 4],
}

impl ClientEntity {
    fn new(index: u16) -> Self {
        Self {
            index,
            player: false,
            model: None,
            baseline: GoldSrcEntityState::default(),
            curstate: GoldSrcEntityState::default(),
            prevstate: GoldSrcEntityState::default(),
            cur_msg_time: 0.0,
            prev_msg_time: 0.0,
            messagenum: 0,
            latched: Latched::default(),
            ph: [PositionHistory::default(); HISTORY_MAX],
            current_position: 0,
            lastmove: 0.0,
            origin: Vec3::ZERO,
            angles: Vec3::ZERO,
            attachment: [Vec3::ZERO; 4],
        }
    }

    /// `ent->curstate.starttime != 0 && ent->curstate.impacttime != 0`.
    pub fn is_parametric(&self) -> bool {
        self.curstate.starttime != 0.0 && self.curstate.impacttime != 0.0
    }

    /// `CL_UpdatePositions`: stores the current state in the interpolation ring.
    ///
    /// Quirk kept from the source (its comment: "for some reason, this sometimes still may
    /// happen"): an animtime that goes backwards is replaced by the previous one plus the time
    /// between the two server messages.
    pub fn update_positions(&mut self) {
        let prev = self.ph[(self.current_position & HISTORY_MASK) as usize];
        self.current_position = (self.current_position + 1) & HISTORY_MASK;
        let ph = &mut self.ph[self.current_position as usize];
        ph.origin = self.curstate.origin;
        ph.angles = self.curstate.angles;
        ph.animtime = self.curstate.animtime;
        if ph.animtime < prev.animtime {
            let diff = (self.cur_msg_time - self.prev_msg_time).max(0.0);
            ph.animtime = prev.animtime + diff;
        }
    }

    /// `CL_ResetPositions`: forgets the history after a teleport and starts it again from the
    /// newest entry.
    pub fn reset_positions(&mut self) {
        let store = self.ph[(self.current_position & HISTORY_MASK) as usize];
        self.current_position = 1;
        self.ph = [PositionHistory::default(); HISTORY_MAX];
        self.ph[1] = store;
        self.ph[0] = store;
    }

    /// `CL_EntityTeleported`: the entity moved further than `maxvelocity` allows in one update.
    pub fn teleported(&self, max_velocity: f32) -> bool {
        let d = self.curstate.origin - self.prevstate.origin;
        let maxlen = max_velocity * (1.0f32 / GAME_FPS);
        let len = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
        len > maxlen
    }

    /// `CL_ResetLatchedVars` (studio models only: the source returns early for the others).
    ///
    /// Deviation: `latched.prevframe` is the state's `frame`; the engine estimates the frame
    /// from the sequence data of the model (`R_StudioEstimateFrame`), which this crate does not
    /// have.
    pub fn reset_latched_vars(&mut self, full_reset: bool, mtime: f64) {
        if self.model != Some(ModelKind::Studio) {
            return;
        }
        if full_reset {
            self.latched.prevblending = self.curstate.blending;
            self.latched.sequencetime = self.curstate.animtime;
            self.latched.prevcontroller = self.curstate.controller;
            self.latched.prevframe = self.curstate.frame;
            self.prevstate = self.curstate.clone();
            self.prev_msg_time = self.cur_msg_time;
        }
        self.latched.prevanimtime = mtime as f32;
        self.curstate.animtime = mtime as f32;
        self.latched.prevorigin = self.curstate.origin;
        self.latched.prevangles = self.curstate.angles;
        self.latched.prevsequence = self.curstate.sequence;
    }

    /// `CL_UpdateLatchedVars` (studio models only).
    fn update_latched_vars(&mut self) {
        if self.model != Some(ModelKind::Studio) {
            return;
        }
        self.latched.prevorigin = self.prevstate.origin;
        self.latched.prevangles = self.prevstate.angles;
        self.latched.prevanimtime = self.prevstate.animtime;
        self.latched.prevframe = self.prevstate.frame;
        if self.curstate.sequence != self.prevstate.sequence {
            self.latched.prevseqblending = self.prevstate.blending;
            self.latched.prevsequence = self.prevstate.sequence;
            self.latched.sequencetime = self.curstate.animtime;
        }
        self.latched.prevcontroller = self.prevstate.controller;
        self.latched.prevblending = self.prevstate.blending;
    }
}

/// `CL_CompareTimestamps`: whether two animtimes are the same to within a millisecond.
fn compare_timestamps(t1: f32, t2: f32) -> bool {
    let i1 = (t1 * 1000.0) as i32;
    let i2 = (t2 * 1000.0) as i32;
    (i1.wrapping_sub(i2)) <= 1
}

/// `CL_EntityIgnoreLerp`: `cl_nointerp`, or an entity that never moves by itself.
fn entity_ignore_lerp(e: &ClientEntity, settings: &InterpSettings) -> bool {
    if settings.nointerp {
        return true;
    }
    e.curstate.movetype == movetype::NONE
}

/// `CL_EntityCustomLerp`: the movetypes whose position the server does not step, so the
/// client interpolates their history.
pub fn entity_custom_lerp(e: &ClientEntity) -> bool {
    !matches!(
        e.curstate.movetype,
        movetype::NONE | movetype::STEP | movetype::WALK | movetype::FLY | movetype::COMPOUND
    )
}

/// What `CL_ProcessPacket` needs of the client's clock and settings.
pub struct ProcessContext<'a> {
    /// `cl.parsecount`: the sequence of the packet being processed.
    pub parsecount: u32,
    /// `cl.mtime[0]`.
    pub mtime: f64,
    /// `cl.time`.
    pub time: f64,
    pub models: &'a PrecacheTables,
    pub settings: &'a InterpSettings,
}

/// The entities the client knows, by number.
#[derive(Debug, Clone)]
pub struct EntityStore {
    pub(crate) ents: BTreeMap<u16, ClientEntity>,
    /// `cl.maxclients`.
    pub max_clients: u8,
    /// `cl.playernum`: the local player's slot, from 0; its entity number is one more.
    pub playernum: u8,
    /// `clgame.movevars.maxvelocity`; the default is the server's `sv_maxvelocity`.
    pub max_velocity: f32,
}

impl Default for EntityStore {
    fn default() -> Self {
        Self {
            ents: BTreeMap::new(),
            max_clients: 32,
            playernum: 0,
            max_velocity: 2000.0,
        }
    }
}

impl EntityStore {
    /// Forgets every entity (a new map).
    pub fn reset(&mut self) {
        self.ents.clear();
    }

    /// `CL_IsPlayerIndex`.
    pub fn is_player_index(&self, number: u16) -> bool {
        number >= 1 && number <= u16::from(self.max_clients)
    }

    /// The entity number of the local player.
    pub fn local_entity_number(&self) -> u16 {
        u16::from(self.playernum) + 1
    }

    pub fn get(&self, number: u16) -> Option<&ClientEntity> {
        self.ents.get(&number)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ClientEntity> {
        self.ents.values()
    }

    pub fn len(&self) -> usize {
        self.ents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ents.is_empty()
    }

    /// `CL_ParseBaseline`: stores a baseline and clears the previous state of its entity.
    pub fn set_baseline(&mut self, state: GoldSrcEntityState) {
        let number = state.number;
        if number >= MAX_ENTITIES {
            return;
        }
        let ent = self
            .ents
            .entry(number)
            .or_insert_with(|| ClientEntity::new(number));
        ent.index = number;
        ent.prevstate = GoldSrcEntityState::default();
        ent.baseline = state;
    }

    /// `CL_ProcessPacket`: applies a frame to the entities it names.
    ///
    /// Not ported here: `pfnTxferLocalOverrides` (the local player's state takes the
    /// `clientdata` origin and velocity: prediction, task 5) and `CL_CheckPredictionError`.
    pub fn process_packet(&mut self, frame: &Frame, cx: &ProcessContext<'_>) {
        for fe in &frame.entities {
            let number = fe.state.number;
            if number >= MAX_ENTITIES {
                continue;
            }
            // `CL_ProcessEntityUpdate` reads the origin of the entity this one follows.
            let aiment_origin = (fe.state.movetype == movetype::FOLLOW
                && fe.state.origin == Vec3::ZERO
                && fe.state.aiment != 0)
                .then(|| self.ents.get(&fe.state.aiment).map(|e| e.origin))
                .flatten();
            let player = self.is_player_index(number);
            let ent = self
                .ents
                .entry(number)
                .or_insert_with(|| ClientEntity::new(number));
            ent.player = player;

            // Shuffle the states.
            ent.prevstate = std::mem::replace(&mut ent.curstate, fe.state.clone());
            ent.prev_msg_time = ent.cur_msg_time;
            ent.cur_msg_time = cx.mtime as f32;
            ent.messagenum = cx.parsecount;

            process_entity_update(ent, aiment_origin, cx);
            if fe.is_new {
                reset_latched_state(ent, cx);
            }
        }
    }
}

/// `CL_ProcessEntityUpdate`: changes since a new frame arrived.
fn process_entity_update(ent: &mut ClientEntity, aiment_origin: Option<Vec3>, cx: &ProcessContext<'_>) {
    ent.model = cx
        .models
        .model(ent.curstate.modelindex)
        .and_then(ModelKind::from_name);
    ent.index = ent.curstate.number;

    if ent.curstate.entity_type & ENTITY_NORMAL != 0 {
        normalize_angles(&mut ent.curstate.angles);
    }

    // The source's own remark: follow entities come with a null origin, and the origin of
    // the entity they follow stands in, "it's probably should be done somewhere else, as
    // goldsrc doesn't do this here".
    if let Some(origin) = aiment_origin {
        ent.curstate.origin = origin;
    }

    let parametric = ent.is_parametric();

    // Allow interpolation on bmodels too.
    if ent.model == Some(ModelKind::Brush) {
        ent.curstate.animtime = ent.cur_msg_time;
    }
    if entity_custom_lerp(ent) && !parametric {
        ent.curstate.animtime = ent.cur_msg_time;
    }

    if !compare_timestamps(ent.curstate.animtime, ent.prevstate.animtime)
        || entity_ignore_lerp(ent, cx.settings)
    {
        ent.update_latched_vars();
        ent.update_positions();
    }

    // "g-cont. it should be done for all the players?"
    if ent.player && !cx.settings.compute_studio_lerp {
        ent.curstate.angles.x /= -3.0;
    }

    ent.origin = ent.curstate.origin;
    ent.angles = ent.curstate.angles;
    ent.attachment = [ent.origin; 4];
}

/// `CL_ResetLatchedState` for an entity that is new in the frame (or `EF_NOINTERP`).
fn reset_latched_state(ent: &mut ClientEntity, cx: &ProcessContext<'_>) {
    ent.latched.prevorigin = ent.curstate.origin;
    ent.latched.prevangles = ent.curstate.angles;
    ent.reset_latched_vars(true, cx.mtime);
    ent.reset_positions();
    // Parametric interpolation starts at this point.
    if ent.is_parametric() {
        ent.lastmove = cx.time as f32;
    }
}

/// `COM_NormalizeAngles`.
fn normalize_angles(angles: &mut Vec3) {
    for a in [&mut angles.x, &mut angles.y, &mut angles.z] {
        if *a > 180.0 {
            *a -= 360.0;
        } else if *a < -180.0 {
            *a += 360.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_kind_follows_the_name() {
        assert_eq!(ModelKind::from_name("maps/de_dust2.bsp"), Some(ModelKind::Brush));
        assert_eq!(ModelKind::from_name("*12"), Some(ModelKind::Brush));
        assert_eq!(ModelKind::from_name("models/w_ak47.MDL"), Some(ModelKind::Studio));
        assert_eq!(ModelKind::from_name("sprites/muzzleflash1.spr"), Some(ModelKind::Sprite));
        assert_eq!(ModelKind::from_name("sprites/hud.txt"), None);
        assert_eq!(ModelKind::from_name(""), None);
    }

    #[test]
    fn timestamps_within_a_millisecond_are_equal() {
        assert!(compare_timestamps(1.0, 1.0));
        assert!(compare_timestamps(1.001, 1.0));
        assert!(!compare_timestamps(1.05, 1.0));
        // The test is one-sided: an older time on the left is always "equal".
        assert!(compare_timestamps(0.5, 1.0));
    }

    #[test]
    fn positions_ring_wraps_and_reset_keeps_the_newest() {
        let mut e = ClientEntity::new(7);
        for i in 0..70u32 {
            e.curstate.origin = Vec3::new(i as f32, 0.0, 0.0);
            e.curstate.animtime = 1.0 + i as f32 * 0.1;
            e.cur_msg_time = e.curstate.animtime;
            e.update_positions();
        }
        let newest = e.ph[(e.current_position & HISTORY_MASK) as usize];
        assert_eq!(newest.origin.x, 69.0);
        e.reset_positions();
        assert_eq!(e.current_position, 1);
        assert_eq!(e.ph[0], newest);
        assert_eq!(e.ph[1], newest);
        assert_eq!(e.ph[2], PositionHistory::default());
    }

    #[test]
    fn a_backwards_animtime_is_repaired() {
        let mut e = ClientEntity::new(7);
        e.curstate.animtime = 2.0;
        e.prev_msg_time = 1.0;
        e.cur_msg_time = 1.1;
        e.update_positions();
        e.curstate.animtime = 1.5;
        e.prev_msg_time = 1.1;
        e.cur_msg_time = 1.2;
        e.update_positions();
        let ph = e.ph[(e.current_position & HISTORY_MASK) as usize];
        assert!((ph.animtime - 2.1).abs() < 1e-5);
    }
}
