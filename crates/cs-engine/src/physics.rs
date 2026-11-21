//! The physics pass of a server frame, as HLDS `SV_Physics` and `SV_RunThink` (ReHLDS
//! `rehlds/engine/sv_phys.cpp`, read 2026-11-21): `start_frame`, then every live entity in
//! index order is run by its `movetype`, which calls its think when it is due.
//!
//! Handled here: `NONE` (think only), `NOCLIP`, `FOLLOW`, `TOSS`, `BOUNCE`, `BOUNCEMISSILE`,
//! `FLY`, `FLYMISSILE`, `STEP` and `PUSHSTEP`. `PUSH` behaves as `NONE` until its physics is
//! ported.
//!
//! Known differences from HLDS:
//! - A `movetype` HLDS ends the process for (`Sys_Error`: 1, 2, `WALK` on a non-player, anything
//!   above 13) behaves as `NONE` here, with one warning per entity index per map.
//! - `SV_CheckMovingGround` (the conveyor and `basevelocity` bookkeeping before the switch) is
//!   not ported.
//! - The game's `touch` is not called inside `SV_FlyMove` and `SV_PushEntity`: the impacts are
//!   queued (see `touch.rs`) and delivered when the entity's step is over, so a touch cannot
//!   change the move that is under way (HLDS: a touch that frees the entity ends the move, one
//!   that changes the velocity or origin changes the rest of it). `SV_FlyMove` has no
//!   `ent->free` exit for the same reason.
//! - `groupinfo` (`g_groupmask`) is not modelled; `SV_Trace_Toss` (`PF_TraceToss`) is not ported.
//! - `IS_NAN` of `SV_CheckVelocity` is taken as a plain NaN test (assumption: the macro reads
//!   the exponent bits, which would zero an infinity as well; an infinite component is clamped
//!   to `sv_maxvelocity` here).
//! - `sv_time` is not advanced while a think runs, so the game's time global reverts to `sv_time`
//!   for every call after it; HLDS leaves the global at the think's due time until the next
//!   `SV_Physics` or `SV_Impact` resets it.
//! - A `nextthink` that is NaN or infinite is never due (C compares false and would fire NaN).
//! - `force_retouch` is lowered while it is above 0 (HLDS: while it is not 0), so a fractional
//!   value cannot keep the pass relinking forever.
//! - The settle passes of map activation (`settle_physics`) assume `runPhysics` is true (a new
//!   map, not a saved game).
//! - The frame filter of `Host_FilterTime` that skips a frame that is too early (`sys_ticrate`)
//!   is not applied: the host decides when to call `Server::frame`.

use glam::Vec3;

use crate::collision::{
    CONTENTS_CURRENT_0, CONTENTS_CURRENT_DOWN, CONTENTS_EMPTY, CONTENTS_LAVA, CONTENTS_SLIME,
    CONTENTS_SOLID, CONTENTS_WATER,
};
use crate::edict::{EdictId, EntVars};
use crate::game::GameDll;
use crate::server::{SETTLE_PASSES_MULTI, SETTLE_PASSES_SINGLE, Server};
use crate::trace_ents::{EntTrace, MoveFilter};
use crate::world::consts::{
    FL_CLIENT, FL_FLOAT, FL_FLY, FL_GODMODE, FL_IMMUNE_LAVA, FL_IMMUNE_SLIME, FL_IMMUNE_WATER,
    FL_INWATER, FL_KILLME, FL_MONSTER, FL_MONSTERCLIP, FL_ONGROUND, FL_SWIM, FL_WATERJUMP,
    MOVETYPE_BOUNCE, MOVETYPE_BOUNCEMISSILE, MOVETYPE_FLY, MOVETYPE_FLYMISSILE, MOVETYPE_FOLLOW,
    MOVETYPE_NONE, MOVETYPE_NOCLIP, MOVETYPE_PUSH, MOVETYPE_PUSHSTEP, MOVETYPE_STEP,
    MOVETYPE_TOSS, MOVETYPE_WALK, SOLID_BSP, SOLID_NOT, SOLID_SLIDEBOX, SOLID_TRIGGER,
};
use crate::world::free_entity;

/// `CONTENTS_TRANSLUCENT` (`rehlds/common/const.h`, read 2026-11-21).
const CONTENTS_TRANSLUCENT: i32 = -15;
/// `STOP_EPSILON` of `SV_ClipVelocity`.
const STOP_EPSILON: f32 = 0.1;
/// `MAX_CLIP_PLANES` of `SV_FlyMove`.
const MAX_CLIP_PLANES: usize = 5;
/// `DEAD_NO`.
const DEAD_NO: i32 = 0;
/// Sound channels and attenuation the water code plays on.
const CHAN_AUTO: u8 = 0;
const CHAN_BODY: u8 = 4;
const ATTN_NORM: f32 = 0.8;

/// `v * scale` for a `scale` that is a double in the source (`host_frametime`).
fn scale_by(v: Vec3, scale: f64) -> Vec3 {
    let component = |v: f32| (f64::from(v) * scale) as f32;
    Vec3::new(component(v.x), component(v.y), component(v.z))
}

/// `SV_ClipVelocity`: `velocity` with the part against `normal` taken out, `overbounce` times
/// (1 slides along the plane, 2 reflects); components within `STOP_EPSILON` of 0 become 0.
fn clip_velocity(velocity: Vec3, normal: Vec3, overbounce: f32) -> Vec3 {
    let backoff = velocity.dot(normal) * overbounce;
    let clip = |velocity: f32, normal: f32| {
        let out = velocity - normal * backoff;
        if out > -STOP_EPSILON && out < STOP_EPSILON {
            0.0
        } else {
            out
        }
    };
    Vec3::new(
        clip(velocity.x, normal.x),
        clip(velocity.y, normal.y),
        clip(velocity.z, normal.z),
    )
}

