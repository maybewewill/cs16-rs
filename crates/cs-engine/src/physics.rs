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

use crate::edict::EdictId;
use crate::game::GameDll;
use crate::server::{SETTLE_PASSES_MULTI, SETTLE_PASSES_SINGLE, Server};
use crate::world::consts::{
    FL_KILLME, MOVETYPE_BOUNCE, MOVETYPE_BOUNCEMISSILE, MOVETYPE_FLY, MOVETYPE_FLYMISSILE,
    MOVETYPE_FOLLOW, MOVETYPE_NONE, MOVETYPE_NOCLIP, MOVETYPE_PUSH, MOVETYPE_PUSHSTEP,
    MOVETYPE_STEP, MOVETYPE_TOSS,
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
