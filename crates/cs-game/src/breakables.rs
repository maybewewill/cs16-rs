//! Breakables and utility brush entities ported from ReGameDLL_CS
//! (`func_break.cpp`, `bmodels.cpp`, `triggers.cpp`):
//! `func_breakable`, `func_wall_toggle`, `func_friction`, `func_rotating`,
//! `func_pendulum`, `func_grencatch`, `func_weaponcheck`, and `func_guntarget`.

use std::f32::consts::TAU;

use cs_engine::consts::{
    FL_WORLDBRUSH, MOVETYPE_NONE, MOVETYPE_PUSH, SOLID_BBOX, SOLID_BSP, SOLID_NOT, SOLID_TRIGGER,
};
use cs_engine::{EdictId, EngineApi, MsgDest};
use cs_proto48::tempent::{self, explflag};
use glam::Vec3;

use crate::GameState;
use crate::entity::{GameEntity, UseType};
use crate::grenade::radius_damage;
use crate::menus::atoi;
use crate::player::{self, EF_NODRAW, dmg, player_slot};
use crate::targets::{find_by_targetname, use_targets};
use crate::weapons::item_info;
use crate::world::{atof, breakable_precache, brush_model_number};

const SF_BREAK_TRIGGER_ONLY: u32 = 1 << 0;
const SF_BREAK_TOUCH: u32 = 1 << 1;
const SF_BREAK_PRESSURE: u32 = 1 << 2;

const SF_WALL_START_OFF: u32 = 1 << 0;

const SF_ROTATING_START_ON: u32 = 1 << 0;
const SF_ROTATING_BACKWARDS: u32 = 1 << 1;
const SF_ROTATING_Z_AXIS: u32 = 1 << 2;
const SF_ROTATING_X_AXIS: u32 = 1 << 3;
const SF_ROTATING_NOTSOLID: u32 = 1 << 6;

const SF_PENDULUM_START_ON: u32 = 1 << 0;
const SF_PENDULUM_PASSABLE: u32 = 1 << 3;
const SF_PENDULUM_AUTO_RETURN: u32 = 1 << 4;

const SF_GUNTARGET_START_ON: u32 = 1 << 0;

const MAT_UNBREAKABLE_GLASS: i32 = 7;

fn key_str<'a>(keys: &'a [(String, String)], name: &str) -> Option<&'a str> {
    keys.iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn brush_solid_or_bbox(eng: &EngineApi<'_>, id: EdictId, not_solid: bool) -> Option<u8> {
    let v = eng.ent(id)?;
    if brush_model_number(&v.model).is_some() {
        Some(if not_solid { SOLID_NOT } else { SOLID_BSP })
    } else if !eng.has_world_collision() || (v.maxs - v.mins).length_squared() > 0.0 {
        Some(if not_solid { SOLID_NOT } else { SOLID_BBOX })
    } else {
        None
    }
}

/// Breakable map brush (`func_breakable`, `CBreakable`). Hidden on break and restored on
/// round restart so subsequent rounds rebuild broken glass/vents/crates cleanly.
pub(crate) struct FuncBreakable {
    spawn_health: f32,
    spawn_solid: u8,
    spawn_takedamage: bool,
    gib_model: String,
    explode_magnitude: f32,
    broken: bool,
}

impl Default for FuncBreakable {
    fn default() -> Self {
        Self {
            spawn_health: 1.0,
            spawn_solid: SOLID_BSP,
            spawn_takedamage: true,
            gib_model: String::new(),
            explode_magnitude: 0.0,
            broken: false,
        }
    }
}

