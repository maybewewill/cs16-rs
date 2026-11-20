//! GoldSrc BSP v30 hull tracing (`hull 0/1/2/3`), game zone queries, and anatomical player hitbox raycasts.

use std::collections::HashSet;

use cs_assets::bsp::{BspMap, BspModel, BrushEntity, BrushVolumeKind, ClipNode, Node, Plane, SpawnTeam};
use cs_assets::materials::{MaterialTable, SurfaceMaterial};
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

pub const CONTENTS_EMPTY: i32 = -1;
pub const CONTENTS_SOLID: i32 = -2;
pub const CONTENTS_WATER: i32 = -3;
pub const CONTENTS_SLIME: i32 = -4;
pub const CONTENTS_LAVA: i32 = -5;
pub const CONTENTS_SKY: i32 = -6;

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
        }
    }
}

/// GoldSrc hitgroup IDs matching StudioModel `mstudiobbox_t.group`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HitGroup {
    Generic = 0,
    Head = 1,
    Chest = 2,
    Stomach = 3,
    LeftArm = 4,
    RightArm = 5,
    LeftLeg = 6,
    RightLeg = 7,
}

impl HitGroup {
    /// Exact Counter-Strike 1.6 damage multiplier by hitgroup (ReGameDLL_CS).
    pub fn damage_multiplier(self) -> f32 {
        match self {
            Self::Head => 4.0,
            Self::Stomach => 1.25,
            Self::LeftLeg | Self::RightLeg => 0.75,
            Self::Generic | Self::Chest | Self::LeftArm | Self::RightArm => 1.0,
        }
    }

