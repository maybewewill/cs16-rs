// SPDX-License-Identifier: GPL-3.0-or-later
// Ported from ReGameDLL_CS `regamedll/game_shared/bot/nav_area.h` and `nav_path.cpp`.

//! A* navigation mesh pathfinder with per-team danger weights, door/breakable obstacle
//! penalties, crouch/jump/ladder traversal metadata, and waypoint string-pulling.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use cs_assets::nav::{NavAttribute, NavDir};
use glam::Vec3;

use crate::nav_gen::{HALF_HUMAN_WIDTH, HUMAN_HEIGHT, JUMP_CROUCH_HEIGHT, STEP_HEIGHT};
use crate::nav_mesh::NavMesh;

/// Maximum waypoints generated in a single path (`MAX_PATH_SEGMENTS = 256`).
pub const MAX_PATH_SEGMENTS: usize = 256;

/// Movement action required to traverse a [`PathWaypoint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathAction {
    Walk,
    CrouchRequired,
    JumpRequired,
    LadderTraverse {
        ladder_idx: usize,
        ladder_id: u32,
        ascending: bool,
        dir: NavDir,
    },
}

/// Edge transition type used to enter a waypoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraverseKind {
    Start,
    Cardinal(NavDir),
    JumpDown(NavDir),
    Jump,
    LadderUp,
    LadderDown,
}

/// A single waypoint along a [`NavPath`].
#[derive(Debug, Clone, PartialEq)]
pub struct PathWaypoint {
    pub area_idx: usize,
    pub area_id: u32,
    pub pos: Vec3,
    pub action: PathAction,
    pub traverse: TraverseKind,
    pub has_door: bool,
    pub has_breakable: bool,
}

/// Complete A* path result from `start` to `goal` (or closest reachable area).
#[derive(Debug, Clone, PartialEq)]
pub struct NavPath {
    pub waypoints: Vec<PathWaypoint>,
    pub reached_goal: bool,
    pub total_cost: f32,
}

impl NavPath {
    pub fn is_empty(&self) -> bool {
        self.waypoints.is_empty()
    }

    pub fn len(&self) -> usize {
        self.waypoints.len()
    }

    pub fn endpoint(&self) -> Option<Vec3> {
        self.waypoints.last().map(|w| w.pos)
    }

    pub fn length(&self) -> f32 {
        self.waypoints
            .windows(2)
            .map(|w| w[0].pos.distance(w[1].pos))
            .sum()
    }

    pub fn is_at_end(&self, pos: Vec3, tolerance: f32) -> bool {
        self.endpoint()
            .is_some_and(|end| pos.distance_squared(end) <= tolerance * tolerance)
    }

    /// Sample a position at distance `dist_along` from the start of the path.
    pub fn point_along_path(&self, dist_along: f32) -> Option<Vec3> {
        let first = self.waypoints.first()?;
        if dist_along <= 0.0 || self.waypoints.len() == 1 {
            return Some(first.pos);
        }
        let mut so_far = 0.0f32;
        for w in self.waypoints.windows(2) {
            let seg = w[1].pos - w[0].pos;
            let len = seg.length();
            if len > 1e-4 && so_far + len >= dist_along {
                let t = ((dist_along - so_far) / len).clamp(0.0, 1.0);
                return Some(w[0].pos + seg * t);
            }
            so_far += len;
        }
        self.endpoint()
    }
}

/// Dynamic door (`func_door`) and breakable (`func_breakable`) AABBs for path cost penalties.
#[derive(Debug, Clone, Default)]
pub struct PathObstacles {
    pub doors: Vec<(Vec3, Vec3)>,
    pub breakables: Vec<(Vec3, Vec3)>,
}

impl PathObstacles {
    pub fn is_empty(&self) -> bool {
        self.doors.is_empty() && self.breakables.is_empty()
    }

    fn check_segment(&self, a: Vec3, b: Vec3) -> (bool, bool) {
        let has_door = self
            .doors
            .iter()
            .any(|&(mn, mx)| segment_intersects_aabb(a, b, mn, mx));
        let has_breakable = self
            .breakables
            .iter()
            .any(|&(mn, mx)| segment_intersects_aabb(a, b, mn, mx));
        (has_door, has_breakable)
    }
}

/// Cost tuning parameters for [`find_path`].
#[derive(Debug, Clone, Copy)]
pub struct PathCostParams {
    /// Team index (`0 = T`, `1 = CT`) for reading area danger.
    pub team: usize,
    /// Current simulation timestamp (seconds) for danger decay.
    pub now: f64,
    /// Multiplier applied to decayed area danger (`0.0` = fastest route, `> 0.0` = safer route).
    pub danger_weight: f32,
    /// Penalty multiplier for `NAV_CROUCH` areas (ReGameDLL default `20.0`).
    pub crouch_penalty: f32,
    /// Penalty multiplier for `NAV_JUMP` areas or jump edges (ReGameDLL default `5.0`).
    pub jump_penalty: f32,
    /// Penalty multiplier for `NAV_AVOID` areas.
    pub avoid_penalty: f32,
    /// Multiplier for ladder traversals.
    pub ladder_penalty: f32,
    /// Additive cost when an edge crosses a `func_door`.
    pub door_cost: f32,
    /// Additive cost when an edge crosses a `func_breakable`.
    pub breakable_cost: f32,
    pub allow_jump_down: bool,
    pub allow_ladders: bool,
    pub allow_crouch: bool,
}

