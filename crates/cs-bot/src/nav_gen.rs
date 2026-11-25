// SPDX-License-Identifier: GPL-3.0-or-later
// Ported from ReGameDLL_CS `regamedll/game_shared/bot/nav_area.cpp` and
// `regamedll/dlls/bot/cs_bot_learn.cpp`.

//! Condition Zero-style automatic navigation mesh generator over a [`NavWorld`] collision
//! and entity-hint interface.
//!
//! Pipeline:
//! 1. 25-unit walkable flood-fill from spawn/objective/ladder seeds with crouch & jump-height tests.
//! 2. Greedy rectangular [`NavArea`] generation (up to 50x50 cells) + 4-way + jump-down linking.
//! 3. Coplanar rectangular merging + aspect-ratio squaring (`> 3.01`) + steep-slope jump tagging.
//! 4. Ladder extraction (`func_ladder`) and top/bottom area linking.
//! 5. Corner hiding spot cover classification (`IN_COVER` / `EXPOSED`), sniper LOS analysis
//!    (`GOOD_SNIPER_SPOT` / `IDEAL_SNIPER_SPOT`), and automatic place tagging.

use std::collections::{HashMap, HashSet, VecDeque};

use cs_assets::nav::{
    HidingSpot, HidingSpotFlags, NAV_VERSION, NavArea, NavAttribute, NavCorner, NavDir, NavFile,
    NavLadder, SpotEncounter, SpotOrder, UNDEFINED_PLACE,
};
use cs_pm::collision::{CONTENTS_SOLID, Hull, TraceResult};
use glam::Vec3;
use tracing::debug;

use crate::nav_mesh::NavMesh;

/// Grid step size in GoldSrc units (`GenerationStepSize = 25.0`).
pub const GENERATION_STEP_SIZE: f32 = 25.0;
/// Maximum step-up height without jumping (`StepHeight = 18.0`).
pub const STEP_HEIGHT: f32 = 18.0;
/// Maximum normal jump height (`JumpHeight = 41.8`).
pub const JUMP_HEIGHT: f32 = 41.8;
/// Maximum crouch-jump height (`JumpCrouchHeight = 58.0`).
pub const JUMP_CROUCH_HEIGHT: f32 = 58.0;
/// Minimum walkable floor normal Z (`MaxUnitZSlope = 0.7`, ~45 degrees).
pub const MAX_UNIT_Z_SLOPE: f32 = 0.7;
/// Maximum safe drop height (`DeathDrop = 200.0`).
pub const DEATH_DROP: f32 = 200.0;
/// Half player width (`HalfHumanWidth = 16.0`).
pub const HALF_HUMAN_WIDTH: f32 = 16.0;
/// Half standing player height (`HalfHumanHeight = 36.0`).
pub const HALF_HUMAN_HEIGHT: f32 = 36.0;
/// Standing player height (`HumanHeight = 72.0`).
pub const HUMAN_HEIGHT: f32 = 72.0;

/// Semantic map zone kind used for automatic seed placement and `.nav` place tagging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavZoneKind {
    CtSpawn,
    TSpawn,
    BombSite,
    Hostage,
    HostageRescue,
    VipRescue,
}

/// Axis-aligned bounds of a `func_ladder` volume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavLadderHint {
    pub mins: Vec3,
    pub maxs: Vec3,
}

/// Semantic objective/spawn volume hint for seeds and automatic place tagging.
#[derive(Debug, Clone, PartialEq)]
pub struct NavZoneHint {
    pub kind: NavZoneKind,
    pub origin: Vec3,
    pub mins: Option<Vec3>,
    pub maxs: Option<Vec3>,
    pub label: Option<String>,
}

/// Collision & map-entity abstraction used by the Condition Zero navmesh generator.
pub trait NavWorld {
    /// Sweep `hull` from `start` to `end` through world BSP + solid brushes.
    fn trace_hull(&self, start: Vec3, end: Vec3, hull: Hull) -> TraceResult;

    /// Point raycast (`Hull::Point`) from `start` to `end`.
    fn trace_line(&self, start: Vec3, end: Vec3) -> TraceResult {
        self.trace_hull(start, end, Hull::Point)
    }

    /// Query GoldSrc contents (`CONTENTS_EMPTY`, `CONTENTS_SOLID`, ...) at `pos`.
    fn point_contents(&self, pos: Vec3, hull: Hull) -> i32;

    /// Seed world positions for flood-fill (e.g. spawn points, bomb sites, hostages).
    fn seed_points(&self) -> Vec<Vec3>;

    /// Ladder brush volumes (`func_ladder`).
    fn ladders(&self) -> Vec<NavLadderHint> {
        Vec::new()
    }

    /// Spawn/bomb/hostage/VIP semantic zone hints for place tagging and seeding.
    fn zone_hints(&self) -> Vec<NavZoneHint> {
        Vec::new()
    }

    /// Returns `true` if a blocked `trace_line` segment crosses only a walkable door/breakable.
    fn is_walkable_obstacle(&self, _start: Vec3, _end: Vec3) -> bool {
        false
    }
}

/// Configuration for [`generate_nav_mesh`].
#[derive(Debug, Clone)]
pub struct NavGenConfig {
    pub step_size: f32,
    pub max_nodes: usize,
    pub max_merge_width: usize,
    pub max_merge_height: usize,
    pub compute_hiding_spots: bool,
    pub compute_sniper_spots: bool,
    pub compute_encounters: bool,
    pub tag_places: bool,
    pub bsp_size: u32,
}

impl Default for NavGenConfig {
    fn default() -> Self {
        Self {
            step_size: GENERATION_STEP_SIZE,
            max_nodes: 65_536,
            max_merge_width: 50,
            max_merge_height: 50,
            compute_hiding_spots: true,
            compute_sniper_spots: true,
            compute_encounters: true,
            tag_places: true,
            bsp_size: 0,
        }
    }
}

#[derive(Debug, Clone)]
struct NavNode {
    pos: Vec3,
    normal: Vec3,
    to: [Option<usize>; 4],
    attributes: NavAttribute,
    covered: bool,
    area_idx: Option<usize>,
}

#[derive(Debug, Clone)]
struct GenArea {
    id: u32,
    attributes: NavAttribute,
    nw_node: usize,
    ne_node: usize,
    se_node: usize,
    sw_node: usize,
    nw_corner: Vec3,
    se_corner: Vec3,
    ne_z: f32,
    sw_z: f32,
    connections: [Vec<usize>; 4],
    alive: bool,
}

impl GenArea {
    fn from_nodes(id: u32, nodes: &[NavNode], nw: usize, ne: usize, se: usize, sw: usize) -> Self {
        Self {
            id,
            attributes: nodes[nw].attributes,
            nw_node: nw,
            ne_node: ne,
            se_node: se,
            sw_node: sw,
            nw_corner: nodes[nw].pos,
            se_corner: nodes[se].pos,
            ne_z: nodes[ne].pos.z,
            sw_z: nodes[sw].pos.z,
            connections: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            alive: true,
        }
    }

    fn size_x(&self) -> f32 {
        self.se_corner.x - self.nw_corner.x
    }

    fn size_y(&self) -> f32 {
        self.se_corner.y - self.nw_corner.y
    }

    fn z_at(&self, x: f32, y: f32) -> f32 {
        let dx = self.size_x();
        let dy = self.size_y();
        if dx.abs() < f32::EPSILON || dy.abs() < f32::EPSILON {
            return self.ne_z;
        }
        let u = ((x - self.nw_corner.x) / dx).clamp(0.0, 1.0);
        let v = ((y - self.nw_corner.y) / dy).clamp(0.0, 1.0);
        let north_z = self.nw_corner.z + u * (self.ne_z - self.nw_corner.z);
        let south_z = self.sw_z + u * (self.se_corner.z - self.sw_z);
        north_z + v * (south_z - north_z)
    }

    fn surface_normal(&self) -> Vec3 {
        let u = Vec3::new(self.size_x(), 0.0, self.ne_z - self.nw_corner.z);
        let v = Vec3::new(0.0, self.size_y(), self.sw_z - self.nw_corner.z);
        u.cross(v).normalize_or_zero()
    }

    fn is_coplanar(&self, other: &Self) -> bool {
        let n1 = self.surface_normal();
        let n2 = other.surface_normal();
        n1.dot(n2) > 0.99
    }

    fn is_roughly_square(&self) -> bool {
        let sx = self.size_x();
        let sy = self.size_y();
        if sx <= 0.0 || sy <= 0.0 {
            return true;
        }
        let aspect = sx / sy;
        const MAX_ASPECT: f32 = 3.01;
        ((1.0 / MAX_ASPECT)..=MAX_ASPECT).contains(&aspect)
    }

    fn is_overlapping_xy(&self, p: Vec3) -> bool {
        p.x >= self.nw_corner.x
            && p.x <= self.se_corner.x
            && p.y >= self.nw_corner.y
            && p.y <= self.se_corner.y
    }
}