impl FuncBreakable {
    fn break_now(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        attacker: Option<EdictId>,
    ) {
        if self.broken {
            return;
        }
        self.broken = true;
        let (center, size) = eng
            .ent(id)
            .map_or((Vec3::ZERO, Vec3::ZERO), |v| ((v.absmin + v.absmax) * 0.5, v.size));
        let gib_idx = eng.model_index(&self.gib_model).unwrap_or(0);
        let allow_gibs = !self.gib_model.to_ascii_lowercase().contains("flesh")
            || crate::bullets::show_gibs(eng, true);
        if allow_gibs {
            let te = tempent::break_model(
                center,
                size,
                Vec3::new(0.0, 0.0, 80.0),
                10,
                gib_idx,
                10,
                25,
                0,
            );
            eng.send_temp_entity(MsgDest::Pvs(center), &te);
        }

        if let Some(v) = eng.ent_mut(id) {
            v.solid = SOLID_NOT;
            v.effects |= EF_NODRAW;
            v.health = 0.0;
        }
        eng.link_entity(id, false);

        if self.explode_magnitude > 0.0 {
            let fire = eng.model_index("sprites/zerogxplode.spr").unwrap_or(0);
            let exp = tempent::explosion(center, fire, 25, 30, explflag::NONE);
            eng.send_temp_entity(MsgDest::Pvs(center), &exp);
            radius_damage(
                g,
                eng,
                center,
                Some(id),
                None,
                attacker,
                self.explode_magnitude,
                self.explode_magnitude * 2.5,
                dmg::BLAST,
            );
        }

        use_targets(g, eng, id, attacker, UseType::Toggle, 0.0);
    }
}

impl GameEntity for FuncBreakable {
    fn classname(&self) -> &'static str {
        "func_breakable"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn takes_damage(&self) -> bool {
        !self.broken && self.spawn_takedamage
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let (material, gib_model) = breakable_precache(eng, keys);
        self.gib_model = gib_model.unwrap_or_default();
        self.explode_magnitude = key_str(keys, "explodemagnitude").map(atof).unwrap_or(0.0);

        let Some(solid) = brush_solid_or_bbox(eng, id, false) else {
            return false;
        };
        self.spawn_solid = solid;

        let parsed_health = key_str(keys, "health").map(atof);
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        let trigger_only = v.spawnflags & SF_BREAK_TRIGGER_ONLY != 0;
        self.spawn_takedamage = !trigger_only && material != MAT_UNBREAKABLE_GLASS;

        if let Some(h) = parsed_health {
            v.health = h;
        }
        if v.health <= 0.0 && self.spawn_takedamage {
            v.health = 1.0;
        }
        self.spawn_health = v.health;
        v.solid = solid;
        v.movetype = MOVETYPE_PUSH;
        v.velocity = Vec3::ZERO;
        v.effects &= !EF_NODRAW;
        self.broken = false;
        eng.link_entity(id, false);
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
        self.break_now(g, eng, id, activator);
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if self.broken || !self.spawn_takedamage || player_slot(g, eng, other).is_none() {
            return;
        }
        let Some((flags, self_max_z)) = eng.ent(id).map(|v| (v.spawnflags, v.absmax.z)) else {
            return;
        };
        let touch_break = flags & SF_BREAK_TOUCH != 0;
        let pressure_break = flags & SF_BREAK_PRESSURE != 0
            && eng.ent(other).is_some_and(|o| o.absmin.z >= self_max_z - 2.0);
        if touch_break || pressure_break {
            self.break_now(g, eng, id, Some(other));
        }
    }

    fn take_damage(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _inflictor: Option<EdictId>,
        attacker: Option<EdictId>,
        damage: f32,
        _bits: u32,
    ) -> bool {
        if !self.takes_damage() {
            return false;
        }
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.health -= damage;
        if v.health <= 0.0 {
            self.break_now(g, eng, id, attacker);
            return false;
        }
        true
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.broken = false;
        let (health, solid) = (self.spawn_health, self.spawn_solid);
        if let Some(v) = eng.ent_mut(id) {
            v.health = health;
            v.solid = solid;
            v.effects &= !EF_NODRAW;
        }
        eng.link_entity(id, false);
    }
}

/// Toggleable solid wall (`func_wall_toggle`, `CFuncWallToggle`).
pub(crate) struct FuncWallToggle {
    start_off: bool,
    on: bool,
    base_solid: u8,
}

impl Default for FuncWallToggle {
    fn default() -> Self {
        Self {
            start_off: false,
            on: true,
            base_solid: SOLID_BSP,
        }
    }
}

impl FuncWallToggle {
    fn apply_state(&self, eng: &mut EngineApi<'_>, id: EdictId) {
        let on = self.on;
        let base_solid = self.base_solid;
        if let Some(v) = eng.ent_mut(id) {
            if on {
                v.solid = base_solid;
                v.effects &= !EF_NODRAW;
            } else {
                v.solid = SOLID_NOT;
                v.effects |= EF_NODRAW;
            }
        }
        eng.link_entity(id, false);
    }
}

