//! The services the engine gives the player movement code (`pmove`), here without an engine: the entities the player
//! can collide with (`physents`, `moveents`), traces of the player's hull through them,
//! position tests, point contents, ladders and the trace against a ladder model.
//!
//! Ported from ReHLDS `rehlds/engine/pmovetst.cpp` (`PM_RecursiveHullCheck`,
//! `PM_HullPointContents`, `PM_HullForBox`, `PM_HullForBsp`, `_PM_PlayerTrace`,
//! `_PM_TestPlayerPosition`, `PM_LinkContents`, `PM_PointContents`, `PM_TraceModel`),
//! `rehlds/engine/sv_user.cpp` (`SV_CopyEdictToPhysent`, in `cs-engine`) and `rehlds/engine/pmove.cpp` (the player hull tables), all read
//! 2026-11-22. The BSP walks go through the geometry primitives of `collision.rs` (hull nodes,
//! leaf contents, box hulls). The server builds its `physents` from edicts (`cs-engine`), a
//! client from its entity states; both run the traces below.
//!
//! The vanilla branch of the sources is the target, and the original client predicts with the
//! same code, so the quirks are kept (each is named where it is ported):
//! - a trace that hits nothing leaves the plane normal at zero and `ent` at -1;
//! - `PM_TestPlayerPosition` fills its trace from the player's own origin, not from the
//!   position it tests;
//! - an equal fraction never replaces the earlier physent (`<`, not `<=`).
//!
//! Deviations and gaps, on purpose:
//! - Studio hitbox hulls (`PM_HullForStudioModel`, `STUDIO_TRACE_HITBOX`, `PM_STUDIO_BOX`) are
//!   not built. The original reaches them only for a `SOLID_BBOX` entity whose model carries
//!   the `STUDIO_TRACE_HITBOX` flag (`SV_CopyEdictToPhysent` is the only place that sets
//!   `physent_t::studiomodel`); a player (`SOLID_SLIDEBOX`) never does, and the engine reads no
//!   model flags. Such an entity is traced as its box, which is what the original does for a
//!   model without the flag. `PM_STUDIO_IGNORE` and `PM_STUDIO_BOX` therefore change nothing.
//! - `visents` and `PM_TraceLine` (`PM_TRACELINE_PHYSENTSONLY` / `PM_TRACELINE_ANYVISIBLE`)
//!   trace against solid physents or visible entities (`cl_pmove.c` / `pmovetst.cpp`).
//! - The `groupinfo` filter and the `SV_AllowPhysent` hook do not exist in this engine.
//! - Where the original fills a physent field it does not set for the entity's solid kind, the
//!   array element keeps what an earlier command left there (stale); here such a field is zero.
//!   Only `mins` and `maxs` of brush physents are concerned, and nothing reads them.
//! - When the 600th physent is taken, the original stops the walk of the area-tree node it is
//!   in and goes on with the other nodes; here the walk stops. A map with 600 solid entities
//!   near one player does not exist.
//! - A hull whose model is out of range, an entity with a non-finite origin or box, and a
//!   `usehull` outside `0..=3` (the original reads past `player_mins`) hit nothing.
//! - `PM_TraceModel` hands its trace to `SV_RecursiveHullCheck` uninitialised in the original
//!   (`trace_t trace;` in `PM_LadderMove`). Assumption: it is initialised as
//!   `SV_SingleClipMoveToEntity` does (fraction 1, nothing hit), and the recursion is the
//!   engine's `WorldCollision::trace_local` with its documented deviations (see
//!   `collision.rs`), except that a trace that never leaves solid keeps fraction 1.
//! - A world without a model (a stub map): `PM_PointContents` reads the world model
//!   unguarded in the original (it tests the pointer against a magic value). Assumption: such
//!   a world is empty space, as `point_contents` of the engine says.

use glam::Vec3;

use crate::collision::{
    BoxHull, CONTENTS_CURRENT_0, CONTENTS_CURRENT_DOWN, CONTENTS_EMPTY, CONTENTS_SOLID,
    CONTENTS_WATER, DIST_EPSILON, Geometry, Hull, WorldCollision, in_world,
};
use cs_assets::bsp::Plane;

/// `SOLID_BSP` (`const.h`): the only solid kind whose angles rotate its hull.
const SOLID_BSP: i32 = 4;

/// `PM_NORMAL` (`pm_defs.h`): no trace flag.
pub const PM_NORMAL: i32 = 0;
/// `PM_STUDIO_IGNORE`: ignore studio-model entities (no effect here, see the module doc).
pub const PM_STUDIO_IGNORE: i32 = 1;
/// `PM_STUDIO_BOX`: use boxes for studio models (no effect here, see the module doc).
pub const PM_STUDIO_BOX: i32 = 2;
/// `PM_GLASS_IGNORE`: ignore entities whose `rendermode` is not `kRenderNormal`.
pub const PM_GLASS_IGNORE: i32 = 4;
/// `PM_WORLD_ONLY`: trace the world only.
pub const PM_WORLD_ONLY: i32 = 8;

