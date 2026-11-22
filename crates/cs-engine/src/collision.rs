//! BSP hull collision: traces of the four GoldSrc hulls (`hull 0/1/2/3`) through the
//! world model and brush sub-models, and point contents. Pure geometry, no game data.

use cs_assets::bsp::{BspMap, BspModel, ClipNode, Node, Plane};
use glam::Vec3;

pub const CONTENTS_EMPTY: i32 = -1;
pub const CONTENTS_SOLID: i32 = -2;
pub const CONTENTS_WATER: i32 = -3;
pub const CONTENTS_SLIME: i32 = -4;
pub const CONTENTS_LAVA: i32 = -5;
pub const CONTENTS_SKY: i32 = -6;

/// Largest coordinate magnitude the engine's geometry calls accept; input beyond it is refused
/// like non-finite input, so traces never overflow to infinities. Assumption (from memory, not
/// checked against ReHLDS): GoldSrc maps stay well inside it (±4096 per axis by default).
pub const MAX_WORLD_COORD: f32 = 131_072.0;

/// Classic Quake / GoldSrc epsilon preventing surface sinking (`1 / 32`).
pub const DIST_EPSILON: f32 = 0.03125;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hull {
    /// Point / bullet ray.
    Point = 0,
    /// Standing player (`[-16,-16,-36]..[16,16,36]`).
    Standing = 1,
    /// Large hull (`[-32,-32,-32]..[32,32,32]`).
    Large = 2,
    /// Crouched player (`[-16,-16,-18]..[16,16,18]`).
    Crouched = 3,
}

impl Hull {
    pub fn half_extents(self) -> Vec3 {
        match self {
            Self::Point => Vec3::ZERO,
            Self::Standing => Vec3::new(16.0, 16.0, 36.0),
            Self::Large => Vec3::new(32.0, 32.0, 32.0),
            Self::Crouched => Vec3::new(16.0, 16.0, 18.0),
        }
    }

    /// `clip_mins` of the BSP hull: the corner of the box the hull was built for.
    pub(crate) fn clip_mins(self) -> Vec3 {
        -self.half_extents()
    }
}

/// `CONTENTS_CURRENT_0` and `CONTENTS_CURRENT_DOWN` (`rehlds/common/const.h`): the water
/// currents, which `SV_PointContents` reports as water.
pub const CONTENTS_CURRENT_0: i32 = -9;
pub const CONTENTS_CURRENT_DOWN: i32 = -14;

/// What a trace walks: a hull of a BSP model, or a box turned into a six-plane hull.
#[derive(Clone, Copy)]
pub(crate) enum Geometry<'a> {
    Bsp { model: usize, hull: Hull },
    Box(&'a BoxHull),
}

impl Geometry<'_> {
    /// The node the walk starts at (`hull->firstclipnode`).
    pub(crate) fn root(self, world: &WorldCollision) -> i32 {
        match self {
            Self::Bsp { model, hull } => world
                .models
                .get(model)
                .map_or(CONTENTS_EMPTY, |m| m.head_nodes[hull as usize]),
            Self::Box(_) => 0,
        }
    }

    /// The node the "back up out of solid" step tests: the root of the hull being traced
    /// (`SV_HullPointContents(hull, hull->firstclipnode, mid)` of `SV_RecursiveHullCheck`).
    fn backup_root(self, world: &WorldCollision) -> i32 {
        self.root(world)
    }
}


/// `box_hull` of `SV_InitBoxHull`: planes 0..6 are `+x max`, `+x min`, `+y max`, `+y min`,
/// `+z max`, `+z min`; clipnode `i` has the plane `i`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BoxHull {
    planes: [Plane; 6],
}

/// Children of the six clipnodes of the box hull: `children[i & 1]` is empty, the other one
/// the next clipnode (the last one: solid).
const BOX_CHILDREN: [[i32; 2]; 6] = {
    let mut out = [[0; 2]; 6];
    let mut i = 0;
    while i < 6 {
        let side = i & 1;
        out[i][side] = CONTENTS_EMPTY;
        out[i][side ^ 1] = if i != 5 {
            i as i32 + 1
        } else {
            CONTENTS_SOLID
        };
        i += 1;
    }
    out
};

