//! Traces that hit linked entities: the movement clipping of the original server
//! (`SV_Move` and what it calls), on top of the world hull traces of `collision.rs` and the
//! area tree of `world.rs`.
//!
//! Ported from ReHLDS `rehlds/engine/world.cpp` (`SV_HullForBox`, `SV_HullForBsp`,
//! `SV_HullForEntity`, `SV_LinkContents`, `SV_PointContents`, `SV_TestEntityPosition`,
//! `SV_SingleClipMoveToEntity`, `SV_ClipMoveToEntity`, `SV_ClipToLinks`, `SV_MoveBounds`,
//! `SV_Move`), `rehlds/engine/pr_cmds.cpp` (`PF_traceline_DLL`, `TraceHull`, `TraceModel`) and
//! `rehlds/engine/mathlib.cpp` (`AngleVectors`, `AngleVectorsTranspose`), read 2026-11-21.
//!
//! Left out, on purpose:
//! - `pfnShouldCollide` of the game, the `groupinfo` filters and `FMOVE_IGNORE_GLASS`: the
//!   engine has no such hook, field or flag yet.
//! - `SV_CheckSphereIntersection` (`rehlds/engine/r_studio.cpp`, read 2026-11-21): called from
//!   `SV_ClipToLinks` for entities that are not `SOLID_SLIDEBOX`, or, with `REHLDS_FIXES`, for
//!   all when `sv_force_ent_intersection` is on. It never rejects an entity without
//!   `FL_CLIENT`; for a client it rejects when the move misses a sphere around the player's
//!   current sequence bounding box. Players are `SOLID_SLIDEBOX` in CS 1.6, so by default it
//!   never runs and skipping it only adds exact clips. The cvar's widening to players would
//!   need the studio sequence bounding boxes, which the engine does not read.
//! - `SV_HullForStudioModel` (`r_studio.cpp`): in the original a zero-size trace (every bullet)
//!   against an entity with a studio model clips against its hitbox hulls (unless
//!   `FTRACE_SIMPLEBOX`; for clients only when `sv_clienttrace != 0`) and the result carries a
//!   `hitgroup`. Here every studio entity is clipped as its bounding box and there is no
//!   `hitgroup`: no headshots or damage zones yet (parity registry `studio_hitbox_traces`;
//!   the weapons of phase 2D need it).
//!
//! Where the original stops the server (`Sys_Error`: a `SOLID_BSP` entity without a brush
//! model or without `MOVETYPE_PUSH`/`MOVETYPE_PUSHSTEP`), the entity is not hit.
//!
//! Deviations kept on purpose:
//! - BSP hulls (the world and brush entities) go through `WorldCollision::trace_local`, which
//!   keeps the phase 2A behaviour: a trace that never leaves solid reports fraction 0 and
//!   `end_pos = start`, and the "back up out of solid" step moves 0.05 of the segment, at most
//!   8 times (the source: fraction 1 and `end`; 0.1 and no cap).
//! - Box-hull entities are new here, so they follow the source: a trace that starts and ends
//!   inside the box is `all_solid` and `start_solid` with fraction 1 and `end_pos = end`.
//! - `EntTrace::ent` is `None` for a miss, as the null `trace.ent` of `SV_Move` and `TraceHull`;
//!   [`EngineApi::trace_line`] alone maps it to the world, as `PF_traceline_DLL` does
//!   (`SV_SetGlobalTrace`).

use glam::Vec3;

use crate::api_entities::{in_world, refused_trace};
use crate::collision::{
    BoxHull, CONTENTS_CURRENT_0, CONTENTS_CURRENT_DOWN, CONTENTS_EMPTY, CONTENTS_SOLID,
    CONTENTS_WATER, Geometry, Hull, NO_COLLISION, TraceResult,
};
use crate::edict::{EdictId, EntVars};
use crate::game::EngineApi;
use crate::world::consts::{
    FL_MONSTER, FL_MONSTERCLIP, MOVETYPE_PUSH, MOVETYPE_PUSHSTEP, SOLID_BSP, SOLID_NOT,
    SOLID_TRIGGER,
};

/// How a trace treats monsters (`MOVE_NORMAL`, `MOVE_NOMONSTERS`, `MOVE_MISSILE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveFilter {
    /// Everything solid is hit.
    Normal,
    /// Every entity that is not a brush model is ignored, except those with
    /// `MOVETYPE_PUSHSTEP` (the original's rule, as the code has it).
    NoMonsters,
    /// As `Normal`, but entities with `FL_MONSTER` are hit with a box of +-15 around the moving
    /// one, which also widens the area searched for entities.
    Missile,
}

/// A trace and the entity it hit.
#[derive(Debug, Clone, Copy)]
pub struct EntTrace {
    pub trace: TraceResult,
    /// The entity the trace was stopped by (the world for the map itself), `None` when
    /// nothing stopped it. Also set when the trace starts inside a solid.
    pub ent: Option<EdictId>,
}

impl EntTrace {
    fn miss(end: Vec3) -> Self {
        Self {
            trace: TraceResult::clear(end),
            ent: None,
        }
    }
}

