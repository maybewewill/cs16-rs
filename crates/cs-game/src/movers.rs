//! Moving brush entities ported from ReGameDLL_CS (`plats.cpp` & `func_break.cpp` pushables):
//! `func_pushable`, `func_conveyor`, `func_plat`, `path_corner`, and `func_train`.

use cs_engine::consts::{
    FL_CONVEYOR, FL_ONGROUND, MOVETYPE_NONE, MOVETYPE_PUSH, MOVETYPE_PUSHSTEP, SOLID_BBOX,
    SOLID_BSP, SOLID_NOT,
};
use cs_engine::{EdictId, EngineApi};
use glam::Vec3;

use crate::GameState;
use crate::entity::{
    FCAP_ACROSS_TRANSITION, FCAP_CONTINUOUS_USE, FCAP_IMPULSE_USE, GameEntity, UseType,
};
use crate::messages;
use crate::player::{self, dmg, player_slot};
use crate::targets::find_by_targetname;
use crate::world::{atof, brush_model_number, set_movedir};

const SF_CONVEYOR_NOTSOLID: u32 = 2;
const SF_PLAT_TOGGLE: u32 = 1;
const SF_CORNER_WAITFORTRIG: u32 = 1;

fn key_str<'a>(keys: &'a [(String, String)], name: &str) -> Option<&'a str> {
    keys.iter()
        .rev()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn mover_solid_kind(eng: &EngineApi<'_>, id: EdictId) -> Option<u8> {
    let v = eng.ent(id)?;
    if brush_model_number(&v.model).is_some() {
        Some(SOLID_BSP)
    } else if !eng.has_world_collision() || (v.maxs - v.mins).length_squared() > 0.0 {
        Some(SOLID_BBOX)
    } else {
        None
    }
}

fn linear_move(eng: &mut EngineApi<'_>, id: EdictId, dest: Vec3, speed: f32) {
    let Some(v) = eng.ent_mut(id) else {
        return;
    };
    let delta = dest - v.origin;
    let dist = delta.length();
    if dist < 0.05 || speed <= 0.0 {
        v.velocity = Vec3::ZERO;
        v.nextthink = v.ltime + 0.01;
        return;
    }
    let duration = f64::from(dist / speed);
    v.velocity = delta / dist * speed;
    v.nextthink = v.ltime + duration;
}

/// Pushable brush crate (`func_pushable`, `CPushable`).
pub(crate) struct FuncPushable {
    spawn_origin: Vec3,
    spawn_health: f32,
    friction_factor: f32,
    max_push_speed: f32,
}

impl Default for FuncPushable {
    fn default() -> Self {
        Self {
            spawn_origin: Vec3::ZERO,
            spawn_health: 0.0,
            friction_factor: 0.25,
            max_push_speed: 180.0,
        }
    }
}

