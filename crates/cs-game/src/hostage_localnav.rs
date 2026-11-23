//! Local obstacle-avoidance pathfinder for classic hostages (`CLocalNav`, ReGameDLL_CS
//! `hostage/hostage_localnav.h`, `hostage/hostage_localnav.cpp`, read 2026-11-23).
//!
//! Classic CS 1.6 uses this per-hostage 26-unit 2D-lattice best-first search with hull traces
//! (`TRACE_MONSTER_HULL`) rather than the Condition Zero `.nav` mesh (`CHostageImprov`). A global
//! CPU budget (`node_value`) drains at `250/s` and gates requests once it exceeds `17`, serving at
//! most one queued hostage per frame from `StartFrame`.

use std::f32::consts::PI;

use cs_engine::{EdictId, EngineApi, EntTrace, MoveFilter};
use glam::Vec3;

use crate::GameState;
use crate::bullets::normalize_hl;
use crate::entity::with_entity;
use crate::intro::vec_to_angles;

/// `MAX_NODES` (`hostage.h`).
pub const MAX_NODES: usize = 100;
/// `MAX_HOSTAGES_NAV` (`hostage.h`).
pub(crate) const MAX_HOSTAGES_NAV: usize = 20;
/// `HOSTAGE_STEPSIZE` (`hostage.h`): grid spacing in world units.
pub(crate) const HOSTAGE_STEPSIZE: f32 = 26.0;
/// `MaxUnitZSlope` (`nav.h`): minimum floor `plane_normal.z` considered walkable.
pub(crate) const MAX_UNIT_Z_SLOPE: f32 = 0.7;
/// `CONTENTS_LADDER` (`const.h`).
const CONTENTS_LADDER: i32 = -16;

/// `PathTraversAble` (`hostage_localnav.h`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathTraversable {
    Empty,
    Slope,
    Step,
    StepJumpable,
}

#[derive(Debug, Clone, Copy)]
struct LocalNode {
    loc: Vec3,
    offset_x: i32,
    offset_y: i32,
    depth: u8,
    searched: bool,
    parent: Option<usize>,
}

/// Global `CLocalNav` state stored on [`GameState`]: step-size cache, node-budget throttle,
/// FIFO request queue of up to [`MAX_HOSTAGES_NAV`], and registered hostage list for `PreThink`.
#[derive(Debug, Clone)]
pub(crate) struct LocalNavManager {
    pub(crate) step_size: f32,
    next_cvar_check: f64,
    last_think_time: f64,
    pub(crate) node_value: i32,
    queue: [Option<EdictId>; MAX_HOSTAGES_NAV],
    cur_request: usize,
    num_request: usize,
    hostages: Vec<EdictId>,
}

impl Default for LocalNavManager {
    fn default() -> Self {
        Self {
            step_size: 18.0,
            next_cvar_check: 0.0,
            last_think_time: 0.0,
            node_value: 0,
            queue: [None; MAX_HOSTAGES_NAV],
            cur_request: 0,
            num_request: 0,
            hostages: Vec::new(),
        }
    }
}

impl LocalNavManager {
    /// `CLocalNav::Reset`: clears queue, registered hostages, and budget on map deactivation.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn register_hostage(&mut self, id: EdictId) {
        if self.hostages.len() < MAX_HOSTAGES_NAV && !self.hostages.contains(&id) {
            self.hostages.push(id);
        }
    }

    fn charge_nodes(&mut self, used: usize) {
        let cost = match used {
            0..=10 => 2,
            11..=20 => 4,
            21..=30 => 8,
            31..=40 => 13,
            41..=50 => 19,
            51..=60 => 26,
            61..=70 => 34,
            71..=80 => 43,
            81..=90 => 53,
            91..=100 => 64,
            101..=110 => 76,
            111..=120 => 89,
            121..=130 => 103,
            131..=140 => 118,
            141..=150 => 134,
            151..=160 => 151,
            _ => 169,
        };
        self.node_value += cost;
    }

    pub(crate) fn can_run_immediate(&self) -> bool {
        self.node_value <= 17 && self.num_request == 0
    }

    pub(crate) fn enqueue_nav(&mut self, caller: EdictId) {
        self.enqueue(caller);
    }

    fn enqueue(&mut self, caller: EdictId) {
        if self.num_request >= MAX_HOSTAGES_NAV {
            return;
        }
        let mut curr = self.cur_request;
        for _ in 0..self.num_request {
            if self.queue[curr] == Some(caller) {
                return;
            }
            curr = (curr + 1) % MAX_HOSTAGES_NAV;
        }
        self.queue[curr] = Some(caller);
        self.num_request += 1;
    }

    fn pop_valid(&mut self, eng: &EngineApi<'_>) -> Option<EdictId> {
        while self.num_request > 0 {
            let slot = self.queue[self.cur_request];
            self.cur_request = (self.cur_request + 1) % MAX_HOSTAGES_NAV;
            self.num_request -= 1;
            if let Some(id) = slot
                && eng.ent(id).is_some()
            {
                return Some(id);
            }
        }
        None
    }
}

