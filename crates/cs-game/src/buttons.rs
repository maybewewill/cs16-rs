//! Button brush entities ported from ReGameDLL_CS (`buttons.cpp`):
//! `func_button`, `func_rot_button`, `momentary_rot_button`, and `button_target`.

use cs_engine::consts::{MOVETYPE_PUSH, SOLID_BBOX, SOLID_BSP, SOLID_NOT};
use cs_engine::{EdictId, EngineApi};
use glam::Vec3;

use crate::GameState;
use crate::entity::{
    FCAP_ACROSS_TRANSITION, FCAP_CONTINUOUS_USE, FCAP_IMPULSE_USE, GameEntity, UseType,
};
use crate::player::player_slot;
use crate::targets::{is_master_triggered, use_targets};
use crate::world::{atof, brush_model_number, set_movedir};

const SF_BUTTON_DONTMOVE: u32 = 1 << 0;
const SF_BUTTON_TOGGLE: u32 = 1 << 5;
const SF_BUTTON_TOUCH_ONLY: u32 = 1 << 8;

const SF_ROTBUTTON_NOTSOLID: u32 = 1 << 0;
const SF_ROTBUTTON_REVERSE: u32 = 1 << 1;
const SF_ROTBUTTON_X_AXIS: u32 = 1 << 6;
const SF_ROTBUTTON_Y_AXIS: u32 = 1 << 7;

const SF_MOMENTARY_DOOR: u32 = 1 << 0;
const SF_MOMENTARY_NOT_SOLID: u32 = 1 << 4;
const SF_MOMENTARY_AUTO_RETURN: u32 = 1 << 4;

const SF_BTARGET_USE: u32 = 1 << 0;
const SF_BTARGET_ON: u32 = 1 << 1;