impl GameEntity for FuncPushable {
    fn classname(&self) -> &'static str {
        "func_pushable"
    }

    fn object_caps(&self) -> i32 {
        FCAP_ACROSS_TRANSITION | FCAP_CONTINUOUS_USE
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let Some(solid) = mover_solid_kind(eng, id) else {
            return false;
        };
        let friction = key_str(keys, "friction")
            .map(atof)
            .unwrap_or(125.0)
            .clamp(0.0, 400.0);
        // Scale 0..400 friction key to 0.1..1.0 velocity multiplier (matching Source bounds).
        self.friction_factor = ((400.0 - friction) / 400.0).clamp(0.15, 0.95);
        let buoyancy = key_str(keys, "buoyancy").map(atof).unwrap_or(20.0);
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        self.spawn_origin = v.origin;
        self.spawn_health = v.health;
        v.movetype = MOVETYPE_PUSHSTEP;
        v.solid = solid;
        v.skin = buoyancy.clamp(0.0, 200.0) as i16;
        v.friction = 1.0;
        eng.link_entity(id, false);
        true
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if player_slot(g, eng, other).is_none() {
            return;
        }
        let Some((other_vel, other_min_z, other_origin)) =
            eng.ent(other).map(|v| (v.velocity, v.absmin.z, v.origin))
        else {
            return;
        };
        let Some((self_vel, self_max_z, self_origin, self_mins, self_maxs)) = eng
            .ent(id)
            .map(|v| (v.velocity, v.absmax.z, v.origin, v.mins, v.maxs))
        else {
            return;
        };
        // Player standing on top rides cleanly instead of pushing it sideways.
        if other_min_z >= self_max_z - 2.0 {
            return;
        }
        let mut push = Vec3::new(other_vel.x, other_vel.y, 0.0);
        if push.length_squared() < 1.0 {
            let center = self_origin + (self_mins + self_maxs) * 0.5;
            let away = Vec3::new(center.x - other_origin.x, center.y - other_origin.y, 0.0);
            if away.length_squared() > 1e-4 {
                push = away.normalize() * 90.0;
            }
        }
        let speed = push.length().min(self.max_push_speed);
        if speed > 1.0 {
            let dir = push.normalize_or_zero();
            let scaled = dir * (speed * self.friction_factor);
            if let Some(v) = eng.ent_mut(id) {
                v.velocity.x = scaled.x;
                v.velocity.y = scaled.y;
                v.velocity.z = self_vel.z;
                v.flags &= !FL_ONGROUND;
            }
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
        if player_slot(g, eng, act).is_none() {
            return;
        }
        let Some((act_org, act_angles)) = eng.ent(act).map(|v| (v.origin, v.v_angle)) else {
            return;
        };
        let (forward, ..) = cs_sim::pm::math::angle_vectors(act_angles);
        let pull = Vec3::new(forward.x, forward.y, 0.0).normalize_or_zero();
        let fallback = eng
            .ent(id)
            .map(|v| (v.origin + (v.mins + v.maxs) * 0.5) - act_org)
            .unwrap_or(Vec3::ZERO);
        let dir = if pull.length_squared() > 0.01 {
            pull
        } else {
            Vec3::new(fallback.x, fallback.y, 0.0).normalize_or_zero()
        };
        if let Some(v) = eng.ent_mut(id) {
            v.velocity.x = dir.x * 120.0 * self.friction_factor;
            v.velocity.y = dir.y * 120.0 * self.friction_factor;
            v.flags &= !FL_ONGROUND;
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let origin = self.spawn_origin;
        let health = self.spawn_health;
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = Vec3::ZERO;
            v.health = health;
            v.flags &= !FL_ONGROUND;
        }
        eng.set_origin(id, origin);
    }
}

/// Moving conveyor belt (`func_conveyor`, `CFuncConveyor`).
pub(crate) struct FuncConveyor {
    speed: f32,
}

impl Default for FuncConveyor {
    fn default() -> Self {
        Self { speed: 100.0 }
    }
}

