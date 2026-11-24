//! Sliding doors, rotating doors, momentary doors, and `func_water`
//! ported from ReGameDLL_CS (`doors.cpp` / `doors.h`).

use cs_engine::consts::{MOVETYPE_PUSH, SOLID_BBOX, SOLID_BSP, SOLID_NOT};
use cs_engine::{EdictId, EngineApi};
use glam::Vec3;

use crate::GameState;
use crate::entity::{FCAP_ACROSS_TRANSITION, GameEntity, UseType};
use crate::menus::atoi;
use crate::player::{self, dmg, player_slot};
use crate::targets::{is_master_triggered, use_targets};
use crate::world::{atof, brush_model_number, set_movedir};

const SF_DOOR_START_OPEN: u32 = 1 << 0;
const SF_DOOR_ROTATE_BACKWARDS: u32 = 1 << 1;
const SF_DOOR_PASSABLE: u32 = 1 << 3;
const SF_DOOR_ROTATE_ONEWAY: u32 = 1 << 4;
const SF_DOOR_NO_AUTO_RETURN: u32 = 1 << 5;
const SF_DOOR_ROTATE_X_AXIS: u32 = 1 << 6;
const SF_DOOR_ROTATE_Y_AXIS: u32 = 1 << 7;
const SF_DOOR_USE_ONLY: u32 = 1 << 8;
/// `SF_DOOR_ACTUALLY_WATER` (`BIT(31)`, ReGameDLL `doors.h`): marks a water-contents door.
const SF_DOOR_ACTUALLY_WATER: u32 = 1 << 31;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum DoorState {
    #[default]
    AtBottom,
    GoingUp,
    AtTop,
    GoingDown,
}

fn key_str<'a>(keys: &'a [(String, String)], name: &str) -> Option<&'a str> {
    keys.iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn door_solid(eng: &EngineApi<'_>, id: EdictId, passable: bool) -> Option<u8> {
    let v = eng.ent(id)?;
    if brush_model_number(&v.model).is_some() {
        Some(if passable { SOLID_NOT } else { SOLID_BSP })
    } else if !eng.has_world_collision() || (v.maxs - v.mins).length_squared() > 0.0 {
        Some(if passable { SOLID_NOT } else { SOLID_BBOX })
    } else {
        None
    }
}

/// Sliding brush door (`func_door`) and non-solid water brush (`func_water`).
pub(crate) struct FuncDoor {
    class: &'static str,
    pos1: Vec3,
    pos2: Vec3,
    speed: f32,
    wait: f32,
    damage: f32,
    master: String,
    start_open: bool,
    is_water: bool,
    state: DoorState,
}

impl FuncDoor {
    pub(crate) fn new(class: &'static str) -> Self {
        Self {
            class,
            pos1: Vec3::ZERO,
            pos2: Vec3::ZERO,
            speed: 100.0,
            wait: 3.0,
            damage: 0.0,
            master: String::new(),
            start_open: false,
            is_water: false,
            state: DoorState::AtBottom,
        }
    }

    fn go_up(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        if matches!(self.state, DoorState::GoingUp | DoorState::AtTop) {
            return;
        }
        self.state = DoorState::GoingUp;
        let dest = self.pos2;
        let speed = self.speed;
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        let delta = dest - v.origin;
        let dist = delta.length();
        if dist < 0.05 || speed <= 0.0 {
            v.origin = dest;
            v.velocity = Vec3::ZERO;
            v.nextthink = v.ltime + 0.01;
            return;
        }
        v.velocity = delta / dist * speed;
        v.nextthink = v.ltime + f64::from(dist / speed);
    }

    fn go_down(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        if matches!(self.state, DoorState::GoingDown | DoorState::AtBottom) {
            return;
        }
        self.state = DoorState::GoingDown;
        let dest = self.pos1;
        let speed = self.speed;
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        let delta = dest - v.origin;
        let dist = delta.length();
        if dist < 0.05 || speed <= 0.0 {
            v.origin = dest;
            v.velocity = Vec3::ZERO;
            v.nextthink = v.ltime + 0.01;
            return;
        }
        v.velocity = delta / dist * speed;
        v.nextthink = v.ltime + f64::from(dist / speed);
    }
}

