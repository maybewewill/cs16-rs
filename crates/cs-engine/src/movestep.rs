//! Entity walking: the movement calls of the original server's monster code.
//!
//! Ported from ReHLDS `rehlds/engine/sv_move.cpp` (`SV_CheckBottom`, `SV_movetest`,
//! `SV_movestep`, `SV_StepDirection`, `SV_FlyDirection`, `SV_FixCheckBottom`,
//! `SV_NewChaseDir2`, `SV_MoveToOrigin_I`) and `rehlds/engine/pr_cmds.cpp` (`PF_walkmove_I`,
//! `PF_droptofloor_I`), read 2026-11-21.
//!
//! Left out, on purpose:
//! - the `groupinfo` filter (`g_groupmask`): the engine has no such field yet;
//! - `SV_NewChaseDir`, `SV_CloseEnough` and `SV_ReachedGoal` (`NOXREF` in ReHLDS: nothing calls
//!   them);
//! - the `pr_global_struct->trace_normal` the source's comments mention: the engine has no trace
//!   globals.
//!
//! Deviations kept on purpose:
//! - `RandomLong(0, 1)` of `SV_NewChaseDir2` is a xorshift32 of the engine's own, state
//!   `WorldLinks::rng`, set to a constant whenever a map's world is created. The original's
//!   generator is a time-seeded global, so its choices cannot be reproduced anyway; this one
//!   makes the walking of a map repeatable (tests count on that).
//! - Input that is not finite or lies outside the world (see `in_world`) moves nothing and
//!   reports failure; the original would trace through garbage.
//! - A stale `enemy` counts as no enemy in the flying branch of `SV_movestep`.

use glam::Vec3;

use crate::api_entities::{in_world, refused_trace};
use crate::collision::{CONTENTS_EMPTY, CONTENTS_SOLID};
use crate::edict::EdictId;
use crate::game::EngineApi;
use crate::trace_ents::{EntTrace, MoveFilter};
use crate::world::consts::{FL_FLY, FL_MONSTERCLIP, FL_ONGROUND, FL_PARTIALGROUND, FL_SWIM};

/// What `PF_walkmove_I` does with the move (its `iMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkMoveMode {
    /// `SV_movestep` and link (mode 0 and any mode not listed here).
    Normal,
    /// `SV_movetest` and link (mode 1): the same walk, but the floor search sees the world
    /// model only, not other entities.
    WorldOnly,
    /// `SV_movestep` without linking (mode 2). As in the source the origin still changes when
    /// the move succeeds; only the area tree and the touches are left alone.
    CheckOnly,
}

/// `MOVE_NORMAL` of `SV_MoveToOrigin_I`'s `iMoveType`; any other value flies straight at the
/// goal.
pub const MOVE_NORMAL: i32 = 0;

/// `DI_NODIR`.
const DI_NODIR: f32 = -1.0;

/// `anglemod`: `a` wrapped to `[0, 360)` in steps of 360/65536.
fn anglemod(a: f32) -> f32 {
    (360.0 / 65536.0) * ((((a * (65536.0 / 360.0)) as i32) & 65535) as f32)
}

