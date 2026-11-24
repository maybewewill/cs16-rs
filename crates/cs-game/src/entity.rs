//! The game's entity classes: the [`GameEntity`] trait, the registry that makes one from a map
//! `classname`, and the dispatch of the engine's `think`, `touch` and `blocked` calls to the
//! entity that owns the edict.
//!
//! An entity lives in the private data of its edict as a `Box<dyn GameEntity>`. For the length
//! of one call it is taken out of the edict (a [`Vacant`] stand-in takes its place) and put back
//! afterwards if the edict is still the same live entity, so the entity, the [`GameState`] and
//! the [`EngineApi`] are never borrowed together.

use cs_engine::consts::{FL_KILLME, MOVETYPE_STEP, MOVETYPE_WALK, SOLID_NOT, SOLID_TRIGGER};
use cs_engine::{EdictId, EngineApi, EntTrace};
use glam::Vec3;

use crate::GameState;
use crate::buy::BuyZone;
use crate::bullets::{self, MultiDamage};
use crate::player::{DEAD_DEAD, DEAD_NO, player_slot};
use crate::world::{BrushKind, FuncLadder, InfoPlayer, InfoTarget, StaticBrush, TriggerCamera, World};

/// `USE_TYPE` (the Half-Life SDK's `cbase.h`; assumption: the header is not among the fetched
/// sources): how an entity is used. `USE_OFF` is the default argument of `CBaseEntity::Use`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UseType {
    #[default]
    Off = 0,
    On = 1,
    Set = 2,
    Toggle = 3,
}

impl UseType {
    /// The `USE_TYPE` a stored integer stands for (`(USE_TYPE)pev->button` of a `DelayedUse`);
    /// a number that is none of them is `USE_OFF`.
    pub fn from_i32(n: i32) -> Self {
        match n {
            1 => Self::On,
            2 => Self::Set,
            3 => Self::Toggle,
            _ => Self::Off,
        }
    }
}

/// `FCAP_*` (`regamedll_const.h`, read 2026-11-23): what an entity can do across a level change
/// and with the use key. `FCAP_ACROSS_TRANSITION` is what `CBaseEntity::ObjectCaps` says.
pub const FCAP_ACROSS_TRANSITION: i32 = 0x0000_0002;
/// The player's use key calls `Use` once, when it goes down.
pub const FCAP_IMPULSE_USE: i32 = 0x0000_0008;
/// The player's use key calls `Use` on every command it is held.
pub const FCAP_CONTINUOUS_USE: i32 = 0x0000_0010;
/// The player's use key calls `Use` when it goes down (on) and when it is released (off).
pub const FCAP_ONOFF_USE: i32 = 0x0000_0020;

/// `DONT_BLEED` (`util.h`): the `BloodColor` of an entity that has no blood.
pub const DONT_BLEED: i32 = -1;

/// `GIB_NORMAL` (`const.h`): the `iGib` of an ordinary death.
pub const GIB_NORMAL: i32 = 0;

