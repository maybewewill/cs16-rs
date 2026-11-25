//! Trigger brushes, relays, counters, autos, teleports, point markers, and `infodecal`
//! ported from ReGameDLL_CS (`triggers.cpp`, `effects.cpp`, `subs.cpp`).

use cs_engine::consts::{
    EF_NODRAW, FL_BASEVELOCITY, FL_KILLME, MOVETYPE_NONE, SOLID_NOT, SOLID_TRIGGER,
};
use cs_engine::{EdictId, EngineApi};
use cs_proto48::tempent;
use glam::Vec3;

use crate::GameState;
use crate::entity::{GameEntity, UseType};
use crate::menus::atoi;
use crate::player::{self, dmg, player_slot};
use crate::targets::{find_by_targetname, is_master_triggered, use_targets};
use crate::world::{atof, brush_model_number, set_movedir};

const SF_TRIGGER_ALLOWMONSTERS: u32 = 1 << 0;
const SF_TRIGGER_NOCLIENTS: u32 = 1 << 1;
const SF_TRIGGER_PUSHABLES: u32 = 1 << 2;

const SF_TRIGGER_HURT_TARGETONCE: u32 = 1 << 0;
const SF_TRIGGER_HURT_START_OFF: u32 = 1 << 1;
const SF_TRIGGER_HURT_NO_CLIENTS: u32 = 1 << 3;
const SF_TRIGGER_HURT_CLIENTONLYFIRE: u32 = 1 << 4;
const SF_TRIGGER_HURT_CLIENTONLYTOUCH: u32 = 1 << 5;

const SF_TRIGGER_PUSH_ONCE: u32 = 1 << 0;
const SF_TRIGGER_PUSH_START_OFF: u32 = 1 << 1;

const SF_RELAY_FIREONCE: u32 = 1 << 0;
const SF_COUNT_NOMESSAGE: u32 = 1 << 0;
const SF_AUTO_FIREONCE: u32 = 1 << 0;

fn key_str<'a>(keys: &'a [(String, String)], name: &str) -> Option<&'a str> {
    keys.iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn spawn_brush_trigger(eng: &mut EngineApi<'_>, id: EdictId) -> bool {
    let showtriggers = eng.cvar_f32("showtriggers") != 0.0;
    let has_coll = eng.has_world_collision();
    let Some(v) = eng.ent_mut(id) else {
        return false;
    };
    if brush_model_number(&v.model).is_none() && (v.maxs - v.mins).length_squared() == 0.0 && has_coll {
        return false;
    }
    v.solid = SOLID_TRIGGER;
    v.movetype = MOVETYPE_NONE;
    if !showtriggers {
        v.effects |= EF_NODRAW;
    }
    eng.link_entity(id, false);
    true
}

fn can_touch_trigger(g: &GameState, eng: &EngineApi<'_>, trigger_id: EdictId, other: EdictId) -> bool {
    let Some(tv) = eng.ent(trigger_id) else {
        return false;
    };
    let Some(ov) = eng.ent(other) else {
        return false;
    };
    if ov.flags & FL_KILLME != 0 {
        return false;
    }
    let sf = tv.spawnflags;
    let is_player = player_slot(g, eng, other).is_some();
    if is_player {
        return sf & SF_TRIGGER_NOCLIENTS == 0;
    }
    if ov.classname == "func_pushable" {
        return sf & SF_TRIGGER_PUSHABLES != 0;
    }
    sf & SF_TRIGGER_ALLOWMONSTERS != 0
}

/// Base `trigger` brush placeholder (`CBaseTrigger`).
#[derive(Default)]
pub(crate) struct TriggerBase {
    master: String,
}

impl GameEntity for TriggerBase {
    fn classname(&self) -> &'static str {
        "trigger"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        spawn_brush_trigger(eng, id)
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if can_touch_trigger(g, eng, id, other) && is_master_triggered(g, eng, &self.master, Some(other)) {
            use_targets(g, eng, id, Some(other), UseType::Toggle, 0.0);
        }
    }
}

