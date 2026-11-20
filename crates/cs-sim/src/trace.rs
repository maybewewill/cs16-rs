//! GoldSrc BSP v30 hull tracing (`hull 0/1/2/3`, delegated to `cs_engine::collision`), game zone
//! queries, and anatomical player hitbox raycasts.

use std::collections::HashSet;
use std::sync::Arc;

use cs_assets::bsp::{BrushEntity, BrushVolumeKind, BspMap, SpawnTeam};
use cs_assets::materials::{MaterialTable, SurfaceMaterial};
use cs_engine::collision::WorldCollision;
use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

pub use cs_engine::collision::{
    CONTENTS_EMPTY, CONTENTS_LAVA, CONTENTS_SKY, CONTENTS_SLIME, CONTENTS_SOLID, CONTENTS_WATER,
    DIST_EPSILON, Hull, TraceResult,
};

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
    world: Arc<WorldCollision>,
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
            world: Arc::new(WorldCollision::from_bsp(map)),
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
            let res = self
                .world
                .trace_model(b.model_index, b.origin, start, end, Hull::Point);
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
        let mut c = self.world.point_contents(point, hull);
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
            let bc = self
                .world
                .model_point_contents(brush.model_index, brush.origin, point, hull);
            if bc == CONTENTS_SOLID {
                c = CONTENTS_SOLID;
                break;
            }
        }
        c
    }

    /// Trace a hull from `start` to `end` against the world BSP and all solid brush entities.
    pub fn trace_hull(&self, start: Vec3, end: Vec3, hull: Hull) -> TraceResult {
        let mut best = self.world.trace_world(start, end, hull);
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
            // Out of range, `trace_model` hits nothing, which never replaces `best`.
            let res = self
                .world
                .trace_model(brush.model_index, brush.origin, start, end, hull);
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
