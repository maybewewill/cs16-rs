//! BSP hull collision: traces of the four GoldSrc hulls (`hull 0/1/2/3`) through the
//! world model and brush sub-models, and point contents. Pure geometry, no game data.

use cs_assets::bsp::{BspMap, BspModel, ClipNode, Face, Node, Plane, TexInfo};
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

/// Whether every coordinate of `v` is finite and within [`MAX_WORLD_COORD`].
pub fn in_world(v: Vec3) -> bool {
    v.is_finite() && v.abs().max_element() <= MAX_WORLD_COORD
}

/// Classic Quake / GoldSrc epsilon preventing surface sinking (`1 / 32`).
pub const DIST_EPSILON: f32 = 0.03125;

/// `TEX_SPECIAL` (`texinfo` flag bit 0: no lightmap, the surface is tiled or turbulent).
const TEX_SPECIAL: i32 = 1;
/// Deepest node chain `SurfaceAtPoint` follows. A real tree is far shallower; the cap keeps the
/// recursion's stack use small for a hostile map (the `budget` bounds the work).
const MAX_SURFACE_WALK_DEPTH: u32 = 256;

/// A face's `texturemins` and `extents` (`msurface_t`), as `CalcSurfaceExtents` makes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceExtents {
    pub texturemins: [i32; 2],
    pub extents: [i32; 2],
}

/// Compact face record for `SurfaceAtPoint` texture lookup.
#[derive(Debug, Clone, Copy)]
struct SurfaceFace {
    texinfo: u16,
    extents: Option<SurfaceExtents>,
}

/// Compact texinfo record for `SurfaceAtPoint` texture lookup.
#[derive(Debug, Clone, Copy)]
struct SurfaceTexInfo {
    s_vec: Vec3,
    s_shift: f32,
    t_vec: Vec3,
    t_shift: f32,
    miptex: i32,
}

/// `CalcSurfaceExtents` and then the overrides of `Mod_LoadFaces` (`rehlds/engine/model.cpp`,
/// read 2026-11-22): the texture-space box of the face, in whole 16-unit lumps. The dot
/// products are taken in double and stored as `float`, as the source has it (the `FIXED: loss
/// of floating point` there). `None` for a face whose edges or vertices are outside the map's
/// arrays.
///
/// The overrides, in the source's order by the name of the face's texture: `sky*` keeps the
/// computed box; `scroll*` (case-sensitive) takes the texture's width and height as `extents`
/// (`texturemins` stay); `!*`, `laser*` and `water*` (the last two case-insensitive) take
/// `texturemins` -8192 and `extents` 16384 on both axes, so every point of the plane is in the
/// face; any other face whose texinfo has `TEX_SPECIAL` takes the texture's width and height.
pub fn calc_surface_extents(bsp: &BspMap, face: &Face, tex: &TexInfo) -> Option<SurfaceExtents> {
    let mut mins = [999_999.0f32; 2];
    let mut maxs = [-99_999.0f32; 2];
    let first = usize::try_from(face.first_edge).ok()?;
    for i in 0..usize::from(face.num_edges) {
        let e = *bsp.surf_edges.get(first.checked_add(i)?)?;
        let vertex = if e >= 0 {
            bsp.edges.get(e as usize)?[0]
        } else {
            bsp.edges.get(e.unsigned_abs() as usize)?[1]
        };
        let p = *bsp.vertices_raw.get(usize::from(vertex))?;
        for (j, (vec, shift)) in [(tex.s_vec, tex.s_shift), (tex.t_vec, tex.t_shift)]
            .into_iter()
            .enumerate()
        {
            let val = (f64::from(p.x) * f64::from(vec.x)
                + f64::from(p.y) * f64::from(vec.y)
                + f64::from(p.z) * f64::from(vec.z)
                + f64::from(shift)) as f32;
            if val < mins[j] {
                mins[j] = val;
            }
            if val > maxs[j] {
                maxs[j] = val;
            }
        }
    }
    let mut out = SurfaceExtents {
        texturemins: [0; 2],
        extents: [0; 2],
    };
    for i in 0..2 {
        let bmins = (mins[i] / 16.0).floor() as i32;
        let bmaxs = (maxs[i] / 16.0).ceil() as i32;
        out.texturemins[i] = bmins.wrapping_mul(16);
        out.extents[i] = bmaxs.wrapping_sub(bmins).wrapping_mul(16);
    }
    let texture = usize::try_from(tex.miptex)
        .ok()
        .and_then(|m| bsp.textures.get(m));
    if let Some(texture) = texture {
        let name = texture.name.as_bytes();
        let starts = |prefix: &[u8]| name.starts_with(prefix);
        let starts_nocase = |prefix: &[u8]| {
            name.len() >= prefix.len() && name[..prefix.len()].eq_ignore_ascii_case(prefix)
        };
        let size = [texture.width as i32, texture.height as i32];
        if starts(b"sky") {
            // Keeps the computed box.
        } else if starts(b"scroll") {
            out.extents = size;
        } else if starts(b"!") || starts_nocase(b"laser") || starts_nocase(b"water") {
            out.extents = [16384; 2];
            out.texturemins = [-8192; 2];
        } else if tex.flags & TEX_SPECIAL != 0 {
            out.extents = size;
        }
    }
    Some(out)
}

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
    pub fn clip_mins(self) -> Vec3 {
        -self.half_extents()
    }
}