/// `trigger_multiple` (`CTriggerMultiple`): fires targets repeatedly with configurable `wait` cooldown.
#[derive(Default)]
pub(crate) struct TriggerMultiple {
    master: String,
    wait: f32,
    locked_until: f64,
}

impl GameEntity for TriggerMultiple {
    fn classname(&self) -> &'static str {
        "trigger_multiple"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.wait = key_str(keys, "wait").map_or(0.2, atof);
        self.locked_until = 0.0;
        spawn_brush_trigger(eng, id)
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        self.activate(g, eng, id, Some(other));
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: UseType,
        _value: f32,
    ) {
        self.activate(g, eng, id, activator);
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.locked_until = 0.0;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_TRIGGER;
            v.nextthink = 0.0;
        }
        eng.link_entity(id, false);
    }
}

impl TriggerMultiple {
    fn activate(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, activator: Option<EdictId>) {
        if let Some(other) = activator
            && !can_touch_trigger(g, eng, id, other)
        {
            return;
        }
        let now = eng.time();
        if now < self.locked_until {
            return;
        }
        if !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        if self.wait >= 0.0 {
            self.locked_until = now + f64::from(self.wait.max(0.05));
        } else {
            if let Some(v) = eng.ent_mut(id) {
                v.solid = SOLID_NOT;
            }
            eng.link_entity(id, false);
        }
        use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
    }
}

/// `trigger_once` (`CTriggerOnce`): fires targets once and disables until round restart.
#[derive(Default)]
pub(crate) struct TriggerOnce {
    master: String,
    fired: bool,
}

impl GameEntity for TriggerOnce {
    fn classname(&self) -> &'static str {
        "trigger_once"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.fired = false;
        spawn_brush_trigger(eng, id)
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if can_touch_trigger(g, eng, id, other) {
            self.fire_once(g, eng, id, Some(other));
        }
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: UseType,
        _value: f32,
    ) {
        self.fire_once(g, eng, id, activator);
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.fired = false;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_TRIGGER;
        }
        eng.link_entity(id, false);
    }
}

impl TriggerOnce {
    fn fire_once(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, activator: Option<EdictId>) {
        if self.fired || !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        self.fired = true;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
        }
        eng.link_entity(id, false);
        use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
    }
}

/// `trigger_hurt` (`CTriggerHurt`): hurts touching entities (`dmg`, toggleable, reset on restart).
#[derive(Default)]
pub(crate) struct TriggerHurt {
    master: String,
    damage: f32,
    bits: u32,
    next_hurt: f64,
    active: bool,
    fired_target: bool,
}

