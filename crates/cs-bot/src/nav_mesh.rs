// SPDX-License-Identifier: GPL-3.0-or-later
// Ported from ReGameDLL_CS `regamedll/game_shared/bot/nav_area.h` and `nav_area.cpp`.

//! Runtime [`NavMesh`] wrapper over [`cs_assets::nav::NavFile`] providing an $O(1)$
//! 300-unit spatial grid index, place dictionary resolution, hiding/sniper/ambush spot
//! queries, and dynamic per-team danger/blocked area costs.

use std::collections::{HashMap, HashSet, VecDeque};

use cs_assets::nav::{
    HidingSpotFlags, NAV_VERSION, NavArea, NavAttribute, NavDir, NavFile, NavLadder, SpotEncounter,
    UNDEFINED_PLACE,
};
use glam::Vec3;

/// Grid cell width/height matching `CNavAreaGrid::m_cellSize` (`300.0` units).
pub const GRID_CELL_SIZE: f32 = 300.0;
/// Danger decay rate per second (`1.0 / 120.0` matching `CNavArea::DecayDanger`).
pub const DANGER_DECAY_RATE: f32 = 1.0 / 120.0;
/// Number of tracked teams for danger and clear timestamps (`0 = T`, `1 = CT`).
pub const NUM_NAV_TEAMS: usize = 2;

/// Resolved reference to a [`cs_assets::nav::HidingSpot`] inside a [`NavMesh`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HidingSpotRef {
    pub area_idx: usize,
    pub spot_idx: usize,
    pub spot_id: u32,
    pub pos: Vec3,
    pub flags: HidingSpotFlags,
}

/// Runtime navigation mesh with spatial grid indexing and dynamic tactical state.
#[derive(Debug, Clone)]
pub struct NavMesh {
    version: u32,
    bsp_size: u32,
    places: Vec<String>,
    areas: Vec<NavArea>,
    ladders: Vec<NavLadder>,
    area_id_to_idx: HashMap<u32, usize>,
    ladder_id_to_idx: HashMap<u32, usize>,
    /// Pre-resolved area indices per cardinal direction (`NORTH..WEST`).
    adj_indices: Vec<[Vec<usize>; 4]>,
    /// Pre-resolved ladder indices ascending from each area.
    ladder_up_indices: Vec<Vec<usize>>,
    /// Pre-resolved ladder indices descending from each area.
    ladder_down_indices: Vec<Vec<usize>>,
    // Spatial grid (`300.0` unit cells)
    grid_min_x: f32,
    grid_min_y: f32,
    grid_size_x: usize,
    grid_size_y: usize,
    grid_cells: Vec<Vec<usize>>,
    // Dynamic runtime state
    blocked_areas: Vec<bool>,
    danger: Vec<[f32; NUM_NAV_TEAMS]>,
    danger_timestamp: Vec<[f64; NUM_NAV_TEAMS]>,
    cleared_timestamp: Vec<[f64; NUM_NAV_TEAMS]>,
}

impl Default for NavMesh {
    fn default() -> Self {
        Self::from_nav_file(NavFile::default())
    }
}

