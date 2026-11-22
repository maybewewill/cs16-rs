//! The game's entity classes: the [`GameEntity`] trait, the registry that makes one from a map
//! `classname`, and the dispatch of the engine's `think`, `touch` and `blocked` calls to the
//! entity that owns the edict.
//!
//! An entity lives in the private data of its edict as a `Box<dyn GameEntity>`. For the length
//! of one call it is taken out of the edict (a [`Vacant`] stand-in takes its place) and put back
//! afterwards if the edict is still the same live entity, so the entity, the [`GameState`] and
//! the [`EngineApi`] are never borrowed together.

use cs_engine::{EdictId, EngineApi};

use crate::GameState;
use crate::buy::BuyZone;
use crate::world::{BrushKind, FuncLadder, InfoPlayer, InfoTarget, StaticBrush, TriggerCamera, World};

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
        "func_wall" => Box::new(StaticBrush::new("func_wall", BrushKind::Solid)),
        "func_door" => Box::new(StaticBrush::new("func_door", BrushKind::Door)),
        "func_water" => Box::new(StaticBrush::new("func_water", BrushKind::Door)),
        "func_door_rotating" => {
            Box::new(StaticBrush::new("func_door_rotating", BrushKind::RotatingDoor))
        }
        "func_breakable" => Box::new(StaticBrush::new("func_breakable", BrushKind::Solid)),
        "func_button" => Box::new(StaticBrush::new("func_button", BrushKind::Solid)),
        "func_plat" => Box::new(StaticBrush::new("func_plat", BrushKind::Solid)),
        "func_train" => Box::new(StaticBrush::new("func_train", BrushKind::Solid)),
        "func_pushable" => Box::new(StaticBrush::new("func_pushable", BrushKind::Solid)),
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
        "armoury_entity" => Box::new(crate::inventory::Armoury::default()),
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
            "weapon_ak47",
            "weapon_knife",
        ] {
            let entity = create_entity(class).unwrap_or_else(|| panic!("{class} missing"));
            assert_eq!(entity.classname(), class);
        }
        for unknown in ["", "light", "FUNC_WALL", "func_buyzones", "worldspawn "] {
            assert!(create_entity(unknown).is_none(), "{unknown:?}");
        }
    }
}