impl EngineApi<'_> {
    /// `PF_walkmove_I`: tries to walk entity `id` `dist` units along `yaw` (degrees), by the
    /// rules of `mode`. Does nothing, and returns false, unless the entity is on the ground,
    /// flies or swims, or when the numbers are not finite or the entity is outside the world.
    /// True when the entity moved.
    pub fn walk_move(&mut self, id: EdictId, yaw: f32, dist: f32, mode: WalkMoveMode) -> bool {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return false;
        };
        if v.flags & (FL_SWIM | FL_FLY | FL_ONGROUND) == 0
            || !in_world(v.origin)
            || !yaw.is_finite()
            || !dist.is_finite()
        {
            return false;
        }
        let mv = yaw_move(yaw, dist);
        match mode {
            WalkMoveMode::WorldOnly => self.ground_step(id, mv, true, true),
            WalkMoveMode::CheckOnly => self.move_step(id, mv, false),
            WalkMoveMode::Normal => self.move_step(id, mv, true),
        }
    }

    /// `PF_droptofloor_I`: drops entity `id` up to 256 units onto the floor and links it
    /// (without touches), setting `FL_ONGROUND` and `groundentity`. 1 when it landed, 0 when
    /// there is no floor within 256 units (or the entity is outside the world or stale), -1
    /// when it starts stuck in something solid.
    pub fn drop_to_floor(&mut self, id: EdictId) -> i32 {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return 0;
        };
        let (origin, mins, maxs) = (v.origin, v.mins, v.maxs);
        let monster_clip = v.flags & FL_MONSTERCLIP != 0;
        if !in_world(origin) {
            return 0;
        }
        let end = Vec3::new(origin.x, origin.y, origin.z - 256.0);
        let (t, _) = self.move_counted(
            origin,
            mins,
            maxs,
            end,
            MoveFilter::Normal,
            Some(id),
            monster_clip,
        );
        if t.trace.all_solid {
            return -1;
        }
        if t.trace.fraction == 1.0 {
            return 0;
        }
        if let Some(e) = self.edicts.get_mut(id) {
            e.vars.origin = t.trace.end_pos;
        }
        self.link_entity(id, false);
        if let Some(e) = self.edicts.get_mut(id) {
            e.vars.flags |= FL_ONGROUND;
            e.vars.groundentity = t.ent;
        }
        1
    }

    /// `SV_MoveToOrigin_I`: one step of `dist` units of entity `id` toward `goal`. With
    /// `move_type` [`MOVE_NORMAL`] it walks along its `ideal_yaw` and, when that fails, picks a
    /// new direction (`SV_NewChaseDir2`); any other `move_type` walks (or, for a flyer or
    /// swimmer, flies) straight at the goal. Does nothing for an entity that is not on the
    /// ground and neither flies nor swims.
    pub fn move_to_origin(&mut self, id: EdictId, goal: Vec3, dist: f32, move_type: i32) {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return;
        };
        if v.flags & (FL_FLY | FL_SWIM | FL_ONGROUND) == 0
            || !in_world(v.origin)
            || !in_world(goal)
            || !dist.is_finite()
        {
            return;
        }
        let (origin, flags, ideal_yaw) = (v.origin, v.flags, v.ideal_yaw);
        if move_type == MOVE_NORMAL {
            if !self.step_direction(id, ideal_yaw, dist) {
                self.new_chase_dir2(id, goal, dist);
            }
        } else {
            let mut dir = goal - origin;
            if flags & (FL_SWIM | FL_FLY) == 0 {
                dir.z = 0.0;
            }
            let dir = dir.normalize_or_zero() * dist;
            self.fly_direction(id, dir);
        }
    }

    /// `SV_CheckBottom`: false if any part of the bottom of the entity is off an edge that is
    /// not a staircase.
    fn check_bottom(&self, id: EdictId) -> bool {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return false;
        };
        let monster_clip = v.flags & FL_MONSTERCLIP != 0;
        let mins = v.origin + v.mins;
        let maxs = v.origin + v.maxs;
        let stepsize = self.cvar_f32("sv_stepsize");

        // If all of the points under the corners are solid world, don't bother with the
        // tougher checks.
        let corner = |x: bool, y: bool, z: f32| {
            Vec3::new(
                if x { maxs.x } else { mins.x },
                if y { maxs.y } else { mins.y },
                z,
            )
        };
        let all_solid = [(false, false), (false, true), (true, false), (true, true)]
            .iter()
            .all(|&(x, y)| self.point_contents_ents(corner(x, y, mins.z - 1.0)) == CONTENTS_SOLID);
        if all_solid {
            return true;
        }

        // Check it for real: the midpoint must be within a step of the bottom.
        let mid_xy = Vec3::new((mins.x + maxs.x) * 0.5, (mins.y + maxs.y) * 0.5, 0.0);
        let start = Vec3::new(mid_xy.x, mid_xy.y, mins.z + stepsize);
        let stop = Vec3::new(mid_xy.x, mid_xy.y, start.z - 2.0 * stepsize);
        let point = |from: Vec3, to: Vec3| {
            self.move_counted(
                from,
                Vec3::ZERO,
                Vec3::ZERO,
                to,
                MoveFilter::NoMonsters,
                Some(id),
                monster_clip,
            )
            .0
            .trace
        };
        let trace = point(start, stop);
        if trace.fraction == 1.0 {
            return false;
        }
        let mid = trace.end_pos.z;

        // The corners must be within a step of the midpoint.
        for (x, y) in [(false, false), (false, true), (true, false), (true, true)] {
            let start = corner(x, y, start.z);
            let stop = corner(x, y, stop.z);
            let trace = point(start, stop);
            if trace.fraction == 1.0 || mid - trace.end_pos.z > stepsize {
                return false;
            }
        }
        true
    }

    /// `SV_movestep`: the move is adjusted for slopes and stairs; when it is not possible
    /// nothing moves and the result is false.
    fn move_step(&mut self, id: EdictId, mv: Vec3, relink: bool) -> bool {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return false;
        };
        if v.flags & (FL_FLY | FL_SWIM) == 0 {
            return self.ground_step(id, mv, relink, false);
        }
        // Flying monsters don't step up: try one move with vertical motion, then one without.
        let (origin, mins, maxs, flags) = (v.origin, v.mins, v.maxs, v.flags);
        let monster_clip = flags & FL_MONSTERCLIP != 0;
        let enemy_z = v
            .enemy
            .and_then(|e| self.edicts.get(e))
            .map(|e| e.vars.origin.z);
        for i in 0..2 {
            let mut neworg = origin + mv;
            if i == 0
                && let Some(enemy_z) = enemy_z
            {
                let dz = origin.z - enemy_z;
                if dz > 40.0 {
                    neworg.z -= 8.0;
                } else if dz < 30.0 {
                    neworg.z += 8.0;
                }
            }
            let (t, _) = self.move_counted(
                origin,
                mins,
                maxs,
                neworg,
                MoveFilter::Normal,
                Some(id),
                monster_clip,
            );
            if t.trace.fraction == 1.0 {
                if flags & FL_SWIM != 0
                    && self.point_contents_ents(t.trace.end_pos) == CONTENTS_EMPTY
                {
                    // A swimmer that left the water.
                    return false;
                }
                if let Some(e) = self.edicts.get_mut(id) {
                    e.vars.origin = t.trace.end_pos;
                }
                if relink {
                    self.link_entity(id, true);
                }
                return true;
            }
            if enemy_z.is_none() {
                break;
            }
        }
        false
    }

    /// The walking half shared by `SV_movestep` and `SV_movetest`: push down from a step height
    /// above the wished position, then check for dangling corners. `world_only` is
    /// `SV_movetest`'s `SV_MoveNoEnts`.
    fn ground_step(&mut self, id: EdictId, mv: Vec3, relink: bool, world_only: bool) -> bool {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return false;
        };
        let (oldorg, mins, maxs) = (v.origin, v.mins, v.maxs);
        let monster_clip = v.flags & FL_MONSTERCLIP != 0;
        let stepsize = self.cvar_f32("sv_stepsize");

        // Push down from a step height above the wished position.
        let mut neworg = oldorg + mv;
        neworg.z += stepsize;
        let mut end = neworg;
        end.z -= stepsize * 2.0;
        if !in_world(neworg) || !in_world(end) {
            return false;
        }

        let mut trace = self.step_trace(id, neworg, mins, maxs, end, monster_clip, world_only);
        if trace.trace.all_solid {
            return false;
        }
        if trace.trace.start_solid {
            neworg.z -= stepsize;
            trace = self.step_trace(id, neworg, mins, maxs, end, monster_clip, world_only);
            if trace.trace.all_solid || trace.trace.start_solid {
                return false;
            }
        }

        if trace.trace.fraction == 1.0 {
            // If the monster had the ground pulled out, go ahead and fall.
            let partial = self
                .edicts
                .get(id)
                .is_some_and(|e| e.vars.flags & FL_PARTIALGROUND != 0);
            if partial {
                if let Some(e) = self.edicts.get_mut(id) {
                    e.vars.origin += mv;
                    e.vars.flags &= !FL_ONGROUND;
                }
                if relink {
                    self.link_entity(id, true);
                }
                return true;
            }
            // Walked off an edge.
            return false;
        }

        // Check point traces down for dangling corners.
        if let Some(e) = self.edicts.get_mut(id) {
            e.vars.origin = trace.trace.end_pos;
        }
        if !self.check_bottom(id) {
            let partial = self
                .edicts
                .get(id)
                .is_some_and(|e| e.vars.flags & FL_PARTIALGROUND != 0);
            if !partial {
                if let Some(e) = self.edicts.get_mut(id) {
                    e.vars.origin = oldorg;
                }
                return false;
            }
            // The floor was mostly pulled out from underneath it and it is trying to correct.
        } else if let Some(e) = self.edicts.get_mut(id) {
            // Back on ground.
            e.vars.flags &= !FL_PARTIALGROUND;
            e.vars.groundentity = trace.ent;
        }

        if relink {
            self.link_entity(id, true);
        }
        true
    }

    /// The trace of the floor search: `SV_Move` of the entity, or `SV_MoveNoEnts` (the world
    /// model alone) for `SV_movetest`.
    #[allow(clippy::too_many_arguments)]
    fn step_trace(
        &self,
        id: EdictId,
        start: Vec3,
        mins: Vec3,
        maxs: Vec3,
        end: Vec3,
        monster_clip: bool,
        world_only: bool,
    ) -> EntTrace {
        if world_only {
            if !in_world(mins) || !in_world(maxs) {
                return EntTrace {
                    trace: refused_trace(start),
                    ent: None,
                };
            }
            self.clip_world(start, mins, maxs, end)
        } else {
            self.move_counted(
                start,
                mins,
                maxs,
                end,
                MoveFilter::Normal,
                Some(id),
                monster_clip,
            )
            .0
        }
    }

    /// `SV_StepDirection`: turns to the movement direction and walks `dist` that way. The
    /// entity is linked whether or not the step worked.
    fn step_direction(&mut self, id: EdictId, yaw: f32, dist: f32) -> bool {
        let mv = yaw_move(yaw, dist);
        let ok = self.move_step(id, mv, false);
        self.link_entity(id, true);
        ok
    }

    /// `SV_FlyDirection`.
    fn fly_direction(&mut self, id: EdictId, direction: Vec3) -> bool {
        let ok = self.move_step(id, direction, false);
        self.link_entity(id, true);
        ok
    }

    /// `SV_FixCheckBottom`.
    fn fix_check_bottom(&mut self, id: EdictId) {
        if let Some(e) = self.edicts.get_mut(id) {
            e.vars.flags |= FL_PARTIALGROUND;
        }
    }

    /// `RandomLong(0, 1)` of the engine's own generator (see the module notes).
    fn random_bool(&mut self) -> bool {
        let mut x = self.world.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.world.rng = x;
        (x >> 16) & 1 != 0
    }

    /// `SV_NewChaseDir2`: picks a direction toward `goal` and walks `dist` along it.
    fn new_chase_dir2(&mut self, id: EdictId, goal: Vec3, dist: f32) {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return;
        };
        let (origin, ideal_yaw) = (v.origin, v.ideal_yaw);
        let olddir = anglemod(45.0 * ((ideal_yaw / 45.0) as i32) as f32);
        let turnaround = anglemod(olddir - 180.0);

        let deltax = goal.x - origin.x;
        let deltay = goal.y - origin.y;

        // `d[1]` and `d[2]` of the source.
        let mut d1 = if deltax > 10.0 {
            0.0
        } else if deltax < -10.0 {
            180.0
        } else {
            DI_NODIR
        };
        let mut d2 = if deltay < -10.0 {
            270.0
        } else if deltay > 10.0 {
            90.0
        } else {
            DI_NODIR
        };

        // Try the direct route.
        if d1 != DI_NODIR && d2 != DI_NODIR {
            // 215 is as the source has it (not 225).
            let tempdir = if d1 == 0.0 {
                if d2 == 90.0 { 45.0 } else { 315.0 }
            } else if d2 == 90.0 {
                135.0
            } else {
                215.0
            };
            if tempdir != turnaround && self.step_direction(id, tempdir, dist) {
                return;
            }
        }

        // Try other directions. (`abs` of the source is the integer one.)
        if self.random_bool() || (deltay as i32).abs() > (deltax as i32).abs() {
            std::mem::swap(&mut d1, &mut d2);
        }
        if d1 != DI_NODIR && d1 != turnaround && self.step_direction(id, d1, dist) {
            return;
        }
        if d2 != DI_NODIR && d2 != turnaround && self.step_direction(id, d2, dist) {
            return;
        }

        // There is no direct path to the goal, so pick another direction.
        if olddir != DI_NODIR && self.step_direction(id, olddir, dist) {
            return;
        }

        // Randomly determine the direction of the search.
        if self.random_bool() {
            let mut tempdir = 0.0;
            while tempdir <= 315.0 {
                if tempdir != turnaround && self.step_direction(id, tempdir, dist) {
                    return;
                }
                tempdir += 45.0;
            }
        } else {
            let mut tempdir = 315.0;
            while tempdir >= 0.0 {
                if tempdir != turnaround && self.step_direction(id, tempdir, dist) {
                    return;
                }
                tempdir -= 45.0;
            }
        }

        // We tried. Run backwards. That ought to work...
        if turnaround != DI_NODIR && self.step_direction(id, turnaround, dist) {
            return;
        }

        // Can't move, we're stuck somehow.
        if let Some(e) = self.edicts.get_mut(id) {
            e.vars.ideal_yaw = olddir;
        }
        // If a bridge was pulled out from underneath a monster, it may not have a valid
        // standing position at all.
        if !self.check_bottom(id) {
            self.fix_check_bottom(id);
        }
    }
}

/// The horizontal move of `dist` units along `yaw` degrees, computed in double as the source
/// does.
fn yaw_move(yaw: f32, dist: f32) -> Vec3 {
    let a = f64::from(yaw) * 2.0 * std::f64::consts::PI / 360.0;
    Vec3::new(
        (a.cos() * f64::from(dist)) as f32,
        (a.sin() * f64::from(dist)) as f32,
        0.0,
    )
}
