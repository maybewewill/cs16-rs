//! Automatic BSP walkable navigation graph and CS 1.6 bot AI (patrol, combat, buy, plant/defuse C4).

use std::collections::{HashMap, VecDeque};

use cs_assets::bsp::SpawnTeam;
use glam::Vec3;

use crate::grenades::{SmokeCloud, segment_blocked_by_smoke};
use crate::pmove::snap_spawn_to_floor;
use crate::trace::{BspCollision, CONTENTS_SOLID, Hull, TraceResult};

const NAV_STEP: f32 = 64.0;
const MAX_NAV_NODES: usize = 6400;

/// Minimal collision/visibility interface needed to build a [`NavGraph`] and run [`think_bot_with`].
pub trait BotWorld {
    fn snap_to_floor(&self, seed: Vec3) -> Vec3;
    fn is_solid_standing(&self, pos: Vec3) -> bool;
    fn trace_standing(&self, start: Vec3, end: Vec3) -> TraceResult;
    fn has_line_of_sight(&self, start: Vec3, end: Vec3) -> bool;
}

impl BotWorld for BspCollision {
    fn snap_to_floor(&self, seed: Vec3) -> Vec3 {
        snap_spawn_to_floor(seed, self)
    }

    fn is_solid_standing(&self, pos: Vec3) -> bool {
        self.point_contents(pos, Hull::Standing) == CONTENTS_SOLID
    }

    fn trace_standing(&self, start: Vec3, end: Vec3) -> TraceResult {
        self.trace_hull(start, end, Hull::Standing)
    }

    fn has_line_of_sight(&self, start: Vec3, end: Vec3) -> bool {
        self.has_line_of_sight(start, end)
    }
}

#[derive(Debug, Clone)]
pub struct NavNode {
    pub pos: Vec3,
    pub neighbors: Vec<usize>,
}

/// Lightweight waypoint graph automatically flood-filled from spawns & bomb sites on ANY BSP map.
#[derive(Debug, Clone, Default)]
pub struct NavGraph {
    pub nodes: Vec<NavNode>,
}

impl NavGraph {
    pub fn build(collision: &BspCollision, seeds: &[Vec3]) -> Self {
        Self::build_with(collision, seeds)
    }