impl GameEntity for FuncWallToggle {
    fn classname(&self) -> &'static str {
        "func_wall_toggle"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        let Some(solid) = brush_solid_or_bbox(eng, id, false) else {
            return false;
        };
        self.base_solid = solid;
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.movetype = MOVETYPE_PUSH;
        v.angles = Vec3::ZERO;
        v.flags |= FL_WORLDBRUSH;
        self.start_off = v.spawnflags & SF_WALL_START_OFF != 0;
        self.on = !self.start_off;
        self.apply_state(eng, id);
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
        self.on = match use_type {
            UseType::On => true,
            UseType::Off => false,
            UseType::Toggle | UseType::Set => !self.on,
        };
        self.apply_state(eng, id);
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.on = !self.start_off;
        self.apply_state(eng, id);
    }
}

/// Low/high friction floor trigger brush (`func_friction`, `CFrictionModifier`).
pub(crate) struct FuncFriction {
    friction_fraction: f32,
}

impl Default for FuncFriction {
    fn default() -> Self {
        Self {
            friction_fraction: 0.15,
        }
    }
}

impl GameEntity for FuncFriction {
    fn classname(&self) -> &'static str {
        "func_friction"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let modifier = key_str(keys, "modifier").map(atof).unwrap_or(15.0);
        self.friction_fraction = (modifier / 100.0).clamp(0.0, 2.0);
        let has_coll = eng.has_world_collision();
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        if brush_model_number(&v.model).is_none()
            && has_coll
            && (v.maxs - v.mins).length_squared() <= 0.0
        {
            return false;
        }
        v.solid = SOLID_TRIGGER;
        v.movetype = MOVETYPE_NONE;
        v.effects |= EF_NODRAW;
        v.friction = self.friction_fraction;
        eng.link_entity(id, false);
        true
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, _id: EdictId, other: EdictId) {
        if player_slot(g, eng, other).is_some()
            && let Some(v) = eng.ent_mut(other)
        {
            v.friction = self.friction_fraction;
        }
    }
}

/// Continuously rotating brush (`func_rotating`, `CFuncRotating`).
pub(crate) struct FuncRotating {
    speed: f32,
    damage: f32,
    axis: Vec3,
    start_on: bool,
    active: bool,
    spawn_angles: Vec3,
}

impl Default for FuncRotating {
    fn default() -> Self {
        Self {
            speed: 100.0,
            damage: 0.0,
            axis: Vec3::Y,
            start_on: false,
            active: false,
            spawn_angles: Vec3::ZERO,
        }
    }
}

impl GameEntity for FuncRotating {
    fn classname(&self) -> &'static str {
        "func_rotating"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let not_solid = eng
            .ent(id)
            .is_some_and(|v| v.spawnflags & SF_ROTATING_NOTSOLID != 0);
        let Some(solid) = brush_solid_or_bbox(eng, id, not_solid) else {
            return false;
        };
        self.speed = key_str(keys, "speed").map(atof).unwrap_or(100.0);
        self.damage = key_str(keys, "dmg").map(atof).unwrap_or(0.0);
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        let sign = if v.spawnflags & SF_ROTATING_BACKWARDS != 0 {
            -1.0
        } else {
            1.0
        };
        self.axis = if v.spawnflags & SF_ROTATING_X_AXIS != 0 {
            Vec3::Z
        } else if v.spawnflags & SF_ROTATING_Z_AXIS != 0 {
            Vec3::X
        } else {
            Vec3::Y
        } * sign;
        self.spawn_angles = v.angles;
        self.start_on = v.spawnflags & SF_ROTATING_START_ON != 0;
        self.active = self.start_on;
        v.solid = solid;
        v.movetype = MOVETYPE_PUSH;
        v.avelocity = if self.active {
            self.axis * self.speed
        } else {
            Vec3::ZERO
        };
        eng.link_entity(id, false);
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
            UseType::Toggle | UseType::Set => !self.active,
        };
        let avel = if self.active {
            self.axis * self.speed
        } else {
            Vec3::ZERO
        };
        if let Some(v) = eng.ent_mut(id) {
            v.avelocity = avel;
        }
    }

    fn blocked(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if self.damage > 0.0
            && let Some(slot) = player_slot(g, eng, other)
        {
            player::take_damage(g, eng, slot as u8, Some(id), Some(id), self.damage, dmg::CRUSH);
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.active = self.start_on;
        let avel = if self.active {
            self.axis * self.speed
        } else {
            Vec3::ZERO
        };
        let angles = self.spawn_angles;
        if let Some(v) = eng.ent_mut(id) {
            v.angles = angles;
            v.avelocity = avel;
        }
    }
}

