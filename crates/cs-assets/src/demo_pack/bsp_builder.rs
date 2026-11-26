//! Clean-room procedural GoldSrc `BSP v30` compiler and RGB lightmap baker for `dm_rusthouse`.
//!
//! Builds an airtight symmetric courtyard (`1536x1536x320`) with:
//! - Exact convex-polyhedron BSP splitting across Hulls 0, 1, 2, 3 (`nodes`, `leafs`, `clipnodes`)
//! - Central raised steel bridge + two smooth 26.6° walk-up ramps (`normal.z = 0.8944 >= 0.7`)
//! - 4 tall tactical cover crates, 2 mid-flank crouch crates, 2 spawn cover walls
//! - 8 CT (`info_player_start`) & 8 T (`info_player_deathmatch`) spawns with >= 160u clearance
//! - CT & T `func_buyzone` brush submodels (`*1`, `*2`)
//! - Raycasted warm sunlight + soft penumbra shadows + sky ambient fill RGB lightmaps
//! - Valid PVS visibility lump and node-attached faces for `R_LightPoint` and `trace_texture`

use glam::Vec3;

use crate::bsp::{BSP_VERSION_GOLDSRC, ClipNode, Face, LUMP_COUNT, Leaf, Node, Plane, TexInfo};
use crate::demo_pack::textures::{
    TEX_CRATE, TEX_DECK, TEX_FLOOR, TEX_RUSTTRIM, TEX_SANDWALL, TEX_SKY, TEX_SPAWNCT, TEX_SPAWNT,
    build_arena_miptex_list,
};

const CONTENTS_EMPTY: i32 = -1;
const CONTENTS_SOLID: i32 = -2;
const CONTENTS_SKY: i32 = -6;

/// Arena interior half-extents (`1536 x 1536` footprint, `320` ceiling).
pub const ARENA_HALF: f32 = 768.0;
pub const ARENA_HEIGHT: f32 = 320.0;
pub const WALL_HEIGHT: f32 = 256.0;
pub const BRIDGE_Z: f32 = 64.0;

/// Hull half-extents matching GoldSrc (`Hull::Point/Standing/Large/Crouched`).
const HULL_EXTENTS: [Vec3; 4] = [
    Vec3::ZERO,
    Vec3::new(16.0, 16.0, 36.0),
    Vec3::new(32.0, 32.0, 32.0),
    Vec3::new(16.0, 16.0, 18.0),
];

/// An outward-facing half-space `normal · p <= dist`.
#[derive(Debug, Clone, Copy)]
struct HalfSpace {
    normal: Vec3,
    dist: f32,
}

/// A solid convex brush (axis-aligned box or sloped ramp wedge).
#[derive(Debug, Clone)]
pub(crate) struct ConvexBrush {
    planes: Vec<HalfSpace>,
    pub(crate) mins: Vec3,
    pub(crate) maxs: Vec3,
    /// Optional visual faces `(verts_ccw_outward, tex_idx)`.
    faces: Vec<(Vec<Vec3>, usize)>,
}

impl ConvexBrush {
    fn from_box(mins: Vec3, maxs: Vec3, side_tex: usize, top_tex: usize) -> Self {
        let planes = vec![
            HalfSpace {
                normal: Vec3::NEG_X,
                dist: -mins.x,
            },
            HalfSpace {
                normal: Vec3::X,
                dist: maxs.x,
            },
            HalfSpace {
                normal: Vec3::NEG_Y,
                dist: -mins.y,
            },
            HalfSpace {
                normal: Vec3::Y,
                dist: maxs.y,
            },
            HalfSpace {
                normal: Vec3::NEG_Z,
                dist: -mins.z,
            },
            HalfSpace {
                normal: Vec3::Z,
                dist: maxs.z,
            },
        ];
        let (x0, y0, z0) = (mins.x, mins.y, mins.z);
        let (x1, y1, z1) = (maxs.x, maxs.y, maxs.z);
        // Outward-facing CCW quads (skip bottom face sitting flush on z=0).
        let mut faces = vec![
            // +Z top
            (
                vec![
                    Vec3::new(x0, y0, z1),
                    Vec3::new(x1, y0, z1),
                    Vec3::new(x1, y1, z1),
                    Vec3::new(x0, y1, z1),
                ],
                top_tex,
            ),
            // -X west
            (
                vec![
                    Vec3::new(x0, y1, z0),
                    Vec3::new(x0, y0, z0),
                    Vec3::new(x0, y0, z1),
                    Vec3::new(x0, y1, z1),
                ],
                side_tex,
            ),
            // +X east
            (
                vec![
                    Vec3::new(x1, y0, z0),
                    Vec3::new(x1, y1, z0),
                    Vec3::new(x1, y1, z1),
                    Vec3::new(x1, y0, z1),
                ],
                side_tex,
            ),
            // -Y north
            (
                vec![
                    Vec3::new(x0, y0, z0),
                    Vec3::new(x1, y0, z0),
                    Vec3::new(x1, y0, z1),
                    Vec3::new(x0, y0, z1),
                ],
                side_tex,
            ),
            // +Y south
            (
                vec![
                    Vec3::new(x1, y1, z0),
                    Vec3::new(x0, y1, z0),
                    Vec3::new(x0, y1, z1),
                    Vec3::new(x1, y1, z1),
                ],
                side_tex,
            ),
        ];
        if z0 > 0.5 {
            faces.push((
                vec![
                    Vec3::new(x0, y1, z0),
                    Vec3::new(x1, y1, z0),
                    Vec3::new(x1, y0, z0),
                    Vec3::new(x0, y0, z0),
                ],
                side_tex,
            ));
        }
        Self {
            planes,
            mins,
            maxs,
            faces,
        }
    }

    /// Ramp rising toward `+X` (`x0` at `z=0`, `x1` at `z=height`).
    fn ramp_rising_pos_x(
        x0: f32,
        x1: f32,
        y0: f32,
        y1: f32,
        height: f32,
        side_tex: usize,
        top_tex: usize,
    ) -> Self {
        let run = x1 - x0;
        let slope_n = Vec3::new(-height, 0.0, run).normalize();
        let slope_d = slope_n.dot(Vec3::new(x0, y0, 0.0));
        let planes = vec![
            HalfSpace {
                normal: Vec3::X,
                dist: x1,
            },
            HalfSpace {
                normal: Vec3::NEG_Y,
                dist: -y0,
            },
            HalfSpace {
                normal: Vec3::Y,
                dist: y1,
            },
            HalfSpace {
                normal: Vec3::NEG_Z,
                dist: 0.0,
            },
            HalfSpace {
                normal: slope_n,
                dist: slope_d,
            },
        ];
        let faces = vec![
            // Sloped top deck
            (
                vec![
                    Vec3::new(x0, y0, 0.0),
                    Vec3::new(x1, y0, height),
                    Vec3::new(x1, y1, height),
                    Vec3::new(x0, y1, 0.0),
                ],
                top_tex,
            ),
            // -Y triangle side
            (
                vec![
                    Vec3::new(x0, y0, 0.0),
                    Vec3::new(x1, y0, 0.0),
                    Vec3::new(x1, y0, height),
                ],
                side_tex,
            ),
            // +Y triangle side
            (
                vec![
                    Vec3::new(x1, y1, 0.0),
                    Vec3::new(x0, y1, 0.0),
                    Vec3::new(x1, y1, height),
                ],
                side_tex,
            ),
        ];
        Self {
            planes,
            mins: Vec3::new(x0, y0, 0.0),
            maxs: Vec3::new(x1, y1, height),
            faces,
        }
    }

    /// Ramp rising toward `-X` (`x1` at `z=0`, `x0` at `z=height`).
    fn ramp_rising_neg_x(
        x0: f32,
        x1: f32,
        y0: f32,
        y1: f32,
        height: f32,
        side_tex: usize,
        top_tex: usize,
    ) -> Self {
        let run = x1 - x0;
        let slope_n = Vec3::new(height, 0.0, run).normalize();
        let slope_d = slope_n.dot(Vec3::new(x1, y0, 0.0));
        let planes = vec![
            HalfSpace {
                normal: Vec3::NEG_X,
                dist: -x0,
            },
            HalfSpace {
                normal: Vec3::NEG_Y,
                dist: -y0,
            },
            HalfSpace {
                normal: Vec3::Y,
                dist: y1,
            },
            HalfSpace {
                normal: Vec3::NEG_Z,
                dist: 0.0,
            },
            HalfSpace {
                normal: slope_n,
                dist: slope_d,
            },
        ];
        let faces = vec![
            // Sloped top deck
            (
                vec![
                    Vec3::new(x0, y0, height),
                    Vec3::new(x1, y0, 0.0),
                    Vec3::new(x1, y1, 0.0),
                    Vec3::new(x0, y1, height),
                ],
                top_tex,
            ),
            // -Y triangle side
            (
                vec![
                    Vec3::new(x0, y0, 0.0),
                    Vec3::new(x1, y0, 0.0),
                    Vec3::new(x0, y0, height),
                ],
                side_tex,
            ),
            // +Y triangle side
            (
                vec![
                    Vec3::new(x1, y1, 0.0),
                    Vec3::new(x0, y1, 0.0),
                    Vec3::new(x0, y1, height),
                ],
                side_tex,
            ),
        ];
        Self {
            planes,
            mins: Vec3::new(x0, y0, 0.0),
            maxs: Vec3::new(x1, y1, height),
            faces,
        }
    }