impl BoxHull {
    /// A hull of six given planes, ordered as `SV_HullForBox`'s: the same clipnodes walk them
    /// (`studio_hull` of `SV_InitStudioHull` has the same children).
    pub(crate) fn from_planes(planes: [Plane; 6]) -> Self {
        Self { planes }
    }

    /// `SV_HullForBox`: the hull of the box `mins..maxs`.
    pub(crate) fn new(mins: Vec3, maxs: Vec3) -> Self {
        let plane = |i: usize, dist: f32| {
            let mut normal = Vec3::ZERO;
            normal[i >> 1] = 1.0;
            Plane {
                normal,
                dist,
                plane_type: (i >> 1) as i32,
            }
        };
        Self {
            planes: [
                plane(0, maxs.x),
                plane(1, mins.x),
                plane(2, maxs.y),
                plane(3, mins.y),
                plane(4, maxs.z),
                plane(5, mins.z),
            ],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TraceResult {
    pub fraction: f32,
    pub end_pos: Vec3,
    pub plane_normal: Vec3,
    pub all_solid: bool,
    pub start_solid: bool,
    pub hit_sky: bool,
    pub hit_solid: bool,
    /// `trace_t::hitgroup` (`tr.iHitgroup` of the game): the hitgroup of the studio hitbox a
    /// trace hit (`HITGROUP_HEAD` is 1 and so on), 0 (`HITGROUP_GENERIC`) for anything else,
    /// brush and box hits included.
    pub hitgroup: i32,
}

impl TraceResult {
    pub fn clear(end: Vec3) -> Self {
        Self {
            fraction: 1.0,
            end_pos: end,
            plane_normal: Vec3::Z,
            all_solid: false,
            start_solid: false,
            hit_sky: false,
            hit_solid: false,
            hitgroup: 0,
        }
    }
}

/// A map's collision geometry: planes, nodes, clipnodes, leaf contents and models
/// (model 0 is the world, the rest are brush sub-models).
#[derive(Debug, Clone)]
pub struct WorldCollision {
    planes: Vec<Plane>,
    nodes: Vec<Node>,
    clipnodes: Vec<ClipNode>,
    leaf_contents: Vec<i32>,
    models: Vec<BspModel>,
}

/// A map without any model: the stand-in for geometry that does not need the map (box hulls).
pub(crate) static NO_COLLISION: WorldCollision = WorldCollision {
    planes: Vec::new(),
    nodes: Vec::new(),
    clipnodes: Vec::new(),
    leaf_contents: Vec::new(),
    models: Vec::new(),
};

// Checked in every build: the engine, and the map collision it holds, may run on its own thread.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<WorldCollision>();
};

impl WorldCollision {
    pub fn from_bsp(map: &BspMap) -> Self {
        Self {
            planes: map.planes.clone(),
            nodes: map.nodes.clone(),
            clipnodes: map.clipnodes.clone(),
            leaf_contents: map.leafs.iter().map(|l| l.contents).collect(),
            models: map.models.clone(),
        }
    }

    /// Number of models, the world included.
    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    /// `(mins, maxs)` of `model` as stored in the BSP, or `None` out of range.
    pub fn model_bounds(&self, model: usize) -> Option<(Vec3, Vec3)> {
        self.models.get(model).map(|m| (m.mins, m.maxs))
    }

    /// Trace `hull` from `start` to `end` through the world model alone.
    pub fn trace_world(&self, start: Vec3, end: Vec3, hull: Hull) -> TraceResult {
        if self.models.is_empty() {
            return TraceResult::clear(end);
        }
        self.trace_bsp_model(0, Vec3::ZERO, start, end, hull)
    }

    /// Trace `hull` from `start` to `end` through brush model `model` placed at `origin`.
    /// Out of range: nothing is hit.
    pub fn trace_model(
        &self,
        model: usize,
        origin: Vec3,
        start: Vec3,
        end: Vec3,
        hull: Hull,
    ) -> TraceResult {
        if model >= self.models.len() {
            return TraceResult::clear(end);
        }
        self.trace_bsp_model(model, origin, start, end, hull)
    }

    /// Contents of the world model at `p` for `hull`.
    pub fn point_contents(&self, p: Vec3, hull: Hull) -> i32 {
        let Some(world) = self.models.first() else {
            return CONTENTS_EMPTY;
        };
        self.hull_point_contents(hull, world.head_nodes[hull as usize], p)
    }

    /// Contents of brush model `model` placed at `origin`, at `p` for `hull`.
    /// Out of range: `CONTENTS_EMPTY`.
    pub fn model_point_contents(&self, model: usize, origin: Vec3, p: Vec3, hull: Hull) -> i32 {
        let Some(m) = self.models.get(model) else {
            return CONTENTS_EMPTY;
        };
        self.hull_point_contents(hull, m.head_nodes[hull as usize], p - origin)
    }

    /// Whether point `p` is inside the solid of brush model `model` placed at `origin`.
    /// Out of range: `false`.
    pub fn point_in_model(&self, model: usize, origin: Vec3, p: Vec3) -> bool {
        self.model_point_contents(model, origin, p, Hull::Point) == CONTENTS_SOLID
    }

    fn trace_bsp_model(
        &self,
        model: usize,
        origin: Vec3,
        start: Vec3,
        end: Vec3,
        hull: Hull,
    ) -> TraceResult {
        let mut tr = self.trace_local(Geometry::Bsp { model, hull }, start - origin, end - origin);
        tr.end_pos += origin;
        tr
    }

    /// `SV_RecursiveHullCheck` over `geo` for the segment `start..end`, both in the
    /// geometry's own frame; the result is in that frame too. Model indices are in range.
    ///
    /// Deviations from the source, kept from phase 2A (its characterization hash depends on
    /// them): a trace that never leaves solid reports `all_solid`, fraction 0 and
    /// `end_pos = start` (the source keeps fraction 1 and `end`); the "back up out of solid"
    /// step moves 0.05 of the segment at a time, at most 8 times (the source: 0.1, no cap).
    pub(crate) fn trace_local(&self, geo: Geometry<'_>, start: Vec3, end: Vec3) -> TraceResult {
        let mut ctx = TraceCtx {
            fraction: 1.0,
            end_pos: end,
            plane_normal: Vec3::Z,
            all_solid: true,
            start_solid: false,
            in_open: false,
            in_water: false,
            hit_sky: false,
        };

        self.recursive_hull_check(geo, geo.root(self), 0.0, 1.0, start, end, &mut ctx);

        if ctx.all_solid {
            ctx.start_solid = true;
            ctx.fraction = 0.0;
            ctx.end_pos = start;
        } else if ctx.fraction < 1.0 {
            ctx.end_pos = start + (end - start) * ctx.fraction;
        }

        TraceResult {
            fraction: ctx.fraction,
            end_pos: ctx.end_pos,
            plane_normal: ctx.plane_normal,
            all_solid: ctx.all_solid,
            start_solid: ctx.start_solid,
            hit_sky: ctx.hit_sky,
            hit_solid: ctx.fraction < 1.0 || ctx.start_solid,
            hitgroup: 0,
        }
    }

    fn hull_point_contents(&self, hull: Hull, node_idx: i32, p: Vec3) -> i32 {
        self.geo_point_contents(Geometry::Bsp { model: 0, hull }, node_idx, p)
    }

    /// `SV_HullPointContents` from node `node_idx` of `geo`.
    pub(crate) fn geo_point_contents(&self, geo: Geometry<'_>, mut node_idx: i32, p: Vec3) -> i32 {
        let mut guard = 0usize;
        while node_idx >= 0 && guard < 4096 {
            guard += 1;
            let Some((plane, children)) = self.geo_node(geo, node_idx) else {
                return CONTENTS_EMPTY;
            };
            let d = plane_distance(&plane, p);
            node_idx = if d < 0.0 { children[1] } else { children[0] };
        }
        self.geo_leaf_contents(geo, node_idx)
    }

    /// `(firstclipnode, lastclipnode)` of the `hull_t` of `geo`: the first node of the hull and
    /// the last node of the array it indexes (the nodes for the point hull, the clipnodes for the
    /// others; a box hull is its six nodes). `None` for a model out of range.
    pub(crate) fn hull_range(&self, geo: Geometry<'_>) -> Option<(i32, i32)> {
        match geo {
            Geometry::Bsp { model, hull } => {
                let first = self.models.get(model)?.head_nodes[hull as usize];
                let len = if hull == Hull::Point {
                    self.nodes.len()
                } else {
                    self.clipnodes.len()
                };
                Some((first, len as i32 - 1))
            }
            Geometry::Box(_) => Some((0, 5)),
        }
    }

    /// Plane and children of node `idx` of `geo`; `None` out of range.
    pub(crate) fn geo_node(&self, geo: Geometry<'_>, idx: i32) -> Option<(Plane, [i32; 2])> {
        let idx = usize::try_from(idx).ok()?;
        match geo {
            Geometry::Bsp {
                hull: Hull::Point, ..
            } => {
                let node = self.nodes.get(idx)?;
                let plane = *self.planes.get(node.plane as usize)?;
                Some((plane, node.children.map(i32::from)))
            }
            Geometry::Bsp { .. } => {
                let cn = self.clipnodes.get(idx)?;
                let plane = *self.planes.get(usize::try_from(cn.plane).ok()?)?;
                Some((plane, cn.children.map(i32::from)))
            }
            Geometry::Box(b) => {
                let plane = *b.planes.get(idx)?;
                Some((plane, BOX_CHILDREN[idx]))
            }
        }
    }

    /// Contents of the leaf `idx` (negative) of `geo`.
    pub(crate) fn geo_leaf_contents(&self, geo: Geometry<'_>, idx: i32) -> i32 {
        match geo {
            Geometry::Bsp {
                hull: Hull::Point, ..
            } => {
                let leaf_idx = (-idx - 1) as usize;
                self.leaf_contents
                    .get(leaf_idx)
                    .copied()
                    .unwrap_or(CONTENTS_EMPTY)
            }
            _ => idx,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn recursive_hull_check(
        &self,
        geo: Geometry<'_>,
        node_idx: i32,
        p1f: f32,
        p2f: f32,
        p1: Vec3,
        p2: Vec3,
        ctx: &mut TraceCtx,
    ) -> bool {
        if node_idx < 0 {
            let contents = self.geo_leaf_contents(geo, node_idx);

            if contents != CONTENTS_SOLID {
                ctx.all_solid = false;
                if contents == CONTENTS_EMPTY {
                    ctx.in_open = true;
                } else if matches!(contents, CONTENTS_WATER | CONTENTS_SLIME | CONTENTS_LAVA) {
                    ctx.in_water = true;
                } else if contents == CONTENTS_SKY {
                    ctx.hit_sky = true;
                }
            } else {
                ctx.start_solid = true;
            }
            return true;
        }

        let Some((plane, children)) = self.geo_node(geo, node_idx) else {
            return true;
        };

        let t1 = plane_distance(&plane, p1);
        let t2 = plane_distance(&plane, p2);

        if t1 >= 0.0 && t2 >= 0.0 {
            return self.recursive_hull_check(geo, children[0], p1f, p2f, p1, p2, ctx);
        }
        if t1 < 0.0 && t2 < 0.0 {
            return self.recursive_hull_check(geo, children[1], p1f, p2f, p1, p2, ctx);
        }

        // Split the segment across the plane with classic GoldSrc DIST_EPSILON padding
        let frac = if t1 < 0.0 {
            ((t1 + DIST_EPSILON) / (t1 - t2)).clamp(0.0, 1.0)
        } else {
            ((t1 - DIST_EPSILON) / (t1 - t2)).clamp(0.0, 1.0)
        };

        let midf = p1f + (p2f - p1f) * frac;
        let mid = p1 + (p2 - p1) * frac;
        let side = usize::from(t1 < 0.0);

        // Near side first
        if !self.recursive_hull_check(geo, children[side], p1f, midf, p1, mid, ctx) {
            return false;
        }

        // If far side is not solid at `mid`, recurse into it
        if self.geo_point_contents(geo, children[side ^ 1], mid) != CONTENTS_SOLID {
            return self.recursive_hull_check(
                geo,
                children[side ^ 1],
                midf,
                p2f,
                mid,
                p2,
                ctx,
            );
        }

        // Never got out of solid
        if ctx.all_solid {
            return false;
        }

        // Found first impact plane!
        let impact_normal = if side == 0 {
            plane.normal
        } else {
            -plane.normal
        };

        let mut adj_frac = frac;
        let mut adj_midf = midf;
        let mut adj_mid = mid;

        // Back up slightly if `mid` still lands in solid due to numerical precision.
        // The hull tested is the one being traced, as in the source.
        let mut tries = 0;
        while self.geo_point_contents(geo, geo.backup_root(self), adj_mid) == CONTENTS_SOLID
            && tries < 8
        {
            adj_frac -= 0.05;
            if adj_frac < 0.0 {
                ctx.fraction = adj_midf;
                ctx.end_pos = adj_mid;
                ctx.plane_normal = impact_normal;
                return false;
            }
            adj_midf = p1f + (p2f - p1f) * adj_frac;
            adj_mid = p1 + (p2 - p1) * adj_frac;
            tries += 1;
        }

        if adj_midf < ctx.fraction {
            ctx.fraction = adj_midf;
            ctx.end_pos = adj_mid;
            ctx.plane_normal = impact_normal;
        }
        false
    }
}

struct TraceCtx {
    fraction: f32,
    end_pos: Vec3,
    plane_normal: Vec3,
    all_solid: bool,
    start_solid: bool,
    in_open: bool,
    in_water: bool,
    hit_sky: bool,
}

#[inline]
fn plane_distance(plane: &Plane, p: Vec3) -> f32 {
    match plane.plane_type {
        0 => p.x - plane.dist,
        1 => p.y - plane.dist,
        2 => p.z - plane.dist,
        _ => plane.normal.dot(p) - plane.dist,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::map::ServerMap;
    use cs_vfs::{Vfs, VfsConfig};

    /// `WORLD_ONLY_TRACE_HASH` of `cs-sim/tests/collision_characterization.rs`: the 2000
    /// traces of that test through `BspCollision` with every brush model disabled, recorded
    /// before the move.
    const WORLD_ONLY_TRACE_HASH: u64 = 0xc2187f037968731c;

    /// One world model whose every hull is a single empty leaf.
    fn empty_world() -> WorldCollision {
        WorldCollision {
            planes: Vec::new(),
            nodes: Vec::new(),
            clipnodes: Vec::new(),
            leaf_contents: vec![CONTENTS_EMPTY],
            models: vec![BspModel {
                mins: Vec3::splat(-64.0),
                maxs: Vec3::splat(64.0),
                origin: Vec3::ZERO,
                head_nodes: [-1, CONTENTS_EMPTY, CONTENTS_EMPTY, CONTENTS_EMPTY],
                vis_leafs: 0,
                first_face: 0,
                num_faces: 0,
            }],
        }
    }

    #[test]
    fn out_of_range_model_is_harmless() {
        let world = empty_world();
        assert_eq!(world.model_count(), 1);
        assert_eq!(
            world.model_bounds(0),
            Some((Vec3::splat(-64.0), Vec3::splat(64.0)))
        );
        assert_eq!(world.model_bounds(1), None);
        assert_eq!(world.model_bounds(usize::MAX), None);

        let start = Vec3::new(1.0, 2.0, 3.0);
        let end = Vec3::new(40.0, -5.0, 9.0);
        for model in [1, 7, usize::MAX] {
            for hull in [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched] {
                let tr = world.trace_model(model, Vec3::new(5.0, 5.0, 5.0), start, end, hull);
                assert_eq!(tr.fraction, 1.0);
                assert_eq!(tr.end_pos, end);
                assert!(!tr.all_solid && !tr.start_solid && !tr.hit_solid && !tr.hit_sky);
            }
            assert!(!world.point_in_model(model, Vec3::ZERO, start));
        }

        // The world model itself is open space.
        let tr = world.trace_world(start, end, Hull::Standing);
        assert_eq!(tr.fraction, 1.0);
        assert_eq!(tr.end_pos, end);
        assert_eq!(world.point_contents(start, Hull::Point), CONTENTS_EMPTY);
        assert!(!world.point_in_model(0, Vec3::ZERO, start));

        // A map without any model answers "nothing there".
        let none = WorldCollision {
            models: Vec::new(),
            ..empty_world()
        };
        assert_eq!(none.model_count(), 0);
        assert_eq!(none.trace_world(start, end, Hull::Point).fraction, 1.0);
        assert_eq!(none.point_contents(start, Hull::Large), CONTENTS_EMPTY);
    }

    #[test]
    fn stub_map_has_no_collision() {
        assert!(ServerMap::stub("de_test").collision().is_none());
    }

    #[test]
    fn real_map_has_collision() {
        let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
            return;
        };
        let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("de_dust2 reads");
        let bsp = Arc::new(BspMap::load(&vfs, "de_dust2").expect("de_dust2 parses"));
        let map = ServerMap::from_bsp("de_dust2", &raw, bsp.clone()).expect("valid BSP header");
        let world = map.collision().expect("a BSP map has collision");
        assert_eq!(world.model_count(), bsp.models.len());
        let n = world.model_count();
        assert!(world.model_bounds(n).is_none());
        let tr = world.trace_model(n, Vec3::ZERO, Vec3::ZERO, Vec3::X, Hull::Point);
        assert_eq!(tr.fraction, 1.0);
        assert!(!world.point_in_model(n, Vec3::ZERO, Vec3::ZERO));
    }

    /// `point_in_model` is true inside a real brush: among the first brush models of de_dust2,
    /// one has a point on an 8x8x8 grid inside its bounds whose model contents are solid.
    #[test]
    fn point_in_model_is_true_inside_a_real_brush() {
        let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
            return;
        };
        let bsp = BspMap::load(&vfs, "de_dust2").expect("de_dust2 parses");
        let world = WorldCollision::from_bsp(&bsp);
        const MODELS: usize = 8;
        const STEPS: usize = 8;
        let solid = (1..world.model_count().min(MODELS + 1)).find_map(|model| {
            let (mins, maxs) = world.model_bounds(model)?;
            let at = |i: usize| (i as f32 + 0.5) / STEPS as f32;
            (0..STEPS * STEPS * STEPS)
                .map(|i| {
                    let t = Vec3::new(at(i % STEPS), at(i / STEPS % STEPS), at(i / STEPS / STEPS));
                    mins + (maxs - mins) * t
                })
                .find(|&p| {
                    world.model_point_contents(model, Vec3::ZERO, p, Hull::Point) == CONTENTS_SOLID
                })
                .map(|p| (model, p))
        });
        let (model, p) = solid.expect("a solid point in one of the first brush models");
        assert!(
            world.point_in_model(model, Vec3::ZERO, p),
            "model {model} at {p}"
        );
    }

    /// The 2000 traces of `cs-sim/tests/collision_characterization.rs` (same LCG, seed, point
    /// and offset rules, hull order and hash) through `trace_world`.
    #[test]
    fn world_trace_matches_bsp_collision() {
        let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
            return;
        };
        let bsp = BspMap::load(&vfs, "de_dust2").expect("de_dust2 parses");
        let world = WorldCollision::from_bsp(&bsp);
        let (mins, maxs) = world.model_bounds(0).expect("world model");

        let mut state: u64 = 0xC516;
        let mut unit = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 40) as f32 / 16_777_216.0
        };
        let hulls = [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched];
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |bytes: &[u8]| {
            for b in bytes {
                hash ^= u64::from(*b);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        for i in 0..2000 {
            let x = mins.x + (maxs.x - mins.x) * unit();
            let y = mins.y + (maxs.y - mins.y) * unit();
            let z = mins.z + (maxs.z - mins.z) * unit();
            let start = Vec3::new(x, y, z);
            let dx = (unit() * 2.0 - 1.0) * 512.0;
            let dy = (unit() * 2.0 - 1.0) * 512.0;
            let dz = (unit() * 2.0 - 1.0) * 512.0;
            let end = start + Vec3::new(dx, dy, dz);
            let tr = world.trace_world(start, end, hulls[i % 4]);
            let v = [
                tr.fraction,
                tr.end_pos.x,
                tr.end_pos.y,
                tr.end_pos.z,
                tr.plane_normal.x,
                tr.plane_normal.y,
                tr.plane_normal.z,
            ];
            for f in v {
                feed(&f.to_bits().to_le_bytes());
            }
            let flags = u8::from(tr.all_solid)
                | u8::from(tr.start_solid) << 1
                | u8::from(tr.hit_sky) << 2
                | u8::from(tr.hit_solid) << 3;
            feed(&[flags]);
        }
        assert_eq!(hash, WORLD_ONLY_TRACE_HASH);
    }
}