/// Counted work of one [`EngineApi::move_counted`] call.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MoveStats {
    /// Calls of the exact clip against a linked entity (`SV_ClipMoveToEntity`), the world's
    /// own clip not included.
    pub(crate) entity_clips: usize,
}

/// `AngleVectors`.
fn angle_vectors(angles: Vec3) -> (Vec3, Vec3, Vec3) {
    let (sy, cy, sp, cp, sr, cr) = sines(angles);
    let forward = Vec3::new(cp * cy, cp * sy, -sp);
    let right = Vec3::new(-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp);
    let up = Vec3::new(cr * sp * cy + -sr * -sy, cr * sp * sy + -sr * cy, cr * cp);
    (forward, right, up)
}

/// `AngleVectorsTranspose`.
fn angle_vectors_transpose(angles: Vec3) -> (Vec3, Vec3, Vec3) {
    let (sy, cy, sp, cp, sr, cr) = sines(angles);
    let forward = Vec3::new(cp * cy, sr * sp * cy + cr * -sy, cr * sp * cy + -sr * -sy);
    let right = Vec3::new(cp * sy, sr * sp * sy + cr * cy, cr * sp * sy + -sr * cy);
    let up = Vec3::new(-sp, sr * cp, cr * cp);
    (forward, right, up)
}

/// Sine and cosine of yaw, pitch and roll, computed in double as the original does.
fn sines(angles: Vec3) -> (f32, f32, f32, f32, f32, f32) {
    let sc = |degrees: f32| {
        let angle = (f64::from(degrees) * (std::f64::consts::PI * 2.0 / 360.0)) as f32;
        let angle = f64::from(angle);
        (angle.sin() as f32, angle.cos() as f32)
    };
    let (sy, cy) = sc(angles.y);
    let (sp, cp) = sc(angles.x);
    let (sr, cr) = sc(angles.z);
    (sy, cy, sp, cp, sr, cr)
}

/// The brush model number of a `*N` model name.
fn brush_model(v: &EntVars) -> Option<usize> {
    v.model.strip_prefix('*')?.parse().ok()
}

/// `SV_HullForBsp`'s choice of the BSP hull by the size of the moving box.
fn bsp_hull_for(mins: Vec3, maxs: Vec3) -> Hull {
    let size = maxs - mins;
    if size.x <= 8.0 {
        Hull::Point
    } else if size.x <= 36.0 {
        if size.z <= 36.0 {
            Hull::Crouched
        } else {
            Hull::Standing
        }
    } else {
        Hull::Large
    }
}

/// The offset `SV_HullForBsp` gives: the box's `mins` corner is moved onto the hull's
/// `clip_mins` (nothing for the point hull), then the model's `origin` is added.
fn bsp_offset(hull: Hull, mins: Vec3, origin: Vec3) -> Vec3 {
    if hull == Hull::Point {
        origin
    } else {
        hull.clip_mins() - mins + origin
    }
}

/// `SV_MoveBounds`: the box that holds the whole move.
fn move_bounds(start: Vec3, mins: Vec3, maxs: Vec3, end: Vec3) -> (Vec3, Vec3) {
    let mut boxmins = Vec3::ZERO;
    let mut boxmaxs = Vec3::ZERO;
    for i in 0..3 {
        if end[i] > start[i] {
            boxmins[i] = start[i] + mins[i] - 1.0;
            boxmaxs[i] = end[i] + maxs[i] + 1.0;
        } else {
            boxmins[i] = end[i] + mins[i] - 1.0;
            boxmaxs[i] = start[i] + maxs[i] + 1.0;
        }
    }
    (boxmins, boxmaxs)
}