impl Default for PathCostParams {
    fn default() -> Self {
        Self {
            team: 0,
            now: 0.0,
            danger_weight: 0.0,
            crouch_penalty: 20.0,
            jump_penalty: 5.0,
            avoid_penalty: 10.0,
            ladder_penalty: 1.0,
            door_cost: 60.0,
            breakable_cost: 120.0,
            allow_jump_down: true,
            allow_ladders: true,
            allow_crouch: true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct HeapEntry {
    area_idx: usize,
    f_score: f32,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.area_idx == other.area_idx && self.f_score.to_bits() == other.f_score.to_bits()
    }
}

impl Eq for HeapEntry {}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .f_score
            .total_cmp(&self.f_score)
            .then_with(|| self.area_idx.cmp(&other.area_idx))
    }
}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Copy)]
struct ParentLink {
    prev_idx: usize,
    how: TraverseKind,
    ladder_idx: Option<usize>,
    has_door: bool,
    has_breakable: bool,
}

impl ParentLink {
    const EMPTY: Self = Self {
        prev_idx: usize::MAX,
        how: TraverseKind::Start,
        ladder_idx: None,
        has_door: false,
        has_breakable: false,
    };
}

/// Reusable generation-stamped A* scratch buffers (`O(1)` reset per search, zero per-query
/// allocation in steady state).
#[derive(Debug, Clone)]
pub struct PathScratch {
    g_score: Vec<f32>,
    state_gen: Vec<u32>,
    closed_gen: Vec<u32>,
    parents: Vec<ParentLink>,
    open: BinaryHeap<HeapEntry>,
    chain: Vec<(usize, ParentLink)>,
    generation: u32,
}

impl Default for PathScratch {
    fn default() -> Self {
        Self::new()
    }
}

impl PathScratch {
    pub fn new() -> Self {
        Self {
            g_score: Vec::new(),
            state_gen: Vec::new(),
            closed_gen: Vec::new(),
            parents: Vec::new(),
            open: BinaryHeap::new(),
            chain: Vec::new(),
            generation: 0,
        }
    }

    fn prepare(&mut self, n: usize) -> u32 {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.state_gen.fill(0);
            self.closed_gen.fill(0);
            self.generation = 1;
        }
        if self.g_score.len() < n {
            self.g_score.resize(n, 0.0);
            self.state_gen.resize(n, 0);
            self.closed_gen.resize(n, 0);
            self.parents.resize(n, ParentLink::EMPTY);
        }
        self.open.clear();
        self.chain.clear();
        self.generation
    }

    #[inline]
    fn is_closed(&self, idx: usize, gen_id: u32) -> bool {
        self.closed_gen[idx] == gen_id
    }

    #[inline]
    fn mark_closed(&mut self, idx: usize, gen_id: u32) {
        self.closed_gen[idx] = gen_id;
    }

    #[inline]
    fn g_score(&self, idx: usize, gen_id: u32) -> f32 {
        if self.state_gen[idx] == gen_id {
            self.g_score[idx]
        } else {
            f32::INFINITY
        }
    }

    #[inline]
    fn set_open_node(&mut self, idx: usize, gen_id: u32, g: f32, parent: ParentLink) {
        self.g_score[idx] = g;
        self.state_gen[idx] = gen_id;
        self.parents[idx] = parent;
    }

    #[inline]
    fn parent(&self, idx: usize, gen_id: u32) -> Option<ParentLink> {
        (self.state_gen[idx] == gen_id && self.parents[idx].prev_idx != usize::MAX)
            .then_some(self.parents[idx])
    }
}

/// Shortest path with default parameters and no dynamic obstacles.
pub fn find_path_simple(mesh: &NavMesh, start: Vec3, goal: Vec3) -> Option<NavPath> {
    find_path(
        mesh,
        start,
        goal,
        &PathCostParams::default(),
        &PathObstacles::default(),
    )
}

/// Compute an A* path from `start` to `goal` over `mesh`.
///
/// If the exact `goal` area is unreachable, returns a partial path (`reached_goal = false`)
/// to the visited area closest to `goal`.
pub fn find_path(
    mesh: &NavMesh,
    start: Vec3,
    goal: Vec3,
    params: &PathCostParams,
    obstacles: &PathObstacles,
) -> Option<NavPath> {
    let mut scratch = PathScratch::new();
    find_path_with_scratch(mesh, start, goal, params, obstacles, &mut scratch)
}