/// Swinging pendulum brush (`func_pendulum`, `CPendulum`).
pub(crate) struct FuncPendulum {
    start_angles: Vec3,
    axis: Vec3,
    distance: f32,
    speed: f32,
    damage: f32,
    phase: f32,
    start_on: bool,
    auto_return: bool,
    active: bool,
}

impl Default for FuncPendulum {
    fn default() -> Self {
        Self {
            start_angles: Vec3::ZERO,
            axis: Vec3::X,
            distance: 30.0,
            speed: 100.0,
            damage: 0.0,
            phase: 0.0,
            start_on: false,
            auto_return: false,
            active: false,
        }
    }
}

impl GameEntity for FuncPendulum {
    fn classname(&self) -> &'static str {
        "func_pendulum"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let passable = eng
            .ent(id)
            .is_some_and(|v| v.spawnflags & SF_PENDULUM_PASSABLE != 0);
        let Some(solid) = brush_solid_or_bbox(eng, id, passable) else {
            return false;
        };
        self.distance = key_str(keys, "distance").map(atof).unwrap_or(30.0);
        let speed = key_str(keys, "speed").map(atof).unwrap_or(100.0);
        self.speed = if speed > 0.0 { speed } else { 100.0 };
        self.damage = key_str(keys, "dmg").map(atof).unwrap_or(0.0);
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.solid = solid;
        v.movetype = MOVETYPE_PUSH;
        self.start_angles = v.angles;
        self.start_on = v.spawnflags & SF_PENDULUM_START_ON != 0;
        self.auto_return = v.spawnflags & SF_PENDULUM_AUTO_RETURN != 0;
        self.active = self.start_on;
        self.phase = 0.0;
        if self.active {
            v.nextthink = v.ltime + 0.05;
        }
        eng.link_entity(id, false);
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
            UseType::Toggle | UseType::Set => !self.active,
        };
        if let Some(v) = eng.ent_mut(id) {
            if self.active {
                v.nextthink = v.ltime + 0.05;
            } else {
                v.avelocity = Vec3::ZERO;
                if self.auto_return {
                    v.angles = self.start_angles;
                }
                v.nextthink = 0.0;
            }
        }
    }

    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if !self.active {
            return;
        }
        let dt = eng.frametime().max(0.05);
        self.phase += dt * (self.speed / self.distance.abs().max(1.0));
        let angle = self.start_angles + self.axis * (self.distance * (self.phase * TAU * 0.25).sin());
        if let Some(v) = eng.ent_mut(id) {
            v.angles = angle;
            v.nextthink = v.ltime + 0.05;
        }
    }

    fn blocked(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if self.damage > 0.0
            && let Some(slot) = player_slot(g, eng, other)
        {
            player::take_damage(g, eng, slot as u8, Some(id), Some(id), self.damage, dmg::CRUSH);
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.active = self.start_on;
        self.phase = 0.0;
        let reset = self.start_angles;
        if let Some(v) = eng.ent_mut(id) {
            v.angles = reset;
            v.avelocity = Vec3::ZERO;
            v.nextthink = if self.active { v.ltime + 0.05 } else { 0.0 };
        }
    }
}

/// Grenade-catching trigger zone (`func_grencatch`, `CFuncGrenCatch`).
#[derive(Default)]
pub(crate) struct FuncGrenCatch {
    grenade_type: String,
    trigger_on_grenade: String,
    caught: bool,
}