fn key_str<'a>(keys: &'a [(String, String)], name: &str) -> Option<&'a str> {
    keys.iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn button_solid(eng: &EngineApi<'_>, id: EdictId, not_solid: bool) -> Option<u8> {
    let v = eng.ent(id)?;
    if brush_model_number(&v.model).is_some() {
        Some(if not_solid { SOLID_NOT } else { SOLID_BSP })
    } else if !eng.has_world_collision() || (v.maxs - v.mins).length_squared() > 0.0 {
        Some(if not_solid { SOLID_NOT } else { SOLID_BBOX })
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ButtonState {
    #[default]
    Ready,
    GoingDown,
    Pressed,
    GoingUp,
}

/// Sliding push button (`func_button`, `CBaseButton`).
pub(crate) struct FuncButton {
    pos1: Vec3,
    pos2: Vec3,
    speed: f32,
    wait: f32,
    spawn_health: f32,
    damageable: bool,
    master: String,
    touch_only: bool,
    dont_move: bool,
    toggle: bool,
    stay_pressed: bool,
    state: ButtonState,
}

impl Default for FuncButton {
    fn default() -> Self {
        Self {
            pos1: Vec3::ZERO,
            pos2: Vec3::ZERO,
            speed: 40.0,
            wait: 1.0,
            spawn_health: 0.0,
            damageable: false,
            master: String::new(),
            touch_only: false,
            dont_move: false,
            toggle: false,
            stay_pressed: false,
            state: ButtonState::Ready,
        }
    }
}

impl FuncButton {
    fn press(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, activator: Option<EdictId>) {
        if !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        match self.state {
            ButtonState::GoingDown | ButtonState::GoingUp => return,
            ButtonState::Pressed if !self.toggle => return,
            ButtonState::Pressed => {
                // Toggle back to unpressed position.
                self.return_to_ready(eng, id);
                use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
                return;
            }
            ButtonState::Ready => {}
        }

        use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
        if self.dont_move || self.pos1 == self.pos2 {
            self.state = ButtonState::Pressed;
            if !self.stay_pressed && !self.toggle && self.wait >= 0.0 {
                let wait = self.wait;
                if let Some(v) = eng.ent_mut(id) {
                    v.nextthink = v.ltime + f64::from(wait);
                }
            }
            return;
        }
        self.state = ButtonState::GoingDown;
        let dest = self.pos2;
        let speed = self.speed;
        if let Some(v) = eng.ent_mut(id) {
            let delta = dest - v.origin;
            let dist = delta.length();
            if dist < 0.05 || speed <= 0.0 {
                v.origin = dest;
                v.velocity = Vec3::ZERO;
                v.nextthink = v.ltime + 0.01;
            } else {
                v.velocity = delta / dist * speed;
                v.nextthink = v.ltime + f64::from(dist / speed);
            }
        }
    }

    fn return_to_ready(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        if self.dont_move || self.pos1 == self.pos2 {
            self.state = ButtonState::Ready;
            if let Some(v) = eng.ent_mut(id) {
                v.frame = 0.0;
                v.nextthink = 0.0;
            }
            return;
        }
        self.state = ButtonState::GoingUp;
        let dest = self.pos1;
        let speed = self.speed;
        if let Some(v) = eng.ent_mut(id) {
            let delta = dest - v.origin;
            let dist = delta.length();
            if dist < 0.05 || speed <= 0.0 {
                v.origin = dest;
                v.velocity = Vec3::ZERO;
                v.nextthink = v.ltime + 0.01;
            } else {
                v.velocity = delta / dist * speed;
                v.nextthink = v.ltime + f64::from(dist / speed);
            }
        }
    }
}

impl GameEntity for FuncButton {
    fn classname(&self) -> &'static str {
        "func_button"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn object_caps(&self) -> i32 {
        if self.touch_only {
            FCAP_ACROSS_TRANSITION
        } else {
            FCAP_ACROSS_TRANSITION | FCAP_IMPULSE_USE
        }
    }

    fn takes_damage(&self) -> bool {
        self.damageable
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let Some(solid) = button_solid(eng, id, false) else {
            return false;
        };
        let speed = key_str(keys, "speed").map(atof).unwrap_or(40.0);
        let wait = key_str(keys, "wait").map(atof).unwrap_or(1.0);
        let lip = key_str(keys, "lip").map(atof).unwrap_or(4.0);
        let health = key_str(keys, "health").map(atof).unwrap_or(0.0);
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.speed = if speed > 0.0 { speed } else { 40.0 };
        self.wait = wait;
        self.spawn_health = health;
        self.damageable = health > 0.0;

        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        set_movedir(v);
        v.solid = solid;
        v.movetype = MOVETYPE_PUSH;
        v.velocity = Vec3::ZERO;
        v.health = health;

        self.dont_move = v.spawnflags & SF_BUTTON_DONTMOVE != 0;
        self.toggle = v.spawnflags & SF_BUTTON_TOGGLE != 0;
        self.touch_only = v.spawnflags & SF_BUTTON_TOUCH_ONLY != 0;
        self.stay_pressed = self.wait < 0.0;

        let size = v.maxs - v.mins;
        let travel = (v.movedir.abs().dot(size) - lip).max(0.0);
        self.pos1 = v.origin;
        self.pos2 = if self.dont_move {
            self.pos1
        } else {
            self.pos1 + v.movedir * travel
        };
        self.state = ButtonState::Ready;
        eng.link_entity(id, false);
        true
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if !self.touch_only || player_slot(g, eng, other).is_none() {
            return;
        }
        self.press(g, eng, id, Some(other));
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
        self.press(g, eng, id, activator);
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
        if !self.damageable {
            return false;
        }
        let reset = self.spawn_health;
        let broken = if let Some(v) = eng.ent_mut(id) {
            v.health -= damage;
            if v.health <= 0.0 {
                v.health = reset;
                true
            } else {
                false
            }
        } else {
            false
        };
        if broken {
            self.press(g, eng, id, attacker);
        }
        true
    }

    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        match self.state {
            ButtonState::GoingDown => {
                eng.set_origin(id, self.pos2);
                self.state = ButtonState::Pressed;
                let auto_return = !self.stay_pressed && !self.toggle && self.wait >= 0.0;
                if let Some(v) = eng.ent_mut(id) {
                    v.velocity = Vec3::ZERO;
                    v.frame = 1.0;
                    v.nextthink = if auto_return {
                        v.ltime + f64::from(self.wait)
                    } else {
                        0.0
                    };
                }
            }
            ButtonState::Pressed => {
                self.return_to_ready(eng, id);
            }
            ButtonState::GoingUp => {
                eng.set_origin(id, self.pos1);
                self.state = ButtonState::Ready;
                if let Some(v) = eng.ent_mut(id) {
                    v.velocity = Vec3::ZERO;
                    v.frame = 0.0;
                    v.nextthink = 0.0;
                }
            }
            ButtonState::Ready => {}
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.state = ButtonState::Ready;
        let reset_health = self.spawn_health;
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = Vec3::ZERO;
            v.frame = 0.0;
            v.nextthink = 0.0;
            v.health = reset_health;
        }
        eng.set_origin(id, self.pos1);
    }
}

/// Rotating push button (`func_rot_button`, `CRotButton`).
pub(crate) struct FuncRotButton {
    angle1: Vec3,
    angle2: Vec3,
    speed: f32,
    wait: f32,
    spawn_health: f32,
    damageable: bool,
    master: String,
    touch_only: bool,
    toggle: bool,
    state: ButtonState,
}

impl Default for FuncRotButton {
    fn default() -> Self {
        Self {
            angle1: Vec3::ZERO,
            angle2: Vec3::ZERO,
            speed: 40.0,
            wait: 1.0,
            spawn_health: 0.0,
            damageable: false,
            master: String::new(),
            touch_only: false,
            toggle: false,
            state: ButtonState::Ready,
        }
    }
}

impl FuncRotButton {
    fn move_angles(&mut self, eng: &mut EngineApi<'_>, id: EdictId, dest: Vec3, state: ButtonState) {
        self.state = state;
        let speed = self.speed;
        if let Some(v) = eng.ent_mut(id) {
            let delta = dest - v.angles;
            let dist = delta.length();
            if dist < 0.05 || speed <= 0.0 {
                v.angles = dest;
                v.avelocity = Vec3::ZERO;
                v.nextthink = v.ltime + 0.01;
            } else {
                v.avelocity = delta / dist * speed;
                v.nextthink = v.ltime + f64::from(dist / speed);
            }
        }
    }
}

impl GameEntity for FuncRotButton {
    fn classname(&self) -> &'static str {
        "func_rot_button"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn object_caps(&self) -> i32 {
        if self.touch_only {
            FCAP_ACROSS_TRANSITION
        } else {
            FCAP_ACROSS_TRANSITION | FCAP_IMPULSE_USE
        }
    }

    fn takes_damage(&self) -> bool {
        self.damageable
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let not_solid = eng
            .ent(id)
            .is_some_and(|v| v.spawnflags & SF_ROTBUTTON_NOTSOLID != 0);
        let Some(solid) = button_solid(eng, id, not_solid) else {
            return false;
        };
        let speed = key_str(keys, "speed").map(atof).unwrap_or(40.0);
        let wait = key_str(keys, "wait").map(atof).unwrap_or(1.0);
        let distance = key_str(keys, "distance").map(atof).unwrap_or(90.0);
        let health = key_str(keys, "health").map(atof).unwrap_or(0.0);
        self.master = key_str(keys, "master").unwrap_or("").to_string();
        self.speed = if speed > 0.0 { speed } else { 40.0 };
        self.wait = wait;
        self.spawn_health = health;
        self.damageable = health > 0.0;

        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.solid = solid;
        v.movetype = MOVETYPE_PUSH;
        v.avelocity = Vec3::ZERO;
        v.health = health;
        self.touch_only = v.spawnflags & SF_BUTTON_TOUCH_ONLY != 0;
        self.toggle = v.spawnflags & SF_BUTTON_TOGGLE != 0;

        let sign = if v.spawnflags & SF_ROTBUTTON_REVERSE != 0 {
            -1.0
        } else {
            1.0
        };
        let axis = if v.spawnflags & SF_ROTBUTTON_X_AXIS != 0 {
            Vec3::Z
        } else if v.spawnflags & SF_ROTBUTTON_Y_AXIS != 0 {
            Vec3::X
        } else {
            Vec3::Y
        };
        self.angle1 = v.angles;
        self.angle2 = self.angle1 + axis * (distance * sign);
        self.state = ButtonState::Ready;
        eng.link_entity(id, false);
        true
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if self.touch_only && player_slot(g, eng, other).is_some() {
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
        if !is_master_triggered(g, eng, &self.master, activator) {
            return;
        }
        match self.state {
            ButtonState::Ready => {
                use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
                self.move_angles(eng, id, self.angle2, ButtonState::GoingDown);
            }
            ButtonState::Pressed if self.toggle => {
                use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
                self.move_angles(eng, id, self.angle1, ButtonState::GoingUp);
            }
            _ => {}
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
        if !self.damageable {
            return false;
        }
        let reset = self.spawn_health;
        let hit = if let Some(v) = eng.ent_mut(id) {
            v.health -= damage;
            if v.health <= 0.0 {
                v.health = reset;
                true
            } else {
                false
            }
        } else {
            false
        };
        if hit {
            self.use_(g, eng, id, attacker, attacker, UseType::Toggle, 0.0);
        }
        true
    }

    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        match self.state {
            ButtonState::GoingDown => {
                self.state = ButtonState::Pressed;
                let auto_return = !self.toggle && self.wait >= 0.0;
                if let Some(v) = eng.ent_mut(id) {
                    v.angles = self.angle2;
                    v.avelocity = Vec3::ZERO;
                    v.nextthink = if auto_return {
                        v.ltime + f64::from(self.wait)
                    } else {
                        0.0
                    };
                }
            }
            ButtonState::Pressed => self.move_angles(eng, id, self.angle1, ButtonState::GoingUp),
            ButtonState::GoingUp => {
                self.state = ButtonState::Ready;
                if let Some(v) = eng.ent_mut(id) {
                    v.angles = self.angle1;
                    v.avelocity = Vec3::ZERO;
                    v.nextthink = 0.0;
                }
            }
            ButtonState::Ready => {}
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.state = ButtonState::Ready;
        let reset_health = self.spawn_health;
        if let Some(v) = eng.ent_mut(id) {
            v.angles = self.angle1;
            v.avelocity = Vec3::ZERO;
            v.nextthink = 0.0;
            v.health = reset_health;
        }
        eng.link_entity(id, false);
    }
}

/// Continuous rotating crank/wheel (`momentary_rot_button`, `CMomentaryRotButton`).
pub(crate) struct MomentaryRotButton {
    start: Vec3,
    end: Vec3,
    axis: Vec3,
    distance: f32,
    speed: f32,
    return_speed: f32,
    fraction: f32,
    auto_return: bool,
    last_used: f64,
}

impl Default for MomentaryRotButton {
    fn default() -> Self {
        Self {
            start: Vec3::ZERO,
            end: Vec3::ZERO,
            axis: Vec3::Y,
            distance: 90.0,
            speed: 40.0,
            return_speed: 40.0,
            fraction: 0.0,
            auto_return: false,
            last_used: 0.0,
        }
    }
}

impl GameEntity for MomentaryRotButton {
    fn classname(&self) -> &'static str {
        "momentary_rot_button"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn object_caps(&self) -> i32 {
        let _ = SF_MOMENTARY_DOOR;
        FCAP_ACROSS_TRANSITION | FCAP_CONTINUOUS_USE
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let not_solid = eng
            .ent(id)
            .is_some_and(|v| v.spawnflags & SF_MOMENTARY_NOT_SOLID != 0);
        let Some(solid) = button_solid(eng, id, not_solid) else {
            return false;
        };
        let speed = key_str(keys, "speed").map(atof).unwrap_or(40.0);
        let return_speed = key_str(keys, "returnspeed").map(atof).unwrap_or(speed);
        let distance = key_str(keys, "distance").map(atof).unwrap_or(90.0);
        self.speed = if speed > 0.0 { speed } else { 40.0 };
        self.return_speed = return_speed.max(0.0);
        self.distance = distance.abs().max(1.0);

        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.solid = solid;
        v.movetype = MOVETYPE_PUSH;
        self.auto_return = v.spawnflags & SF_MOMENTARY_AUTO_RETURN == 0;
        let axis = if v.spawnflags & SF_ROTBUTTON_X_AXIS != 0 {
            Vec3::Z
        } else if v.spawnflags & SF_ROTBUTTON_Y_AXIS != 0 {
            Vec3::X
        } else {
            Vec3::Y
        };
        let sign = if v.spawnflags & SF_ROTBUTTON_REVERSE != 0 {
            -1.0
        } else {
            1.0
        };
        self.axis = axis * sign;
        self.start = v.angles;
        self.end = self.start + self.axis * self.distance;
        self.fraction = 0.0;
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
        let dt = eng.frametime().max(0.05);
        let now = eng.time();
        self.last_used = now;
        let step = (self.speed * dt) / self.distance;
        self.fraction = (self.fraction + step).clamp(0.0, 1.0);
        let target_angles = self.start.lerp(self.end, self.fraction);
        if let Some(v) = eng.ent_mut(id) {
            v.angles = target_angles;
            v.nextthink = if self.auto_return && self.return_speed > 0.0 {
                v.ltime + 0.1
            } else {
                0.0
            };
        }
        use_targets(g, eng, id, activator, UseType::Set, self.fraction);
    }

    fn think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let now = eng.time();
        if !self.auto_return || now - self.last_used < 0.08 || self.return_speed <= 0.0 {
            return;
        }
        let dt = eng.frametime().max(0.05);
        let step = (self.return_speed * dt) / self.distance;
        self.fraction = (self.fraction - step).clamp(0.0, 1.0);
        let target_angles = self.start.lerp(self.end, self.fraction);
        let more = self.fraction > 0.0;
        if let Some(v) = eng.ent_mut(id) {
            v.angles = target_angles;
            v.nextthink = if more { v.ltime + 0.05 } else { 0.0 };
        }
        use_targets(g, eng, id, None, UseType::Set, self.fraction);
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.fraction = 0.0;
        self.last_used = 0.0;
        if let Some(v) = eng.ent_mut(id) {
            v.angles = self.start;
            v.avelocity = Vec3::ZERO;
            v.nextthink = 0.0;
        }
    }
}

/// Shootable/usable target brush (`button_target`, `CButtonTarget`).
pub(crate) struct ButtonTarget {
    use_enabled: bool,
    start_on: bool,
    on: bool,
}

impl Default for ButtonTarget {
    fn default() -> Self {
        Self {
            use_enabled: false,
            start_on: false,
            on: false,
        }
    }
}

impl GameEntity for ButtonTarget {
    fn classname(&self) -> &'static str {
        "button_target"
    }

    fn is_delay(&self) -> bool {
        true
    }

    fn object_caps(&self) -> i32 {
        if self.use_enabled {
            FCAP_ACROSS_TRANSITION | FCAP_IMPULSE_USE
        } else {
            FCAP_ACROSS_TRANSITION
        }
    }

    fn takes_damage(&self) -> bool {
        true
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        let Some(solid) = button_solid(eng, id, false) else {
            return false;
        };
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.solid = solid;
        v.movetype = MOVETYPE_PUSH;
        self.use_enabled = v.spawnflags & SF_BTARGET_USE != 0;
        self.start_on = v.spawnflags & SF_BTARGET_ON != 0;
        self.on = self.start_on;
        v.frame = if self.on { 1.0 } else { 0.0 };
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
        if !self.use_enabled {
            return;
        }
        self.on = !self.on;
        if let Some(v) = eng.ent_mut(id) {
            v.frame = if self.on { 1.0 } else { 0.0 };
        }
        use_targets(g, eng, id, activator, UseType::Toggle, 0.0);
    }

    fn take_damage(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        _inflictor: Option<EdictId>,
        attacker: Option<EdictId>,
        _damage: f32,
        _bits: u32,
    ) -> bool {
        self.on = !self.on;
        if let Some(v) = eng.ent_mut(id) {
            v.frame = if self.on { 1.0 } else { 0.0 };
        }
        use_targets(g, eng, id, attacker, UseType::Toggle, 0.0);
        true
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.on = self.start_on;
        if let Some(v) = eng.ent_mut(id) {
            v.frame = if self.on { 1.0 } else { 0.0 };
        }
    }
}
