//! The physics pass of a server frame, as HLDS `SV_Physics` and `SV_RunThink` (ReHLDS
//! `rehlds/engine/sv_phys.cpp`, read 2026-11-21): `start_frame`, then every live entity in
//! index order is run by its `movetype`, which calls its think when it is due.
//!
//! Handled here: `NONE` (think only), `NOCLIP`, `FOLLOW`, `TOSS`, `BOUNCE`, `BOUNCEMISSILE`,
//! `FLY`, `FLYMISSILE`, `STEP`, `PUSHSTEP` and `PUSH` (the pushers: brush entities that carry
//! what stands on them and push what is in their way).
//!
//! Known differences from HLDS:
//! - A `movetype` HLDS ends the process for (`Sys_Error`: 1, 2, `WALK` on a non-player, anything
//!   above 13) behaves as `NONE` here, with one warning per entity index per map.
//! - `SV_Impact` is delivered at the point of impact, as in HLDS: `SV_PushEntity` and
//!   `SV_FlyMove` queue the impact and dispatch the touches at once (physics is driven by the
//!   engine, so this is not a nested game call), then read the entity back; a touch that frees
//!   the entity or marks it `FL_KILLME` ends the move (the source's `ent->free` exit; it also
//!   stops at `FL_KILLME`). A touch in `SV_FlyMove` sees the origin and velocity the move has
//!   reached. `SV_LinkEdict`'s trigger touches are delivered at once the same way.
//! - `groupinfo` (`g_groupmask`) is not modelled; `SV_Trace_Toss` (`PF_TraceToss`) is not ported.
//! - `IS_NAN` of `SV_CheckVelocity` is taken as a test of the exponent bits (assumption: the
//!   macro could not be read; Quake's `IS_NAN` is that test, which an infinity passes too), so a
//!   non-finite velocity or origin component becomes 0. `avelocity` is guarded the same way
//!   before it turns the angles.
//! - Pushers (`SV_Physics_Pusher`, `SV_PushMove`, `SV_PushRotate`, `SV_PushEntity`; ReHLDS
//!   `rehlds/engine/sv_phys.cpp`, read 2026-11-21): `movetime`, `ltime` and the vector maths are
//!   `f64` where the source has `float` (`movetime` is cast to `f32` for the vector maths). A
//!   pusher gets back the `solid` it had before the push, where the source writes `SOLID_BSP`
//!   (the same for every pusher the game makes). The source's `pfnBlocked` runs in the middle
//!   of the push; here it is a normal game call, so the touches queued so far are delivered
//!   before it, and a pusher or entity it removes is skipped when the entities moved so far are
//!   put back. The pass over the entities is the live pool as it is (no `num_edicts`).
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

use crate::edict::{EdictId, EntVars};
use crate::game::GameDll;
use crate::server::{SETTLE_PASSES_MULTI, SETTLE_PASSES_SINGLE, Server};
use crate::trace_ents::{angle_vectors, angle_vectors_transpose};
use crate::world::consts::{
    FL_ALWAYSTHINK, FL_BASEVELOCITY, FL_CLIENT, FL_CONVEYOR, FL_KILLME,
    FL_ONGROUND, MOVETYPE_BOUNCE, MOVETYPE_BOUNCEMISSILE, MOVETYPE_FLY, MOVETYPE_FLYMISSILE,
    MOVETYPE_FOLLOW, MOVETYPE_NONE, MOVETYPE_NOCLIP, MOVETYPE_PUSH, MOVETYPE_PUSHSTEP,
    MOVETYPE_STEP, MOVETYPE_TOSS, MOVETYPE_WALK, SOLID_NOT, SOLID_TRIGGER,
};
use crate::world::free_entity;

/// `a + scale * b`, computed in double precision as `VectorMA` does with `host_frametime`.
pub(crate) fn vector_ma(a: Vec3, scale: f64, b: Vec3) -> Vec3 {
    let component = |a: f32, b: f32| (f64::from(a) + scale * f64::from(b)) as f32;
    Vec3::new(
        component(a.x, b.x),
        component(a.y, b.y),
        component(a.z, b.z),
    )
}