/// Zero-allocation A* pathfinder reusing generation-stamped buffers in `scratch`.
pub fn find_path_with_scratch(
    mesh: &NavMesh,
    start: Vec3,
    goal: Vec3,
    params: &PathCostParams,
    obstacles: &PathObstacles,
    scratch: &mut PathScratch,
) -> Option<NavPath> {
    let n = mesh.areas().len();
    if n == 0 {
        return None;
    }

    let start_idx = mesh.get_nearest_nav_area(start, false)?;
    let goal_idx_opt = mesh.get_nav_area(goal, 120.0);

    if goal_idx_opt == Some(start_idx) {
        return Some(build_trivial_path(mesh, start_idx, start, goal, obstacles));
    }

    let goal_pos_snapped = if let Some(g_idx) = goal_idx_opt {
        let g = &mesh.areas()[g_idx];
        Vec3::new(goal.x, goal.y, g.z_at(goal.x, goal.y))
    } else {
        goal
    };

    let gen_id = scratch.prepare(n);
    scratch.set_open_node(start_idx, gen_id, 0.0, ParentLink::EMPTY);
    let start_h = mesh.areas()[start_idx].center().distance(goal_pos_snapped);
    scratch.open.push(HeapEntry {
        area_idx: start_idx,
        f_score: start_h,
    });

    let mut closest_idx = start_idx;
    let mut closest_dist = start_h;
    let mut reached_goal = false;

    while let Some(HeapEntry { area_idx: curr, .. }) = scratch.open.pop() {
        if scratch.is_closed(curr, gen_id) {
            continue;
        }
        scratch.mark_closed(curr, gen_id);

        if Some(curr) == goal_idx_opt {
            closest_idx = curr;
            reached_goal = true;
            break;
        }

        let curr_area = &mesh.areas()[curr];
        let curr_center = curr_area.center();
        let curr_g = scratch.g_score(curr, gen_id);

        // 1. Expand 4 cardinal directions.
        for dir in NavDir::ALL {
            for &next in mesh.adjacent_indices(curr, dir) {
                if next == curr || scratch.is_closed(next, gen_id) || mesh.is_area_blocked(next) {
                    continue;
                }
                let next_area = &mesh.areas()[next];
                if !params.allow_crouch && next_area.attributes.contains(NavAttribute::CROUCH) {
                    continue;
                }

                let is_bidir = mesh
                    .adjacent_indices(next, dir.opposite())
                    .contains(&curr);
                let dz = next_area.center().z - curr_center.z;
                let is_jump_down = !is_bidir && dz < -STEP_HEIGHT;
                if is_jump_down && !params.allow_jump_down {
                    continue;
                }
                if dz > JUMP_CROUCH_HEIGHT + 4.0 {
                    continue;
                }

                let how = if is_jump_down {
                    TraverseKind::JumpDown(dir)
                } else if next_area.attributes.contains(NavAttribute::JUMP) || dz > STEP_HEIGHT {
                    TraverseKind::Jump
                } else {
                    TraverseKind::Cardinal(dir)
                };

                let next_center = next_area.center();
                let dist = curr_center.distance(next_center).max(1.0);
                let (has_door, has_breakable) = obstacles.check_segment(curr_center, next_center);
                let edge_cost = compute_edge_cost(
                    mesh,
                    next,
                    dist,
                    None,
                    how,
                    has_door,
                    has_breakable,
                    params,
                );
                let cand_g = curr_g + edge_cost;
                if cand_g < scratch.g_score(next, gen_id) {
                    let h = next_center.distance(goal_pos_snapped);
                    if h < closest_dist {
                        closest_dist = h;
                        closest_idx = next;
                    }
                    scratch.set_open_node(
                        next,
                        gen_id,
                        cand_g,
                        ParentLink {
                            prev_idx: curr,
                            how,
                            ladder_idx: None,
                            has_door,
                            has_breakable,
                        },
                    );
                    scratch.open.push(HeapEntry {
                        area_idx: next,
                        f_score: cand_g + h,
                    });
                }
            }
        }

        // 2. Expand ladder connections (up and down).
        if params.allow_ladders {
            for &l_idx in mesh.ladder_up_indices(curr) {
                let ladder = &mesh.ladders()[l_idx];
                if ladder.is_dangling {
                    continue;
                }
                for top_id in [
                    ladder.top_forward_area_id,
                    ladder.top_left_area_id,
                    ladder.top_right_area_id,
                ]
                .into_iter()
                .flatten()
                {
                    let Some(next) = mesh.area_index_by_id(top_id) else {
                        continue;
                    };
                    if next == curr || scratch.is_closed(next, gen_id) || mesh.is_area_blocked(next)
                    {
                        continue;
                    }
                    let dist = ladder.length.max(25.0);
                    let edge_cost = compute_edge_cost(
                        mesh,
                        next,
                        dist,
                        Some(l_idx),
                        TraverseKind::LadderUp,
                        false,
                        false,
                        params,
                    );
                    let cand_g = curr_g + edge_cost;
                    if cand_g < scratch.g_score(next, gen_id) {
                        let h = mesh.areas()[next].center().distance(goal_pos_snapped);
                        if h < closest_dist {
                            closest_dist = h;
                            closest_idx = next;
                        }
                        scratch.set_open_node(
                            next,
                            gen_id,
                            cand_g,
                            ParentLink {
                                prev_idx: curr,
                                how: TraverseKind::LadderUp,
                                ladder_idx: Some(l_idx),
                                has_door: false,
                                has_breakable: false,
                            },
                        );
                        scratch.open.push(HeapEntry {
                            area_idx: next,
                            f_score: cand_g + h,
                        });
                    }
                }
            }

            for &l_idx in mesh.ladder_down_indices(curr) {
                let ladder = &mesh.ladders()[l_idx];
                let Some(bot_id) = ladder.bottom_area_id else {
                    continue;
                };
                let Some(next) = mesh.area_index_by_id(bot_id) else {
                    continue;
                };
                if next == curr || scratch.is_closed(next, gen_id) || mesh.is_area_blocked(next) {
                    continue;
                }
                let dist = ladder.length.max(25.0);
                let edge_cost = compute_edge_cost(
                    mesh,
                    next,
                    dist,
                    Some(l_idx),
                    TraverseKind::LadderDown,
                    false,
                    false,
                    params,
                );
                let cand_g = curr_g + edge_cost;
                if cand_g < scratch.g_score(next, gen_id) {
                    let h = mesh.areas()[next].center().distance(goal_pos_snapped);
                    if h < closest_dist {
                        closest_dist = h;
                        closest_idx = next;
                    }
                    scratch.set_open_node(
                        next,
                        gen_id,
                        cand_g,
                        ParentLink {
                            prev_idx: curr,
                            how: TraverseKind::LadderDown,
                            ladder_idx: Some(l_idx),
                            has_door: false,
                            has_breakable: false,
                        },
                    );
                    scratch.open.push(HeapEntry {
                        area_idx: next,
                        f_score: cand_g + h,
                    });
                }
            }
        }
    }

    if closest_idx == start_idx && !reached_goal {
        let end_pt = mesh.areas()[start_idx].closest_point(goal);
        let mut p = build_trivial_path(mesh, start_idx, start, end_pt, obstacles);
        p.reached_goal = false;
        return Some(p);
    }

    // Reconstruct reversed area chain using reusable scratch buffer.
    let mut cursor = closest_idx;
    while cursor != start_idx {
        let link = scratch.parent(cursor, gen_id)?;
        scratch.chain.push((cursor, link));
        cursor = link.prev_idx;
    }
    scratch.chain.reverse();

    if scratch.chain.len() > MAX_PATH_SEGMENTS - 2 {
        scratch.chain.truncate(MAX_PATH_SEGMENTS - 2);
    }

    let final_area = scratch.chain.last().map_or(start_idx, |(idx, _)| *idx);
    let end_pos = if reached_goal {
        goal_pos_snapped
    } else {
        mesh.areas()[final_area].closest_point(goal)
    };

    let waypoints = build_waypoints(
        mesh,
        start_idx,
        start,
        &scratch.chain,
        final_area,
        end_pos,
        obstacles,
    );
    Some(NavPath {
        waypoints,
        reached_goal,
        total_cost: scratch.g_score(closest_idx, gen_id),
    })
}