/// Generate a complete [`NavMesh`] from any [`NavWorld`].
pub fn generate_nav_mesh<W: NavWorld + ?Sized>(world: &W, config: &NavGenConfig) -> NavMesh {
    let step = config.step_size.max(5.0);
    let ladder_geoms = build_ladder_geometries(world, step);

    // 1. Walkable 25-unit flood-fill.
    let mut nodes = sample_walkable_nodes(world, &ladder_geoms, step, config.max_nodes);
    if nodes.is_empty() {
        return NavMesh::from_nav_file(NavFile {
            version: NAV_VERSION,
            bsp_size: config.bsp_size,
            ..NavFile::default()
        });
    }

    // 2. Greedy rectangular area generation.
    let mut areas = build_rectangular_areas(&mut nodes, config.max_merge_width, config.max_merge_height);

    // 3. Connect adjacent areas (4 cardinal directions + jump-down links).
    connect_generated_areas(world, &nodes, &mut areas, step);

    // 4. Merge coplanar rectangular areas with matching attributes.
    merge_generated_areas(&mut nodes, &mut areas);

    // 5. Split overly elongated areas (> 3.01 aspect ratio) and mark jump slopes.
    square_up_areas(&mut nodes, &mut areas, step);
    mark_jump_areas(&mut areas);

    // Compact surviving areas into NavFile structs with sequential 1-based IDs.
    let mut nav_areas = compact_areas(&areas);
    if nav_areas.is_empty() {
        return NavMesh::from_nav_file(NavFile {
            version: NAV_VERSION,
            bsp_size: config.bsp_size,
            ..NavFile::default()
        });
    }

    // 6. Build and link NavLadders to top/bottom NavAreas.
    let ladders = link_ladders(world, &ladder_geoms, &mut nav_areas, step);

    let mut file = NavFile {
        version: NAV_VERSION,
        bsp_size: config.bsp_size,
        places: Vec::new(),
        areas: nav_areas,
        ladders,
    };

    // 7. Automatic place tagging from spawn/bomb/hostage/VIP hints.
    if config.tag_places {
        tag_places_from_hints(world, &mut file);
    }

    // 8. LOS hiding / sniper / encounter analysis.
    if config.compute_hiding_spots {
        compute_hiding_spots(world, &mut file);
        if config.compute_sniper_spots {
            classify_sniper_spots(world, &mut file, step);
        }
        if config.compute_encounters {
            compute_spot_encounters(world, &mut file);
        }
    }

    debug!(
        nodes = nodes.len(),
        areas = file.areas.len(),
        ladders = file.ladders.len(),
        places = file.places.len(),
        "nav_gen completed"
    );

    NavMesh::from_nav_file(file)
}

#[derive(Debug, Clone)]
struct LadderGeom {
    top: Vec3,
    bottom: Vec3,
    width: f32,
    dir: NavDir,
}

fn snap_to_grid(v: f32, step: f32) -> f32 {
    (v / step).round() * step
}

fn snap_xy(p: Vec3, step: f32) -> Vec3 {
    Vec3::new(snap_to_grid(p.x, step), snap_to_grid(p.y, step), p.z)
}

fn get_ground_height<W: NavWorld + ?Sized>(world: &W, pos: Vec3) -> Option<(f32, Vec3)> {
    // Try starting 30 units above feet; if inside low ceiling, retry from 16 units above feet.
    for z_off in [30.0, 16.0] {
        let start = Vec3::new(pos.x, pos.y, pos.z + z_off);
        let end = Vec3::new(pos.x, pos.y, pos.z - DEATH_DROP - 32.0);
        let tr = world.trace_line(start, end);
        if !tr.start_solid && !tr.all_solid && tr.fraction < 1.0 && !tr.hit_sky {
            return Some((tr.end_pos.z, tr.plane_normal));
        }
    }
    None
}

fn build_ladder_geometries<W: NavWorld + ?Sized>(world: &W, step: f32) -> Vec<LadderGeom> {
    let mut out = Vec::new();
    for hint in world.ladders() {
        let center_x = (hint.mins.x + hint.maxs.x) * 0.5;
        let center_y = (hint.mins.y + hint.maxs.y) * 0.5;
        let mut top = Vec3::new(center_x, center_y, hint.maxs.z);
        let mut bottom = Vec3::new(center_x, center_y, hint.mins.z);

        let x_size = hint.maxs.x - hint.mins.x;
        let y_size = hint.maxs.y - hint.mins.y;
        let (dir, width) = if x_size > y_size {
            let from = bottom + Vec3::new(0.0, step, step);
            let to = top + Vec3::new(0.0, step, -step);
            let tr = world.trace_line(from, to);
            let dir = if tr.fraction < 1.0 || tr.start_solid {
                NavDir::North
            } else {
                NavDir::South
            };
            (dir, x_size)
        } else {
            let from = bottom + Vec3::new(step, 0.0, step);
            let to = top + Vec3::new(step, 0.0, -step);
            let tr = world.trace_line(from, to);
            let dir = if tr.fraction < 1.0 || tr.start_solid {
                NavDir::West
            } else {
                NavDir::East
            };
            (dir, y_size)
        };

        let mut along = top - bottom;
        let raw_len = along.length();
        if raw_len > 1.0 {
            along /= raw_len;
            let clearance = 32.0;
            let d2 = dir.to_vec2() * clearance;
            let offset = Vec3::new(d2.x, d2.y, 0.0);

            let mut t = 0.0;
            while t <= raw_len {
                let on = bottom + along * t;
                let tr = world.trace_line(on, on + offset);
                if tr.fraction >= 1.0 && !tr.start_solid {
                    bottom = on;
                    break;
                }
                t += 10.0;
            }
            let mut t = 0.0;
            while t <= raw_len {
                let on = top - along * t;
                let tr = world.trace_line(on, on + offset);
                if tr.fraction >= 1.0 && !tr.start_solid {
                    top = on;
                    break;
                }
                t += 10.0;
            }
        }

        out.push(LadderGeom {
            top,
            bottom,
            width,
            dir,
        });
    }
    out
}

fn find_node_at(
    nodes: &[NavNode],
    grid_buckets: &HashMap<(i32, i32), Vec<usize>>,
    pos: Vec3,
    step: f32,
) -> Option<usize> {
    let gx = (pos.x / step).round() as i32;
    let gy = (pos.y / step).round() as i32;
    let tol = 0.45 * step;
    let bucket = grid_buckets.get(&(gx, gy))?;
    for &idx in bucket {
        let p = nodes[idx].pos;
        if (p.x - pos.x).abs() < tol && (p.y - pos.y).abs() < tol && (p.z - pos.z).abs() < tol {
            return Some(idx);
        }
    }
    None
}

fn compute_node_attributes<W: NavWorld + ?Sized>(world: &W, feet: Vec3) -> Option<NavAttribute> {
    // Must at least fit crouched hull at `feet + (0,0,18.5)`
    let crouch_center = feet + Vec3::new(0.0, 0.0, 18.5);
    if world.point_contents(crouch_center, Hull::Crouched) == CONTENTS_SOLID {
        return None;
    }

    // 3x3 vertical ceiling probe matching `cs_bot_learn.cpp:118-140`
    let eps = 0.1;
    let mut crouch = false;
    for y in [-16.0f32, 0.0, 16.0] {
        for x in [-16.0f32, 0.0, 16.0] {
            let floor = feet + Vec3::new(x, y, 5.0);
            let ceiling = feet + Vec3::new(x, y, HUMAN_HEIGHT - eps);
            let tr = world.trace_line(floor, ceiling);
            if tr.fraction < 1.0 || tr.start_solid {
                crouch = true;
                break;
            }
        }
        if crouch {
            break;
        }
    }
    if crouch {
        Some(NavAttribute::CROUCH)
    } else {
        Some(NavAttribute::NONE)
    }
}