/// Whether `contents` is one `SV_CheckWater` calls water: from water down to, not including,
/// translucent.
fn in_liquid(contents: i32) -> bool {
    contents <= CONTENTS_WATER && contents > CONTENTS_TRANSLUCENT
}

/// `SV_RecursiveWaterLevel`, as a loop: the height, relative to `origin`, of the water surface
/// found by 6 bisections of `mins..maxs`.
fn water_level_between(
    contents_at: &mut impl FnMut(Vec3) -> i32,
    origin: Vec3,
    mut mins: f32,
    mut maxs: f32,
) -> f32 {
    let mut level = (mins - maxs) * 0.5 + maxs;
    for _ in 0..5 {
        let test = Vec3::new(origin.x, origin.y, origin.z + level);
        if contents_at(test) == CONTENTS_WATER {
            maxs = level;
        } else {
            mins = level;
        }
        level = (mins - maxs) * 0.5 + maxs;
    }
    level
}

/// `a + scale * b`, computed in double precision as `VectorMA` does with `host_frametime`.
fn vector_ma(a: Vec3, scale: f64, b: Vec3) -> Vec3 {
    let component = |a: f32, b: f32| (f64::from(a) + scale * f64::from(b)) as f32;
    Vec3::new(
        component(a.x, b.x),
        component(a.y, b.y),
        component(a.z, b.z),
    )
}

impl<G: GameDll> Server<G> {
    /// `SV_Physics`: lets the game start the frame, then runs every live entity, in ascending
    /// index order, by its `movetype`. The pass follows the pool as it is when each entity's
    /// turn comes, so an entity created during the pass at a higher index than the one running
    /// is run in the same pass, and one freed before its turn is skipped. The world is run
    /// (its think); player slots are not. An entity with `FL_KILLME` set after its turn is
    /// freed. The touches an entity's step queued are dispatched right after it. While `force_retouch` is above 0, every live entity (players included) is linked
    /// with trigger touching before its turn, and the value is lowered by 1 at the end.
    pub(crate) fn run_physics(&mut self) {
        let frametime = self.frametime as f32;
        self.with_game(|game, eng| game.start_frame(eng, frametime));
        let max_clients = usize::from(self.edicts.max_clients());
        let mut next = 0;
        loop {
            let Some(id) = self.edicts.iter_live_from(next).next().map(|(id, _)| id) else {
                break;
            };
            next = usize::from(id.index) + 1;
            if self.world.force_retouch > 0.0 {
                // Forces a retouch even of stationary entities.
                self.with_engine(|eng| eng.link_entity(id, true));
                self.dispatch_touches();
            }
            if id.index != 0 && usize::from(id.index) <= max_clients {
                continue;
            }
            self.run_entity(id);
            // HLDS calls the touches inside the step; here they are delivered before the next
            // entity's turn (the moves of `NOCLIP` and `FOLLOW` link without a game call).
            self.dispatch_touches();
            let killed = self
                .edicts
                .get(id)
                .is_some_and(|e| e.vars.flags & FL_KILLME != 0);
            if killed {
                free_entity(&mut self.edicts, &mut self.world, id, self.sv_time);
            }
        }
        if self.world.force_retouch > 0.0 {
            self.world.force_retouch = (self.world.force_retouch - 1.0).max(0.0);
        }
    }

    /// The physics passes `SV_ActivateServer` (ReHLDS `rehlds/engine/sv_main.cpp`, read
    /// 2026-11-21, with `runPhysics` true) runs after the game activated the map and before the
    /// baselines are cut: with one player slot 2 passes of `host_frametime` 0.1, otherwise 16 of
    /// 0.8. `sv_time` does not advance, so a think due before `sv_time + 0.1` (or `+ 0.8`) runs
    /// now and its effects are in the baselines.
    pub(crate) fn settle_physics(&mut self) {
        let (passes, frametime) = if self.cfg.max_clients <= 1 {
            (SETTLE_PASSES_SINGLE, 0.1)
        } else {
            (SETTLE_PASSES_MULTI, 0.8)
        };
        self.frametime = frametime;
        for _ in 0..passes {
            self.run_physics();
        }
        self.frametime = 0.0;
    }

    /// The `switch (ent->v.movetype)` of `SV_Physics`.
    fn run_entity(&mut self, id: EdictId) {
        let Some(movetype) = self.edicts.get(id).map(|e| e.vars.movetype) else {
            return;
        };
        match movetype {
            MOVETYPE_NOCLIP => self.physics_noclip(id),
            MOVETYPE_FOLLOW => self.physics_follow(id),
            MOVETYPE_TOSS => self.physics_toss(id),
            MOVETYPE_BOUNCE | MOVETYPE_BOUNCEMISSILE | MOVETYPE_FLY | MOVETYPE_FLYMISSILE => {
                self.physics_bounce(id);
            }
            MOVETYPE_STEP | MOVETYPE_PUSHSTEP => self.physics_step(id),
            // `PUSH` is not ported yet: it thinks and nothing else.
            MOVETYPE_NONE | MOVETYPE_PUSH => {
                self.run_think(id);
            }
            other => {
                if self.warned_movetypes.insert(id.index) {
                    self.movetype_warnings += 1;
                    tracing::warn!(
                        entity = id.index,
                        movetype = other,
                        "movetype HLDS rejects; the entity only thinks"
                    );
                }
                self.run_think(id);
            }
        }
    }