impl GameEntity for FuncGrenCatch {
    fn classname(&self) -> &'static str {
        "func_grencatch"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.grenade_type = key_str(keys, "grenadetype")
            .unwrap_or("smoke")
            .to_ascii_lowercase();
        self.trigger_on_grenade = key_str(keys, "triggerongrenade").unwrap_or("").to_string();
        self.caught = false;
        let has_coll = eng.has_world_collision();
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        if brush_model_number(&v.model).is_none()
            && has_coll
            && (v.maxs - v.mins).length_squared() <= 0.0
        {
            return false;
        }
        v.solid = SOLID_TRIGGER;
        v.movetype = MOVETYPE_NONE;
        v.effects |= EF_NODRAW;
        eng.link_entity(id, false);
        true
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if self.caught {
            return;
        }
        let Some((class, model)) = eng
            .ent(other)
            .map(|v| (v.classname.clone(), v.model.to_ascii_lowercase()))
        else {
            return;
        };
        if class != "grenade" {
            return;
        }
        let matches_type = match self.grenade_type.as_str() {
            "smoke" | "smokegrenade" => model.contains("smokegrenade"),
            "flash" | "flashbang" => model.contains("flashbang"),
            "he" | "hegrenade" => model.contains("hegrenade"),
            _ => true,
        };
        if !matches_type {
            return;
        }
        self.caught = true;
        if !self.trigger_on_grenade.is_empty() {
            let target = self.trigger_on_grenade.clone();
            crate::targets::fire_targets(g, eng, &target, Some(other), Some(id), UseType::Toggle, 0.0);
        } else {
            use_targets(g, eng, id, Some(other), UseType::Toggle, 0.0);
        }
    }

    fn restart(&mut self, _g: &mut GameState, _eng: &mut EngineApi<'_>, _id: EdictId) {
        self.caught = false;
    }
}

/// Player inventory checking trigger (`func_weaponcheck`, `CFuncWeaponCheck`).
#[derive(Default)]
pub(crate) struct FuncWeaponCheck {
    items: Vec<String>,
    required_count: usize,
    any_items: bool,
    alt_target: String,
}

impl GameEntity for FuncWeaponCheck {
    fn classname(&self) -> &'static str {
        "func_weaponcheck"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.items.clear();
        for i in 1..=16 {
            if let Some(val) = key_str(keys, &format!("item{i}"))
                && !val.trim().is_empty()
            {
                self.items.push(val.trim().to_string());
            }
        }
        self.required_count = atoi(key_str(keys, "trigger_items_count").unwrap_or("0")).max(0) as usize;
        self.any_items = atoi(key_str(keys, "any_items").unwrap_or("0")) != 0;
        self.alt_target = key_str(keys, "netname").unwrap_or("").to_string();

        let has_coll = eng.has_world_collision();
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        if brush_model_number(&v.model).is_none()
            && has_coll
            && (v.maxs - v.mins).length_squared() <= 0.0
        {
            return false;
        }
        v.solid = SOLID_TRIGGER;
        v.movetype = MOVETYPE_NONE;
        v.effects |= EF_NODRAW;
        eng.link_entity(id, false);
        true
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if player_slot(g, eng, other).is_some() {
            self.use_(g, eng, id, Some(other), Some(other), UseType::Toggle, 0.0);
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
        let Some(act) = activator else {
            return;
        };
        let Some(slot) = player_slot(g, eng, act) else {
            return;
        };
        let Some(p) = g.players[slot].as_ref() else {
            return;
        };
        let matched = self
            .items
            .iter()
            .filter(|name| {
                if name.eq_ignore_ascii_case("item_kevlar")
                    || name.eq_ignore_ascii_case("item_assaultsuit")
                {
                    return p.armor > 0.0;
                }
                if name.eq_ignore_ascii_case("item_thighpack") {
                    return p.has_defuser;
                }
                p.items.iter().flatten().any(|slot_item| {
                    item_info(slot_item.weapon.id)
                        .is_some_and(|info| info.name.eq_ignore_ascii_case(name))
                })
            })
            .count();

        let pass = if self.items.is_empty() {
            true
        } else if self.any_items {
            matched >= 1
        } else if self.required_count > 0 {
            matched >= self.required_count
        } else {
            matched >= self.items.len()
        };

        if pass {
            use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
        } else if !self.alt_target.is_empty() {
            crate::targets::fire_targets(
                g,
                eng,
                &self.alt_target,
                activator,
                Some(id),
                UseType::Toggle,
                0.0,
            );
        }
    }
}

/// Moving shooting-range target (`func_guntarget`, `CGunTarget`).
pub(crate) struct FuncGunTarget {
    speed: f32,
    spawn_health: f32,
    spawn_origin: Vec3,
    initial_target: String,
    current_target: String,
    message_target: String,
    active: bool,
    disabled: bool,
    dest: Vec3,
}