fn sample_walkable_nodes<W: NavWorld + ?Sized>(
    world: &W,
    ladders: &[LadderGeom],
    step: f32,
    max_nodes: usize,
) -> Vec<NavNode> {
    let mut nodes: Vec<NavNode> = Vec::new();
    let mut buckets: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    let mut queue: VecDeque<usize> = VecDeque::new();

    let insert_node = |nodes: &mut Vec<NavNode>,
                       buckets: &mut HashMap<(i32, i32), Vec<usize>>,
                       pos: Vec3,
                       normal: Vec3,
                       attrs: NavAttribute|
     -> usize {
        let idx = nodes.len();
        nodes.push(NavNode {
            pos,
            normal,
            to: [None; 4],
            attributes: attrs,
            covered: false,
            area_idx: None,
        });
        let gx = (pos.x / step).round() as i32;
        let gy = (pos.y / step).round() as i32;
        buckets.entry((gx, gy)).or_default().push(idx);
        idx
    };

    // Collect initial seeds from world + zone hints + ladder endpoints.
    let mut raw_seeds = world.seed_points();
    for hint in world.zone_hints() {
        raw_seeds.push(hint.origin);
    }
    for l in ladders {
        let out = l.dir.to_vec2() * HALF_HUMAN_WIDTH;
        let bottom_seed = l.bottom + Vec3::new(out.x, out.y, step);
        raw_seeds.push(bottom_seed);
        for d in NavDir::ALL {
            let dv = d.to_vec2() * step;
            raw_seeds.push(l.top + Vec3::new(out.x + dv.x, out.y + dv.y, step));
            raw_seeds.push(l.bottom + Vec3::new(out.x + dv.x, out.y + dv.y, step));
        }
    }

    for seed in raw_seeds {
        if nodes.len() >= max_nodes {
            break;
        }
        let snapped = snap_xy(seed, step);
        let Some((z, normal)) = get_ground_height(world, snapped) else {
            continue;
        };
        if normal.z < MAX_UNIT_Z_SLOPE {
            continue;
        }
        let feet = Vec3::new(snapped.x, snapped.y, z);
        let Some(attrs) = compute_node_attributes(world, feet) else {
            continue;
        };
        if find_node_at(&nodes, &buckets, feet, step).is_none() {
            let idx = insert_node(&mut nodes, &mut buckets, feet, normal, attrs);
            queue.push_back(idx);
        }
    }

    while let Some(curr_idx) = queue.pop_front() {
        let from = nodes[curr_idx].pos;
        for dir in NavDir::ALL {
            if nodes[curr_idx].to[dir.as_index()].is_some() {
                continue;
            }
            let dv = dir.to_vec2() * step;
            let candidate_xy = snap_xy(Vec3::new(from.x + dv.x, from.y + dv.y, from.z), step);
            let Some((to_z, to_normal)) = get_ground_height(world, candidate_xy) else {
                continue;
            };
            let to = Vec3::new(candidate_xy.x, candidate_xy.y, to_z);

            // Height delta bounds (cs_bot_learn.cpp:288)
            if to.z - from.z > JUMP_CROUCH_HEIGHT + 0.1 || from.z - to.z > DEATH_DROP {
                continue;
            }
            if to_normal.z < MAX_UNIT_Z_SLOPE {
                continue;
            }

            // Clearance trace at crouch-waist height (step_up safe)
            let lift = (to.z - from.z).max(0.0) + 18.5;
            let from_probe = from + Vec3::new(0.0, 0.0, lift.min(36.0));
            let to_probe = to + Vec3::new(0.0, 0.0, 18.5);
            let tr_line = world.trace_line(from_probe, to_probe);
            if (tr_line.fraction < 1.0 || tr_line.start_solid)
                && !world.is_walkable_obstacle(from_probe, to_probe)
            {
                continue;
            }

            // Slope check along the step
            let mut slope_ok = true;
            for frac in [0.25f32, 0.5, 0.75] {
                let mid = from.lerp(to, frac);
                let Some((_, mid_normal)) = get_ground_height(world, mid) else {
                    slope_ok = false;
                    break;
                };
                if mid_normal.z < MAX_UNIT_Z_SLOPE {
                    slope_ok = false;
                    break;
                }
            }
            if !slope_ok {
                continue;
            }

            let Some(mut attrs) = compute_node_attributes(world, to) else {
                continue;
            };
            if to.z - from.z > STEP_HEIGHT {
                attrs.insert(NavAttribute::JUMP);
            }

            let next_idx = if let Some(existing) = find_node_at(&nodes, &buckets, to, step) {
                existing
            } else {
                if nodes.len() >= max_nodes {
                    continue;
                }
                let idx = insert_node(&mut nodes, &mut buckets, to, to_normal, attrs);
                queue.push_back(idx);
                idx
            };

            if next_idx == curr_idx {
                continue;
            }
            nodes[curr_idx].to[dir.as_index()] = Some(next_idx);
            if (from.z - to.z).abs() < 10.0 && nodes[next_idx].to[dir.opposite().as_index()].is_none()
            {
                nodes[next_idx].to[dir.opposite().as_index()] = Some(curr_idx);
            }
        }
    }

    nodes
}

fn is_bi_linked(nodes: &[NavNode], idx: usize, dir: NavDir) -> bool {
    let Some(other) = nodes[idx].to[dir.as_index()] else {
        return false;
    };
    nodes[other].to[dir.opposite().as_index()] == Some(idx)
}

fn is_closed_cell(nodes: &[NavNode], idx: usize) -> bool {
    if !is_bi_linked(nodes, idx, NavDir::South) || !is_bi_linked(nodes, idx, NavDir::East) {
        return false;
    }
    let Some(east) = nodes[idx].to[NavDir::East.as_index()] else {
        return false;
    };
    let Some(south) = nodes[idx].to[NavDir::South.as_index()] else {
        return false;
    };
    if !is_bi_linked(nodes, east, NavDir::South) || !is_bi_linked(nodes, south, NavDir::East) {
        return false;
    }
    nodes[east].to[NavDir::South.as_index()] == nodes[south].to[NavDir::East.as_index()]
}

fn test_area(nodes: &[NavNode], nw_idx: usize, width: usize, height: usize) -> bool {
    let normal = nodes[nw_idx].normal;
    let d = -normal.dot(nodes[nw_idx].pos);
    let attrs = nodes[nw_idx].attributes;
    const OFF_PLANE_TOL: f32 = 5.0;

    let mut vert = nw_idx;
    for _y in 0..height {
        let mut horiz = vert;
        for _x in 0..width {
            if nodes[horiz].attributes != attrs || nodes[horiz].covered || !is_closed_cell(nodes, horiz)
            {
                return false;
            }
            let Some(next_e) = nodes[horiz].to[NavDir::East.as_index()] else {
                return false;
            };
            horiz = next_e;
            if width > 1 || height > 1 {
                let dist = (nodes[horiz].pos.dot(normal) + d).abs();
                if dist > OFF_PLANE_TOL {
                    return false;
                }
            }
        }
        let Some(next_s) = nodes[vert].to[NavDir::South.as_index()] else {
            return false;
        };
        vert = next_s;
        if width > 1 || height > 1 {
            let dist = (nodes[vert].pos.dot(normal) + d).abs();
            if dist > OFF_PLANE_TOL {
                return false;
            }
        }
    }

    if width > 1 || height > 1 {
        let mut horiz = vert;
        for _x in 0..width {
            let Some(next_e) = nodes[horiz].to[NavDir::East.as_index()] else {
                return false;
            };
            horiz = next_e;
            let dist = (nodes[horiz].pos.dot(normal) + d).abs();
            if dist > OFF_PLANE_TOL {
                return false;
            }
        }
    }
    true
}

fn build_area(
    nodes: &mut [NavNode],
    nw_idx: usize,
    width: usize,
    height: usize,
    area_id: u32,
    area_idx: usize,
) -> Option<(GenArea, usize)> {
    let mut ne_idx = None;
    let mut vert = nw_idx;
    let mut covered_count = 0;

    for y in 0..height {
        let mut horiz = vert;
        for _x in 0..width {
            nodes[horiz].covered = true;
            nodes[horiz].area_idx = Some(area_idx);
            covered_count += 1;
            horiz = nodes[horiz].to[NavDir::East.as_index()]?;
        }
        if y == 0 {
            ne_idx = Some(horiz);
        }
        vert = nodes[vert].to[NavDir::South.as_index()]?;
    }

    let sw_idx = vert;
    let mut se_horiz = vert;
    for _x in 0..width {
        se_horiz = nodes[se_horiz].to[NavDir::East.as_index()]?;
    }
    let ne_idx = ne_idx?;
    let area = GenArea::from_nodes(area_id, nodes, nw_idx, ne_idx, se_horiz, sw_idx);
    Some((area, covered_count))
}

fn build_rectangular_areas(
    nodes: &mut [NavNode],
    max_width: usize,
    max_height: usize,
) -> Vec<GenArea> {
    let mut areas = Vec::new();
    let mut try_w = max_width.max(1);
    let mut try_h = max_height.max(1);
    let mut uncovered = nodes.len();
    let mut next_id = 1u32;

    while uncovered > 0 {
        for idx in 0..nodes.len() {
            if nodes[idx].covered {
                continue;
            }
            if test_area(nodes, idx, try_w, try_h) {
                let area_idx = areas.len();
                if let Some((area, covered)) = build_area(nodes, idx, try_w, try_h, next_id, area_idx)
                {
                    next_id += 1;
                    areas.push(area);
                    uncovered = uncovered.saturating_sub(covered);
                }
            }
        }
        if try_w == 1 && try_h == 1 {
            break;
        }
        if try_w >= try_h && try_w > 1 {
            try_w -= 1;
        } else if try_h > 1 {
            try_h -= 1;
        }
    }
    areas
}

