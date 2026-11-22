//! The game-facing calls of [`EngineApi`] for entities, resources, cvars, time and geometry.
//! Every call refuses quietly (or returns a default) on bad game-supplied data: a stale or
//! foreign [`EdictId`], an out-of-range slot or model, non-finite numbers.

use std::any::Any;

use glam::Vec3;

use crate::collision::{CONTENTS_EMPTY, Hull, MAX_WORLD_COORD, NO_COLLISION, TraceResult};
use crate::edict::{EdictId, EntVars};
use crate::game::EngineApi;
use crate::pm::{
    self, CONTENTS_LADDER, MAX_MOVEENTS, MAX_PHYSENTS, PmPhysEnts, PmTraceResult,
    physent_from_edict, reach_box, world_physent,
};
use crate::trace_ents::{EntTrace, move_bounds};
use crate::world::consts::{
    FL_CLIENT, FL_MONSTERCLIP, SOLID_BBOX, SOLID_BSP, SOLID_NOT, SOLID_SLIDEBOX,
};
use crate::world::free_entity;
use crate::mapents::MAX_VALUE_BYTES;
use crate::precache::PrecacheError;

impl EngineApi<'_> {
    /// Simulation time in seconds (HLDS `gpGlobals->time`). It starts at `SV_TIME_AT_MAP_START`
    /// (1.0) on every map and advances by the frame time after the frame's physics. During a
    /// `think` it is the time that think was due (`max(nextthink, sv_time)`); in every other
    /// game call it is the server time `sv_time` (HLDS `sv.time`).
    pub fn time(&self) -> f64 {
        self.time
    }

    /// The server's real time in seconds (HLDS `realtime`, `Sys_FloatTime`): the same for every
    /// call of one frame, so the commands of a frame batch share it.
    pub fn realtime(&self) -> f64 {
        self.realtime
    }

    /// Seconds of the current server frame (HLDS `host_frametime`: the real time since the
    /// previous frame, at most 0.25); 0 for a call made outside a frame (server start-up, a map
    /// change).
    pub fn frametime(&self) -> f32 {
        self.frametime
    }

    /// Sets the game's `force_retouch` global. While it is above 0, every physics pass links
    /// every live entity with trigger touching (HLDS `SV_Physics`) and then lowers the value by
    /// 1, so `n` makes the next `n` frames re-find the triggers stationary entities rest in.
    /// A non-finite or negative `n` is taken as 0.
    pub fn set_force_retouch(&mut self, n: f32) {
        self.world.force_retouch = if n.is_finite() && n > 0.0 { n } else { 0.0 };
    }

    /// Number of player slots.
    pub fn max_clients(&self) -> u8 {
        self.edicts.max_clients()
    }

    /// Name of the current map, without `maps/` and `.bsp`.
    pub fn map_name(&self) -> &str {
        &self.map.name
    }

    /// Worldspawn (index 0); always live.
    pub fn world(&self) -> EdictId {
        self.edicts.world()
    }

    /// Edict of player slot `slot` (index `slot + 1`). For `slot >= max_clients` returns an id
    /// that no call resolves.
    pub fn client_edict(&self, slot: u8) -> EdictId {
        self.edicts.client(slot)
    }

    /// Allocates an entity at the current time and sets its `classname`. Returns `None` when
    /// the pool has no free slot (slots freed less than `EDICT_REUSE_DELAY` ago do not count,
    /// except slots freed at server time `EDICT_REUSE_ALWAYS_BEFORE` or earlier, which are
    /// reused at once), and for a `classname` longer than [`MAX_VALUE_BYTES`] or containing a
    /// NUL byte (nothing is allocated then).
    pub fn create_entity(&mut self, classname: &str) -> Option<EdictId> {
        if classname.len() > MAX_VALUE_BYTES || classname.contains('\0') {
            return None;
        }
        let id = self.edicts.alloc(self.sv_time)?;
        if let Some(edict) = self.edicts.get_mut(id) {
            edict.vars.classname = classname.to_string();
        }
        Some(id)
    }

    /// Frees entity `id` with its variables and private data. A stale or foreign id, the
    /// world and the player slots are left alone.
    pub fn remove_entity(&mut self, id: EdictId) {
        free_entity(self.edicts, self.world, id, self.sv_time);
    }

    /// Variables of entity `id`; `None` for a stale or foreign id.
    pub fn ent(&self, id: EdictId) -> Option<&EntVars> {
        self.edicts.get(id).map(|e| &e.vars)
    }

    /// Mutable variables of entity `id`; `None` for a stale or foreign id.
    pub fn ent_mut(&mut self, id: EdictId) -> Option<&mut EntVars> {
        self.edicts.get_mut(id).map(|e| &mut e.vars)
    }

    /// The game's private data of entity `id` as a `T`; `None` for a stale or foreign id, an
    /// entity without private data, or data of another type.
    pub fn private<T: Any + Send>(&self, id: EdictId) -> Option<&T> {
        self.edicts.get(id)?.private.as_deref()?.downcast_ref()
    }

    /// Mutable private data of entity `id` as a `T`; `None` in the cases
    /// [`private`](Self::private) returns `None`.
    pub fn private_mut<T: Any + Send>(&mut self, id: EdictId) -> Option<&mut T> {
        self.edicts
            .get_mut(id)?
            .private
            .as_deref_mut()?
            .downcast_mut()
    }

    /// Replaces the private data of entity `id`, whatever its type was, with `value`. A stale
    /// or foreign id is ignored (`value` is dropped).
    pub fn set_private<T: Any + Send>(&mut self, id: EdictId, value: T) {
        if let Some(edict) = self.edicts.get_mut(id) {
            edict.private = Some(Box::new(value));
        }
    }

    /// Every live entity (world and player slots included) in ascending index order.
    pub fn entity_ids(&self) -> Vec<EdictId> {
        self.edicts.iter_live().map(|(id, _)| id).collect()
    }

    /// The first live entity after `after` (from the start for `None`), in ascending index
    /// order, whose `classname` equals `classname` exactly (case-sensitive). An `after` that no
    /// longer resolves starts after its index. `None` when there is no further match.
    pub fn find_by_classname(&self, after: Option<EdictId>, classname: &str) -> Option<EdictId> {
        let first = after.map_or(0, |id| usize::from(id.index) + 1);
        self.edicts
            .iter_live_from(first)
            .find(|(_, e)| e.vars.classname == classname)
            .map(|(id, _)| id)
    }

    /// Live entities (world and player slots included) whose `origin` lies within `radius` of
    /// `center`, in ascending index order. A non-finite `center` or `radius`, or a negative
    /// `radius`, gives an empty list; an entity with a non-finite origin is never included.
    pub fn entities_in_sphere(&self, center: Vec3, radius: f32) -> Vec<EdictId> {
        if !center.is_finite() || !radius.is_finite() || radius < 0.0 {
            return Vec::new();
        }
        let radius_sq = radius * radius;
        self.edicts
            .iter_live()
            .filter(|(_, e)| e.vars.origin.distance_squared(center) <= radius_sq)
            .map(|(id, _)| id)
            .collect()
    }

    /// Adds `name` to the model table and returns its index. Errors when the table is full or
    /// when a new name comes after `server_activate`; a name already present always resolves.
    pub fn precache_model(&mut self, name: &str) -> Result<u16, PrecacheError> {
        self.precache.precache_model(name)
    }

    /// Adds `name` to the sound table; errors as [`precache_model`](Self::precache_model).
    pub fn precache_sound(&mut self, name: &str) -> Result<u16, PrecacheError> {
        self.precache.precache_sound(name)
    }

    /// Adds `name` to the generic file table; errors as
    /// [`precache_model`](Self::precache_model).
    pub fn precache_generic(&mut self, name: &str) -> Result<u16, PrecacheError> {
        self.precache.precache_generic(name)
    }

    /// Index of model `name` (case-insensitive, `\` and `/` alike); `None` if not precached.
    pub fn model_index(&self, name: &str) -> Option<u16> {
        self.precache.model_index(name)
    }

    /// Index of sound `name` (case-insensitive, `\` and `/` alike); `None` if not precached.
    pub fn sound_index(&self, name: &str) -> Option<u16> {
        self.precache.sound_index(name)
    }

    /// Gives entity `id` model `name`, which must already be in the model table: stores the
    /// name in `model` and its index in `modelindex`. For a brush sub-model (`*N`) on a map
    /// with collision data it also sets `mins`/`maxs`/`size` to that model's bounds. The entity
    /// is linked (see [`link_entity`](Self::link_entity)). Returns `false`
    /// and changes nothing for a stale or foreign id or a name not in the table.
    pub fn set_model(&mut self, id: EdictId, name: &str) -> bool {
        let Some(index) = self.precache.model_index(name) else {
            return false;
        };
        let bounds = name
            .strip_prefix('*')
            .and_then(|n| n.parse::<usize>().ok())
            .and_then(|n| self.map.collision()?.model_bounds(n));
        let Some(vars) = self.edicts.get_mut(id).map(|e| &mut e.vars) else {
            return false;
        };
        vars.model = name.to_string();
        vars.modelindex = index;
        if let Some((mins, maxs)) = bounds {
            // `Mod_LoadSubmodels` stores every brush model one unit larger on each side than
            // the BSP says, and `SetMinMaxSize` copies those bounds: the baselines of the
            // original carry them (read off the golden capture of de_dust2, 2026-11-22; the
            // function itself was not fetched).
            let (mins, maxs) = (mins - Vec3::ONE, maxs + Vec3::ONE);
            vars.mins = mins;
            vars.maxs = maxs;
            vars.size = maxs - mins;
        }
        // `PF_setmodel_I`: `SetMinMaxSize` for a brush model, then `SV_LinkEdict`.
        self.link_entity(id, false);
        true
    }

    /// Numeric value of cvar `name` (case-insensitive); 0.0 for a cvar that is missing or
    /// whose value is not a finite number.
    pub fn cvar_f32(&self, name: &str) -> f32 {
        let value = self.cvars.get_f32(name);
        if value.is_finite() { value } else { 0.0 }
    }

    /// Value of cvar `name` (case-insensitive); `None` if it is not registered.
    pub fn cvar_str(&self, name: &str) -> Option<String> {
        self.cvars.get(name).map(|c| c.value.clone())
    }

    /// The server's current movevars (ReHLDS `SV_SetMoveVars`, read 2026-11-22, which the
    /// original refreshes whenever one of its cvars changes): an owned copy, built from the
    /// cvars as they are now. What a client was last sent can differ until the next frame.
    pub fn move_vars(&self) -> cs_proto48::messages::MoveVarsMsg {
        crate::signon::move_vars(self.cvars, &self.map.entities)
    }

    /// Sets cvar `name` (case-insensitive) to `value`; a name that is not registered is
    /// ignored.
    pub fn set_cvar(&mut self, name: &str, value: &str) {
        self.cvars.set(name, value);
    }

    /// Traces `hull` from `start` to `end` through the world model only; for a trace that
    /// also hits entities see [`trace_hull_ents`](Self::trace_hull_ents). Without collision data
    /// (stub map) nothing is hit: fraction 1, `end_pos = end`. A `start` or `end` outside the
    /// world (non-finite, or a coordinate beyond [`MAX_WORLD_COORD`]) is refused: nothing is
    /// hit, `end_pos = start` if `start` is inside the world, else the origin. Every returned
    /// float is finite.
    pub fn trace_hull(&self, start: Vec3, end: Vec3, hull: Hull) -> TraceResult {
        if !in_world(start) || !in_world(end) {
            return refused_trace(start);
        }
        match self.map.collision() {
            Some(c) => c.trace_world(start, end, hull),
            None => TraceResult::clear(end),
        }
    }

    /// Traces `hull` from `start` to `end` through brush model `model` (0 is the world) placed
    /// at `origin`. Nothing is hit (fraction 1, `end_pos = end`) for a model out of range or
    /// without collision data. An `origin`, `start` or `end` outside the world is refused as in
    /// [`trace_hull`](Self::trace_hull). Every returned float is finite.
    pub fn trace_model(
        &self,
        model: usize,
        origin: Vec3,
        start: Vec3,
        end: Vec3,
        hull: Hull,
    ) -> TraceResult {
        if !in_world(origin) || !in_world(start) || !in_world(end) {
            return refused_trace(start);
        }
        match self.map.collision() {
            Some(c) => c.trace_model(model, origin, start, end, hull),
            None => TraceResult::clear(end),
        }
    }

    /// Contents (`CONTENTS_*`) of the world model at `p` for `hull`; `CONTENTS_EMPTY` without
    /// collision data or for a `p` outside the world (non-finite, or a coordinate beyond
    /// [`MAX_WORLD_COORD`]).
    pub fn point_contents(&self, p: Vec3, hull: Hull) -> i32 {
        match self.map.collision() {
            Some(c) if in_world(p) => c.point_contents(p, hull),
            _ => CONTENTS_EMPTY,
        }
    }

    /// Whether `p` is inside the solid of brush model `model` placed at `origin`; `false` for
    /// a model out of range, without collision data, or for an `origin` or `p` outside the
    /// world (non-finite, or a coordinate beyond [`MAX_WORLD_COORD`]).
    pub fn point_in_model(&self, model: usize, origin: Vec3, p: Vec3) -> bool {
        match self.map.collision() {
            Some(c) if in_world(origin) && in_world(p) => c.point_in_model(model, origin, p),
            _ => false,
        }
    }

    /// The trace of the player movement code: the box of `hull` from `start` to `end` through
    /// the world and the entities `player`'s movement collides with, which are the physents of
    /// `SV_AddLinksToPM` and `SV_IsCollidableInPM` (ReHLDS `rehlds/engine/sv_user.cpp`, read
    /// 2026-11-21). An entity is left out when it is `player` itself or is owned by it; when it
    /// is not `SOLID_BSP`, `SOLID_BBOX` or `SOLID_SLIDEBOX` (triggers and `SOLID_NOT` entities,
    /// whose `skin` contents only matter to `point_contents_ents`); when it is a `SOLID_BSP`
    /// entity with `FL_MONSTERCLIP`; when it is a client with no health left; when its box is
    /// `mins.z == 0`, `maxs.z == 1` or its `size` is zero; and when its absolute box misses
    /// the box around the player's origin (256 units each way, the original's reach), which
    /// here also takes in the whole move so that a trace never misses what it passes through.
    /// A client's box is the entity's own, except that a player the lag compensation moved for
    /// the command being run (`unlag.rs`) is met where it really is (`SV_GetTrueOrigin`,
    /// `SV_GetTrueMinMax`); neither the `groupinfo` filter nor the `SV_AllowPhysent` hook
    /// exists.
    ///
    /// The results combine as the legacy `BspCollision::trace_hull` did: the world first, an
    /// all-solid world trace returned at once; then each entity in ascending index order, an
    /// all-solid one returned at once, and any other replacing the best when it stops nearer
    /// or starts in solid while the best does not. `EntTrace::ent` is the world or the entity
    /// that stopped the trace, `None` for a clear one. A stale `player` or a `start` or `end`
    /// outside the world is refused: nothing is hit and `end_pos` is `start`.
    pub fn trace_player_move(
        &self,
        player: EdictId,
        start: Vec3,
        end: Vec3,
        hull: Hull,
    ) -> EntTrace {
        let refused = EntTrace {
            trace: refused_trace(start),
            ent: None,
        };
        let Some(origin) = self.edicts.get(player).map(|e| e.vars.origin) else {
            return refused;
        };
        if !in_world(start) || !in_world(end) {
            return refused;
        }
        let half = hull.half_extents();
        let (mins, maxs) = (-half, half);

        let mut best = self.trace_hull(start, end, hull);
        let mut best_ent = (best.fraction < 1.0 || best.start_solid).then(|| self.world());
        if best.fraction <= 0.0 && best.all_solid {
            return EntTrace {
                trace: finish_player_trace(best),
                ent: best_ent,
            };
        }

        // The reach box of `SV_AddLinksToPM`, widened to the move.
        let (move_min, move_max) = move_bounds(start, mins, maxs, end);
        let reach = Vec3::splat(PM_REACH);
        let box_min = (origin - reach).min(move_min);
        let box_max = (origin + reach).max(move_max);
        let mut candidates = Vec::new();
        self.world
            .solids_in_box(self.edicts, box_min, box_max, &mut candidates);
        // Players the lag compensation moved are linked where they were rewound to, but the
        // movement meets them where they really are (`SV_GetTrueOrigin`).
        candidates.extend(
            self.world
                .moved_players
                .iter()
                .filter(|m| {
                    m.true_absmin.cmple(box_max).all() && box_min.cmple(m.true_absmax).all()
                })
                .map(|m| m.id),
        );
        candidates.sort_by_key(|id| id.index);
        candidates.dedup();

        for id in candidates {
            if !self.collidable_in_pm(id, player, box_min, box_max) {
                continue;
            }
            let Some(solid) = self.edicts.get(id).map(|e| e.vars.solid) else {
                continue;
            };
            // `SV_IsCollidableInPM` lets a `SOLID_NOT` brush (water) through; a trace never
            // hits one.
            if solid == SOLID_NOT {
                continue;
            }
            let true_origin = self.world.moved_player(id).map(|m| m.true_origin);
            let hit = self
                .clip_to_entity_at(id, true_origin, start, mins, maxs, end, solid == SOLID_BSP)
                .trace;
            if hit.all_solid {
                return EntTrace {
                    trace: finish_player_trace(hit),
                    ent: Some(id),
                };
            }
            if hit.fraction < best.fraction || (hit.start_solid && !best.start_solid) {
                best = hit;
                best_ent = Some(id);
            }
        }
        EntTrace {
            trace: finish_player_trace(best),
            ent: best_ent,
        }
    }

    /// `SV_AddLinksToPM` for `player` (ReHLDS `sv_user.cpp`, read 2026-11-22): the world first,
    /// then the area-tree walk within 256 units of the player's origin. Ladders (`SOLID_NOT`
    /// with `skin` `CONTENTS_LADDER`) go only into the moveents. Another player is met at its
    /// true (not lag-rewound) origin and dead players are left out. Entities are taken in the
    /// tree's order, not in edict order; at most `MAX_PHYSENTS` physents and `MAX_MOVEENTS`
    /// moveents. A stale `player` gives an empty list (not even the world); a player whose
    /// origin is outside the world gets the world alone. See `pm.rs` for what is left out.
    pub fn pm_physents(&self, player: EdictId) -> PmPhysEnts {
        let mut out = PmPhysEnts::default();
        let Some(origin) = self.edicts.get(player).map(|e| e.vars.origin) else {
            return out;
        };
        let coll = self.map.collision();
        out.push_physent(world_physent(self.world(), coll));
        if !in_world(origin) {
            return out;
        }
        let (mins, maxs) = reach_box(origin);
        let mut candidates = Vec::new();
        self.world
            .solids_in_tree(self.edicts, mins, maxs, &mut candidates);
        let max_clients = u16::from(self.edicts.max_clients());
        for id in candidates {
            let Some(check) = self.edicts.get(id).map(|e| &e.vars) else {
                continue;
            };
            if !self.collidable_in_pm(id, player, mins, maxs) {
                continue;
            }
            let client_slot = (1..=max_clients).contains(&id.index);
            let true_origin = self.world.moved_player(id).map(|m| m.true_origin);
            let pe = physent_from_edict(id, check, client_slot, true_origin, coll);
            // `check->v.solid || check->v.skin != -16`: only a ladder goes to the moveents.
            if check.solid != SOLID_NOT || i32::from(check.skin) != CONTENTS_LADDER {
                if out.num_physents() >= MAX_PHYSENTS {
                    break;
                }
                out.push_physent(pe);
            } else if out.num_moveents() < MAX_MOVEENTS {
                out.push_moveent(pe);
            }
        }
        out
    }

    /// `_PM_PlayerTrace` (ReHLDS `pmovetst.cpp`, read 2026-11-22): the player's hull
    /// `usehull` (0 stand, 1 duck, 2 point, 3 large) traced from `start` to `end` through
    /// every physent of `pe` in order (BSP hulls by the map `{0 -> 1, 1 -> 3, 2 -> 0,
    /// 3 -> 2}`, box hulls for the rest, rotated brush models), the flags `PM_WORLD_ONLY` and
    /// `PM_GLASS_IGNORE` honoured and the physent `ignore_pe` (-1: none) skipped. A trace
    /// that hits nothing has `ent` -1 and a zero normal. A `start` or `end` outside the world
    /// (non-finite or beyond `MAX_WORLD_COORD`) or a `usehull` outside `0..=3` hits nothing.
    pub fn pm_player_trace(
        &self,
        pe: &PmPhysEnts,
        start: Vec3,
        end: Vec3,
        usehull: i32,
        trace_flags: i32,
        ignore_pe: i32,
    ) -> PmTraceResult {
        if !in_world(start) || !in_world(end) {
            return PmTraceResult::miss(if in_world(end) { end } else { Vec3::ZERO });
        }
        let coll = self.map.collision().unwrap_or(&NO_COLLISION);
        pm::player_trace(
            coll,
            pe.physents_slice(),
            start,
            end,
            usehull,
            trace_flags,
            ignore_pe,
        )
    }

    /// `_PM_TestPlayerPosition` (ReHLDS `pmovetst.cpp`, read 2026-11-22): the index of the
    /// first physent whose hull holds `pos` in solid, or -1, and the trace the original
    /// fills. Quirk: that trace is taken at `trace_origin` (`pmove->origin`), not at `pos`.
    /// Input outside the world, or a `usehull` outside `0..=3`, gives -1.
    pub fn pm_test_player_position(
        &self,
        pe: &PmPhysEnts,
        trace_origin: Vec3,
        pos: Vec3,
        usehull: i32,
    ) -> (i32, PmTraceResult) {
        if !in_world(trace_origin) || !in_world(pos) {
            return (-1, PmTraceResult::miss(Vec3::ZERO));
        }
        let coll = self.map.collision().unwrap_or(&NO_COLLISION);
        pm::test_player_position(coll, pe.physents_slice(), trace_origin, pos, usehull)
    }

    /// `PM_PointContents` (ReHLDS `pmovetst.cpp`, read 2026-11-22): `(contents,
    /// truecontents)`. The true contents are the world model's alone. The contents fold the
    /// water currents into water and are overridden by a non-solid brush physent holding `p`
    /// (a `func_water`'s `skin`); solid wins. `p` outside the world is empty.
    pub fn pm_point_contents(&self, pe: &PmPhysEnts, p: Vec3) -> (i32, i32) {
        if !in_world(p) {
            return (CONTENTS_EMPTY, CONTENTS_EMPTY);
        }
        let coll = self.map.collision().unwrap_or(&NO_COLLISION);
        pm::point_contents(coll, pe.physents_slice(), p)
    }

    /// The test of `PM_Ladder` for moveent `index`: a brush model with `skin`
    /// `CONTENTS_LADDER` whose hull for `usehull`, shifted as `PM_HullForBsp` does, holds
    /// `origin` (its contents are not `CONTENTS_EMPTY`). `false` for a bad index, `usehull` or
    /// `origin`.
    pub fn pm_ladder_contains(
        &self,
        pe: &PmPhysEnts,
        index: usize,
        origin: Vec3,
        usehull: i32,
    ) -> bool {
        let Some(moveent) = pe.moveent_at(index) else {
            return false;
        };
        if !in_world(origin) {
            return false;
        }
        let coll = self.map.collision().unwrap_or(&NO_COLLISION);
        pm::ladder_contains(coll, moveent, origin, usehull)
    }

    /// `PM_TraceModel` (ReHLDS `pmovetst.cpp`, read 2026-11-22) against moveent `index`: the
    /// point hull of its brush model from `start` to `end`, as `(fraction, plane normal)`;
    /// `(1, 0)` for a miss, a bad index, a moveent that is no brush model or input outside the
    /// world.
    pub fn pm_trace_model(
        &self,
        pe: &PmPhysEnts,
        index: usize,
        start: Vec3,
        end: Vec3,
    ) -> (f32, Vec3) {
        let Some(moveent) = pe.moveent_at(index) else {
            return (1.0, Vec3::ZERO);
        };
        if !in_world(start) || !in_world(end) {
            return (1.0, Vec3::ZERO);
        }
        let coll = self.map.collision().unwrap_or(&NO_COLLISION);
        pm::trace_model(coll, moveent, start, end)
    }

    /// `SV_AddLinksToPM_`'s tests and `SV_IsCollidableInPM` for entity `id` against the box
    /// `mins..maxs` (see [`trace_player_move`](Self::trace_player_move)): the entity is a kind
    /// the movement lists (`SOLID_BSP`, `SOLID_BBOX`, `SOLID_SLIDEBOX`, or `SOLID_NOT` with a
    /// model and a `skin`, water and ladders).
    fn collidable_in_pm(&self, id: EdictId, player: EdictId, mins: Vec3, maxs: Vec3) -> bool {
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return false;
        };
        // A moved player is judged by the box it really has (`SV_GetTrueMinMax`).
        let (absmin, absmax) = self
            .world
            .moved_player(id)
            .map_or((v.absmin, v.absmax), |m| (m.true_absmin, m.true_absmax));
        id.index != player.index
            && v.owner.is_none_or(|o| o.index != player.index)
            && !(v.solid == SOLID_NOT && (v.skin == 0 || v.modelindex == 0))
            && matches!(v.solid, SOLID_BSP | SOLID_BBOX | SOLID_SLIDEBOX | SOLID_NOT)
            && !(v.flags & FL_MONSTERCLIP != 0 && v.solid == SOLID_BSP)
            && !(v.flags & FL_CLIENT != 0 && v.health <= 0.0)
            && !(v.mins.z == 0.0 && v.maxs.z == 1.0)
            && v.size.length() != 0.0
            && absmin.cmple(maxs).all()
            && mins.cmple(absmax).all()
    }

    /// `SV_Impact` for a touch the movement of `player` found with `other` (`SV_ProcessImpacts`
    /// does this for each `touchindex` of the original's `pmove`). The touches are kept until
    /// the command ends; then the engine relinks `player` (which queues the triggers it
    /// overlaps) and queues one impact for each, as `SV_Impact(other, player)`. Ignored for
    /// anything but a client's edict, a stale id, `other == player`, and past
    /// `MAX_PLAYER_IMPACTS` touches in one command. The original also sets the player's
    /// velocity to the trace's `deltavelocity` for each touch; here the game sees the velocity
    /// the command ended with.
    pub fn player_touched(&mut self, player: EdictId, other: EdictId) {
        let is_client = (1..=u16::from(self.edicts.max_clients())).contains(&player.index);
        if !is_client
            || player.index == other.index
            || self.edicts.get(player).is_none()
            || self.edicts.get(other).is_none()
            || self.world.player_impacts.len() >= MAX_PLAYER_IMPACTS
        {
            return;
        }
        self.world.player_impacts.push((player, other));
    }

    /// Whether a box with absolute corners `mins`/`maxs` is potentially visible (PVS) from
    /// `eye`. A map without visibility data (stub map) sees everything; an `eye`, `mins` or
    /// `maxs` outside the world (non-finite, or a coordinate beyond [`MAX_WORLD_COORD`]) sees
    /// nothing.
    pub fn can_see(&self, eye: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        in_world(eye) && in_world(mins) && in_world(maxs) && self.map.can_see(eye, mins, maxs)
    }
}