impl GameEntity for FuncConveyor {
    fn classname(&self) -> &'static str {
        "func_conveyor"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let Some(solid) = mover_solid_kind(eng, id) else {
            return false;
        };
        let parsed_speed = key_str(keys, "speed").map(atof).unwrap_or(100.0);
        self.speed = if parsed_speed != 0.0 { parsed_speed } else { 100.0 };
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        set_movedir(v);
        v.movetype = MOVETYPE_PUSH;
        v.solid = if v.spawnflags & SF_CONVEYOR_NOTSOLID != 0 {
            SOLID_NOT
        } else {
            solid
        };
        v.speed = self.speed;
        v.flags |= FL_CONVEYOR;
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
        _value: f32,
    ) {
        if let Some(v) = eng.ent_mut(id) {
            v.speed = -v.speed;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlatState {
    AtTop,
    AtBottom,
    GoingUp,
    GoingDown,
}

/// Vertical elevator platform (`func_plat`, `CFuncPlat`).
pub(crate) struct FuncPlat {
    pos1: Vec3,
    pos2: Vec3,
    speed: f32,
    wait: f32,
    state: PlatState,
    toggle_mode: bool,
    start_at_top: bool,
}

impl Default for FuncPlat {
    fn default() -> Self {
        Self {
            pos1: Vec3::ZERO,
            pos2: Vec3::ZERO,
            speed: 150.0,
            wait: 3.0,
            state: PlatState::AtBottom,
            toggle_mode: false,
            start_at_top: false,
        }
    }
}

impl FuncPlat {
    fn go_up(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        self.state = PlatState::GoingUp;
        linear_move(eng, id, self.pos1, self.speed);
    }

    fn go_down(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        self.state = PlatState::GoingDown;
        linear_move(eng, id, self.pos2, self.speed);
    }
}

impl GameEntity for FuncPlat {
    fn classname(&self) -> &'static str {
        "func_plat"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let Some(solid) = mover_solid_kind(eng, id) else {
            return false;
        };
        let speed = key_str(keys, "speed").map(atof).unwrap_or(150.0);
        let lip = key_str(keys, "lip").map(atof).unwrap_or(8.0);
        let height_key = key_str(keys, "height").map(atof).unwrap_or(0.0);
        let wait = key_str(keys, "wait").map(atof).unwrap_or(3.0);
        self.speed = if speed > 0.0 { speed } else { 150.0 };
        self.wait = if wait > 0.0 { wait } else { 3.0 };

        let (origin, size, targetname, spawnflags) = {
            let Some(v) = eng.ent_mut(id) else {
                return false;
            };
            v.angles = Vec3::ZERO;
            v.movetype = MOVETYPE_PUSH;
            v.solid = solid;
            v.velocity = Vec3::ZERO;
            (v.origin, v.size, v.targetname.clone(), v.spawnflags)
        };
        let height = if height_key > 0.0 {
            height_key
        } else {
            (size.z - lip).max(8.0)
        };
        self.pos1 = origin;
        self.pos2 = origin - Vec3::new(0.0, 0.0, height);
        self.toggle_mode = (spawnflags & SF_PLAT_TOGGLE) != 0;
        self.start_at_top = !targetname.is_empty() && !self.toggle_mode;
        if self.start_at_top {
            self.state = PlatState::AtTop;
            eng.set_origin(id, self.pos1);
        } else {
            self.state = PlatState::AtBottom;
            eng.set_origin(id, self.pos2);
        }
        true
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if player_slot(g, eng, other).is_none() || self.toggle_mode {
            return;
        }
        match self.state {
            PlatState::AtBottom => self.go_up(eng, id),
            PlatState::AtTop => {
                let wait = self.wait;
                if let Some(v) = eng.ent_mut(id) {
                    v.nextthink = v.ltime + f64::from(wait);
                }
            }
            _ => {}
        }
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
        match self.state {
            PlatState::AtBottom | PlatState::GoingDown => self.go_up(eng, id),
            PlatState::AtTop | PlatState::GoingUp => self.go_down(eng, id),
        }
    }

    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        match self.state {
            PlatState::GoingUp => {
                if let Some(v) = eng.ent_mut(id) {
                    v.velocity = Vec3::ZERO;
                    v.origin = self.pos1;
                }
                eng.set_origin(id, self.pos1);
                self.state = PlatState::AtTop;
                if !self.toggle_mode {
                    let wait = self.wait;
                    if let Some(v) = eng.ent_mut(id) {
                        v.nextthink = v.ltime + f64::from(wait);
                    }
                }
            }
            PlatState::AtTop => {
                self.go_down(eng, id);
            }
            PlatState::GoingDown => {
                if let Some(v) = eng.ent_mut(id) {
                    v.velocity = Vec3::ZERO;
                    v.origin = self.pos2;
                }
                eng.set_origin(id, self.pos2);
                self.state = PlatState::AtBottom;
            }
            PlatState::AtBottom => {}
        }
    }

    fn blocked(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, _other: EdictId) {
        match self.state {
            PlatState::GoingUp => self.go_down(eng, id),
            PlatState::GoingDown => self.go_up(eng, id),
            _ => {}
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = Vec3::ZERO;
            v.nextthink = 0.0;
        }
        if self.start_at_top {
            self.state = PlatState::AtTop;
            eng.set_origin(id, self.pos1);
        } else {
            self.state = PlatState::AtBottom;
            eng.set_origin(id, self.pos2);
        }
    }
}