/// What one class of map entity does. Every method but [`classname`](Self::classname) has a
/// default that does nothing.
pub trait GameEntity: Send + 'static {
    fn classname(&self) -> &'static str;

    /// Whether this is the stand-in of an entity that is out for a call; no game class says yes.
    #[doc(hidden)]
    fn is_vacant(&self) -> bool {
        false
    }

    /// Sets the entity up after the engine filled the edict's variables from `keys`. `false`
    /// removes the entity (the engine frees the edict).
    fn spawn(
        &mut self,
        _eng: &mut EngineApi<'_>,
        _id: EdictId,
        _keys: &[(String, String)],
    ) -> bool {
        true
    }

    /// The think scheduled through `EntVars::nextthink` is due.
    fn think(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {}

    /// `other` touched this entity.
    fn touch(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _id: EdictId,
        _other: EdictId,
    ) {
    }

    /// The round starts over and the map's entities are made again (`UTIL_RestartOther`): the
    /// entity goes back to how it spawned. Only the `armoury_entity` does anything.
    fn restart(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {}

    /// `other` blocked this entity's push movement.
    fn blocked(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _id: EdictId,
        _other: EdictId,
    ) {
    }

    /// `CBaseEntity::Use(pActivator, pCaller, useType, value)`: the entity is used by the
    /// player's use key, by another entity's `target` (`FireTargets`) or by a touch. The default
    /// (an entity with no `SetUse` function) does nothing.
    #[allow(clippy::too_many_arguments)]
    fn use_(
        &mut self,
        _g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        _id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: UseType,
        _value: f32,
    ) {
    }

    /// `CBaseEntity::ObjectCaps` (`FCAP_*`): `FCAP_ACROSS_TRANSITION`, no use key.
    fn object_caps(&self) -> i32 {
        FCAP_ACROSS_TRANSITION
    }

    /// `pev->takedamage != DAMAGE_NO`. The engine's entity variables have no `takedamage`, so
    /// the class says it: the default is `DAMAGE_NO`; an entity that can be shot says yes (and
    /// no again once its [`killed`](Self::killed) ran, the source's `pev->takedamage =
    /// DAMAGE_NO`).
    fn takes_damage(&self) -> bool {
        false
    }

    /// `CBaseEntity::BloodColor`: `DONT_BLEED`.
    fn blood_color(&self) -> i32 {
        DONT_BLEED
    }

    /// `CBaseEntity::IsAlive`: `pev->deadflag == DEAD_NO && pev->health > 0`.
    fn is_alive(&self, eng: &EngineApi<'_>, id: EdictId) -> bool {
        eng.ent(id).is_some_and(|v| v.deadflag == DEAD_NO && v.health > 0.0)
    }

    /// Whether this entity is a dropped C4 (`CWeaponBox::m_bIsBomb` or map `weapon_c4`).
    fn is_dropped_bomb(&self) -> bool {
        false
    }

    /// Where the `weapon_c4` of this dropped C4 lies, for `InitHUD`'s `BombDrop`
    /// (`pWeaponC4->pev->origin`); `own_origin` is this entity's current origin. `None` when
    /// this is no dropped bomb. A map `weapon_c4` is where it is; the C4 in a weapon box stays
    /// where it was packed (the box moves, the packed weapon's origin does not).
    fn dropped_bomb_origin(&self, own_origin: glam::Vec3) -> Option<glam::Vec3> {
        self.is_dropped_bomb().then_some(own_origin)
    }

    /// Whether this entity is a planted C4 (`CGrenade::m_bIsC4`).
    fn is_planted_c4(&self) -> bool {
        false
    }

    /// Whether this entity is a planted, unexploded C4 (`CGrenade::m_bIsC4 && !m_bJustBlew`).
    fn is_planted_live_c4(&self) -> bool {
        false
    }

    /// Whether this entity is a hostage (`CLASS_HUMAN_PASSIVE`).
    fn is_hostage(&self) -> bool {
        false
    }

    /// Whether this hostage has already reached rescue (`m_bRescued`).
    fn is_hostage_rescued(&self) -> bool {
        false
    }

    /// Player edict this hostage is currently following (`State::Follow`), if any.
    fn hostage_follower(&self) -> Option<EdictId> {
        None
    }

    /// Sets the 1-based hostage index assigned at spawn (`m_iHostageIndex`).
    fn set_hostage_index(&mut self, _idx: u8) {}

    /// The 1-based hostage index (`m_iHostageIndex`), if this entity is a hostage.
    fn hostage_index(&self) -> Option<u8> {
        None
    }

    /// Marks a hostage as standing inside a rescue zone (`m_bRescueMe = TRUE`).
    fn mark_hostage_rescue(&mut self) {}

    /// Runs `CHostage::NavReady` when `CLocalNav` grants a path request.
    fn hostage_nav_ready(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {}

    /// Runs `CHostage::PreThink` per-frame stair-step logic.
    fn hostage_pre_think(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {}

    /// `CBaseEntity::BodyTarget(posSrc)`: `Center()`, `(pev->absmax + pev->absmin) * 0.5`.
    fn body_target(&self, eng: &EngineApi<'_>, id: EdictId, _src: Vec3) -> Vec3 {
        eng.ent(id).map_or(Vec3::ZERO, |v| (v.absmax + v.absmin) * 0.5)
    }

    /// `CBaseEntity::TraceAttack(pevAttacker, flDamage, vecDir, ptr, bitsDamageType)`
    /// (`cbase.cpp:1000`, read 2026-11-22): an entity that takes damage adds the damage to the
    /// shot's gathered damage (`mm`, the source's `gMultiDamage`; `attacker` is what the source
    /// passes as `pevAttacker`, which `AddMultiDamage` takes for the inflictor) and, if it
    /// bleeds, makes a little surface blood and bleed decals.
    #[allow(clippy::too_many_arguments)]
    fn trace_attack(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        attacker: Option<EdictId>,
        mm: &mut MultiDamage,
        damage: f32,
        dir: Vec3,
        tr: &EntTrace,
        bits: u32,
    ) {
        let origin = tr.trace.end_pos - dir * 4.0;
        if self.takes_damage() {
            bullets::add_multi_damage(g, eng, mm, attacker, id, damage, bits);
            let blood = self.blood_color();
            if blood != DONT_BLEED {
                // A little surface blood. Assumption: the only blood the game has is red.
                bullets::spawn_blood(eng, origin, blood as u8, damage);
                bullets::trace_bleed(g, eng, id, damage, dir, tr.trace.end_pos, bits);
            }
        }
    }

    /// `CBaseEntity::TakeDamage(pevInflictor, pevAttacker, flDamage, bitsDamageType)`
    /// (`cbase.cpp:660`, read 2026-11-22, vanilla): a `DAMAGE_NO` entity takes nothing. A walking
    /// or stepping entity is pushed away from the inflictor's centre by
    /// `min(damage * (32*32*72 / volume) * 5, 1000)`, unless the attacker is a trigger. Then the
    /// health goes down and at 0 or below the entity is [`killed`](Self::killed) (`GIB_NORMAL`).
    /// Returns `TRUE` for an entity that survived the hit.
    ///
    /// Not kept: `g_vecAttackDir` (nothing reads it until `func_breakable` is ported). A guard
    /// the source lacks: a box with no volume gives a non-finite force, which is dropped instead
    /// of poisoning the velocity; a missing attacker (a null pointer in the source) is no push.
    #[allow(clippy::too_many_arguments)]
    fn take_damage(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        inflictor: Option<EdictId>,
        attacker: Option<EdictId>,
        damage: f32,
        _bits: u32,
    ) -> bool {
        if !self.takes_damage() {
            return false;
        }
        // Figure the momentum: "don't let hurt brushes or other triggers move player".
        let attacker_solid = attacker.and_then(|a| eng.ent(a)).map(|v| v.solid);
        if let (Some(inflictor), Some(attacker_solid)) = (inflictor, attacker_solid)
            && attacker_solid != SOLID_TRIGGER
            && let (Some(i), Some(v)) = (eng.ent(inflictor), eng.ent(id))
            && (v.movetype == MOVETYPE_WALK || v.movetype == MOVETYPE_STEP)
        {
            let dir = bullets::normalize_reciprocal(v.origin - (i.absmin + i.absmax) * 0.5);
            // `real_t flForce = flDamage * ((32 * 32 * 72.0) / (size.x * size.y * size.z)) * 5`:
            // the `double` literal makes the arithmetic `double`, stored to a float.
            let volume = f64::from(v.size.x * v.size.y * v.size.z);
            let mut force = (f64::from(damage) * ((32.0 * 32.0 * 72.0) / volume) * 5.0) as f32;
            if force > 1000.0 {
                force = 1000.0;
            }
            if force.is_finite() {
                let velocity = v.velocity + dir * force;
                if let Some(v) = eng.ent_mut(id) {
                    v.velocity = velocity;
                }
            }
        }
        // Do the damage.
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.health -= damage;
        if v.health <= 0.0 {
            self.killed(g, eng, id, attacker, GIB_NORMAL);
            return false;
        }
        true
    }

    /// `CBaseEntity::Killed(pevAttacker, iGib)` (`cbase.cpp:715`, read 2026-11-22): `deadflag`
    /// becomes `DEAD_DEAD`, `takedamage` becomes `DAMAGE_NO` ([`stop_damage`](Self::stop_damage))
    /// and the entity is removed ([`util_remove`]). A class that overrides this does the same.
    fn killed(
        &mut self,
        _g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _attacker: Option<EdictId>,
        _gib: i32,
    ) {
        self.stop_damage();
        if let Some(v) = eng.ent_mut(id) {
            v.deadflag = DEAD_DEAD;
        }
        util_remove(eng, id);
    }

    /// `pev->takedamage = DAMAGE_NO`, as `Killed` does: a class that keeps its own damageable
    /// flag clears it here (the default class has none, it never takes damage).
    fn stop_damage(&mut self) {}

    /// Whether the class derives from `CBaseDelay` (`CBaseToggle`, triggers, buttons, doors...),
    /// the only ones that read the `delay` and `killtarget` keys ([`crate::targets::note_keys`]).
    fn is_delay(&self) -> bool {
        false
    }
}

/// `UTIL_Remove(pEntity)` (`util.cpp:1450`, read 2026-11-22, vanilla): the entity is not freed
/// at once; it becomes non-solid, is marked `FL_KILLME` (the engine frees it after its turn) and
/// loses its `targetname`, so nothing finds it any more. `UpdateOnRemove` (the cleanup of the
/// `m_pGoalEnt` links, which no game class here has) is not called.
pub(crate) fn util_remove(eng: &mut EngineApi<'_>, id: EdictId) {
    if let Some(v) = eng.ent_mut(id) {
        v.solid = SOLID_NOT;
        v.flags |= FL_KILLME;
        v.targetname.clear();
    }
}

/// `CBaseEntity::Use` of the entity `id`, if it has a game class. The entity is out of its
/// edict for the call (see the module notes), so an entity used from within its own call is
/// not told.
#[allow(clippy::too_many_arguments)]
pub(crate) fn use_entity(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    id: EdictId,
    activator: Option<EdictId>,
    caller: Option<EdictId>,
    use_type: UseType,
    value: f32,
) {
    with_entity(g, eng, id, |entity, g, eng| {
        entity.use_(g, eng, id, activator, caller, use_type, value);
    });
}

/// `ObjectCaps()` of the entity `id`: a player's has no use bits (`CBasePlayer::ObjectCaps`
/// drops `FCAP_ACROSS_TRANSITION`: 0), an edict with no game class has 0, a game entity says
/// its own.
pub(crate) fn object_caps_of(eng: &EngineApi<'_>, id: EdictId) -> i32 {
    eng.private::<Box<dyn GameEntity>>(id).map_or(0, |e| e.object_caps())
}

/// `pev->takedamage != DAMAGE_NO` of an entity that is not a player: what its class says. Like
/// the source it does not look at `FL_KILLME`: a plain `UTIL_Remove` keeps `takedamage` (only
/// `Killed` sets `DAMAGE_NO`), and the entity stays damageable until the engine frees it. An
/// entity that is out of its edict for its own call is a [`Vacant`] (no damage); that cannot be
/// seen yet, as no class damages entities from within its own method.
pub(crate) fn entity_takes_damage(eng: &EngineApi<'_>, id: EdictId) -> bool {
    eng.private::<Box<dyn GameEntity>>(id).is_some_and(|e| e.takes_damage())
}

/// `BodyTarget(posSrc)` of whichever entity `id` is: a player's is its centre plus `view_ofs *
/// RANDOM_FLOAT(0.5, 1.1)` (vanilla; `REGAMEDLL_FIXES` uses the eyes), the class's
/// [`GameEntity::body_target`] for the rest. A stale edict has the origin.
pub(crate) fn body_target_of(
    g: &mut GameState,
    eng: &EngineApi<'_>,
    id: EdictId,
    src: Vec3,
) -> Vec3 {
    if player_slot(g, eng, id).is_some() {
        let Some(v) = eng.ent(id) else {
            return Vec3::ZERO;
        };
        let (center, view_ofs) = ((v.absmax + v.absmin) * 0.5, v.view_ofs);
        return center + view_ofs * bullets::random_float(g, 0.5, 1.1);
    }
    eng.private::<Box<dyn GameEntity>>(id)
        .map_or_else(|| eng.ent(id).map_or(Vec3::ZERO, |v| (v.absmax + v.absmin) * 0.5), |e| e.body_target(eng, id, src))
}

/// `TakeDamage(pevInflictor, pevAttacker, flDamage, bitsDamageType)` of whichever entity `id`
/// is: [`player::take_damage`](crate::player::take_damage) for a player (its attacker's weapon
/// found as usual), the class's [`GameEntity::take_damage`] for the rest. `false` for a stale
/// edict or one with no class.
pub(crate) fn take_damage_of(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    id: EdictId,
    inflictor: Option<EdictId>,
    attacker: Option<EdictId>,
    damage: f32,
    bits: u32,
) -> bool {
    if let Some(slot) = player_slot(g, eng, id) {
        return crate::player::take_damage(g, eng, slot as u8, inflictor, attacker, damage, bits);
    }
    let mut hurt = false;
    with_entity(g, eng, id, |entity, g, eng| {
        hurt = entity.take_damage(g, eng, id, inflictor, attacker, damage, bits);
    });
    hurt
}

/// The entity of class `classname` (map entity names are case-sensitive), `None` for a class
/// the game does not know.
pub fn create_entity(classname: &str) -> Option<Box<dyn GameEntity>> {
    let entity: Box<dyn GameEntity> = match classname {
        "worldspawn" => Box::new(World),
        "info_player_start" => Box::new(InfoPlayer("info_player_start")),
        "info_player_deathmatch" => Box::new(InfoPlayer("info_player_deathmatch")),
        "info_target" => Box::new(InfoTarget),
        "trigger_camera" => Box::new(TriggerCamera::default()),
        "func_ladder" => Box::new(FuncLadder),
        "func_buyzone" => Box::new(BuyZone::default()),
        "func_bomb_target" => Box::new(crate::zones::BombTarget),
        "info_bomb_target" => Box::new(crate::zones::PointMarker("info_bomb_target")),
        "func_hostage_rescue" => Box::new(crate::zones::HostageRescue),
        "info_hostage_rescue" => Box::new(crate::zones::PointMarker("info_hostage_rescue")),
        "func_escapezone" => Box::new(crate::zones::EscapeZone),
        "func_vip_safetyzone" => Box::new(crate::zones::VipSafetyZone),
        "info_vip_start" => Box::new(crate::zones::PointMarker("info_vip_start")),
        "info_map_parameters" => Box::new(crate::zones::InfoMapParameters),
        "hostage_entity" | "monster_scientist" => Box::new(crate::hostage::Hostage::default()),
        #[cfg(feature = "test-hooks")]
        "test_use_counter" => Box::new(crate::zones::UseCounter),
        "func_wall" => Box::new(StaticBrush::new("func_wall", BrushKind::Solid)),
        "func_door" => Box::new(StaticBrush::new("func_door", BrushKind::Door)),
        "func_water" => Box::new(StaticBrush::new("func_water", BrushKind::Door)),
        "func_door_rotating" => {
            Box::new(StaticBrush::new("func_door_rotating", BrushKind::RotatingDoor))
        }
        "func_breakable" => Box::new(StaticBrush::new("func_breakable", BrushKind::Solid)),
        "func_button" => Box::new(StaticBrush::new("func_button", BrushKind::Solid)),
        "func_plat" => Box::new(crate::movers::FuncPlat::default()),
        "func_train" => Box::new(crate::movers::FuncTrain::default()),
        "func_pushable" => Box::new(crate::movers::FuncPushable::default()),
        "func_conveyor" => Box::new(crate::movers::FuncConveyor::default()),
        "path_corner" => Box::new(crate::movers::PathCorner::default()),
        "func_illusionary" => Box::new(StaticBrush::new("func_illusionary", BrushKind::Illusionary)),
        #[cfg(feature = "test-hooks")]
        "test_dispatch" => Box::new(DispatchProbe(ProbeMode::Plain)),
        #[cfg(feature = "test-hooks")]
        "test_dispatch_remove_on_think" => Box::new(DispatchProbe(ProbeMode::RemoveOnThink)),
        #[cfg(feature = "test-hooks")]
        "test_dispatch_remove_on_touch" => Box::new(DispatchProbe(ProbeMode::RemoveOnTouch)),
        #[cfg(feature = "test-hooks")]
        "test_dispatch_replace_on_think" => Box::new(DispatchProbe(ProbeMode::ReplaceOnThink)),
        #[cfg(feature = "test-hooks")]
        "test_dispatch_create_on_think" => Box::new(DispatchProbe(ProbeMode::CreateOnThink)),
        #[cfg(feature = "test-hooks")]
        "test_target" => Box::new(TestTarget::default()),
        "armoury_entity" => Box::new(crate::inventory::Armoury::default()),
        "item_thighpack" => Box::new(crate::inventory::ItemThighPack),
        "grenade" => Box::new(crate::grenade::Grenade::default()),
        "spark_shower" => Box::new(crate::grenade::SparkShower),
        "spraycan" => Box::new(crate::spray::SprayCan),
        other => Box::new(crate::inventory::WorldWeapon::of_class(other)?),
    };
    Some(entity)
}

/// Stands in an edict's private data while its entity is out for a call.
struct Vacant;

impl GameEntity for Vacant {
    fn classname(&self) -> &'static str {
        "(vacant)"
    }

    fn is_vacant(&self) -> bool {
        true
    }
}

/// Stores `entity` as the game entity of `id`; a stale id is ignored.
pub(crate) fn attach(eng: &mut EngineApi<'_>, id: EdictId, entity: Box<dyn GameEntity>) {
    eng.set_private::<Box<dyn GameEntity>>(id, entity);
}

/// Runs `call` on the entity that owns `id`, if any. The entity is out of the edict during the
/// call and goes back afterwards unless the edict was freed (or its entity replaced) meanwhile.
pub(crate) fn with_entity(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    id: EdictId,
    call: impl FnOnce(&mut dyn GameEntity, &mut GameState, &mut EngineApi<'_>),
) {
    let Some(slot) = eng.private_mut::<Box<dyn GameEntity>>(id) else {
        return;
    };
    let mut entity = std::mem::replace(slot, Box::new(Vacant));
    call(entity.as_mut(), g, eng);
    if let Some(slot) = eng.private_mut::<Box<dyn GameEntity>>(id)
        && slot.is_vacant()
    {
        *slot = entity;
    }
}

/// One call the dispatch delivered to a [`DispatchProbe`]. For tests only (feature
/// `test-hooks`).
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchEvent {
    Think { id: EdictId },
    Touch { id: EdictId, other: EdictId },
    Blocked { id: EdictId, other: EdictId },
    /// `Use` of a `test_target`; `value_bits` is the `f32` value's bits.
    Use {
        id: EdictId,
        activator: Option<EdictId>,
        caller: Option<EdictId>,
        use_type: UseType,
        value_bits: u32,
    },
    /// `Killed` of a `test_target`.
    Killed { id: EdictId },
}

/// What a [`DispatchProbe`] does besides logging. For tests only (feature `test-hooks`).
#[cfg(feature = "test-hooks")]
#[derive(Clone, Copy)]
enum ProbeMode {
    Plain,
    RemoveOnThink,
    RemoveOnTouch,
    /// Replaces its own entity with a plain probe during its think.
    ReplaceOnThink,
    /// Creates a plain probe, due to think soon, during its think.
    CreateOnThink,
}

/// The classes `test_dispatch*`: log every call they receive into `GameState::dispatch_log`,
/// and some do something odd to themselves during the call. For tests only (feature
/// `test-hooks`).
#[cfg(feature = "test-hooks")]
struct DispatchProbe(ProbeMode);

#[cfg(feature = "test-hooks")]
impl GameEntity for DispatchProbe {
    fn classname(&self) -> &'static str {
        match self.0 {
            ProbeMode::Plain => "test_dispatch",
            ProbeMode::RemoveOnThink => "test_dispatch_remove_on_think",
            ProbeMode::RemoveOnTouch => "test_dispatch_remove_on_touch",
            ProbeMode::ReplaceOnThink => "test_dispatch_replace_on_think",
            ProbeMode::CreateOnThink => "test_dispatch_create_on_think",
        }
    }

    fn think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        g.dispatch_log.push(DispatchEvent::Think { id });
        match self.0 {
            ProbeMode::RemoveOnThink => eng.remove_entity(id),
            ProbeMode::ReplaceOnThink => {
                attach(eng, id, Box::new(DispatchProbe(ProbeMode::Plain)));
            }
            ProbeMode::CreateOnThink => {
                if let Some(child) = eng.create_entity("test_dispatch") {
                    attach(eng, child, Box::new(DispatchProbe(ProbeMode::Plain)));
                    let due = eng.time() + 0.015;
                    if let Some(v) = eng.ent_mut(child) {
                        v.nextthink = due;
                    }
                }
            }
            ProbeMode::Plain | ProbeMode::RemoveOnTouch => {}
        }
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        g.dispatch_log.push(DispatchEvent::Touch { id, other });
        if matches!(self.0, ProbeMode::RemoveOnTouch) {
            eng.remove_entity(id);
        }
    }

    fn blocked(
        &mut self,
        g: &mut GameState,
        _eng: &mut EngineApi<'_>,
        id: EdictId,
        other: EdictId,
    ) {
        g.dispatch_log.push(DispatchEvent::Blocked { id, other });
    }
}

/// The class `test_target`: an entity with the use caps, the health and the blood the map's keys
/// give it (`caps`, `health`, `damageable`, `blood`, `fires`), whose `use` and death go to
/// `GameState::dispatch_log`. For tests only (feature `test-hooks`).
#[cfg(feature = "test-hooks")]
#[derive(Default)]
struct TestTarget {
    caps: i32,
    damageable: bool,
    blood: i32,
    /// A use of it uses its own targets (`SUB_UseTargets`).
    fires: bool,
}

#[cfg(feature = "test-hooks")]
impl GameEntity for TestTarget {
    fn classname(&self) -> &'static str {
        "test_target"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let number = |key: &str| {
            keys.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| crate::world::atof(v))
        };
        self.caps = number("caps").unwrap_or(0.0) as i32;
        self.damageable = number("damageable").is_some_and(|n| n != 0.0);
        self.blood = number("blood").map_or(DONT_BLEED, |_| 247);
        self.fires = number("fires").is_some_and(|n| n != 0.0);
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.solid = cs_engine::consts::SOLID_BBOX;
        v.health = number("health").unwrap_or(100.0);
        let origin = v.origin;
        eng.set_size(id, Vec3::splat(-16.0), Vec3::splat(16.0));
        eng.set_origin(id, origin);
        true
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        caller: Option<EdictId>,
        use_type: UseType,
        value: f32,
    ) {
        g.dispatch_log.push(DispatchEvent::Use {
            id,
            activator,
            caller,
            use_type,
            value_bits: value.to_bits(),
        });
        if self.fires {
            crate::targets::use_targets(g, eng, id, activator, use_type, value);
        }
    }

    fn object_caps(&self) -> i32 {
        self.caps
    }

    fn takes_damage(&self) -> bool {
        self.damageable
    }

    fn stop_damage(&mut self) {
        self.damageable = false;
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn blood_color(&self) -> i32 {
        self.blood
    }

    fn killed(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _attacker: Option<EdictId>,
        _gib: i32,
    ) {
        g.dispatch_log.push(DispatchEvent::Killed { id });
        self.stop_damage();
        if let Some(v) = eng.ent_mut(id) {
            v.deadflag = DEAD_DEAD;
        }
        util_remove(eng, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_knows_the_map_classes_and_nothing_else() {
        for class in [
            "worldspawn",
            "info_player_start",
            "info_player_deathmatch",
            "info_target",
            "trigger_camera",
            "func_ladder",
            "func_buyzone",
            "func_bomb_target",
            "info_bomb_target",
            "func_hostage_rescue",
            "info_hostage_rescue",
            "func_escapezone",
            "func_vip_safetyzone",
            "info_vip_start",
            "info_map_parameters",
            "func_wall",
            "func_door",
            "func_door_rotating",
            "func_water",
            "func_breakable",
            "func_button",
            "func_plat",
            "func_train",
            "func_pushable",
            "func_illusionary",
            "armoury_entity",
            "item_thighpack",
            "grenade",
            "spark_shower",
            "hostage_entity",
            "weapon_ak47",
            "weapon_knife",
        ] {
            let entity = create_entity(class).unwrap_or_else(|| panic!("{class} missing"));
            assert_eq!(entity.classname(), class);
        }
        assert_eq!(
            create_entity("monster_scientist").unwrap().classname(),
            "hostage_entity"
        );
        for unknown in ["", "light", "FUNC_WALL", "func_buyzones", "worldspawn "] {
            assert!(create_entity(unknown).is_none(), "{unknown:?}");
        }
    }
}