impl GameEntity for TriggerHurt {
    fn classname(&self) -> &'static str {
        "trigger_hurt"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.damage = key_str(keys, "dmg").map_or(10.0, atof);
        self.bits = key_str(keys, "damagetype").map_or(dmg::GENERIC, |v| atoi(v) as u32);
        self.next_hurt = 0.0;
        self.fired_target = false;
        if !spawn_brush_trigger(eng, id) {
            return false;
        }
        let start_off = eng.ent(id).is_some_and(|v| v.spawnflags & SF_TRIGGER_HURT_START_OFF != 0);
        self.active = !start_off;
        if start_off {
            if let Some(v) = eng.ent_mut(id) {
                v.solid = SOLID_NOT;
            }
            eng.link_entity(id, false);
        }
        true
    }

    fn use_(
        &mut self,
        _g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        _value: f32,
    ) {
        self.active = match use_type {
            UseType::On => true,
            UseType::Off => false,
            _ => !self.active,
        };
        if let Some(v) = eng.ent_mut(id) {
            v.solid = if self.active { SOLID_TRIGGER } else { SOLID_NOT };
        }
        eng.link_entity(id, false);
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if !self.active {
            return;
        }
        let now = eng.time();
        if now < self.next_hurt {
            return;
        }
        let sf = eng.ent(id).map_or(0, |v| v.spawnflags);
        let is_player = player_slot(g, eng, other).is_some();
        if (sf & SF_TRIGGER_HURT_NO_CLIENTS != 0 && is_player)
            || (sf & SF_TRIGGER_HURT_CLIENTONLYTOUCH != 0 && !is_player)
        {
            return;
        }
        if !is_master_triggered(g, eng, &self.master, Some(other)) {
            return;
        }
        let half_damage = self.damage * 0.5;
        if let Some(slot) = player_slot(g, eng, other) {
            player::take_damage(g, eng, slot as u8, Some(id), Some(id), half_damage, self.bits);
        } else {
            crate::entity::take_damage_of(g, eng, other, Some(id), Some(id), half_damage, self.bits);
        }
        self.next_hurt = now + 0.5;
        if (sf & SF_TRIGGER_HURT_CLIENTONLYFIRE == 0 || is_player)
            && !(sf & SF_TRIGGER_HURT_TARGETONCE != 0 && self.fired_target)
        {
            self.fired_target = true;
            use_targets(g, eng, id, Some(other), UseType::Toggle, 0.0);
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.next_hurt = 0.0;
        self.fired_target = false;
        let start_off = eng.ent(id).is_some_and(|v| v.spawnflags & SF_TRIGGER_HURT_START_OFF != 0);
        self.active = !start_off;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = if self.active { SOLID_TRIGGER } else { SOLID_NOT };
        }
        eng.link_entity(id, false);
    }
}

/// `trigger_push` (`CTriggerPush`): accelerates touching entities along `movedir * speed`.
#[derive(Default)]
pub(crate) struct TriggerPush {
    master: String,
    speed: f32,
    active: bool,
}

impl GameEntity for TriggerPush {
    fn classname(&self) -> &'static str {
        "trigger_push"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.speed = key_str(keys, "speed").map_or(100.0, atof);
        if !spawn_brush_trigger(eng, id) {
            return false;
        }
        let start_off = if let Some(v) = eng.ent_mut(id) {
            set_movedir(v);
            v.spawnflags & SF_TRIGGER_PUSH_START_OFF != 0
        } else {
            false
        };
        self.active = !start_off;
        if start_off {
            if let Some(v) = eng.ent_mut(id) {
                v.solid = SOLID_NOT;
            }
            eng.link_entity(id, false);
        }
        true
    }

    fn use_(
        &mut self,
        _g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        _value: f32,
    ) {
        self.active = match use_type {
            UseType::On => true,
            UseType::Off => false,
            _ => !self.active,
        };
        if let Some(v) = eng.ent_mut(id) {
            v.solid = if self.active { SOLID_TRIGGER } else { SOLID_NOT };
        }
        eng.link_entity(id, false);
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if !self.active || !can_touch_trigger(g, eng, id, other) {
            return;
        }
        if !is_master_triggered(g, eng, &self.master, Some(other)) {
            return;
        }
        let Some((dir, sf)) = eng.ent(id).map(|v| (v.movedir, v.spawnflags)) else {
            return;
        };
        let push = dir * self.speed;
        crate::kz::on_player_push(g, eng, other);
        if sf & SF_TRIGGER_PUSH_ONCE != 0 {
            if let Some(ov) = eng.ent_mut(other) {
                ov.velocity += push;
            }
            self.active = false;
            if let Some(v) = eng.ent_mut(id) {
                v.solid = SOLID_NOT;
            }
            eng.link_entity(id, false);
        } else if let Some(ov) = eng.ent_mut(other) {
            if ov.flags & FL_BASEVELOCITY != 0 {
                ov.basevelocity += push;
            } else {
                ov.basevelocity = push;
            }
            ov.flags |= FL_BASEVELOCITY;
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let start_off = eng.ent(id).is_some_and(|v| v.spawnflags & SF_TRIGGER_PUSH_START_OFF != 0);
        self.active = !start_off;
        if let Some(v) = eng.ent_mut(id) {
            v.solid = if self.active { SOLID_TRIGGER } else { SOLID_NOT };
        }
        eng.link_entity(id, false);
    }
}