fn find_jump_down_area<W: NavWorld + ?Sized>(
    world: &W,
    areas: &[GenArea],
    from_pos: Vec3,
    dir: NavDir,
    step: f32,
) -> Option<usize> {
    let dv = dir.to_vec2();
    let start = Vec3::new(
        from_pos.x + dv.x * (step * 0.5),
        from_pos.y + dv.y * (step * 0.5),
        from_pos.z + HALF_HUMAN_HEIGHT,
    );
    for i in 1..=4 {
        let probe = start + Vec3::new(dv.x * step * i as f32, dv.y * step * i as f32, 0.0);
        let tr = world.trace_line(start, probe);
        if tr.fraction < 1.0 || tr.start_solid {
            break;
        }
        // Find highest alive area beneath `probe` within DeathDrop
        let mut best: Option<(usize, f32)> = None;
        for (idx, area) in areas.iter().enumerate() {
            if !area.alive || !area.is_overlapping_xy(probe) {
                continue;
            }
            let z = area.z_at(probe.x, probe.y);
            if z <= probe.z + 5.0 && z >= probe.z - DEATH_DROP && best.is_none_or(|(_, bz)| z > bz)
            {
                best = Some((idx, z));
            }
        }
        if let Some((target_idx, target_z)) = best {
            let dz = from_pos.z - target_z;
            if dz > JUMP_CROUCH_HEIGHT && dz < DEATH_DROP {
                let top_a = Vec3::new(from_pos.x, from_pos.y, from_pos.z + HUMAN_HEIGHT);
                let top_b = Vec3::new(probe.x, probe.y, top_a.z);
                let tr1 = world.trace_line(top_a, top_b);
                let tr2 = world.trace_line(top_b, Vec3::new(probe.x, probe.y, target_z + 2.0));
                if tr1.fraction >= 1.0 && !tr1.start_solid && tr2.fraction >= 1.0 && !tr2.start_solid
                {
                    return Some(target_idx);
                }
            }
            break;
        }
    }
    None
}

fn push_unique(list: &mut Vec<usize>, val: usize) {
    if !list.contains(&val) {
        list.push(val);
    }
}

fn connect_generated_areas<W: NavWorld + ?Sized>(
    world: &W,
    nodes: &[NavNode],
    areas: &mut [GenArea],
    step: f32,
) {
    for area_idx in 0..areas.len() {
        let nw = areas[area_idx].nw_node;
        let ne = areas[area_idx].ne_node;
        let sw = areas[area_idx].sw_node;
        let se = areas[area_idx].se_node;

        let mut north_links = Vec::new();
        let mut west_links = Vec::new();
        let mut south_links = Vec::new();
        let mut east_links = Vec::new();

        // North edge: nw -> ne along East
        let mut cur = Some(nw);
        while let Some(n) = cur {
            if n == ne {
                break;
            }
            if let Some(adj) = nodes[n].to[NavDir::North.as_index()]
                && let Some(adj_area) = nodes[adj].area_idx
                && nodes[adj].to[NavDir::South.as_index()] == Some(n)
            {
                push_unique(&mut north_links, adj_area);
            } else if let Some(down) =
                find_jump_down_area(world, areas, nodes[n].pos, NavDir::North, step)
                && down != area_idx
            {
                push_unique(&mut north_links, down);
            }
            cur = nodes[n].to[NavDir::East.as_index()];
        }

        // West edge: nw -> sw along South
        cur = Some(nw);
        while let Some(n) = cur {
            if n == sw {
                break;
            }
            if let Some(adj) = nodes[n].to[NavDir::West.as_index()]
                && let Some(adj_area) = nodes[adj].area_idx
                && nodes[adj].to[NavDir::East.as_index()] == Some(n)
            {
                push_unique(&mut west_links, adj_area);
            } else if let Some(down) =
                find_jump_down_area(world, areas, nodes[n].pos, NavDir::West, step)
                && down != area_idx
            {
                push_unique(&mut west_links, down);
            }
            cur = nodes[n].to[NavDir::South.as_index()];
        }

        // South edge: step 1 north from sw, scan East up to se->north
        if let Some(start_s) = nodes[sw].to[NavDir::North.as_index()] {
            let end_s = nodes[se].to[NavDir::North.as_index()];
            let mut n_opt = Some(start_s);
            while let Some(n) = n_opt {
                if Some(n) == end_s {
                    break;
                }
                if let Some(adj) = nodes[n].to[NavDir::South.as_index()]
                    && let Some(adj_area) = nodes[adj].area_idx
                    && nodes[adj].to[NavDir::North.as_index()] == Some(n)
                {
                    push_unique(&mut south_links, adj_area);
                } else if let Some(down) =
                    find_jump_down_area(world, areas, nodes[n].pos, NavDir::South, step)
                    && down != area_idx
                {
                    push_unique(&mut south_links, down);
                }
                n_opt = nodes[n].to[NavDir::East.as_index()];
            }
        }

        // East edge: step 1 west from ne, scan South up to se->west
        if let Some(start_e) = nodes[ne].to[NavDir::West.as_index()] {
            let end_e = nodes[se].to[NavDir::West.as_index()];
            let mut n_opt = Some(start_e);
            while let Some(n) = n_opt {
                if Some(n) == end_e {
                    break;
                }
                if let Some(adj) = nodes[n].to[NavDir::East.as_index()]
                    && let Some(adj_area) = nodes[adj].area_idx
                    && nodes[adj].to[NavDir::West.as_index()] == Some(n)
                {
                    push_unique(&mut east_links, adj_area);
                } else if let Some(down) =
                    find_jump_down_area(world, areas, nodes[n].pos, NavDir::East, step)
                    && down != area_idx
                {
                    push_unique(&mut east_links, down);
                }
                n_opt = nodes[n].to[NavDir::South.as_index()];
            }
        }

        areas[area_idx].connections[NavDir::North.as_index()] = north_links;
        areas[area_idx].connections[NavDir::East.as_index()] = east_links;
        areas[area_idx].connections[NavDir::South.as_index()] = south_links;
        areas[area_idx].connections[NavDir::West.as_index()] = west_links;
    }
}

fn finish_merge(nodes: &mut [NavNode], areas: &mut [GenArea], keep_idx: usize, adj_idx: usize) {
    areas[keep_idx].nw_corner = nodes[areas[keep_idx].nw_node].pos;
    areas[keep_idx].se_corner = nodes[areas[keep_idx].se_node].pos;
    areas[keep_idx].ne_z = nodes[areas[keep_idx].ne_node].pos.z;
    areas[keep_idx].sw_z = nodes[areas[keep_idx].sw_node].pos.z;

    for node in nodes.iter_mut() {
        if node.area_idx == Some(adj_idx) {
            node.area_idx = Some(keep_idx);
        }
    }

    // Inherit connections from adj_idx
    for dir in 0..4 {
        let adj_conns = areas[adj_idx].connections[dir].clone();
        for c in adj_conns {
            if c != adj_idx && c != keep_idx {
                push_unique(&mut areas[keep_idx].connections[dir], c);
            }
        }
        areas[keep_idx].connections[dir].retain(|&c| c != adj_idx);
    }

    // Rewrite references to adj_idx across all surviving areas
    for (idx, area) in areas.iter_mut().enumerate() {
        if !area.alive || idx == keep_idx || idx == adj_idx {
            continue;
        }
        for conns in &mut area.connections {
            if conns.contains(&adj_idx) {
                conns.retain(|&c| c != adj_idx && c != keep_idx);
                conns.push(keep_idx);
            }
        }
    }
    areas[adj_idx].alive = false;
}

fn merge_generated_areas(nodes: &mut [NavNode], areas: &mut [GenArea]) {
    loop {
        let mut merged = false;
        for i in 0..areas.len() {
            if !areas[i].alive {
                continue;
            }
            // North
            let north_list = areas[i].connections[NavDir::North.as_index()].clone();
            for adj in north_list {
                if !areas[adj].alive {
                    continue;
                }
                if areas[i].nw_node == areas[adj].sw_node
                    && areas[i].ne_node == areas[adj].se_node
                    && areas[i].attributes == areas[adj].attributes
                    && areas[i].is_coplanar(&areas[adj])
                {
                    areas[i].nw_node = areas[adj].nw_node;
                    areas[i].ne_node = areas[adj].ne_node;
                    finish_merge(nodes, areas, i, adj);
                    merged = true;
                    break;
                }
            }
            if merged {
                break;
            }
            // South
            let south_list = areas[i].connections[NavDir::South.as_index()].clone();
            for adj in south_list {
                if !areas[adj].alive {
                    continue;
                }
                if areas[adj].nw_node == areas[i].sw_node
                    && areas[adj].ne_node == areas[i].se_node
                    && areas[i].attributes == areas[adj].attributes
                    && areas[i].is_coplanar(&areas[adj])
                {
                    areas[i].sw_node = areas[adj].sw_node;
                    areas[i].se_node = areas[adj].se_node;
                    finish_merge(nodes, areas, i, adj);
                    merged = true;
                    break;
                }
            }
            if merged {
                break;
            }
            // West
            let west_list = areas[i].connections[NavDir::West.as_index()].clone();
            for adj in west_list {
                if !areas[adj].alive {
                    continue;
                }
                if areas[i].nw_node == areas[adj].ne_node
                    && areas[i].sw_node == areas[adj].se_node
                    && areas[i].attributes == areas[adj].attributes
                    && areas[i].is_coplanar(&areas[adj])
                {
                    areas[i].nw_node = areas[adj].nw_node;
                    areas[i].sw_node = areas[adj].sw_node;
                    finish_merge(nodes, areas, i, adj);
                    merged = true;
                    break;
                }
            }
            if merged {
                break;
            }
            // East
            let east_list = areas[i].connections[NavDir::East.as_index()].clone();
            for adj in east_list {
                if !areas[adj].alive {
                    continue;
                }
                if areas[adj].nw_node == areas[i].ne_node
                    && areas[adj].sw_node == areas[i].se_node
                    && areas[i].attributes == areas[adj].attributes
                    && areas[i].is_coplanar(&areas[adj])
                {
                    areas[i].ne_node = areas[adj].ne_node;
                    areas[i].se_node = areas[adj].se_node;
                    finish_merge(nodes, areas, i, adj);
                    merged = true;
                    break;
                }
            }
            if merged {
                break;
            }
        }
        if !merged {
            break;
        }
    }
}

