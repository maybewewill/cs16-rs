//! Toss, bounce, fly and step movement: `SV_Physics_Toss`, `SV_Physics_Bounce`,
//! `SV_Physics_Step` and what they call (`SV_CheckVelocity`, `SV_AddGravity`, `SV_ClipVelocity`,
//! `SV_FlyMove`, `SV_PushEntity`), ported from ReHLDS `rehlds/engine/sv_phys.cpp` (read
//! 2026-11-21). The dispatch, the think and the simple movetypes are in `physics.rs`, the water
//! in `physics_water.rs`.

use glam::Vec3;

use crate::collision::CONTENTS_SOLID;
use crate::edict::{EdictId, EntVars};
use crate::game::GameDll;
use crate::physics::vector_ma;
use crate::server::Server;
use crate::trace_ents::{EntTrace, MoveFilter};
use crate::world::consts::{
    FL_CLIENT, FL_FLOAT, FL_FLY, FL_KILLME, FL_MONSTER, FL_MONSTERCLIP, FL_ONGROUND, FL_SWIM,
    MOVETYPE_BOUNCE, MOVETYPE_BOUNCEMISSILE, MOVETYPE_FLY, MOVETYPE_FLYMISSILE, MOVETYPE_PUSHSTEP,
    MOVETYPE_TOSS, MOVETYPE_WALK, SOLID_BSP, SOLID_NOT, SOLID_SLIDEBOX, SOLID_TRIGGER,
};

/// `STOP_EPSILON` of `SV_ClipVelocity`.
const STOP_EPSILON: f32 = 0.1;
/// `MAX_CLIP_PLANES` of `SV_FlyMove`.
const MAX_CLIP_PLANES: usize = 5;

/// `v * scale` for a `scale` that is a double in the source (`host_frametime`).
pub(crate) fn scale_by(v: Vec3, scale: f64) -> Vec3 {
    let component = |v: f32| (f64::from(v) * scale) as f32;
    Vec3::new(component(v.x), component(v.y), component(v.z))
}

/// `SV_ClipVelocity`: `velocity` with the part against `normal` taken out, `overbounce` times
/// (1 slides along the plane, 2 reflects); components within `STOP_EPSILON` of 0 become 0.
pub(crate) fn clip_velocity(velocity: Vec3, normal: Vec3, overbounce: f32) -> Vec3 {
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

/// The cvars the movement physics reads, read once at the start of every physics pass.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PhysCvars {
    gravity: f32,
    max_velocity: f32,
    friction: f32,
    stop_speed: f32,
    bounce: f32,
}

impl Default for PhysCvars {
    /// The values of the original's cvars.
    fn default() -> Self {
        Self {
            gravity: 800.0,
            max_velocity: 2000.0,
            friction: 4.0,
            stop_speed: 100.0,
            bounce: 1.0,
        }
    }
}

impl PhysCvars {
    /// The current values; a cvar that is not registered or not a finite number keeps its
    /// default.
    pub(crate) fn read(cvars: &cs_cvar::CVarRegistry) -> Self {
        let default = Self::default();
        let get = |name: &str, default: f32| {
            cvars
                .get(name)
                .and_then(|c| c.value.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(default)
        };
        Self {
            gravity: get("sv_gravity", default.gravity),
            max_velocity: get("sv_maxvelocity", default.max_velocity),
            friction: get("sv_friction", default.friction),
            stop_speed: get("sv_stopspeed", default.stop_speed),
            bounce: get("sv_bounce", default.bounce),
        }
    }
}

impl<G: GameDll> Server<G> {
    /// `SV_Physics_Toss` (the `REHLDS_FIXES` dispatch, where `TOSS` alone has this function;
    /// ReHLDS `sv_phys.cpp`, read 2026-11-21): the water, the think and then, unless the entity
    /// rests on the ground, gravity and `SV_FlyMove` (overbounce 1.1). Afterwards the entity is
    /// marked as on the ground when a point under a corner of its box is solid world, linked
    /// with trigger touching, and its water is checked again.
    pub(crate) fn physics_toss(&mut self, id: EdictId) {
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
    pub(crate) fn physics_bounce(&mut self, id: EdictId) {
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
            let gravity = self.phys_cvars.gravity;
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
    pub(crate) fn physics_step(&mut self, id: EdictId) {
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
                self.impact_now(id, other);
            }
        }
        self.run_think(id);
        self.check_water_transition(id);
    }