/// `PM_TRACELINE_PHYSENTSONLY` (`pm_defs.h`): `PM_TraceLine` traces against `physents`.
pub const PM_TRACELINE_PHYSENTSONLY: i32 = 0;
/// `PM_TRACELINE_ANYVISIBLE` (`pm_defs.h`): `PM_TraceLine` traces against `visents`.
pub const PM_TRACELINE_ANYVISIBLE: i32 = 1;

/// `CONTENTS_LADDER`: the `skin` of a `func_ladder`.
pub const CONTENTS_LADDER: i32 = -16;

/// `MAX_PHYSENTS` (`pm_defs.h`).
pub const MAX_PHYSENTS: usize = 600;
/// `MAX_MOVEENTS` (`pm_defs.h`).
pub const MAX_MOVEENTS: usize = 64;

/// The player hulls of `rehlds/engine/pmove.cpp` (`player_mins`), indexed by `usehull`:
/// 0 standing, 1 ducking, 2 point, 3 large.
pub const PM_PLAYER_MINS: [Vec3; 4] = [
    Vec3::new(-16.0, -16.0, -36.0),
    Vec3::new(-16.0, -16.0, -18.0),
    Vec3::new(0.0, 0.0, 0.0),
    Vec3::new(-32.0, -32.0, -32.0),
];
/// The player hulls of `rehlds/engine/pmove.cpp` (`player_maxs`), see [`PM_PLAYER_MINS`].
pub const PM_PLAYER_MAXS: [Vec3; 4] = [
    Vec3::new(16.0, 16.0, 36.0),
    Vec3::new(16.0, 16.0, 18.0),
    Vec3::new(0.0, 0.0, 0.0),
    Vec3::new(32.0, 32.0, 32.0),
];

/// What a physent's `model` is: the original keeps a `model_t` pointer, which is null, a brush
/// model (it has hulls) or something else (a studio model or sprite: its hulls are empty, so
/// nothing ever hits it and it holds no point).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysModel {
    None,
    Brush(usize),
    Other,
}

/// One entry of `physents` or `moveents` (`physent_t`) with what the trace code needs besides
/// the fields the game sees. Who owns it (a server edict, a client entity) is the caller's
/// business: the trace code never looks at it.
#[derive(Clone, Debug)]
pub struct PhysEnt {
    pub view: PhysEntView,
    pub model: PhysModel,
}

/// `SV_AddLinksToPM` / `CL_AddLinksToPmove` snapshot for one player's command:
/// physents (world first), moveents (ladders), and visents (visible entities for `PM_TraceLine`).
#[derive(Clone, Debug, Default)]
pub struct PmPhysEnts {
    physents: Vec<PhysEnt>,
    moveents: Vec<PhysEnt>,
    visents: Vec<PhysEnt>,
}

impl PmPhysEnts {
    /// `numphysent`.
    pub fn num_physents(&self) -> usize {
        self.physents.len()
    }

    /// `nummoveent`.
    pub fn num_moveents(&self) -> usize {
        self.moveents.len()
    }

    /// `numvisent`.
    pub fn num_visents(&self) -> usize {
        self.visents.len()
    }

    /// Physent `index` (`physents[index]`), `None` when out of range.
    pub fn physent(&self, index: i32) -> Option<PhysEntView> {
        let index = usize::try_from(index).ok()?;
        self.physents.get(index).map(|p| p.view)
    }

    /// Moveent `index` (`moveents[index]`), `None` when out of range.
    pub fn moveent(&self, index: usize) -> Option<PhysEntView> {
        self.moveents.get(index).map(|p| p.view)
    }

    /// Visent `index` (`visents[index]`), `None` when out of range.
    pub fn visent(&self, index: i32) -> Option<PhysEntView> {
        let index = usize::try_from(index).ok()?;
        self.visents.get(index).map(|p| p.view)
    }

    /// Appends to `physents`.
    pub fn push_physent(&mut self, pe: PhysEnt) {
        self.physents.push(pe);
    }

    /// Appends to `moveents`.
    pub fn push_moveent(&mut self, pe: PhysEnt) {
        self.moveents.push(pe);
    }

    /// Appends to `visents`.
    pub fn push_visent(&mut self, pe: PhysEnt) {
        self.visents.push(pe);
    }

    /// The `physents` array.
    pub fn physents_slice(&self) -> &[PhysEnt] {
        &self.physents
    }

    /// The `visents` array.
    pub fn visents_slice(&self) -> &[PhysEnt] {
        &self.visents
    }

    /// `moveents[index]`, `None` when out of range.
    pub fn moveent_at(&self, index: usize) -> Option<&PhysEnt> {
        self.moveents.get(index)
    }
}

/// The fields of a `physent_t` the movement code reads.
#[derive(Clone, Copy, Debug)]
pub struct PhysEntView {
    /// Edict number (`info`).
    pub info: i32,
    /// Edict number for a client's edict, else 0 (`player`).
    pub player: i32,
    pub origin: Vec3,
    pub angles: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub solid: i32,
    pub skin: i32,
    pub rendermode: i32,
    pub movetype: i32,
    /// `PM_GetModelType(model) == mod_brush`.
    pub is_brush_model: bool,
    /// `PM_GetModelBounds`: the model's bounds, zero for a physent without a brush model.
    pub model_mins: Vec3,
    pub model_maxs: Vec3,
}