    pub fn build_with<W: BotWorld + ?Sized>(world: &W, seeds: &[Vec3]) -> Self {
        let mut nodes: Vec<NavNode> = Vec::new();
        let mut grid: HashMap<(i32, i32, i32), usize> = HashMap::new();
        let mut queue: VecDeque<usize> = VecDeque::new();

        let quantize = |p: Vec3| -> (i32, i32, i32) {
            (
                (p.x / NAV_STEP).round() as i32,
                (p.y / NAV_STEP).round() as i32,
                (p.z / 72.0).round() as i32,
            )
        };

        for &seed in seeds {
            let snapped = world.snap_to_floor(seed);
            if world.is_solid_standing(snapped) {
                continue;
            }
            let key = quantize(snapped);
            if let std::collections::hash_map::Entry::Vacant(e) = grid.entry(key) {
                let idx = nodes.len();
                nodes.push(NavNode {
                    pos: snapped,
                    neighbors: Vec::new(),
                });
                e.insert(idx);
                queue.push_back(idx);
            }
        }

        let dirs: [(f32, f32); 8] = [
            (NAV_STEP, 0.0),
            (-NAV_STEP, 0.0),
            (0.0, NAV_STEP),
            (0.0, -NAV_STEP),
            (NAV_STEP, NAV_STEP),
            (NAV_STEP, -NAV_STEP),
            (-NAV_STEP, NAV_STEP),
            (-NAV_STEP, -NAV_STEP),
        ];

        while let Some(curr_idx) = queue.pop_front() {
            if nodes.len() >= MAX_NAV_NODES {
                break;
            }
            let curr_pos = nodes[curr_idx].pos;

            for (dx, dy) in dirs {
                let target_xy = Vec3::new(curr_pos.x + dx, curr_pos.y + dy, curr_pos.z);
                // First try a flat doorway-safe trace (+2u) with full drop check; if blocked by
                // stairs/curbs, simulate 3 GoldSrc micro-steps (+18u up per 1/3 step).
                let flat_tr = world.trace_standing(
                    curr_pos + Vec3::new(0.0, 0.0, 2.0),
                    target_xy + Vec3::new(0.0, 0.0, 2.0),
                );
                let next_pos = if !flat_tr.start_solid && flat_tr.fraction >= 0.92 {
                    let tr_d = world.trace_standing(
                        flat_tr.end_pos,
                        flat_tr.end_pos - Vec3::new(0.0, 0.0, 96.0),
                    );
                    if tr_d.all_solid || tr_d.fraction >= 1.0 || tr_d.plane_normal.z < 0.7 {
                        continue;
                    }
                    tr_d.end_pos + Vec3::new(0.0, 0.0, 0.5)
                } else {
                    let mut p = curr_pos;
                    let mut ok = true;
                    let sx = dx / 4.0;
                    let sy = dy / 4.0;
                    for _ in 0..4 {
                        let up = world.trace_standing(p, p + Vec3::new(0.0, 0.0, 18.0));
                        let end_h = up.end_pos + Vec3::new(sx, sy, 0.0);
                        let tr_h = world.trace_standing(up.end_pos, end_h);
                        if tr_h.start_solid || tr_h.fraction < 0.92 {
                            ok = false;
                            break;
                        }
                        let tr_d = world.trace_standing(
                            tr_h.end_pos,
                            tr_h.end_pos - Vec3::new(0.0, 0.0, 36.0),
                        );
                        if tr_d.all_solid || tr_d.fraction >= 1.0 || tr_d.plane_normal.z < 0.7 {
                            ok = false;
                            break;
                        }
                        p = tr_d.end_pos + Vec3::new(0.0, 0.0, 0.5);
                    }
                    if !ok {
                        continue;
                    }
                    p
                };
                if next_pos.z - curr_pos.z > 44.0 || curr_pos.z - next_pos.z > 68.0 {
                    continue;
                }

                let key = quantize(next_pos);
                let next_idx = if let Some(&existing) = grid.get(&key) {
                    existing
                } else {
                    if nodes.len() >= MAX_NAV_NODES {
                        continue;
                    }
                    let idx = nodes.len();
                    nodes.push(NavNode {
                        pos: next_pos,
                        neighbors: Vec::new(),
                    });
                    grid.insert(key, idx);
                    queue.push_back(idx);
                    idx
                };

                if next_idx != curr_idx && !nodes[curr_idx].neighbors.contains(&next_idx) {
                    nodes[curr_idx].neighbors.push(next_idx);
                }
            }
        }

        Self { nodes }
    }

    pub fn nearest(&self, pos: Vec3) -> Option<usize> {
        self.nodes
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                a.pos
                    .distance_squared(pos)
                    .total_cmp(&b.pos.distance_squared(pos))
            })
            .map(|(i, _)| i)
    }

    /// Nearest node to `target` reachable from `start` along the nav graph.
    pub fn nearest_reachable(&self, start: usize, target: Vec3) -> Option<usize> {
        if start >= self.nodes.len() {
            return None;
        }
        let mut visited = vec![false; self.nodes.len()];
        let mut q = VecDeque::new();
        visited[start] = true;
        q.push_back(start);
        let mut best = (start, self.nodes[start].pos.distance_squared(target));

        while let Some(u) = q.pop_front() {
            let d2 = self.nodes[u].pos.distance_squared(target);
            if d2 < best.1 {
                best = (u, d2);
            }
            for &v in &self.nodes[u].neighbors {
                if !visited[v] {
                    visited[v] = true;
                    q.push_back(v);
                }
            }
        }
        Some(best.0)
    }

    /// Shortest path via BFS from `start_node` to `goal_node` (returns next waypoint node index).
    pub fn next_step_towards(&self, start: usize, goal: usize) -> Option<usize> {
        if start == goal || start >= self.nodes.len() || goal >= self.nodes.len() {
            return None;
        }
        let mut prev = vec![usize::MAX; self.nodes.len()];
        let mut q = VecDeque::new();
        prev[start] = start;
        q.push_back(start);

        while let Some(u) = q.pop_front() {
            if u == goal {
                let mut cur = goal;
                while prev[cur] != start && prev[cur] != usize::MAX {
                    cur = prev[cur];
                }
                return Some(cur);
            }
            for &v in &self.nodes[u].neighbors {
                if prev[v] == usize::MAX {
                    prev[v] = u;
                    q.push_back(v);
                }
            }
        }
        None
    }
}