fn split_area_x(areas: &mut Vec<GenArea>, idx: usize, step: f32) {
    if !areas[idx].alive || areas[idx].is_roughly_square() {
        return;
    }
    let split_x = snap_to_grid(areas[idx].nw_corner.x + areas[idx].size_x() * 0.5, step);
    if (split_x - areas[idx].nw_corner.x).abs() < 0.1
        || (split_x - areas[idx].se_corner.x).abs() < 0.1
    {
        return;
    }
    let nw_z = areas[idx].z_at(split_x, areas[idx].nw_corner.y);
    let se_z = areas[idx].z_at(split_x, areas[idx].se_corner.y);

    let new_idx = areas.len();
    let new_id = (areas.len() + 1) as u32;

    let old_east = areas[idx].connections[NavDir::East.as_index()].clone();
    let mut east_area = areas[idx].clone();
    east_area.id = new_id;
    east_area.nw_corner = Vec3::new(split_x, areas[idx].nw_corner.y, nw_z);
    east_area.sw_z = se_z;
    east_area.connections[NavDir::West.as_index()] = vec![idx];
    east_area.connections[NavDir::East.as_index()] = old_east.clone();

    areas[idx].se_corner = Vec3::new(split_x, areas[idx].se_corner.y, se_z);
    areas[idx].ne_z = nw_z;
    areas[idx].connections[NavDir::East.as_index()] = vec![new_idx];

    for other in old_east {
        areas[other].connections[NavDir::West.as_index()].retain(|&c| c != idx);
        push_unique(&mut areas[other].connections[NavDir::West.as_index()], new_idx);
    }

    // Keep overlapping north/south connections on both halves.
    for dir in [NavDir::North, NavDir::South] {
        let opp = dir.opposite();
        let list = areas[idx].connections[dir.as_index()].clone();
        areas[idx].connections[dir.as_index()].clear();
        east_area.connections[dir.as_index()].clear();
        for other in list {
            let overlap_left = areas[idx].nw_corner.x < areas[other].se_corner.x - 0.1
                && areas[idx].se_corner.x > areas[other].nw_corner.x + 0.1;
            let overlap_right = east_area.nw_corner.x < areas[other].se_corner.x - 0.1
                && east_area.se_corner.x > areas[other].nw_corner.x + 0.1;
            if overlap_left {
                push_unique(&mut areas[idx].connections[dir.as_index()], other);
            } else {
                areas[other].connections[opp.as_index()].retain(|&c| c != idx);
            }
            if overlap_right {
                push_unique(&mut east_area.connections[dir.as_index()], other);
                if areas[other].connections[opp.as_index()].contains(&idx) || !overlap_left {
                    push_unique(&mut areas[other].connections[opp.as_index()], new_idx);
                }
            }
        }
    }

    areas.push(east_area);
    split_area_x(areas, idx, step);
    split_area_x(areas, new_idx, step);
}

fn split_area_y(areas: &mut Vec<GenArea>, idx: usize, step: f32) {
    if !areas[idx].alive || areas[idx].is_roughly_square() {
        return;
    }
    let split_y = snap_to_grid(areas[idx].nw_corner.y + areas[idx].size_y() * 0.5, step);
    if (split_y - areas[idx].nw_corner.y).abs() < 0.1
        || (split_y - areas[idx].se_corner.y).abs() < 0.1
    {
        return;
    }
    let nw_z = areas[idx].z_at(areas[idx].nw_corner.x, split_y);
    let se_z = areas[idx].z_at(areas[idx].se_corner.x, split_y);

    let new_idx = areas.len();
    let new_id = (areas.len() + 1) as u32;

    let old_south = areas[idx].connections[NavDir::South.as_index()].clone();
    let mut south_area = areas[idx].clone();
    south_area.id = new_id;
    south_area.nw_corner = Vec3::new(areas[idx].nw_corner.x, split_y, nw_z);
    south_area.ne_z = se_z;
    south_area.connections[NavDir::North.as_index()] = vec![idx];
    south_area.connections[NavDir::South.as_index()] = old_south.clone();

    areas[idx].se_corner = Vec3::new(areas[idx].se_corner.x, split_y, se_z);
    areas[idx].sw_z = nw_z;
    areas[idx].connections[NavDir::South.as_index()] = vec![new_idx];

    for other in old_south {
        areas[other].connections[NavDir::North.as_index()].retain(|&c| c != idx);
        push_unique(&mut areas[other].connections[NavDir::North.as_index()], new_idx);
    }

    for dir in [NavDir::East, NavDir::West] {
        let opp = dir.opposite();
        let list = areas[idx].connections[dir.as_index()].clone();
        areas[idx].connections[dir.as_index()].clear();
        south_area.connections[dir.as_index()].clear();
        for other in list {
            let overlap_top = areas[idx].nw_corner.y < areas[other].se_corner.y - 0.1
                && areas[idx].se_corner.y > areas[other].nw_corner.y + 0.1;
            let overlap_bot = south_area.nw_corner.y < areas[other].se_corner.y - 0.1
                && south_area.se_corner.y > areas[other].nw_corner.y + 0.1;
            if overlap_top {
                push_unique(&mut areas[idx].connections[dir.as_index()], other);
            } else {
                areas[other].connections[opp.as_index()].retain(|&c| c != idx);
            }
            if overlap_bot {
                push_unique(&mut south_area.connections[dir.as_index()], other);
                if areas[other].connections[opp.as_index()].contains(&idx) || !overlap_top {
                    push_unique(&mut areas[other].connections[opp.as_index()], new_idx);
                }
            }
        }
    }

    areas.push(south_area);
    split_area_y(areas, idx, step);
    split_area_y(areas, new_idx, step);
}

fn square_up_areas(_nodes: &mut [NavNode], areas: &mut Vec<GenArea>, step: f32) {
    let initial_len = areas.len();
    for idx in 0..initial_len {
        if !areas[idx].alive || areas[idx].is_roughly_square() {
            continue;
        }
        if areas[idx].size_x() > areas[idx].size_y() {
            split_area_x(areas, idx, step);
        } else {
            split_area_y(areas, idx, step);
        }
    }
}

fn mark_jump_areas(areas: &mut [GenArea]) {
    for area in areas.iter_mut() {
        if !area.alive {
            continue;
        }
        if area.surface_normal().z < MAX_UNIT_Z_SLOPE {
            area.attributes.insert(NavAttribute::JUMP);
        }
    }
}

fn compact_areas(areas: &[GenArea]) -> Vec<NavArea> {
    let mut old_to_new_id = HashMap::new();
    let mut next_id = 1u32;
    for (idx, a) in areas.iter().enumerate() {
        if a.alive {
            old_to_new_id.insert(idx, next_id);
            next_id += 1;
        }
    }

    let mut out = Vec::with_capacity(old_to_new_id.len());
    for (idx, a) in areas.iter().enumerate() {
        if !a.alive {
            continue;
        }
        let id = old_to_new_id[&idx];
        let map_conns = |list: &[usize]| -> Vec<u32> {
            let mut ids: Vec<u32> = list
                .iter()
                .filter_map(|old| old_to_new_id.get(old).copied())
                .collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        };
        out.push(NavArea {
            id,
            attributes: a.attributes,
            nw_corner: a.nw_corner,
            se_corner: a.se_corner,
            ne_z: a.ne_z,
            sw_z: a.sw_z,
            connections: [
                map_conns(&a.connections[0]),
                map_conns(&a.connections[1]),
                map_conns(&a.connections[2]),
                map_conns(&a.connections[3]),
            ],
            hiding_spots: Vec::new(),
            approach_spots: Vec::new(),
            encounters: Vec::new(),
            place_id: UNDEFINED_PLACE,
            ladders_up: Vec::new(),
            ladders_down: Vec::new(),
        });
    }
    out
}

fn find_first_area_in_direction<W: NavWorld + ?Sized>(
    world: &W,
    areas: &[NavArea],
    start: Vec3,
    dir: NavDir,
    range: f32,
    beneath_limit: f32,
    step: f32,
) -> Option<usize> {
    let steps = ((range / step) + 0.5) as usize;
    let dv = dir.to_vec2() * step;
    let mut pos = start;
    for _ in 0..steps {
        pos.x += dv.x;
        pos.y += dv.y;
        let tr = world.trace_line(start, pos);
        if tr.fraction < 1.0 || tr.start_solid {
            break;
        }
        let mut best: Option<(usize, f32)> = None;
        for (idx, area) in areas.iter().enumerate() {
            if !area.is_overlapping_xy(pos) {
                continue;
            }
            let z = area.z_at(pos.x, pos.y);
            if z <= pos.z + 5.0 && z >= pos.z - beneath_limit && best.is_none_or(|(_, bz)| z > bz) {
                best = Some((idx, z));
            }
        }
        if let Some((idx, _)) = best {
            return Some(idx);
        }
    }
    None
}