/// `pmtrace_t` (pmtrace.h) without `deltavelocity` (pm fills it).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PmTraceResult {
    pub allsolid: bool,
    pub startsolid: bool,
    pub inopen: bool,
    pub inwater: bool,
    pub fraction: f32,
    pub endpos: Vec3,
    pub plane_normal: Vec3,
    pub plane_dist: f32,
    pub ent: i32,
    pub hitgroup: i32,
}

impl PmTraceResult {
    /// The trace that hits nothing (`_PM_PlayerTrace`'s initial value): fraction 1, `ent` -1,
    /// `endpos` the end, everything else zero.
    pub fn miss(end: Vec3) -> Self {
        Self {
            fraction: 1.0,
            endpos: end,
            ent: -1,
            ..Self::default()
        }
    }
}

/// `hull_t` index of the BSP hull the physent trace uses for `usehull`
/// (`_PM_PlayerTrace`: 1 -> hulls[3], 2 -> hulls[0], 3 -> hulls[2], else hulls[1]).
fn bsp_hull_for_usehull(usehull: usize) -> Hull {
    match usehull {
        1 => Hull::Crouched,
        2 => Hull::Point,
        3 => Hull::Large,
        _ => Hull::Standing,
    }
}

/// `usehull` as an index of the hull tables, `None` outside `0..=3`.
fn valid_hull(usehull: i32) -> Option<usize> {
    usize::try_from(usehull)
        .ok()
        .filter(|&h| h < PM_PLAYER_MINS.len())
}

/// `_DotProduct` (float, in the source's order of operations).
fn dot(a: Vec3, b: Vec3) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

/// Deliberately a second copy of `collision.rs::plane_distance`: this one keeps the original's
/// float order of operations (`_DotProduct`, `plane->type >= 3u`) for the prediction-exact walk
/// of `PM_RecursiveHullCheck`, and must not be merged with the engine's version.
/// The signed distance of `p` from `plane` as `PM_RecursiveHullCheck` and `PM_HullPointContents`
/// compute it: a dot product for planes of type 3 and up (`plane->type >= 3u`), else one axis.
fn plane_dist(plane: &Plane, p: Vec3) -> f32 {
    if (plane.plane_type as u32) >= 3 {
        dot(p, plane.normal) - plane.dist
    } else {
        p[plane.plane_type as usize] - plane.dist
    }
}

/// Deepest recursion of the hull walk; a malformed map with a cyclic hull ends there.
const MAX_WALK_DEPTH: u32 = 4096;

/// Nodes one hull walk may visit: twice the node count of the hull plus slack. A valid tree is
/// walked in at most one visit per node and leaf; a cyclic hull would otherwise cost `2^depth`
/// visits (both sides of a plane are walked when the segment crosses it). Like the `budget` of
/// `texture.rs`; an exhausted walk stops and reports nothing hit further.
fn walk_budget(last: i32) -> usize {
    (usize::try_from(last).unwrap_or(0) + 1) * 2 + 16
}

/// `PM_HullPointContents(hull, hull->firstclipnode, p)` for `geo` (`CONTENTS_EMPTY` for a hull
/// of a model out of range; the original stops the server on a bad node number). A hull with
/// no clipnode (`firstclipnode >= lastclipnode`) is -1, as in the source.
fn hull_contents(coll: &WorldCollision, geo: Geometry<'_>, p: Vec3) -> i32 {
    let Some((first, last)) = coll.hull_range(geo) else {
        return CONTENTS_EMPTY;
    };
    if first >= last {
        return -1;
    }
    coll.geo_point_contents(geo, geo.root(coll), p)
}

/// The state of one `PM_RecursiveHullCheck` walk.
struct Walk<'a> {
    coll: &'a WorldCollision,
    geo: Geometry<'a>,
    first: i32,
    last: i32,
    /// Visits left, see [`walk_budget`].
    budget: std::cell::Cell<usize>,
}