#[derive(Debug, Clone, Default)]
pub struct BotBrain {
    pub goal_node: Option<usize>,
    pub repath_timer: f32,
    pub strafe_dir: f32,
    pub strafe_timer: f32,
    pub reaction_timer: f32,
    pub burst_timer: f32,
    pub last_objective: Option<Vec3>,
}

#[derive(Debug, Clone, Copy)]
pub struct BotObservation {
    pub self_id: u32,
    pub team: SpawnTeam,
    pub pos: Vec3,
    pub eye: Vec3,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub has_c4: bool,
    pub in_bomb_zone: bool,
    pub clip: u16,
    pub reserve: u16,
    pub flash_alpha: f32,
    pub bomb_planted_pos: Option<Vec3>,
    pub bomb_targets: [Option<Vec3>; 2],
    pub priority_goal: Option<Vec3>,
    pub difficulty: u8,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BotDecision {
    pub forward: f32,
    pub right: f32,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub jump: bool,
    pub duck: bool,
    pub attack: bool,
    pub use_key: bool,
    pub reload: bool,
    pub want_c4: bool,
}

pub const CLASSIC_BOT_NAMES: &[&str] = &[
    "Cliffe", "Minh", "Vinnie", "Yogi", "Gabe", "Steel", "Stone", "Crusher", "Gunner", "Phoenix",
    "Viper", "Wolf", "Ghost", "Hawk", "Tank", "Spike", "Rico", "Toby", "Zane", "Boris",
];

#[allow(clippy::too_many_arguments)]
pub fn think_bot(
    brain: &mut BotBrain,
    obs: &BotObservation,
    enemies: &[(u32, Vec3, Vec3)], // (id, origin, eye)
    nav: &NavGraph,
    collision: &BspCollision,
    smokes: &[SmokeCloud],
    rng_seed: u64,
    dt: f32,
) -> BotDecision {
    think_bot_with(brain, obs, enemies, nav, collision, smokes, rng_seed, dt)
}

#[allow(clippy::too_many_arguments)]
pub fn think_bot_with<W: BotWorld + ?Sized>(
    brain: &mut BotBrain,
    obs: &BotObservation,
    enemies: &[(u32, Vec3, Vec3)], // (id, origin, eye)
    nav: &NavGraph,
    world: &W,
    smokes: &[SmokeCloud],
    rng_seed: u64,
    dt: f32,
) -> BotDecision {
    let mut dec = BotDecision {
        yaw_deg: obs.yaw_deg,
        pitch_deg: obs.pitch_deg,
        ..BotDecision::default()
    };

    // Reload if clip empty and reserve available
    if obs.clip == 0 && obs.reserve > 0 {
        dec.reload = true;
    }

    // 1. Scan for nearest visible enemy (unless heavily flashed or blocked by smoke)
    let mut target_enemy: Option<(Vec3, f32)> = None;
    if obs.flash_alpha < 0.55 {
        for &(_, e_origin, e_eye) in enemies {
            let aim_pt = e_origin.lerp(e_eye, 0.72);
            let dist = obs.eye.distance(aim_pt);
            if dist > 2600.0 {
                continue;
            }
            if !world.has_line_of_sight(obs.eye, aim_pt) {
                continue;
            }
            if segment_blocked_by_smoke(obs.eye, aim_pt, smokes) {
                continue;
            }
            if target_enemy.is_none_or(|(_, bd)| dist < bd) {
                target_enemy = Some((aim_pt, dist));
            }
        }
    }

    // 2. If enemy visible -> engage!
    if let Some((aim_pt, dist)) = target_enemy {
        let delta = aim_pt - obs.eye;
        let horiz = (delta.x * delta.x + delta.y * delta.y).sqrt().max(0.001);
        let desired_yaw = delta.y.atan2(delta.x).to_degrees().rem_euclid(360.0);
        let desired_pitch = (-delta.z.atan2(horiz).to_degrees()).clamp(-85.0, 85.0);

        let turn_rate = match obs.difficulty {
            0 => 220.0,
            1 => 340.0,
            2 => 480.0,
            _ => 680.0,
        };
        dec.yaw_deg = step_angle(obs.yaw_deg, desired_yaw, turn_rate * dt);
        dec.pitch_deg = step_angle(obs.pitch_deg, desired_pitch, turn_rate * dt);

        let yaw_err = angle_diff(dec.yaw_deg, desired_yaw).abs();
        let pitch_err = (dec.pitch_deg - desired_pitch).abs();

        brain.strafe_timer -= dt;
        if brain.strafe_timer <= 0.0 {
            brain.strafe_timer = 0.45 + ((rng_seed % 50) as f32) * 0.01;
            brain.strafe_dir = match rng_seed % 3 {
                0 => -1.0,
                1 => 1.0,
                _ => 0.0,
            };
        }

        // Engage with CS 1.6 burst/tap discipline
        if yaw_err < 12.0 && pitch_err < 12.0 {
            brain.reaction_timer += dt;
            let react_delay = match obs.difficulty {
                0 => 0.35,
                1 => 0.22,
                2 => 0.12,
                _ => 0.06,
            };
            if brain.reaction_timer >= react_delay {
                brain.burst_timer = (brain.burst_timer + dt) % 0.65;
                dec.attack = brain.burst_timer < 0.32 || dist < 350.0;
                // Stop or crouch briefly while shooting for accuracy
                if dec.attack && dist > 450.0 {
                    dec.forward = 0.0;
                    dec.right = 0.0;
                    dec.duck = obs.difficulty >= 2 && dist > 750.0;
                } else {
                    dec.right = brain.strafe_dir * 0.7;
                    dec.forward = if dist > 500.0 { 0.4 } else if dist < 160.0 { -0.5 } else { 0.0 };
                }
            }
        } else {
            dec.forward = 0.6;
        }
        return dec;
    }

    brain.reaction_timer = 0.0;

    // 3. Objective logic: plant C4 (Terrorist with bomb) or defuse C4 (CT near planted bomb)
    if obs.team == SpawnTeam::Terrorist && obs.has_c4 && obs.in_bomb_zone && obs.bomb_planted_pos.is_none() {
        dec.want_c4 = true;
        dec.attack = true;
        dec.duck = true;
        return dec;
    }

    if obs.team == SpawnTeam::CounterTerrorist
        && let Some(bomb_pos) = obs.bomb_planted_pos
    {
        let dist = obs.pos.distance(bomb_pos);
        if dist < 52.0 {
            let d = bomb_pos - obs.eye;
            let h = (d.x * d.x + d.y * d.y).sqrt().max(0.001);
            dec.yaw_deg = step_angle(obs.yaw_deg, d.y.atan2(d.x).to_degrees().rem_euclid(360.0), 720.0 * dt);
            dec.pitch_deg = step_angle(obs.pitch_deg, (-d.z.atan2(h).to_degrees()).clamp(-80.0, 80.0), 720.0 * dt);
            dec.use_key = true;
            dec.duck = true;
            return dec;
        }
        if dist < 96.0 && world.has_line_of_sight(obs.eye, bomb_pos) {
            let d = bomb_pos - obs.eye;
            let h = (d.x * d.x + d.y * d.y).sqrt().max(0.001);
            dec.yaw_deg = step_angle(obs.yaw_deg, d.y.atan2(d.x).to_degrees().rem_euclid(360.0), 540.0 * dt);
            dec.pitch_deg = step_angle(obs.pitch_deg, (-d.z.atan2(h).to_degrees()).clamp(-80.0, 80.0), 540.0 * dt);
            dec.forward = 0.7;
            return dec;
        }
    }

    // 4. Waypoint navigation on the BSP NavGraph
    if nav.nodes.is_empty() {
        return dec;
    }

    brain.repath_timer -= dt;
    let curr_node = nav.nearest(obs.pos);

    let objective_pos = if let Some(bpos) = obs.bomb_planted_pos {
        Some(bpos)
    } else if let Some(pgoal) = obs.priority_goal {
        Some(pgoal)
    } else if (obs.team == SpawnTeam::Terrorist && obs.has_c4) || !rng_seed.is_multiple_of(3) {
        let site_idx = (rng_seed as usize) % 2;
        obs.bomb_targets[site_idx].or(obs.bomb_targets[0])
    } else {
        None
    };

    let objective_shifted = match (objective_pos, brain.last_objective) {
        (Some(a), Some(b)) => a.distance_squared(b) > 64.0 * 64.0,
        (Some(_), None) => true,
        _ => false,
    };

    let c4_goal_valid = obs.team == SpawnTeam::Terrorist
        && obs.has_c4
        && brain.goal_node.is_some()
        && obs.bomb_planted_pos.is_none()
        && !objective_shifted;

    let need_new_goal = objective_shifted
        || if c4_goal_valid {
            brain.goal_node.is_some_and(|g| {
                curr_node.is_some_and(|c| c != g && nav.next_step_towards(c, g).is_none())
            })
        } else {
            brain.goal_node.is_none()
                || brain.repath_timer <= 0.0
                || brain
                    .goal_node
                    .and_then(|g| nav.nodes.get(g))
                    .is_some_and(|n| n.pos.distance(obs.pos) < 88.0)
        };

    if need_new_goal {
        brain.last_objective = objective_pos;
        brain.repath_timer = 4.0 + ((rng_seed % 30) as f32) * 0.1;
        brain.goal_node = objective_pos
            .and_then(|p| {
                curr_node
                    .and_then(|c| nav.nearest_reachable(c, p))
                    .or_else(|| nav.nearest(p))
            })
            .or_else(|| Some((rng_seed as usize) % nav.nodes.len()));
    }

    if let Some(p) = objective_pos
        && (obs.priority_goal.is_some()
            || (obs.team == SpawnTeam::Terrorist && obs.has_c4 && !obs.in_bomb_zone))
        && obs.pos.distance(p) < 220.0
        && curr_node.is_some_and(|c| {
            brain.goal_node == Some(c)
                || brain
                    .goal_node
                    .and_then(|g| nav.nodes.get(g))
                    .is_some_and(|n| n.pos.distance(obs.pos) < 88.0)
        })
        && world.has_line_of_sight(obs.eye, p + Vec3::new(0.0, 0.0, 24.0))
    {
        let delta = p - obs.pos;
        if delta.x * delta.x + delta.y * delta.y > 4.0 {
            let desired_yaw = delta.y.atan2(delta.x).to_degrees().rem_euclid(360.0);
            dec.yaw_deg = step_angle(obs.yaw_deg, desired_yaw, 360.0 * dt);
            dec.pitch_deg = step_angle(obs.pitch_deg, 0.0, 180.0 * dt);
            dec.forward = 1.0;
        }
        return dec;
    }

    if let (Some(c), Some(g)) = (curr_node, brain.goal_node) {
        let next_wp = nav
            .next_step_towards(c, g)
            .and_then(|idx| nav.nodes.get(idx))
            .or_else(|| nav.nodes.get(g));

        if let Some(wp) = next_wp {
            let delta = wp.pos - obs.pos;
            let desired_yaw = delta.y.atan2(delta.x).to_degrees().rem_euclid(360.0);
            dec.yaw_deg = step_angle(obs.yaw_deg, desired_yaw, 280.0 * dt);
            dec.pitch_deg = step_angle(obs.pitch_deg, 0.0, 180.0 * dt);
            dec.forward = 1.0;
        }
    }

    dec
}

fn angle_diff(from: f32, to: f32) -> f32 {
    let mut d = (to - from).rem_euclid(360.0);
    if d > 180.0 {
        d -= 360.0;
    }
    d
}

fn step_angle(current: f32, target: f32, max_delta: f32) -> f32 {
    let d = angle_diff(current, target);
    if d.abs() <= max_delta {
        target
    } else {
        (current + d.signum() * max_delta).rem_euclid(360.0)
    }
}
