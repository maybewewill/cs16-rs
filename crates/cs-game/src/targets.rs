//! `SUB_UseTargets`, `FireTargets`, the `killtarget` and the `delay` of `CBaseDelay`: how one
//! entity uses the entities named by its `target` (ReGameDLL_CS `subs.cpp`, read 2026-11-23,
//! vanilla build: not `REGAMEDLL_FIXES`).
//!
//! [`use_targets`] is the `SUB_UseTargets` an entity calls on itself, e.g. a bomb target when the
//! planted bomb explodes in its zone. The `delay` and `killtarget` keys of the map entity are
//! kept by [`note_keys`] (`CBaseDelay::KeyValue`) because the entity is out of its edict while
//! its own method runs (see [`crate::entity`]). Only a class that is a `CBaseDelay`
//! ([`GameEntity::is_delay`]) has them; the others never read the keys, so their call behaves as
//! `CBaseEntity::SUB_UseTargets` (it fires the targets).
//!
//! Gaps (named, none is needed by a classic map): the `REGAMEDLL_FIXES` recursion guard of
//! `FireTargets` (vanilla has none: an entity that targets itself calls itself as far as the
//! stack goes; here an entity that is out for its own call is not told again, so the loop ends),
//! and `m_flDelay` being saved across level changes.

use cs_engine::consts::FL_KILLME;
use cs_engine::{EdictId, EngineApi};

use crate::GameState;
use crate::entity::{GameEntity, UseType, attach, use_entity, util_remove};
use crate::player::player_slot;
use crate::world::atof;

/// `CBaseDelay`'s own keys: `m_flDelay` and `m_iszKillTarget`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct DelayKeys {
    /// `delay`: seconds to wait before the targets are used; 0 for none.
    pub(crate) delay: f32,
    /// `killtarget`: the `targetname` of the entities to remove first.
    pub(crate) kill_target: Option<String>,
}

/// `CBaseDelay::KeyValue` (`subs.cpp:84-100`, read 2026-11-23): keeps the `delay` (C `atof`) and
/// `killtarget` keys the map gave entity `id`. The last of a repeated key wins; an entity with
/// neither has no entry.
pub(crate) fn note_keys(g: &mut GameState, id: EdictId, keys: &[(String, String)]) {
    let mut found = DelayKeys::default();
    for (key, value) in keys {
        match key.as_str() {
            "delay" => found.delay = atof(value),
            "killtarget" => found.kill_target = Some(value.clone()),
            _ => {}
        }
    }
    if found.delay != 0.0 || found.kill_target.is_some() {
        g.delay_keys.insert(id, found);
    } else {
        g.delay_keys.remove(&id);
    }
}

/// `FIND_ENTITY_BY_TARGETNAME(pentStart, name)` (`PF_find_Shared`, ReHLDS `pr_cmds.cpp:930`,
/// read 2026-11-22): the first live entity after `after` (the start for `None`) whose
/// `targetname` is `name`, in ascending index order. QUIRKS: an entity with no `targetname` never
/// matches (not even for the name `""`); the world is never listed.
pub fn find_by_targetname(
    eng: &EngineApi<'_>,
    after: Option<EdictId>,
    name: &str,
) -> Option<EdictId> {
    if name.is_empty() {
        return None;
    }
    let first = after.map_or(1, |a| usize::from(a.index) + 1);
    eng.entity_ids()
        .into_iter()
        .filter(|id| usize::from(id.index) >= first)
        .find(|&id| eng.ent(id).is_some_and(|v| v.targetname == name))
}

/// `FireTargets(targetName, pActivator, pCaller, useType, value)` (`subs.cpp:122`, read
/// 2026-11-23): every entity named `target_name`, in index order, is used with `activator` and
/// `caller`, unless it is on its way out (`FL_KILLME`, "Don't use dying ents"). The search
/// carries on from each entity found, so what a `Use` creates or removes is seen as the source
/// sees it. An empty name finds nothing.
pub fn fire_targets(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    target_name: &str,
    activator: Option<EdictId>,
    caller: Option<EdictId>,
    use_type: UseType,
    value: f32,
) {
    let mut found = None;
    while let Some(target) = find_by_targetname(eng, found, target_name) {
        found = Some(target);
        // `pTarget && !(pTarget->pev->flags & FL_KILLME)`.
        if eng.ent(target).is_some_and(|v| v.flags & FL_KILLME == 0) {
            use_entity(g, eng, target, activator, caller, use_type, value);
        }
    }
}