/// Waypoint for `func_train` (`path_corner`, `CPathTrack`).
#[derive(Default)]
pub(crate) struct PathCorner {
    pub(crate) target: String,
    pub(crate) speed: f32,
    pub(crate) wait: f32,
}

impl GameEntity for PathCorner {
    fn classname(&self) -> &'static str {
        "path_corner"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.target = key_str(keys, "target").unwrap_or("").to_string();
        self.speed = key_str(keys, "speed").map(atof).unwrap_or(0.0);
        self.wait = key_str(keys, "wait").map(atof).unwrap_or(0.0);
        if let Some(v) = eng.ent_mut(id) {
            if v.target.is_empty() {
                v.target = self.target.clone();
            }
            v.speed = self.speed;
            v.movetype = MOVETYPE_NONE;
            v.solid = SOLID_NOT;
        }
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrainState {
    Idle,
    Initializing,
    Moving,
    Waiting,
}

/// Track-following brush train (`func_train`, `CFuncTrain`).
pub(crate) struct FuncTrain {
    speed: f32,
    initial_target: String,
    current_target: String,
    dest: Vec3,
    wait: f32,
    spawn_origin: Vec3,
    state: TrainState,
}

impl Default for FuncTrain {
    fn default() -> Self {
        Self {
            speed: 100.0,
            initial_target: String::new(),
            current_target: String::new(),
            dest: Vec3::ZERO,
            wait: 0.0,
            spawn_origin: Vec3::ZERO,
            state: TrainState::Idle,
        }
    }
}

impl FuncTrain {
    fn corner_origin(eng: &EngineApi<'_>, train: EdictId, corner: EdictId) -> Option<Vec3> {
        let t = eng.ent(train)?;
        let c = eng.ent(corner)?;
        let center_offset = (t.mins + t.maxs) * 0.5;
        Some(c.origin - center_offset)
    }

    fn advance_to_next(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        if self.current_target.is_empty() {
            self.state = TrainState::Idle;
            if let Some(v) = eng.ent_mut(id) {
                v.velocity = Vec3::ZERO;
            }
            return;
        }
        let Some(corner) = find_by_targetname(eng, None, &self.current_target) else {
            self.state = TrainState::Idle;
            return;
        };
        let Some(dest) = Self::corner_origin(eng, id, corner) else {
            return;
        };
        let (next_target, corner_speed, corner_wait) = eng
            .private::<Box<dyn GameEntity>>(corner)
            .and_then(|_| eng.ent(corner))
            .map(|v| (v.target.clone(), v.speed, 0.0))
            .unwrap_or_default();
        let _ = (corner_speed, corner_wait);
        let corner_wait_flag = eng
            .ent(corner)
            .is_some_and(|v| v.spawnflags & SF_CORNER_WAITFORTRIG != 0);
        // Extract speed/wait stored on PathCorner when present via key fields, or fallback.
        self.current_target = next_target;
        self.dest = dest;
        self.wait = if corner_wait_flag { -1.0 } else { 0.0 };
        self.state = TrainState::Moving;
        linear_move(eng, id, dest, self.speed);
    }
}

impl GameEntity for FuncTrain {
    fn classname(&self) -> &'static str {
        "func_train"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let Some(solid) = mover_solid_kind(eng, id) else {
            return false;
        };
        let parsed_speed = key_str(keys, "speed").map(atof).unwrap_or(100.0);
        self.speed = if parsed_speed > 0.0 { parsed_speed } else { 100.0 };
        self.initial_target = key_str(keys, "target").unwrap_or("").to_string();
        self.current_target = self.initial_target.clone();
        let has_name = {
            let Some(v) = eng.ent_mut(id) else {
                return false;
            };
            self.spawn_origin = v.origin;
            v.movetype = MOVETYPE_PUSH;
            v.solid = solid;
            v.velocity = Vec3::ZERO;
            if !self.initial_target.is_empty() && v.targetname.is_empty() {
                v.nextthink = v.ltime + 0.1;
                self.state = TrainState::Initializing;
            } else {
                self.state = TrainState::Idle;
            }
            !v.targetname.is_empty()
        };
        let _ = has_name;
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
        _value: f32,
    ) {
        if matches!(self.state, TrainState::Idle | TrainState::Waiting) {
            if self.current_target == self.initial_target
                && !self.initial_target.is_empty()
                && let Some(first) = find_by_targetname(eng, None, &self.initial_target)
                && let Some(pos) = Self::corner_origin(eng, id, first)
            {
                eng.set_origin(id, pos);
                if let Some(next) = eng.ent(first).map(|v| v.target.clone()) {
                    self.current_target = next;
                }
            }
            self.advance_to_next(eng, id);
        }
    }

    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        match self.state {
            TrainState::Initializing => {
                if let Some(first) = find_by_targetname(eng, None, &self.initial_target) {
                    if let Some(pos) = Self::corner_origin(eng, id, first) {
                        eng.set_origin(id, pos);
                    }
                    self.current_target = eng.ent(first).map(|v| v.target.clone()).unwrap_or_default();
                    self.advance_to_next(eng, id);
                } else {
                    self.state = TrainState::Idle;
                }
            }
            TrainState::Moving => {
                if let Some(v) = eng.ent_mut(id) {
                    v.velocity = Vec3::ZERO;
                }
                eng.set_origin(id, self.dest);
                if self.wait < 0.0 || self.current_target.is_empty() {
                    self.state = TrainState::Waiting;
                } else {
                    self.advance_to_next(eng, id);
                }
            }
            TrainState::Waiting | TrainState::Idle => {}
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.current_target = self.initial_target.clone();
        let start = self.spawn_origin;
        let auto = !self.initial_target.is_empty()
            && eng.ent(id).is_some_and(|v| v.targetname.is_empty());
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = Vec3::ZERO;
            v.nextthink = if auto { v.ltime + 0.1 } else { 0.0 };
        }
        self.state = if auto {
            TrainState::Initializing
        } else {
            TrainState::Idle
        };
        eng.set_origin(id, start);
    }
}