#[allow(clippy::too_many_arguments)]
fn compute_edge_cost(
    mesh: &NavMesh,
    next_idx: usize,
    dist: f32,
    ladder_idx: Option<usize>,
    how: TraverseKind,
    has_door: bool,
    has_breakable: bool,
    params: &PathCostParams,
) -> f32 {
    let next_area = &mesh.areas()[next_idx];
    let mut cost = dist;

    if next_area.attributes.contains(NavAttribute::CROUCH) {
        cost += params.crouch_penalty * dist;
    }
    if next_area.attributes.contains(NavAttribute::JUMP)
        || matches!(how, TraverseKind::Jump | TraverseKind::JumpDown(_))
    {
        cost += params.jump_penalty * dist;
    }
    if next_area.attributes.contains(NavAttribute::AVOID) {
        cost += params.avoid_penalty * dist;
    }
    if ladder_idx.is_some() {
        cost *= params.ladder_penalty.max(0.1);
    }
    if params.danger_weight > 0.0 {
        let danger = mesh.peek_danger(next_idx, params.team, params.now);
        cost += danger * params.danger_weight;
    }
    if has_door {
        cost += params.door_cost;
    }
    if has_breakable {
        cost += params.breakable_cost;
    }
    cost
}

fn build_trivial_path(
    mesh: &NavMesh,
    area_idx: usize,
    start: Vec3,
    goal: Vec3,
    obstacles: &PathObstacles,
) -> NavPath {
    let area = &mesh.areas()[area_idx];
    let p0 = Vec3::new(start.x, start.y, area.z_at(start.x, start.y));
    let p1 = Vec3::new(goal.x, goal.y, area.z_at(goal.x, goal.y));
    let action = if area.attributes.contains(NavAttribute::CROUCH) {
        PathAction::CrouchRequired
    } else if area.attributes.contains(NavAttribute::JUMP) {
        PathAction::JumpRequired
    } else {
        PathAction::Walk
    };
    let (has_door, has_breakable) = obstacles.check_segment(p0, p1);
    NavPath {
        waypoints: vec![
            PathWaypoint {
                area_idx,
                area_id: area.id,
                pos: p0,
                action,
                traverse: TraverseKind::Start,
                has_door: false,
                has_breakable: false,
            },
            PathWaypoint {
                area_idx,
                area_id: area.id,
                pos: p1,
                action,
                traverse: TraverseKind::Start,
                has_door,
                has_breakable,
            },
        ],
        reached_goal: true,
        total_cost: p0.distance(p1),
    }
}