fn find_nearest_area_idx(areas: &[NavArea], pos: Vec3) -> Option<usize> {
    areas
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            a.distance_squared_to_point(pos)
                .total_cmp(&b.distance_squared_to_point(pos))
        })
        .map(|(i, _)| i)
}

fn link_ladders<W: NavWorld + ?Sized>(
    world: &W,
    geoms: &[LadderGeom],
    areas: &mut [NavArea],
    step: f32,
) -> Vec<NavLadder> {
    let near_range = 75.0;
    let mut ladders = Vec::with_capacity(geoms.len());

    for (i, g) in geoms.iter().enumerate() {
        let ladder_id = (i + 1) as u32;
        let d2 = g.dir.to_vec2() * HALF_HUMAN_WIDTH;

        let bottom_center = g.bottom + Vec3::new(d2.x, d2.y, step);
        let bottom_idx = find_nearest_area_idx(areas, bottom_center);

        let top_center = g.top + Vec3::new(d2.x, d2.y, step);
        let top_fwd_idx = find_first_area_in_direction(
            world,
            areas,
            top_center,
            g.dir.opposite(),
            near_range,
            120.0,
            step,
        )
        .filter(|&idx| Some(idx) != bottom_idx);

        let top_left_idx = find_first_area_in_direction(
            world,
            areas,
            top_center,
            g.dir.left(),
            near_range,
            120.0,
            step,
        )
        .filter(|&idx| Some(idx) != bottom_idx);

        let top_right_idx = find_first_area_in_direction(
            world,
            areas,
            top_center,
            g.dir.right(),
            near_range,
            120.0,
            step,
        )
        .filter(|&idx| Some(idx) != bottom_idx);

        let top_behind_idx = find_first_area_in_direction(
            world,
            areas,
            top_center,
            g.dir,
            2.0 * near_range,
            120.0,
            step,
        )
        .filter(|&idx| Some(idx) != bottom_idx);

        let mut adjusted_top = g.top;
        let mut max_top_z = f32::NEG_INFINITY;
        for idx in [top_fwd_idx, top_left_idx, top_right_idx, top_behind_idx]
            .into_iter()
            .flatten()
        {
            let cp = areas[idx].closest_point(g.top);
            if cp.z > max_top_z {
                max_top_z = cp.z;
            }
        }
        if max_top_z.is_finite() {
            adjusted_top.z = max_top_z;
        }

        let is_dangling = bottom_idx.is_some_and(|b_idx| {
            let spot = areas[b_idx].closest_point(g.bottom);
            g.bottom.z - spot.z > HUMAN_HEIGHT
        });

        if let Some(b_idx) = bottom_idx {
            areas[b_idx].ladders_up.push(ladder_id);
        }
        for t_idx in [top_fwd_idx, top_left_idx, top_right_idx, top_behind_idx]
            .into_iter()
            .flatten()
        {
            if !areas[t_idx].ladders_down.contains(&ladder_id) {
                areas[t_idx].ladders_down.push(ladder_id);
            }
        }

        ladders.push(NavLadder {
            id: ladder_id,
            top: adjusted_top,
            bottom: g.bottom,
            width: g.width,
            length: (adjusted_top - g.bottom).length(),
            dir: g.dir,
            top_forward_area_id: top_fwd_idx.map(|idx| areas[idx].id),
            top_left_area_id: top_left_idx.map(|idx| areas[idx].id),
            top_right_area_id: top_right_idx.map(|idx| areas[idx].id),
            top_behind_area_id: top_behind_idx.map(|idx| areas[idx].id),
            bottom_area_id: bottom_idx.map(|idx| areas[idx].id),
            is_dangling,
        });
    }

    ladders
}

fn is_hiding_spot_in_cover<W: NavWorld + ?Sized>(world: &W, spot: Vec3) -> bool {
    let from = spot + Vec3::new(0.0, 0.0, HALF_HUMAN_HEIGHT);
    // Crouched underneath an overhang counts as cover (`nav_area.cpp:2601`).
    let ceiling = world.trace_line(from, from + Vec3::new(0.0, 0.0, 20.0));
    if ceiling.fraction < 1.0 || ceiling.start_solid {
        return true;
    }

    const COVER_RANGE: f32 = 100.0;
    let mut cover_count = 0usize;
    for step in 0..16 {
        let angle = (step as f32) * (std::f32::consts::TAU / 16.0);
        let to = from
            + Vec3::new(
                COVER_RANGE * angle.cos(),
                COVER_RANGE * angle.sin(),
                HALF_HUMAN_HEIGHT,
            );
        let tr = world.trace_line(from, to);
        if tr.fraction < 1.0 || tr.start_solid {
            cover_count += 1;
        }
    }
    cover_count >= 8
}

fn compute_hiding_spots<W: NavWorld + ?Sized>(world: &W, file: &mut NavFile) {
    let id_to_idx: HashMap<u32, usize> = file
        .areas
        .iter()
        .enumerate()
        .map(|(i, a)| (a.id, i))
        .collect();
    let mut next_spot_id = 1u32;

    for area_idx in 0..file.areas.len() {
        let attrs = file.areas[area_idx].attributes;
        if attrs.contains(NavAttribute::JUMP) || attrs.contains(NavAttribute::DONT_HIDE) {
            continue;
        }

        let nw = file.areas[area_idx].nw_corner;
        let se = file.areas[area_idx].se_corner;
        let mut corner_count = [0u8; 4];
        const CORNER_SIZE: f32 = 20.0;

        for dir in NavDir::ALL {
            let mut min_ext = f32::INFINITY;
            let mut max_ext = f32::NEG_INFINITY;
            let is_horiz = matches!(dir, NavDir::North | NavDir::South);

            for &adj_id in &file.areas[area_idx].connections[dir.as_index()] {
                let Some(&adj_idx) = id_to_idx.get(&adj_id) else {
                    continue;
                };
                let adj = &file.areas[adj_idx];
                if !adj.is_connected_in_dir(file.areas[area_idx].id, dir.opposite())
                    || adj.attributes.contains(NavAttribute::JUMP)
                {
                    continue;
                }
                if is_horiz {
                    min_ext = min_ext.min(adj.nw_corner.x);
                    max_ext = max_ext.max(adj.se_corner.x);
                } else {
                    min_ext = min_ext.min(adj.nw_corner.y);
                    max_ext = max_ext.max(adj.se_corner.y);
                }
            }

            match dir {
                NavDir::North => {
                    if min_ext - nw.x >= CORNER_SIZE {
                        corner_count[NavCorner::NorthWest as usize] += 1;
                    }
                    if se.x - max_ext >= CORNER_SIZE {
                        corner_count[NavCorner::NorthEast as usize] += 1;
                    }
                }
                NavDir::South => {
                    if min_ext - nw.x >= CORNER_SIZE {
                        corner_count[NavCorner::SouthWest as usize] += 1;
                    }
                    if se.x - max_ext >= CORNER_SIZE {
                        corner_count[NavCorner::SouthEast as usize] += 1;
                    }
                }
                NavDir::East => {
                    if min_ext - nw.y >= CORNER_SIZE {
                        corner_count[NavCorner::NorthEast as usize] += 1;
                    }
                    if se.y - max_ext >= CORNER_SIZE {
                        corner_count[NavCorner::SouthEast as usize] += 1;
                    }
                }
                NavDir::West => {
                    if min_ext - nw.y >= CORNER_SIZE {
                        corner_count[NavCorner::NorthWest as usize] += 1;
                    }
                    if se.y - max_ext >= CORNER_SIZE {
                        corner_count[NavCorner::SouthWest as usize] += 1;
                    }
                }
            }
        }

        const OFFSET: f32 = 12.5;
        const COLLISION_RANGE: f32 = 30.0;
        let corner_offsets = [
            (NavCorner::NorthWest, Vec3::new(OFFSET, OFFSET, 0.0)),
            (NavCorner::NorthEast, Vec3::new(-OFFSET, OFFSET, 0.0)),
            (NavCorner::SouthWest, Vec3::new(OFFSET, -OFFSET, 0.0)),
            (NavCorner::SouthEast, Vec3::new(-OFFSET, -OFFSET, 0.0)),
        ];

        let mut spots = Vec::new();
        for (corner, delta) in corner_offsets {
            if corner_count[corner as usize] != 2 {
                continue;
            }
            let pos = file.areas[area_idx].corner(corner) + delta;
            if spots
                .iter()
                .any(|s: &HidingSpot| s.pos.distance_squared(pos) < COLLISION_RANGE * COLLISION_RANGE)
            {
                continue;
            }
            let flags = if is_hiding_spot_in_cover(world, pos) {
                HidingSpotFlags::IN_COVER
            } else {
                HidingSpotFlags::EXPOSED
            };
            spots.push(HidingSpot {
                id: next_spot_id,
                pos,
                flags,
            });
            next_spot_id += 1;
        }
        file.areas[area_idx].hiding_spots = spots;
    }
}