/// `CBaseDelay::SUB_UseTargets(pActivator, useType, value)` of the entity `ent` (`subs.cpp:172`,
/// read 2026-11-23), which is also `CBaseEntity::SUB_UseTargets` for an entity with no `delay`
/// and no `killtarget` (that one only fires the targets).
///
/// With neither a `target` nor a `killtarget` nothing happens. With a `delay` a `DelayedUse`
/// entity is made that does the rest that many seconds later (`m_flDelay` is not kept on it, so
/// it cannot delay again; it remembers the use type, the `killtarget` and the `target`, and the
/// activator only if it is a player: a delayed use of a non-player activator has none). Without a
/// delay, the entities named by the `killtarget` are removed ([`util_remove`]) and then the ones
/// named by the `target` are used with `ent` as the caller ([`fire_targets`]).
pub fn use_targets(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    ent: EdictId,
    activator: Option<EdictId>,
    use_type: UseType,
    value: f32,
) {
    let Some(target) = eng.ent(ent).map(|v| v.target.clone()) else {
        return;
    };
    let keys = g.delay_keys.get(&ent).cloned().unwrap_or_default();
    // Exit immediately if we don't have a target or kill target.
    if target.is_empty() && keys.kill_target.is_none() {
        return;
    }
    // Check for a delay.
    if keys.delay != 0.0 {
        // Create a temp object to fire at a later time.
        let Some(temp) = eng.create_entity("DelayedUse") else {
            return;
        };
        attach(eng, temp, Box::new(DelayedUse { kill_target: keys.kill_target }));
        // `gpGlobals->time + m_flDelay` is a float sum.
        let due = f64::from(eng.time() as f32 + keys.delay);
        // "If a player activates, then save it."
        let owner = activator.filter(|&a| player_slot(g, eng, a).is_some());
        if let Some(v) = eng.ent_mut(temp) {
            v.nextthink = due;
            // The use type is cached (and stashed) in `pev->button`.
            v.button = use_type as i32;
            v.target = target;
            v.owner = owner;
        }
        return;
    }
    fire_now(g, eng, ent, keys.kill_target.as_deref(), &target, activator, use_type, value);
}

/// What `SUB_UseTargets` does once it has no delay to wait for: removes the `killtarget`s, then
/// fires the `target`s with `caller` as the caller.
#[allow(clippy::too_many_arguments)]
fn fire_now(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    caller: EdictId,
    kill_target: Option<&str>,
    target: &str,
    activator: Option<EdictId>,
    use_type: UseType,
    value: f32,
) {
    // Kill the killtargets.
    if let Some(name) = kill_target {
        let mut found = None;
        while let Some(victim) = find_by_targetname(eng, found, name) {
            found = Some(victim);
            util_remove(eng, victim);
        }
    }
    // Fire targets.
    if !target.is_empty() {
        fire_targets(g, eng, target, activator, Some(caller), use_type, value);
    }
}

/// The class `DelayedUse` (`CBaseDelay::DelayThink`): a temporary entity that uses its `target`
/// (and removes its `killtarget`) when its think is due, then goes.
struct DelayedUse {
    kill_target: Option<String>,
}

impl GameEntity for DelayedUse {
    fn classname(&self) -> &'static str {
        "DelayedUse"
    }

    /// `CBaseDelay::DelayThink` (`subs.cpp:259`, read 2026-11-23): the activator is the player
    /// saved in `pev->owner`, the use type is in `pev->button`, the value is 0; the entity is
    /// the caller. `REMOVE_ENTITY` frees it at once.
    fn think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some(v) = eng.ent(id) else {
            return;
        };
        let (target, activator, use_type) = (v.target.clone(), v.owner, UseType::from_i32(v.button));
        fire_now(g, eng, id, self.kill_target.as_deref(), &target, activator, use_type, 0.0);
        eng.remove_entity(id);
    }
}