fn build_waypoints(
    mesh: &NavMesh,
    start_idx: usize,
    start: Vec3,
    chain: &[(usize, ParentLink)],
    final_area_idx: usize,
    end_pos: Vec3,
    obstacles: &PathObstacles,
) -> Vec<PathWaypoint> {
    let start_area = &mesh.areas()[start_idx];
    let mut waypoints = Vec::with_capacity(chain.len() + 4);

    let start_action = if start_area.attributes.contains(NavAttribute::CROUCH) {
        PathAction::CrouchRequired
    } else {
        PathAction::Walk
    };
    waypoints.push(PathWaypoint {
        area_idx: start_idx,
        area_id: start_area.id,
        pos: Vec3::new(start.x, start.y, start_area.z_at(start.x, start.y)),
        action: start_action,
        traverse: TraverseKind::Start,
        has_door: false,
        has_breakable: false,
    });

    for &(to_idx, link) in chain {
        let from_pos = waypoints.last().map_or(start, |w| w.pos);
        let to_area = &mesh.areas()[to_idx];

        match link.how {
            TraverseKind::Cardinal(_) | TraverseKind::JumpDown(_) | TraverseKind::Jump => {
                // Find the cardinal direction if `Jump` was triggered by steep slope/step-up.
                let card_dir = match link.how {
                    TraverseKind::Cardinal(d) | TraverseKind::JumpDown(d) => Some(d),
                    TraverseKind::Jump => NavDir::ALL
                        .into_iter()
                        .find(|&d| mesh.adjacent_indices(link.prev_idx, d).contains(&to_idx)),
                    _ => None,
                };
                let mut portal_pos = if let Some(d) = card_dir {
                    let mut p = mesh.compute_closest_point_in_portal(
                        link.prev_idx,
                        to_idx,
                        d,
                        from_pos,
                    );
                    let dv = d.to_vec2() * 5.0;
                    p.x += dv.x;
                    p.y += dv.y;
                    p.z = mesh.areas()[link.prev_idx].z_at(p.x, p.y);
                    p
                } else {
                    to_area.center()
                };

                let action = if to_area.attributes.contains(NavAttribute::CROUCH) {
                    PathAction::CrouchRequired
                } else if matches!(link.how, TraverseKind::Jump)
                    || to_area.attributes.contains(NavAttribute::JUMP)
                {
                    PathAction::JumpRequired
                } else {
                    PathAction::Walk
                };

                if matches!(link.how, TraverseKind::JumpDown(_))
                    && let Some(d) = card_dir
                {
                    // Push ledge node 25 units out, then add landing node (`nav_path.cpp:44-66`).
                    let dv = d.to_vec2() * 25.0;
                    portal_pos.x += dv.x;
                    portal_pos.y += dv.y;
                    waypoints.push(PathWaypoint {
                        area_idx: to_idx,
                        area_id: to_area.id,
                        pos: portal_pos,
                        action: PathAction::Walk,
                        traverse: link.how,
                        has_door: link.has_door,
                        has_breakable: link.has_breakable,
                    });
                    let mut land_pos = portal_pos + Vec3::new(dv.x, dv.y, 0.0);
                    land_pos.z = to_area.z_at(land_pos.x, land_pos.y);
                    waypoints.push(PathWaypoint {
                        area_idx: to_idx,
                        area_id: to_area.id,
                        pos: land_pos,
                        action,
                        traverse: link.how,
                        has_door: false,
                        has_breakable: false,
                    });
                    continue;
                }

                waypoints.push(PathWaypoint {
                    area_idx: to_idx,
                    area_id: to_area.id,
                    pos: portal_pos,
                    action,
                    traverse: link.how,
                    has_door: link.has_door,
                    has_breakable: link.has_breakable,
                });
            }
            TraverseKind::LadderUp => {
                if let Some(l_idx) = link.ladder_idx {
                    let ladder = &mesh.ladders()[l_idx];
                    let dv = ladder.dir.to_vec2() * (2.0 * HALF_HUMAN_WIDTH);
                    let mount_pos = ladder.bottom + Vec3::new(dv.x, dv.y, 0.0);
                    waypoints.push(PathWaypoint {
                        area_idx: to_idx,
                        area_id: to_area.id,
                        pos: mount_pos,
                        action: PathAction::LadderTraverse {
                            ladder_idx: l_idx,
                            ladder_id: ladder.id,
                            ascending: true,
                            dir: ladder.dir,
                        },
                        traverse: TraverseKind::LadderUp,
                        has_door: false,
                        has_breakable: false,
                    });
                }
            }
            TraverseKind::LadderDown => {
                if let Some(l_idx) = link.ladder_idx {
                    let ladder = &mesh.ladders()[l_idx];
                    let dv = ladder.dir.opposite().to_vec2() * (2.0 * HALF_HUMAN_WIDTH);
                    let mount_pos = ladder.top + Vec3::new(dv.x, dv.y, 0.0);
                    waypoints.push(PathWaypoint {
                        area_idx: to_idx,
                        area_id: to_area.id,
                        pos: mount_pos,
                        action: PathAction::LadderTraverse {
                            ladder_idx: l_idx,
                            ladder_id: ladder.id,
                            ascending: false,
                            dir: ladder.dir,
                        },
                        traverse: TraverseKind::LadderDown,
                        has_door: false,
                        has_breakable: false,
                    });
                }
            }
            TraverseKind::Start => {}
        }
    }

    let final_area = &mesh.areas()[final_area_idx];
    let last_pos = waypoints.last().map_or(start, |w| w.pos);
    let (has_door, has_breakable) = obstacles.check_segment(last_pos, end_pos);
    let final_action = if final_area.attributes.contains(NavAttribute::CROUCH) {
        PathAction::CrouchRequired
    } else {
        PathAction::Walk
    };
    if last_pos.distance_squared(end_pos) > 1.0 {
        waypoints.push(PathWaypoint {
            area_idx: final_area_idx,
            area_id: final_area.id,
            pos: end_pos,
            action: final_action,
            traverse: TraverseKind::Start,
            has_door,
            has_breakable,
        });
    }

    waypoints
}