impl EngineApi<'_> {
    /// `PF_traceline_DLL`: traces a point from `start` to `end` through the world and the
    /// solid entities. `pass` (the world when `None`) and the entities it owns, or is owned
    /// by, are not hit. Unlike the other traces, a miss reports the world as `ent`
    /// (`SV_SetGlobalTrace` does). Input outside the world (non-finite, or a coordinate beyond
    /// `MAX_WORLD_COORD`) is refused: nothing is hit and `end_pos` is `start` (the origin when
    /// `start` is itself outside the world).
    pub fn trace_line(
        &self,
        start: Vec3,
        end: Vec3,
        filter: MoveFilter,
        pass: Option<EdictId>,
    ) -> EntTrace {
        let pass = pass.unwrap_or_else(|| self.world());
        let mut t = self.trace_box(start, Vec3::ZERO, Vec3::ZERO, end, filter, Some(pass));
        if t.ent.is_none() && in_world(start) && in_world(end) {
            t.ent = Some(self.world());
        }
        t
    }

    /// `TraceHull`: traces the box of `hull` (see [`Hull::half_extents`]) from `start` to
    /// `end`. Rules as [`trace_line`](Self::trace_line), except that no `pass` means no
    /// entity is passed (the original hands the null pointer on).
    pub fn trace_hull_ents(
        &self,
        start: Vec3,
        end: Vec3,
        hull: Hull,
        filter: MoveFilter,
        pass: Option<EdictId>,
    ) -> EntTrace {
        let h = hull.half_extents();
        self.trace_box(start, -h, h, end, filter, pass)
    }

    /// `SV_Move`: traces the box `mins..maxs` (relative to the moving point) from `start` to
    /// `end`. Rules as [`trace_hull_ents`](Self::trace_hull_ents); `mins` and `maxs` must be
    /// in the world too.
    pub fn trace_box(
        &self,
        start: Vec3,
        mins: Vec3,
        maxs: Vec3,
        end: Vec3,
        filter: MoveFilter,
        pass: Option<EdictId>,
    ) -> EntTrace {
        self.move_counted(start, mins, maxs, end, filter, pass, false)
            .0
    }

    /// `PF_TraceModel` (`SV_ClipMoveToEntity`): traces the box of `hull` from `start` to `end`
    /// against entity `id` alone, whatever its solidity. A brush-model entity is clipped as a
    /// `SOLID_BSP` one. A stale `id` or input outside the world hits nothing.
    pub fn trace_entity(&self, id: EdictId, start: Vec3, end: Vec3, hull: Hull) -> EntTrace {
        if !in_world(start) || !in_world(end) {
            return EntTrace {
                trace: refused_trace(start),
                ent: None,
            };
        }
        let h = hull.half_extents();
        self.clip_to_entity(id, start, -h, h, end, true)
    }

    /// `SV_PointContents`: the contents (`CONTENTS_*`) at `p`, the world's overridden by a
    /// non-solid brush entity that holds `p` (`func_water` and the like, whose `skin` is the
    /// contents; `SV_LinkContents`). The current contents of the world count as water.
    /// `CONTENTS_EMPTY` for a `p` outside the world.
    pub fn point_contents_ents(&self, p: Vec3) -> i32 {
        if !in_world(p) {
            return CONTENTS_EMPTY;
        }
        let mut cont = self
            .map
            .collision()
            .map_or(CONTENTS_EMPTY, |c| c.point_contents(p, Hull::Point));
        if (CONTENTS_CURRENT_DOWN..=CONTENTS_CURRENT_0).contains(&cont) {
            cont = CONTENTS_WATER;
        } else if cont == CONTENTS_SOLID {
            return CONTENTS_SOLID;
        }
        match self.link_contents(p) {
            CONTENTS_EMPTY => cont,
            entity_contents => entity_contents,
        }
    }

    /// `SV_TestEntityPosition`: the entity (the world for the map) that entity `id` stands in,
    /// found by a move of its box from its origin to its origin; `None` when it is free, for
    /// a stale id, and for an entity whose origin or box is outside the world.
    pub fn test_entity_position(&self, id: EdictId) -> Option<EdictId> {
        let v = self.edicts.get(id).map(|e| &e.vars)?;
        let monster_clip = v.flags & FL_MONSTERCLIP != 0;
        let (origin, mins, maxs) = (v.origin, v.mins, v.maxs);
        let (trace, _) = self.move_counted(
            origin,
            mins,
            maxs,
            origin,
            MoveFilter::Normal,
            Some(id),
            monster_clip,
        );
        if trace.trace.start_solid {
            trace.ent
        } else {
            None
        }
    }

    /// `SV_LinkContents`: the `skin` of the first non-solid brush entity on the way down the
    /// area tree to `pos` that holds `pos`, `CONTENTS_EMPTY` when there is none.
    fn link_contents(&self, pos: Vec3) -> i32 {
        let Some(collision) = self.map.collision() else {
            return CONTENTS_EMPTY;
        };
        for id in self.world.solids_along_point(self.edicts, pos) {
            let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
                continue;
            };
            if v.solid != SOLID_NOT {
                continue;
            }
            let Some(model) = brush_model(v).filter(|&m| m < collision.model_count()) else {
                continue;
            };
            if !(v.absmin.cmple(pos).all() && pos.cmple(v.absmax).all()) || !in_world(v.origin) {
                continue;
            }
            // The point hull, shifted by the entity's origin; not rotated.
            if collision.model_point_contents(model, v.origin, pos, Hull::Point) != CONTENTS_EMPTY {
                return i32::from(v.skin);
            }
        }
        CONTENTS_EMPTY
    }

    /// `SV_Move` with its work counted (see [`MoveStats`]).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn move_counted(
        &self,
        start: Vec3,
        mins: Vec3,
        maxs: Vec3,
        end: Vec3,
        filter: MoveFilter,
        pass: Option<EdictId>,
        monster_clip: bool,
    ) -> (EntTrace, MoveStats) {
        let mut stats = MoveStats::default();
        if !in_world(start) || !in_world(end) || !in_world(mins) || !in_world(maxs) {
            return (
                EntTrace {
                    trace: refused_trace(start),
                    ent: None,
                },
                stats,
            );
        }

        // The world first: a move that cannot start is over.
        let world = self.clip_world(start, mins, maxs, end);
        let (mut clip, mut clip_ent) = (world.trace, world.ent);
        if clip.fraction == 0.0 {
            return (world, stats);
        }
        let world_fraction = clip.fraction;
        let end = clip.end_pos;
        clip.fraction = 1.0;

        let (mins2, maxs2) = if filter == MoveFilter::Missile {
            (Vec3::splat(-15.0), Vec3::splat(15.0))
        } else {
            (mins, maxs)
        };
        let (boxmins, boxmaxs) = move_bounds(start, mins2, maxs2, end);

        let pass_vars = pass.and_then(|p| self.edicts.get(p).map(|e| (p.index, &e.vars)));
        let mut candidates = Vec::new();
        self.world
            .solids_in_box(self.edicts, boxmins, boxmaxs, &mut candidates);

        // `SV_ClipToLinks`.
        for id in candidates {
            let Some(touch) = self.edicts.get(id).map(|e| &e.vars) else {
                continue;
            };
            if touch.solid == SOLID_NOT || pass_vars.is_some_and(|(index, _)| index == id.index) {
                continue;
            }
            if touch.solid == SOLID_TRIGGER {
                // The original stops the server for a trigger in the clipping list.
                continue;
            }
            if touch.solid == SOLID_BSP {
                if touch.flags & FL_MONSTERCLIP != 0 && !monster_clip {
                    continue;
                }
            } else if filter == MoveFilter::NoMonsters && touch.movetype != MOVETYPE_PUSHSTEP {
                // Everything that is not a brush is a "monster", pushables apart.
                continue;
            }
            if let Some((_, p)) = pass_vars
                && p.size.x != 0.0
                && touch.size.x == 0.0
            {
                continue; // points never interact
            }
            if clip.all_solid {
                break;
            }
            if let Some((pass_index, p)) = pass_vars
                && (touch.owner.is_some_and(|o| o.index == pass_index)
                    || p.owner.is_some_and(|o| o.index == id.index))
            {
                continue; // own missiles, and the owner
            }

            stats.entity_clips += 1;
            let (cmins, cmaxs) = if touch.flags & FL_MONSTER != 0 {
                (mins2, maxs2)
            } else {
                (mins, maxs)
            };
            let hit = self
                .clip_to_entity(id, start, cmins, cmaxs, end, false)
                .trace;
            if hit.all_solid || hit.start_solid || hit.fraction < clip.fraction {
                let was_start_solid = clip.start_solid;
                clip = hit;
                clip_ent = Some(id);
                if was_start_solid {
                    clip.start_solid = true;
                }
            }
        }

        clip.fraction *= world_fraction;
        clip.hit_solid = clip.fraction < 1.0 || clip.start_solid;
        (
            EntTrace {
                trace: clip,
                ent: clip_ent,
            },
            stats,
        )
    }

    /// `SV_ClipMoveToEntity` of the world edict: the world model through the hull the box
    /// selects.
    fn clip_world(&self, start: Vec3, mins: Vec3, maxs: Vec3, end: Vec3) -> EntTrace {
        let Some(collision) = self.map.collision().filter(|c| c.model_count() > 0) else {
            return EntTrace::miss(end);
        };
        let hull = bsp_hull_for(mins, maxs);
        let offset = bsp_offset(hull, mins, Vec3::ZERO);
        let mut tr = collision.trace_local(
            Geometry::Bsp { model: 0, hull },
            start - offset,
            end - offset,
        );
        tr.end_pos = if tr.fraction != 1.0 {
            start + (end - start) * tr.fraction
        } else {
            end
        };
        let hit = tr.fraction < 1.0 || tr.start_solid;
        EntTrace {
            trace: tr,
            ent: hit.then(|| self.world()),
        }
    }

    /// `SV_SingleClipMoveToEntity`: traces the box `mins..maxs` from `start` to `end` against
    /// entity `id` alone. `force_bsp` is `PF_TraceModel`'s: a brush-model entity is clipped as
    /// `SOLID_BSP` with `MOVETYPE_PUSH`.
    fn clip_to_entity(
        &self,
        id: EdictId,
        start: Vec3,
        mins: Vec3,
        maxs: Vec3,
        end: Vec3,
        force_bsp: bool,
    ) -> EntTrace {
        let miss = EntTrace::miss(end);
        let Some(v) = self.edicts.get(id).map(|e| &e.vars) else {
            return miss;
        };
        // Hostile numbers (set behind the linking's back) hit nothing.
        if !in_world(v.origin) || !in_world(v.mins) || !in_world(v.maxs) || !v.angles.is_finite() {
            return miss;
        }
        let collision = self.map.collision();
        let brush = brush_model(v).filter(|&m| collision.is_some_and(|c| m < c.model_count()));
        let is_bsp = v.solid == SOLID_BSP || (force_bsp && brush.is_some());

        let (mut tr, rotated, boxed) = if is_bsp {
            let pushable = matches!(v.movetype, MOVETYPE_PUSH | MOVETYPE_PUSHSTEP);
            let (Some(model), Some(collision)) = (brush, collision) else {
                return miss;
            };
            if !pushable && !force_bsp {
                return miss;
            }
            let hull = bsp_hull_for(mins, maxs);
            let offset = bsp_offset(hull, mins, v.origin);
            let mut start_l = start - offset;
            let mut end_l = end - offset;
            let rotated = v.angles != Vec3::ZERO;
            if rotated {
                let (forward, right, up) = angle_vectors(v.angles);
                let to_local = |p: Vec3| Vec3::new(p.dot(forward), -p.dot(right), p.dot(up));
                start_l = to_local(start_l);
                end_l = to_local(end_l);
            }
            (
                collision.trace_local(Geometry::Bsp { model, hull }, start_l, end_l),
                rotated,
                false,
            )
        } else {
            // A hull made of the entity's box grown by the moving box.
            let boxhull = BoxHull::new(v.mins - maxs, v.maxs - mins);
            (
                NO_COLLISION.trace_local(Geometry::Box(&boxhull), start - v.origin, end - v.origin),
                false,
                true,
            )
        };
        if boxed && tr.all_solid {
            // The source keeps fraction 1 (and so `end`) for a trace that never leaves solid.
            tr.fraction = 1.0;
        }

        if tr.fraction != 1.0 {
            if rotated {
                let (forward, right, up) = angle_vectors_transpose(v.angles);
                let n = tr.plane_normal;
                tr.plane_normal = Vec3::new(n.dot(forward), n.dot(right), n.dot(up));
            }
            tr.end_pos = start + (end - start) * tr.fraction;
        } else {
            tr.end_pos = end;
        }
        let hit = tr.fraction < 1.0 || tr.start_solid;
        EntTrace {
            trace: tr,
            ent: hit.then_some(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cs_assets::bsp::{BspMap, BspModel, ClipNode, Leaf, LightmapAtlas, Node, Plane};
    use cs_cvar::CVarRegistry;
    use cs_proto48::usermsg::UserMsgRegistry;

    use super::*;
    use crate::edict::EdictPool;
    use crate::game::{ClientView, Outbox};
    use crate::map::ServerMap;
    use crate::precache::PrecacheTables;
    use crate::world::WorldLinks;
    use crate::world::consts::{SOLID_BBOX, SOLID_SLIDEBOX};

    struct Fixture {
        edicts: EdictPool,
        precache: PrecacheTables,
        cvars: cs_cvar::CVarRegistry,
        usermsgs: UserMsgRegistry,
        map: ServerMap,
        world: WorldLinks,
        outbox: Outbox,
        clients: Vec<ClientView>,
    }

    impl Fixture {
        fn on(map: ServerMap, max_edicts: usize) -> Self {
            Self {
                edicts: EdictPool::new(1, max_edicts),
                precache: PrecacheTables::new(&map.path, map.submodel_count),
                cvars: CVarRegistry::new_empty(),
                usermsgs: UserMsgRegistry::cs16_default(),
                world: WorldLinks::new(map.world_bounds()),
                map,
                outbox: Outbox::new(1),
                clients: vec![ClientView::default()],
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
                1.0,
                0.0,
                &mut self.outbox,
                &mut self.clients,
            )
        }
    }

    /// A map whose world is empty space and whose models 1.. are boxes of the given half-size
    /// and inside contents, each with all four hulls (hull `h` is the box grown by the half
    /// size of hull `h`, as the map compiler does).
    fn box_map(boxes: &[(Vec3, i32)]) -> BspMap {
        box_map_with_world(boxes, None)
    }

    /// As [`box_map`], with the world solid for `x < world_solid_x` when that is given.
    fn box_map_with_world(boxes: &[(Vec3, i32)], world_solid_x: Option<f32>) -> BspMap {
        let leaf = |contents: i32| Leaf {
            contents,
            vis_offset: -1,
            mins: [0; 3],
            maxs: [0; 3],
            first_mark_surface: 0,
            num_mark_surfaces: 0,
            ambient_levels: [0; 4],
        };
        let mut planes = Vec::new();
        let mut nodes: Vec<Node> = Vec::new();
        let mut clipnodes: Vec<ClipNode> = Vec::new();
        // Leaf 0 is solid, leaf 1 empty, leaf 2 + k is the inside of box k.
        let mut leafs = vec![leaf(CONTENTS_SOLID), leaf(CONTENTS_EMPTY)];
        let mut models = vec![BspModel {
            mins: Vec3::splat(-1000.0),
            maxs: Vec3::splat(1000.0),
            origin: Vec3::ZERO,
            head_nodes: [-2, CONTENTS_EMPTY, CONTENTS_EMPTY, CONTENTS_EMPTY],
            vis_leafs: 1,
            first_face: 0,
            num_faces: 0,
        }];
        if let Some(x) = world_solid_x {
            planes.push(Plane {
                normal: Vec3::X,
                dist: x,
                plane_type: 0,
            });
            nodes.push(Node {
                plane: 0,
                children: [-2, -1],
                mins: [0; 3],
                maxs: [0; 3],
                first_face: 0,
                num_faces: 0,
            });
            clipnodes.push(ClipNode {
                plane: 0,
                children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16],
            });
            models[0].head_nodes = [0; 4];
        }
        for (k, &(half, inside)) in boxes.iter().enumerate() {
            leafs.push(leaf(inside));
            let mut head_nodes = [0i32; 4];
            for hull in [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched] {
                let grown = half + hull.half_extents();
                let plane_base = planes.len() as i32;
                for i in 0..6 {
                    let axis = i >> 1;
                    let mut normal = Vec3::ZERO;
                    normal[axis] = 1.0;
                    let dist = if i & 1 == 0 {
                        grown[axis]
                    } else {
                        -grown[axis]
                    };
                    planes.push(Plane {
                        normal,
                        dist,
                        plane_type: axis as i32,
                    });
                }
                let first = if hull == Hull::Point {
                    nodes.len()
                } else {
                    clipnodes.len()
                } as i16;
                let outside = if hull == Hull::Point {
                    -2
                } else {
                    CONTENTS_EMPTY as i16
                };
                let inside_child = if hull == Hull::Point {
                    -(2 + k as i16) - 1
                } else {
                    inside as i16
                };
                for i in 0..6i16 {
                    let side = (i & 1) as usize;
                    let mut children = [0i16; 2];
                    children[side] = outside;
                    children[side ^ 1] = if i != 5 { first + i + 1 } else { inside_child };
                    if hull == Hull::Point {
                        nodes.push(Node {
                            plane: (plane_base + i32::from(i)) as u32,
                            children,
                            mins: [0; 3],
                            maxs: [0; 3],
                            first_face: 0,
                            num_faces: 0,
                        });
                    } else {
                        clipnodes.push(ClipNode {
                            plane: plane_base + i32::from(i),
                            children,
                        });
                    }
                }
                head_nodes[hull as usize] = i32::from(first);
            }
            models.push(BspModel {
                mins: -half,
                maxs: half,
                origin: Vec3::ZERO,
                head_nodes,
                vis_leafs: 0,
                first_face: 0,
                num_faces: 0,
            });
        }
        BspMap {
            name: "boxes".into(),
            skyname: String::new(),
            entities: Vec::new(),
            spawns: Vec::new(),
            hostage_spawns: Vec::new(),
            brush_entities: Vec::new(),
            planes,
            vertices_raw: Vec::new(),
            visibility: Vec::new(),
            nodes,
            texinfos: Vec::new(),
            faces: Vec::new(),
            lighting: Vec::new(),
            clipnodes,
            leafs,
            mark_surfaces: Vec::new(),
            edges: Vec::new(),
            surf_edges: Vec::new(),
            models,
            textures: Vec::new(),
            lightmap_atlas: LightmapAtlas {
                width: 0,
                height: 0,
                rgba: Vec::new(),
                packed_faces: 0,
            },
            render_vertices: Vec::new(),
            render_indices: Vec::new(),
            batches: Vec::new(),
        }
    }

    /// Model 1: a slab 64 long in x, 16 wide in y and z. Model 2: a water cube of side 64.
    fn slab_fixture() -> Fixture {
        let bsp = box_map(&[
            (Vec3::new(32.0, 8.0, 8.0), CONTENTS_SOLID),
            (Vec3::splat(32.0), CONTENTS_WATER),
        ]);
        Fixture::on(ServerMap::with_bsp_for_test(Arc::new(bsp)), 64)
    }

    fn brush(eng: &mut EngineApi<'_>, model: &str, origin: Vec3, angles: Vec3) -> EdictId {
        let id = eng.create_entity("func_wall").expect("free edict");
        assert!(eng.set_model(id, model));
        let v = eng.ent_mut(id).unwrap();
        v.solid = SOLID_BSP;
        v.movetype = MOVETYPE_PUSH;
        v.angles = angles;
        eng.set_origin(id, origin);
        id
    }

    fn boxed(eng: &mut EngineApi<'_>, solid: u8, origin: Vec3, half: f32) -> EdictId {
        let id = eng.create_entity("test_box").expect("free edict");
        eng.ent_mut(id).unwrap().solid = solid;
        eng.set_size(id, Vec3::splat(-half), Vec3::splat(half));
        eng.set_origin(id, origin);
        id
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.001
    }

    #[test]
    fn moved_slab_collides_at_its_origin() {
        let mut f = slab_fixture();
        let mut eng = f.api();
        let (start, end) = (Vec3::new(-100.0, 0.0, 64.0), Vec3::new(100.0, 0.0, 64.0));
        // Nothing at z = 64 while the slab is at the origin.
        let slab = brush(&mut eng, "*1", Vec3::ZERO, Vec3::ZERO);
        let t = eng.trace_line(start, end, MoveFilter::Normal, None);
        assert_eq!(t.trace.fraction, 1.0);
        assert_eq!(t.ent, Some(eng.world()));
        // Moved up by 64, it is hit at its -x face.
        eng.set_origin(slab, Vec3::new(0.0, 0.0, 64.0));
        let t = eng.trace_line(start, end, MoveFilter::Normal, None);
        assert_eq!(t.ent, Some(slab));
        assert!(near(t.trace.end_pos.x, -32.0 - 0.03125), "{:?}", t.trace);
        assert!(near(t.trace.fraction, (68.0 - 0.03125) / 200.0));
        assert_eq!(t.trace.plane_normal, Vec3::NEG_X);
        // The standing hull sees hull 1, grown by 16 in x.
        let t = eng.trace_hull_ents(start, end, Hull::Standing, MoveFilter::Normal, None);
        assert_eq!(t.ent, Some(slab));
        assert!(near(t.trace.end_pos.x, -48.0 - 0.03125), "{:?}", t.trace);
    }

    /// A brush entity far from the origin whose surface is in the local frame where the world
    /// is solid: the backup step must test the entity's own hull, not the world's.
    #[test]
    fn brush_trace_does_not_back_up_into_the_world_tree() {
        let bsp = box_map_with_world(&[(Vec3::new(32.0, 8.0, 8.0), CONTENTS_SOLID)], Some(-20.0));
        let mut f = Fixture::on(ServerMap::with_bsp_for_test(Arc::new(bsp)), 64);
        let mut eng = f.api();
        let slab = brush(&mut eng, "*1", Vec3::new(500.0, 0.0, 0.0), Vec3::ZERO);
        // The slab's -x face is at local x = -32, which is world-solid (x < -20) in the local
        // frame the trace runs in.
        let t = eng.trace_entity(
            slab,
            Vec3::new(400.0, 0.0, 0.0),
            Vec3::new(600.0, 0.0, 0.0),
            Hull::Point,
        );
        assert_eq!(t.ent, Some(slab));
        let face = 468.0 - 0.03125;
        assert!(near(t.trace.end_pos.x, face), "{:?}", t.trace);
        assert!(
            near(t.trace.fraction, (face - 400.0) / 200.0),
            "{:?}",
            t.trace
        );
    }

    #[test]
    fn rotated_slab_collides_rotated() {
        let mut f = slab_fixture();
        let mut eng = f.api();
        let slab = brush(&mut eng, "*1", Vec3::ZERO, Vec3::new(0.0, 90.0, 0.0));
        // Long in y now: along y it is hit at +-32, along x at +-8.
        let t = eng.trace_line(
            Vec3::new(0.0, -100.0, 0.0),
            Vec3::new(0.0, 100.0, 0.0),
            MoveFilter::Normal,
            None,
        );
        assert_eq!(t.ent, Some(slab));
        assert!(near(t.trace.end_pos.y, -32.0 - 0.03125), "{:?}", t.trace);
        assert!(
            (t.trace.plane_normal - Vec3::NEG_Y).length() < 1e-5,
            "{:?}",
            t.trace.plane_normal
        );
        let t = eng.trace_line(
            Vec3::new(-100.0, 0.0, 0.0),
            Vec3::new(100.0, 0.0, 0.0),
            MoveFilter::Normal,
            None,
        );
        assert!(near(t.trace.end_pos.x, -8.0 - 0.03125), "{:?}", t.trace);
        assert!((t.trace.plane_normal - Vec3::NEG_X).length() < 1e-5);
        // x = 20 is inside the unrotated slab's x range, outside the rotated one's.
        let t = eng.trace_line(
            Vec3::new(20.0, -100.0, 0.0),
            Vec3::new(20.0, 100.0, 0.0),
            MoveFilter::Normal,
            None,
        );
        assert_eq!(t.ent, Some(eng.world()));
        // `trace_entity` agrees with the move.
        let t = eng.trace_entity(
            slab,
            Vec3::new(0.0, -100.0, 0.0),
            Vec3::new(0.0, 100.0, 0.0),
            Hull::Point,
        );
        assert!(near(t.trace.end_pos.y, -32.0 - 0.03125));
    }

    #[test]
    fn monster_clip_brushes_need_the_flag() {
        let mut f = slab_fixture();
        let mut eng = f.api();
        let slab = brush(&mut eng, "*1", Vec3::ZERO, Vec3::ZERO);
        eng.ent_mut(slab).unwrap().flags |= FL_MONSTERCLIP;
        let (start, end) = (Vec3::new(-100.0, 0.0, 0.0), Vec3::new(100.0, 0.0, 0.0));
        let t = eng.trace_line(start, end, MoveFilter::Normal, None);
        assert_eq!(
            t.ent,
            Some(eng.world()),
            "an ordinary trace goes through monster clip"
        );
        // An entity with the flag that stands in it finds it.
        let monster = boxed(&mut eng, SOLID_SLIDEBOX, Vec3::ZERO, 4.0);
        assert_eq!(eng.test_entity_position(monster), None);
        eng.ent_mut(monster).unwrap().flags |= FL_MONSTERCLIP;
        assert_eq!(eng.test_entity_position(monster), Some(slab));
    }

    #[test]
    fn water_brush_sets_point_contents() {
        let mut f = slab_fixture();
        let mut eng = f.api();
        let water = eng.create_entity("func_water").unwrap();
        assert!(eng.set_model(water, "*2"));
        {
            let v = eng.ent_mut(water).unwrap();
            v.solid = SOLID_NOT;
            v.movetype = MOVETYPE_PUSH;
            v.skin = CONTENTS_WATER as i16;
        }
        eng.set_origin(water, Vec3::new(500.0, 0.0, 0.0));
        let inside = Vec3::new(500.0, 10.0, -10.0);
        assert_eq!(eng.point_contents_ents(inside), CONTENTS_WATER);
        assert_eq!(
            eng.point_contents_ents(Vec3::new(500.0, 40.0, 0.0)),
            CONTENTS_EMPTY
        );
        assert_eq!(eng.point_contents_ents(Vec3::ZERO), CONTENTS_EMPTY);
        // The world's own answer is still the world's.
        assert_eq!(eng.point_contents(inside, Hull::Point), CONTENTS_EMPTY);
        // Not solid for traces.
        let t = eng.trace_line(
            Vec3::new(400.0, 0.0, 0.0),
            Vec3::new(600.0, 0.0, 0.0),
            MoveFilter::Normal,
            None,
        );
        assert_eq!(t.ent, Some(eng.world()));
        assert_eq!(
            eng.point_contents_ents(Vec3::splat(f32::NAN)),
            CONTENTS_EMPTY
        );
    }

    #[test]
    fn missile_widens_monsters_only() {
        let mut f = Fixture::on(ServerMap::stub("t"), 64);
        let mut eng = f.api();
        let monster = boxed(&mut eng, SOLID_SLIDEBOX, Vec3::new(100.0, 40.0, 0.0), 16.0);
        eng.ent_mut(monster).unwrap().flags |= FL_MONSTER;
        let wall = boxed(&mut eng, SOLID_BBOX, Vec3::new(100.0, -40.0, 0.0), 16.0);
        let (start, end) = (Vec3::ZERO, Vec3::new(200.0, 0.0, 0.0));
        let t = eng.trace_line(start, end, MoveFilter::Normal, None);
        assert_eq!(t.ent, Some(eng.world()));
        // y = 0 is 24 from both boxes' near faces; the +-15 box is short of that.
        let t = eng.trace_line(start, end, MoveFilter::Missile, None);
        assert_eq!(t.ent, Some(eng.world()), "15 is short of 24");
        eng.set_origin(monster, Vec3::new(100.0, 30.0, 0.0));
        let t = eng.trace_line(start, end, MoveFilter::Missile, None);
        assert_eq!(t.ent, Some(monster), "y 14..46 grown by 15 holds y = 0");
        let t = eng.trace_line(start, end, MoveFilter::Normal, None);
        assert_eq!(t.ent, Some(eng.world()));
        // The wall is no monster: it is not widened.
        eng.set_origin(wall, Vec3::new(100.0, -30.0, 0.0));
        let t = eng.trace_line(start, end, MoveFilter::Missile, None);
        assert_eq!(t.ent, Some(monster));
    }

    /// The per-entity clips of one trace never exceed the entities whose absolute boxes
    /// overlap the move bounds, however many there are or what is wrong with them.
    #[test]
    fn clips_are_bounded_by_overlapping_entities() {
        let mut f = Fixture::on(ServerMap::stub("t"), 1100);
        let mut eng = f.api();
        let mut ids = Vec::new();
        // A thousand boxes in one place, plus hostile ones.
        while ids.len() < 1000 {
            let Some(id) = eng.create_entity("crowd") else {
                break;
            };
            eng.ent_mut(id).unwrap().solid = SOLID_BBOX;
            eng.set_size(id, Vec3::splat(-8.0), Vec3::splat(8.0));
            eng.set_origin(id, Vec3::new(100.0, 0.0, 0.0));
            ids.push(id);
        }
        assert!(ids.len() >= 900, "{}", ids.len());
        let nan = ids[0];
        eng.ent_mut(nan).unwrap().origin = Vec3::splat(f32::NAN);
        eng.ent_mut(nan).unwrap().mins = Vec3::splat(f32::NAN);
        let huge = ids[1];
        eng.ent_mut(huge).unwrap().origin = Vec3::splat(1.0e30);
        eng.ent_mut(huge).unwrap().maxs = Vec3::splat(f32::INFINITY);
        let big = ids[2];
        eng.set_origin(big, Vec3::splat(100_000.0));
        eng.ent_mut(big).unwrap().mins = Vec3::splat(-1.0e9);
        eng.link_entity(big, false);

        let (start, end) = (Vec3::ZERO, Vec3::new(200.0, 0.0, 0.0));
        let (t, stats) = eng.move_counted(
            start,
            Vec3::ZERO,
            Vec3::ZERO,
            end,
            MoveFilter::Normal,
            None,
            false,
        );
        assert!(t.trace.fraction.is_finite() && t.trace.end_pos.is_finite());
        let (bmins, bmaxs) = move_bounds(start, Vec3::ZERO, Vec3::ZERO, end);
        let overlapping = eng.entities_in_box(bmins, bmaxs).len();
        assert!(overlapping >= ids.len() - 3, "{overlapping}");
        assert!(stats.entity_clips <= overlapping, "{stats:?} {overlapping}");
        assert!(stats.entity_clips > 0);
        assert!(t.ent.is_some_and(|e| e.index != 0));

        // A move away from the crowd: only what the world clamp puts everywhere is clipped.
        let (start, end) = (Vec3::new(0.0, 500.0, 0.0), Vec3::new(200.0, 500.0, 0.0));
        let (_, stats) = eng.move_counted(
            start,
            Vec3::ZERO,
            Vec3::ZERO,
            end,
            MoveFilter::Normal,
            None,
            false,
        );
        let (bmins, bmaxs) = move_bounds(start, Vec3::ZERO, Vec3::ZERO, end);
        let overlapping = eng.entities_in_box(bmins, bmaxs).len();
        assert!(overlapping <= 2, "{overlapping}");
        assert!(stats.entity_clips <= overlapping);
    }
}