/// `CONTENTS_CURRENT_0` and `CONTENTS_CURRENT_DOWN` (`rehlds/common/const.h`): the water
/// currents, which `SV_PointContents` reports as water.
pub const CONTENTS_CURRENT_0: i32 = -9;
pub const CONTENTS_CURRENT_DOWN: i32 = -14;

/// What a trace walks: a hull of a BSP model, or a box turned into a six-plane hull.
#[derive(Clone, Copy)]
pub enum Geometry<'a> {
    Bsp { model: usize, hull: Hull },
    Box(&'a BoxHull),
}

impl Geometry<'_> {
    /// The node the walk starts at (`hull->firstclipnode`).
    pub fn root(self, world: &WorldCollision) -> i32 {
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
pub struct BoxHull {
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
    pub fn from_planes(planes: [Plane; 6]) -> Self {
        Self { planes }
    }

    /// `SV_HullForBox`: the hull of the box `mins..maxs`.
    pub fn new(mins: Vec3, maxs: Vec3) -> Self {
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

/// A map's collision geometry: planes, nodes, clipnodes, leaf contents, models, and compact
/// surface-texture tables (model 0 is the world, the rest are brush sub-models).
#[derive(Debug, Clone)]
pub struct WorldCollision {
    planes: Vec<Plane>,
    nodes: Vec<Node>,
    clipnodes: Vec<ClipNode>,
    leaf_contents: Vec<i32>,
    models: Vec<BspModel>,
    faces: Vec<SurfaceFace>,
    texinfos: Vec<SurfaceTexInfo>,
    texture_names: Vec<String>,
}

/// A map without any model: the stand-in for geometry that does not need the map (box hulls).
pub static NO_COLLISION: WorldCollision = WorldCollision {
    planes: Vec::new(),
    nodes: Vec::new(),
    clipnodes: Vec::new(),
    leaf_contents: Vec::new(),
    models: Vec::new(),
    faces: Vec::new(),
    texinfos: Vec::new(),
    texture_names: Vec::new(),
};

// Checked in every build: the engine, and the map collision it holds, may run on its own thread.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<WorldCollision>();
};

impl WorldCollision {
    pub fn from_bsp(map: &BspMap) -> Self {
        let faces = map
            .faces
            .iter()
            .map(|face| {
                let extents = map
                    .texinfos
                    .get(usize::from(face.texinfo))
                    .and_then(|tex| calc_surface_extents(map, face, tex));
                SurfaceFace {
                    texinfo: face.texinfo,
                    extents,
                }
            })
            .collect();
        let texinfos = map
            .texinfos
            .iter()
            .map(|t| SurfaceTexInfo {
                s_vec: t.s_vec,
                s_shift: t.s_shift,
                t_vec: t.t_vec,
                t_shift: t.t_shift,
                miptex: t.miptex,
            })
            .collect();
        let texture_names = map.textures.iter().map(|t| t.name.clone()).collect();

        Self {
            planes: map.planes.clone(),
            nodes: map.nodes.clone(),
            clipnodes: map.clipnodes.clone(),
            leaf_contents: map.leafs.iter().map(|l| l.contents).collect(),
            // `Mod_LoadSubmodels` (ReHLDS `model.cpp`, read 2026-11-22): every model, the world
            // included, is stored one unit larger on each side than the BSP says.
            models: map
                .models
                .iter()
                .map(|m| BspModel {
                    mins: m.mins - Vec3::ONE,
                    maxs: m.maxs + Vec3::ONE,
                    ..*m
                })
                .collect(),
            faces,
            texinfos,
            texture_names,
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

    /// `SurfaceAtPoint` (`rehlds/engine/pr_cmds.cpp`, read 2026-11-22 / 2026-11-25): walks
    /// `models[model_index].head_nodes[0]` along the local-space line `start..end`, returning the
    /// raw BSP miptex name (up to 16 chars, preserving `{`, `!`, `-0` prefixes) of the first face
    /// whose texture-space box encloses the plane crossing point.
    pub fn trace_texture(&self, model_index: usize, start: Vec3, end: Vec3) -> Option<&str> {
        if !start.is_finite() || !end.is_finite() {
            return None;
        }
        let root = self.models.get(model_index)?.head_nodes[0];
        let mut budget = self.nodes.len();
        let face_idx = self.surface_at_point(root, start, end, &mut budget, 0)?;
        let face = self.faces.get(face_idx)?;
        let tex = self.texinfos.get(usize::from(face.texinfo))?;
        let name = self.texture_names.get(usize::try_from(tex.miptex).ok()?)?;
        Some(name.as_str())
    }

    /// `SurfaceAtPoint` (`rehlds/engine/pr_cmds.cpp`, read 2026-11-22): walks the node tree from
    /// `node` along the line `start`..`end`, nearest side first, and gives the first face whose
    /// texture-space box holds the point where the line crosses the node's plane (the source's
    /// order: the near child, then the node's own faces, then the far child). `None` in a leaf.
    fn surface_at_point(
        &self,
        node: i32,
        start: Vec3,
        end: Vec3,
        budget: &mut usize,
        depth: u32,
    ) -> Option<usize> {
        if node < 0 || depth > MAX_SURFACE_WALK_DEPTH {
            // `node->contents < 0`: a leaf.
            return None;
        }
        *budget = budget.checked_sub(1)?;
        let n = self.nodes.get(node as usize)?;
        let plane = self.planes.get(n.plane as usize)?;
        let front = start.dot(plane.normal) - plane.dist;
        let back = end.dot(plane.normal) - plane.dist;
        let s = usize::from(front < 0.0);
        let t = usize::from(back < 0.0);
        if t == s {
            return self.surface_at_point(i32::from(n.children[s]), start, end, budget, depth + 1);
        }
        let frac = front / (front - back);
        let mid = Vec3::new(
            (end.x - start.x) * frac + start.x,
            (end.y - start.y) * frac + start.y,
            (end.z - start.z) * frac + start.z,
        );
        if let Some(face) =
            self.surface_at_point(i32::from(n.children[s]), start, mid, budget, depth + 1)
        {
            return Some(face);
        }
        let first = usize::from(n.first_face);
        for i in 0..usize::from(n.num_faces) {
            let index = first + i;
            let Some(face) = self.faces.get(index) else {
                continue;
            };
            let Some(extents) = face.extents else {
                continue;
            };
            let Some(tex) = self.texinfos.get(usize::from(face.texinfo)) else {
                continue;
            };
            // `(int)` truncates toward zero.
            let ds = (mid.dot(tex.s_vec) + tex.s_shift) as i32;
            let dt = (mid.dot(tex.t_vec) + tex.t_shift) as i32;
            if ds >= extents.texturemins[0]
                && dt >= extents.texturemins[1]
                && ds - extents.texturemins[0] <= extents.extents[0]
                && dt - extents.texturemins[1] <= extents.extents[1]
            {
                return Some(index);
            }
        }
        self.surface_at_point(i32::from(n.children[s ^ 1]), mid, end, budget, depth + 1)
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
    pub fn trace_local(&self, geo: Geometry<'_>, start: Vec3, end: Vec3) -> TraceResult {
        let visits_left = match geo {
            Geometry::Bsp {
                hull: Hull::Point, ..
            } => (self.nodes.len() + 1) * 2 + 16,
            Geometry::Bsp { .. } => (self.clipnodes.len() + 1) * 2 + 16,
            Geometry::Box(_) => 64,
        };
        let mut ctx = TraceCtx {
            fraction: 1.0,
            end_pos: end,
            plane_normal: Vec3::Z,
            all_solid: true,
            start_solid: false,
            in_open: false,
            in_water: false,
            hit_sky: false,
            depth: 0,
            visits_left,
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
    pub fn geo_point_contents(&self, geo: Geometry<'_>, mut node_idx: i32, p: Vec3) -> i32 {
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
    pub fn hull_range(&self, geo: Geometry<'_>) -> Option<(i32, i32)> {
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
    pub fn geo_node(&self, geo: Geometry<'_>, idx: i32) -> Option<(Plane, [i32; 2])> {
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
    pub fn geo_leaf_contents(&self, geo: Geometry<'_>, idx: i32) -> i32 {
        match geo {
            Geometry::Bsp {
                hull: Hull::Point, ..
            } => {
                let leaf_idx = (-1i32).wrapping_sub(idx) as usize;
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

        if ctx.depth >= 4096 || ctx.visits_left == 0 {
            return true;
        }
        ctx.visits_left -= 1;

        let Some((plane, children)) = self.geo_node(geo, node_idx) else {
            return true;
        };

        let t1 = plane_distance(&plane, p1);
        let t2 = plane_distance(&plane, p2);

        if t1 >= 0.0 && t2 >= 0.0 {
            ctx.depth += 1;
            let res = self.recursive_hull_check(geo, children[0], p1f, p2f, p1, p2, ctx);
            ctx.depth -= 1;
            return res;
        }
        if t1 < 0.0 && t2 < 0.0 {
            ctx.depth += 1;
            let res = self.recursive_hull_check(geo, children[1], p1f, p2f, p1, p2, ctx);
            ctx.depth -= 1;
            return res;
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
        ctx.depth += 1;
        let near_ok = self.recursive_hull_check(geo, children[side], p1f, midf, p1, mid, ctx);
        ctx.depth -= 1;
        if !near_ok {
            return false;
        }

        // If far side is not solid at `mid`, recurse into it
        if self.geo_point_contents(geo, children[side ^ 1], mid) != CONTENTS_SOLID {
            ctx.depth += 1;
            let far_ok = self.recursive_hull_check(
                geo,
                children[side ^ 1],
                midf,
                p2f,
                mid,
                p2,
                ctx,
            );
            ctx.depth -= 1;
            return far_ok;
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
    depth: u32,
    visits_left: usize,
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
    use super::*;
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
            faces: Vec::new(),
            texinfos: Vec::new(),
            texture_names: Vec::new(),
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
        // The characterization samples the BSP's own bounds, not the enlarged loaded ones.
        let (mins, maxs) = (bsp.models[0].mins, bsp.models[0].maxs);

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

    #[test]
    fn leaf_contents_i32_min_and_cyclic_clipnodes_do_not_panic_or_hang() {
        let cyclic = WorldCollision {
            planes: vec![Plane {
                normal: Vec3::X,
                dist: 0.0,
                plane_type: 0,
            }],
            nodes: vec![Node {
                plane: 0,
                children: [0, 0],
                mins: [0; 3],
                maxs: [0; 3],
                first_face: 0,
                num_faces: 0,
            }],
            clipnodes: vec![ClipNode {
                plane: 0,
                children: [0, 0],
            }],
            leaf_contents: vec![CONTENTS_EMPTY],
            models: vec![BspModel {
                mins: Vec3::splat(-64.0),
                maxs: Vec3::splat(64.0),
                origin: Vec3::ZERO,
                head_nodes: [0, 0, 0, 0],
                vis_leafs: 0,
                first_face: 0,
                num_faces: 0,
            }],
            faces: Vec::new(),
            texinfos: Vec::new(),
            texture_names: Vec::new(),
        };
        assert_eq!(
            cyclic.geo_leaf_contents(
                Geometry::Bsp {
                    model: 0,
                    hull: Hull::Point
                },
                i32::MIN
            ),
            CONTENTS_EMPTY
        );
        let tr_pt = cyclic.trace_world(Vec3::new(-10.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Hull::Point);
        let tr_st = cyclic.trace_world(Vec3::new(-10.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0), Hull::Standing);
        assert_eq!(tr_pt.fraction, 0.0);
        assert_eq!(tr_st.fraction, 0.0);
        assert_eq!(
            cyclic.trace_texture(0, Vec3::new(-10.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0)),
            None
        );
    }

    #[test]
    fn no_collision_trace_texture_returns_none() {
        assert_eq!(
            NO_COLLISION.trace_texture(0, Vec3::ZERO, Vec3::new(0.0, 0.0, -64.0)),
            None
        );
    }
}