impl GameEntity for FuncDoor {
    fn classname(&self) -> &'static str {
        self.class
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn object_caps(&self) -> i32 {
        FCAP_ACROSS_TRANSITION
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let skin = key_str(keys, "skin")
            .map(|s| atoi(s).clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
        let speed = key_str(keys, "speed").map(atof).unwrap_or(100.0);
        let wait = key_str(keys, "wait").map(atof).unwrap_or(3.0);
        let lip = key_str(keys, "lip").map(atof).unwrap_or(0.0);
        let dmg = key_str(keys, "dmg").map(atof).unwrap_or(0.0);
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.speed = if speed > 0.0 { speed } else { 100.0 };
        self.wait = wait;
        self.damage = dmg;

        let passable_flag = eng
            .ent(id)
            .is_some_and(|v| v.spawnflags & SF_DOOR_PASSABLE != 0);
        let Some(base_solid) = door_solid(eng, id, passable_flag) else {
            return false;
        };

        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        if let Some(s) = skin {
            v.skin = s;
        }
        let special = v.skin != 0;
        self.is_water = special || self.class == "func_water";
        // `CBaseDoor::Spawn` runs `SetMovedir(pev)` before anything else so `angles` are cleared.
        set_movedir(v);
        if special {
            v.spawnflags |= SF_DOOR_ACTUALLY_WATER;
        }
        v.solid = if special { SOLID_NOT } else { base_solid };
        v.movetype = MOVETYPE_PUSH;
        v.velocity = Vec3::ZERO;

        let size = v.maxs - v.mins;
        let travel = (v.movedir.abs().dot(size) - lip).max(0.0);
        self.pos1 = v.origin;
        self.pos2 = self.pos1 + v.movedir * travel;
        self.start_open = v.spawnflags & SF_DOOR_START_OPEN != 0;
        if self.start_open {
            v.origin = self.pos2;
            self.pos2 = self.pos1;
            self.pos1 = v.origin;
        }
        self.state = DoorState::AtBottom;
        eng.link_entity(id, false);
        true
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if self.is_water {
            return;
        }
        let Some(v) = eng.ent(id) else {
            return;
        };
        if v.spawnflags & SF_DOOR_USE_ONLY != 0 {
            return;
        }
        let touching_actor = player_slot(g, eng, other).is_some()
            || eng
                .private::<Box<dyn GameEntity>>(other)
                .is_some_and(|e| e.is_hostage());
        if !touching_actor {
            return;
        }
        if !is_master_triggered(g, eng, &self.master, Some(other)) {
            return;
        }
        if matches!(self.state, DoorState::AtBottom) {
            self.go_up(eng, id);
            use_targets(g, eng, id, Some(other), UseType::Toggle, 0.0);
        }
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        _value: f32,
    ) {
        if !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        match use_type {
            UseType::On => self.go_up(eng, id),
            UseType::Off => self.go_down(eng, id),
            UseType::Toggle | UseType::Set => match self.state {
                DoorState::AtBottom | DoorState::GoingDown => self.go_up(eng, id),
                DoorState::AtTop | DoorState::GoingUp => self.go_down(eng, id),
            },
        }
        use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
    }

    fn think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        match self.state {
            DoorState::GoingUp => {
                eng.set_origin(id, self.pos2);
                self.state = DoorState::AtTop;
                let auto_return = eng
                    .ent(id)
                    .is_some_and(|v| v.spawnflags & SF_DOOR_NO_AUTO_RETURN == 0)
                    && self.wait >= 0.0;
                if let Some(v) = eng.ent_mut(id) {
                    v.velocity = Vec3::ZERO;
                    v.nextthink = if auto_return {
                        v.ltime + f64::from(self.wait)
                    } else {
                        0.0
                    };
                }
                use_targets(g, eng, id, None, UseType::Toggle, 0.0);
            }
            DoorState::AtTop => {
                self.go_down(eng, id);
            }
            DoorState::GoingDown => {
                eng.set_origin(id, self.pos1);
                self.state = DoorState::AtBottom;
                if let Some(v) = eng.ent_mut(id) {
                    v.velocity = Vec3::ZERO;
                    v.nextthink = 0.0;
                }
            }
            DoorState::AtBottom => {}
        }
    }

    fn blocked(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if self.damage > 0.0
            && let Some(slot) = player_slot(g, eng, other)
        {
            player::take_damage(
                g,
                eng,
                slot as u8,
                Some(id),
                Some(id),
                self.damage,
                dmg::CRUSH,
            );
        }
        if eng
            .ent(id)
            .is_some_and(|v| v.spawnflags & SF_DOOR_NO_AUTO_RETURN != 0)
        {
            return;
        }
        match self.state {
            DoorState::GoingUp => self.go_down(eng, id),
            DoorState::GoingDown => self.go_up(eng, id),
            _ => {}
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.state = DoorState::AtBottom;
        let reset_pos = self.pos1;
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = Vec3::ZERO;
            v.nextthink = 0.0;
        }
        eng.set_origin(id, reset_pos);
    }
}