impl NavMesh {
    /// Build a runtime [`NavMesh`] and 300-unit spatial grid from a [`NavFile`].
    pub fn from_nav_file(file: NavFile) -> Self {
        let area_count = file.areas.len();
        let ladder_count = file.ladders.len();

        let mut area_id_to_idx = HashMap::with_capacity(area_count);
        for (idx, area) in file.areas.iter().enumerate() {
            area_id_to_idx.insert(area.id, idx);
        }

        let mut ladder_id_to_idx = HashMap::with_capacity(ladder_count);
        for (idx, ladder) in file.ladders.iter().enumerate() {
            ladder_id_to_idx.insert(ladder.id, idx);
        }

        let mut adj_indices = Vec::with_capacity(area_count);
        let mut ladder_up_indices = Vec::with_capacity(area_count);
        let mut ladder_down_indices = Vec::with_capacity(area_count);

        for area in &file.areas {
            let dirs = [
                resolve_ids(&area.connections[0], &area_id_to_idx),
                resolve_ids(&area.connections[1], &area_id_to_idx),
                resolve_ids(&area.connections[2], &area_id_to_idx),
                resolve_ids(&area.connections[3], &area_id_to_idx),
            ];
            adj_indices.push(dirs);
            ladder_up_indices.push(resolve_ids(&area.ladders_up, &ladder_id_to_idx));
            ladder_down_indices.push(resolve_ids(&area.ladders_down, &ladder_id_to_idx));
        }

        // Compute world bounds for the 300-unit spatial grid.
        let (grid_min_x, grid_max_x, grid_min_y, grid_max_y) = if file.areas.is_empty() {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            let mut min_x = f32::INFINITY;
            let mut min_y = f32::INFINITY;
            let mut max_x = f32::NEG_INFINITY;
            let mut max_y = f32::NEG_INFINITY;
            for area in &file.areas {
                min_x = min_x.min(area.nw_corner.x);
                min_y = min_y.min(area.nw_corner.y);
                max_x = max_x.max(area.se_corner.x);
                max_y = max_y.max(area.se_corner.y);
            }
            (min_x, max_x, min_y, max_y)
        };

        let grid_size_x = (((grid_max_x - grid_min_x) / GRID_CELL_SIZE).max(0.0) as usize) + 1;
        let grid_size_y = (((grid_max_y - grid_min_y) / GRID_CELL_SIZE).max(0.0) as usize) + 1;
        let mut grid_cells = vec![Vec::new(); grid_size_x.saturating_mul(grid_size_y)];

        for (idx, area) in file.areas.iter().enumerate() {
            let lo_x = world_to_grid(area.nw_corner.x, grid_min_x, grid_size_x);
            let hi_x = world_to_grid(area.se_corner.x, grid_min_x, grid_size_x);
            let lo_y = world_to_grid(area.nw_corner.y, grid_min_y, grid_size_y);
            let hi_y = world_to_grid(area.se_corner.y, grid_min_y, grid_size_y);
            for gy in lo_y..=hi_y {
                for gx in lo_x..=hi_x {
                    grid_cells[gx + gy * grid_size_x].push(idx);
                }
            }
        }

        Self {
            version: file.version,
            bsp_size: file.bsp_size,
            places: file.places,
            areas: file.areas,
            ladders: file.ladders,
            area_id_to_idx,
            ladder_id_to_idx,
            adj_indices,
            ladder_up_indices,
            ladder_down_indices,
            grid_min_x,
            grid_min_y,
            grid_size_x,
            grid_size_y,
            grid_cells,
            blocked_areas: vec![false; area_count],
            danger: vec![[0.0; NUM_NAV_TEAMS]; area_count],
            danger_timestamp: vec![[0.0; NUM_NAV_TEAMS]; area_count],
            cleared_timestamp: vec![[0.0; NUM_NAV_TEAMS]; area_count],
        }
    }