    /// Minkowski-expanded planes of this convex brush for a GoldSrc player hull box `[-e, +e]`,
    /// including axial bevel planes so sloped ramps never catch player corners.
    fn expanded_planes(&self, extents: Vec3) -> Vec<HalfSpace> {
        let mut out = Vec::with_capacity(self.planes.len() + 6);
        let push_unique = |list: &mut Vec<HalfSpace>, hs: HalfSpace| {
            if !list
                .iter()
                .any(|p| p.normal.dot(hs.normal) > 0.9999 && (p.dist - hs.dist).abs() < 1e-3)
            {
                list.push(hs);
            }
        };
        for p in &self.planes {
            let expand = p.normal.x.abs() * extents.x
                + p.normal.y.abs() * extents.y
                + p.normal.z.abs() * extents.z;
            push_unique(
                &mut out,
                HalfSpace {
                    normal: p.normal,
                    dist: p.dist + expand,
                },
            );
        }
        // Always include the 6 axial bevels of the expanded AABB (Quake/GoldSrc qbsp rule).
        let bmin = self.mins - extents;
        let bmax = self.maxs + extents;
        for hs in [
            HalfSpace {
                normal: Vec3::NEG_X,
                dist: -bmin.x,
            },
            HalfSpace {
                normal: Vec3::X,
                dist: bmax.x,
            },
            HalfSpace {
                normal: Vec3::NEG_Y,
                dist: -bmin.y,
            },
            HalfSpace {
                normal: Vec3::Y,
                dist: bmax.y,
            },
            HalfSpace {
                normal: Vec3::NEG_Z,
                dist: -bmin.z,
            },
            HalfSpace {
                normal: Vec3::Z,
                dist: bmax.z,
            },
        ] {
            push_unique(&mut out, hs);
        }
        out
    }

    /// Raycast intersection `t in (0, max_t]` against this solid convex brush (for shadow baking).
    fn ray_hit(&self, origin: Vec3, dir: Vec3, max_t: f32) -> bool {
        let mut t_enter = 0.0f32;
        let mut t_exit = max_t;
        for p in &self.planes {
            let denom = p.normal.dot(dir);
            let num = p.dist - p.normal.dot(origin);
            if denom.abs() < 1e-6 {
                if num < 0.0 {
                    return false;
                }
            } else {
                let t = num / denom;
                if denom > 0.0 {
                    // Exiting half-space
                    t_exit = t_exit.min(t);
                } else {
                    // Entering half-space
                    t_enter = t_enter.max(t);
                }
                if t_enter > t_exit {
                    return false;
                }
            }
        }
        t_exit > 0.05 && t_enter < max_t
    }
}

/// The 11 solid architectural & cover brushes of `dm_rusthouse` (shared by BSP and `.nav`).
pub(crate) fn dm_rusthouse_brushes() -> Vec<ConvexBrush> {
    vec![
        // 0. Central raised steel platform (bridge)
        ConvexBrush::from_box(
            Vec3::new(-128.0, -112.0, 0.0),
            Vec3::new(128.0, 112.0, BRIDGE_Z),
            TEX_RUSTTRIM,
            TEX_DECK,
        ),
        // 1. West walk-up ramp (CT side -> central bridge, 26.6 deg)
        ConvexBrush::ramp_rising_pos_x(
            -256.0,
            -128.0,
            -80.0,
            80.0,
            BRIDGE_Z,
            TEX_RUSTTRIM,
            TEX_DECK,
        ),
        // 2. East walk-up ramp (T side -> central bridge, 26.6 deg)
        ConvexBrush::ramp_rising_neg_x(128.0, 256.0, -80.0, 80.0, BRIDGE_Z, TEX_RUSTTRIM, TEX_DECK),
        // 3..6. Four symmetric tall military cover crates in the 4 quadrants
        ConvexBrush::from_box(
            Vec3::new(-368.0, -336.0, 0.0),
            Vec3::new(-272.0, -240.0, 96.0),
            TEX_CRATE,
            TEX_CRATE,
        ),
        ConvexBrush::from_box(
            Vec3::new(-368.0, 240.0, 0.0),
            Vec3::new(-272.0, 336.0, 96.0),
            TEX_CRATE,
            TEX_CRATE,
        ),
        ConvexBrush::from_box(
            Vec3::new(272.0, -336.0, 0.0),
            Vec3::new(368.0, -240.0, 96.0),
            TEX_CRATE,
            TEX_CRATE,
        ),
        ConvexBrush::from_box(
            Vec3::new(272.0, 240.0, 0.0),
            Vec3::new(368.0, 336.0, 96.0),
            TEX_CRATE,
            TEX_CRATE,
        ),
        // 7..8. Two tactical mid-flank crates (jumpable / crouch-cover)
        ConvexBrush::from_box(
            Vec3::new(-48.0, -512.0, 0.0),
            Vec3::new(48.0, -416.0, 56.0),
            TEX_CRATE,
            TEX_CRATE,
        ),
        ConvexBrush::from_box(
            Vec3::new(-48.0, 416.0, 0.0),
            Vec3::new(48.0, 512.0, 56.0),
            TEX_CRATE,
            TEX_CRATE,
        ),
        // 9..10. CT & T spawn protection walls (sandstone + rusted steel top)
        ConvexBrush::from_box(
            Vec3::new(-448.0, -96.0, 0.0),
            Vec3::new(-416.0, 96.0, 88.0),
            TEX_SANDWALL,
            TEX_RUSTTRIM,
        ),
        ConvexBrush::from_box(
            Vec3::new(416.0, -96.0, 0.0),
            Vec3::new(448.0, 96.0, 88.0),
            TEX_SANDWALL,
            TEX_RUSTTRIM,
        ),
    ]
}

pub(crate) type SpawnPair = (Vec3, f32);

/// Spawn positions `(origin, yaw_deg)` for CT (first 8) and T (second 8).
pub(crate) fn dm_rusthouse_spawns() -> (Vec<SpawnPair>, Vec<SpawnPair>) {
    let ct = vec![
        (Vec3::new(-608.0, -224.0, 40.0), 12.0),
        (Vec3::new(-608.0, 224.0, 40.0), -12.0),
        (Vec3::new(-608.0, -400.0, 40.0), 18.0),
        (Vec3::new(-608.0, 400.0, 40.0), -18.0),
        (Vec3::new(-608.0, -64.0, 40.0), 0.0),
        (Vec3::new(-608.0, 64.0, 40.0), 0.0),
        (Vec3::new(-528.0, -320.0, 40.0), 10.0),
        (Vec3::new(-528.0, 320.0, 40.0), -10.0),
    ];
    let t = vec![
        (Vec3::new(608.0, 224.0, 40.0), 168.0),
        (Vec3::new(608.0, -224.0, 40.0), 192.0),
        (Vec3::new(608.0, 400.0, 40.0), 162.0),
        (Vec3::new(608.0, -400.0, 40.0), 198.0),
        (Vec3::new(608.0, 64.0, 40.0), 180.0),
        (Vec3::new(608.0, -64.0, 40.0), 180.0),
        (Vec3::new(528.0, 320.0, 40.0), 170.0),
        (Vec3::new(528.0, -320.0, 40.0), 190.0),
    ];
    (ct, t)
}

/// Bounded convex cell represented by its boundary polygons for exact half-space splitting.
#[derive(Clone)]
struct ConvexCell {
    faces: Vec<Vec<Vec3>>,
}