/// Rotating brush door (`func_door_rotating`, `CRotDoor`).
pub(crate) struct FuncDoorRotating {
    angle1: Vec3,
    angle2: Vec3,
    speed: f32,
    wait: f32,
    damage: f32,
    master: String,
    state: DoorState,
}

impl Default for FuncDoorRotating {
    fn default() -> Self {
        Self {
            angle1: Vec3::ZERO,
            angle2: Vec3::ZERO,
            speed: 100.0,
            wait: 3.0,
            damage: 0.0,
            master: String::new(),
            state: DoorState::AtBottom,
        }
    }
}

impl FuncDoorRotating {
    fn move_angles(&mut self, eng: &mut EngineApi<'_>, id: EdictId, dest: Vec3, target_state: DoorState) {
        self.state = target_state;
        let speed = self.speed;
        let Some(v) = eng.ent_mut(id) else {
            return;
        };
        let delta = dest - v.angles;
        let dist = delta.length();
        if dist < 0.05 || speed <= 0.0 {
            v.angles = dest;
            v.avelocity = Vec3::ZERO;
            v.nextthink = v.ltime + 0.01;
            return;
        }
        v.avelocity = delta / dist * speed;
        v.nextthink = v.ltime + f64::from(dist / speed);
    }
}

impl GameEntity for FuncDoorRotating {
    fn classname(&self) -> &'static str {
        "func_door_rotating"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn object_caps(&self) -> i32 {
        FCAP_ACROSS_TRANSITION
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let passable = eng
            .ent(id)
            .is_some_and(|v| v.spawnflags & SF_DOOR_PASSABLE != 0);
        let Some(solid) = door_solid(eng, id, passable) else {
            return false;
        };
        let speed = key_str(keys, "speed").map(atof).unwrap_or(100.0);
        let wait = key_str(keys, "wait").map(atof).unwrap_or(3.0);
        let distance = key_str(keys, "distance").map(atof).unwrap_or(90.0);
        self.damage = key_str(keys, "dmg").map(atof).unwrap_or(0.0);
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.speed = if speed > 0.0 { speed } else { 100.0 };
        self.wait = wait;

        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        // `CRotDoor` ignores `skin` (`SOLID_BSP` unless `SF_DOOR_PASSABLE`).
        v.solid = solid;
        v.movetype = MOVETYPE_PUSH;
        v.velocity = Vec3::ZERO;
        v.avelocity = Vec3::ZERO;

        let dir = if v.spawnflags & SF_DOOR_ROTATE_BACKWARDS != 0 {
            -1.0
        } else {
            1.0
        };
        let axis = if v.spawnflags & SF_DOOR_ROTATE_X_AXIS != 0 {
            Vec3::Z // Roll in GoldSrc angle triplet (pitch, yaw, roll)
        } else if v.spawnflags & SF_DOOR_ROTATE_Y_AXIS != 0 {
            Vec3::X // Pitch
        } else {
            Vec3::Y // Yaw
        };
        self.angle1 = v.angles;
        self.angle2 = self.angle1 + axis * (distance * dir);
        if v.spawnflags & SF_DOOR_START_OPEN != 0 {
            v.angles = self.angle2;
            self.angle2 = self.angle1;
            self.angle1 = v.angles;
        }
        self.state = DoorState::AtBottom;
        eng.link_entity(id, false);
        true
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if eng
            .ent(id)
            .is_some_and(|v| v.spawnflags & SF_DOOR_USE_ONLY != 0)
        {
            return;
        }
        if player_slot(g, eng, other).is_none() {
            return;
        }
        if !is_master_triggered(g, eng, &self.master, Some(other)) {
            return;
        }
        if matches!(self.state, DoorState::AtBottom) {
            if let Some(v) = eng.ent(id)
                && v.spawnflags & SF_DOOR_ROTATE_ONEWAY == 0
                && !self.angle1.is_nan()
            {
                // Rotate away from activator if two-way.
                let origin = v.origin;
                if let Some(o) = eng.ent(other)
                    && o.origin.x < origin.x
                {
                    // Keep deterministic orientation unless opposite wanted.
                }
            }
            self.move_angles(eng, id, self.angle2, DoorState::GoingUp);
            use_targets(g, eng, id, Some(other), UseType::Toggle, 0.0);
        }
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        _value: f32,
    ) {
        if !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        match use_type {
            UseType::On => self.move_angles(eng, id, self.angle2, DoorState::GoingUp),
            UseType::Off => self.move_angles(eng, id, self.angle1, DoorState::GoingDown),
            UseType::Toggle | UseType::Set => match self.state {
                DoorState::AtBottom | DoorState::GoingDown => {
                    self.move_angles(eng, id, self.angle2, DoorState::GoingUp);
                }
                DoorState::AtTop | DoorState::GoingUp => {
                    self.move_angles(eng, id, self.angle1, DoorState::GoingDown);
                }
            },
        }
        use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
    }