    /// `SV_Physics_Noclip`: after its think the entity moves by `velocity` and `avelocity` for
    /// the frame time, with no collision, and is relinked without touching triggers.
    fn physics_noclip(&mut self, id: EdictId) {
        if !self.run_think(id) {
            return;
        }
        let frametime = self.frametime;
        if let Some(edict) = self.edicts.get_mut(id) {
            let v = &mut edict.vars;
            v.origin = vector_ma(v.origin, frametime, v.velocity);
            v.angles = vector_ma(v.angles, frametime, v.avelocity);
        }
        self.with_engine(|eng| eng.link_entity(id, false));
    }

    /// `SV_Physics_Follow`: after its think the entity takes the origin of its `aiment` plus its
    /// own `v_angle` and the angles of the `aiment`, and is relinked with trigger touching.
    /// Without an `aiment` (or one that no longer resolves) it becomes `MOVETYPE_NONE`.
    fn physics_follow(&mut self, id: EdictId) {
        if !self.run_think(id) {
            return;
        }
        let parent = self
            .edicts
            .get(id)
            .and_then(|e| e.vars.aiment)
            .and_then(|p| self.edicts.get(p))
            .map(|p| (p.vars.origin, p.vars.angles));
        let Some(edict) = self.edicts.get_mut(id) else {
            return;
        };
        let Some((origin, angles)) = parent else {
            edict.vars.movetype = MOVETYPE_NONE;
            return;
        };
        edict.vars.origin = origin + edict.vars.v_angle;
        edict.vars.angles = angles;
        self.with_engine(|eng| eng.link_entity(id, true));
    }

    /// `SV_Physics_Toss` (the `REHLDS_FIXES` dispatch, where `TOSS` alone has this function;
    /// ReHLDS `sv_phys.cpp`, read 2026-11-21): the water, the think and then, unless the entity
    /// rests on the ground, gravity and `SV_FlyMove` (overbounce 1.1). Afterwards the entity is
    /// marked as on the ground when a point under a corner of its box is solid world, linked
    /// with trigger touching, and its water is checked again.
    fn physics_toss(&mut self, id: EdictId) {
        self.check_water(id);
        if !self.run_think(id) || self.rests_on_ground(id) {
            return;
        }
        self.check_velocity(id);
        self.add_gravity(id);
        self.advance_angles(id);
        self.add_basevelocity(id);
        self.check_velocity(id);
        self.fly_move(id, self.frametime as f32, 1.1);
        self.check_velocity(id);
        self.sub_basevelocity(id);
        self.check_velocity(id);
        self.mark_ground_by_corners(id);
        self.link_touching(id);
        self.check_water_transition(id);
    }

    /// `SV_Physics_Bounce` (ReHLDS `sv_phys.cpp`, read 2026-11-21): `BOUNCE`, `BOUNCEMISSILE`,
    /// `FLY` and `FLYMISSILE`. Only `BOUNCE` falls. The entity is pushed along its velocity for
    /// the frame by `SV_PushEntity`; what it hits reflects its velocity (`2 - friction` for
    /// `BOUNCE`, 2 for `BOUNCEMISSILE`, 1 for the flyers), and a floor flat enough stops it, or
    /// makes it roll, or throws it up again for what is left of the frame.
    fn physics_bounce(&mut self, id: EdictId) {
        self.check_water(id);
        if !self.run_think(id) || self.rests_on_ground(id) {
            return;
        }
        self.check_velocity(id);
        let Some(movetype) = self.edicts.get(id).map(|e| e.vars.movetype) else {
            return;
        };
        if !matches!(
            movetype,
            MOVETYPE_FLY | MOVETYPE_FLYMISSILE | MOVETYPE_BOUNCEMISSILE
        ) {
            self.add_gravity(id);
        }
        self.advance_angles(id);
        self.add_basevelocity(id);
        self.check_velocity(id);
        let frametime = self.frametime;
        let Some(v) = self.vars_mut(id) else {
            return;
        };
        let step = scale_by(v.velocity, frametime);
        v.velocity -= v.basevelocity;

        let Some(trace) = self.push_entity(id, step) else {
            return;
        };
        self.check_velocity(id);
        let Some(v) = self.vars_mut(id) else {
            return;
        };
        if trace.trace.all_solid {
            // Trapped in another solid.
            v.avelocity = Vec3::ZERO;
            v.velocity = Vec3::ZERO;
            return;
        }
        if trace.trace.fraction == 1.0 {
            self.check_water_transition(id);
            return;
        }
        let backoff = match movetype {
            MOVETYPE_BOUNCE => 2.0 - v.friction,
            MOVETYPE_BOUNCEMISSILE => 2.0,
            _ => 1.0,
        };
        v.velocity = clip_velocity(v.velocity, trace.trace.plane_normal, backoff);

        if trace.trace.plane_normal.z > 0.7 {
            let gravity = self.cvar_or("sv_gravity", 800.0);
            let Some(v) = self.vars_mut(id) else {
                return;
            };
            let moving = v.velocity + v.basevelocity;
            let speed_squared = moving.dot(moving);
            if f64::from(moving.z) < f64::from(gravity) * frametime {
                // Rolling on the ground: static friction.
                v.flags |= FL_ONGROUND;
                v.velocity.z = 0.0;
                v.groundentity = trace.ent;
            }
            let bounces = movetype == MOVETYPE_BOUNCE || movetype == MOVETYPE_BOUNCEMISSILE;
            if speed_squared < 900.0 || !bounces {
                v.flags |= FL_ONGROUND;
                v.groundentity = trace.ent;
                v.velocity = Vec3::ZERO;
                v.avelocity = Vec3::ZERO;
            } else {
                // What is left of the frame, spent on the way up.
                let scale = ((1.0 - f64::from(trace.trace.fraction)) * frametime * 0.9) as f32;
                let step = v.velocity * scale + v.basevelocity * scale;
                if self.push_entity(id, step).is_none() {
                    return;
                }
            }
        }
        self.check_water_transition(id);
    }