/// Reach, in units each way from the player's origin, of the entities `SV_AddLinksToPM` hands
/// to the player movement.
const PM_REACH: f32 = 256.0;

/// Touches of one player command `player_touched` keeps (`MAX_PHYSENTS`, the most physents the
/// original's `pmove` has).
const MAX_PLAYER_IMPACTS: usize = 600;

/// The flags of a trace that [`EngineApi::trace_player_move`] sets once, at the end.
fn finish_player_trace(mut t: TraceResult) -> TraceResult {
    t.hit_solid = t.fraction < 1.0 || t.start_solid;
    t
}

/// Whether every coordinate of `v` is finite and within [`MAX_WORLD_COORD`].
pub(crate) fn in_world(v: Vec3) -> bool {
    v.is_finite() && v.abs().max_element() <= MAX_WORLD_COORD
}

/// The answer to a refused trace: nothing hit, ending where it started if `start` is inside the
/// world, else at the origin.
pub(crate) fn refused_trace(start: Vec3) -> TraceResult {
    TraceResult::clear(if in_world(start) { start } else { Vec3::ZERO })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cs_assets::bsp::BspMap;
    use cs_cvar::{CVarFlags, CVarRegistry};
    use cs_proto48::usermsg::UserMsgRegistry;
    use cs_vfs::{Vfs, VfsConfig};
    use glam::Vec3;

    use crate::collision::{CONTENTS_EMPTY, Hull, TraceResult};
    use crate::edict::{EdictId, EdictPool, EntVars};
    use crate::game::{ClientView, EngineApi, Outbox};
    use crate::map::ServerMap;
    use crate::precache::PrecacheTables;
    use crate::world::WorldLinks;

    struct Fixture {
        edicts: EdictPool,
        precache: PrecacheTables,
        cvars: CVarRegistry,
        usermsgs: UserMsgRegistry,
        map: ServerMap,
        world: WorldLinks,
        outbox: Outbox,
        clients: Vec<ClientView>,
    }

    impl Fixture {
        fn new(max_clients: u8, max_edicts: usize) -> Self {
            Self::on(ServerMap::stub("de_test"), max_clients, max_edicts)
        }

        fn on(map: ServerMap, max_clients: u8, max_edicts: usize) -> Self {
            Self {
                world: WorldLinks::new(map.world_bounds()),
                edicts: EdictPool::new(max_clients, max_edicts),
                precache: PrecacheTables::new(&map.path, map.submodel_count),
                cvars: CVarRegistry::new_empty(),
                usermsgs: UserMsgRegistry::cs16_default(),
                map,
                outbox: Outbox::new(max_clients),
                clients: (0..max_clients).map(|_| ClientView::default()).collect(),
            }
        }

        fn api(&mut self) -> EngineApi<'_> {
            EngineApi::new(
                &mut self.edicts,
                &mut self.precache,
                &mut self.cvars,
                &self.usermsgs,
                &self.map,
                &mut self.world,
                10.0,
                0.25,
                &mut self.outbox,
                &mut self.clients,
            )
        }
    }

    #[test]
    fn time_and_identity_calls() {
        let mut f = Fixture::new(2, 8);
        let eng = f.api();
        assert_eq!(eng.time(), 10.0);
        assert_eq!(eng.frametime(), 0.25);
        assert_eq!(eng.max_clients(), 2);
        assert_eq!(eng.map_name(), "de_test");
        assert_eq!(eng.world().index, 0);
        assert_eq!(eng.client_edict(0).index, 1);
        assert_eq!(eng.client_edict(1).index, 2);
        assert!(eng.ent(eng.client_edict(2)).is_none());
        assert!(eng.ent(eng.client_edict(255)).is_none());
    }

    #[test]
    fn create_entity_refuses_a_bad_classname_without_allocating() {
        let mut f = Fixture::new(1, 5);
        let mut eng = f.api();
        let long = "c".repeat(crate::mapents::MAX_VALUE_BYTES + 1);
        assert!(eng.create_entity(&long).is_none());
        assert!(eng.create_entity("a\0b").is_none());
        assert!(eng.create_entity("\0").is_none());
        // Nothing was taken from the pool: the 3 free slots are still there.
        let exact = "c".repeat(crate::mapents::MAX_VALUE_BYTES);
        let first = eng.create_entity(&exact).expect("longest classname");
        assert_eq!(eng.ent(first).map(|v| v.classname.len()), Some(exact.len()));
        assert!(eng.create_entity("ok").is_some());
        assert!(eng.create_entity("full").is_some());
        assert!(eng.create_entity("full").is_none());
    }

    #[test]
    fn stale_and_foreign_ids_are_rejected_everywhere() {
        let mut f = Fixture::new(1, 8);
        f.precache.precache_model("models/a.mdl").unwrap();
        let mut eng = f.api();
        let stale = eng.create_entity("info_target").unwrap();
        eng.remove_entity(stale);
        let fresh = eng.create_entity("info_target").unwrap();
        let foreign = EdictId {
            index: 5,
            serial: 77,
        };
        let out_of_range = EdictId {
            index: u16::MAX,
            serial: 0,
        };
        for id in [stale, foreign, out_of_range] {
            assert!(eng.ent(id).is_none(), "{id:?}");
            assert!(eng.ent_mut(id).is_none(), "{id:?}");
            assert!(eng.private::<u32>(id).is_none(), "{id:?}");
            assert!(eng.private_mut::<u32>(id).is_none(), "{id:?}");
            eng.set_private(id, 5u32);
            assert!(eng.private::<u32>(id).is_none(), "{id:?}");
            assert!(!eng.set_model(id, "models/a.mdl"), "{id:?}");
            eng.remove_entity(id);
        }
        // None of it touched the live entity.
        assert_eq!(eng.ent(fresh).unwrap().classname, "info_target");
        assert_eq!(eng.ent(fresh).unwrap().modelindex, 0);
        assert!(eng.private::<u32>(fresh).is_none());
        assert_eq!(eng.entity_ids().len(), 3);
    }

    #[test]
    fn create_and_remove_roundtrip() {
        let mut f = Fixture::new(2, 5);
        let mut eng = f.api();
        let a = eng.create_entity("func_wall").unwrap();
        let b = eng.create_entity("info_target").unwrap();
        assert_eq!((a.index, b.index), (3, 4));
        assert_eq!(eng.ent(a).unwrap().classname, "func_wall");
        assert_eq!(eng.ent(b).unwrap().classname, "info_target");
        // The pool is exhausted.
        assert!(eng.create_entity("one_too_many").is_none());

        eng.remove_entity(a);
        assert!(eng.ent(a).is_none());
        assert!(eng.ent_mut(a).is_none());
        assert_eq!(
            eng.entity_ids()
                .iter()
                .map(|id| id.index)
                .collect::<Vec<_>>(),
            [0, 1, 2, 4]
        );
        // The freed slot is not reused within the reuse delay of the current time.
        assert!(eng.create_entity("too_soon").is_none());

        let world = eng.world();
        let player = eng.client_edict(1);
        eng.remove_entity(world);
        eng.remove_entity(player);
        assert!(eng.ent(world).is_some());
        assert!(eng.ent(player).is_some());
        assert_eq!(eng.world(), world);
        assert_eq!(eng.client_edict(1), player);
    }

    #[test]
    fn private_data_is_typed() {
        let mut f = Fixture::new(1, 8);
        let mut eng = f.api();
        let id = eng.create_entity("info_target").unwrap();
        assert!(eng.private::<u32>(id).is_none());
        eng.set_private(id, 7u32);
        assert_eq!(eng.private::<u32>(id), Some(&7));
        assert!(eng.private::<String>(id).is_none());
        assert!(eng.private_mut::<String>(id).is_none());
        *eng.private_mut::<u32>(id).unwrap() += 1;
        assert_eq!(eng.private::<u32>(id), Some(&8));
        // `set_private` replaces whatever was there, of any type.
        eng.set_private(id, String::from("hello"));
        assert!(eng.private::<u32>(id).is_none());
        assert_eq!(eng.private::<String>(id).map(String::as_str), Some("hello"));
        // A removed entity's data goes with it.
        eng.remove_entity(id);
        assert!(eng.private::<String>(id).is_none());
    }

    #[test]
    fn find_by_classname_iterates_in_index_order() {
        let mut f = Fixture::new(1, 16);
        let mut eng = f.api();
        let a = eng.create_entity("light").unwrap();
        let _other = eng.create_entity("info_target").unwrap();
        let b = eng.create_entity("light").unwrap();
        let _case = eng.create_entity("Light").unwrap();
        let c = eng.create_entity("light").unwrap();

        let mut found = Vec::new();
        let mut after = None;
        while let Some(id) = eng.find_by_classname(after, "light") {
            found.push(id);
            after = Some(id);
        }
        assert_eq!(found, [a, b, c]);
        assert_eq!(eng.find_by_classname(Some(c), "light"), None);
        assert_eq!(eng.find_by_classname(None, "nothing"), None);
        assert_eq!(eng.find_by_classname(None, ""), Some(eng.world()));

        // An `after` that no longer resolves still starts after its index.
        eng.remove_entity(b);
        assert_eq!(eng.find_by_classname(Some(b), "light"), Some(c));
        assert_eq!(eng.find_by_classname(Some(a), "light"), Some(c));
        let far = EdictId {
            index: u16::MAX,
            serial: 3,
        };
        assert_eq!(eng.find_by_classname(Some(far), "light"), None);
    }

    #[test]
    fn entities_in_sphere_uses_origin_and_handles_nan() {
        let mut f = Fixture::new(1, 16);
        let mut eng = f.api();
        let near = eng.create_entity("near").unwrap();
        eng.ent_mut(near).unwrap().origin = Vec3::new(100.0, 0.0, 0.0);
        let edge = eng.create_entity("edge").unwrap();
        eng.ent_mut(edge).unwrap().origin = Vec3::new(110.0, 0.0, 0.0);
        let far = eng.create_entity("far").unwrap();
        eng.ent_mut(far).unwrap().origin = Vec3::new(200.0, 0.0, 0.0);
        let lost = eng.create_entity("lost").unwrap();
        eng.ent_mut(lost).unwrap().origin = Vec3::NAN;
        let player = eng.client_edict(0);
        eng.ent_mut(player).unwrap().origin = Vec3::new(95.0, 5.0, 0.0);

        let center = Vec3::new(100.0, 0.0, 0.0);
        assert_eq!(eng.entities_in_sphere(center, 10.0), [player, near, edge]);
        // The world sits at the origin and is included like any other entity.
        assert_eq!(eng.entities_in_sphere(Vec3::ZERO, 1.0), [eng.world()]);
        assert!(eng.entities_in_sphere(Vec3::NAN, 10.0).is_empty());
        assert!(eng.entities_in_sphere(center, f32::NAN).is_empty());
        assert!(eng.entities_in_sphere(center, f32::INFINITY).is_empty());
        assert!(
            eng.entities_in_sphere(Vec3::splat(f32::INFINITY), 10.0)
                .is_empty()
        );
        assert!(eng.entities_in_sphere(center, -1.0).is_empty());
    }

    #[test]
    fn set_model_requires_a_precached_name() {
        let mut f = Fixture::new(1, 8);
        let mut eng = f.api();
        let id = eng.create_entity("cycler").unwrap();
        eng.ent_mut(id).unwrap().mins = Vec3::splat(-1.0);
        let before: EntVars = eng.ent(id).unwrap().clone();
        assert!(!eng.set_model(id, "models/unknown.mdl"));
        // `*1` is not in the table of a map without brush models.
        assert!(!eng.set_model(id, "*1"));
        assert_eq!(eng.ent(id).unwrap(), &before);

        let index = eng.precache_model("models/Barney.mdl").unwrap();
        assert_eq!(eng.model_index("models/barney.mdl"), Some(index));
        assert_eq!(eng.model_index("models/unknown.mdl"), None);
        assert!(eng.set_model(id, "models/barney.mdl"));
        let v = eng.ent(id).unwrap();
        assert_eq!(
            (v.model.as_str(), v.modelindex),
            ("models/barney.mdl", index)
        );
        // Not a brush model: the size is left alone.
        assert_eq!(v.mins, Vec3::splat(-1.0));

        let shot = eng.precache_sound("weapons/shot.wav").unwrap();
        assert_eq!(eng.sound_index("WEAPONS/SHOT.WAV"), Some(shot));
        assert_eq!(eng.sound_index("weapons/none.wav"), None);
        assert_eq!(eng.precache_generic("sprites/hud.txt"), Ok(1));
    }

    #[test]
    fn traces_on_a_stub_map_hit_nothing() {
        let mut f = Fixture::new(1, 8);
        let eng = f.api();
        let start = Vec3::new(1.0, 2.0, 3.0);
        let end = Vec3::new(500.0, -20.0, 7.0);
        for hull in [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched] {
            let tr = eng.trace_hull(start, end, hull);
            assert_eq!((tr.fraction, tr.end_pos), (1.0, end));
            assert!(!tr.all_solid && !tr.start_solid && !tr.hit_solid && !tr.hit_sky);
            let tr = eng.trace_model(0, Vec3::ZERO, start, end, hull);
            assert_eq!((tr.fraction, tr.end_pos), (1.0, end));
            assert_eq!(eng.point_contents(start, hull), CONTENTS_EMPTY);
        }
        assert!(!eng.point_in_model(0, Vec3::ZERO, start));
        assert!(!eng.point_in_model(usize::MAX, Vec3::ZERO, start));
        // The stub map sees everything; non-finite input sees nothing.
        assert!(eng.can_see(start, end - 16.0, end + 16.0));
        assert!(!eng.can_see(Vec3::NAN, end - 16.0, end + 16.0));

        // Refused input is "no hit" too, ending at `start` if it is usable, else at the origin.
        let tr = eng.trace_hull(Vec3::NAN, end, Hull::Point);
        assert_eq!((tr.fraction, tr.end_pos), (1.0, Vec3::ZERO));
        assert!(!tr.hit_solid);
        let tr = eng.trace_hull(start, Vec3::splat(1e30), Hull::Point);
        assert_eq!((tr.fraction, tr.end_pos), (1.0, start));
        let tr = eng.trace_model(0, Vec3::INFINITY, start, end, Hull::Point);
        assert_eq!((tr.fraction, tr.end_pos), (1.0, start));
        assert!(!eng.can_see(start, end - 16.0, Vec3::splat(-1e30)));
        assert_eq!(
            eng.point_contents(Vec3::INFINITY, Hull::Point),
            CONTENTS_EMPTY
        );
    }

    #[test]
    fn cvar_access_roundtrips_and_missing_is_zero() {
        let mut f = Fixture::new(1, 8);
        f.cvars
            .register("sv_gravity", "800", CVarFlags::NONE, "gravity");
        f.cvars
            .register("hostname", "my server", CVarFlags::NONE, "name");
        let mut eng = f.api();
        assert_eq!(eng.cvar_f32("sv_gravity"), 800.0);
        assert_eq!(eng.cvar_f32("SV_GRAVITY"), 800.0);
        assert_eq!(eng.cvar_str("sv_gravity").as_deref(), Some("800"));
        eng.set_cvar("sv_gravity", "600.5");
        assert_eq!(eng.cvar_f32("sv_gravity"), 600.5);
        assert_eq!(eng.cvar_f32("hostname"), 0.0);
        assert_eq!(eng.cvar_str("hostname").as_deref(), Some("my server"));
        eng.set_cvar("sv_gravity", "nan");
        assert_eq!(eng.cvar_f32("sv_gravity"), 0.0);

        assert_eq!(eng.cvar_f32("missing"), 0.0);
        assert_eq!(eng.cvar_str("missing"), None);
        eng.set_cvar("missing", "1");
        assert_eq!(eng.cvar_str("missing"), None);
    }

    /// On de_dust2: `set_model("*N")` takes the brush model's bounds, and the geometry calls
    /// answer what the map's `WorldCollision` answers.
    #[test]
    fn real_map_brush_models_and_traces() {
        let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
            return;
        };
        let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("de_dust2 reads");
        let bsp = Arc::new(BspMap::load(&vfs, "de_dust2").expect("de_dust2 parses"));
        let map = ServerMap::from_bsp("de_dust2", &raw, bsp).expect("valid BSP header");
        let mut f = Fixture::on(map, 1, 64);
        let collision = f.map.collision().expect("collision").clone();
        let mut eng = f.api();

        let id = eng.create_entity("func_wall").unwrap();
        assert!(eng.set_model(id, "*1"));
        let (mins, maxs) = collision.model_bounds(1).expect("model 1");
        let v = eng.ent(id).unwrap();
        assert_eq!((v.model.as_str(), v.modelindex), ("*1", 2));
        // The brush model's bounds are one unit larger on every side (`Mod_LoadSubmodels`).
        assert_eq!((v.mins, v.maxs), (mins - Vec3::ONE, maxs + Vec3::ONE));

        let (wmins, wmaxs) = collision.model_bounds(0).expect("world");
        let start = (wmins + wmaxs) * 0.5;
        let end = start + Vec3::new(4096.0, 0.0, -4096.0);
        for hull in [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched] {
            let a = eng.trace_hull(start, end, hull);
            let b = collision.trace_world(start, end, hull);
            assert_eq!((a.fraction, a.end_pos), (b.fraction, b.end_pos));
            assert_eq!(
                eng.point_contents(start, hull),
                collision.point_contents(start, hull)
            );
        }

        // Refused input on a real map: no hit, nothing solid, nothing seen, and no non-finite
        // number handed back. `1e30` is finite but overflows inside the trace code.
        let finite = |tr: &TraceResult| {
            tr.fraction.is_finite() && tr.end_pos.is_finite() && tr.plane_normal.is_finite()
        };
        let p = mins + (maxs - mins) * 0.5;
        for bad in [Vec3::NAN, Vec3::splat(1e30), Vec3::new(0.0, -1e30, 0.0)] {
            for hull in [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched] {
                for (from, to) in [(bad, end), (start, bad), (bad, bad)] {
                    let tr = eng.trace_hull(from, to, hull);
                    assert!(finite(&tr), "trace_hull {from} -> {to}: {tr:?}");
                    assert!(
                        tr.fraction == 1.0 && !tr.hit_solid && !tr.start_solid,
                        "{tr:?}"
                    );
                    for model in [0, 1] {
                        let tr = eng.trace_model(model, Vec3::ZERO, from, to, hull);
                        assert!(finite(&tr), "trace_model {model} {from} -> {to}: {tr:?}");
                        assert!(tr.fraction == 1.0 && !tr.hit_solid, "{tr:?}");
                    }
                }
                let tr = eng.trace_model(1, bad, start, end, hull);
                assert!(
                    finite(&tr) && tr.fraction == 1.0,
                    "trace_model at {bad}: {tr:?}"
                );
                assert_eq!(eng.point_contents(bad, hull), CONTENTS_EMPTY);
            }
            assert!(!eng.point_in_model(1, Vec3::ZERO, bad));
            assert!(!eng.point_in_model(1, bad, p));
            assert!(!eng.can_see(bad, start - 16.0, start + 16.0));
            assert!(!eng.can_see(start, bad, start + 16.0));
            assert!(!eng.can_see(start, start - 16.0, bad));
        }
    }
}