    fn think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        match self.state {
            DoorState::GoingUp => {
                self.state = DoorState::AtTop;
                let auto_return = eng
                    .ent(id)
                    .is_some_and(|v| v.spawnflags & SF_DOOR_NO_AUTO_RETURN == 0)
                    && self.wait >= 0.0;
                if let Some(v) = eng.ent_mut(id) {
                    v.angles = self.angle2;
                    v.avelocity = Vec3::ZERO;
                    v.nextthink = if auto_return {
                        v.ltime + f64::from(self.wait)
                    } else {
                        0.0
                    };
                }
                eng.link_entity(id, false);
                use_targets(g, eng, id, None, UseType::Toggle, 0.0);
            }
            DoorState::AtTop => {
                self.move_angles(eng, id, self.angle1, DoorState::GoingDown);
            }
            DoorState::GoingDown => {
                self.state = DoorState::AtBottom;
                if let Some(v) = eng.ent_mut(id) {
                    v.angles = self.angle1;
                    v.avelocity = Vec3::ZERO;
                    v.nextthink = 0.0;
                }
                eng.link_entity(id, false);
            }
            DoorState::AtBottom => {}
        }
    }

    fn blocked(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if self.damage > 0.0
            && let Some(slot) = player_slot(g, eng, other)
        {
            player::take_damage(g, eng, slot as u8, Some(id), Some(id), self.damage, dmg::CRUSH);
        }
        if eng
            .ent(id)
            .is_some_and(|v| v.spawnflags & SF_DOOR_NO_AUTO_RETURN == 0)
        {
            match self.state {
                DoorState::GoingUp => self.move_angles(eng, id, self.angle1, DoorState::GoingDown),
                DoorState::GoingDown => self.move_angles(eng, id, self.angle2, DoorState::GoingUp),
                _ => {}
            }
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.state = DoorState::AtBottom;
        if let Some(v) = eng.ent_mut(id) {
            v.angles = self.angle1;
            v.avelocity = Vec3::ZERO;
            v.nextthink = 0.0;
        }
        eng.link_entity(id, false);
    }
}

/// Momentary sliding door (`momentary_door`, `CMomentaryDoor`): moves proportionally to a
/// `UseType::Set` normalized value in `[0, 1]`.
pub(crate) struct MomentaryDoor {
    pos1: Vec3,
    pos2: Vec3,
    dest: Vec3,
    speed: f32,
}

impl Default for MomentaryDoor {
    fn default() -> Self {
        Self {
            pos1: Vec3::ZERO,
            pos2: Vec3::ZERO,
            dest: Vec3::ZERO,
            speed: 100.0,
        }
    }
}

impl GameEntity for MomentaryDoor {
    fn classname(&self) -> &'static str {
        "momentary_door"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let Some(solid) = door_solid(eng, id, false) else {
            return false;
        };
        let speed = key_str(keys, "speed").map(atof).unwrap_or(100.0);
        let lip = key_str(keys, "lip").map(atof).unwrap_or(0.0);
        self.speed = if speed > 0.0 { speed } else { 100.0 };

        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        set_movedir(v);
        v.solid = solid;
        v.movetype = MOVETYPE_PUSH;
        let size = v.maxs - v.mins;
        let travel = (v.movedir.abs().dot(size) - lip).max(0.0);
        self.pos1 = v.origin;
        self.pos2 = self.pos1 + v.movedir * travel;
        self.dest = self.pos1;
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
        _use_type: UseType,
        value: f32,
    ) {
        let frac = value.clamp(0.0, 1.0);
        self.dest = self.pos1.lerp(self.pos2, frac);
        let speed = self.speed;
        let dest = self.dest;
        if let Some(v) = eng.ent_mut(id) {
            let delta = dest - v.origin;
            let dist = delta.length();
            if dist < 0.05 || speed <= 0.0 {
                v.origin = dest;
                v.velocity = Vec3::ZERO;
                v.nextthink = 0.0;
            } else {
                v.velocity = delta / dist * speed;
                v.nextthink = v.ltime + f64::from(dist / speed);
            }
        }
    }

    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        eng.set_origin(id, self.dest);
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = Vec3::ZERO;
            v.nextthink = 0.0;
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.dest = self.pos1;
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = Vec3::ZERO;
            v.nextthink = 0.0;
        }
        eng.set_origin(id, self.pos1);
    }
}