    /// `SV_Physics_Step` (ReHLDS `sv_phys.cpp`, read 2026-11-21): `STEP` and `PUSHSTEP`, a
    /// monster that falls when nothing holds it and whose walking the game makes by steps.
    /// `PF_WaterMove`, then gravity (not for a flyer, for a swimmer only out of the water, not
    /// for an entity that was on the ground and not in the water), buoyancy for `FL_FLOAT`, the
    /// ground friction of an entity that was on the ground (`sv_friction` times its own
    /// `friction`, `sv_stopspeed`), the move itself, and the think. An entity that does not
    /// move is traced against what it stands in while `force_retouch` is on.
    fn physics_step(&mut self, id: EdictId) {
        self.water_move(id);
        self.check_velocity(id);
        let Some(was_on_ground) = self
            .edicts
            .get(id)
            .map(|e| e.vars.flags & FL_ONGROUND != 0)
        else {
            return;
        };
        let in_water = self.check_water(id);
        let frametime = self.frametime;

        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return;
        };
        if v.flags & FL_FLOAT != 0 && v.waterlevel > 0 {
            let skin = v.skin;
            let buoyancy = (f64::from(self.submerged(id)) * f64::from(skin) * frametime) as f32;
            self.add_gravity(id);
            if let Some(v) = self.vars_mut(id) {
                v.velocity.z += buoyancy;
            }
        }
        // Gravity, except for flyers and for swimmers in the water.
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return;
        };
        let falls = !was_on_ground
            && v.flags & FL_FLY == 0
            && (v.flags & FL_SWIM == 0 || v.waterlevel <= 0)
            && !in_water;
        if falls {
            self.add_gravity(id);
        }

        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return;
        };
        let moving = v.velocity != Vec3::ZERO || v.basevelocity != Vec3::ZERO;
        let alive = v.health > 0.0;
        if moving {
            if let Some(v) = self.vars_mut(id) {
                v.flags &= !FL_ONGROUND;
            }
            // Friction; a dead monster that is not completely on the ground slides.
            if was_on_ground && (alive || self.with_engine(|eng| eng.check_bottom(id))) {
                self.apply_ground_friction(id);
            }
            self.add_basevelocity(id);
            self.check_velocity(id);
            self.fly_move(id, frametime as f32, 1.0);
            self.check_velocity(id);
            self.sub_basevelocity(id);
            self.check_velocity(id);
            self.mark_ground_by_corners(id);
            self.link_touching(id);
        } else if self.world.force_retouch != 0.0 {
            // The "hentacle impact code": what the entity stands in touches it.
            let (origin, mins, maxs, monster_clip) = (
                v.origin,
                v.mins,
                v.maxs,
                v.flags & FL_MONSTERCLIP != 0,
            );
            let hit =
                self.move_trace(origin, mins, maxs, origin, MoveFilter::Normal, id, monster_clip);
            if (hit.trace.fraction < 1.0 || hit.trace.start_solid)
                && let Some(other) = hit.ent
            {
                self.impact(id, other);
            }
        }
        if self.run_think(id) {
            self.check_water_transition(id);
        }
    }

    /// The friction of `SV_Physics_Step`: the horizontal speed loses `control * friction` for
    /// the frame, where `control` is the speed, or `sv_stopspeed` when that is higher, and
    /// `friction` is `sv_friction` times the entity's own (which is then reset to 1).
    fn apply_ground_friction(&mut self, id: EdictId) {
        let sv_friction = self.cvar_or("sv_friction", 4.0);
        let stopspeed = self.cvar_or("sv_stopspeed", 100.0);
        let frametime = self.frametime;
        let Some(v) = self.vars_mut(id) else {
            return;
        };
        let speed =
            f64::from(v.velocity.x * v.velocity.x + v.velocity.y * v.velocity.y).sqrt() as f32;
        if speed == 0.0 {
            return;
        }
        let friction = sv_friction * v.friction;
        v.friction = 1.0;
        let control = if speed < stopspeed { stopspeed } else { speed };
        let newspeed =
            (f64::from(speed) - frametime * f64::from(control) * f64::from(friction)) as f32;
        let newspeed = newspeed.max(0.0) / speed;
        v.velocity.x *= newspeed;
        v.velocity.y *= newspeed;
    }

    /// The part of `SV_Physics_Toss` and `SV_Physics_Bounce` after the think: clears
    /// `FL_ONGROUND` when the entity moves up or its ground is gone, or is a monster or a
    /// client, and returns whether it now rests (on the ground, no velocity, no base velocity)
    /// and is left alone this frame. An entity at rest also loses its `avelocity`.
    fn rests_on_ground(&mut self, id: EdictId) -> bool {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return true;
        };
        let ground_moves = v
            .groundentity
            .and_then(|ground| self.edicts.get(ground))
            .is_none_or(|ground| ground.vars.flags & (FL_MONSTER | FL_CLIENT) != 0);
        let leaves_ground = v.velocity.z > 0.0 || ground_moves;
        let Some(v) = self.vars_mut(id) else {
            return true;
        };
        if leaves_ground {
            v.flags &= !FL_ONGROUND;
        }
        if v.flags & FL_ONGROUND != 0 && v.velocity == Vec3::ZERO {
            v.avelocity = Vec3::ZERO;
            return v.basevelocity == Vec3::ZERO;
        }
        false
    }

    /// `SV_CheckVelocity`: a NaN velocity or origin component becomes 0 (one `warn` for the
    /// entity), a velocity component beyond `sv_maxvelocity` is clamped to it.
    fn check_velocity(&mut self, id: EdictId) {
        let max = self.cvar_or("sv_maxvelocity", 2000.0);
        let Some(v) = self.vars_mut(id) else {
            return;
        };
        let mut had_nan = false;
        for i in 0..3 {
            if v.velocity[i].is_nan() {
                v.velocity[i] = 0.0;
                had_nan = true;
            }
            if v.origin[i].is_nan() {
                v.origin[i] = 0.0;
                had_nan = true;
            }
            if v.velocity[i] > max {
                v.velocity[i] = max;
            } else if v.velocity[i] < -max {
                v.velocity[i] = -max;
            }
        }
        if had_nan {
            tracing::warn!(classname = %v.classname, "got a NaN velocity or origin");
        }
    }

    /// `SV_AddGravity`: `sv_gravity` times the entity's `gravity` (0 means 1) pulls the velocity
    /// down for the frame, and the vertical `basevelocity` is added in and cleared.
    fn add_gravity(&mut self, id: EdictId) {
        let gravity = self.cvar_or("sv_gravity", 800.0);
        let frametime = self.frametime;
        if let Some(v) = self.vars_mut(id) {
            let own = if v.gravity != 0.0 { v.gravity } else { 1.0 };
            v.velocity.z = (f64::from(v.velocity.z) - f64::from(own * gravity) * frametime) as f32;
            v.velocity.z =
                (f64::from(v.velocity.z) + f64::from(v.basevelocity.z) * frametime) as f32;
            v.basevelocity.z = 0.0;
        }
        self.check_velocity(id);
    }

    /// `VectorMA(angles, host_frametime, avelocity, angles)`.
    fn advance_angles(&mut self, id: EdictId) {
        let frametime = self.frametime;
        if let Some(v) = self.vars_mut(id) {
            v.angles = vector_ma(v.angles, frametime, v.avelocity);
        }
    }

    /// `VectorAdd(velocity, basevelocity, velocity)`.
    fn add_basevelocity(&mut self, id: EdictId) {
        if let Some(v) = self.vars_mut(id) {
            v.velocity += v.basevelocity;
        }
    }

    /// `VectorSubtract(velocity, basevelocity, velocity)`.
    fn sub_basevelocity(&mut self, id: EdictId) {
        if let Some(v) = self.vars_mut(id) {
            v.velocity -= v.basevelocity;
        }
    }

    /// `SV_LinkEdict(ent, TRUE)`.
    fn link_touching(&mut self, id: EdictId) {
        self.with_engine(|eng| eng.link_entity(id, true));
    }

    fn vars_mut(&mut self, id: EdictId) -> Option<&mut EntVars> {
        self.edicts.get_mut(id).map(|e| &mut e.vars)
    }

    /// The value of cvar `name`, or `default` when it is not registered or not a finite number.
    fn cvar_or(&self, name: &str, default: f32) -> f32 {
        self.cvars
            .get(name)
            .and_then(|c| c.value.trim().parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(default)
    }

    /// `SV_PointContents` at `p`.
    fn contents(&mut self, p: Vec3) -> i32 {
        self.with_engine(|eng| eng.point_contents_ents(p))
    }

    /// `SV_Move` of the box `mins..maxs` for entity `pass`.
    #[allow(clippy::too_many_arguments)]
    fn move_trace(
        &mut self,
        start: Vec3,
        mins: Vec3,
        maxs: Vec3,
        end: Vec3,
        filter: MoveFilter,
        pass: EdictId,
        monster_clip: bool,
    ) -> EntTrace {
        self.with_engine(|eng| {
            eng.move_counted(start, mins, maxs, end, filter, Some(pass), monster_clip)
                .0
        })
    }

    /// `SV_StartSound(0, ent, channel, sample, 255, attenuation, 0, PITCH_NORM)`.
    fn start_sound(&mut self, id: EdictId, channel: u8, sample: &str, attenuation: f32) {
        if self.with_engine(|eng| eng.emit_sound(id, channel, sample, 1.0, attenuation, 100)) {
            self.drain_messages();
        }
    }

    /// One of the four wade sounds, as `RandomLong(0, 3)` picks (assumption: any generator
    /// will do; the engine's own is not reproduced).
    fn random_wade_sound(&mut self) -> &'static str {
        let mut x = self.phys_rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.phys_rng = x;
        match x % 4 {
            0 => "player/pl_wade1.wav",
            1 => "player/pl_wade2.wav",
            2 => "player/pl_wade3.wav",
            _ => "player/pl_wade4.wav",
        }
    }

    /// The ground test at the end of `SV_Physics_Toss` and `SV_Physics_Step`: `FL_ONGROUND` is
    /// set when the point 1 unit under any bottom corner of the box is in solid world.
    fn mark_ground_by_corners(&mut self, id: EdictId) {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return;
        };
        let (mins, maxs) = (v.origin + v.mins, v.origin + v.maxs);
        let z = mins.z - 1.0;
        let solid = self.with_engine(|eng| {
            [(mins.x, mins.y), (mins.x, maxs.y), (maxs.x, mins.y), (maxs.x, maxs.y)]
                .into_iter()
                .any(|(x, y)| eng.point_contents_ents(Vec3::new(x, y, z)) == CONTENTS_SOLID)
        });
        if solid && let Some(v) = self.vars_mut(id) {
            v.flags |= FL_ONGROUND;
        }
    }

    /// `SV_PushEntity`: moves the entity by `push` as far as it goes (a missile also stops at
    /// the monsters around it, a trigger or a non-solid entity only at brush models), links it
    /// with trigger touching and runs the impact of what it hit. The trace, `None` for an
    /// entity that is gone.
    fn push_entity(&mut self, id: EdictId, push: Vec3) -> Option<EntTrace> {
        let v = self.edicts.get(id).map(|e| &e.vars)?;
        let (origin, mins, maxs) = (v.origin, v.mins, v.maxs);
        let filter = if v.movetype == MOVETYPE_FLYMISSILE {
            MoveFilter::Missile
        } else if v.solid == SOLID_TRIGGER || v.solid == SOLID_NOT {
            MoveFilter::NoMonsters
        } else {
            MoveFilter::Normal
        };
        let monster_clip = v.flags & FL_MONSTERCLIP != 0;
        let trace = self.move_trace(origin, mins, maxs, origin + push, filter, id, monster_clip);
        if trace.trace.fraction != 0.0
            && let Some(v) = self.vars_mut(id)
        {
            v.origin = trace.trace.end_pos;
        }
        self.link_touching(id);
        if let Some(other) = trace.ent {
            self.impact(id, other);
        }
        Some(trace)
    }

    /// `SV_FlyMove`: slides the entity along what it hits for `time` seconds, up to 4 times,
    /// clipping its velocity to every plane met (`bounce` is the overbounce), and queues the
    /// impact of each hit. Hitting a flat floor (normal above 0.7) that is a brush, a
    /// `SOLID_SLIDEBOX` entity or a pushable, or being hit by a client, marks the entity as on
    /// the ground; with `REHLDS_FIXES` a `TOSS` entity that is on the ground then stops dead.
    fn fly_move(&mut self, id: EdictId, time: f32, bounce: f32) {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return;
        };
        let (mut origin, mut velocity) = (v.origin, v.velocity);
        let (mins, maxs, movetype) = (v.mins, v.maxs, v.movetype);
        let mut original_velocity = velocity;
        let primal_velocity = velocity;
        let filter = if v.solid == SOLID_TRIGGER || v.solid == SOLID_NOT {
            MoveFilter::NoMonsters
        } else {
            MoveFilter::Normal
        };
        let monster_clip = v.flags & FL_MONSTERCLIP != 0;
        let walk_friction = v.friction;
        let sv_bounce = self.cvar_or("sv_bounce", 1.0);

        let mut planes = [Vec3::ZERO; MAX_CLIP_PLANES];
        let mut numplanes = 0usize;
        let mut time_left = time;
        for _bump in 0..MAX_CLIP_PLANES - 1 {
            if velocity == Vec3::ZERO {
                break;
            }
            // Assume the entity can move all the way.
            let end = origin + velocity * time_left;
            let trace = self.move_trace(origin, mins, maxs, end, filter, id, monster_clip);
            if trace.trace.all_solid {
                // Trapped in another solid.
                velocity = Vec3::ZERO;
                break;
            }
            if trace.trace.fraction > 0.0 {
                let there = trace.trace.end_pos;
                let test = self.move_trace(there, mins, maxs, there, filter, id, monster_clip);
                if !test.trace.all_solid {
                    // Actually covered some distance.
                    origin = there;
                    original_velocity = velocity;
                    numplanes = 0;
                }
            }
            if trace.trace.fraction == 1.0 {
                break;
            }
            let Some(hit) = trace.ent else {
                // `Sys_Error` in the source; nothing was hit that could be named.
                break;
            };
            let normal = trace.trace.plane_normal;

            // Stop if on ground.
            if normal.z > 0.7 {
                let hit_vars = self.edicts.get(hit).map(|e| &e.vars);
                let grounds = hit_vars.is_some_and(|h| {
                    h.solid == SOLID_BSP
                        || h.solid == SOLID_SLIDEBOX
                        || h.movetype == MOVETYPE_PUSHSTEP
                        || h.flags & FL_CLIENT != 0
                });
                if grounds && let Some(v) = self.vars_mut(id) {
                    v.flags |= FL_ONGROUND;
                    v.groundentity = Some(hit);
                }
                let on_ground = self
                    .edicts
                    .get(id)
                    .is_some_and(|e| e.vars.flags & FL_ONGROUND != 0);
                if on_ground && movetype == MOVETYPE_TOSS {
                    velocity = Vec3::ZERO;
                    self.impact(id, hit);
                    break;
                }
            }

            self.impact(id, hit);

            // Reduce the time left by the fraction covered.
            time_left -= time_left * trace.trace.fraction;

            // Out of planes to clip against: stop.
            if numplanes >= MAX_CLIP_PLANES {
                velocity = Vec3::ZERO;
                break;
            }
            planes[numplanes] = normal;
            numplanes += 1;

            let on_ground = self
                .edicts
                .get(id)
                .is_some_and(|e| e.vars.flags & FL_ONGROUND != 0);
            if numplanes == 1 && movetype == MOVETYPE_WALK && (!on_ground || walk_friction != 1.0) {
                let d = if planes[0].z <= 0.7 {
                    (1.0 - walk_friction) * sv_bounce + 1.0
                } else {
                    1.0
                };
                // Go along this plane.
                velocity = clip_velocity(original_velocity, planes[0], d);
                original_velocity = velocity;
            } else {
                // Make the original velocity parallel to all the planes.
                let mut along = None;
                for (i, plane) in planes[..numplanes].iter().enumerate() {
                    let clipped = clip_velocity(original_velocity, *plane, bounce);
                    let fits = planes[..numplanes]
                        .iter()
                        .enumerate()
                        .all(|(j, other)| j == i || clipped.dot(*other) >= 0.0);
                    if fits {
                        along = Some(clipped);
                        break;
                    }
                }
                if let Some(clipped) = along {
                    velocity = clipped;
                } else {
                    // Go along the crease.
                    if numplanes != 2 {
                        break;
                    }
                    let dir = planes[0].cross(planes[1]);
                    velocity = dir * dir.dot(velocity);
                }
                // Against the original direction: stop dead, to avoid oscillating in a corner.
                if velocity.dot(primal_velocity) <= 0.0 {
                    velocity = Vec3::ZERO;
                    break;
                }
            }
        }
        if let Some(v) = self.vars_mut(id) {
            v.origin = origin;
            v.velocity = velocity;
        }
    }

    /// `SV_CheckWater`: sets `watertype` and `waterlevel` (1 feet, 2 waist, 3 eyes) from what
    /// is at the entity's feet, middle and view point, and returns whether it is deeper than
    /// the feet. Water with a current pushes the `basevelocity` (the current contents of the
    /// world are reported as water by `SV_PointContents`, so that branch is out of reach in
    /// the source and here).
    fn check_water(&mut self, id: EdictId) -> bool {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return false;
        };
        let (absmin, absmax, view_ofs) = (v.absmin, v.absmax, v.view_ofs);
        let mut point = Vec3::new(
            (absmax.x + absmin.x) * 0.5,
            (absmax.y + absmin.y) * 0.5,
            absmin.z + 1.0,
        );
        let mut watertype = CONTENTS_EMPTY;
        let mut waterlevel = 0;
        let mut current = None;

        let cont = self.contents(point);
        if in_liquid(cont) {
            watertype = cont;
            waterlevel = 1;
            if absmin.z == absmax.z {
                waterlevel = 3;
            } else {
                point.z = (absmax.z + absmin.z) * 0.5;
                if in_liquid(self.contents(point)) {
                    waterlevel = 2;
                    point += view_ofs;
                    if in_liquid(self.contents(point)) {
                        waterlevel = 3;
                    }
                }
            }
            if (CONTENTS_CURRENT_DOWN..=CONTENTS_CURRENT_0).contains(&cont) {
                const CURRENTS: [Vec3; 6] = [
                    Vec3::X,
                    Vec3::Y,
                    Vec3::NEG_X,
                    Vec3::NEG_Y,
                    Vec3::Z,
                    Vec3::NEG_Z,
                ];
                current = CURRENTS
                    .get((CONTENTS_CURRENT_0 - cont) as usize)
                    .map(|dir| *dir * (150.0 * waterlevel as f32 / 3.0));
            }
        }
        if let Some(v) = self.vars_mut(id) {
            v.watertype = watertype;
            v.waterlevel = waterlevel;
            if let Some(push) = current {
                v.basevelocity += push;
            }
        }
        waterlevel > 1
    }

    /// `SV_CheckWaterTransition`: after a move, tracks the entity's water. A fresh entity
    /// (`watertype` 0) takes what it is in; leaving water plays `player/pl_wade2.wav`; entering
    /// it plays `player/pl_wade1.wav` and halves the vertical velocity; in the water the level
    /// is worked out again.
    fn check_water_transition(&mut self, id: EdictId) {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return;
        };
        let (absmin, absmax, view_ofs, watertype) = (v.absmin, v.absmax, v.view_ofs, v.watertype);
        let mut point = Vec3::new(
            (absmax.x + absmin.x) * 0.5,
            (absmax.y + absmin.y) * 0.5,
            absmin.z + 1.0,
        );
        let cont = self.contents(point);
        if watertype == 0 {
            // Just spawned here.
            if let Some(v) = self.vars_mut(id) {
                v.watertype = cont;
                v.waterlevel = 1;
            }
            return;
        }
        if !in_liquid(cont) {
            if watertype != CONTENTS_EMPTY {
                self.start_sound(id, CHAN_AUTO, "player/pl_wade2.wav", 1.0);
            }
            if let Some(v) = self.vars_mut(id) {
                v.watertype = CONTENTS_EMPTY;
                v.waterlevel = 0;
            }
            return;
        }
        if watertype == CONTENTS_EMPTY {
            // Just crossed into water.
            self.start_sound(id, CHAN_AUTO, "player/pl_wade1.wav", 1.0);
            if let Some(v) = self.vars_mut(id) {
                v.velocity.z *= 0.5;
            }
        }
        let mut waterlevel = 1;
        if absmin.z == absmax.z {
            // A point entity.
            waterlevel = 3;
        } else {
            point.z = (absmin.z + absmax.z) * 0.5;
            if in_liquid(self.contents(point)) {
                waterlevel = 2;
                if in_liquid(self.contents(point + view_ofs)) {
                    waterlevel = 3;
                }
            }
        }
        if let Some(v) = self.vars_mut(id) {
            v.watertype = cont;
            v.waterlevel = waterlevel;
        }
    }

    /// `SV_Submerged`: how much of the entity's height is under water, from its `waterlevel`.
    fn submerged(&mut self, id: EdictId) -> f32 {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return 0.0;
        };
        let (absmin, absmax, level) = (v.absmin, v.absmax, v.waterlevel);
        let height = v.maxs.z - v.mins.z;
        let center = (absmax + absmin) * 0.5;
        let feet = absmin.z - center.z;
        self.with_engine(|eng| {
            let mut contents_at = |p: Vec3| eng.point_contents_ents(p);
            let top = absmax.z - center.z;
            match level {
                1 => water_level_between(&mut contents_at, center, 0.0, feet) - feet,
                2 => water_level_between(&mut contents_at, center, top, 0.0) - feet,
                3 => {
                    let head = Vec3::new(center.x, center.y, absmax.z);
                    if contents_at(head) == CONTENTS_WATER {
                        height
                    } else {
                        water_level_between(&mut contents_at, center, top, 0.0) - feet
                    }
                }
                _ => 0.0,
            }
        })
    }

    /// `PF_WaterMove` (ReHLDS `sv_phys.cpp`, read 2026-11-21): the water movement of a monster,
    /// before its step. It keeps the drowning clock (`air_finished`, `pain_finished`, `dmg`),
    /// the lava and slime damage clock (`dmgtime`), plays the wade sounds when `FL_INWATER`
    /// changes and slows the entity down in the water by `0.8 * waterlevel` per second. The
    /// game reads the clocks and does the damage.
    fn water_move(&mut self, id: EdictId) {
        let time = self.sv_time;
        let frametime = self.frametime;
        let Some(v) = self.vars_mut(id) else {
            return;
        };
        if v.movetype == MOVETYPE_NOCLIP {
            v.air_finished = (time + 12.0) as f32;
            return;
        }
        if v.health < 0.0 {
            return;
        }
        let drownlevel = if v.deadflag == DEAD_NO { 3.0 } else { 1.0 };
        let (waterlevel, watertype, flags) = (v.waterlevel, v.watertype, v.flags);
        let level = waterlevel as f32;

        if flags & (FL_IMMUNE_WATER | FL_GODMODE) == 0 {
            if (flags & FL_SWIM != 0 && level < drownlevel) || level >= drownlevel {
                if f64::from(v.air_finished) < time && f64::from(v.pain_finished) < time {
                    v.dmg += 2.0;
                    if v.dmg > 15.0 {
                        v.dmg = 10.0;
                    }
                    v.pain_finished = (time + 1.0) as f32;
                }
            } else {
                v.dmg = 2.0;
                v.air_finished = (time + 12.0) as f32;
            }
        }

        if waterlevel == 0 {
            // Leaving the water.
            v.air_finished = (time + 12.0) as f32;
            let was_in_water = flags & FL_INWATER != 0;
            if was_in_water {
                v.flags &= !FL_INWATER;
                let sound = self.random_wade_sound();
                self.start_sound(id, CHAN_BODY, sound, ATTN_NORM);
            }
            return;
        }

        if watertype == CONTENTS_LAVA {
            if flags & (FL_IMMUNE_LAVA | FL_GODMODE) == 0 && f64::from(v.dmgtime) < time {
                v.dmgtime = if time <= f64::from(v.radsuit_finished) {
                    (time + 1.0) as f32
                } else {
                    (time + 0.2) as f32
                };
            }
        } else if watertype == CONTENTS_SLIME
            && flags & (FL_IMMUNE_SLIME | FL_GODMODE) == 0
            && f64::from(v.dmgtime) < time
            && f64::from(v.radsuit_finished) < time
        {
            v.dmgtime = (time + 1.0) as f32;
        }

        if flags & FL_INWATER == 0 {
            v.dmgtime = 0.0;
            v.flags |= FL_INWATER;
            if watertype == CONTENTS_WATER {
                let sound = self.random_wade_sound();
                self.start_sound(id, CHAN_BODY, sound, ATTN_NORM);
            }
        }

        if flags & FL_WATERJUMP == 0
            && let Some(v) = self.vars_mut(id)
        {
            let drag = -0.8 * f64::from(waterlevel) * frametime;
            v.velocity = vector_ma(v.velocity, drag, v.velocity);
        }
    }

    /// `SV_RunThink`: calls the game's think of `id` when it is due, `0 < nextthink <= sv_time +
    /// frametime`, unless the entity has `FL_KILLME`. A `nextthink` that is not finite is not
    /// due. A think due in the past runs at `sv_time`; `eng.time()` is the due time for the call
    /// and `nextthink` is 0 before it, so a think that reschedules itself runs again in a later
    /// frame, not in this one. An entity with `FL_KILLME` after the think is freed. Returns
    /// whether the entity is still live.
    fn run_think(&mut self, id: EdictId) -> bool {
        let Some(vars) = self.edicts.get(id).map(|e| &e.vars) else {
            return false;
        };
        if vars.flags & FL_KILLME == 0 {
            let at = vars.nextthink;
            if !at.is_finite() || at <= 0.0 || at > self.sv_time + self.frametime {
                return true;
            }
            let time = at.max(self.sv_time);
            if let Some(edict) = self.edicts.get_mut(id) {
                edict.vars.nextthink = 0.0;
            }
            self.with_game_at(time, |game, eng| game.think(eng, id));
        }
        if self
            .edicts
            .get(id)
            .is_some_and(|e| e.vars.flags & FL_KILLME != 0)
        {
            free_entity(&mut self.edicts, &mut self.world, id, self.sv_time);
        }
        self.edicts.get(id).is_some()
    }
}