    /// The friction of `SV_Physics_Step`: the horizontal speed loses `control * friction` for
    /// the frame, where `control` is the speed, or `sv_stopspeed` when that is higher, and
    /// `friction` is `sv_friction` times the entity's own (which is then reset to 1).
    pub(crate) fn apply_ground_friction(&mut self, id: EdictId) {
        let sv_friction = self.phys_cvars.friction;
        let stopspeed = self.phys_cvars.stop_speed;
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
    pub(crate) fn rests_on_ground(&mut self, id: EdictId) -> bool {
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

    /// `SV_CheckVelocity`: a velocity or origin component that is not finite becomes 0 (one
    /// `warn` for the entity; the original's `IS_NAN` is taken to catch an infinity too, see
    /// the header of `physics.rs`), a velocity component beyond `sv_maxvelocity` is clamped to
    /// it.
    pub(crate) fn check_velocity(&mut self, id: EdictId) {
        let max = self.phys_cvars.max_velocity;
        let Some(v) = self.vars_mut(id) else {
            return;
        };
        let mut had_nan = false;
        for i in 0..3 {
            if !v.velocity[i].is_finite() {
                v.velocity[i] = 0.0;
                had_nan = true;
            }
            if !v.origin[i].is_finite() {
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
            tracing::warn!(classname = %v.classname, "got a non-finite velocity or origin");
        }
    }

    /// `SV_AddGravity`: `sv_gravity` times the entity's `gravity` (0 means 1) pulls the velocity
    /// down for the frame, and the vertical `basevelocity` is added in and cleared.
    pub(crate) fn add_gravity(&mut self, id: EdictId) {
        let gravity = self.phys_cvars.gravity;
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
    pub(crate) fn advance_angles(&mut self, id: EdictId) {
        let frametime = self.frametime;
        if let Some(v) = self.vars_mut(id) {
            // A non-finite angular velocity (or one that overflows the angles) is dropped, so
            // the angles stay finite.
            if !v.avelocity.is_finite() {
                v.avelocity = Vec3::ZERO;
            }
            let angles = vector_ma(v.angles, frametime, v.avelocity);
            if angles.is_finite() {
                v.angles = angles;
            } else {
                v.avelocity = Vec3::ZERO;
            }
        }
    }

    /// `VectorAdd(velocity, basevelocity, velocity)`.
    pub(crate) fn add_basevelocity(&mut self, id: EdictId) {
        if let Some(v) = self.vars_mut(id) {
            v.velocity += v.basevelocity;
        }
    }

    /// `VectorSubtract(velocity, basevelocity, velocity)`.
    pub(crate) fn sub_basevelocity(&mut self, id: EdictId) {
        if let Some(v) = self.vars_mut(id) {
            v.velocity -= v.basevelocity;
        }
    }

    /// `SV_LinkEdict(ent, TRUE)`; the trigger touches it finds are delivered at once, as in
    /// HLDS. Returns whether the entity is still live and not marked `FL_KILLME` afterwards.
    pub(crate) fn link_touching(&mut self, id: EdictId) -> bool {
        self.with_engine(|eng| eng.link_entity(id, true));
        self.dispatch_touches();
        self.is_running(id)
    }

    /// Whether the entity still exists and is not marked `FL_KILLME`.
    fn is_running(&self, id: EdictId) -> bool {
        self.edicts
            .get(id)
            .is_some_and(|e| e.vars.flags & FL_KILLME == 0)
    }

    /// `SV_Impact(ent, other)` with the touches delivered at once (physics is run by the engine,
    /// so this is not a nested call into the game). Returns whether `ent` is still live and not
    /// marked `FL_KILLME` afterwards; the source's `ent->free` exit.
    fn impact_now(&mut self, ent: EdictId, other: EdictId) -> bool {
        self.impact(ent, other);
        self.dispatch_touches();
        self.is_running(ent)
    }

    pub(crate) fn vars_mut(&mut self, id: EdictId) -> Option<&mut EntVars> {
        self.edicts.get_mut(id).map(|e| &mut e.vars)
    }

    /// `SV_Move` of the box `mins..maxs` for entity `pass`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn move_trace(
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

    /// The ground test at the end of `SV_Physics_Toss` and `SV_Physics_Step`: `FL_ONGROUND` is
    /// set when the point 1 unit under any bottom corner of the box is in solid world.
    pub(crate) fn mark_ground_by_corners(&mut self, id: EdictId) {
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
    /// with trigger touching and runs the impact of what it hit, the touches delivered at once.
    /// The trace; `None` for an entity that is gone, or marked `FL_KILLME` by a touch.
    pub(crate) fn push_entity(&mut self, id: EdictId, push: Vec3) -> Option<EntTrace> {
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
        if !self.link_touching(id) {
            return None;
        }
        if let Some(other) = trace.ent
            && !self.impact_now(id, other)
        {
            return None;
        }
        Some(trace)
    }

    /// `SV_FlyMove`: slides the entity along what it hits for `time` seconds, up to 4 times,
    /// clipping its velocity to every plane met (`bounce` is the overbounce), and runs the
    /// impact of each hit with the touches delivered at once: a touch sees the origin and
    /// velocity reached, may change them (the move goes on from what it left) and ends the move
    /// when it frees the entity. Hitting a flat floor (normal above 0.7) that is a brush, a
    /// `SOLID_SLIDEBOX` entity or a pushable, or being hit by a client, marks the entity as on
    /// the ground; with `REHLDS_FIXES` a `TOSS` entity that is on the ground then stops dead.
    pub(crate) fn fly_move(&mut self, id: EdictId, time: f32, bounce: f32) {
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
        let sv_bounce = self.phys_cvars.bounce;

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
                    // Stops dead; what the touch leaves is the result.
                    if let Some(v) = self.vars_mut(id) {
                        v.origin = origin;
                        v.velocity = Vec3::ZERO;
                    }
                    self.impact_now(id, hit);
                    return;
                }
            }

            // The touch sees where the move has got to, and what it changes is kept.
            if let Some(v) = self.vars_mut(id) {
                v.origin = origin;
                v.velocity = velocity;
            }
            if !self.impact_now(id, hit) {
                return;
            }
            let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
                return;
            };
            (origin, velocity) = (v.origin, v.velocity);

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
}
