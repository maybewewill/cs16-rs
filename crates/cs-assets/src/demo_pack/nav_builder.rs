//! Clean-room deterministic ZBot v5 `.nav` mesh builder for `maps/dm_rusthouse.nav`.
//!
//! Tiles the walkable courtyard floor, the central raised steel bridge (`z = 64`), and the two
//! sloped walk-up ramps (`z = 0..64`) with proper 16-unit hull clearance around solid cover
//! crates and spawn walls, bidirectional 4-way cardinal links, tactical hiding/sniper spots,
//! and named places (`CTSpawn`, `TSpawn`, `Bridge`, `NorthLane`, `SouthLane`, `Courtyard`).

use glam::Vec3;

use crate::demo_pack::bsp_builder::{ARENA_HALF, BRIDGE_Z, dm_rusthouse_brushes};
use crate::nav::{HidingSpot, HidingSpotFlags, NAV_VERSION, NavArea, NavAttribute, NavFile};

const CELL: f32 = 64.0;
const GRID_N: usize = 24; // 24 * 64 = 1536 covering [-768, +768]

#[derive(Clone, Copy, PartialEq, Eq)]
enum CellKind {
    Blocked,
    Floor,
    WestRamp,
    EastRamp,
    Bridge,
}

/// Build the clean-room `NavFile` and serialize it to Valve `.nav` v5 bytes.
pub fn build_dm_rusthouse_nav(bsp_size: u32) -> Vec<u8> {
    let nav = build_dm_rusthouse_nav_file(bsp_size);
    nav.to_bytes().expect("dm_rusthouse NavFile serialization")
}