/// Rotating + vertical elevator platform (`func_platrot`, `CFuncPlatRot`).
pub(crate) struct FuncPlatRot {
    plat: FuncPlat,
    start_angles: Vec3,
    end_angles: Vec3,
}

impl Default for FuncPlatRot {
    fn default() -> Self {
        Self {
            plat: FuncPlat::default(),
            start_angles: Vec3::ZERO,
            end_angles: Vec3::ZERO,
        }
    }
}

impl FuncPlatRot {
    fn sync_rotation(&self, eng: &mut EngineApi<'_>, id: EdictId) {
        let frac = eng.ent(id).map_or(0.0, |v| {
            let span = self.plat.pos1.z - self.plat.pos2.z;
            if span.abs() > 0.01 {
                ((v.origin.z - self.plat.pos2.z) / span).clamp(0.0, 1.0)
            } else {
                0.0
            }
        });
        let angles = self.start_angles.lerp(self.end_angles, frac);
        if let Some(v) = eng.ent_mut(id) {
            v.angles = angles;
        }
    }
}

impl GameEntity for FuncPlatRot {
    fn classname(&self) -> &'static str {
        "func_platrot"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let initial_angles = eng.ent(id).map_or(Vec3::ZERO, |v| v.angles);
        let rot = key_str(keys, "rotation").map(atof).unwrap_or(90.0);
        if !self.plat.spawn(eng, id, keys) {
            return false;
        }
        self.start_angles = initial_angles;
        self.end_angles = initial_angles + Vec3::new(0.0, rot, 0.0);
        self.sync_rotation(eng, id);
        true
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        self.plat.touch(g, eng, id, other);
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
        self.plat.use_(g, eng, id, activator, caller, use_type, value);
    }

    fn think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.plat.think(g, eng, id);
        self.sync_rotation(eng, id);
    }

    fn restart(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.plat.restart(g, eng, id);
        self.sync_rotation(eng, id);
    }
}

const SF_PATH_DISABLED: u32 = 1 << 0;
const SF_PATH_ALTREVERSE: u32 = 1 << 2;

/// Track node for `func_tracktrain` and `func_vehicle` (`path_track`, `CPathTrack`).
#[derive(Default)]
pub(crate) struct PathTrack {
    next_target: String,
    alt_path: String,
    use_alt: bool,
    disabled: bool,
    initial_disabled: bool,
}