    /// Snapshot the current mesh back into a serializable [`NavFile`].
    pub fn to_nav_file(&self) -> NavFile {
        NavFile {
            version: if self.version == 0 {
                NAV_VERSION
            } else {
                self.version
            },
            bsp_size: self.bsp_size,
            places: self.places.clone(),
            areas: self.areas.clone(),
            ladders: self.ladders.clone(),
        }
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn bsp_size(&self) -> u32 {
        self.bsp_size
    }

    pub fn set_bsp_size(&mut self, bsp_size: u32) {
        self.bsp_size = bsp_size;
    }

    pub fn areas(&self) -> &[NavArea] {
        &self.areas
    }

    pub fn area(&self, idx: usize) -> Option<&NavArea> {
        self.areas.get(idx)
    }

    pub fn area_index_by_id(&self, id: u32) -> Option<usize> {
        self.area_id_to_idx.get(&id).copied()
    }

    pub fn area_by_id(&self, id: u32) -> Option<(usize, &NavArea)> {
        let idx = self.area_index_by_id(id)?;
        Some((idx, &self.areas[idx]))
    }

    pub fn ladders(&self) -> &[NavLadder] {
        &self.ladders
    }

    pub fn ladder(&self, idx: usize) -> Option<&NavLadder> {
        self.ladders.get(idx)
    }

    pub fn ladder_index_by_id(&self, id: u32) -> Option<usize> {
        self.ladder_id_to_idx.get(&id).copied()
    }

    pub fn adjacent_indices(&self, area_idx: usize, dir: NavDir) -> &[usize] {
        self.adj_indices
            .get(area_idx)
            .map_or(&[], |dirs| &dirs[dir.as_index()])
    }

    pub fn ladder_up_indices(&self, area_idx: usize) -> &[usize] {
        self.ladder_up_indices.get(area_idx).map_or(&[], Vec::as_slice)
    }

    pub fn ladder_down_indices(&self, area_idx: usize) -> &[usize] {
        self.ladder_down_indices
            .get(area_idx)
            .map_or(&[], Vec::as_slice)
    }

    /// Returns `true` if `from_idx` has a cardinal or ladder link to `to_idx`.
    pub fn is_connected(&self, from_idx: usize, to_idx: usize) -> bool {
        if from_idx == to_idx {
            return true;
        }
        let Some(dirs) = self.adj_indices.get(from_idx) else {
            return false;
        };
        if dirs.iter().any(|list| list.contains(&to_idx)) {
            return true;
        }
        let Some(to_area) = self.areas.get(to_idx) else {
            return false;
        };
        let to_id = to_area.id;
        for &l_idx in self.ladder_up_indices(from_idx) {
            let l = &self.ladders[l_idx];
            if l.top_forward_area_id == Some(to_id)
                || l.top_left_area_id == Some(to_id)
                || l.top_right_area_id == Some(to_id)
                || l.top_behind_area_id == Some(to_id)
            {
                return true;
            }
        }
        for &l_idx in self.ladder_down_indices(from_idx) {
            if self.ladders[l_idx].bottom_area_id == Some(to_id) {
                return true;
            }
        }
        false
    }

    /// Returns `true` if `area_idx` has no bidirectional link along `dir` (`CNavArea::IsEdge`).
    pub fn is_edge(&self, area_idx: usize, dir: NavDir) -> bool {
        let opp = dir.opposite();
        !self
            .adjacent_indices(area_idx, dir)
            .iter()
            .any(|&adj_idx| self.adjacent_indices(adj_idx, opp).contains(&area_idx))
    }

    /// Spatial grid lookup returning the area immediately beneath `pos` (within `beneath_limit`).
    /// Matches `CNavAreaGrid::GetNavArea`.
    pub fn get_nav_area(&self, pos: Vec3, beneath_limit: f32) -> Option<usize> {
        if self.areas.is_empty() || self.grid_cells.is_empty() {
            return None;
        }
        let gx = world_to_grid(pos.x, self.grid_min_x, self.grid_size_x);
        let gy = world_to_grid(pos.y, self.grid_min_y, self.grid_size_y);
        let cell = &self.grid_cells[gx + gy * self.grid_size_x];

        let test_z = pos.z + 5.0;
        let mut best: Option<(usize, f32)> = None;
        for &idx in cell {
            let area = &self.areas[idx];
            if !area.is_overlapping_xy(pos) {
                continue;
            }
            let z = area.z_at(pos.x, pos.y);
            if z > test_z || z < pos.z - beneath_limit {
                continue;
            }
            if best.is_none_or(|(_, bz)| z > bz) {
                best = Some((idx, z));
            }
        }
        best.map(|(idx, _)| idx)
    }

    /// Return the area directly beneath `pos` (using Valve's default `120.0` unit limit),
    /// or the nearest area in 3D space (`CNavAreaGrid::GetNearestNavArea`).
    pub fn get_nearest_nav_area(&self, pos: Vec3, any_z: bool) -> Option<usize> {
        if let Some(exact) = self.get_nav_area(pos, 120.0) {
            return Some(exact);
        }
        if self.areas.is_empty() {
            return None;
        }
        let mut best: Option<(usize, f32)> = None;
        for (idx, area) in self.areas.iter().enumerate() {
            let z = area.z_at(pos.x, pos.y);
            if !any_z && z > pos.z + 72.0 {
                continue;
            }
            let d2 = area.distance_squared_to_point(pos);
            if best.is_none_or(|(_, bd2)| d2 < bd2) {
                best = Some((idx, d2));
            }
        }
        if best.is_none() && !any_z {
            return self.get_nearest_nav_area(pos, true);
        }
        best.map(|(idx, _)| idx)
    }

    /// Nearest nav area whose center/closest-point passes `has_los(pos, target)`.
    pub fn get_nearest_nav_area_with_los(
        &self,
        pos: Vec3,
        any_z: bool,
        has_los: impl Fn(Vec3, Vec3) -> bool,
    ) -> Option<usize> {
        if let Some(exact) = self.get_nav_area(pos, 120.0) {
            return Some(exact);
        }
        let mut candidates: Vec<(usize, f32, Vec3)> = self
            .areas
            .iter()
            .enumerate()
            .filter_map(|(idx, area)| {
                let z = area.z_at(pos.x, pos.y);
                if !any_z && z > pos.z + 72.0 {
                    return None;
                }
                let cp = area.closest_point(pos);
                Some((idx, area.distance_squared_to_point(pos), cp))
            })
            .collect();
        candidates.sort_by(|a, b| a.1.total_cmp(&b.1));
        for (idx, _, cp) in &candidates {
            if has_los(pos + Vec3::new(0.0, 0.0, 36.0), *cp + Vec3::new(0.0, 0.0, 36.0)) {
                return Some(*idx);
            }
        }
        candidates.first().map(|(idx, _, _)| *idx)
    }

    // ---- Places ----

    pub fn places(&self) -> &[String] {
        &self.places
    }

    /// Resolve a 1-based `place_id` into its place name (`None` for `UNDEFINED_PLACE` `0`).
    pub fn place_name(&self, place_id: u16) -> Option<&str> {
        if place_id == UNDEFINED_PLACE {
            return None;
        }
        self.places.get((place_id - 1) as usize).map(String::as_str)
    }

    /// Case-insensitive lookup for a place name's 1-based `place_id`.
    pub fn place_id(&self, name: &str) -> Option<u16> {
        self.places
            .iter()
            .position(|p| p.eq_ignore_ascii_case(name))
            .map(|i| (i + 1) as u16)
    }

    /// Resolve the place name at world position `pos`.
    pub fn get_place(&self, pos: Vec3) -> Option<&str> {
        let idx = self.get_nearest_nav_area(pos, true)?;
        self.place_name(self.areas[idx].place_id)
    }

    /// Return all area indices tagged with `place_id`.
    pub fn areas_in_place(&self, place_id: u16) -> Vec<usize> {
        self.areas
            .iter()
            .enumerate()
            .filter_map(|(i, a)| (a.place_id == place_id).then_some(i))
            .collect()
    }

    /// Assign (and intern if needed) a place name onto `area_idx`. Returns the 1-based `place_id`.
    pub fn set_area_place(&mut self, area_idx: usize, place_name: &str) -> u16 {
        let Some(area) = self.areas.get(area_idx) else {
            return UNDEFINED_PLACE;
        };
        let _ = area;
        if place_name.is_empty() {
            self.areas[area_idx].place_id = UNDEFINED_PLACE;
            return UNDEFINED_PLACE;
        }
        let pid = if let Some(existing) = self.place_id(place_name) {
            existing
        } else {
            self.places.push(place_name.to_string());
            self.places.len() as u16
        };
        self.areas[area_idx].place_id = pid;
        pid
    }

    // ---- Dynamic blocked & danger costs ----

    pub fn set_area_blocked(&mut self, area_idx: usize, blocked: bool) {
        if let Some(slot) = self.blocked_areas.get_mut(area_idx) {
            *slot = blocked;
        }
    }

    pub fn is_area_blocked(&self, area_idx: usize) -> bool {
        self.blocked_areas.get(area_idx).copied().unwrap_or(false)
    }

    pub fn clear_blocked(&mut self) {
        self.blocked_areas.fill(false);
    }

    fn decay_danger_slot(&mut self, area_idx: usize, team: usize, now: f64) {
        if team >= NUM_NAV_TEAMS || area_idx >= self.areas.len() {
            return;
        }
        let dt = (now - self.danger_timestamp[area_idx][team]).max(0.0) as f32;
        let decayed = self.danger[area_idx][team] - DANGER_DECAY_RATE * dt;
        self.danger[area_idx][team] = decayed.max(0.0);
        self.danger_timestamp[area_idx][team] = now;
    }

    /// Increase danger on `area_idx` for `team` (`0 = T, 1 = CT`) at simulation time `now`.
    pub fn increase_danger(&mut self, area_idx: usize, team: usize, amount: f32, now: f64) {
        if team >= NUM_NAV_TEAMS || area_idx >= self.areas.len() || amount <= 0.0 {
            return;
        }
        self.decay_danger_slot(area_idx, team, now);
        self.danger[area_idx][team] += amount;
    }

    /// Decay and return the current danger of `area_idx` for `team`.
    pub fn get_danger(&mut self, area_idx: usize, team: usize, now: f64) -> f32 {
        if team >= NUM_NAV_TEAMS || area_idx >= self.areas.len() {
            return 0.0;
        }
        self.decay_danger_slot(area_idx, team, now);
        self.danger[area_idx][team]
    }

    /// Non-mutating read of the decayed danger value at `now` (used during A* search).
    pub fn peek_danger(&self, area_idx: usize, team: usize, now: f64) -> f32 {
        if team >= NUM_NAV_TEAMS || area_idx >= self.areas.len() {
            return 0.0;
        }
        let dt = (now - self.danger_timestamp[area_idx][team]).max(0.0) as f32;
        (self.danger[area_idx][team] - DANGER_DECAY_RATE * dt).max(0.0)
    }

    /// Propagate danger outward via BFS up to `max_radius` (`IncreaseDangerNearby`).
    pub fn increase_danger_nearby(
        &mut self,
        team: usize,
        amount: f32,
        center_area_idx: usize,
        pos: Vec3,
        max_radius: f32,
        now: f64,
    ) {
        if team >= NUM_NAV_TEAMS || center_area_idx >= self.areas.len() || max_radius <= 0.0 {
            return;
        }
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        visited.insert(center_area_idx);
        queue.push_back(center_area_idx);
        self.increase_danger(center_area_idx, team, amount, now);

        while let Some(curr) = queue.pop_front() {
            for dir in NavDir::ALL {
                let adj_list = self.adj_indices[curr][dir.as_index()].clone();
                for adj_idx in adj_list {
                    if !visited.insert(adj_idx) {
                        continue;
                    }
                    let dist = self.areas[adj_idx].center().distance(pos);
                    if dist <= max_radius {
                        let falloff = (1.0 - dist / max_radius).clamp(0.1, 1.0);
                        self.increase_danger(adj_idx, team, amount * falloff, now);
                        queue.push_back(adj_idx);
                    }
                }
            }
        }
    }

    pub fn set_cleared_timestamp(&mut self, area_idx: usize, team: usize, now: f64) {
        if let Some(slot) = self.cleared_timestamp.get_mut(area_idx)
            && team < NUM_NAV_TEAMS
        {
            slot[team] = now;
        }
    }

    pub fn get_cleared_timestamp(&self, area_idx: usize, team: usize) -> f64 {
        self.cleared_timestamp
            .get(area_idx)
            .and_then(|s| s.get(team).copied())
            .unwrap_or(0.0)
    }

    // ---- Tactical Spot Queries ----

    /// Collect hiding spots matching `required_flags` within `max_range` of `origin`
    /// (or anywhere on the map if `max_range <= 0.0`), sorted by distance from `origin`.
    pub fn find_hiding_spots(
        &self,
        origin: Vec3,
        max_range: f32,
        required_flags: HidingSpotFlags,
        place_id: Option<u16>,
        allow_crouch_areas: bool,
    ) -> Vec<HidingSpotRef> {
        let range_sq = if max_range > 0.0 {
            Some(max_range * max_range)
        } else {
            None
        };
        let mut results = Vec::new();
        for (area_idx, area) in self.areas.iter().enumerate() {
            if self.is_area_blocked(area_idx) {
                continue;
            }
            if area.attributes.contains(NavAttribute::DONT_HIDE) {
                continue;
            }
            if !allow_crouch_areas && area.attributes.contains(NavAttribute::CROUCH) {
                continue;
            }
            if let Some(pid) = place_id
                && pid != UNDEFINED_PLACE
                && area.place_id != pid
            {
                continue;
            }
            for (spot_idx, spot) in area.hiding_spots.iter().enumerate() {
                if required_flags.bits() != 0 && !spot.flags.intersects(required_flags) {
                    continue;
                }
                if let Some(max_d2) = range_sq
                    && spot.pos.distance_squared(origin) > max_d2
                {
                    continue;
                }
                results.push(HidingSpotRef {
                    area_idx,
                    spot_idx,
                    spot_id: spot.id,
                    pos: spot.pos,
                    flags: spot.flags,
                });
            }
        }
        results.sort_by(|a, b| {
            a.pos
                .distance_squared(origin)
                .total_cmp(&b.pos.distance_squared(origin))
        });
        results
    }

    /// Find the nearest hiding spot (or sniper spot when `is_sniper == true`), matching
    /// ReGameDLL's `FindNearbyHidingSpot` priority order (`IDEAL_SNIPER` -> `GOOD_SNIPER` -> `IN_COVER`).
    pub fn find_nearest_hiding_spot(
        &self,
        origin: Vec3,
        max_range: f32,
        is_sniper: bool,
        place_id: Option<u16>,
        allow_crouch_areas: bool,
    ) -> Option<HidingSpotRef> {
        if is_sniper {
            if let Some(ideal) = self
                .find_hiding_spots(
                    origin,
                    max_range,
                    HidingSpotFlags::IDEAL_SNIPER_SPOT,
                    place_id,
                    allow_crouch_areas,
                )
                .into_iter()
                .next()
            {
                return Some(ideal);
            }
            if let Some(good) = self
                .find_hiding_spots(
                    origin,
                    max_range,
                    HidingSpotFlags::GOOD_SNIPER_SPOT,
                    place_id,
                    allow_crouch_areas,
                )
                .into_iter()
                .next()
            {
                return Some(good);
            }
        }
        self.find_hiding_spots(
            origin,
            max_range,
            HidingSpotFlags::IN_COVER,
            place_id,
            allow_crouch_areas,
        )
        .into_iter()
        .next()
    }

    /// Return all sniper spots (`IDEAL_SNIPER_SPOT` prioritized before `GOOD_SNIPER_SPOT`).
    pub fn find_sniper_spots(
        &self,
        origin: Vec3,
        max_range: f32,
        place_id: Option<u16>,
    ) -> Vec<HidingSpotRef> {
        let mut spots = self.find_hiding_spots(
            origin,
            max_range,
            HidingSpotFlags::IDEAL_SNIPER_SPOT | HidingSpotFlags::GOOD_SNIPER_SPOT,
            place_id,
            true,
        );
        spots.sort_by(|a, b| {
            let a_ideal = a.flags.contains(HidingSpotFlags::IDEAL_SNIPER_SPOT);
            let b_ideal = b.flags.contains(HidingSpotFlags::IDEAL_SNIPER_SPOT);
            b_ideal.cmp(&a_ideal).then_with(|| {
                a.pos
                    .distance_squared(origin)
                    .total_cmp(&b.pos.distance_squared(origin))
            })
        });
        spots
    }

    /// Find defensive ambush spots within `[min_range, max_range]` of an objective.
    pub fn find_ambush_spots(
        &self,
        objective_pos: Vec3,
        min_range: f32,
        max_range: f32,
        place_id: Option<u16>,
    ) -> Vec<HidingSpotRef> {
        let min_sq = min_range * min_range;
        self.find_hiding_spots(
            objective_pos,
            max_range,
            HidingSpotFlags::IN_COVER
                | HidingSpotFlags::GOOD_SNIPER_SPOT
                | HidingSpotFlags::IDEAL_SNIPER_SPOT,
            place_id,
            true,
        )
        .into_iter()
        .filter(|s| s.pos.distance_squared(objective_pos) >= min_sq)
        .collect()
    }

    pub fn get_spot_encounter(
        &self,
        area_idx: usize,
        from_id: u32,
        to_id: u32,
    ) -> Option<&SpotEncounter> {
        let area = self.areas.get(area_idx)?;
        area.encounters
            .iter()
            .find(|e| e.from_id == from_id && e.to_id == to_id)
    }

    /// Compute closest point within the shared portal between `from_idx` and `to_idx` along `dir`.
    /// Matches `CNavArea::ComputeClosestPointInPortal` (`nav_area.cpp:2209-2280`).
    pub fn compute_closest_point_in_portal(
        &self,
        from_idx: usize,
        to_idx: usize,
        dir: NavDir,
        from_pos: Vec3,
    ) -> Vec3 {
        const MARGIN: f32 = 12.5; // GenerationStepSize / 2.0
        let from = &self.areas[from_idx];
        let to = &self.areas[to_idx];
        let mut close = from_pos;

        match dir {
            NavDir::North | NavDir::South => {
                close.y = if dir == NavDir::North {
                    from.nw_corner.y
                } else {
                    from.se_corner.y
                };
                let left = from
                    .nw_corner
                    .x
                    .max(to.nw_corner.x)
                    .clamp(from.nw_corner.x, from.se_corner.x);
                let right = from
                    .se_corner
                    .x
                    .min(to.se_corner.x)
                    .clamp(from.nw_corner.x, from.se_corner.x);
                let left_margin = if self.is_edge(to_idx, NavDir::West) {
                    (left + MARGIN).min(right)
                } else {
                    left
                };
                let right_margin = if self.is_edge(to_idx, NavDir::East) {
                    (right - MARGIN).max(left_margin)
                } else {
                    right
                };
                close.x = from_pos.x.clamp(left_margin, right_margin);
            }
            NavDir::East | NavDir::West => {
                close.x = if dir == NavDir::West {
                    from.nw_corner.x
                } else {
                    from.se_corner.x
                };
                let top = from
                    .nw_corner
                    .y
                    .max(to.nw_corner.y)
                    .clamp(from.nw_corner.y, from.se_corner.y);
                let bottom = from
                    .se_corner
                    .y
                    .min(to.se_corner.y)
                    .clamp(from.nw_corner.y, from.se_corner.y);
                let top_margin = if self.is_edge(to_idx, NavDir::North) {
                    (top + MARGIN).min(bottom)
                } else {
                    top
                };
                let bottom_margin = if self.is_edge(to_idx, NavDir::South) {
                    (bottom - MARGIN).max(top_margin)
                } else {
                    bottom
                };
                close.y = from_pos.y.clamp(top_margin, bottom_margin);
            }
        }
        close.z = from.z_at(close.x, close.y);
        close
    }
}

fn world_to_grid(val: f32, min_val: f32, grid_size: usize) -> usize {
    if grid_size <= 1 {
        return 0;
    }
    let idx = ((val - min_val) / GRID_CELL_SIZE).floor() as isize;
    idx.clamp(0, (grid_size - 1) as isize) as usize
}

fn resolve_ids(ids: &[u32], map: &HashMap<u32, usize>) -> Vec<usize> {
    ids.iter().filter_map(|id| map.get(id).copied()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_assets::nav::HidingSpot;

    fn make_test_mesh() -> NavMesh {
        let a1 = NavArea {
            id: 1,
            attributes: NavAttribute::NONE,
            nw_corner: Vec3::new(0.0, 0.0, 0.0),
            se_corner: Vec3::new(200.0, 200.0, 0.0),
            ne_z: 0.0,
            sw_z: 0.0,
            connections: [vec![], vec![2], vec![], vec![]],
            hiding_spots: vec![
                HidingSpot {
                    id: 1,
                    pos: Vec3::new(20.0, 20.0, 0.0),
                    flags: HidingSpotFlags::IN_COVER,
                },
                HidingSpot {
                    id: 2,
                    pos: Vec3::new(180.0, 180.0, 0.0),
                    flags: HidingSpotFlags::IN_COVER | HidingSpotFlags::IDEAL_SNIPER_SPOT,
                },
            ],
            approach_spots: Vec::new(),
            encounters: Vec::new(),
            place_id: 1,
            ladders_up: Vec::new(),
            ladders_down: Vec::new(),
        };
        let a2 = NavArea {
            id: 2,
            attributes: NavAttribute::CROUCH,
            nw_corner: Vec3::new(200.0, 0.0, 0.0),
            se_corner: Vec3::new(400.0, 200.0, 0.0),
            ne_z: 0.0,
            sw_z: 0.0,
            connections: [vec![], vec![], vec![], vec![1]],
            hiding_spots: vec![HidingSpot {
                id: 3,
                pos: Vec3::new(380.0, 100.0, 0.0),
                flags: HidingSpotFlags::IN_COVER,
            }],
            approach_spots: Vec::new(),
            encounters: Vec::new(),
            place_id: 2,
            ladders_up: Vec::new(),
            ladders_down: Vec::new(),
        };
        NavMesh::from_nav_file(NavFile {
            version: NAV_VERSION,
            bsp_size: 42,
            places: vec!["CTSpawn".to_string(), "Ducts".to_string()],
            areas: vec![a1, a2],
            ladders: Vec::new(),
        })
    }

    #[test]
    fn spatial_grid_and_place_lookup() {
        let mesh = make_test_mesh();
        assert_eq!(mesh.get_nav_area(Vec3::new(100.0, 100.0, 10.0), 120.0), Some(0));
        assert_eq!(mesh.get_nav_area(Vec3::new(350.0, 50.0, 10.0), 120.0), Some(1));
        assert_eq!(
            mesh.get_nearest_nav_area(Vec3::new(500.0, 100.0, 0.0), true),
            Some(1)
        );
        assert_eq!(mesh.get_place(Vec3::new(50.0, 50.0, 0.0)), Some("CTSpawn"));
        assert_eq!(mesh.place_id("ducts"), Some(2));
    }

    #[test]
    fn danger_decay_and_nearby_propagation() {
        let mut mesh = make_test_mesh();
        mesh.increase_danger_nearby(0, 1.0, 0, Vec3::new(100.0, 100.0, 0.0), 500.0, 10.0);
        assert!((mesh.get_danger(0, 0, 10.0) - 1.0).abs() < 1e-4);
        assert!(mesh.get_danger(1, 0, 10.0) > 0.1);
        // After 60s, danger decays by 0.5
        assert!((mesh.get_danger(0, 0, 70.0) - 0.5).abs() < 1e-4);
        // After 130s, decays to 0
        assert_eq!(mesh.get_danger(0, 0, 130.0), 0.0);
    }

    #[test]
    fn hiding_sniper_and_ambush_queries() {
        let mut mesh = make_test_mesh();
        let sniper = mesh
            .find_nearest_hiding_spot(Vec3::new(10.0, 10.0, 0.0), 500.0, true, None, true)
            .expect("ideal sniper spot");
        assert_eq!(sniper.spot_id, 2);

        // Disallowing crouch areas skips area 2's spot #3
        let spots = mesh.find_hiding_spots(
            Vec3::new(350.0, 100.0, 0.0),
            500.0,
            HidingSpotFlags::IN_COVER,
            None,
            false,
        );
        assert!(spots.iter().all(|s| s.spot_id != 3));

        // Blocking area 0 hides its spots
        mesh.set_area_blocked(0, true);
        let ambush = mesh.find_ambush_spots(Vec3::new(100.0, 100.0, 0.0), 50.0, 500.0, None);
        assert_eq!(ambush.len(), 1);
        assert_eq!(ambush[0].spot_id, 3);
    }
}