/// Stores `candidate` in `target` when it is finite. Otherwise `target` keeps its value and
/// `offender` (the velocity or angular velocity the candidate came from) is zeroed, as
/// `advance_angles` does. Returns whether the candidate was taken.
pub(crate) fn set_finite(target: &mut Vec3, candidate: Vec3, offender: &mut Vec3) -> bool {
    if candidate.is_finite() {
        *target = candidate;
        true
    } else {
        *offender = Vec3::ZERO;
        false
    }
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
        // After the game's frame start, which may set them.
        self.phys_cvars = crate::physics_move::PhysCvars::read(&self.cvars);
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
            let frametime = self.frametime as f32;
            self.check_moving_ground(id, frametime);
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
            // Each pass gets the touch budget of a frame: the passes and the first frame must
            // not share one.
            self.finish_frame_touches();
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
            MOVETYPE_PUSH => self.physics_pusher(id),
            MOVETYPE_NONE => {
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
            // `check_velocity` does not run for this movetype: a non-finite result (or a
            // velocity that overflows the origin) must not reach the entity.
            let origin = vector_ma(v.origin, frametime, v.velocity);
            set_finite(&mut v.origin, origin, &mut v.velocity);
            let angles = vector_ma(v.angles, frametime, v.avelocity);
            set_finite(&mut v.angles, angles, &mut v.avelocity);
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
        // The parent's values are taken only when finite (the parent may have been poisoned by
        // the game); a non-finite `v_angle` is dropped.
        let v = &mut edict.vars;
        set_finite(&mut v.origin, origin + v.v_angle, &mut v.v_angle);
        let mut ignored = Vec3::ZERO;
        set_finite(&mut v.angles, angles, &mut ignored);
        self.with_engine(|eng| eng.link_entity(id, true));
    }

    /// `SV_RunThink`: calls the game's think of `id` when it is due, `0 < nextthink <= sv_time +
    /// frametime`, unless the entity has `FL_KILLME`. A `nextthink` that is not finite is not
    /// due. A think due in the past runs at `sv_time`; `eng.time()` is the due time for the call
    /// and `nextthink` is 0 before it, so a think that reschedules itself runs again in a later
    /// frame, not in this one. An entity with `FL_KILLME` after the think is freed. Returns
    /// whether the entity is still live.
    pub(crate) fn run_think(&mut self, id: EdictId) -> bool {
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

/// `VectorIsZero`.
fn is_zero(v: Vec3) -> bool {
    v.x == 0.0 && v.y == 0.0 && v.z == 0.0
}

// The pushers. Ported from ReHLDS `rehlds/engine/sv_phys.cpp` (`SV_PushEntity`, `SV_PushMove`,
// `SV_PushRotate`, `SV_Physics_Pusher`) and `rehlds/engine/sv_user.cpp`
// (`SV_CheckMovingGround`), read 2026-11-21.
impl<G: GameDll> Server<G> {
    /// `SV_CheckMovingGround`: the conveyor under an entity adds its belt speed to the entity's
    /// `basevelocity`; an entity that got no base velocity this frame (`FL_BASEVELOCITY` clear)
    /// has what it had folded into its `velocity` and cleared. The flag is cleared at the end.
    /// Called before the movetype switch for every entity the physics runs.
    pub(crate) fn check_moving_ground(&mut self, id: EdictId, frametime: f32) {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return;
        };
        let belt = (v.flags & FL_ONGROUND != 0)
            .then_some(v.groundentity)
            .flatten()
            .and_then(|ground| self.edicts.get(ground))
            .filter(|ground| ground.vars.flags & FL_CONVEYOR != 0)
            .map(|ground| (ground.vars.speed, ground.vars.movedir));
        let Some(v) = self.edicts.get_mut(id).map(|e| &mut e.vars) else {
            return;
        };
        if let Some((speed, movedir)) = belt {
            if v.flags & FL_BASEVELOCITY != 0 {
                v.basevelocity += movedir * speed;
            } else {
                v.basevelocity = movedir * speed;
            }
            v.flags |= FL_BASEVELOCITY;
        }
        if v.flags & FL_BASEVELOCITY == 0 {
            v.velocity += v.basevelocity * (frametime * 0.5 + 1.0);
            v.basevelocity = Vec3::ZERO;
        }
        v.flags &= !FL_BASEVELOCITY;
    }

    /// `SV_Physics_Pusher`: a pusher moves by the time its own clock `ltime` runs, not the
    /// server's. `movetime` is the frame time, or what is left until `nextthink` when that
    /// comes first (never negative); a pusher with no `nextthink` does not move. It rotates when
    /// it has an `avelocity` and translates when it has a `velocity` (when both, the rotation
    /// first and the translation only if the rotation was not blocked, with `ltime` advanced
    /// once), then calls its think when `ltime` has reached `nextthink` (always for
    /// `FL_ALWAYSTHINK`), with the game's time set to the server time.
    pub(crate) fn physics_pusher(&mut self, id: EdictId) {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return;
        };
        let (thinktime, oldltime) = (v.nextthink, v.ltime);
        let movetime = if thinktime < oldltime + self.frametime {
            (thinktime - oldltime).max(0.0)
        } else {
            self.frametime
        };
        if movetime != 0.0 {
            let Some((rotates, moves)) = self
                .edicts
                .get(id)
                .map(|e| (!is_zero(e.vars.avelocity), !is_zero(e.vars.velocity)))
            else {
                return;
            };
            if !rotates {
                self.push_move(id, movetime);
            } else if !moves {
                self.push_rotate(id, movetime);
            } else if self.push_rotate(id, movetime) {
                let Some(edict) = self.edicts.get_mut(id) else {
                    return;
                };
                let savetime = edict.vars.ltime;
                // Back to the local time from before the rotation.
                edict.vars.ltime = oldltime;
                self.push_move(id, movetime);
                if let Some(edict) = self.edicts.get_mut(id)
                    && edict.vars.ltime < savetime
                {
                    edict.vars.ltime = savetime;
                }
            }
        }
        let Some(v) = self.edicts.get_mut(id).map(|e| &mut e.vars) else {
            return;
        };
        for angle in [&mut v.angles.x, &mut v.angles.y, &mut v.angles.z] {
            if *angle < -3600.0 || *angle > 3600.0 {
                *angle %= 3600.0;
            }
        }
        if thinktime > oldltime
            && (v.flags & FL_ALWAYSTHINK != 0 || thinktime <= v.ltime)
            && v.flags & FL_KILLME == 0
        {
            v.nextthink = 0.0;
            self.with_game_at(self.sv_time, |game, eng| game.think(eng, id));
        }
    }

    /// Whether `check` is to be moved by the pusher whose box (after the move) is
    /// `mins..maxs`: not an entity that never moves with a pusher, and either standing on the
    /// pusher or inside its final box and stuck in it.
    fn pushed_by(&mut self, pusher: EdictId, check: EdictId, mins: Vec3, maxs: Vec3) -> bool {
        let Some(c) = self.edicts.get(check).map(|e| &e.vars) else {
            return false;
        };
        if matches!(
            c.movetype,
            MOVETYPE_PUSH | MOVETYPE_NONE | MOVETYPE_FOLLOW | MOVETYPE_NOCLIP
        ) {
            return false;
        }
        if c.flags & FL_ONGROUND != 0 && c.groundentity == Some(pusher) {
            return true; // standing on the pusher: definitely moved
        }
        if c.absmin.cmpge(maxs).any() || c.absmax.cmple(mins).any() {
            return false;
        }
        self.test_position(check)
    }

    /// `SV_TestEntityPosition(id) != NULL`: whether the entity is stuck in something.
    fn test_position(&mut self, id: EdictId) -> bool {
        let stuck = self.with_engine(|eng| eng.test_entity_position(id));
        stuck.is_some()
    }

    /// The test after a push: whether `check` is still stuck, so that the push has failed. An
    /// entity without width never blocks. A non-solid or trigger one that is stuck (a corpse)
    /// is shrunk to a line and does not block either.
    fn push_blocked_by(&mut self, check: EdictId) -> bool {
        if !self.test_position(check) {
            return false;
        }
        let Some(v) = self.edicts.get_mut(check).map(|e| &mut e.vars) else {
            return false;
        };
        if v.mins.x == v.maxs.x {
            return false;
        }
        if v.solid == SOLID_NOT || v.solid == SOLID_TRIGGER {
            v.mins.x = 0.0;
            v.mins.y = 0.0;
            v.maxs.x = 0.0;
            v.maxs.y = 0.0;
            v.maxs.z = v.mins.z;
            return false;
        }
        true
    }

    /// Moves `check` by `push` with the pusher out of the way (`SOLID_NOT` for the push). The
    /// impact touches are delivered inside [`push_entity`](Self::push_entity), so `check` or
    /// the pusher may be gone, or `check` marked `FL_KILLME`, when this returns.
    fn push_clear_of(&mut self, pusher: EdictId, check: EdictId, push: Vec3) {
        let solid = self.edicts.get(pusher).map(|e| e.vars.solid);
        if let Some(edict) = self.edicts.get_mut(pusher) {
            edict.vars.solid = SOLID_NOT;
        }
        self.push_entity(check, push);
        if let (Some(solid), Some(edict)) = (solid, self.edicts.get_mut(pusher)) {
            edict.vars.solid = solid;
        }
    }

    /// The failure of a push: `blocker`, the entity that stayed stuck and where it was, goes
    /// back there (linked with trigger touching), the pusher is put back by `restore_pusher`
    /// and gets its `ltime` back, the touches the push queued are delivered (HLDS delivers
    /// them inside the push, so they precede `blocked`), the game is told the pusher was
    /// blocked, and everything moved so far goes back to where it was. `rotation` is the yaw the pusher turned them by, when it rotates.
    fn push_failed(
        &mut self,
        pusher: EdictId,
        (check, check_origin): (EdictId, Vec3),
        movetime: f64,
        restore_pusher: impl FnOnce(&mut EntVars),
        moved: &[(EdictId, Vec3)],
        rotation: Option<f32>,
    ) {
        if let Some(edict) = self.edicts.get_mut(check) {
            edict.vars.origin = check_origin;
        }
        self.with_engine(|eng| eng.link_entity(check, true));
        if let Some(edict) = self.edicts.get_mut(pusher) {
            restore_pusher(&mut edict.vars);
            edict.vars.ltime -= movetime;
        }
        self.with_engine(|eng| eng.link_entity(pusher, false));
        // The game may remove the pusher or the entity that blocked it; what is left of the
        // entities moved so far is put back either way.
        self.dispatch_touches();
        self.with_game(|game, eng| game.blocked(eng, pusher, check));
        for &(moved_id, from) in moved {
            let Some(v) = self.edicts.get_mut(moved_id).map(|e| &mut e.vars) else {
                continue;
            };
            v.origin = from;
            if let Some(yaw) = rotation {
                if v.flags & FL_CLIENT != 0 {
                    v.avelocity.y = 0.0;
                } else if v.movetype != MOVETYPE_PUSHSTEP {
                    v.angles.y -= yaw;
                }
            }
            self.with_engine(|eng| eng.link_entity(moved_id, false));
        }
    }

    /// `SV_PushMove`: the pusher moves by `velocity * movetime` and `ltime` advances. Every
    /// entity that stands on it (`FL_ONGROUND` with the pusher as `groundentity`) or is stuck in
    /// the final position is moved by the same vector, with its `FL_ONGROUND` cleared unless it
    /// is a `WALK` entity. If one of them is still stuck afterwards, the whole push is undone
    /// and `GameDll::blocked(pusher, that entity)` is called.
    fn push_move(&mut self, pusher: EdictId, movetime: f64) {
        let Some(v) = self.edicts.get_mut(pusher).map(|e| &mut e.vars) else {
            return;
        };
        if is_zero(v.velocity) {
            v.ltime += movetime;
            return;
        }
        let step = v.velocity * movetime as f32;
        let (mins, maxs) = (v.absmin + step, v.absmax + step);
        let pushorig = v.origin;
        // The pusher goes to its final position; a non-finite one is refused and the velocity
        // dropped (`check_velocity` does not run for pushers).
        let destination = v.origin + step;
        v.ltime += movetime;
        if !set_finite(&mut v.origin, destination, &mut v.velocity) {
            return;
        }
        let solid = v.solid;
        self.with_engine(|eng| eng.link_entity(pusher, false));
        if solid == SOLID_NOT {
            return;
        }

        let mut moved: Vec<(EdictId, Vec3)> = Vec::new();
        let mut next = 1;
        loop {
            let Some(check) = self.edicts.iter_live_from(next).next().map(|(id, _)| id) else {
                break;
            };
            next = usize::from(check.index) + 1;
            if !self.pushed_by(pusher, check, mins, maxs) {
                continue;
            }
            let Some(c) = self.edicts.get_mut(check).map(|e| &mut e.vars) else {
                continue;
            };
            if c.movetype != MOVETYPE_WALK {
                c.flags &= !FL_ONGROUND;
            }
            let entorig = c.origin;
            moved.push((check, entorig));

            self.push_clear_of(pusher, check, step);
            // A touch delivered during the push may have removed the pusher.
            if self.edicts.get(pusher).is_none() {
                return;
            }
            if self.push_blocked_by(check) {
                self.push_failed(
                    pusher,
                    (check, entorig),
                    movetime,
                    |v| v.origin = pushorig,
                    &moved,
                    None,
                );
                return;
            }
        }
    }

    /// `SV_PushRotate`: as [`push_move`](Self::push_move) for `avelocity * movetime`. What the
    /// pusher carries is moved around the pusher's origin and turned by the yaw (a client gets
    /// `fixangle` 2 and the yaw in its `avelocity`, a `PUSHSTEP` entity is not turned). `false`
    /// when the push failed. Without an `avelocity` only `ltime` advances.
    fn push_rotate(&mut self, pusher: EdictId, movetime: f64) -> bool {
        let Some(v) = self.edicts.get_mut(pusher).map(|e| &mut e.vars) else {
            return false;
        };
        if is_zero(v.avelocity) {
            v.ltime += movetime;
            return true;
        }
        let amove = v.avelocity * movetime as f32;
        let (forward, right, up) = angle_vectors(v.angles);
        let pushorig = v.angles;
        // The pusher goes to its final angles; non-finite ones are refused and the angular
        // velocity dropped.
        let turned = v.angles + amove;
        v.ltime += movetime;
        if !set_finite(&mut v.angles, turned, &mut v.avelocity) {
            return true;
        }
        let (forward_now, right_now, up_now) = angle_vectors_transpose(v.angles);
        let solid = v.solid;
        self.with_engine(|eng| eng.link_entity(pusher, false));
        // Non-solid pushers cannot push anything.
        if solid == SOLID_NOT {
            return true;
        }

        let mut moved: Vec<(EdictId, Vec3)> = Vec::new();
        let mut next = 1;
        loop {
            let Some(check) = self.edicts.iter_live_from(next).next().map(|(id, _)| id) else {
                break;
            };
            next = usize::from(check.index) + 1;
            let Some(p) = self.edicts.get(pusher).map(|e| &e.vars) else {
                return false;
            };
            let (mins, maxs, pusher_origin) = (p.absmin, p.absmax, p.origin);
            if !self.pushed_by(pusher, check, mins, maxs) {
                continue;
            }
            let Some(c) = self.edicts.get_mut(check).map(|e| &mut e.vars) else {
                continue;
            };
            if c.movetype != MOVETYPE_WALK {
                c.flags &= !FL_ONGROUND;
            }
            let entorig = c.origin;
            moved.push((check, entorig));

            let start = if c.movetype == MOVETYPE_PUSHSTEP {
                (c.absmax + c.absmin) * 0.5 - pusher_origin
            } else {
                c.origin - pusher_origin
            };
            // The destination: where the point is after the pusher turned.
            let local = Vec3::new(forward.dot(start), -right.dot(start), up.dot(start));
            let end = Vec3::new(
                forward_now.dot(local),
                right_now.dot(local),
                up_now.dot(local),
            );

            self.push_clear_of(pusher, check, end - start);
            if self.edicts.get(pusher).is_none() {
                return false;
            }
            if let Some(c) = self.edicts.get_mut(check).map(|e| &mut e.vars)
                && c.movetype != MOVETYPE_PUSHSTEP
            {
                if c.flags & FL_CLIENT != 0 {
                    c.fixangle = 2;
                    c.avelocity.y += amove.y;
                } else {
                    c.angles.y += amove.y;
                }
            }
            if self.push_blocked_by(check) {
                self.push_failed(
                    pusher,
                    (check, entorig),
                    movetime,
                    |v| v.angles = pushorig,
                    &moved,
                    Some(amove.y),
                );
                return false;
            }
        }
        true
    }
}