impl Default for FuncGunTarget {
    fn default() -> Self {
        Self {
            speed: 100.0,
            spawn_health: 50.0,
            spawn_origin: Vec3::ZERO,
            initial_target: String::new(),
            current_target: String::new(),
            message_target: String::new(),
            active: false,
            disabled: false,
            dest: Vec3::ZERO,
        }
    }
}

impl FuncGunTarget {
    fn move_to_next(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        if !self.active || self.current_target.is_empty() {
            if let Some(v) = eng.ent_mut(id) {
                v.velocity = Vec3::ZERO;
            }
            return;
        }
        let Some(corner) = find_by_targetname(eng, None, &self.current_target) else {
            return;
        };
        let Some((c_org, next)) = eng.ent(corner).map(|v| (v.origin, v.target.clone())) else {
            return;
        };
        let offset = eng.ent(id).map_or(Vec3::ZERO, |v| (v.mins + v.maxs) * 0.5);
        self.dest = c_org - offset;
        self.current_target = next;
        let dest = self.dest;
        let speed = self.speed;
        if let Some(v) = eng.ent_mut(id) {
            let delta = dest - v.origin;
            let dist = delta.length();
            if dist < 0.05 || speed <= 0.0 {
                v.origin = dest;
                v.velocity = Vec3::ZERO;
                v.nextthink = v.ltime + 0.05;
            } else {
                v.velocity = delta / dist * speed;
                v.nextthink = v.ltime + f64::from(dist / speed);
            }
        }
    }
}

impl GameEntity for FuncGunTarget {
    fn classname(&self) -> &'static str {
        "func_guntarget"
    }

    fn takes_damage(&self) -> bool {
        !self.disabled
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let Some(solid) = brush_solid_or_bbox(eng, id, false) else {
            return false;
        };
        let speed = key_str(keys, "speed").map(atof).unwrap_or(100.0);
        let health = key_str(keys, "health").map(atof).unwrap_or(50.0);
        self.speed = if speed > 0.0 { speed } else { 100.0 };
        self.spawn_health = if health > 0.0 { health } else { 50.0 };
        self.initial_target = key_str(keys, "target").unwrap_or("").to_string();
        self.current_target = self.initial_target.clone();
        self.message_target = key_str(keys, "message").unwrap_or("").to_string();

        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        self.spawn_origin = v.origin;
        v.solid = solid;
        v.movetype = MOVETYPE_PUSH;
        v.health = self.spawn_health;
        self.disabled = false;
        self.active = v.spawnflags & SF_GUNTARGET_START_ON != 0;
        eng.link_entity(id, false);
        if self.active {
            self.move_to_next(eng, id);
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
            UseType::Toggle | UseType::Set => !self.active,
        };
        if self.active {
            self.disabled = false;
            let h = self.spawn_health;
            if let Some(v) = eng.ent_mut(id) {
                v.health = h;
            }
            self.move_to_next(eng, id);
        } else if let Some(v) = eng.ent_mut(id) {
            v.velocity = Vec3::ZERO;
            v.nextthink = 0.0;
        }
    }

    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if !self.active {
            return;
        }
        eng.set_origin(id, self.dest);
        self.move_to_next(eng, id);
    }

    fn take_damage(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _inflictor: Option<EdictId>,
        attacker: Option<EdictId>,
        damage: f32,
        _bits: u32,
    ) -> bool {
        if self.disabled {
            return false;
        }
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.health -= damage;
        if v.health <= 0.0 {
            v.health = 0.0;
            v.velocity = Vec3::ZERO;
            v.nextthink = 0.0;
            self.active = false;
            self.disabled = true;
            if !self.message_target.is_empty() {
                let target = self.message_target.clone();
                crate::targets::fire_targets(g, eng, &target, attacker, Some(id), UseType::Toggle, 0.0);
            }
            return false;
        }
        true
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.current_target = self.initial_target.clone();
        self.disabled = false;
        self.active = eng
            .ent(id)
            .is_some_and(|v| v.spawnflags & SF_GUNTARGET_START_ON != 0);
        let (origin, health) = (self.spawn_origin, self.spawn_health);
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = Vec3::ZERO;
            v.nextthink = 0.0;
            v.health = health;
        }
        eng.set_origin(id, origin);
        if self.active {
            self.move_to_next(eng, id);
        }
    }
}
