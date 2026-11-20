//! Counter-Strike 1.6 `pm_shared` movement physics ported cleanly to Rust.
//!
//! Preserves authentic GoldSrc feel: ground friction, sharp counter-strafing (`sv_accelerate 5`),
//! air-strafing & bunnyhopping (`sv_airaccelerate 10`, wishspeed cap 30), 18-unit step climbing,
//! crouching (`hull 1 <-> hull 3`, duck-jump), ladders, fall damage, and footsteps.

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::trace::{BspCollision, CONTENTS_SOLID, Hull, TraceResult};

/// What [`step_player_move`] asks of the world it moves in: exactly the queries it makes of
/// [`BspCollision`], so the legacy path and the engine-backed game share one movement code.
pub trait MoveWorld {
    /// Traces `hull` from `start` to `end` against everything the player collides with.
    fn trace_hull(&self, start: Vec3, end: Vec3, hull: Hull) -> TraceResult;
    /// Whether a player of `hull` standing at `pos` touches a ladder.
    fn is_on_ladder(&self, pos: Vec3, hull: Hull) -> bool;
    /// `CONTENTS_*` of the world at `point` for `hull`.
    fn point_contents(&self, point: Vec3, hull: Hull) -> i32;
}

impl MoveWorld for BspCollision {
    fn trace_hull(&self, start: Vec3, end: Vec3, hull: Hull) -> TraceResult {
        BspCollision::trace_hull(self, start, end, hull)
    }

    fn is_on_ladder(&self, pos: Vec3, hull: Hull) -> bool {
        BspCollision::is_on_ladder(self, pos, hull)
    }

    fn point_contents(&self, point: Vec3, hull: Hull) -> i32 {
        BspCollision::point_contents(self, point, hull)
    }
}

pub const VEC_VIEW: f32 = 17.0;
pub const VEC_DUCK_VIEW: f32 = 12.0;
pub const STOP_EPSILON: f32 = 0.1;
pub const MAX_CLIP_PLANES: usize = 5;
pub const OVERCLIP: f32 = 1.0;
pub const FALL_SAFE_SPEED: f32 = 580.0;
pub const FALL_FATAL_SPEED: f32 = 1024.0;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MoveParams {
    pub gravity: f32,
    pub accelerate: f32,
    pub air_accelerate: f32,
    pub friction: f32,
    pub stop_speed: f32,
    pub step_size: f32,
    pub max_speed: f32,
}