/// `trigger_teleport` (`CTriggerTeleport`): teleports touching entities to `target` (`info_teleport_destination`).
#[derive(Default)]
pub(crate) struct TriggerTeleport {
    master: String,
}

impl GameEntity for TriggerTeleport {
    fn classname(&self) -> &'static str {
        "trigger_teleport"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        spawn_brush_trigger(eng, id)
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if !can_touch_trigger(g, eng, id, other) || !is_master_triggered(g, eng, &self.master, Some(other)) {
            return;
        }
        let target = eng.ent(id).map(|v| v.target.clone()).unwrap_or_default();
        let Some(dest) = find_by_targetname(eng, None, &target) else {
            return;
        };
        let Some((dest_origin, dest_angles)) = eng.ent(dest).map(|v| (v.origin, v.angles)) else {
            return;
        };
        let keep_vel = eng.cvar_f32("sv_kz_keep_teleport_vel") != 0.0;
        crate::kz::on_player_teleport(g, eng, other);
        eng.set_origin(other, dest_origin + Vec3::new(0.0, 0.0, 1.0));
        if let Some(ov) = eng.ent_mut(other) {
            if !keep_vel {
                ov.velocity = Vec3::ZERO;
                ov.basevelocity = Vec3::ZERO;
            }
            ov.angles = dest_angles;
            ov.v_angle = dest_angles;
            ov.fixangle = 1;
        }
    }
}

/// `trigger_gravity` (`CTriggerGravity`): applies configured `gravity` multiplier to touching players.
#[derive(Default)]
pub(crate) struct TriggerGravity {
    master: String,
    gravity: f32,
}

impl GameEntity for TriggerGravity {
    fn classname(&self) -> &'static str {
        "trigger_gravity"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.gravity = key_str(keys, "gravity").map_or(1.0, atof);
        spawn_brush_trigger(eng, id)
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if !can_touch_trigger(g, eng, id, other) || !is_master_triggered(g, eng, &self.master, Some(other)) {
            return;
        }
        if let Some(ov) = eng.ent_mut(other) {
            ov.gravity = self.gravity;
        }
    }
}

/// `trigger_relay` (`CTriggerRelay`): translates incoming `use_` into a configured `UseType`.
#[derive(Default)]
pub(crate) struct TriggerRelay {
    trigger_type: UseType,
    fired: bool,
}

impl GameEntity for TriggerRelay {
    fn classname(&self) -> &'static str {
        "trigger_relay"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId, keys: &[(String, String)]) -> bool {
        self.trigger_type = match key_str(keys, "triggerstate").map_or(0, atoi) {
            1 => UseType::On,
            2 => UseType::Toggle,
            _ => UseType::Off,
        };
        self.fired = false;
        true
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: UseType,
        _value: f32,
    ) {
        if self.fired {
            return;
        }
        if eng.ent(id).is_some_and(|v| v.spawnflags & SF_RELAY_FIREONCE != 0) {
            self.fired = true;
        }
        use_targets(g, eng, id, activator, self.trigger_type, 0.0);
    }

    fn restart(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        self.fired = false;
    }
}

/// `trigger_counter` (`CTriggerCounter`): decrements `count` on each `use_`, firing `target` at zero.
#[derive(Default)]
pub(crate) struct TriggerCounter {
    initial_count: i32,
    remaining: i32,
    master: String,
}