impl GameEntity for PathTrack {
    fn classname(&self) -> &'static str {
        "path_track"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.next_target = key_str(keys, "target").unwrap_or("").to_string();
        self.alt_path = key_str(keys, "altpath").unwrap_or("").to_string();
        self.use_alt = false;
        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        v.solid = SOLID_NOT;
        v.movetype = MOVETYPE_NONE;
        self.initial_disabled = v.spawnflags & SF_PATH_DISABLED != 0;
        self.disabled = self.initial_disabled;
        let _ = SF_PATH_ALTREVERSE;
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
        if !self.alt_path.is_empty() {
            self.use_alt = match use_type {
                UseType::On => true,
                UseType::Off => false,
                UseType::Toggle | UseType::Set => !self.use_alt,
            };
            let chosen = if self.use_alt {
                self.alt_path.clone()
            } else {
                self.next_target.clone()
            };
            if let Some(v) = eng.ent_mut(id) {
                v.target = chosen;
            }
        } else {
            self.disabled = !self.disabled;
            if let Some(v) = eng.ent_mut(id) {
                if self.disabled {
                    v.spawnflags |= SF_PATH_DISABLED;
                } else {
                    v.spawnflags &= !SF_PATH_DISABLED;
                }
            }
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.disabled = self.initial_disabled;
        self.use_alt = false;
        let next = self.next_target.clone();
        if let Some(v) = eng.ent_mut(id) {
            v.target = next;
            if self.disabled {
                v.spawnflags |= SF_PATH_DISABLED;
            } else {
                v.spawnflags &= !SF_PATH_DISABLED;
            }
        }
    }
}

const SF_TRACK_PASSABLE: u32 = 1 << 3;
const GEAR_SPEEDS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

/// Player-controllable `path_track` train (`func_tracktrain`, `CFuncTrackTrain`).
pub(crate) struct FuncTrackTrain {
    initial_target: String,
    current_target: String,
    max_speed: f32,
    speed: f32,
    damage: f32,
    gear: u8,
    driver_slot: Option<u8>,
    spawn_origin: Vec3,
    spawn_angles: Vec3,
}

impl Default for FuncTrackTrain {
    fn default() -> Self {
        Self {
            initial_target: String::new(),
            current_target: String::new(),
            max_speed: 200.0,
            speed: 0.0,
            damage: 0.0,
            gear: 0,
            driver_slot: None,
            spawn_origin: Vec3::ZERO,
            spawn_angles: Vec3::ZERO,
        }
    }
}

impl FuncTrackTrain {
    fn step_along_track(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        if self.speed.abs() < 1.0 || self.current_target.is_empty() {
            if let Some(v) = eng.ent_mut(id) {
                v.velocity = Vec3::ZERO;
            }
            return;
        }
        let Some(node) = find_by_targetname(eng, None, &self.current_target) else {
            if let Some(v) = eng.ent_mut(id) {
                v.velocity = Vec3::ZERO;
            }
            return;
        };
        let Some((node_org, next, disabled)) = eng
            .ent(node)
            .map(|v| (v.origin, v.target.clone(), v.spawnflags & SF_PATH_DISABLED != 0))
        else {
            return;
        };
        if disabled {
            self.speed = 0.0;
            self.gear = 1;
            if let Some(v) = eng.ent_mut(id) {
                v.velocity = Vec3::ZERO;
            }
            return;
        }
        let cur_org = eng.ent(id).map_or(Vec3::ZERO, |v| v.origin);
        let delta = node_org - cur_org;
        let dist = delta.length();
        if dist < 12.0 && !next.is_empty() {
            self.current_target = next;
        }
        let dir = if dist > 0.1 { delta / dist } else { Vec3::X };
        let vel = dir * self.speed;
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = vel;
            v.nextthink = v.ltime + 0.05;
        }
    }
}