    pub fn is_armored(self, has_helmet: bool) -> bool {
        match self {
            Self::Head => has_helmet,
            Self::Chest | Self::Stomach | Self::LeftArm | Self::RightArm => true,
            Self::LeftLeg | Self::RightLeg | Self::Generic => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BspCollision {
    planes: Vec<Plane>,
    nodes: Vec<Node>,
    clipnodes: Vec<ClipNode>,
    leaf_contents: Vec<i32>,
    models: Vec<BspModel>,
    solid_brushes: Vec<BrushEntity>,
    ladders: Vec<BrushEntity>,
    buy_zones: Vec<BrushEntity>,
    bomb_targets: Vec<BrushEntity>,
    hostage_rescue_zones: Vec<BrushEntity>,
    breakables: Vec<BrushEntity>,
    doors: Vec<BrushEntity>,
    pub disabled_models: HashSet<usize>,
    materials: MaterialTable,
    // Keep minimal leaf/face texture info for surface material queries
    world_bounds: (Vec3, Vec3),
}

impl BspCollision {
    pub fn from_map(map: &BspMap, materials: MaterialTable) -> Self {
        let leaf_contents = map.leafs.iter().map(|l| l.contents).collect();
        let mut solid_brushes = Vec::new();
        let mut ladders = Vec::new();
        let mut buy_zones = Vec::new();
        let mut bomb_targets = Vec::new();
        let mut hostage_rescue_zones = Vec::new();
        let mut breakables = Vec::new();
        let mut doors = Vec::new();

        for b in &map.brush_entities {
            match b.kind {
                BrushVolumeKind::Solid => solid_brushes.push(b.clone()),
                BrushVolumeKind::Breakable { .. } => {
                    solid_brushes.push(b.clone());
                    breakables.push(b.clone());
                }
                BrushVolumeKind::Door { .. } => {
                    solid_brushes.push(b.clone());
                    doors.push(b.clone());
                }
                BrushVolumeKind::Ladder => ladders.push(b.clone()),
                BrushVolumeKind::BuyZone { .. } => buy_zones.push(b.clone()),
                BrushVolumeKind::BombTarget => bomb_targets.push(b.clone()),
                BrushVolumeKind::HostageRescue => hostage_rescue_zones.push(b.clone()),
            }
        }

        let world_bounds = map
            .models
            .first()
            .map(|m| (m.mins, m.maxs))
            .unwrap_or((Vec3::splat(-4096.0), Vec3::splat(4096.0)));

        Self {
            planes: map.planes.clone(),
            nodes: map.nodes.clone(),
            clipnodes: map.clipnodes.clone(),
            leaf_contents,
            models: map.models.clone(),
            solid_brushes,
            ladders,
            buy_zones,
            bomb_targets,
            hostage_rescue_zones,
            breakables,
            doors,
            disabled_models: HashSet::new(),
            materials,
            world_bounds,
        }
    }

    pub fn world_bounds(&self) -> (Vec3, Vec3) {
        self.world_bounds
    }

    pub fn bomb_target_centers(&self) -> Vec<Vec3> {
        self.bomb_targets
            .iter()
            .map(|b| (b.mins + b.maxs) * 0.5)
            .collect()
    }

    pub fn hostage_rescue_centers(&self) -> Vec<Vec3> {
        self.hostage_rescue_zones
            .iter()
            .map(|b| (b.mins + b.maxs) * 0.5)
            .collect()
    }

    pub fn is_in_hostage_rescue(&self, pos: Vec3) -> bool {
        self.hostage_rescue_zones
            .iter()
            .any(|z| aabb_contains(z.mins - Vec3::splat(24.0), z.maxs + Vec3::splat(24.0), pos))
    }

    pub fn breakables(&self) -> &[BrushEntity] {
        &self.breakables
    }

    pub fn doors(&self) -> &[BrushEntity] {
        &self.doors
    }

    /// Raycast strictly against intact `func_breakable` brushes, returning `(model_index, fraction, hit_pos, material)`.
    pub fn raycast_breakable(&self, start: Vec3, end: Vec3) -> Option<(usize, f32, Vec3, i32)> {
        let seg_min = start.min(end) - Vec3::splat(4.0);
        let seg_max = start.max(end) + Vec3::splat(4.0);
        let mut best: Option<(usize, f32, Vec3, i32)> = None;
        for b in &self.breakables {
            if self.disabled_models.contains(&b.model_index) {
                continue;
            }
            if !aabb_overlaps(seg_min, seg_max, b.mins, b.maxs) {
                continue;
            }
            let Some(m) = self.models.get(b.model_index) else {
                continue;
            };
            let res = self.trace_model(m, b.origin, start, end, Hull::Point);
            if res.fraction < 1.0 {
                let mat = match b.kind {
                    BrushVolumeKind::Breakable { material, .. } => material,
                    _ => 0,
                };
                if best.is_none_or(|(_, bf, _, _)| res.fraction < bf) {
                    best = Some((b.model_index, res.fraction, res.end_pos, mat));
                }
            }
        }
        best
    }

    pub fn has_buy_zones(&self) -> bool {
        !self.buy_zones.is_empty()
    }

    pub fn is_in_buy_zone(&self, pos: Vec3, team: SpawnTeam) -> bool {
        if self.buy_zones.is_empty() {
            return true; // Maps like aim_map without func_buyzone allow buying
        }
        self.buy_zones.iter().any(|z| {
            let team_ok = match z.kind {
                BrushVolumeKind::BuyZone { team: Some(t) } => t == team,
                BrushVolumeKind::BuyZone { team: None } => true,
                _ => false,
            };
            team_ok && aabb_contains(z.mins - Vec3::splat(16.0), z.maxs + Vec3::splat(16.0), pos)
        })
    }

    pub fn is_in_bomb_target(&self, pos: Vec3) -> bool {
        self.bomb_targets
            .iter()
            .any(|z| aabb_contains(z.mins - Vec3::splat(18.0), z.maxs + Vec3::splat(18.0), pos))
    }

    pub fn is_on_ladder(&self, pos: Vec3, hull: Hull) -> bool {
        let ext = hull.half_extents() + Vec3::new(3.0, 3.0, 0.0);
        let pmin = pos - ext;
        let pmax = pos + ext;
        self.ladders
            .iter()
            .any(|l| aabb_overlaps(pmin, pmax, l.mins, l.maxs))
    }

    pub fn material_for_texture(&self, tex_name: Option<&str>) -> SurfaceMaterial {
        tex_name
            .map(|n| self.materials.lookup(n))
            .unwrap_or(SurfaceMaterial::Concrete)
    }

    /// Query BSP contents at `point` for a given `hull`.
    pub fn point_contents(&self, point: Vec3, hull: Hull) -> i32 {
        let Some(world) = self.models.first() else {
            return CONTENTS_EMPTY;
        };
        let head = world.head_nodes[hull as usize];
        let mut c = self.hull_point_contents(hull, head, point);
        if c == CONTENTS_SOLID {
            return c;
        }
        for brush in &self.solid_brushes {
            if self.disabled_models.contains(&brush.model_index) {
                continue;
            }
            let pad = hull.half_extents() + Vec3::splat(2.0);
            if !aabb_contains(brush.mins - pad, brush.maxs + pad, point) {
                continue;
            }
            let Some(m) = self.models.get(brush.model_index) else {
                continue;
            };
            let bc = self.hull_point_contents(hull, m.head_nodes[hull as usize], point - brush.origin);
            if bc == CONTENTS_SOLID {
                c = CONTENTS_SOLID;
                break;
            }
        }
        c
    }

    /// Trace a hull from `start` to `end` against the world BSP and all solid brush entities.
    pub fn trace_hull(&self, start: Vec3, end: Vec3, hull: Hull) -> TraceResult {
        let Some(world) = self.models.first() else {
            return TraceResult::clear(end);
        };

        let mut best = self.trace_model(world, Vec3::ZERO, start, end, hull);
        if best.fraction <= 0.0 && best.all_solid {
            return best;
        }

        // Broadphase AABB of the swept trace
        let ext = hull.half_extents() + Vec3::splat(4.0);
        let seg_min = start.min(end) - ext;
        let seg_max = start.max(end) + ext;

        for brush in &self.solid_brushes {
            if self.disabled_models.contains(&brush.model_index) {
                continue;
            }
            if !aabb_overlaps(seg_min, seg_max, brush.mins, brush.maxs) {
                continue;
            }
            let Some(model) = self.models.get(brush.model_index) else {
                continue;
            };
            let res = self.trace_model(model, brush.origin, start, end, hull);
            if res.all_solid {
                return res;
            }
            if res.fraction < best.fraction || (res.start_solid && !best.start_solid) {
                best = res;
            }
        }

        best
    }

    /// Fast point-to-point line-of-sight check (returns `true` if unobstructed).
    pub fn has_line_of_sight(&self, a: Vec3, b: Vec3) -> bool {
        let tr = self.trace_hull(a, b, Hull::Point);
        !tr.start_solid && tr.fraction >= 0.999
    }

    fn trace_model(
        &self,
        model: &BspModel,
        origin: Vec3,
        start: Vec3,
        end: Vec3,
        hull: Hull,
    ) -> TraceResult {
        let local_start = start - origin;
        let local_end = end - origin;
        let head = model.head_nodes[hull as usize];

        let mut ctx = TraceCtx {
            fraction: 1.0,
            end_pos: local_end,
            plane_normal: Vec3::Z,
            all_solid: true,
            start_solid: false,
            in_open: false,
            in_water: false,
            hit_sky: false,
        };

        self.recursive_hull_check(hull, head, 0.0, 1.0, local_start, local_end, &mut ctx);

        if ctx.all_solid {
            ctx.start_solid = true;
            ctx.fraction = 0.0;
            ctx.end_pos = local_start;
        } else if ctx.fraction < 1.0 {
            ctx.end_pos = local_start + (local_end - local_start) * ctx.fraction;
        }

        TraceResult {
            fraction: ctx.fraction,
            end_pos: ctx.end_pos + origin,
            plane_normal: ctx.plane_normal,
            all_solid: ctx.all_solid,
            start_solid: ctx.start_solid,
            hit_sky: ctx.hit_sky,
            hit_solid: ctx.fraction < 1.0 || ctx.start_solid,
        }
    }

    fn hull_point_contents(&self, hull: Hull, mut node_idx: i32, p: Vec3) -> i32 {
        let mut guard = 0usize;
        while node_idx >= 0 && guard < 4096 {
            guard += 1;
            let (plane_idx, children) = if hull == Hull::Point {
                let Some(node) = self.nodes.get(node_idx as usize) else {
                    return CONTENTS_EMPTY;
                };
                (node.plane as usize, node.children)
            } else {
                let Some(cn) = self.clipnodes.get(node_idx as usize) else {
                    return CONTENTS_EMPTY;
                };
                (cn.plane as usize, cn.children)
            };

            let Some(plane) = self.planes.get(plane_idx) else {
                return CONTENTS_EMPTY;
            };
            let d = plane_distance(plane, p);
            node_idx = if d < 0.0 {
                children[1] as i32
            } else {
                children[0] as i32
            };
        }

        if hull == Hull::Point {
            let leaf_idx = (-node_idx - 1) as usize;
            self.leaf_contents
                .get(leaf_idx)
                .copied()
                .unwrap_or(CONTENTS_EMPTY)
        } else {
            node_idx
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn recursive_hull_check(
        &self,
        hull: Hull,
        node_idx: i32,
        p1f: f32,
        p2f: f32,
        p1: Vec3,
        p2: Vec3,
        ctx: &mut TraceCtx,
    ) -> bool {
        if node_idx < 0 {
            let contents = if hull == Hull::Point {
                let leaf_idx = (-node_idx - 1) as usize;
                self.leaf_contents
                    .get(leaf_idx)
                    .copied()
                    .unwrap_or(CONTENTS_EMPTY)
            } else {
                node_idx
            };

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

        let (plane_idx, children) = if hull == Hull::Point {
            let Some(node) = self.nodes.get(node_idx as usize) else {
                return true;
            };
            (node.plane as usize, node.children)
        } else {
            let Some(cn) = self.clipnodes.get(node_idx as usize) else {
                return true;
            };
            (cn.plane as usize, cn.children)
        };

        let Some(plane) = self.planes.get(plane_idx) else {
            return true;
        };

        let t1 = plane_distance(plane, p1);
        let t2 = plane_distance(plane, p2);

        if t1 >= 0.0 && t2 >= 0.0 {
            return self.recursive_hull_check(hull, children[0] as i32, p1f, p2f, p1, p2, ctx);
        }
        if t1 < 0.0 && t2 < 0.0 {
            return self.recursive_hull_check(hull, children[1] as i32, p1f, p2f, p1, p2, ctx);
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
        if !self.recursive_hull_check(hull, children[side] as i32, p1f, midf, p1, mid, ctx) {
            return false;
        }

        // If far side is not solid at `mid`, recurse into it
        if self.hull_point_contents(hull, children[side ^ 1] as i32, mid) != CONTENTS_SOLID {
            return self.recursive_hull_check(hull, children[side ^ 1] as i32, midf, p2f, mid, p2, ctx);
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

        // Back up slightly if `mid` still lands in solid due to numerical precision
        let mut tries = 0;
        while self.hull_point_contents(hull, self.models[0].head_nodes[hull as usize], adj_mid)
            == CONTENTS_SOLID
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

#[inline]
fn aabb_contains(mins: Vec3, maxs: Vec3, p: Vec3) -> bool {
    p.x >= mins.x
        && p.x <= maxs.x
        && p.y >= mins.y
        && p.y <= maxs.y
        && p.z >= mins.z
        && p.z <= maxs.z
}

#[inline]
fn aabb_overlaps(a_min: Vec3, a_max: Vec3, b_min: Vec3, b_max: Vec3) -> bool {
    a_min.x <= b_max.x
        && a_max.x >= b_min.x
        && a_min.y <= b_max.y
        && a_max.y >= b_min.y
        && a_min.z <= b_max.z
        && a_max.z >= b_min.z
}

/// Raycast against anatomical CS 1.6 player hitboxes (standing or crouched, rotated by `yaw_deg`).
/// Returns `Some((fraction_0_to_1, hit_pos, HitGroup))` for the closest hitbox struck.
pub fn raycast_player_hitboxes(
    ray_start: Vec3,
    ray_end: Vec3,
    player_origin: Vec3,
    player_yaw_deg: f32,
    crouched: bool,
) -> Option<(f32, Vec3, HitGroup)> {
    // Quick sphere broadphase (radius 48 units around player center)
    let center = player_origin;
    let dir = ray_end - ray_start;
    let len_sq = dir.length_squared();
    if len_sq < 1e-6 {
        return None;
    }
    let t_closest = ((center - ray_start).dot(dir) / len_sq).clamp(0.0, 1.0);
    let closest = ray_start + dir * t_closest;
    if closest.distance_squared(center) > 52.0 * 52.0 {
        return None;
    }

    // Transform ray into player-local yaw space
    let inv_rot = Quat::from_rotation_z(-player_yaw_deg.to_radians());
    let local_start = inv_rot * (ray_start - player_origin);
    let local_end = inv_rot * (ray_end - player_origin);
    let local_dir = local_end - local_start;

    // Anatomical boxes relative to GoldSrc hull center (`origin`):
    // Standing hull: z in [-36, +36], head at z in [+19, +34].
    // Crouched hull: z in [-18, +18], head at z in [+6, +18].
    let boxes: [(Vec3, Vec3, HitGroup); 6] = if crouched {
        [
            (Vec3::new(-5.5, -5.5, 5.5), Vec3::new(5.5, 5.5, 18.0), HitGroup::Head),
            (Vec3::new(-8.5, -10.5, -4.0), Vec3::new(8.5, 10.5, 5.5), HitGroup::Chest),
            (Vec3::new(-8.0, -9.0, -10.0), Vec3::new(8.0, 9.0, -4.0), HitGroup::Stomach),
            (Vec3::new(-5.5, 10.5, -5.0), Vec3::new(5.5, 15.5, 5.0), HitGroup::LeftArm),
            (Vec3::new(-5.5, -15.5, -5.0), Vec3::new(5.5, -10.5, 5.0), HitGroup::RightArm),
            (Vec3::new(-8.0, -9.5, -18.0), Vec3::new(8.0, 9.5, -10.0), HitGroup::LeftLeg),
        ]
    } else {
        [
            (Vec3::new(-5.5, -5.5, 19.0), Vec3::new(5.5, 5.5, 34.0), HitGroup::Head),
            (Vec3::new(-8.5, -10.5, 4.0), Vec3::new(8.5, 10.5, 19.0), HitGroup::Chest),
            (Vec3::new(-8.0, -9.5, -6.0), Vec3::new(8.0, 9.5, 4.0), HitGroup::Stomach),
            (Vec3::new(-5.0, 10.5, 2.0), Vec3::new(5.0, 15.5, 18.0), HitGroup::LeftArm),
            (Vec3::new(-5.0, -15.5, 2.0), Vec3::new(5.0, -10.5, 18.0), HitGroup::RightArm),
            (Vec3::new(-7.5, -9.5, -36.0), Vec3::new(7.5, 9.5, -6.0), HitGroup::LeftLeg),
        ]
    };

    let mut best: Option<(f32, HitGroup)> = None;
    for (bmin, bmax, group) in boxes {
        if let Some(t) = ray_aabb(local_start, local_dir, bmin, bmax)
            && best.is_none_or(|(bt, _)| t < bt)
        {
            best = Some((t, group));
        }
    }

    best.map(|(t, group)| (t, ray_start + dir * t, group))
}

fn ray_aabb(origin: Vec3, dir: Vec3, bmin: Vec3, bmax: Vec3) -> Option<f32> {
    let mut tmin = 0.0f32;
    let mut tmax = 1.0f32;

    for axis in 0..3 {
        let o = origin[axis];
        let d = dir[axis];
        let mn = bmin[axis];
        let mx = bmax[axis];
        if d.abs() < 1e-8 {
            if o < mn || o > mx {
                return None;
            }
        } else {
            let inv = 1.0 / d;
            let mut t1 = (mn - o) * inv;
            let mut t2 = (mx - o) * inv;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            tmin = tmin.max(t1);
            tmax = tmax.min(t2);
            if tmin > tmax {
                return None;
            }
        }
    }

    Some(tmin)
}