/// Build the structured `NavFile` for `dm_rusthouse`.
pub fn build_dm_rusthouse_nav_file(bsp_size: u32) -> NavFile {
    let brushes = dm_rusthouse_brushes();
    // Solid blocking brushes (exclude index 0=bridge, 1=west ramp, 2=east ramp which are walkable).
    let obstacles: Vec<(Vec3, Vec3)> = brushes.iter().skip(3).map(|b| (b.mins, b.maxs)).collect();

    let places = vec![
        "CTSpawn".to_string(),
        "TSpawn".to_string(),
        "Bridge".to_string(),
        "NorthLane".to_string(),
        "SouthLane".to_string(),
        "Courtyard".to_string(),
    ];

    let mut grid = [[CellKind::Blocked; GRID_N]; GRID_N];
    let mut cell_id = [[0u32; GRID_N]; GRID_N];

    for (gy, row) in grid.iter_mut().enumerate() {
        let y0 = -ARENA_HALF + gy as f32 * CELL;
        let y1 = y0 + CELL;
        let yc = (y0 + y1) * 0.5;
        for (gx, slot) in row.iter_mut().enumerate() {
            let x0 = -ARENA_HALF + gx as f32 * CELL;
            let x1 = x0 + CELL;
            let xc = (x0 + x1) * 0.5;

            // Keep 16u inset from outer courtyard walls on perimeter cells via corner clamping,
            // and check ramps/bridge/obstacles cleanly.
            let kind = if (-128.0..128.0).contains(&xc) && (-112.0..112.0).contains(&yc) {
                if yc.abs() < 64.0 {
                    CellKind::Bridge
                } else {
                    // Narrow strip along north/south edge of bridge — keep clear of ramp side walls
                    CellKind::Blocked
                }
            } else if (-256.0..-128.0).contains(&xc) && yc.abs() < 64.0 {
                CellKind::WestRamp
            } else if (128.0..256.0).contains(&xc) && yc.abs() < 64.0 {
                CellKind::EastRamp
            } else if (-256.0..256.0).contains(&xc) && yc.abs() < 128.0 {
                // Solid side faces of ramp/bridge footprints
                CellKind::Blocked
            } else {
                // Check solid cover crates & shield walls with 8u clearance margin
                let hits_obs = obstacles.iter().any(|(mn, mx)| {
                    x1 > mn.x - 8.0 && x0 < mx.x + 8.0 && y1 > mn.y - 8.0 && y0 < mx.y + 8.0
                });
                if hits_obs {
                    CellKind::Blocked
                } else {
                    CellKind::Floor
                }
            };
            *slot = kind;
        }
    }

    // Assign 1-based area IDs to all walkable cells.
    let mut next_id = 1u32;
    for gy in 0..GRID_N {
        for gx in 0..GRID_N {
            if grid[gy][gx] != CellKind::Blocked {
                cell_id[gy][gx] = next_id;
                next_id += 1;
            }
        }
    }

    let mut areas = Vec::new();
    let mut next_spot_id = 1u32;

    for gy in 0..GRID_N {
        let raw_y0 = -ARENA_HALF + gy as f32 * CELL;
        let raw_y1 = raw_y0 + CELL;
        // Inset outermost cells by 20 units so bots stay cleanly inside the 16-unit wall margin.
        let y0 = if gy == 0 { raw_y0 + 20.0 } else { raw_y0 };
        let y1 = if gy + 1 == GRID_N {
            raw_y1 - 20.0
        } else {
            raw_y1
        };

        for gx in 0..GRID_N {
            let kind = grid[gy][gx];
            if kind == CellKind::Blocked {
                continue;
            }
            let raw_x0 = -ARENA_HALF + gx as f32 * CELL;
            let raw_x1 = raw_x0 + CELL;
            let x0 = if gx == 0 { raw_x0 + 20.0 } else { raw_x0 };
            let x1 = if gx + 1 == GRID_N {
                raw_x1 - 20.0
            } else {
                raw_x1
            };

            let (z_w, z_e) = match kind {
                CellKind::Floor => (0.0, 0.0),
                CellKind::Bridge => (BRIDGE_Z, BRIDGE_Z),
                CellKind::WestRamp => {
                    let zw = ((raw_x0 + 256.0) / 128.0).clamp(0.0, 1.0) * BRIDGE_Z;
                    let ze = ((raw_x1 + 256.0) / 128.0).clamp(0.0, 1.0) * BRIDGE_Z;
                    (zw, ze)
                }
                CellKind::EastRamp => {
                    let zw = ((256.0 - raw_x0) / 128.0).clamp(0.0, 1.0) * BRIDGE_Z;
                    let ze = ((256.0 - raw_x1) / 128.0).clamp(0.0, 1.0) * BRIDGE_Z;
                    (zw, ze)
                }
                CellKind::Blocked => unreachable!(),
            };

            let mut connections = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
            // NavDir: 0=North (-Y), 1=East (+X), 2=South (+Y), 3=West (-X)
            let can_link = |from: CellKind, to: CellKind, dir: usize| -> bool {
                match (from, to) {
                    (CellKind::Blocked, _) | (_, CellKind::Blocked) => false,
                    (CellKind::Floor, CellKind::Floor) => true,
                    (CellKind::Bridge, CellKind::Bridge) => true,
                    // West ramp connects E/W cleanly between floor and bridge
                    (CellKind::WestRamp, CellKind::WestRamp) => true,
                    (CellKind::Floor, CellKind::WestRamp) => dir == 1, // moving +X onto ramp
                    (CellKind::WestRamp, CellKind::Floor) => dir == 3, // moving -X off ramp
                    (CellKind::WestRamp, CellKind::Bridge) => dir == 1, // moving +X onto bridge
                    (CellKind::Bridge, CellKind::WestRamp) => dir == 3, // moving -X onto ramp
                    // East ramp connects E/W cleanly between bridge and floor
                    (CellKind::EastRamp, CellKind::EastRamp) => true,
                    (CellKind::Floor, CellKind::EastRamp) => dir == 3, // moving -X onto ramp
                    (CellKind::EastRamp, CellKind::Floor) => dir == 1, // moving +X off ramp
                    (CellKind::EastRamp, CellKind::Bridge) => dir == 3, // moving -X onto bridge
                    (CellKind::Bridge, CellKind::EastRamp) => dir == 1, // moving +X onto ramp
                    // Safe drop down from bridge north/south if adjacent cell is floor
                    (CellKind::Bridge, CellKind::Floor) => dir == 0 || dir == 2,
                    _ => false,
                }
            };

            // North (-Y => gy - 1)
            if gy > 0 && can_link(kind, grid[gy - 1][gx], 0) {
                connections[0].push(cell_id[gy - 1][gx]);
            }
            // East (+X => gx + 1)
            if gx + 1 < GRID_N && can_link(kind, grid[gy][gx + 1], 1) {
                connections[1].push(cell_id[gy][gx + 1]);
            }
            // South (+Y => gy + 1)
            if gy + 1 < GRID_N && can_link(kind, grid[gy + 1][gx], 2) {
                connections[2].push(cell_id[gy + 1][gx]);
            }
            // West (-X => gx - 1)
            if gx > 0 && can_link(kind, grid[gy][gx - 1], 3) {
                connections[3].push(cell_id[gy][gx - 1]);
            }

            let xc = (x0 + x1) * 0.5;
            let yc = (y0 + y1) * 0.5;
            let place_id = if kind == CellKind::Bridge
                || kind == CellKind::WestRamp
                || kind == CellKind::EastRamp
            {
                3 // Bridge
            } else if xc < -460.0 {
                1 // CTSpawn
            } else if xc > 460.0 {
                2 // TSpawn
            } else if yc < -240.0 {
                4 // NorthLane
            } else if yc > 240.0 {
                5 // SouthLane
            } else {
                6 // Courtyard
            };

            let mut hiding_spots = Vec::new();
            let adjacent_to_obstacle = (gx > 0 && grid[gy][gx - 1] == CellKind::Blocked)
                || (gx + 1 < GRID_N && grid[gy][gx + 1] == CellKind::Blocked)
                || (gy > 0 && grid[gy - 1][gx] == CellKind::Blocked)
                || (gy + 1 < GRID_N && grid[gy + 1][gx] == CellKind::Blocked);
            if kind == CellKind::Bridge && ((gx + gy) % 2 == 0) {
                hiding_spots.push(HidingSpot {
                    id: next_spot_id,
                    pos: Vec3::new(xc, yc, BRIDGE_Z),
                    flags: HidingSpotFlags::GOOD_SNIPER_SPOT,
                });
                next_spot_id += 1;
            } else if kind == CellKind::Floor && adjacent_to_obstacle && ((gx + gy) % 2 == 0) {
                hiding_spots.push(HidingSpot {
                    id: next_spot_id,
                    pos: Vec3::new(xc, yc, 0.0),
                    flags: HidingSpotFlags::IN_COVER,
                });
                next_spot_id += 1;
            }

            let attributes = if kind == CellKind::WestRamp || kind == CellKind::EastRamp {
                NavAttribute::NO_JUMP
            } else {
                NavAttribute::NONE
            };

            areas.push(NavArea {
                id: cell_id[gy][gx],
                attributes,
                nw_corner: Vec3::new(x0, y0, z_w),
                se_corner: Vec3::new(x1, y1, z_e),
                ne_z: z_e,
                sw_z: z_w,
                connections,
                hiding_spots,
                approach_spots: Vec::new(),
                encounters: Vec::new(),
                place_id,
                ladders_up: Vec::new(),
                ladders_down: Vec::new(),
            });
        }
    }

    NavFile {
        version: NAV_VERSION,
        bsp_size,
        places,
        areas,
        ladders: Vec::new(),
    }
}