impl ConvexCell {
    fn from_aabb(mins: Vec3, maxs: Vec3) -> Self {
        let (x0, y0, z0) = (mins.x, mins.y, mins.z);
        let (x1, y1, z1) = (maxs.x, maxs.y, maxs.z);
        Self {
            faces: vec![
                vec![
                    Vec3::new(x0, y0, z1),
                    Vec3::new(x1, y0, z1),
                    Vec3::new(x1, y1, z1),
                    Vec3::new(x0, y1, z1),
                ],
                vec![
                    Vec3::new(x0, y1, z0),
                    Vec3::new(x1, y1, z0),
                    Vec3::new(x1, y0, z0),
                    Vec3::new(x0, y0, z0),
                ],
                vec![
                    Vec3::new(x0, y1, z0),
                    Vec3::new(x0, y0, z0),
                    Vec3::new(x0, y0, z1),
                    Vec3::new(x0, y1, z1),
                ],
                vec![
                    Vec3::new(x1, y0, z0),
                    Vec3::new(x1, y1, z0),
                    Vec3::new(x1, y1, z1),
                    Vec3::new(x1, y0, z1),
                ],
                vec![
                    Vec3::new(x0, y0, z0),
                    Vec3::new(x1, y0, z0),
                    Vec3::new(x1, y0, z1),
                    Vec3::new(x0, y0, z1),
                ],
                vec![
                    Vec3::new(x1, y1, z0),
                    Vec3::new(x0, y1, z0),
                    Vec3::new(x0, y1, z1),
                    Vec3::new(x1, y1, z1),
                ],
            ],
        }
    }

    fn bounds(&self) -> (Vec3, Vec3) {
        let mut mn = Vec3::splat(f32::INFINITY);
        let mut mx = Vec3::splat(f32::NEG_INFINITY);
        for f in &self.faces {
            for &v in f {
                mn = mn.min(v);
                mx = mx.max(v);
            }
        }
        (mn, mx)
    }