impl Walk<'_> {
    /// `PM_RecursiveHullCheck` (`rehlds/engine/pmovetst.cpp`, read 2026-11-22), the
    /// non-`REHLDS_OPT_PEDANTIC` form. Returns the source's `qboolean`.
    #[allow(clippy::too_many_arguments)]
    fn check(
        &self,
        num: i32,
        p1f: f32,
        p2f: f32,
        p1: Vec3,
        p2: Vec3,
        trace: &mut PmTraceResult,
        depth: u32,
    ) -> bool {
        if num < 0 {
            let contents = self.coll.geo_leaf_contents(self.geo, num);
            if contents == CONTENTS_SOLID {
                trace.startsolid = true;
            } else {
                trace.allsolid = false;
                if contents == CONTENTS_EMPTY {
                    trace.inopen = true;
                } else {
                    // Quirk: every other contents (sky and currents too) counts as water.
                    trace.inwater = true;
                }
            }
            return true;
        }

        if self.first >= self.last {
            trace.allsolid = false;
            trace.inopen = true;
            return true;
        }
        if depth > MAX_WALK_DEPTH {
            return true;
        }
        let Some(left) = self.budget.get().checked_sub(1) else {
            return true;
        };
        self.budget.set(left);

        let Some((plane, children)) = self.coll.geo_node(self.geo, num) else {
            return true;
        };
        let t1 = plane_dist(&plane, p1);
        let t2 = plane_dist(&plane, p2);

        if t1 >= 0.0 && t2 >= 0.0 {
            return self.check(children[0], p1f, p2f, p1, p2, trace, depth + 1);
        }

        let mut midf;
        if t1 >= 0.0 {
            midf = t1 - DIST_EPSILON;
        } else {
            if t2 < 0.0 {
                return self.check(children[1], p1f, p2f, p1, p2, trace, depth + 1);
            }
            midf = t1 + DIST_EPSILON;
        }
        midf /= t1 - t2;
        // `if (midf >= 0) { if (midf > 1) midf = 1 } else midf = 0` (a NaN becomes 0).
        if midf >= 0.0 {
            if midf > 1.0 {
                midf = 1.0;
            }
        } else {
            midf = 0.0;
        }

        let pdif = p2f - p1f;
        let mut frac = pdif * midf + p1f;
        let mut mid = Vec3::new(
            (p2.x - p1.x) * midf + p1.x,
            (p2.y - p1.y) * midf + p1.y,
            (p2.z - p1.z) * midf + p1.z,
        );

        let side = usize::from(t1 < 0.0);

        if !self.check(children[side], p1f, frac, p1, mid, trace, depth + 1) {
            return false;
        }

        if self.contents_from(children[side ^ 1], mid) != CONTENTS_SOLID {
            return self.check(children[side ^ 1], frac, p2f, mid, p2, trace, depth + 1);
        }

        if trace.allsolid {
            return false;
        }

        if side == 1 {
            trace.plane_normal = Vec3::ZERO - plane.normal;
            trace.plane_dist = -plane.dist;
        } else {
            trace.plane_normal = plane.normal;
            trace.plane_dist = plane.dist;
        }

        if self.contents_from(self.first, mid) != CONTENTS_SOLID {
            trace.fraction = frac;
            trace.endpos = mid;
            return false;
        }

        loop {
            // `midf = (float)(midf - 0.05)`: the subtraction is in double.
            midf = (f64::from(midf) - 0.05) as f32;
            if midf < 0.0 {
                break;
            }
            frac = pdif * midf + p1f;
            mid = Vec3::new(
                (p2.x - p1.x) * midf + p1.x,
                (p2.y - p1.y) * midf + p1.y,
                (p2.z - p1.z) * midf + p1.z,
            );
            if self.contents_from(self.first, mid) != CONTENTS_SOLID {
                trace.fraction = frac;
                trace.endpos = mid;
                return false;
            }
        }

        // Quirk: "Trace backed up past 0.0" (a `Con_DPrintf` in the source); the last point
        // tried is taken as the impact.
        trace.fraction = frac;
        trace.endpos = mid;
        false
    }

    /// `PM_HullPointContents(hull, num, p)` of this hull.
    fn contents_from(&self, num: i32, p: Vec3) -> i32 {
        if self.first >= self.last {
            return -1;
        }
        self.coll.geo_point_contents(self.geo, num, p)
    }
}

/// Runs `PM_RecursiveHullCheck` over `geo` and returns the `total` trace of `_PM_PlayerTrace`
/// before its post-processing: fraction 1, `endpos` the original `end`, `allsolid` set.
fn hull_check(
    coll: &WorldCollision,
    geo: Geometry<'_>,
    start_l: Vec3,
    end_l: Vec3,
    end: Vec3,
) -> PmTraceResult {
    let mut total = PmTraceResult {
        allsolid: true,
        fraction: 1.0,
        endpos: end,
        ..PmTraceResult::default()
    };
    if let Some((first, last)) = coll.hull_range(geo) {
        let walk = Walk {
            coll,
            geo,
            first,
            last,
            budget: std::cell::Cell::new(walk_budget(last)),
        };
        walk.check(geo.root(coll), 0.0, 1.0, start_l, end_l, &mut total, 0);
    }
    total
}