impl GameEntity for TriggerCounter {
    fn classname(&self) -> &'static str {
        "trigger_counter"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId, keys: &[(String, String)]) -> bool {
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        let count = key_str(keys, "count").map_or(2, atoi).max(1);
        self.initial_count = count;
        self.remaining = count;
        true
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: UseType,
        _value: f32,
    ) {
        if self.remaining <= 0 || !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        self.remaining -= 1;
        let sf = eng.ent(id).map_or(0, |v| v.spawnflags);
        if self.remaining > 0 {
            if sf & SF_COUNT_NOMESSAGE == 0
                && let Some(slot) = activator.and_then(|a| player_slot(g, eng, a))
            {
                eng.center_print(slot as u8, &format!("{} more to go...", self.remaining));
            }
            return;
        }
        if sf & SF_COUNT_NOMESSAGE == 0
            && let Some(slot) = activator.and_then(|a| player_slot(g, eng, a))
        {
            eng.center_print(slot as u8, "Sequence completed!");
        }
        use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
    }

    fn restart(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        self.remaining = self.initial_count;
    }
}

/// `trigger_changetarget` (`CTriggerChangeTarget`): rewrites `v.target` on entities matching `target`.
#[derive(Default)]
pub(crate) struct TriggerChangeTarget {
    new_target: String,
    initial_targets: Vec<(EdictId, String)>,
}

impl GameEntity for TriggerChangeTarget {
    fn classname(&self) -> &'static str {
        "trigger_changetarget"
    }

    fn spawn(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId, keys: &[(String, String)]) -> bool {
        self.new_target = key_str(keys, "m_iszNewTarget").unwrap_or("").to_string();
        self.initial_targets.clear();
        true
    }

    fn use_(
        &mut self,
        _g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: UseType,
        _value: f32,
    ) {
        let Some(target) = eng.ent(id).map(|v| v.target.clone()) else {
            return;
        };
        let mut after = None;
        while let Some(e) = find_by_targetname(eng, after, &target) {
            after = Some(e);
            if let Some(ev) = eng.ent_mut(e) {
                if !self.initial_targets.iter().any(|(saved, _)| *saved == e) {
                    self.initial_targets.push((e, ev.target.clone()));
                }
                ev.target.clone_from(&self.new_target);
            }
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, _id: EdictId) {
        for (e, old_target) in self.initial_targets.drain(..) {
            if let Some(ev) = eng.ent_mut(e) {
                ev.target = old_target;
            }
        }
    }
}

/// `trigger_auto` (`CAutoTrigger`): fires automatically right after map load/round restart.
#[derive(Default)]
pub(crate) struct TriggerAuto {
    globalstate: String,
    trigger_type: UseType,
    fired: bool,
}

impl GameEntity for TriggerAuto {
    fn classname(&self) -> &'static str {
        "trigger_auto"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.globalstate = key_str(keys, "globalstate").unwrap_or("").to_string();
        self.trigger_type = match key_str(keys, "triggerstate").map_or(2, atoi) {
            0 => UseType::Off,
            1 => UseType::On,
            _ => UseType::Toggle,
        };
        self.fired = false;
        let due = eng.time() + 0.1;
        if let Some(v) = eng.ent_mut(id) {
            v.nextthink = due;
        }
        true
    }

    fn think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if self.fired {
            return;
        }
        if !self.globalstate.is_empty() && g.global_states.get(&self.globalstate).copied().unwrap_or(0) != 1 {
            let due = eng.time() + 0.1;
            if let Some(v) = eng.ent_mut(id) {
                v.nextthink = due;
            }
            return;
        }
        if eng.ent(id).is_some_and(|v| v.spawnflags & SF_AUTO_FIREONCE != 0) {
            self.fired = true;
        }
        use_targets(g, eng, id, None, self.trigger_type, 0.0);
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if eng.ent(id).is_some_and(|v| v.spawnflags & SF_AUTO_FIREONCE == 0) {
            self.fired = false;
            let due = eng.time() + 0.1;
            if let Some(v) = eng.ent_mut(id) {
                v.nextthink = due;
            }
        }
    }
}