/// Smooth out redundant intermediate [`PathAction::Walk`] waypoints using a caller-supplied
/// line-of-sight predicate (`clear_los(a, b)`). Preserves ladders, jumps, crouches,
/// doors, breakables, and `NAV_PRECISE` areas.
pub fn string_pull_with_los(path: &mut NavPath, clear_los: impl Fn(Vec3, Vec3) -> bool) {
    if path.waypoints.len() < 3 {
        return;
    }
    let mut pulled = Vec::with_capacity(path.waypoints.len());
    let mut anchor = 0usize;
    pulled.push(path.waypoints[0].clone());

    while anchor + 1 < path.waypoints.len() {
        let mut furthest = anchor + 1;
        for cand in (anchor + 2)..path.waypoints.len() {
            let mid = &path.waypoints[cand - 1];
            let target = &path.waypoints[cand];
            if !is_pullable_waypoint(mid) || !is_pullable_waypoint(target) {
                break;
            }
            let a = path.waypoints[anchor].pos;
            let b = target.pos;
            // Check feet+4, waist+36, and head+70 (matching CNavPath::FindNextOccludedNode).
            let ok = clear_los(a + Vec3::new(0.0, 0.0, 4.0), b + Vec3::new(0.0, 0.0, 4.0))
                && clear_los(a + Vec3::new(0.0, 0.0, 36.0), b + Vec3::new(0.0, 0.0, 36.0))
                && clear_los(
                    a + Vec3::new(0.0, 0.0, HUMAN_HEIGHT),
                    b + Vec3::new(0.0, 0.0, HUMAN_HEIGHT),
                );
            if ok {
                furthest = cand;
            } else {
                break;
            }
        }
        pulled.push(path.waypoints[furthest].clone());
        anchor = furthest;
    }

    path.waypoints = pulled;
}

/// Pure navmesh-only string-pulling that verifies sampled points along candidate shortcuts
/// stay inside walkable, non-`PRECISE`, non-`JUMP`, non-`CROUCH` areas of `mesh`.
pub fn string_pull_nav_corridor(mesh: &NavMesh, path: &mut NavPath) {
    string_pull_with_los(path, |a, b| {
        let dist = a.distance(b);
        let steps = ((dist / 16.0).ceil() as usize).max(2);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let p = a.lerp(b, t);
            // Adjust probe back down to floor range (handles waist/head probes from string_pull_with_los).
            let Some(idx) = mesh.get_nav_area(p, 120.0) else {
                return false;
            };
            let area = &mesh.areas()[idx];
            if area
                .attributes
                .contains(NavAttribute::PRECISE)
                || area.attributes.contains(NavAttribute::JUMP)
                || area.attributes.contains(NavAttribute::CROUCH)
                || mesh.is_area_blocked(idx)
            {
                return false;
            }
        }
        true
    });
}

fn is_pullable_waypoint(w: &PathWaypoint) -> bool {
    w.action == PathAction::Walk
        && !w.has_door
        && !w.has_breakable
        && !matches!(
            w.traverse,
            TraverseKind::JumpDown(_)
                | TraverseKind::Jump
                | TraverseKind::LadderUp
                | TraverseKind::LadderDown
        )
}