fn classify_sniper_spots<W: NavWorld + ?Sized>(world: &W, file: &mut NavFile, step: f32) {
    const MIN_SNIPER_RANGE_SQ: f32 = 1000.0 * 1000.0;
    const MIN_IDEAL_AREA: f32 = 200.0 * 200.0;
    const LONG_SNIPER_RANGE_SQ: f32 = 1500.0 * 1500.0;

    // Sample representative sight targets per area (to bound O(spots * samples) cleanly).
    let mut targets = Vec::new();
    for area in &file.areas {
        let sx = area.size_x().max(step);
        let sy = area.size_y().max(step);
        let sample_step = (sx.max(sy) / 3.0).max(step);
        let mut y = area.nw_corner.y + step * 0.5;
        while y < area.se_corner.y {
            let mut x = area.nw_corner.x + step * 0.5;
            while x < area.se_corner.x {
                targets.push(Vec3::new(x, y, area.z_at(x, y) + HALF_HUMAN_HEIGHT));
                x += sample_step;
            }
            y += sample_step;
        }
    }

    for area in &mut file.areas {
        for spot in &mut area.hiding_spots {
            let eye = spot.pos + Vec3::new(0.0, 0.0, HALF_HUMAN_HEIGHT);
            let mut farthest_sq = 0.0f32;
            let mut min_vis = Vec3::splat(f32::INFINITY);
            let mut max_vis = Vec3::splat(f32::NEG_INFINITY);
            let mut found = false;

            for &t in &targets {
                let d2 = eye.distance_squared(t);
                if d2 < MIN_SNIPER_RANGE_SQ {
                    continue;
                }
                let tr = world.trace_line(eye, t);
                if tr.fraction >= 1.0 && !tr.start_solid {
                    farthest_sq = farthest_sq.max(d2);
                    min_vis = min_vis.min(t);
                    max_vis = max_vis.max(t);
                    found = true;
                }
            }

            if found {
                let vis_area = (max_vis.x - min_vis.x).max(0.0) * (max_vis.y - min_vis.y).max(0.0);
                if vis_area >= MIN_IDEAL_AREA || farthest_sq >= LONG_SNIPER_RANGE_SQ {
                    spot.flags.insert(HidingSpotFlags::IDEAL_SNIPER_SPOT);
                } else {
                    spot.flags.insert(HidingSpotFlags::GOOD_SNIPER_SPOT);
                }
            }
        }
    }
}

fn compute_spot_encounters<W: NavWorld + ?Sized>(world: &W, file: &mut NavFile) {
    let id_to_idx: HashMap<u32, usize> = file
        .areas
        .iter()
        .enumerate()
        .map(|(i, a)| (a.id, i))
        .collect();

    // Collect cover spots across the mesh.
    let all_cover_spots: Vec<HidingSpot> = file
        .areas
        .iter()
        .flat_map(|a| a.hiding_spots.iter().copied())
        .filter(|s| s.has_good_cover())
        .collect();
    if all_cover_spots.is_empty() {
        return;
    }

    const STEP_SIZE: f32 = 25.0;
    const SEE_RANGE_SQ: f32 = 2000.0 * 2000.0;

    for area_idx in 0..file.areas.len() {
        let mut encounters = Vec::new();
        for from_dir in NavDir::ALL {
            let from_ids = file.areas[area_idx].connections[from_dir.as_index()].clone();
            for from_id in from_ids {
                let Some(&from_idx) = id_to_idx.get(&from_id) else {
                    continue;
                };
                for to_dir in NavDir::ALL {
                    let to_ids = file.areas[area_idx].connections[to_dir.as_index()].clone();
                    for to_id in to_ids {
                        if to_id == from_id {
                            continue;
                        }
                        let Some(&to_idx) = id_to_idx.get(&to_id) else {
                            continue;
                        };
                        let (mut p_from, _) =
                            file.areas[area_idx].compute_portal(&file.areas[from_idx], from_dir);
                        let (mut p_to, _) =
                            file.areas[area_idx].compute_portal(&file.areas[to_idx], to_dir);
                        p_from.z = file.areas[from_idx].z_at(p_from.x, p_from.y) + HALF_HUMAN_HEIGHT;
                        p_to.z = file.areas[to_idx].z_at(p_to.x, p_to.y) + HALF_HUMAN_HEIGHT;

                        let delta = p_to - p_from;
                        let length = delta.length();
                        if length < 1.0 {
                            continue;
                        }
                        let dir = delta / length;
                        let mut seen = HashSet::new();
                        let mut spots = Vec::new();

                        let mut along = 0.0f32;
                        loop {
                            let clamped = along.min(length);
                            let eye = p_from + dir * clamped;
                            for spot in &all_cover_spots {
                                if seen.contains(&spot.id) {
                                    continue;
                                }
                                let spot_eye = spot.pos + Vec3::new(0.0, 0.0, HALF_HUMAN_HEIGHT);
                                if eye.distance_squared(spot_eye) > SEE_RANGE_SQ {
                                    continue;
                                }
                                let tr = world.trace_line(eye, spot_eye);
                                if tr.fraction < 1.0 || tr.start_solid {
                                    continue;
                                }
                                let to_spot = (spot_eye - eye).normalize_or_zero();
                                let dot = dir.dot(to_spot);
                                if dot < std::f32::consts::FRAC_1_SQRT_2
                                    && dot > -std::f32::consts::FRAC_1_SQRT_2
                                    && clamped > 0.0
                                {
                                    spots.push(SpotOrder {
                                        spot_id: spot.id,
                                        t: clamped / length,
                                    });
                                }
                                seen.insert(spot.id);
                            }
                            if along >= length {
                                break;
                            }
                            along += STEP_SIZE;
                        }

                        encounters.push(SpotEncounter {
                            from_id,
                            from_dir,
                            to_id,
                            to_dir,
                            spots,
                        });
                    }
                }
            }
        }
        file.areas[area_idx].encounters = encounters;
    }
}