/// The hull of `pe` for `usehull` and the offset that puts the player's origin into the hull's
/// frame: `PM_HullForBsp` for a brush model (`clip_mins - player_mins + origin`) and
/// `PM_HullForBox` for anything else (the entity's box grown by the player's, offset by its
/// origin). `None` for a physent that can neither hit nor hold a point: a model that is not a
/// brush model, a brush model out of range, a non-finite origin, box or angles.
fn with_hull<R>(
    coll: &WorldCollision,
    pe: &PhysEnt,
    usehull: usize,
    f: impl FnOnce(Geometry<'_>, Vec3) -> R,
) -> Option<R> {
    let v = &pe.view;
    match pe.model {
        PhysModel::Brush(model) => {
            if model >= coll.model_count() || !in_world(v.origin) || !v.angles.is_finite() {
                return None;
            }
            let hull = bsp_hull_for_usehull(usehull);
            // Two float steps, as the source does them.
            let offset = hull.clip_mins() - PM_PLAYER_MINS[usehull] + v.origin;
            Some(f(Geometry::Bsp { model, hull }, offset))
        }
        PhysModel::Other => None,
        PhysModel::None => {
            if !in_world(v.origin) || !in_world(v.mins) || !in_world(v.maxs) {
                return None;
            }
            let mins = v.mins - PM_PLAYER_MAXS[usehull];
            let maxs = v.maxs - PM_PLAYER_MINS[usehull];
            let hull = BoxHull::new(mins, maxs);
            Some(f(Geometry::Box(&hull), v.origin))
        }
    }
}

/// Whether the physent's BSP hull is rotated by its angles (`SOLID_BSP` with any angle).
fn is_rotated(v: &PhysEntView) -> bool {
    v.solid == SOLID_BSP && v.angles != Vec3::ZERO
}

/// `_PM_PlayerTrace` (`rehlds/engine/pmovetst.cpp`, read 2026-11-22) over `ents`.
///
/// `usehull` outside `0..=3` hits nothing.
pub fn player_trace(
    coll: &WorldCollision,
    ents: &[PhysEnt],
    start: Vec3,
    end: Vec3,
    usehull: i32,
    trace_flags: i32,
    ignore_pe: i32,
) -> PmTraceResult {
    let mut trace = PmTraceResult::miss(end);
    let Some(uh) = valid_hull(usehull) else {
        return trace;
    };

    for (i, pe) in ents.iter().enumerate() {
        if i > 0 && trace_flags & PM_WORLD_ONLY != 0 {
            break;
        }
        if ignore_pe != -1 && i as i64 == i64::from(ignore_pe) {
            continue;
        }
        let v = &pe.view;
        // `pe->model && !pe->solid && pe->skin` skips water and ladders; glass is skipped on
        // request. (The `studiomodel` branch is unreachable, see the module doc.)
        if (pe.model != PhysModel::None && v.solid == 0 && v.skin != 0)
            || (trace_flags & PM_GLASS_IGNORE != 0 && v.rendermode != 0)
        {
            continue;
        }

        let Some(total) = with_hull(coll, pe, uh, |geo, offset| {
            trace_one(coll, v, geo, offset, start, end)
        }) else {
            continue;
        };

        // Quirk: an equal fraction does not replace the earlier physent.
        if total.fraction < trace.fraction {
            trace = total;
            trace.ent = i as i32;
        }
    }
    trace
}

/// `PM_TraceLine` (Xash3D FWGS `engine/client/dll_int/cl_pmove.c` / ReHLDS `pmovetst.cpp`,
/// read 2026-11-25): traces against `visents` when `flags & PM_TRACELINE_ANYVISIBLE != 0`,
/// otherwise against `physents`, using `usehull` (`0..=3`, typically `2` for point traces).
#[allow(clippy::too_many_arguments)]
pub fn trace_line(
    coll: &WorldCollision,
    physents: &[PhysEnt],
    visents: &[PhysEnt],
    start: Vec3,
    end: Vec3,
    flags: i32,
    usehull: i32,
    ignore_pe: i32,
) -> PmTraceResult {
    let target = if (flags & PM_TRACELINE_ANYVISIBLE) != 0 && !visents.is_empty() {
        visents
    } else {
        physents
    };
    player_trace(coll, target, start, end, usehull, PM_NORMAL, ignore_pe)
}

/// The body of `_PM_PlayerTrace`'s loop after the hull is chosen: one physent's `total`.
fn trace_one(
    coll: &WorldCollision,
    v: &PhysEntView,
    geo: Geometry<'_>,
    offset: Vec3,
    start: Vec3,
    end: Vec3,
) -> PmTraceResult {
    let mut start_l = start - offset;
    let mut end_l = end - offset;

    let rotated = is_rotated(v);
    if rotated {
        let (forward, right, up) = angle_vectors(v.angles);
        let to_local = |p: Vec3| Vec3::new(dot(forward, p), -dot(right, p), dot(up, p));
        start_l = to_local(start_l);
        end_l = to_local(end_l);
    }

    let mut total = hull_check(coll, geo, start_l, end_l, end);

    if total.allsolid {
        total.startsolid = true;
    }
    // Quirk: a trace that starts in solid reports fraction 0.
    if total.startsolid {
        total.fraction = 0.0;
    }

    if total.fraction != 1.0 {
        if rotated {
            let (forward, right, up) = angle_vectors_transpose(v.angles);
            let n = total.plane_normal;
            total.plane_normal = Vec3::new(dot(forward, n), dot(right, n), dot(up, n));
        }
        // Recomputed from the segment, not taken from the hull's `endpos`.
        total.endpos = Vec3::new(
            (end.x - start.x) * total.fraction + start.x,
            (end.y - start.y) * total.fraction + start.y,
            (end.z - start.z) * total.fraction + start.z,
        );
    }
    total
}

/// `_PM_TestPlayerPosition` (`rehlds/engine/pmovetst.cpp`, read 2026-11-22): the index of the
/// first physent whose hull holds `pos` in solid, else -1; with the trace the original fills.
///
/// Quirk: that trace is `PM_PlayerTrace(pmove->origin, pmove->origin)`, taken at
/// `trace_origin`, not at `pos`.
pub fn test_player_position(
    coll: &WorldCollision,
    ents: &[PhysEnt],
    trace_origin: Vec3,
    pos: Vec3,
    usehull: i32,
) -> (i32, PmTraceResult) {
    let tr = player_trace(
        coll,
        ents,
        trace_origin,
        trace_origin,
        usehull,
        PM_NORMAL,
        -1,
    );
    let Some(uh) = valid_hull(usehull) else {
        return (-1, tr);
    };
    for (i, pe) in ents.iter().enumerate() {
        let v = &pe.view;
        if pe.model != PhysModel::None && v.solid == 0 && v.skin != 0 {
            continue;
        }
        let inside = with_hull(coll, pe, uh, |geo, offset| {
            let mut test = pos - offset;
            if is_rotated(v) {
                let (forward, right, up) = angle_vectors(v.angles);
                test = Vec3::new(dot(forward, test), -dot(right, test), dot(up, test));
            }
            hull_contents(coll, geo, test) == CONTENTS_SOLID
        });
        if inside == Some(true) {
            return (i as i32, tr);
        }
    }
    (-1, tr)
}

/// `PM_LinkContents` (`rehlds/engine/pmovetst.cpp`, read 2026-11-22): the `skin` of the first
/// non-solid physent with a model whose point hull holds `p`, -1 when there is none.
fn link_contents(coll: &WorldCollision, ents: &[PhysEnt], p: Vec3) -> i32 {
    for pe in ents.iter().skip(1) {
        let PhysModel::Brush(model) = pe.model else {
            continue;
        };
        if pe.view.solid != 0 || model >= coll.model_count() || !in_world(pe.view.origin) {
            continue;
        }
        let test = p - pe.view.origin;
        let geo = Geometry::Bsp {
            model,
            hull: Hull::Point,
        };
        if hull_contents(coll, geo, test) != -1 {
            return pe.view.skin;
        }
    }
    -1
}

/// `PM_PointContents` (`rehlds/engine/pmovetst.cpp`, read 2026-11-22): `(contents,
/// truecontents)`. The true contents are the world's alone; the contents are the world's with
/// the currents folded into water, overridden by a non-solid brush physent that holds `p`
/// (its `skin`), and `SOLID` wins over everything.
pub fn point_contents(coll: &WorldCollision, ents: &[PhysEnt], p: Vec3) -> (i32, i32) {
    let world = match ents.first().map(|pe| pe.model) {
        Some(PhysModel::Brush(model)) if model < coll.model_count() => model,
        _ => return (CONTENTS_EMPTY, CONTENTS_EMPTY),
    };
    let geo = Geometry::Bsp {
        model: world,
        hull: Hull::Point,
    };
    let true_contents = hull_contents(coll, geo, p);
    let mut entity_contents = true_contents;
    if !(CONTENTS_CURRENT_DOWN..=CONTENTS_CURRENT_0).contains(&entity_contents) {
        if entity_contents == CONTENTS_SOLID {
            return (entity_contents, true_contents);
        }
    } else {
        entity_contents = CONTENTS_WATER;
    }
    let linked = link_contents(coll, ents, p);
    if linked != -1 {
        return (linked, true_contents);
    }
    (entity_contents, true_contents)
}

/// The test of `PM_Ladder` (`rg_pm_shared.cpp`, read 2026-11-22) for one moveent: a brush model
/// whose `skin` is `CONTENTS_LADDER`, and `PM_HullPointContents(PM_HullForBsp(pe, test),
/// firstclipnode, origin - test)` is not `CONTENTS_EMPTY`.
pub fn ladder_contains(
    coll: &WorldCollision,
    moveent: &PhysEnt,
    origin: Vec3,
    usehull: i32,
) -> bool {
    let Some(uh) = valid_hull(usehull) else {
        return false;
    };
    if !matches!(moveent.model, PhysModel::Brush(_)) || moveent.view.skin != CONTENTS_LADDER {
        return false;
    }
    with_hull(coll, moveent, uh, |geo, offset| {
        hull_contents(coll, geo, origin - offset) != CONTENTS_EMPTY
    })
    .unwrap_or(false)
}

/// `PM_TraceModel` (`rehlds/engine/pmovetst.cpp`, read 2026-11-22): the point hull of the
/// moveent's brush model traced from `start` to `end`; `(fraction, plane normal)`. A moveent
/// that is not a brush model gives `(1, 0)`. See the module doc for the trace's initial state.
pub fn trace_model(
    coll: &WorldCollision,
    pe: &PhysEnt,
    start: Vec3,
    end: Vec3,
) -> (f32, Vec3) {
    let miss = (1.0, Vec3::ZERO);
    // `pmove->usehull = 2` for the `PM_HullForBsp` call: the point hull.
    let Some(tr) = with_hull(coll, pe, 2, |geo, offset| match geo {
        Geometry::Bsp { .. } => Some(coll.trace_local(geo, start - offset, end - offset)),
        Geometry::Box(_) => None,
    })
    .flatten() else {
        return miss;
    };
    if tr.all_solid || tr.fraction >= 1.0 {
        // Raw `SV_RecursiveHullCheck` keeps fraction 1 for a trace that never leaves solid.
        return miss;
    }
    (tr.fraction, tr.plane_normal)
}

/// `PM_TraceTexture` (ReHLDS `rehlds/engine/pr_cmds.cpp` `TraceTexture` and Xash3D FWGS
/// `engine/client/cl_pmove.c` `PM_CL_TraceTexture`, read 2026-11-25): returns the raw BSP
/// texture name of the surface hit by `start..end` on physent `ground`.
///
/// For `ground == 0` (world), traces world model `0` directly. For a brush submodel
/// (`PhysModel::Brush(model)`), transforms `start` and `end` into the entity's local frame by
/// subtracting `pe.view.origin` and rotating by inverse `pe.view.angles` when non-zero.
/// Returns `None` if `ground` is out of range, not a brush model, or coordinates are invalid.
pub fn trace_texture(
    world: &WorldCollision,
    physents: &[PhysEnt],
    ground: i32,
    start: Vec3,
    end: Vec3,
) -> Option<String> {
    if !start.is_finite() || !end.is_finite() {
        return None;
    }
    let idx = usize::try_from(ground).ok()?;
    let pe = physents.get(idx)?;
    let PhysModel::Brush(model) = pe.model else {
        return None;
    };
    if !pe.view.origin.is_finite() || !pe.view.angles.is_finite() {
        return None;
    }
    let mut start_l = start - pe.view.origin;
    let mut end_l = end - pe.view.origin;
    if pe.view.angles != Vec3::ZERO {
        let (forward, right, up) = angle_vectors(pe.view.angles);
        let turn = |p: Vec3| Vec3::new(dot(forward, p), -dot(right, p), dot(up, p));
        start_l = turn(start_l);
        end_l = turn(end_l);
    }
    world.trace_texture(model, start_l, end_l).map(str::to_owned)
}

/// The world's entry of `physents` (`SV_AddLinksToPM`): the world model, `SOLID_BSP`,
/// `MOVETYPE_NONE`, entity 0.
pub fn world_physent(coll: Option<&WorldCollision>) -> PhysEnt {
    let (model_mins, model_maxs) = coll
        .and_then(|c| c.model_bounds(0))
        .unwrap_or((Vec3::ZERO, Vec3::ZERO));
    PhysEnt {
        view: PhysEntView {
            info: 0,
            player: 0,
            origin: Vec3::ZERO,
            angles: Vec3::ZERO,
            mins: Vec3::ZERO,
            maxs: Vec3::ZERO,
            solid: SOLID_BSP,
            skin: 0,
            rendermode: 0,
            movetype: 0,
            is_brush_model: true,
            model_mins,
            model_maxs,
        },
        model: PhysModel::Brush(0),
    }
}

/// `AngleVectors`.
pub fn angle_vectors(angles: Vec3) -> (Vec3, Vec3, Vec3) {
    let (sy, cy, sp, cp, sr, cr) = sines(angles);
    let forward = Vec3::new(cp * cy, cp * sy, -sp);
    let right = Vec3::new(-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp);
    let up = Vec3::new(cr * sp * cy + -sr * -sy, cr * sp * sy + -sr * cy, cr * cp);
    (forward, right, up)
}

/// `AngleVectorsTranspose`.
pub fn angle_vectors_transpose(angles: Vec3) -> (Vec3, Vec3, Vec3) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use cs_assets::bsp::{BspMap, BspModel, Face, Leaf, LightmapAtlas, Node, TexInfo};
    use cs_assets::wad::DecodedTexture;

    fn test_bsp_world_and_submodel() -> BspMap {
        // Model 0 (world): plane z = 0, face [0..64, 0..64] textured "-0metal_floor".
        // Model 1 (submodel): plane x = 16, face y in [-32..32], z in [0..64] textured "wall_duct".
        BspMap {
            name: "test".into(),
            skyname: String::new(),
            entities: Vec::new(),
            spawns: Vec::new(),
            hostage_spawns: Vec::new(),
            brush_entities: Vec::new(),
            planes: vec![
                Plane {
                    normal: Vec3::Z,
                    dist: 0.0,
                    plane_type: 2,
                },
                Plane {
                    normal: Vec3::X,
                    dist: 16.0,
                    plane_type: 0,
                },
            ],
            vertices_raw: vec![
                // Face 0 (floor at z = 0)
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(64.0, 64.0, 0.0),
                Vec3::new(0.0, 64.0, 0.0),
                // Face 1 (wall at x = 16)
                Vec3::new(16.0, -32.0, 0.0),
                Vec3::new(16.0, 32.0, 0.0),
                Vec3::new(16.0, 32.0, 64.0),
                Vec3::new(16.0, -32.0, 64.0),
            ],
            visibility: Vec::new(),
            nodes: vec![
                Node {
                    plane: 0,
                    children: [-1, -1],
                    mins: [0; 3],
                    maxs: [0; 3],
                    first_face: 0,
                    num_faces: 1,
                },
                Node {
                    plane: 1,
                    children: [-1, -1],
                    mins: [0; 3],
                    maxs: [0; 3],
                    first_face: 1,
                    num_faces: 1,
                },
            ],
            texinfos: vec![
                TexInfo {
                    s_vec: Vec3::X,
                    s_shift: 0.0,
                    t_vec: Vec3::Y,
                    t_shift: 0.0,
                    miptex: 0,
                    flags: 0,
                },
                TexInfo {
                    s_vec: Vec3::Y,
                    s_shift: 0.0,
                    t_vec: Vec3::Z,
                    t_shift: 0.0,
                    miptex: 1,
                    flags: 0,
                },
            ],
            faces: vec![
                Face {
                    plane: 0,
                    plane_side: 0,
                    first_edge: 0,
                    num_edges: 4,
                    texinfo: 0,
                    styles: [0; 4],
                    lightmap_offset: -1,
                },
                Face {
                    plane: 1,
                    plane_side: 0,
                    first_edge: 4,
                    num_edges: 4,
                    texinfo: 1,
                    styles: [0; 4],
                    lightmap_offset: -1,
                },
            ],
            lighting: Vec::new(),
            clipnodes: Vec::new(),
            leafs: vec![Leaf {
                contents: CONTENTS_EMPTY,
                vis_offset: -1,
                mins: [0; 3],
                maxs: [0; 3],
                first_mark_surface: 0,
                num_mark_surfaces: 0,
                ambient_levels: [0; 4],
            }],
            mark_surfaces: Vec::new(),
            edges: vec![
                [0, 1],
                [1, 2],
                [2, 3],
                [3, 0],
                [4, 5],
                [5, 6],
                [6, 7],
                [7, 4],
            ],
            surf_edges: vec![0, 1, 2, 3, 4, 5, 6, 7],
            models: vec![
                BspModel {
                    mins: Vec3::splat(-64.0),
                    maxs: Vec3::splat(64.0),
                    origin: Vec3::ZERO,
                    head_nodes: [0, -1, -1, -1],
                    vis_leafs: 0,
                    first_face: 0,
                    num_faces: 1,
                },
                BspModel {
                    mins: Vec3::new(-16.0, -32.0, 0.0),
                    maxs: Vec3::new(16.0, 32.0, 64.0),
                    origin: Vec3::ZERO,
                    head_nodes: [1, -1, -1, -1],
                    vis_leafs: 0,
                    first_face: 1,
                    num_faces: 1,
                },
            ],
            textures: vec![
                DecodedTexture {
                    name: "-0metal_floor".to_string(),
                    width: 16,
                    height: 16,
                    rgba: Vec::new(),
                    alpha_test: false,
                },
                DecodedTexture {
                    name: "wall_duct".to_string(),
                    width: 16,
                    height: 16,
                    rgba: Vec::new(),
                    alpha_test: false,
                },
            ],
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

    fn submodel_physent(origin: Vec3, angles: Vec3, model: PhysModel) -> PhysEnt {
        PhysEnt {
            view: PhysEntView {
                info: 1,
                player: 0,
                origin,
                angles,
                mins: Vec3::ZERO,
                maxs: Vec3::ZERO,
                solid: SOLID_BSP,
                skin: 0,
                rendermode: 0,
                movetype: 7,
                is_brush_model: matches!(model, PhysModel::Brush(_)),
                model_mins: Vec3::ZERO,
                model_maxs: Vec3::ZERO,
            },
            model,
        }
    }

    #[test]
    fn trace_texture_handles_world_rotated_submodel_and_invalid_inputs() {
        let world = WorldCollision::from_bsp(&test_bsp_world_and_submodel());
        let ents = vec![
            world_physent(Some(&world)),
            submodel_physent(
                Vec3::new(500.0, 0.0, 0.0),
                Vec3::new(0.0, 90.0, 0.0),
                PhysModel::Brush(1),
            ),
            submodel_physent(Vec3::ZERO, Vec3::ZERO, PhysModel::None),
            submodel_physent(Vec3::ZERO, Vec3::ZERO, PhysModel::Other),
        ];

        // World hit preserves animation prefix `-0`.
        assert_eq!(
            trace_texture(
                &world,
                &ents,
                0,
                Vec3::new(32.0, 32.0, 10.0),
                Vec3::new(32.0, 32.0, -10.0)
            )
            .as_deref(),
            Some("-0metal_floor")
        );
        assert_eq!(
            trace_texture(
                &world,
                &ents,
                0,
                Vec3::new(100.0, 32.0, 10.0),
                Vec3::new(100.0, 32.0, -10.0)
            ),
            None
        );

        // Submodel at (500, 0, 0) rotated 90 deg yaw: local +x (16) is world (500, 16, z).
        assert_eq!(
            trace_texture(
                &world,
                &ents,
                1,
                Vec3::new(500.0, 32.0, 32.0),
                Vec3::new(500.0, 0.0, 32.0)
            )
            .as_deref(),
            Some("wall_duct")
        );
        // Unrotated world-X trace at origin misses the rotated face.
        assert_eq!(
            trace_texture(
                &world,
                &ents,
                1,
                Vec3::new(532.0, 0.0, 32.0),
                Vec3::new(500.0, 0.0, 32.0)
            ),
            None
        );

        // Non-brush, out-of-range, and non-finite inputs return None.
        assert_eq!(
            trace_texture(
                &world,
                &ents,
                2,
                Vec3::new(32.0, 32.0, 10.0),
                Vec3::new(32.0, 32.0, -10.0)
            ),
            None
        );
        assert_eq!(
            trace_texture(
                &world,
                &ents,
                3,
                Vec3::new(32.0, 32.0, 10.0),
                Vec3::new(32.0, 32.0, -10.0)
            ),
            None
        );
        assert_eq!(
            trace_texture(
                &world,
                &ents,
                -1,
                Vec3::new(32.0, 32.0, 10.0),
                Vec3::new(32.0, 32.0, -10.0)
            ),
            None
        );
        assert_eq!(
            trace_texture(&world, &ents, 0, Vec3::NAN, Vec3::ZERO),
            None
        );
    }
}