/// `trigger_cdaudio` (`CTriggerCDAudio`) and `target_cdaudio` (`CTargetCDAudio`): plays a CD audio track.
#[derive(Default)]
pub(crate) struct TriggerCdAudio {
    class: &'static str,
    track: i32,
    played: bool,
}

impl TriggerCdAudio {
    pub(crate) fn new(class: &'static str) -> Self {
        Self { class, track: -1, played: false }
    }

    fn play(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, activator: Option<EdictId>) {
        if self.played {
            return;
        }
        self.played = true;
        let cmd = if self.track < 0 {
            "cd stop\n".to_string()
        } else {
            format!("cd play {}\n", self.track)
        };
        if let Some(slot) = activator.and_then(|a| player_slot(g, eng, a)) {
            eng.stufftext(slot as u8, &cmd);
        } else {
            for slot in 0..eng.max_clients() {
                if g.players[usize::from(slot)].is_some() {
                    eng.stufftext(slot, &cmd);
                }
            }
        }
        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
        }
    }
}

impl GameEntity for TriggerCdAudio {
    fn classname(&self) -> &'static str {
        self.class
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.track = key_str(keys, "health").map_or(-1, atoi);
        self.played = false;
        if self.class == "trigger_cdaudio" {
            spawn_brush_trigger(eng, id)
        } else {
            true
        }
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if self.class == "trigger_cdaudio" && player_slot(g, eng, other).is_some() {
            self.play(g, eng, id, Some(other));
        }
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: UseType,
        _value: f32,
    ) {
        self.play(g, eng, id, activator);
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.played = false;
        if self.class == "trigger_cdaudio"
            && let Some(v) = eng.ent_mut(id)
        {
            v.solid = SOLID_TRIGGER;
        }
    }
}

/// `info_teleport_destination` (`CPointEntity`): destination marker for `trigger_teleport`.
pub(crate) struct InfoTeleportDestination;
impl GameEntity for InfoTeleportDestination {
    fn classname(&self) -> &'static str {
        "info_teleport_destination"
    }
}

/// `info_intermission` (`CInfoIntermission`): intermission camera spot.
pub(crate) struct InfoIntermission;
impl GameEntity for InfoIntermission {
    fn classname(&self) -> &'static str {
        "info_intermission"
    }
}

/// `info_null` (`CNullEntity`): removes itself immediately at spawn time (baked compile helper).
pub(crate) struct InfoNull;
impl GameEntity for InfoNull {
    fn classname(&self) -> &'static str {
        "info_null"
    }
    fn spawn(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId, _keys: &[(String, String)]) -> bool {
        false
    }
}

/// `infodecal` (`CDecal`): emits static world decal (`TE_BSPDECAL` / `TE_WORLDDECAL`) on spawn or use.
#[derive(Default)]
pub(crate) struct InfoDecal {
    texture: String,
    applied: bool,
}

impl InfoDecal {
    fn apply(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        let Some(v) = eng.ent(id) else {
            return;
        };
        let pos = v.origin;
        let idx = eng.decal_index(&self.texture);
        if idx < 0 {
            return;
        }
        self.applied = true;
        eng.send_temp_entity(
            cs_engine::MsgDest::Init,
            &tempent::bsp_decal(pos, idx as u16, 0, None),
        );
    }
}

impl GameEntity for InfoDecal {
    fn classname(&self) -> &'static str {
        "infodecal"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.texture = key_str(keys, "texture").unwrap_or("").to_string();
        self.applied = false;
        let unnamed = eng.ent(id).is_some_and(|v| v.targetname.is_empty());
        if unnamed {
            self.apply(eng, id);
        }
        true
    }

    fn use_(
        &mut self,
        _g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: UseType,
        _value: f32,
    ) {
        if !self.applied {
            self.apply(eng, id);
        }
    }
}