fn tag_places_from_hints<W: NavWorld + ?Sized>(world: &W, file: &mut NavFile) {
    let hints = world.zone_hints();
    if hints.is_empty() || file.areas.is_empty() {
        return;
    }

    let mut bomb_site_idx = 0usize;
    let bomb_names = ["BombsiteA", "BombsiteB", "BombsiteC"];
    let mut intern_place = |name: &str| -> u16 {
        if let Some(pos) = file.places.iter().position(|p| p.eq_ignore_ascii_case(name)) {
            (pos + 1) as u16
        } else {
            file.places.push(name.to_string());
            file.places.len() as u16
        }
    };

    let id_to_idx: HashMap<u32, usize> = file
        .areas
        .iter()
        .enumerate()
        .map(|(i, a)| (a.id, i))
        .collect();

    let mut seeds: Vec<(usize, u16, Vec3)> = Vec::new();
    for hint in &hints {
        let name: String = match &hint.label {
            Some(lbl) if !lbl.is_empty() => lbl.clone(),
            _ => match hint.kind {
                NavZoneKind::CtSpawn => "CTSpawn".to_string(),
                NavZoneKind::TSpawn => "TSpawn".to_string(),
                NavZoneKind::BombSite => {
                    let s = bomb_names
                        .get(bomb_site_idx)
                        .copied()
                        .unwrap_or("BombsiteC");
                    bomb_site_idx += 1;
                    s.to_string()
                }
                NavZoneKind::Hostage => "Hostages".to_string(),
                NavZoneKind::HostageRescue => "HostageRescueZone".to_string(),
                NavZoneKind::VipRescue => "VipRescueZone".to_string(),
            },
        };
        let pid = intern_place(&name);

        // Tag overlapping or nearest area.
        let mut matched_any = false;
        if let (Some(bmin), Some(bmax)) = (hint.mins, hint.maxs) {
            for (idx, area) in file.areas.iter_mut().enumerate() {
                if area.se_corner.x >= bmin.x
                    && area.nw_corner.x <= bmax.x
                    && area.se_corner.y >= bmin.y
                    && area.nw_corner.y <= bmax.y
                    && (area.center().z - hint.origin.z).abs() <= 140.0
                {
                    area.place_id = pid;
                    seeds.push((idx, pid, hint.origin));
                    matched_any = true;
                }
            }
        }
        if !matched_any && let Some(nearest) = find_nearest_area_idx(&file.areas, hint.origin) {
            file.areas[nearest].place_id = pid;
            seeds.push((nearest, pid, hint.origin));
        }
    }

    // Flood-propagate place IDs up to 350 units from each seed origin without overwriting existing tags.
    const PLACE_FLOOD_RADIUS_SQ: f32 = 350.0 * 350.0;
    let mut queue: VecDeque<(usize, u16, Vec3)> = seeds.into_iter().collect();
    while let Some((idx, pid, origin)) = queue.pop_front() {
        for dir in NavDir::ALL {
            let adj_ids = file.areas[idx].connections[dir.as_index()].clone();
            for adj_id in adj_ids {
                let Some(&adj_idx) = id_to_idx.get(&adj_id) else {
                    continue;
                };
                if file.areas[adj_idx].place_id != UNDEFINED_PLACE {
                    continue;
                }
                if file.areas[adj_idx].center().distance_squared(origin) <= PLACE_FLOOD_RADIUS_SQ {
                    file.areas[adj_idx].place_id = pid;
                    queue.push_back((adj_idx, pid, origin));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;

    /// Simple AABB-backed mock world for deterministic Condition Zero nav_gen tests.
    struct MockWorld {
        solids: Vec<(Vec3, Vec3)>,
        seeds: Vec<Vec3>,
        ladders: Vec<NavLadderHint>,
        zones: Vec<NavZoneHint>,
    }

    impl MockWorld {
        fn room_with_floor(min: Vec2, max: Vec2, floor_z: f32, ceiling_z: f32) -> Self {
            Self {
                solids: vec![
                    // Floor slab
                    (
                        Vec3::new(min.x - 32.0, min.y - 32.0, floor_z - 64.0),
                        Vec3::new(max.x + 32.0, max.y + 32.0, floor_z),
                    ),
                    // Ceiling slab
                    (
                        Vec3::new(min.x - 32.0, min.y - 32.0, ceiling_z),
                        Vec3::new(max.x + 32.0, max.y + 32.0, ceiling_z + 64.0),
                    ),
                    // Perimeter walls
                    (
                        Vec3::new(min.x - 32.0, min.y - 32.0, floor_z),
                        Vec3::new(min.x, max.y + 32.0, ceiling_z),
                    ),
                    (
                        Vec3::new(max.x, min.y - 32.0, floor_z),
                        Vec3::new(max.x + 32.0, max.y + 32.0, ceiling_z),
                    ),
                    (
                        Vec3::new(min.x, min.y - 32.0, floor_z),
                        Vec3::new(max.x, min.y, ceiling_z),
                    ),
                    (
                        Vec3::new(min.x, max.y, floor_z),
                        Vec3::new(max.x, max.y + 32.0, ceiling_z),
                    ),
                ],
                seeds: Vec::new(),
                ladders: Vec::new(),
                zones: Vec::new(),
            }
        }
    }

    fn sweep_ray_aabb(start: Vec3, end: Vec3, bmin: Vec3, bmax: Vec3) -> Option<(f32, Vec3, bool)> {
        let inside = start.x >= bmin.x
            && start.x <= bmax.x
            && start.y >= bmin.y
            && start.y <= bmax.y
            && start.z >= bmin.z
            && start.z <= bmax.z;
        if inside {
            return Some((0.0, Vec3::Z, true));
        }
        let dir = end - start;
        let mut tmin = 0.0f32;
        let mut tmax = 1.0f32;
        let mut hit_normal = Vec3::Z;
        for axis in 0..3 {
            let s = start[axis];
            let d = dir[axis];
            let mn = bmin[axis];
            let mx = bmax[axis];
            if d.abs() < 1e-8 {
                if s < mn || s > mx {
                    return None;
                }
            } else {
                let inv = 1.0 / d;
                let mut t1 = (mn - s) * inv;
                let mut t2 = (mx - s) * inv;
                let mut n = Vec3::ZERO;
                n[axis] = if d > 0.0 { -1.0 } else { 1.0 };
                if t1 > t2 {
                    std::mem::swap(&mut t1, &mut t2);
                }
                if t1 > tmin {
                    tmin = t1;
                    hit_normal = n;
                }
                tmax = tmax.min(t2);
                if tmin > tmax {
                    return None;
                }
            }
        }
        Some((tmin, hit_normal, false))
    }

    impl NavWorld for MockWorld {
        fn trace_hull(&self, start: Vec3, end: Vec3, hull: Hull) -> TraceResult {
            let ext = hull.half_extents();
            let mut best_t = 1.0f32;
            let mut best_n = Vec3::Z;
            let mut start_solid = false;

            for &(bmin, bmax) in &self.solids {
                if let Some((t, n, ss)) = sweep_ray_aabb(start, end, bmin - ext, bmax + ext) {
                    if ss {
                        start_solid = true;
                        best_t = 0.0;
                        best_n = n;
                        break;
                    }
                    if t < best_t {
                        best_t = t;
                        best_n = n;
                    }
                }
            }
            TraceResult {
                fraction: best_t,
                end_pos: start.lerp(end, best_t),
                plane_normal: best_n,
                all_solid: start_solid,
                start_solid,
                hit_sky: false,
                hit_solid: best_t < 1.0 || start_solid,
                hitgroup: 0,
            }
        }

        fn point_contents(&self, pos: Vec3, hull: Hull) -> i32 {
            let ext = hull.half_extents();
            for &(bmin, bmax) in &self.solids {
                let mn = bmin - ext;
                let mx = bmax + ext;
                if pos.x > mn.x
                    && pos.x < mx.x
                    && pos.y > mn.y
                    && pos.y < mx.y
                    && pos.z > mn.z
                    && pos.z < mx.z
                {
                    return CONTENTS_SOLID;
                }
            }
            cs_pm::collision::CONTENTS_EMPTY
        }

        fn seed_points(&self) -> Vec<Vec3> {
            self.seeds.clone()
        }

        fn ladders(&self) -> Vec<NavLadderHint> {
            self.ladders.clone()
        }

        fn zone_hints(&self) -> Vec<NavZoneHint> {
            self.zones.clone()
        }
    }

    #[test]
    fn generates_merged_areas_places_and_crouch_duct() {
        // Main room [0..300, 0..300], ceiling 128, plus a crouch tunnel [300..450, 100..200], ceiling 48
        let mut world = MockWorld::room_with_floor(Vec2::ZERO, Vec2::new(450.0, 300.0), 0.0, 128.0);
        // Add low ceiling over x in [300..450]
        world.solids.push((
            Vec3::new(300.0, 0.0, 48.0),
            Vec3::new(450.0, 300.0, 128.0),
        ));
        // Add pillar in room corner to create hiding cover corners
        world.solids.push((
            Vec3::new(150.0, 0.0, 0.0),
            Vec3::new(200.0, 100.0, 128.0),
        ));

        world.seeds = vec![Vec3::new(50.0, 50.0, 16.0)];
        world.zones = vec![
            NavZoneHint {
                kind: NavZoneKind::CtSpawn,
                origin: Vec3::new(50.0, 50.0, 0.0),
                mins: None,
                maxs: None,
                label: None,
            },
            NavZoneHint {
                kind: NavZoneKind::BombSite,
                origin: Vec3::new(400.0, 150.0, 0.0),
                mins: Some(Vec3::new(350.0, 100.0, 0.0)),
                maxs: Some(Vec3::new(440.0, 200.0, 48.0)),
                label: None,
            },
        ];

        let mesh = generate_nav_mesh(&world, &NavGenConfig::default());
        assert!(!mesh.areas().is_empty(), "expected generated areas");
        assert!(
            mesh.areas().iter().any(|a| a.attributes.contains(NavAttribute::CROUCH)),
            "expected crouch area under 48u ceiling"
        );
        assert_eq!(mesh.get_place(Vec3::new(50.0, 50.0, 0.0)), Some("CTSpawn"));
        assert_eq!(
            mesh.get_place(Vec3::new(400.0, 150.0, 0.0)),
            Some("BombsiteA")
        );
        // Should have at least one hiding spot near the pillar corner
        assert!(
            mesh.areas().iter().any(|a| !a.hiding_spots.is_empty()),
            "expected corner hiding spots"
        );
        // Roundtrip generated mesh through .nav binary codec
        let bytes = mesh.to_nav_file().to_bytes().expect("encode generated nav");
        let reparsed = NavFile::parse(&bytes).expect("decode generated nav");
        assert_eq!(reparsed.areas.len(), mesh.areas().len());
    }

    #[test]
    fn generates_and_links_ladder_between_floors() {
        let mut world = MockWorld::room_with_floor(Vec2::ZERO, Vec2::new(300.0, 300.0), 0.0, 300.0);
        // Upper platform at z=160 for x in [150..300]
        world.solids.push((
            Vec3::new(150.0, 0.0, 144.0),
            Vec3::new(300.0, 300.0, 160.0),
        ));
        world.seeds = vec![Vec3::new(50.0, 150.0, 16.0)];
        // Ladder along x=145..150 facing West (-X) climbing from z=0 to z=160
        world.ladders = vec![NavLadderHint {
            mins: Vec3::new(142.0, 125.0, 0.0),
            maxs: Vec3::new(150.0, 175.0, 160.0),
        }];

        let mesh = generate_nav_mesh(&world, &NavGenConfig::default());
        assert_eq!(mesh.ladders().len(), 1);
        let ladder = &mesh.ladders()[0];
        assert!(ladder.bottom_area_id.is_some(), "ladder bottom should link");
        assert!(
            ladder.top_forward_area_id.is_some()
                || ladder.top_left_area_id.is_some()
                || ladder.top_right_area_id.is_some(),
            "ladder top should link to upper platform"
        );
    }
}