impl GameEntity for FuncTrackTrain {
    fn classname(&self) -> &'static str {
        "func_tracktrain"
    }

    fn object_caps(&self) -> i32 {
        FCAP_ACROSS_TRANSITION | FCAP_IMPULSE_USE
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let Some(base_solid) = mover_solid_kind(eng, id) else {
            return false;
        };
        let speed = key_str(keys, "speed").map(atof).unwrap_or(200.0);
        let start_speed = key_str(keys, "startspeed").map(atof).unwrap_or(0.0);
        self.damage = key_str(keys, "dmg").map(atof).unwrap_or(0.0);
        self.max_speed = if speed > 0.0 { speed } else { 200.0 };
        self.speed = start_speed;
        self.gear = 0;
        self.initial_target = key_str(keys, "target").unwrap_or("").to_string();
        self.current_target = self.initial_target.clone();
        self.driver_slot = None;

        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        self.spawn_origin = v.origin;
        self.spawn_angles = v.angles;
        v.movetype = MOVETYPE_PUSH;
        v.solid = if v.spawnflags & SF_TRACK_PASSABLE != 0 {
            SOLID_NOT
        } else {
            base_solid
        };
        eng.link_entity(id, false);
        if start_speed.abs() > 0.0 && !self.current_target.is_empty() {
            self.step_along_track(eng, id);
        }
        true
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        use_type: UseType,
        value: f32,
    ) {
        if let Some(act) = activator
            && let Some(slot) = player_slot(g, eng, act)
        {
            self.driver_slot = Some(slot as u8);
            let next_gear = if use_type == UseType::Set && (0.0..=5.0).contains(&value) {
                value as u8
            } else if self.gear >= 4 {
                1
            } else {
                (self.gear + 1).max(2)
            };
            self.gear = next_gear;
            let idx = usize::from(self.gear.saturating_sub(1)).min(4);
            self.speed = self.max_speed * GEAR_SPEEDS[idx];
            messages::train(eng, slot as u8, self.gear);
            self.step_along_track(eng, id);
            return;
        }
        // Triggered from map logic (e.g. a button): toggle start/stop.
        if self.speed.abs() > 0.0 {
            self.speed = 0.0;
            if let Some(v) = eng.ent_mut(id) {
                v.velocity = Vec3::ZERO;
                v.nextthink = 0.0;
            }
        } else {
            self.speed = self.max_speed * 0.5;
            self.step_along_track(eng, id);
        }
    }

    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        self.step_along_track(eng, id);
    }

    fn blocked(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if self.damage > 0.0
            && let Some(slot) = player_slot(g, eng, other)
        {
            player::take_damage(g, eng, slot as u8, Some(id), Some(id), self.damage, dmg::CRUSH);
        }
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if let Some(slot) = self.driver_slot.take() {
            messages::train(eng, slot, 0);
        }
        self.speed = 0.0;
        self.gear = 0;
        self.current_target = self.initial_target.clone();
        let (org, ang) = (self.spawn_origin, self.spawn_angles);
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = Vec3::ZERO;
            v.angles = ang;
            v.nextthink = 0.0;
        }
        eng.set_origin(id, org);
    }
}

/// Control console brush linked to a `func_tracktrain` (`func_traincontrols`, `CFuncTrainControls`).
#[derive(Default)]
pub(crate) struct FuncTrainControls {
    target: String,
}

impl GameEntity for FuncTrainControls {
    fn classname(&self) -> &'static str {
        "func_traincontrols"
    }

    fn object_caps(&self) -> i32 {
        FCAP_ACROSS_TRANSITION | FCAP_IMPULSE_USE
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.target = key_str(keys, "target").unwrap_or("").to_string();
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
        v.solid = SOLID_NOT;
        v.movetype = MOVETYPE_NONE;
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
        use_type: UseType,
        value: f32,
    ) {
        if let Some(train) = find_by_targetname(eng, None, &self.target) {
            crate::entity::use_entity(g, eng, train, activator, Some(id), use_type, value);
        }
    }
}

/// CS 1.6 drivable brush vehicle (`func_vehicle`, `CFuncVehicle`, used on `cs_siege`, `de_jeepathon2k`).
pub(crate) struct FuncVehicle {
    initial_target: String,
    max_speed: f32,
    speed: f32,
    damage: f32,
    gear: u8,
    driver_slot: Option<u8>,
    spawn_origin: Vec3,
    spawn_angles: Vec3,
}