    /// True if every vertex of this cell is inside (or on the boundary of) the convex brush.
    fn contained_in(&self, brush_planes: &[HalfSpace]) -> bool {
        const EPS: f32 = 1e-2;
        for p in brush_planes {
            for f in &self.faces {
                for &v in f {
                    if p.normal.dot(v) - p.dist > EPS {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// True if all vertices lie strictly outside (or on the outside of) at least one plane of `brush_planes`.
    fn disjoint_from(&self, brush_planes: &[HalfSpace]) -> bool {
        const EPS: f32 = 1e-2;
        for p in brush_planes {
            let mut all_out = true;
            for f in &self.faces {
                for &v in f {
                    if p.normal.dot(v) - p.dist < -EPS {
                        all_out = false;
                        break;
                    }
                }
                if !all_out {
                    break;
                }
            }
            if all_out {
                return true;
            }
        }
        false
    }

    /// Classify how plane `(normal, dist)` cuts this cell: `(has_front, has_back)`.
    fn classify_plane(&self, normal: Vec3, dist: f32) -> (bool, bool) {
        const EPS: f32 = 1e-2;
        let mut front = false;
        let mut back = false;
        for f in &self.faces {
            for &v in f {
                let d = normal.dot(v) - dist;
                if d > EPS {
                    front = true;
                } else if d < -EPS {
                    back = true;
                }
            }
        }
        (front, back)
    }

    /// Split this convex cell into `(front: d >= 0, back: d <= 0)`.
    fn split(&self, normal: Vec3, dist: f32) -> (ConvexCell, ConvexCell) {
        const EPS: f32 = 1e-3;
        let mut front_faces = Vec::new();
        let mut back_faces = Vec::new();
        let mut cap_pts: Vec<Vec3> = Vec::new();

        for poly in &self.faces {
            let mut f_poly = Vec::new();
            let mut b_poly = Vec::new();
            let n = poly.len();
            for i in 0..n {
                let a = poly[i];
                let b = poly[(i + 1) % n];
                let da = normal.dot(a) - dist;
                let db = normal.dot(b) - dist;

                if da >= -EPS {
                    f_poly.push(a);
                }
                if da <= EPS {
                    b_poly.push(a);
                }
                if da.abs() <= EPS {
                    push_unique_pt(&mut cap_pts, a);
                }

                if (da > EPS && db < -EPS) || (da < -EPS && db > EPS) {
                    let t = (da / (da - db)).clamp(0.0, 1.0);
                    let mid = a + (b - a) * t;
                    f_poly.push(mid);
                    b_poly.push(mid);
                    push_unique_pt(&mut cap_pts, mid);
                }
            }
            dedup_poly(&mut f_poly);
            dedup_poly(&mut b_poly);
            if f_poly.len() >= 3 {
                front_faces.push(f_poly);
            }
            if b_poly.len() >= 3 {
                back_faces.push(b_poly);
            }
        }

        if cap_pts.len() >= 3 {
            let ordered = order_coplanar_ccw(&cap_pts, normal);
            let mut rev = ordered.clone();
            rev.reverse();
            front_faces.push(rev);
            back_faces.push(ordered);
        }

        (
            ConvexCell { faces: front_faces },
            ConvexCell { faces: back_faces },
        )
    }
}

fn push_unique_pt(list: &mut Vec<Vec3>, p: Vec3) {
    if !list.iter().any(|q| q.distance_squared(p) < 1e-3) {
        list.push(p);
    }
}

fn dedup_poly(poly: &mut Vec<Vec3>) {
    let mut out: Vec<Vec3> = Vec::with_capacity(poly.len());
    for &p in poly.iter() {
        if out.last().is_none_or(|q| q.distance_squared(p) > 1e-4) {
            out.push(p);
        }
    }
    if out.len() >= 2 && out.first().unwrap().distance_squared(*out.last().unwrap()) <= 1e-4 {
        out.pop();
    }
    *poly = out;
}

fn order_coplanar_ccw(pts: &[Vec3], normal: Vec3) -> Vec<Vec3> {
    let center = pts.iter().copied().fold(Vec3::ZERO, |a, b| a + b) / (pts.len() as f32);
    let ref_axis = if normal.z.abs() < 0.9 {
        Vec3::Z
    } else {
        Vec3::X
    };
    let u = normal.cross(ref_axis).normalize();
    let v = normal.cross(u).normalize();
    let mut indexed: Vec<(f32, Vec3)> = pts
        .iter()
        .map(|&p| {
            let rel = p - center;
            (rel.dot(v).atan2(rel.dot(u)), p)
        })
        .collect();
    indexed.sort_by(|a, b| a.0.total_cmp(&b.0));
    indexed.into_iter().map(|(_, p)| p).collect()
}

/// Canonical GoldSrc plane table ensuring `plane_type` 0/1/2 ALWAYS have positive `+X/+Y/+Z` normal.
#[derive(Default)]
struct PlaneTable {
    planes: Vec<Plane>,
}

impl PlaneTable {
    /// Intern a plane, returning `(plane_index, flipped)` where `flipped` is true when the
    /// stored canonical plane has the opposite normal of `(normal, dist)`.
    fn intern(&mut self, normal: Vec3, dist: f32) -> (usize, bool) {
        let n = normal.normalize();
        // Canonicalize: exact axial planes use +X/+Y/+Z; non-axial planes keep largest component > 0.
        let (cn, cd, ptype, flipped) = if n.x.abs() > 0.9999 {
            if n.x >= 0.0 {
                (Vec3::X, dist, 0, false)
            } else {
                (Vec3::X, -dist, 0, true)
            }
        } else if n.y.abs() > 0.9999 {
            if n.y >= 0.0 {
                (Vec3::Y, dist, 1, false)
            } else {
                (Vec3::Y, -dist, 1, true)
            }
        } else if n.z.abs() > 0.9999 {
            if n.z >= 0.0 {
                (Vec3::Z, dist, 2, false)
            } else {
                (Vec3::Z, -dist, 2, true)
            }
        } else {
            let ax = n.x.abs();
            let ay = n.y.abs();
            let az = n.z.abs();
            let (lead_neg, ptype) = if ax >= ay && ax >= az {
                (n.x < 0.0, 3)
            } else if ay >= ax && ay >= az {
                (n.y < 0.0, 4)
            } else {
                (n.z < 0.0, 5)
            };
            if lead_neg {
                (-n, -dist, ptype, true)
            } else {
                (n, dist, ptype, false)
            }
        };

        for (idx, p) in self.planes.iter().enumerate() {
            if p.plane_type == ptype && p.normal.dot(cn) > 0.9999 && (p.dist - cd).abs() < 1e-2 {
                return (idx, flipped);
            }
        }
        let idx = self.planes.len();
        self.planes.push(Plane {
            normal: cn,
            dist: cd,
            plane_type: ptype,
        });
        (idx, flipped)
    }
}

/// Visual world face prior to BSP serialization.
struct RawWorldFace {
    verts: Vec<Vec3>,
    normal: Vec3,
    dist: f32,
    tex_idx: usize,
    is_sky: bool,
}

/// Build the complete clean-room GoldSrc BSP v30 binary for `maps/dm_rusthouse.bsp`.
pub fn build_dm_rusthouse_bsp() -> Vec<u8> {
    let miptex_list = build_arena_miptex_list();
    let brushes = dm_rusthouse_brushes();
    let (ct_spawns, t_spawns) = dm_rusthouse_spawns();

    let mut planes = PlaneTable::default();
    let mut nodes: Vec<Node> = Vec::new();
    let mut clipnodes: Vec<ClipNode> = Vec::new();
    let mut leafs: Vec<Leaf> = vec![
        // Leaf 0 is always GoldSrc's shared solid leaf (`vis_offset = -1`).
        Leaf {
            contents: CONTENTS_SOLID,
            vis_offset: -1,
            mins: [0; 3],
            maxs: [0; 3],
            first_mark_surface: 0,
            num_mark_surfaces: 0,
            ambient_levels: [0; 4],
        },
        // Leaf 1 is the sky leaf (`CONTENTS_SKY`) for Hull 0 traces hitting above ceiling.
        Leaf {
            contents: CONTENTS_SKY,
            vis_offset: -1,
            mins: [0; 3],
            maxs: [0; 3],
            first_mark_surface: 0,
            num_mark_surfaces: 0,
            ambient_levels: [0; 4],
        },
    ];

    // 1. Collect all visual faces of the world (floor tiles, walls, sky cap, and brush faces).
    let world_raw_faces = collect_world_faces(&brushes);

    // 2. Build Hull 0 BSP (`nodes` + `leafs`) wrapping the interior room and all 11 solid brushes.
    let hull0_brushes: Vec<Vec<HalfSpace>> = brushes
        .iter()
        .map(|b| b.expanded_planes(HULL_EXTENTS[0]))
        .collect();
    let interior0 = ConvexCell::from_aabb(
        Vec3::new(-ARENA_HALF, -ARENA_HALF, 0.0),
        Vec3::new(ARENA_HALF, ARENA_HALF, ARENA_HEIGHT),
    );
    let inner_root0 = build_node_tree(
        &interior0,
        &hull0_brushes,
        &mut planes,
        &mut nodes,
        &mut leafs,
        0,
    );
    // Wrap interior with the 6 outer room boundary planes (z=0 floor, z=320 sky, x=±768, y=±768).
    let world_head_node0 = wrap_room_nodes(inner_root0, HULL_EXTENTS[0], &mut planes, &mut nodes);

    // 3. Build Hull 1, 2, 3 ClipNode trees (`clipnodes`).
    let mut world_head_clip = [CONTENTS_EMPTY; 3];
    for h in 1..=3 {
        let ext = HULL_EXTENTS[h];
        let hull_brushes: Vec<Vec<HalfSpace>> =
            brushes.iter().map(|b| b.expanded_planes(ext)).collect();
        let room_min = Vec3::new(-ARENA_HALF + ext.x, -ARENA_HALF + ext.y, ext.z);
        let room_max = Vec3::new(ARENA_HALF - ext.x, ARENA_HALF - ext.y, ARENA_HEIGHT - ext.z);
        let interior = ConvexCell::from_aabb(room_min, room_max);
        let inner_clip = build_clip_tree(&interior, &hull_brushes, &mut planes, &mut clipnodes, 0);
        world_head_clip[h - 1] =
            wrap_room_clipnodes(inner_clip, room_min, room_max, &mut planes, &mut clipnodes);
    }

    // 4. Build submodels `*1` (CT func_buyzone) and `*2` (T func_buyzone).
    let ct_buy_min = Vec3::new(-720.0, -560.0, 0.0);
    let ct_buy_max = Vec3::new(-480.0, 560.0, 128.0);
    let t_buy_min = Vec3::new(480.0, -560.0, 0.0);
    let t_buy_max = Vec3::new(720.0, 560.0, 128.0);
    let ct_buy_heads = build_box_submodel_hulls(
        ct_buy_min,
        ct_buy_max,
        &mut planes,
        &mut nodes,
        &mut clipnodes,
        &mut leafs,
    );
    let t_buy_heads = build_box_submodel_hulls(
        t_buy_min,
        t_buy_max,
        &mut planes,
        &mut nodes,
        &mut clipnodes,
        &mut leafs,
    );

    // 5. Assign each visual world face to a matching splitting node so `R_LightPoint` and
    //    `WorldCollision::trace_texture` find the exact surface on vertical/line traces!
    let mut node_face_buckets: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut unassigned_faces: Vec<usize> = Vec::new();
    let mut face_plane_ids: Vec<(usize, bool)> = Vec::with_capacity(world_raw_faces.len());

    for (fi, rf) in world_raw_faces.iter().enumerate() {
        let (pidx, flipped) = planes.intern(rf.normal, rf.dist);
        face_plane_ids.push((pidx, flipped));
        if let Some(nidx) = nodes.iter().position(|n| n.plane as usize == pidx) {
            node_face_buckets[nidx].push(fi);
        } else {
            unassigned_faces.push(fi);
        }
    }

    // 6. Emit vertices, edges, surf_edges, texinfos, faces (ordered per node!), and baked RGB lightmaps.
    let mut vertices: Vec<Vec3> = Vec::new();
    let mut edges: Vec<[u16; 2]> = vec![[0, 0]]; // Edge 0 is unused in GoldSrc (surfedge signedness!)
    let mut surf_edges: Vec<i32> = Vec::new();
    let mut texinfos: Vec<TexInfo> = Vec::new();
    let mut faces: Vec<Face> = Vec::with_capacity(world_raw_faces.len());
    let mut lighting: Vec<u8> = Vec::new();

    let mut emit_face = |fi: usize, faces: &mut Vec<Face>| {
        let rf = &world_raw_faces[fi];
        let (pidx, flipped) = face_plane_ids[fi];
        let ti_idx = intern_texinfo(&mut texinfos, rf.normal, rf.tex_idx, rf.is_sky);
        let first_edge = surf_edges.len() as i32;
        let num_edges = rf.verts.len() as u16;

        let mut v_indices = Vec::with_capacity(rf.verts.len());
        for &v in &rf.verts {
            v_indices.push(intern_vertex(&mut vertices, v));
        }
        for i in 0..v_indices.len() {
            let a = v_indices[i];
            let b = v_indices[(i + 1) % v_indices.len()];
            let edge_idx = edges.len() as i32;
            edges.push([a, b]);
            surf_edges.push(edge_idx);
        }

        let (styles, lm_off) = if rf.is_sky {
            ([255u8; 4], -1i32)
        } else {
            let off = bake_face_lightmap(
                &rf.verts,
                rf.normal,
                &texinfos[ti_idx],
                &brushes,
                &mut lighting,
            );
            ([0, 255, 255, 255], off)
        };

        faces.push(Face {
            plane: pidx as u16,
            plane_side: u16::from(flipped),
            first_edge,
            num_edges,
            texinfo: ti_idx as u16,
            styles,
            lightmap_offset: lm_off,
        });
    };

    for (nidx, bucket) in node_face_buckets.iter().enumerate() {
        if !bucket.is_empty() {
            nodes[nidx].first_face = faces.len() as u16;
            nodes[nidx].num_faces = bucket.len() as u16;
            for &fi in bucket {
                emit_face(fi, &mut faces);
            }
        }
    }
    for &fi in &unassigned_faces {
        emit_face(fi, &mut faces);
    }

    let world_num_faces = faces.len();

    // 7. Populate `mark_surfaces` and `LUMP_VISIBILITY` (all-visible courtyard PVS).
    let mark_surfaces: Vec<u16> = (0..world_num_faces as u16).collect();
    let vis_leafs = leafs.len().saturating_sub(1);
    for leaf in leafs.iter_mut().skip(1) {
        if leaf.contents == CONTENTS_EMPTY {
            leaf.vis_offset = 0;
            leaf.first_mark_surface = 0;
            leaf.num_mark_surfaces = world_num_faces as u16;
        }
    }
    let vis_bytes_len = vis_leafs.div_ceil(8).max(1);
    let visibility = vec![0xFFu8; vis_bytes_len];

    // 8. Build `LUMP_ENTITIES` string.
    let entities_str = build_entities_string(&ct_spawns, &t_spawns);

    // 9. Build `LUMP_TEXTURES` (embedded MipTex blocks).
    let textures_lump = build_textures_lump(&miptex_list);

    // 10. Serialize all 15 lumps into a valid GoldSrc BSP v30 byte stream.
    let models = vec![
        BspModelSpec {
            mins: Vec3::new(-ARENA_HALF, -ARENA_HALF, 0.0),
            maxs: Vec3::new(ARENA_HALF, ARENA_HALF, ARENA_HEIGHT),
            origin: Vec3::ZERO,
            head_nodes: [
                world_head_node0,
                world_head_clip[0],
                world_head_clip[1],
                world_head_clip[2],
            ],
            vis_leafs: vis_leafs as i32,
            first_face: 0,
            num_faces: world_num_faces as i32,
        },
        BspModelSpec {
            mins: ct_buy_min,
            maxs: ct_buy_max,
            origin: Vec3::ZERO,
            head_nodes: ct_buy_heads,
            vis_leafs: 0,
            first_face: 0,
            num_faces: 0,
        },
        BspModelSpec {
            mins: t_buy_min,
            maxs: t_buy_max,
            origin: Vec3::ZERO,
            head_nodes: t_buy_heads,
            vis_leafs: 0,
            first_face: 0,
            num_faces: 0,
        },
    ];

    serialize_bsp_v30(
        entities_str.as_bytes(),
        &planes.planes,
        &textures_lump,
        &vertices,
        &visibility,
        &nodes,
        &texinfos,
        &faces,
        &lighting,
        &clipnodes,
        &leafs,
        &mark_surfaces,
        &edges,
        &surf_edges,
        &models,
    )
}

fn collect_world_faces(brushes: &[ConvexBrush]) -> Vec<RawWorldFace> {
    let mut out = Vec::new();
    let push_quad = |list: &mut Vec<RawWorldFace>, verts: Vec<Vec3>, tex: usize, is_sky: bool| {
        let e1 = verts[1] - verts[0];
        let e2 = verts[2] - verts[0];
        let normal = e1.cross(e2).normalize();
        let dist = normal.dot(verts[0]);
        list.push(RawWorldFace {
            verts,
            normal,
            dist,
            tex_idx: tex,
            is_sky,
        });
    };

    // Subdivided 6x6 floor (256x256 quads -> 17x17 luxels each for crisp shadows!)
    let tile = 256.0f32;
    for gy in 0..6 {
        let y0 = -ARENA_HALF + gy as f32 * tile;
        let y1 = y0 + tile;
        for gx in 0..6 {
            let x0 = -ARENA_HALF + gx as f32 * tile;
            let x1 = x0 + tile;
            let tex = if gx == 0 {
                TEX_SPAWNCT
            } else if gx == 5 {
                TEX_SPAWNT
            } else {
                TEX_FLOOR
            };
            push_quad(
                &mut out,
                vec![
                    Vec3::new(x0, y0, 0.0),
                    Vec3::new(x1, y0, 0.0),
                    Vec3::new(x1, y1, 0.0),
                    Vec3::new(x0, y1, 0.0),
                ],
                tex,
                false,
            );
        }
    }

    // Perimeter walls: sandstone brick [0..224], rusted steel crown beam [224..256], sky [256..320].
    let wall_bands: [(f32, f32, usize, bool); 3] = [
        (0.0, 224.0, TEX_SANDWALL, false),
        (224.0, WALL_HEIGHT, TEX_RUSTTRIM, false),
        (WALL_HEIGHT, ARENA_HEIGHT, TEX_SKY, true),
    ];
    for i in 0..6 {
        let a0 = -ARENA_HALF + i as f32 * tile;
        let a1 = a0 + tile;
        for &(z0, z1, tex, is_sky) in &wall_bands {
            // West wall (x = -768, inward normal = +X)
            push_quad(
                &mut out,
                vec![
                    Vec3::new(-ARENA_HALF, a0, z0),
                    Vec3::new(-ARENA_HALF, a1, z0),
                    Vec3::new(-ARENA_HALF, a1, z1),
                    Vec3::new(-ARENA_HALF, a0, z1),
                ],
                tex,
                is_sky,
            );
            // East wall (x = +768, inward normal = -X)
            push_quad(
                &mut out,
                vec![
                    Vec3::new(ARENA_HALF, a1, z0),
                    Vec3::new(ARENA_HALF, a0, z0),
                    Vec3::new(ARENA_HALF, a0, z1),
                    Vec3::new(ARENA_HALF, a1, z1),
                ],
                tex,
                is_sky,
            );
            // North wall (y = -768, inward normal = +Y)
            push_quad(
                &mut out,
                vec![
                    Vec3::new(a1, -ARENA_HALF, z0),
                    Vec3::new(a0, -ARENA_HALF, z0),
                    Vec3::new(a0, -ARENA_HALF, z1),
                    Vec3::new(a1, -ARENA_HALF, z1),
                ],
                tex,
                is_sky,
            );
            // South wall (y = +768, inward normal = -Y)
            push_quad(
                &mut out,
                vec![
                    Vec3::new(a0, ARENA_HALF, z0),
                    Vec3::new(a1, ARENA_HALF, z0),
                    Vec3::new(a1, ARENA_HALF, z1),
                    Vec3::new(a0, ARENA_HALF, z1),
                ],
                tex,
                is_sky,
            );
        }
    }

    // Open sky ceiling (z = 320, inward normal = -Z)
    push_quad(
        &mut out,
        vec![
            Vec3::new(-ARENA_HALF, ARENA_HALF, ARENA_HEIGHT),
            Vec3::new(ARENA_HALF, ARENA_HALF, ARENA_HEIGHT),
            Vec3::new(ARENA_HALF, -ARENA_HALF, ARENA_HEIGHT),
            Vec3::new(-ARENA_HALF, -ARENA_HALF, ARENA_HEIGHT),
        ],
        TEX_SKY,
        true,
    );

    // Brush faces
    for b in brushes {
        for (verts, tex) in &b.faces {
            push_quad(&mut out, verts.clone(), *tex, false);
        }
    }

    out
}

fn choose_split_plane(cell: &ConvexCell, brushes: &[Vec<HalfSpace>]) -> Option<HalfSpace> {
    let mut best: Option<(i32, HalfSpace)> = None;
    for b in brushes {
        for &hs in b {
            let (has_f, has_b) = cell.classify_plane(hs.normal, hs.dist);
            if !(has_f && has_b) {
                continue;
            }
            // Prefer axial splits that cleanly separate brushes with minimal brush splitting.
            let is_axial =
                hs.normal.x.abs() > 0.999 || hs.normal.y.abs() > 0.999 || hs.normal.z.abs() > 0.999;
            let mut b_only = 0i32;
            let mut both = 0i32;
            for ob in brushes {
                // Quick approximate classification by brush planes vs candidate plane
                let mut same_as_hs = false;
                for op in ob {
                    if op.normal.dot(hs.normal) > 0.999 && (op.dist - hs.dist).abs() < 1e-2 {
                        same_as_hs = true;
                        break;
                    }
                }
                if same_as_hs {
                    b_only += 1;
                } else {
                    both += 1;
                }
            }
            let score = if is_axial { 100 } else { 0 } + b_only * 10 - both;
            if best.is_none_or(|(bs, _)| score > bs) {
                best = Some((score, hs));
            }
        }
    }
    best.map(|(_, hs)| hs)
}

fn clamp_i16_vec(v: Vec3) -> [i16; 3] {
    [
        v.x.round().clamp(-32000.0, 32000.0) as i16,
        v.y.round().clamp(-32000.0, 32000.0) as i16,
        v.z.round().clamp(-32000.0, 32000.0) as i16,
    ]
}

fn build_node_tree(
    cell: &ConvexCell,
    brushes: &[Vec<HalfSpace>],
    planes: &mut PlaneTable,
    nodes: &mut Vec<Node>,
    leafs: &mut Vec<Leaf>,
    depth: usize,
) -> i32 {
    if brushes.is_empty() || depth >= 48 {
        let (mn, mx) = cell.bounds();
        let leaf_idx = leafs.len();
        leafs.push(Leaf {
            contents: CONTENTS_EMPTY,
            vis_offset: 0,
            mins: clamp_i16_vec(mn),
            maxs: clamp_i16_vec(mx),
            first_mark_surface: 0,
            num_mark_surfaces: 0,
            ambient_levels: [0; 4],
        });
        return -1 - (leaf_idx as i32);
    }
    if brushes.iter().any(|b| cell.contained_in(b)) {
        return -1; // Leaf 0 = CONTENTS_SOLID
    }
    let Some(split) = choose_split_plane(cell, brushes) else {
        let (mn, mx) = cell.bounds();
        let leaf_idx = leafs.len();
        leafs.push(Leaf {
            contents: CONTENTS_EMPTY,
            vis_offset: 0,
            mins: clamp_i16_vec(mn),
            maxs: clamp_i16_vec(mx),
            first_mark_surface: 0,
            num_mark_surfaces: 0,
            ambient_levels: [0; 4],
        });
        return -1 - (leaf_idx as i32);
    };

    let (pidx, flipped) = planes.intern(split.normal, split.dist);
    let cp = planes.planes[pidx];
    let (front_cell, back_cell) = cell.split(cp.normal, cp.dist);
    let _ = flipped;

    let front_brushes: Vec<Vec<HalfSpace>> = brushes
        .iter()
        .filter(|b| !front_cell.disjoint_from(b))
        .cloned()
        .collect();
    let back_brushes: Vec<Vec<HalfSpace>> = brushes
        .iter()
        .filter(|b| !back_cell.disjoint_from(b))
        .cloned()
        .collect();

    let front_child = build_node_tree(&front_cell, &front_brushes, planes, nodes, leafs, depth + 1);
    let back_child = build_node_tree(&back_cell, &back_brushes, planes, nodes, leafs, depth + 1);
    if front_child == -1 && back_child == -1 {
        return -1;
    }

    let (mn, mx) = cell.bounds();
    let idx = nodes.len() as i32;
    nodes.push(Node {
        plane: pidx as u32,
        children: [front_child as i16, back_child as i16],
        mins: clamp_i16_vec(mn),
        maxs: clamp_i16_vec(mx),
        first_face: 0,
        num_faces: 0,
    });
    idx
}

fn wrap_room_nodes(
    mut child: i32,
    ext: Vec3,
    planes: &mut PlaneTable,
    nodes: &mut Vec<Node>,
) -> i32 {
    let mn = Vec3::new(-ARENA_HALF + ext.x, -ARENA_HALF + ext.y, ext.z);
    let mx = Vec3::new(ARENA_HALF - ext.x, ARENA_HALF - ext.y, ARENA_HEIGHT - ext.z);
    let mn_i = clamp_i16_vec(mn);
    let mx_i = clamp_i16_vec(mx);

    // Order from innermost wrapper to outermost:
    // floor (z >= 0) -> solid (-1) below; ceiling (z <= 320) -> sky (-2) above;
    // walls x=-768, x=+768, y=-768, y=+768 -> solid (-1) outside.
    let bounds: [(Vec3, f32, bool, i16); 6] = [
        (Vec3::Z, mn.z, true, -1), // front (>= mn.z) is inside, back is solid leaf 0
        (Vec3::Z, mx.z, false, -2), // back (<= mx.z) is inside, front is sky leaf 1
        (Vec3::X, mn.x, true, -1),
        (Vec3::X, mx.x, false, -1),
        (Vec3::Y, mn.y, true, -1),
        (Vec3::Y, mx.y, false, -1),
    ];
    for (normal, dist, inside_is_front, outside_leaf) in bounds {
        let (pidx, _) = planes.intern(normal, dist);
        let children = if inside_is_front {
            [child as i16, outside_leaf]
        } else {
            [outside_leaf, child as i16]
        };
        let nidx = nodes.len() as i32;
        nodes.push(Node {
            plane: pidx as u32,
            children,
            mins: mn_i,
            maxs: mx_i,
            first_face: 0,
            num_faces: 0,
        });
        child = nidx;
    }
    child
}

fn build_clip_tree(
    cell: &ConvexCell,
    brushes: &[Vec<HalfSpace>],
    planes: &mut PlaneTable,
    clipnodes: &mut Vec<ClipNode>,
    depth: usize,
) -> i32 {
    if brushes.is_empty() || depth >= 48 {
        return CONTENTS_EMPTY;
    }
    if brushes.iter().any(|b| cell.contained_in(b)) {
        return CONTENTS_SOLID;
    }
    let Some(split) = choose_split_plane(cell, brushes) else {
        return CONTENTS_EMPTY;
    };

    let (pidx, _) = planes.intern(split.normal, split.dist);
    let cp = planes.planes[pidx];
    let (front_cell, back_cell) = cell.split(cp.normal, cp.dist);

    let front_brushes: Vec<Vec<HalfSpace>> = brushes
        .iter()
        .filter(|b| !front_cell.disjoint_from(b))
        .cloned()
        .collect();
    let back_brushes: Vec<Vec<HalfSpace>> = brushes
        .iter()
        .filter(|b| !back_cell.disjoint_from(b))
        .cloned()
        .collect();

    let front_child = build_clip_tree(&front_cell, &front_brushes, planes, clipnodes, depth + 1);
    let back_child = build_clip_tree(&back_cell, &back_brushes, planes, clipnodes, depth + 1);
    if front_child == back_child && front_child < 0 {
        return front_child;
    }

    let idx = clipnodes.len() as i32;
    clipnodes.push(ClipNode {
        plane: pidx as i32,
        children: [front_child as i16, back_child as i16],
    });
    idx
}

fn wrap_room_clipnodes(
    mut child: i32,
    mn: Vec3,
    mx: Vec3,
    planes: &mut PlaneTable,
    clipnodes: &mut Vec<ClipNode>,
) -> i32 {
    let bounds: [(Vec3, f32, bool); 6] = [
        (Vec3::Z, mn.z, true),
        (Vec3::Z, mx.z, false),
        (Vec3::X, mn.x, true),
        (Vec3::X, mx.x, false),
        (Vec3::Y, mn.y, true),
        (Vec3::Y, mx.y, false),
    ];
    for (normal, dist, inside_is_front) in bounds {
        let (pidx, _) = planes.intern(normal, dist);
        let children = if inside_is_front {
            [child as i16, CONTENTS_SOLID as i16]
        } else {
            [CONTENTS_SOLID as i16, child as i16]
        };
        let idx = clipnodes.len() as i32;
        clipnodes.push(ClipNode {
            plane: pidx as i32,
            children,
        });
        child = idx;
    }
    child
}

/// Build a 6-plane box submodel (`func_buyzone`) across Hulls 0, 1, 2, 3.
fn build_box_submodel_hulls(
    mins: Vec3,
    maxs: Vec3,
    planes: &mut PlaneTable,
    nodes: &mut Vec<Node>,
    clipnodes: &mut Vec<ClipNode>,
    leafs: &mut Vec<Leaf>,
) -> [i32; 4] {
    let empty_leaf_idx = leafs.len();
    leafs.push(Leaf {
        contents: CONTENTS_EMPTY,
        vis_offset: -1,
        mins: clamp_i16_vec(mins),
        maxs: clamp_i16_vec(maxs),
        first_mark_surface: 0,
        num_mark_surfaces: 0,
        ambient_levels: [0; 4],
    });
    let empty_leaf = -1 - (empty_leaf_idx as i32);

    // Hull 0 node chain: inside all 6 planes => solid (-1), outside any => empty_leaf.
    let mut head0 = -1i32;
    let mn_i = clamp_i16_vec(mins);
    let mx_i = clamp_i16_vec(maxs);
    for (normal, dist, inside_is_front) in [
        (Vec3::Z, mins.z, true),
        (Vec3::Z, maxs.z, false),
        (Vec3::Y, mins.y, true),
        (Vec3::Y, maxs.y, false),
        (Vec3::X, mins.x, true),
        (Vec3::X, maxs.x, false),
    ] {
        let (pidx, _) = planes.intern(normal, dist);
        let children = if inside_is_front {
            [head0 as i16, empty_leaf as i16]
        } else {
            [empty_leaf as i16, head0 as i16]
        };
        let idx = nodes.len() as i32;
        nodes.push(Node {
            plane: pidx as u32,
            children,
            mins: mn_i,
            maxs: mx_i,
            first_face: 0,
            num_faces: 0,
        });
        head0 = idx;
    }

    let mut heads = [head0, 0, 0, 0];
    for h in 1..=3 {
        let e = HULL_EXTENTS[h];
        let bmin = mins - e;
        let bmax = maxs + e;
        let mut cur = CONTENTS_SOLID;
        for (normal, dist, inside_is_front) in [
            (Vec3::Z, bmin.z, true),
            (Vec3::Z, bmax.z, false),
            (Vec3::Y, bmin.y, true),
            (Vec3::Y, bmax.y, false),
            (Vec3::X, bmin.x, true),
            (Vec3::X, bmax.x, false),
        ] {
            let (pidx, _) = planes.intern(normal, dist);
            let children = if inside_is_front {
                [cur as i16, CONTENTS_EMPTY as i16]
            } else {
                [CONTENTS_EMPTY as i16, cur as i16]
            };
            let idx = clipnodes.len() as i32;
            clipnodes.push(ClipNode {
                plane: pidx as i32,
                children,
            });
            cur = idx;
        }
        heads[h] = cur;
    }
    heads
}

fn intern_vertex(verts: &mut Vec<Vec3>, p: Vec3) -> u16 {
    for (i, q) in verts.iter().enumerate() {
        if q.distance_squared(p) < 1e-4 {
            return i as u16;
        }
    }
    let idx = verts.len() as u16;
    verts.push(p);
    idx
}

fn intern_texinfo(
    texinfos: &mut Vec<TexInfo>,
    normal: Vec3,
    tex_idx: usize,
    is_sky: bool,
) -> usize {
    let (s_vec, t_vec) = if normal.z.abs() > 0.7 {
        // Horizontal or walkable sloped ramp -> project cleanly in XY!
        (Vec3::X, Vec3::NEG_Y)
    } else if normal.x.abs() > normal.y.abs() {
        (Vec3::Y, Vec3::NEG_Z)
    } else {
        (Vec3::X, Vec3::NEG_Z)
    };
    let flags = i32::from(is_sky);
    for (i, ti) in texinfos.iter().enumerate() {
        if ti.miptex == tex_idx as i32
            && ti.flags == flags
            && ti.s_vec.dot(s_vec) > 0.999
            && ti.t_vec.dot(t_vec) > 0.999
        {
            return i;
        }
    }
    let idx = texinfos.len();
    texinfos.push(TexInfo {
        s_vec,
        s_shift: 0.0,
        t_vec,
        t_shift: 0.0,
        miptex: tex_idx as i32,
        flags,
    });
    idx
}

/// Bake crisp GoldSrc directional warm sunlight + 4-sample soft shadows + sky ambient fill
/// for a face, returning its byte offset in `lighting`.
fn bake_face_lightmap(
    verts: &[Vec3],
    normal: Vec3,
    ti: &TexInfo,
    brushes: &[ConvexBrush],
    lighting: &mut Vec<u8>,
) -> i32 {
    let mut min_s = f32::INFINITY;
    let mut max_s = f32::NEG_INFINITY;
    let mut min_t = f32::INFINITY;
    let mut max_t = f32::NEG_INFINITY;
    for &p in verts {
        let s = p.dot(ti.s_vec) + ti.s_shift;
        let t = p.dot(ti.t_vec) + ti.t_shift;
        min_s = min_s.min(s);
        max_s = max_s.max(s);
        min_t = min_t.min(t);
        max_t = max_t.max(t);
    }
    let ext_min_s = (min_s / 16.0).floor();
    let ext_max_s = (max_s / 16.0).ceil();
    let ext_min_t = (min_t / 16.0).floor();
    let ext_max_t = (max_t / 16.0).ceil();
    let lm_w = ((ext_max_s - ext_min_s) as i32 + 1).clamp(1, 256) as usize;
    let lm_h = ((ext_max_t - ext_min_t) as i32 + 1).clamp(1, 256) as usize;

    // Solve 3D world point from `(s, t)` on the face plane `normal · p = plane_dist`.
    let plane_dist = normal.dot(verts[0]);
    let s_dir = ti.s_vec;
    let t_dir = ti.t_vec;
    // Project onto plane via Cramer's rule / orthonormal frame since s_dir ⟂ t_dir and both unit.
    let n_s = normal.dot(s_dir);
    let n_t = normal.dot(t_dir);
    let w_dir = s_dir.cross(t_dir);
    let n_w = normal.dot(w_dir);

    let sun_dir = Vec3::new(0.45, -0.35, 0.82).normalize();
    let sun_t1 = sun_dir.cross(Vec3::Z).normalize_or_zero();
    let sun_t2 = sun_dir.cross(sun_t1).normalize_or_zero();
    let jitter_dirs = [
        sun_dir,
        (sun_dir + sun_t1 * 0.045 + sun_t2 * 0.03).normalize(),
        (sun_dir - sun_t1 * 0.045 + sun_t2 * 0.03).normalize(),
        (sun_dir - sun_t2 * 0.045).normalize(),
    ];

    let ndotl = normal.dot(sun_dir).max(0.0);
    // Warm bounce on shadow faces so nothing ever looks flat or muddy.
    let hemi = (0.55 + 0.45 * (normal.z * 0.5 + 0.5)).clamp(0.45, 1.0);
    let offset = lighting.len() as i32;
    lighting.reserve(lm_w * lm_h * 3);

    for ty in 0..lm_h {
        let t = (ext_min_t + ty as f32) * 16.0;
        for tx in 0..lm_w {
            let s = (ext_min_s + tx as f32) * 16.0;
            let w_val = if n_w.abs() > 1e-4 {
                (plane_dist - n_s * s - n_t * t) / n_w
            } else {
                0.0
            };
            let world_p = s_dir * s + t_dir * t + w_dir * w_val;
            let ray_origin = world_p + normal * 2.5;

            let mut vis = 0.0f32;
            if ndotl > 0.001 {
                for &sd in &jitter_dirs {
                    let mut blocked = false;
                    // Check against outer courtyard rim walls (up to WALL_HEIGHT = 256).
                    if hits_outer_wall(ray_origin, sd) {
                        blocked = true;
                    } else {
                        for b in brushes {
                            if b.ray_hit(ray_origin, sd, 2400.0) {
                                blocked = true;
                                break;
                            }
                        }
                    }
                    if !blocked {
                        vis += 0.25;
                    }
                }
            }

            // Subtle corner/ground ambient occlusion for tactile depth
            let edge_dist = (ARENA_HALF - world_p.x.abs()).min(ARENA_HALF - world_p.y.abs());
            let wall_ao = (0.84 + 0.16 * (edge_dist / 72.0).clamp(0.0, 1.0)).min(1.0);
            let floor_ao = if normal.z.abs() < 0.5 {
                0.86 + 0.14 * (world_p.z / 48.0).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let ao = wall_ao * floor_ao;

            // Sky ambient + warm golden direct sunlight (calibrated for GoldSrc gamma LUTs!)
            let r = (102.0 * hemi * ao + 158.0 * ndotl * vis).clamp(58.0, 246.0) as u8;
            let g = (112.0 * hemi * ao + 142.0 * ndotl * vis).clamp(62.0, 242.0) as u8;
            let b = (128.0 * hemi * ao + 112.0 * ndotl * vis).clamp(68.0, 232.0) as u8;
            lighting.push(r);
            lighting.push(g);
            lighting.push(b);
        }
    }
    offset
}

fn hits_outer_wall(origin: Vec3, dir: Vec3) -> bool {
    let check_axis = |pos: f32, d: f32, limit: f32| -> bool {
        if d.abs() < 1e-5 {
            return false;
        }
        let t = (limit - pos) / d;
        if t > 1.0 {
            let z = origin.z + dir.z * t;
            return (0.0..=WALL_HEIGHT).contains(&z);
        }
        false
    };
    if dir.x > 0.0 && check_axis(origin.x, dir.x, ARENA_HALF - 0.5) {
        return true;
    }
    if dir.x < 0.0 && check_axis(origin.x, dir.x, -ARENA_HALF + 0.5) {
        return true;
    }
    if dir.y > 0.0 && check_axis(origin.y, dir.y, ARENA_HALF - 0.5) {
        return true;
    }
    if dir.y < 0.0 && check_axis(origin.y, dir.y, -ARENA_HALF + 0.5) {
        return true;
    }
    false
}

fn build_entities_string(ct_spawns: &[(Vec3, f32)], t_spawns: &[(Vec3, f32)]) -> String {
    let mut s = String::with_capacity(4096);
    s.push_str(
        "{\n\
         \"classname\" \"worldspawn\"\n\
         \"message\" \"Rusthouse Instant Demo\"\n\
         \"skyname\" \"rusthouse\"\n\
         \"wad\" \"demo.wad\"\n\
         \"MaxRange\" \"4096\"\n\
         }\n",
    );
    s.push_str(
        "{\n\
         \"classname\" \"light_environment\"\n\
         \"angles\" \"-55 322 0\"\n\
         \"_light\" \"255 240 205 220\"\n\
         }\n",
    );
    s.push_str(
        "{\n\
         \"classname\" \"func_buyzone\"\n\
         \"model\" \"*1\"\n\
         \"team\" \"2\"\n\
         }\n",
    );
    s.push_str(
        "{\n\
         \"classname\" \"func_buyzone\"\n\
         \"model\" \"*2\"\n\
         \"team\" \"1\"\n\
         }\n",
    );
    for &(pos, yaw) in ct_spawns {
        s.push_str(&format!(
            "{{\n\"classname\" \"info_player_start\"\n\"origin\" \"{:.0} {:.0} {:.0}\"\n\"angles\" \"0 {:.0} 0\"\n}}\n",
            pos.x, pos.y, pos.z, yaw
        ));
    }
    for &(pos, yaw) in t_spawns {
        s.push_str(&format!(
            "{{\n\"classname\" \"info_player_deathmatch\"\n\"origin\" \"{:.0} {:.0} {:.0}\"\n\"angles\" \"0 {:.0} 0\"\n}}\n",
            pos.x, pos.y, pos.z, yaw
        ));
    }
    s.push('\0');
    s
}

fn build_textures_lump(miptex_list: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let count = miptex_list.len();
    let header_len = 4 + count * 4;
    let mut out = vec![0u8; header_len];
    out[0..4].copy_from_slice(&(count as u32).to_le_bytes());
    for (i, (_, bytes)) in miptex_list.iter().enumerate() {
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        let off = out.len() as i32;
        out[4 + i * 4..8 + i * 4].copy_from_slice(&off.to_le_bytes());
        out.extend_from_slice(bytes);
    }
    out
}

struct BspModelSpec {
    mins: Vec3,
    maxs: Vec3,
    origin: Vec3,
    head_nodes: [i32; 4],
    vis_leafs: i32,
    first_face: i32,
    num_faces: i32,
}

#[allow(clippy::too_many_arguments)]
fn serialize_bsp_v30(
    entities: &[u8],
    planes: &[Plane],
    textures: &[u8],
    vertices: &[Vec3],
    visibility: &[u8],
    nodes: &[Node],
    texinfos: &[TexInfo],
    faces: &[Face],
    lighting: &[u8],
    clipnodes: &[ClipNode],
    leafs: &[Leaf],
    mark_surfaces: &[u16],
    edges: &[[u16; 2]],
    surf_edges: &[i32],
    models: &[BspModelSpec],
) -> Vec<u8> {
    let mut l_planes = Vec::with_capacity(planes.len() * 20);
    for p in planes {
        l_planes.extend_from_slice(&p.normal.x.to_le_bytes());
        l_planes.extend_from_slice(&p.normal.y.to_le_bytes());
        l_planes.extend_from_slice(&p.normal.z.to_le_bytes());
        l_planes.extend_from_slice(&p.dist.to_le_bytes());
        l_planes.extend_from_slice(&p.plane_type.to_le_bytes());
    }

    let mut l_verts = Vec::with_capacity(vertices.len() * 12);
    for v in vertices {
        l_verts.extend_from_slice(&v.x.to_le_bytes());
        l_verts.extend_from_slice(&v.y.to_le_bytes());
        l_verts.extend_from_slice(&v.z.to_le_bytes());
    }

    let mut l_nodes = Vec::with_capacity(nodes.len() * 24);
    for n in nodes {
        l_nodes.extend_from_slice(&n.plane.to_le_bytes());
        l_nodes.extend_from_slice(&n.children[0].to_le_bytes());
        l_nodes.extend_from_slice(&n.children[1].to_le_bytes());
        for c in n.mins {
            l_nodes.extend_from_slice(&c.to_le_bytes());
        }
        for c in n.maxs {
            l_nodes.extend_from_slice(&c.to_le_bytes());
        }
        l_nodes.extend_from_slice(&n.first_face.to_le_bytes());
        l_nodes.extend_from_slice(&n.num_faces.to_le_bytes());
    }

    let mut l_texinfos = Vec::with_capacity(texinfos.len() * 40);
    for t in texinfos {
        l_texinfos.extend_from_slice(&t.s_vec.x.to_le_bytes());
        l_texinfos.extend_from_slice(&t.s_vec.y.to_le_bytes());
        l_texinfos.extend_from_slice(&t.s_vec.z.to_le_bytes());
        l_texinfos.extend_from_slice(&t.s_shift.to_le_bytes());
        l_texinfos.extend_from_slice(&t.t_vec.x.to_le_bytes());
        l_texinfos.extend_from_slice(&t.t_vec.y.to_le_bytes());
        l_texinfos.extend_from_slice(&t.t_vec.z.to_le_bytes());
        l_texinfos.extend_from_slice(&t.t_shift.to_le_bytes());
        l_texinfos.extend_from_slice(&t.miptex.to_le_bytes());
        l_texinfos.extend_from_slice(&t.flags.to_le_bytes());
    }

    let mut l_faces = Vec::with_capacity(faces.len() * 20);
    for f in faces {
        l_faces.extend_from_slice(&f.plane.to_le_bytes());
        l_faces.extend_from_slice(&f.plane_side.to_le_bytes());
        l_faces.extend_from_slice(&f.first_edge.to_le_bytes());
        l_faces.extend_from_slice(&f.num_edges.to_le_bytes());
        l_faces.extend_from_slice(&f.texinfo.to_le_bytes());
        l_faces.extend_from_slice(&f.styles);
        l_faces.extend_from_slice(&f.lightmap_offset.to_le_bytes());
    }

    let mut l_clipnodes = Vec::with_capacity(clipnodes.len() * 8);
    for c in clipnodes {
        l_clipnodes.extend_from_slice(&c.plane.to_le_bytes());
        l_clipnodes.extend_from_slice(&c.children[0].to_le_bytes());
        l_clipnodes.extend_from_slice(&c.children[1].to_le_bytes());
    }

    let mut l_leafs = Vec::with_capacity(leafs.len() * 28);
    for l in leafs {
        l_leafs.extend_from_slice(&l.contents.to_le_bytes());
        l_leafs.extend_from_slice(&l.vis_offset.to_le_bytes());
        for c in l.mins {
            l_leafs.extend_from_slice(&c.to_le_bytes());
        }
        for c in l.maxs {
            l_leafs.extend_from_slice(&c.to_le_bytes());
        }
        l_leafs.extend_from_slice(&l.first_mark_surface.to_le_bytes());
        l_leafs.extend_from_slice(&l.num_mark_surfaces.to_le_bytes());
        l_leafs.extend_from_slice(&l.ambient_levels);
    }

    let mut l_marks = Vec::with_capacity(mark_surfaces.len() * 2);
    for &m in mark_surfaces {
        l_marks.extend_from_slice(&m.to_le_bytes());
    }

    let mut l_edges = Vec::with_capacity(edges.len() * 4);
    for e in edges {
        l_edges.extend_from_slice(&e[0].to_le_bytes());
        l_edges.extend_from_slice(&e[1].to_le_bytes());
    }

    let mut l_surfedges = Vec::with_capacity(surf_edges.len() * 4);
    for &se in surf_edges {
        l_surfedges.extend_from_slice(&se.to_le_bytes());
    }

    let mut l_models = Vec::with_capacity(models.len() * 64);
    for m in models {
        for v in [m.mins, m.maxs, m.origin] {
            l_models.extend_from_slice(&v.x.to_le_bytes());
            l_models.extend_from_slice(&v.y.to_le_bytes());
            l_models.extend_from_slice(&v.z.to_le_bytes());
        }
        for h in m.head_nodes {
            l_models.extend_from_slice(&h.to_le_bytes());
        }
        l_models.extend_from_slice(&m.vis_leafs.to_le_bytes());
        l_models.extend_from_slice(&m.first_face.to_le_bytes());
        l_models.extend_from_slice(&m.num_faces.to_le_bytes());
    }

    let lumps: [&[u8]; LUMP_COUNT] = [
        entities,
        &l_planes,
        textures,
        &l_verts,
        visibility,
        &l_nodes,
        &l_texinfos,
        &l_faces,
        lighting,
        &l_clipnodes,
        &l_leafs,
        &l_marks,
        &l_edges,
        &l_surfedges,
        &l_models,
    ];

    let header_len = 4 + LUMP_COUNT * 8;
    let total_payload: usize = lumps.iter().map(|l| l.len() + 4).sum();
    let mut out = vec![0u8; header_len];
    out.reserve(total_payload);
    out[0..4].copy_from_slice(&BSP_VERSION_GOLDSRC.to_le_bytes());

    for (i, payload) in lumps.iter().enumerate() {
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        let off = out.len() as i32;
        let len = payload.len() as i32;
        out[4 + i * 8..8 + i * 8].copy_from_slice(&off.to_le_bytes());
        out[8 + i * 8..12 + i * 8].copy_from_slice(&len.to_le_bytes());
        out.extend_from_slice(payload);
    }
    out
}