/// `m_NodeValue -= flElapsedTime * 250` (`hostage_localnav.cpp` 786-787, read 2026-11-23): an `int`
/// minus a `float`, so the subtraction is done in `float` and the result truncated toward zero
/// when it is stored back (not the drain truncated first).
pub(crate) fn drain_node_value(node_value: i32, elapsed: f32) -> i32 {
    (node_value as f32 - elapsed * 250.0) as i32
}

/// `CLocalNav::Think`: called every server frame from `StartFrame`. Updates `sv_stepsize`, runs
/// `HostagePrethink`, drains the CPU budget at `250/s`, and serves at most one queued nav request.
pub(crate) fn think(g: &mut GameState, eng: &mut EngineApi<'_>) {
    let now = eng.time();
    if now >= g.hostage_nav.next_cvar_check {
        let stepsize = eng.cvar_f32("sv_stepsize");
        if stepsize > 0.0 {
            g.hostage_nav.step_size = stepsize;
        }
        g.hostage_nav.next_cvar_check = now + 1.0;
    }

    let hostages = g.hostage_nav.hostages.clone();
    for id in hostages {
        if eng.ent(id).is_some() {
            with_entity(g, eng, id, |entity, g, eng| {
                entity.hostage_pre_think(g, eng, id);
            });
        }
    }

    let elapsed = now as f32 - g.hostage_nav.last_think_time as f32;
    g.hostage_nav.node_value = drain_node_value(g.hostage_nav.node_value, elapsed);
    g.hostage_nav.last_think_time = now;
    if g.hostage_nav.node_value < 0 {
        g.hostage_nav.node_value = 0;
    } else if g.hostage_nav.node_value > 17 {
        return;
    }

    if let Some(hostage) = g.hostage_nav.pop_valid(eng) {
        with_entity(g, eng, hostage, |entity, g, eng| {
            entity.hostage_nav_ready(g, eng, hostage);
        });
    }
}

/// Per-hostage local navigation state (`CLocalNav` instance).
#[derive(Debug, Clone, Default)]
pub struct LocalNav {
    target_ent: Option<EdictId>,
    target_ent_hit: bool,
    nodes: Vec<LocalNode>,
    starting_loc: Vec3,
}

impl LocalNav {
    pub(crate) fn set_target_ent(&mut self, target: Option<EdictId>) {
        self.target_ent = target;
    }

    fn add_node(
        &mut self,
        parent: Option<usize>,
        loc: Vec3,
        offset_x: i32,
        offset_y: i32,
        depth: u8,
    ) -> Option<usize> {
        if self.nodes.len() >= MAX_NODES {
            return None;
        }
        let idx = self.nodes.len();
        self.nodes.push(LocalNode {
            loc,
            offset_x,
            offset_y,
            depth,
            searched: false,
            parent,
        });
        Some(idx)
    }

    fn node_exists(&self, offset_x: i32, offset_y: i32) -> Option<usize> {
        self.nodes
            .iter()
            .rposition(|n| n.offset_x == offset_x && n.offset_y == offset_y)
    }