impl Default for MoveParams {
    fn default() -> Self {
        Self {
            gravity: 800.0,
            accelerate: 5.0,
            air_accelerate: 10.0,
            friction: 4.0,
            stop_speed: 100.0,
            step_size: 18.0,
            max_speed: 320.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct MoveInputCmd {
    /// Forward/backward wish (`-1.0..=1.0`).
    pub forward: f32,
    /// Right/left wish (`-1.0..=1.0`).
    pub right: f32,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub jump: bool,
    pub duck: bool,
    pub walk: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PlayerMoveState {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub view_ofs_z: f32,
    pub on_ground: bool,
    pub ducking: bool,
    pub on_ladder: bool,
    pub noclip: bool,
    pub old_jump_held: bool,
    pub fall_velocity: f32,
    pub step_distance: f32,
}

impl Default for PlayerMoveState {
    fn default() -> Self {
        Self {
            origin: Vec3::new(0.0, 0.0, 64.0),
            velocity: Vec3::ZERO,
            view_ofs_z: VEC_VIEW,
            on_ground: false,
            ducking: false,
            on_ladder: false,
            noclip: false,
            old_jump_held: false,
            fall_velocity: 0.0,
            step_distance: 0.0,
        }
    }
}

impl PlayerMoveState {
    pub fn at_spawn(origin: Vec3) -> Self {
        Self {
            origin,
            view_ofs_z: VEC_VIEW,
            ..Self::default()
        }
    }

    pub fn eye_pos(&self) -> Vec3 {
        self.origin + Vec3::new(0.0, 0.0, self.view_ofs_z)
    }

    pub fn hull(&self) -> Hull {
        if self.ducking {
            Hull::Crouched
        } else {
            Hull::Standing
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MoveStepEvents {
    pub jumped: bool,
    pub landed: bool,
    pub fall_damage: f32,
    pub footstep: bool,
    pub ladder_step: bool,
}

/// Advance player movement by one fixed timestep `dt` (normally `0.01s` at 100 Hz).
pub fn step_player_move<W: MoveWorld + ?Sized>(
    state: &mut PlayerMoveState,
    cmd: &MoveInputCmd,
    weapon_max_speed: f32,
    params: &MoveParams,
    world: &W,
    dt: f32,
) -> MoveStepEvents {
    let mut events = MoveStepEvents::default();

    // 1. Noclip debug mode
    if state.noclip {
        let yaw = cmd.yaw_deg.to_radians();
        let pitch = cmd.pitch_deg.to_radians();
        let fwd = Vec3::new(yaw.cos() * pitch.cos(), yaw.sin() * pitch.cos(), -pitch.sin());
        let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
        let mut wish = fwd * cmd.forward + right * cmd.right;
        if cmd.jump {
            wish.z += 1.0;
        }
        if cmd.duck {
            wish.z -= 1.0;
        }
        let spd = if cmd.walk { 180.0 } else { 480.0 };
        state.velocity = if wish.length_squared() > 1e-6 {
            wish.normalize() * spd
        } else {
            Vec3::ZERO
        };
        state.origin += state.velocity * dt;
        state.on_ground = false;
        state.on_ladder = false;
        state.view_ofs_z = VEC_VIEW;
        return events;
    }

    // 2. Ducking (`hull 1 <-> hull 3` with headroom check & duck-jump tuck)
    pm_duck(state, cmd.duck, world, dt);

    // 3. Categorize position (ground & ladder check)
    categorize_position(state, world, &mut events);

    // 4. Ladder movement
    if state.on_ladder {
        pm_ladder_move(state, cmd, world, dt, &mut events);
        state.old_jump_held = cmd.jump;
        return events;
    }

    // 5. Effective maxspeed (weapon speed, walk shift, crouch)
    let mut max_speed = weapon_max_speed.min(params.max_speed);
    if cmd.walk && state.on_ground {
        max_speed *= 0.52;
    }
    if state.ducking {
        max_speed *= 0.333;
    }

    // 6. Jump (on ground; allows clean bunnyhop when jump is pressed/tapped!)
    if cmd.jump && state.on_ground && !state.old_jump_held {
        state.on_ground = false;
        // Classic GoldSrc jump impulse: sqrt(2 * g * 45) ≈ 268.328
        state.velocity.z = (2.0 * params.gravity * 45.0).sqrt();
        // Half-gravity start (GoldSrc half-step integration)
        state.velocity.z -= params.gravity * 0.5 * dt;
        events.jumped = true;
    }
    state.old_jump_held = cmd.jump;

    // 7. Horizontal basis from yaw
    let yaw = cmd.yaw_deg.to_radians();
    let forward = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
    let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
    let wish_raw = forward * cmd.forward + right * cmd.right;
    let wish_len = wish_raw.length();
    let wish_dir = if wish_len > 1e-5 {
        wish_raw / wish_len
    } else {
        Vec3::ZERO
    };
    let wish_speed = max_speed * wish_len.min(1.0);

    let prev_pos = state.origin;

    if state.on_ground {
        state.velocity.z = 0.0;
        pm_friction(state, params, dt);
        pm_accelerate(state, wish_dir, wish_speed, params.accelerate, dt);
        state.velocity.z = 0.0;
        if state.velocity.length_squared() > 1e-4 {
            pm_step_slide_move(state, params.step_size, world, dt);
        }
    } else {
        // Half gravity before move, half after (exact Quake/GoldSrc symplectic split!)
        state.velocity.z -= params.gravity * 0.5 * dt;
        pm_air_accelerate(state, wish_dir, wish_speed, params.air_accelerate, dt);
        pm_fly_move(state, world, dt);
        state.velocity.z -= params.gravity * 0.5 * dt;
    }

    // Track falling speed for authentic CS 1.6 fall damage
    if !state.on_ground && state.velocity.z < 0.0 {
        state.fall_velocity = state.fall_velocity.max(-state.velocity.z);
    }

    // Re-categorize ground state after move
    categorize_position(state, world, &mut events);

    // Footstep cadence: silent when walking (`shift`) or crouching (`ctrl`)!
    if state.on_ground && !state.ducking && !cmd.walk {
        let horiz_vel = Vec3::new(state.velocity.x, state.velocity.y, 0.0).length();
        if horiz_vel > 135.0 {
            let moved = Vec3::new(state.origin.x - prev_pos.x, state.origin.y - prev_pos.y, 0.0).length();
            state.step_distance += moved;
            if state.step_distance >= 108.0 {
                state.step_distance -= 108.0;
                events.footstep = true;
            }
        }
    }

    events
}

fn pm_duck<W: MoveWorld + ?Sized>(
    state: &mut PlayerMoveState,
    want_duck: bool,
    world: &W,
    dt: f32,
) {
    if want_duck {
        if !state.ducking {
            state.ducking = true;
            if state.on_ground {
                // Standing hull min.z = -36, crouched hull min.z = -18 -> lower origin by 18 so feet stay planted
                state.origin.z -= 18.0;
            } else {
                // In air: tucking feet up gives the classic CS 1.6 duck-jump boost!
                state.origin.z += 18.0;
            }
        }
    } else if state.ducking {
        // Attempt to stand up: shift origin +18 if on ground, check standing hull fits
        let candidate = if state.on_ground {
            state.origin + Vec3::new(0.0, 0.0, 18.0)
        } else {
            state.origin - Vec3::new(0.0, 0.0, 18.0)
        };
        let tr = world.trace_hull(candidate, candidate, Hull::Standing);
        if !tr.start_solid && !tr.all_solid {
            state.ducking = false;
            state.origin = candidate;
        }
    }

    let target_eye = if state.ducking { VEC_DUCK_VIEW } else { VEC_VIEW };
    let blend = (dt * 18.0).min(1.0);
    state.view_ofs_z += (target_eye - state.view_ofs_z) * blend;
}

fn categorize_position<W: MoveWorld + ?Sized>(
    state: &mut PlayerMoveState,
    world: &W,
    events: &mut MoveStepEvents,
) {
    state.on_ladder = world.is_on_ladder(state.origin, state.hull());
    if state.on_ladder {
        state.on_ground = false;
        state.fall_velocity = 0.0;
        return;
    }

    // Moving rapidly upwards cannot be grounded
    if state.velocity.z > 180.0 {
        state.on_ground = false;
        return;
    }

    let down = state.origin - Vec3::new(0.0, 0.0, 2.0);
    let tr = world.trace_hull(state.origin, down, state.hull());
    let was_grounded = state.on_ground;

    if tr.fraction < 1.0 && !tr.all_solid && tr.plane_normal.z >= 0.7 {
        state.on_ground = true;
        if !was_grounded {
            events.landed = true;
            if state.fall_velocity > FALL_SAFE_SPEED {
                let ratio = (state.fall_velocity - FALL_SAFE_SPEED)
                    / (FALL_FATAL_SPEED - FALL_SAFE_SPEED);
                events.fall_damage = (ratio * 100.0).clamp(1.0, 100.0);
            }
            state.fall_velocity = 0.0;
        }
        if !tr.start_solid && tr.fraction > 0.0 {
            state.origin = tr.end_pos;
        }
    } else {
        state.on_ground = false;
    }
}

fn pm_friction(state: &mut PlayerMoveState, params: &MoveParams, dt: f32) {
    let speed = Vec3::new(state.velocity.x, state.velocity.y, 0.0).length();
    if speed < 0.1 {
        state.velocity.x = 0.0;
        state.velocity.y = 0.0;
        return;
    }
    let control = speed.max(params.stop_speed);
    let drop = control * params.friction * dt;
    let new_speed = (speed - drop).max(0.0);
    let scale = new_speed / speed;
    state.velocity.x *= scale;
    state.velocity.y *= scale;
}

fn pm_accelerate(
    state: &mut PlayerMoveState,
    wish_dir: Vec3,
    wish_speed: f32,
    accel: f32,
    dt: f32,
) {
    let current_speed = state.velocity.dot(wish_dir);
    let add_speed = wish_speed - current_speed;
    if add_speed <= 0.0 {
        return;
    }
    let accel_speed = (accel * dt * wish_speed).min(add_speed);
    state.velocity += wish_dir * accel_speed;
}

/// Authentic Quake / CS 1.6 `PM_AirAccelerate`: caps `wishspd` at `30.0`, enabling true air-strafing!
fn pm_air_accelerate(
    state: &mut PlayerMoveState,
    wish_dir: Vec3,
    wish_speed: f32,
    air_accel: f32,
    dt: f32,
) {
    let capped = wish_speed.min(30.0);
    let current_speed = state.velocity.dot(wish_dir);
    let add_speed = capped - current_speed;
    if add_speed <= 0.0 {
        return;
    }
    let accel_speed = (air_accel * wish_speed * dt).min(add_speed);
    state.velocity += wish_dir * accel_speed;
}

fn pm_ladder_move<W: MoveWorld + ?Sized>(
    state: &mut PlayerMoveState,
    cmd: &MoveInputCmd,
    world: &W,
    dt: f32,
    events: &mut MoveStepEvents,
) {
    let yaw = cmd.yaw_deg.to_radians();
    let pitch = cmd.pitch_deg.to_radians();
    let fwd = Vec3::new(yaw.cos() * pitch.cos(), yaw.sin() * pitch.cos(), -pitch.sin());
    let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);

    if cmd.jump && !state.old_jump_held {
        // Jump off ladder
        state.velocity = Vec3::new(yaw.cos() * 200.0, yaw.sin() * 200.0, 220.0);
        state.on_ladder = false;
        pm_fly_move(state, world, dt);
        events.jumped = true;
        return;
    }

    let climb_speed = if cmd.walk { 100.0 } else { 200.0 };
    let mut vel = fwd * (cmd.forward * climb_speed) + right * (cmd.right * climb_speed * 0.75);
    if cmd.forward.abs() < 0.05 && cmd.right.abs() < 0.05 {
        vel = Vec3::ZERO;
    } else if cmd.forward > 0.1 && pitch < 0.3 {
        // Looking mostly forward/up while holding W climbs smoothly up
        vel.z = vel.z.max(climb_speed * 0.75);
    }

    let prev = state.origin;
    state.velocity = vel;
    pm_fly_move(state, world, dt);
    state.step_distance += (state.origin - prev).length();
    if state.step_distance >= 85.0 && !cmd.walk {
        state.step_distance -= 85.0;
        events.ladder_step = true;
    }
}

/// Multi-plane slide movement (`PM_FlyMove` from Half-Life SDK).
fn pm_fly_move<W: MoveWorld + ?Sized>(state: &mut PlayerMoveState, world: &W, dt: f32) {
    let mut time_left = dt;
    let primal_vel = state.velocity;
    let mut planes = [Vec3::ZERO; MAX_CLIP_PLANES];
    let mut num_planes = 0usize;
    let hull = state.hull();

    for _bump in 0..4 {
        if state.velocity.length_squared() < 1e-6 {
            break;
        }
        let end = state.origin + state.velocity * time_left;
        let tr = world.trace_hull(state.origin, end, hull);

        if tr.all_solid {
            state.velocity = Vec3::ZERO;
            return;
        }
        if tr.fraction > 0.0 {
            state.origin = tr.end_pos;
            num_planes = 0;
        }
        if tr.fraction >= 1.0 {
            break;
        }

        time_left -= time_left * tr.fraction;
        if num_planes >= MAX_CLIP_PLANES {
            state.velocity = Vec3::ZERO;
            break;
        }
        planes[num_planes] = tr.plane_normal;
        num_planes += 1;

        // Reflect velocity against all accumulated clip planes
        let mut solved = false;
        for i in 0..num_planes {
            let mut v = clip_velocity(state.velocity, planes[i], OVERCLIP);
            let mut ok = true;
            for &pl in &planes[..num_planes] {
                if v.dot(pl) < 0.0 {
                    ok = false;
                    break;
                }
            }
            if ok {
                state.velocity = v;
                solved = true;
                break;
            }
            // Also try sliding along the crease of 2 planes
            if num_planes == 2 {
                let crease = planes[0].cross(planes[1]);
                let len_sq = crease.length_squared();
                if len_sq > 1e-6 {
                    let dir = crease / len_sq.sqrt();
                    v = dir * dir.dot(state.velocity);
                    state.velocity = v;
                    solved = true;
                    break;
                }
            }
        }

        if !solved {
            state.velocity = Vec3::ZERO;
            break;
        }

        if state.velocity.dot(primal_vel) <= 0.0 {
            state.velocity = Vec3::ZERO;
            break;
        }
    }
}

/// Ground slide + stair climbing up to `step_size` (18 units in CS 1.6).
fn pm_step_slide_move<W: MoveWorld + ?Sized>(
    state: &mut PlayerMoveState,
    step_size: f32,
    world: &W,
    dt: f32,
) {
    let orig_pos = state.origin;
    let orig_vel = state.velocity;

    // 1. Try normal slide first
    pm_fly_move(state, world, dt);
    let down_pos = state.origin;
    let down_vel = state.velocity;

    // 2. Try stepping up `step_size`, sliding, then stepping back down
    let hull = state.hull();
    let up_target = orig_pos + Vec3::new(0.0, 0.0, step_size);
    let up_tr = world.trace_hull(orig_pos, up_target, hull);
    if up_tr.all_solid || up_tr.start_solid {
        return;
    }

    state.origin = up_tr.end_pos;
    state.velocity = orig_vel;
    pm_fly_move(state, world, dt);

    let down_target = state.origin - Vec3::new(0.0, 0.0, step_size + 2.0);
    let down_tr = world.trace_hull(state.origin, down_target, hull);
    if !down_tr.all_solid && down_tr.fraction < 1.0 && down_tr.plane_normal.z >= 0.7 {
        state.origin = down_tr.end_pos;
    } else if !down_tr.all_solid {
        // Keep step height only if we moved further horizontally
        state.origin = down_tr.end_pos;
    }

    let down_dist = (down_pos.x - orig_pos.x).powi(2) + (down_pos.y - orig_pos.y).powi(2);
    let up_dist = (state.origin.x - orig_pos.x).powi(2) + (state.origin.y - orig_pos.y).powi(2);

    if down_dist >= up_dist {
        state.origin = down_pos;
        state.velocity = down_vel;
    } else {
        // Keep vertical velocity from non-stepped move so stairs don't launch you
        state.velocity.z = down_vel.z;
    }
}

#[inline]
pub fn clip_velocity(v: Vec3, normal: Vec3, overbounce: f32) -> Vec3 {
    let backoff = v.dot(normal) * overbounce;
    let mut out = v - normal * backoff;
    if out.x.abs() < STOP_EPSILON {
        out.x = 0.0;
    }
    if out.y.abs() < STOP_EPSILON {
        out.y = 0.0;
    }
    if out.z.abs() < STOP_EPSILON {
        out.z = 0.0;
    }
    out
}

/// Find a safe standing position slightly above `raw_spawn` and snapped cleanly onto the floor.
pub fn snap_spawn_to_floor<W: MoveWorld + ?Sized>(raw_spawn: Vec3, world: &W) -> Vec3 {
    // Try `raw_spawn` or slightly elevated in case mapper placed spawn near floor
    let mut start = raw_spawn;
    for lift in [0.0f32, 8.0, 18.0, 36.0] {
        let candidate = raw_spawn + Vec3::new(0.0, 0.0, lift);
        if world.point_contents(candidate, Hull::Standing) != CONTENTS_SOLID {
            start = candidate;
            break;
        }
    }
    let tr = world.trace_hull(start, start - Vec3::new(0.0, 0.0, 128.0), Hull::Standing);
    if !tr.all_solid && tr.fraction < 1.0 {
        tr.end_pos + Vec3::new(0.0, 0.0, 0.5)
    } else {
        start
    }
}