fn segment_intersects_aabb(a: Vec3, b: Vec3, mn: Vec3, mx: Vec3) -> bool {
    let dir = b - a;
    let mut tmin = 0.0f32;
    let mut tmax = 1.0f32;
    for axis in 0..3 {
        let o = a[axis];
        let d = dir[axis];
        let lo = mn[axis] - 16.0;
        let hi = mx[axis] + 16.0;
        if d.abs() < 1e-6 {
            if o < lo || o > hi {
                return false;
            }
        } else {
            let inv = 1.0 / d;
            let mut t1 = (lo - o) * inv;
            let mut t2 = (hi - o) * inv;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            tmin = tmin.max(t1);
            tmax = tmax.min(t2);
            if tmin > tmax {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_assets::nav::{NAV_VERSION, NavArea, NavFile, NavLadder};

    fn area_rect(id: u32, min_x: f32, min_y: f32, max_x: f32, max_y: f32, z: f32) -> NavArea {
        NavArea {
            id,
            attributes: NavAttribute::NONE,
            nw_corner: Vec3::new(min_x, min_y, z),
            se_corner: Vec3::new(max_x, max_y, z),
            ne_z: z,
            sw_z: z,
            connections: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            hiding_spots: Vec::new(),
            approach_spots: Vec::new(),
            encounters: Vec::new(),
            place_id: 0,
            ladders_up: Vec::new(),
            ladders_down: Vec::new(),
        }
    }

    #[test]
    fn astar_avoids_danger_and_obstacles_and_tags_metadata() {
        // Two routes from area 1 (west) to area 4 (east):
        // North branch: 1 -> 2 (danger/door) -> 4
        // South branch: 1 -> 3 (crouch) -> 4
        let mut a1 = area_rect(1, 0.0, 100.0, 100.0, 200.0, 0.0);
        let mut a2 = area_rect(2, 100.0, 0.0, 200.0, 100.0, 0.0);
        let mut a3 = area_rect(3, 100.0, 200.0, 200.0, 300.0, 0.0);
        let mut a4 = area_rect(4, 200.0, 100.0, 300.0, 200.0, 0.0);

        a1.connections[NavDir::North.as_index()] = vec![2];
        a1.connections[NavDir::South.as_index()] = vec![3];
        a2.connections[NavDir::South.as_index()] = vec![1];
        a2.connections[NavDir::East.as_index()] = vec![4];
        a3.connections[NavDir::North.as_index()] = vec![1];
        a3.connections[NavDir::East.as_index()] = vec![4];
        a4.connections[NavDir::West.as_index()] = vec![2, 3];

        let mut mesh = NavMesh::from_nav_file(NavFile {
            version: NAV_VERSION,
            bsp_size: 1,
            places: Vec::new(),
            areas: vec![a1, a2, a3, a4],
            ladders: Vec::new(),
        });

        let start = Vec3::new(50.0, 150.0, 0.0);
        let goal = Vec3::new(250.0, 150.0, 0.0);

        // With high danger on area 2 (idx 1), safest route picks area 3 (idx 2).
        mesh.increase_danger(1, 0, 5.0, 10.0);
        let safe_params = PathCostParams {
            team: 0,
            now: 10.0,
            danger_weight: 500.0,
            ..Default::default()
        };
        let safe_path = find_path(&mesh, start, goal, &safe_params, &PathObstacles::default())
            .expect("safe path");
        assert!(safe_path.reached_goal);
        assert!(safe_path.waypoints.iter().any(|w| w.area_id == 3));
        assert!(!safe_path.waypoints.iter().any(|w| w.area_id == 2));

        // Now mark area 3 as CROUCH and add a door on branch 2 -> checks CrouchRequired and has_door
        let mut a3_crouch = mesh.areas()[2].clone();
        a3_crouch.attributes = NavAttribute::CROUCH;
        let mut file = mesh.to_nav_file();
        file.areas[2] = a3_crouch;
        let crouch_mesh = NavMesh::from_nav_file(file);
        let crouch_path = find_path(
            &crouch_mesh,
            start,
            goal,
            &PathCostParams {
                crouch_penalty: 0.0,
                ..Default::default()
            },
            &PathObstacles {
                doors: vec![(Vec3::new(140.0, 40.0, 0.0), Vec3::new(160.0, 60.0, 72.0))],
                breakables: Vec::new(),
            },
        )
        .expect("crouch path");
        assert!(
            crouch_path
                .waypoints
                .iter()
                .any(|w| w.action == PathAction::CrouchRequired)
        );
    }

    #[test]
    fn astar_ladder_and_string_pull() {
        let mut a1 = area_rect(1, 0.0, 0.0, 100.0, 100.0, 0.0);
        a1.connections[NavDir::East.as_index()] = vec![2];
        let mut a2 = area_rect(2, 100.0, 0.0, 200.0, 100.0, 0.0);
        a2.connections[NavDir::West.as_index()] = vec![1];
        a2.ladders_up = vec![1];

        let mut a3 = area_rect(3, 200.0, 0.0, 300.0, 100.0, 160.0);
        a3.ladders_down = vec![1];

        let ladder = NavLadder {
            id: 1,
            top: Vec3::new(200.0, 50.0, 160.0),
            bottom: Vec3::new(190.0, 50.0, 0.0),
            width: 32.0,
            length: 160.0,
            dir: NavDir::West,
            top_forward_area_id: Some(3),
            top_left_area_id: None,
            top_right_area_id: None,
            top_behind_area_id: None,
            bottom_area_id: Some(2),
            is_dangling: false,
        };

        let mesh = NavMesh::from_nav_file(NavFile {
            version: NAV_VERSION,
            bsp_size: 1,
            places: Vec::new(),
            areas: vec![a1, a2, a3],
            ladders: vec![ladder],
        });

        let mut path = find_path_simple(
            &mesh,
            Vec3::new(20.0, 50.0, 0.0),
            Vec3::new(280.0, 50.0, 160.0),
        )
        .expect("ladder path");
        assert!(path.reached_goal);
        assert!(path.waypoints.iter().any(|w| matches!(
            w.action,
            PathAction::LadderTraverse {
                ladder_id: 1,
                ascending: true,
                ..
            }
        )));

        // Test string-pulling on flat chain 1 -> 2 -> 4 (no obstacles)
        let mut f1 = area_rect(1, 0.0, 0.0, 100.0, 100.0, 0.0);
        let mut f2 = area_rect(2, 100.0, 0.0, 200.0, 100.0, 0.0);
        let mut f3 = area_rect(3, 200.0, 0.0, 300.0, 100.0, 0.0);
        f1.connections[NavDir::East.as_index()] = vec![2];
        f2.connections[NavDir::West.as_index()] = vec![1];
        f2.connections[NavDir::East.as_index()] = vec![3];
        f3.connections[NavDir::West.as_index()] = vec![2];
        let flat = NavMesh::from_nav_file(NavFile {
            version: NAV_VERSION,
            bsp_size: 1,
            places: Vec::new(),
            areas: vec![f1, f2, f3],
            ladders: Vec::new(),
        });
        let mut flat_path = find_path_simple(
            &flat,
            Vec3::new(10.0, 50.0, 0.0),
            Vec3::new(290.0, 50.0, 0.0),
        )
        .expect("flat path");
        let before = flat_path.len();
        string_pull_nav_corridor(&flat, &mut flat_path);
        assert_eq!(flat_path.len(), 2);
        assert!(flat_path.len() < before);
        // Ensure ladder node is preserved under string pulling
        string_pull_with_los(&mut path, |_, _| true);
        assert!(path.waypoints.iter().any(|w| matches!(
            w.action,
            PathAction::LadderTraverse {
                ladder_id: 1,
                ascending: true,
                ..
            }
        )));
    }

    #[test]
    fn path_scratch_reuse_and_generation_wraparound_match_fresh_find_path() {
        let mut a1 = area_rect(1, 0.0, 100.0, 100.0, 200.0, 0.0);
        let mut a2 = area_rect(2, 100.0, 0.0, 200.0, 100.0, 0.0);
        let mut a3 = area_rect(3, 100.0, 200.0, 200.0, 300.0, 0.0);
        let mut a4 = area_rect(4, 200.0, 100.0, 300.0, 200.0, 0.0);
        let a5_isolated = area_rect(5, 500.0, 500.0, 600.0, 600.0, 0.0);

        a1.connections[NavDir::North.as_index()] = vec![2];
        a1.connections[NavDir::South.as_index()] = vec![3];
        a2.connections[NavDir::South.as_index()] = vec![1];
        a2.connections[NavDir::East.as_index()] = vec![4];
        a3.connections[NavDir::North.as_index()] = vec![1];
        a3.connections[NavDir::East.as_index()] = vec![4];
        a4.connections[NavDir::West.as_index()] = vec![2, 3];

        let mesh = NavMesh::from_nav_file(NavFile {
            version: NAV_VERSION,
            bsp_size: 1,
            places: Vec::new(),
            areas: vec![a1, a2, a3, a4, a5_isolated],
            ladders: Vec::new(),
        });

        let mut scratch = PathScratch::new();
        let params = PathCostParams::default();
        let obs = PathObstacles::default();
        let queries = [
            (Vec3::new(50.0, 150.0, 0.0), Vec3::new(250.0, 150.0, 0.0)),
            (Vec3::new(250.0, 150.0, 0.0), Vec3::new(50.0, 150.0, 0.0)),
            (Vec3::new(50.0, 150.0, 0.0), Vec3::new(550.0, 550.0, 0.0)), // partial unreachable
            (Vec3::new(150.0, 50.0, 0.0), Vec3::new(150.0, 250.0, 0.0)),
        ];

        for &(s, g) in &queries {
            let fresh = find_path(&mesh, s, g, &params, &obs);
            let reused = find_path_with_scratch(&mesh, s, g, &params, &obs, &mut scratch);
            assert_eq!(reused, fresh);
        }

        // Force generation counter wraparound (`u32::MAX` -> `0` -> reset + `1`) and verify exact parity.
        scratch.generation = u32::MAX;
        for &(s, g) in &queries {
            let fresh = find_path(&mesh, s, g, &params, &obs);
            let reused = find_path_with_scratch(&mesh, s, g, &params, &obs, &mut scratch);
            assert_eq!(reused, fresh);
        }
    }
}