    fn add_path_nodes(
        &mut self,
        eng: &EngineApi<'_>,
        hostage: EdictId,
        step_size: f32,
        source: Option<usize>,
        no_monsters: bool,
    ) {
        for (ox, oy) in [
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ] {
            self.add_path_node(eng, hostage, step_size, source, ox, oy, no_monsters);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add_path_node(
        &mut self,
        eng: &EngineApi<'_>,
        hostage: EdictId,
        step_size: f32,
        mut source: Option<usize>,
        offset_x: i32,
        offset_y: i32,
        no_monsters: bool,
    ) {
        let (offset_x_abs, offset_y_abs, vec_source, mut vec_dest, depth);
        if let Some(src_idx) = source {
            let src_node = self.nodes[src_idx];
            offset_x_abs = offset_x + src_node.offset_x;
            offset_y_abs = offset_y + src_node.offset_y;
            if self.node_exists(offset_x_abs, offset_y_abs).is_some() {
                return;
            }
            let base_loc = src_node.loc;
            vec_dest = base_loc
                + Vec3::new(
                    offset_x as f32 * HOSTAGE_STEPSIZE,
                    offset_y as f32 * HOSTAGE_STEPSIZE,
                    0.0,
                );
            let mut best_loc = base_loc;
            let mut best_depth = src_node.depth;
            for nindex_current in (0..self.nodes.len()).rev() {
                let candidate = self.nodes[nindex_current];
                if (candidate.offset_x - offset_x_abs).abs() > 1
                    || (candidate.offset_y - offset_y_abs).abs() > 1
                {
                    continue;
                }
                if self.path_traversable(eng, hostage, step_size, candidate.loc, &mut vec_dest, no_monsters)
                    != PathTraversable::Empty
                {
                    best_loc = candidate.loc;
                    best_depth = candidate.depth;
                    source = Some(nindex_current);
                }
            }
            vec_source = best_loc;
            depth = best_depth.wrapping_add(1);
        } else {
            depth = 1;
            offset_x_abs = offset_x;
            offset_y_abs = offset_y;
            vec_source = self.starting_loc;
            vec_dest = vec_source
                + Vec3::new(
                    offset_x as f32 * HOSTAGE_STEPSIZE,
                    offset_y as f32 * HOSTAGE_STEPSIZE,
                    0.0,
                );
        }

        if self.path_traversable(eng, hostage, step_size, vec_source, &mut vec_dest, no_monsters)
            != PathTraversable::Empty
        {
            self.add_node(source, vec_dest, offset_x_abs, offset_y_abs, depth);
        }
    }

    fn get_best_node(&self, dest: Vec3, step_size: f32) -> Option<usize> {
        let mut best = None;
        let mut best_val = 1_000_000.0_f32;
        for (idx, node) in self.nodes.iter().enumerate() {
            if node.searched {
                continue;
            }
            let dist_from_start = (dest - node.loc).length();
            let z_diff = (node.loc.z - dest.z).abs();
            let z_weight = if z_diff <= step_size { 1.0 } else { 1.25 };
            let val = z_weight * (f32::from(node.depth) * HOSTAGE_STEPSIZE + dist_from_start);
            if val < best_val {
                best_val = val;
                best = Some(idx);
            }
        }
        best
    }

    /// `CLocalNav::FindPath`: returns the index of the goal node in `self.nodes` if reachable.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn find_path(
        &mut self,
        eng: &EngineApi<'_>,
        hostage: EdictId,
        mgr: &mut LocalNavManager,
        start: Vec3,
        dest: Vec3,
        target_radius: f32,
        no_monsters: bool,
    ) -> Option<usize> {
        let step_size = mgr.step_size;
        if let Some(direct) = self.find_direct_path(eng, hostage, step_size, start, dest, target_radius, no_monsters) {
            return Some(direct);
        }

        self.starting_loc = start;
        self.nodes.clear();
        self.add_path_nodes(eng, hostage, step_size, None, no_monsters);
        let mut best = self.get_best_node(dest, step_size);

        while let Some(idx) = best {
            self.nodes[idx].searched = true;
            let node_loc = self.nodes[idx].loc;
            let dist_to_dest = (dest - node_loc).truncate().length();

            if dist_to_dest <= target_radius || dist_to_dest <= HOSTAGE_STEPSIZE {
                break;
            }

            let remaining_budget = (MAX_NODES - self.nodes.len()) as f32 * HOSTAGE_STEPSIZE;
            if (dist_to_dest - target_radius) > remaining_budget || self.nodes.len() == MAX_NODES {
                best = None;
                break;
            }

            self.add_path_nodes(eng, hostage, step_size, Some(idx), no_monsters);
            best = self.get_best_node(dest, step_size);
        }

        mgr.charge_nodes(self.nodes.len());
        best
    }

    #[allow(clippy::too_many_arguments)]
    fn find_direct_path(
        &mut self,
        eng: &EngineApi<'_>,
        hostage: EdictId,
        step_size: f32,
        start: Vec3,
        dest: Vec3,
        target_radius: f32,
        no_monsters: bool,
    ) -> Option<usize> {
        let path_dir = normalize_hl(dest - start);
        let mut actual_dest = dest - path_dir * target_radius;
        if self.path_traversable(eng, hostage, step_size, start, &mut actual_dest, no_monsters)
            == PathTraversable::Empty
        {
            return None;
        }
        let mut last = None;
        let mut node_loc = start;
        self.nodes.clear();
        while (node_loc - actual_dest).truncate().length() >= HOSTAGE_STEPSIZE {
            node_loc += path_dir * HOSTAGE_STEPSIZE;
            match self.add_node(last, node_loc, 0, 0, 0) {
                Some(idx) => last = Some(idx),
                None => break,
            }
        }
        last
    }

    /// `CLocalNav::SetupPathNodes`: copies the path chain into `out` (index 0 is nearest goal).
    pub(crate) fn setup_path_nodes(&self, last_node: usize, out: &mut [Vec3; MAX_NODES]) -> usize {
        let mut cur = Some(last_node);
        let mut count = 0;
        while let Some(idx) = cur
            && count < MAX_NODES
        {
            let node = self.nodes[idx];
            out[count] = node.loc;
            count += 1;
            cur = node.parent;
        }
        count
    }

    /// `CLocalNav::GetFurthestTraversableNode` (`hostage_localnav.cpp` 283-295, read 2026-11-23):
    /// returns the first node in `nodes[..total]` reachable in a straight traversable step from
    /// `starting_loc`. `PathTraversable` takes its destination by reference, so the node it
    /// settled on (the point where the walk ended) is written back into `nodes`.
    #[allow(clippy::too_many_arguments)]
    pub fn get_furthest_traversable_node(
        &mut self,
        eng: &EngineApi<'_>,
        hostage: EdictId,
        step_size: f32,
        starting_loc: Vec3,
        nodes: &mut [Vec3; MAX_NODES],
        total_nodes: usize,
        no_monsters: bool,
    ) -> Option<usize> {
        for (i, node) in nodes.iter_mut().take(total_nodes).enumerate() {
            if self.path_traversable(eng, hostage, step_size, starting_loc, node, no_monsters)
                != PathTraversable::Empty
            {
                return Some(i);
            }
        }
        None
    }

    fn path_clear(
        &mut self,
        eng: &EngineApi<'_>,
        hostage: EdictId,
        origin: Vec3,
        dest: Vec3,
        no_monsters: bool,
    ) -> (bool, EntTrace) {
        let filter = if no_monsters {
            MoveFilter::NoMonsters
        } else {
            MoveFilter::Normal
        };
        let tr = eng.trace_monster_hull(hostage, origin, dest, filter, Some(hostage));
        if tr.trace.start_solid {
            return (false, tr);
        }
        if tr.trace.fraction == 1.0 {
            return (true, tr);
        }
        if self.target_ent.is_some() && tr.ent == self.target_ent {
            self.target_ent_hit = true;
            return (true, tr);
        }
        (false, tr)
    }

    /// `CLocalNav::PathTraversable`: steps `step_size` along XY toward `dest`, handling slopes,
    /// stairs up to `step_size`, and jumps up to 40 units while rejecting drops > 300 units.
    pub(crate) fn path_traversable(
        &mut self,
        eng: &EngineApi<'_>,
        hostage: EdictId,
        step_size: f32,
        source: Vec3,
        dest: &mut Vec3,
        no_monsters: bool,
    ) -> PathTraversable {
        let mut src_tmp = source;
        let diff = *dest - source;
        let mut dir = normalize_hl(diff);
        dir.z = 0.0;
        let mut total = diff.truncate().length();
        let mut retval = PathTraversable::Empty;

        while total > 1.0 {
            let mut dest_tmp = if total >= step_size {
                src_tmp + dir * step_size
            } else {
                *dest
            };
            self.target_ent_hit = false;

            let (clear, tr) = self.path_clear(eng, hostage, src_tmp, dest_tmp, no_monsters);
            if clear {
                dest_tmp = tr.trace.end_pos;
                if retval == PathTraversable::Empty {
                    retval = PathTraversable::Slope;
                }
            } else {
                if tr.trace.start_solid {
                    return PathTraversable::Empty;
                }
                if !no_monsters
                    && tr
                        .ent
                        .and_then(|e| eng.ent(e))
                        .is_some_and(|v| v.classname == "hostage_entity")
                {
                    return PathTraversable::Empty;
                }
                src_tmp = tr.trace.end_pos;
                if tr.trace.plane_normal.z <= MAX_UNIT_Z_SLOPE {
                    if self.step_traversable(
                        eng,
                        hostage,
                        step_size,
                        src_tmp,
                        &mut dest_tmp,
                        no_monsters,
                    ) {
                        if retval == PathTraversable::Empty {
                            retval = PathTraversable::Step;
                        }
                    } else if self.step_jumpable(
                        eng,
                        hostage,
                        step_size,
                        src_tmp,
                        &mut dest_tmp,
                        no_monsters,
                    ) {
                        if retval == PathTraversable::Empty {
                            retval = PathTraversable::StepJumpable;
                        }
                    } else {
                        return PathTraversable::Empty;
                    }
                } else if self.slope_traversable(
                    eng,
                    hostage,
                    step_size,
                    src_tmp,
                    &mut dest_tmp,
                    no_monsters,
                    tr.trace.plane_normal,
                ) {
                    if retval == PathTraversable::Empty {
                        retval = PathTraversable::Slope;
                    }
                } else {
                    return PathTraversable::Empty;
                }
            }

            let drop_dest = dest_tmp - Vec3::new(0.0, 0.0, 300.0);
            let (drop_clear, drop_tr) =
                self.path_clear(eng, hostage, dest_tmp, drop_dest, no_monsters);
            if drop_clear {
                return PathTraversable::Empty;
            }
            if !drop_tr.trace.start_solid {
                dest_tmp = drop_tr.trace.end_pos;
            }
            src_tmp = dest_tmp;
            if self.target_ent_hit {
                break;
            }
            total = (*dest - dest_tmp).truncate().length();
        }

        *dest = src_tmp;
        retval
    }

    #[allow(clippy::too_many_arguments)]
    fn slope_traversable(
        &mut self,
        eng: &EngineApi<'_>,
        hostage: EdictId,
        step_size: f32,
        source: Vec3,
        dest: &mut Vec3,
        no_monsters: bool,
        plane_normal: Vec3,
    ) -> bool {
        let down = *dest - source;
        let angles = vec_to_angles(plane_normal);
        let mut slope_end = *dest;
        slope_end.z =
            down.truncate().length() * ((90.0 - angles.x) * (PI / 180.0)).tan() + source.z;

        let (clear, tr) = self.path_clear(eng, hostage, source, slope_end, no_monsters);
        if !clear {
            if tr.trace.start_solid {
                return false;
            }
            if (tr.trace.end_pos - source).truncate().length() < 1.0 {
                return false;
            }
        }
        slope_end = tr.trace.end_pos;
        let mut drop_down = slope_end;
        drop_down.z -= step_size;
        let (down_clear, down_tr) = self.path_clear(eng, hostage, slope_end, drop_down, no_monsters);
        if !down_clear && down_tr.trace.start_solid {
            *dest = slope_end;
            return true;
        }
        *dest = down_tr.trace.end_pos;
        true
    }

    #[allow(dead_code)]
    fn ladder_traversable(
        &mut self,
        eng: &EngineApi<'_>,
        hostage: EdictId,
        step_size: f32,
        mut dest: Vec3,
        no_monsters: bool,
        hit_end: Vec3,
    ) -> bool {
        let step_start = hit_end;
        let step_dest = Vec3::new(step_start.x, step_start.y, step_start.z + HOSTAGE_STEPSIZE);
        let (clear, tr) = self.path_clear(eng, hostage, step_start, step_dest, no_monsters);
        if !clear && (tr.trace.start_solid || (tr.trace.end_pos - step_start).length() < 1.0) {
            return false;
        }
        let next_start = tr.trace.end_pos;
        dest.z = next_start.z;
        self.path_traversable(eng, hostage, step_size, next_start, &mut dest, no_monsters)
            != PathTraversable::Empty
    }

    fn step_traversable(
        &mut self,
        eng: &EngineApi<'_>,
        hostage: EdictId,
        step_size: f32,
        source: Vec3,
        dest: &mut Vec3,
        no_monsters: bool,
    ) -> bool {
        let mut step_start = source;
        let mut step_dest = *dest;
        step_start.z += step_size;
        step_dest.z = step_start.z;

        let (clear, tr) = self.path_clear(eng, hostage, step_start, step_dest, no_monsters);
        if !clear {
            if tr.trace.start_solid {
                return false;
            }
            if (tr.trace.end_pos - step_start).length() < 1.0 {
                return false;
            }
        }
        step_start = tr.trace.end_pos;
        step_dest = Vec3::new(step_start.x, step_start.y, step_start.z - step_size);
        let (down_clear, down_tr) =
            self.path_clear(eng, hostage, step_start, step_dest, no_monsters);
        if !down_clear && down_tr.trace.start_solid {
            *dest = step_start;
            return true;
        }
        *dest = down_tr.trace.end_pos;
        true
    }

    fn step_jumpable(
        &mut self,
        eng: &EngineApi<'_>,
        hostage: EdictId,
        step_size: f32,
        source: Vec3,
        dest: &mut Vec3,
        no_monsters: bool,
    ) -> bool {
        let mut jump_height = step_size + 1.0;
        let mut step_start = source;
        step_start.z += jump_height;

        while jump_height < 40.0 {
            let mut step_dest = *dest;
            step_dest.z = step_start.z;
            let (clear, tr) = self.path_clear(eng, hostage, step_start, step_dest, no_monsters);
            if !clear {
                if tr.trace.start_solid {
                    break;
                }
                if (tr.trace.end_pos - step_start).truncate().length() < 1.0 {
                    jump_height += 10.0;
                    step_start.z += 10.0;
                    continue;
                }
            }
            step_start = tr.trace.end_pos;
            step_dest = Vec3::new(step_start.x, step_start.y, step_start.z - step_size);
            let (down_clear, down_tr) =
                self.path_clear(eng, hostage, step_start, step_dest, no_monsters);
            if !down_clear && down_tr.trace.start_solid {
                *dest = step_start;
                return true;
            }
            *dest = down_tr.trace.end_pos;
            return true;
        }
        false
    }

    #[allow(dead_code)]
    fn ladder_hit(&self, eng: &EngineApi<'_>, end_pos: Vec3, plane_normal: Vec3) -> bool {
        let angles = vec_to_angles(-plane_normal);
        let (fwd, right, up) = cs_sim::pm::math::angle_vectors(angles);
        for probe in [
            end_pos + fwd * 15.0 + up * 36.0,
            end_pos + fwd * 15.0 - up * 36.0,
            end_pos + fwd * 15.0 + right * 16.0 + up * 36.0,
            end_pos + fwd * 15.0 - right * 16.0 + up * 36.0,
            end_pos + fwd * 15.0 + right * 16.0 - up * 36.0,
        ] {
            if eng.point_contents_ents(probe) == CONTENTS_LADDER {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::drain_node_value;

    #[test]
    fn the_budget_drain_truncates_the_result_not_the_drain() {
        // 250 * 0.0123 = 3.075: `10 - 3.075 = 6.925` truncates to 6 (draining 3 first gives 7).
        assert_eq!(drain_node_value(10, 0.0123), 6);
        // A negative result truncates toward zero; `Think` clamps it to 0 afterwards.
        assert_eq!(drain_node_value(0, 0.1), -25);
        assert_eq!(drain_node_value(100, 0.0), 100);
    }
}