impl Default for FuncVehicle {
    fn default() -> Self {
        Self {
            initial_target: String::new(),
            max_speed: 300.0,
            speed: 0.0,
            damage: 0.0,
            gear: 0,
            driver_slot: None,
            spawn_origin: Vec3::ZERO,
            spawn_angles: Vec3::ZERO,
        }
    }
}

impl GameEntity for FuncVehicle {
    fn classname(&self) -> &'static str {
        "func_vehicle"
    }

    fn object_caps(&self) -> i32 {
        FCAP_ACROSS_TRANSITION | FCAP_IMPULSE_USE
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        let Some(solid) = mover_solid_kind(eng, id) else {
            return false;
        };
        let speed = key_str(keys, "speed").map(atof).unwrap_or(300.0);
        self.max_speed = if speed > 0.0 { speed } else { 300.0 };
        self.damage = key_str(keys, "dmg").map(atof).unwrap_or(0.0);
        self.initial_target = key_str(keys, "target").unwrap_or("").to_string();
        self.speed = 0.0;
        self.gear = 0;
        self.driver_slot = None;

        let Some(v) = eng.ent_mut(id) else {
            return false;
        };
        self.spawn_origin = v.origin;
        self.spawn_angles = v.angles;
        v.movetype = MOVETYPE_PUSH;
        v.solid = solid;
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
        use_type: UseType,
        value: f32,
    ) {
        let Some(act) = activator else {
            return;
        };
        let Some(slot) = player_slot(g, eng, act) else {
            return;
        };
        self.driver_slot = Some(slot as u8);
        self.gear = if use_type == UseType::Set && (0.0..=5.0).contains(&value) {
            value as u8
        } else if self.gear >= 4 {
            1
        } else {
            (self.gear + 1).max(2)
        };
        let idx = usize::from(self.gear.saturating_sub(1)).min(4);
        self.speed = self.max_speed * GEAR_SPEEDS[idx];
        messages::train(eng, slot as u8, self.gear);

        let forward = eng
            .ent(id)
            .map_or(Vec3::X, |v| cs_sim::pm::math::angle_vectors(v.angles).0);
        let vel = forward * self.speed;
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = vel;
            v.nextthink = if self.speed.abs() > 0.0 {
                v.ltime + 0.05
            } else {
                0.0
            };
        }
    }

    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        if self.speed.abs() < 1.0 {
            if let Some(v) = eng.ent_mut(id) {
                v.velocity = Vec3::ZERO;
            }
            return;
        }
        let forward = eng
            .ent(id)
            .map_or(Vec3::X, |v| cs_sim::pm::math::angle_vectors(v.angles).0);
        let vel = forward * self.speed;
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = vel;
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
        if let Some(slot) = self.driver_slot.take() {
            messages::train(eng, slot, 0);
        }
        self.speed = 0.0;
        self.gear = 0;
        let (org, ang) = (self.spawn_origin, self.spawn_angles);
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = Vec3::ZERO;
            v.angles = ang;
            v.nextthink = 0.0;
        }
        eng.set_origin(id, org);
    }
}

/// Steering/throttle wheel brush for a `func_vehicle` (`func_vehiclecontrols`, `CFuncVehicleControls`).
#[derive(Default)]
pub(crate) struct FuncVehicleControls {
    target: String,
}

impl GameEntity for FuncVehicleControls {
    fn classname(&self) -> &'static str {
        "func_vehiclecontrols"
    }

    fn object_caps(&self) -> i32 {
        FCAP_ACROSS_TRANSITION | FCAP_IMPULSE_USE
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        self.target = key_str(keys, "target").unwrap_or("").to_string();
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
        v.solid = SOLID_NOT;
        v.movetype = MOVETYPE_NONE;
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
        use_type: UseType,
        value: f32,
    ) {
        if let Some(vehicle) = find_by_targetname(eng, None, &self.target) {
            crate::entity::use_entity(g, eng, vehicle, activator, Some(id), use_type, value);
        }
    }
}
