//! What the server needs from a BSP map: name, checksum, brush-model count,
//! entity list, visibility (PVS) queries and hull collision.

use std::sync::{Arc, OnceLock};

use cs_assets::bsp::{BspMap, Entity};
use cs_proto48::crypto::crc_map_bsp;
use glam::Vec3;

use crate::collision::{MAX_WORLD_COORD, WorldCollision};
use crate::world::find_touched_leafs;

pub struct ServerMap {
    pub name: String,
    pub path: String,
    pub crc: u32,
    pub submodel_count: usize,
    pub entities: Vec<Entity>,
    vis: Option<Arc<BspMap>>,
    collision: Option<WorldCollision>,
    /// The potentially audible set of every leaf, built by [`calc_pas`](Self::calc_pas) when
    /// the map is activated, outside any client frame. Unset until then, and `None` inside for
    /// a single-client server, a map without visibility data or one over the caps
    /// ([`MAX_PAS_LEAFS`], [`MAX_PAS_WORK_BYTES`]), where everything is audible.
    pas: OnceLock<Option<Pas>>,
}

/// Most visible leaves a PAS is built for. Shipped maps have 1.5 to 3.7 thousand (de_dust2
/// 1455, cs_office 3672). The table is at most `4097 * 512` bytes (2 MiB), and the PVS matrix
/// the build needs for a moment is the same size again. A map with more leaves hears
/// everything instead of making the load allocate or compute without bound.
const MAX_PAS_LEAFS: usize = 4096;
/// Most byte ORs the build of a PAS may do (128 MiB; cs_office needs 40 MB). A map whose
/// visibility is so dense that it would take more hears everything, so the work is bounded,
/// not only the memory.
const MAX_PAS_WORK_BYTES: u64 = 128 << 20;

/// The potentially audible set of every leaf (`CM_CalcPAS`, which ReHLDS calls from
/// `SV_SpawnServer`; the original engine calls it `SV_CalcPHS`): the OR of the PVS of every
/// leaf the leaf can see. One bit row per leaf number `0..=vis_leafs`, in the PVS bit layout
/// (bit `n` of a row is leaf `n + 1`), `row_bytes` each; row 0, the solid leaf, hears
/// everything.
struct Pas {
    vis_leafs: usize,
    row_bytes: usize,
    rows: Vec<u8>,
    /// Row ORs the build did: the work done is `or_rows * row_bytes` byte ORs. Only tests
    /// read it.
    #[cfg_attr(not(test), allow(dead_code))]
    or_rows: u64,
}

impl Pas {
    /// `CM_CalcPAS` (`rehlds/engine/cmodel.cpp`, read 2026-11-21): the PVS rows come from
    /// [`BspMap::decompress_pvs`] (a leaf without visibility data sees every leaf, as
    /// `mod_novis` does there), and each PAS row is its PVS row ORed with the PVS row of every
    /// leaf in it. The table is stored as bit rows, `(vis_leafs + 1) * ceil(vis_leafs / 8)`
    /// bytes, never more than `leafs * ceil(leafs / 8)`. `None` for a map without
    /// visibility data, past [`MAX_PAS_LEAFS`] or needing more than [`MAX_PAS_WORK_BYTES`] of
    /// byte ORs.
    fn build(bsp: &BspMap) -> Option<Self> {
        if bsp.visibility.is_empty() {
            return None;
        }
        let vis_leafs = bsp
            .models
            .first()
            .map_or(bsp.leafs.len().saturating_sub(1), |m| m.vis_leafs.max(0) as usize)
            .min(bsp.leafs.len().saturating_sub(1));
        if vis_leafs == 0 || vis_leafs > MAX_PAS_LEAFS {
            return None;
        }
        let row_bytes = vis_leafs.div_ceil(8);
        let mut pvs = vec![0u8; (vis_leafs + 1) * row_bytes];
        pvs[..row_bytes].fill(0xFF);
        for leaf in 1..=vis_leafs {
            let row = &mut pvs[leaf * row_bytes..(leaf + 1) * row_bytes];
            for (n, _) in bsp.decompress_pvs(leaf).iter().enumerate().filter(|(_, v)| **v) {
                row[n >> 3] |= 1 << (n & 7);
            }
        }
        let mut rows = pvs.clone();
        let mut or_rows = 0u64;
        for leaf in 1..=vis_leafs {
            let own = leaf * row_bytes;
            for n in 0..vis_leafs {
                if pvs[own + (n >> 3)] & (1 << (n & 7)) == 0 {
                    continue;
                }
                or_rows += 1;
                if or_rows * row_bytes as u64 > MAX_PAS_WORK_BYTES {
                    return None;
                }
                let src = (n + 1) * row_bytes;
                for (d, s) in rows[own..own + row_bytes].iter_mut().zip(&pvs[src..src + row_bytes]) {
                    *d |= s;
                }
            }
        }
        rows[..row_bytes].fill(0xFF);
        Some(Self {
            vis_leafs,
            row_bytes,
            rows,
            or_rows,
        })
    }

    /// The row of `leaf` as one flag per leaf (flag `n` is leaf `n + 1`), the shape the leaf
    /// tests take; `None` for a leaf that has no row.
    fn audible_from(&self, leaf: usize) -> Option<Vec<bool>> {
        if leaf > self.vis_leafs {
            return None;
        }
        let row = &self.rows[leaf * self.row_bytes..(leaf + 1) * self.row_bytes];
        Some((0..self.vis_leafs).map(|n| row[n >> 3] & (1 << (n & 7)) != 0).collect())
    }
}

impl ServerMap {
    /// `raw` is the BSP file as read from disk, `bsp` its parsed form.
    /// Returns `None` when `raw` has no valid BSP header for the checksum.
    pub fn from_bsp(name: &str, raw: &[u8], bsp: Arc<BspMap>) -> Option<Self> {
        let crc = crc_map_bsp(raw)?;
        Some(Self {
            name: name.to_string(),
            path: map_path(name),
            crc,
            submodel_count: bsp.models.len(),
            entities: bsp.entities.clone(),
            collision: Some(WorldCollision::from_bsp(&bsp)),
            pas: OnceLock::new(),
            vis: Some(bsp),
        })
    }

    /// Map without geometry: checksum 0, only the world model, sees
    /// everything, no collision.
    pub fn stub(name: &str) -> Self {
        Self {
            name: name.to_string(),
            path: map_path(name),
            crc: 0,
            submodel_count: 1,
            entities: Vec::new(),
            vis: None,
            collision: None,
            pas: OnceLock::new(),
        }
    }

    /// A [`stub`](Self::stub) map whose entity lump is `entities`, spawned in vector order when
    /// the map is activated.
    pub fn stub_with_entities(name: &str, entities: Vec<Entity>) -> Self {
        Self {
            entities,
            ..Self::stub(name)
        }
    }

    /// The map's hull collision; `None` for the stub map.
    pub fn collision(&self) -> Option<&WorldCollision> {
        self.collision.as_ref()
    }

    /// The BSP behind the visibility tests; `None` for the stub map.
    pub(crate) fn bsp(&self) -> Option<&BspMap> {
        self.vis.as_deref()
    }

    /// Mins and maxs of the world model, which the area tree spans; a map without collision
    /// data spans the whole coordinate range.
    pub(crate) fn world_bounds(&self) -> (Vec3, Vec3) {
        let whole = (Vec3::splat(-MAX_WORLD_COORD), Vec3::splat(MAX_WORLD_COORD));
        self.collision()
            .and_then(|c| c.model_bounds(0))
            .filter(|(mins, maxs)| mins.is_finite() && maxs.is_finite() && mins.cmple(*maxs).all())
            .unwrap_or(whole)
    }

    /// The PVS of the leaf `eye` is in, bit `n` for leaf `n + 1`. `None` when the eye sees
    /// everything: a map without visibility data, or an eye in a leaf without any (solid leaf
    /// 0, outside the world).
    pub(crate) fn viewer_pvs(&self, eye: Vec3) -> Option<Vec<bool>> {
        let bsp = self.vis.as_deref()?;
        let eye_leaf = bsp.find_leaf(eye);
        let has_vis = eye_leaf != 0
            && !bsp.visibility.is_empty()
            && bsp.leafs.get(eye_leaf).is_some_and(|l| l.vis_offset >= 0);
        has_vis.then(|| bsp.decompress_pvs(eye_leaf))
    }

    /// Whether a box with absolute corners `mins`/`maxs` is potentially visible from `eye`: any
    /// leaf the box touches (`SV_FindTouchedLeafs`) is in the PVS of the eye's leaf, as for a
    /// linked entity; a box that touches more leaves than an entity keeps is tested by the
    /// leaves under its top node. An eye in a leaf without visibility data (solid leaf 0,
    /// outside the world) sees everything.
    pub fn can_see(&self, eye: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        let Some(bsp) = self.vis.as_deref() else {
            return true;
        };
        let Some(pvs) = self.viewer_pvs(eye) else {
            return true;
        };
        find_touched_leafs(bsp, mins, maxs).visible(bsp, &pvs)
    }

    /// Builds the PAS when the map is activated (ReHLDS `SV_SpawnServer` calls `CM_CalcPAS`
    /// only for `maxclients > 1`, `rehlds/engine/sv_main.cpp`, read 2026-11-21): the work is
    /// done here once, never inside a client frame. With one client, or when called again,
    /// nothing is (re)built; everything is then audible.
    pub fn calc_pas(&self, max_clients: u8) {
        let _ = self.pas.set(if max_clients > 1 {
            self.vis.as_deref().and_then(Pas::build)
        } else {
            None
        });
    }

    /// The PAS, if one was built.
    fn pas(&self) -> Option<&Pas> {
        self.pas.get().and_then(Option::as_ref)
    }

    /// The PAS row of the leaf `point` is in, as the sound leaf of `SV_Multicast`; `None` when
    /// everyone hears: no PAS, a point outside the world or in solid (leaf 0).
    pub(crate) fn pas_row(&self, point: Vec3) -> Option<&[u8]> {
        let (bsp, pas) = (self.vis.as_deref()?, self.pas()?);
        let leaf = bsp.find_leaf(point);
        (leaf != 0 && leaf <= pas.vis_leafs)
            .then(|| &pas.rows[leaf * pas.row_bytes..(leaf + 1) * pas.row_bytes])
    }

    /// The PVS of the leaf `point` is in, as the sound leaf of `SV_Multicast` (flag `n` is leaf
    /// `n + 1`); `None` when everyone gets it: no visibility data, a point outside the world or
    /// in solid (leaf 0).
    pub(crate) fn pvs_row(&self, point: Vec3) -> Option<Vec<bool>> {
        let bsp = self.vis.as_deref()?;
        let leaf = bsp.find_leaf(point);
        (leaf != 0 && !bsp.visibility.is_empty()).then(|| bsp.decompress_pvs(leaf))
    }

    /// Whether the leaf of `listener` (the client's edict origin) is in the PVS `row`; a
    /// listener in leaf 0 gets it, as with [`pas_row_contains`](Self::pas_row_contains).
    pub(crate) fn pvs_row_contains(&self, row: &[bool], listener: Vec3) -> bool {
        let Some(bsp) = self.vis.as_deref() else {
            return true;
        };
        let leaf = bsp.find_leaf(listener);
        leaf == 0 || row.get(leaf - 1).copied().unwrap_or(true)
    }

    /// Whether the leaf of `listener` (the client's edict origin) is in the PAS row `row`
    /// (bit `leaf - 1`, `SV_ValidClientMulticast`). A listener in solid or outside the world
    /// (leaf 0) hears it; HLDS reads the byte before the row there.
    pub(crate) fn pas_row_contains(&self, row: &[u8], listener: Vec3) -> bool {
        let Some(bsp) = self.vis.as_deref() else {
            return true;
        };
        let leaf = bsp.find_leaf(listener);
        if leaf == 0 {
            return true;
        }
        row.get((leaf - 1) >> 3)
            .is_none_or(|byte| byte & (1 << ((leaf - 1) & 7)) != 0)
    }

    /// Whether a listener at `listener` (an edict origin) is in the PAS of the leaf of
    /// `point`: the `SV_Multicast` audience rule for one client. Everyone hears on a map
    /// without a PAS, and a point in solid or outside the world.
    pub fn can_hear_point(&self, point: Vec3, listener: Vec3) -> bool {
        self.pas_row(point)
            .is_none_or(|row| self.pas_row_contains(row, listener))
    }

    /// Whether a box with absolute corners `mins`/`maxs` is potentially audible from `ear`:
    /// any leaf the box touches is in the PAS of the ear's leaf. Everything is audible when no
    /// PAS was built (see [`calc_pas`](Self::calc_pas)) and to an ear in a leaf without any
    /// (solid leaf 0, outside the world). Like [`can_see`](Self::can_see) for the PVS; the
    /// message audience of `MsgDest::Pas` uses [`can_hear_point`](Self::can_hear_point).
    pub fn can_hear(&self, ear: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        let (Some(bsp), Some(pas)) = (self.vis.as_deref(), self.pas()) else {
            return true;
        };
        let ear_leaf = bsp.find_leaf(ear);
        if ear_leaf == 0 {
            return true;
        }
        let Some(audible) = pas.audible_from(ear_leaf) else {
            return true;
        };
        find_touched_leafs(bsp, mins, maxs).visible(bsp, &audible)
    }

    /// Whether leaf `to` is in the PAS of leaf `from`; for tests.
    #[cfg(test)]
    fn pas_leaf_audible(&self, from: usize, to: usize) -> Option<bool> {
        self.pas()?.audible_from(from)?.get(to.checked_sub(1)?).copied()
    }

    /// Size of the PAS table in bytes, and the row ORs its build did; for tests.
    #[cfg(test)]
    fn pas_size(&self) -> Option<(usize, u64)> {
        self.pas().map(|p| (p.rows.len(), p.or_rows))
    }

    /// A map around `bsp` without a file behind it, for tests.
    #[cfg(test)]
    pub(crate) fn with_bsp_for_test(bsp: Arc<BspMap>) -> Self {
        Self {
            submodel_count: bsp.models.len(),
            entities: Vec::new(),
            collision: Some(WorldCollision::from_bsp(&bsp)),
            pas: OnceLock::new(),
            vis: Some(bsp),
            ..Self::stub("test_bsp")
        }
    }
}

fn map_path(name: &str) -> String {
    format!("maps/{name}.bsp")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_assets::bsp::SpawnTeam;
    use cs_vfs::{Vfs, VfsConfig};

    const _: () = {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ServerMap>();
    };

    #[test]
    fn stub_map_sees_everything() {
        let map = ServerMap::stub("de_test");
        assert_eq!(map.name, "de_test");
        assert_eq!(map.path, "maps/de_test.bsp");
        assert_eq!(map.crc, 0);
        assert_eq!(map.submodel_count, 1);
        assert!(map.entities.is_empty());
        let far = Vec3::splat(1.0e5);
        assert!(map.can_see(Vec3::ZERO, far - 16.0, far + 16.0));
    }

    #[test]
    fn real_map_pvs_hides_far_spawn() {
        let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
            return;
        };
        let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("de_dust2 reads");
        let bsp = Arc::new(BspMap::load(&vfs, "de_dust2").expect("de_dust2 parses"));
        let map = ServerMap::from_bsp("de_dust2", &raw, bsp.clone()).expect("valid BSP header");
        assert_eq!(map.path, "maps/de_dust2.bsp");
        assert_ne!(map.crc, 0);
        assert_eq!(map.submodel_count, bsp.models.len());
        assert!(map.submodel_count > 1);
        assert!(map.entities.iter().any(|e| e.classname() == "worldspawn"));

        let ct = bsp
            .spawns
            .iter()
            .find(|s| s.team == SpawnTeam::CounterTerrorist)
            .expect("CT spawn")
            .position;
        let t = bsp
            .spawns
            .iter()
            .find(|s| s.team == SpawnTeam::Terrorist)
            .expect("T spawn")
            .position;
        let hull_mins = Vec3::new(-16.0, -16.0, -36.0);
        let hull_maxs = Vec3::new(16.0, 16.0, 36.0);
        // A player at a spawn sees the box around itself, but not the
        // other team's spawn across the map.
        assert!(map.can_see(ct, ct + hull_mins, ct + hull_maxs));
        assert!(!map.can_see(ct, t + hull_mins, t + hull_maxs));
        assert!(!map.can_see(t, ct + hull_mins, ct + hull_maxs));
    }

    fn real_map(name: &str) -> Option<(Arc<BspMap>, ServerMap)> {
        let vfs = Vfs::discover(None, VfsConfig::default()).ok()?;
        let raw = vfs.read_bytes(&format!("maps/{name}.bsp")).ok()?;
        let bsp = Arc::new(BspMap::load(&vfs, name).expect("map parses"));
        let map = ServerMap::from_bsp(name, &raw, bsp.clone()).expect("valid BSP header");
        map.calc_pas(16);
        Some((bsp, map))
    }

    fn dust2() -> Option<(Arc<BspMap>, ServerMap)> {
        real_map("de_dust2")
    }

    #[test]
    fn stub_map_hears_everything() {
        let map = ServerMap::stub("de_test");
        let far = Vec3::splat(1.0e5);
        assert!(map.can_hear(Vec3::ZERO, far - 16.0, far + 16.0));
    }

    /// The strip map (four leaves in a row along x, see `world::tests::strip_bsp`) with a PVS
    /// for every leaf: leaf 1 sees {2}, leaf 2 sees {1, 3}, leaf 3 sees {2, 4}, leaf 4 sees {3}.
    fn strip_with_vis() -> BspMap {
        let mut bsp = crate::world::tests::strip_bsp();
        bsp.visibility = vec![0b0010, 0b0101, 0b1010, 0b0100];
        for (leaf, offset) in (1..=4).zip(0..) {
            bsp.leafs[leaf].vis_offset = offset;
        }
        bsp
    }

    /// One point inside each leaf of the strip map.
    const STRIP_POINTS: [Vec3; 4] = [
        Vec3::new(-200.0, 0.0, 0.0),
        Vec3::new(-50.0, 0.0, 0.0),
        Vec3::new(100.0, 0.0, 0.0),
        Vec3::new(300.0, 0.0, 0.0),
    ];

    #[test]
    fn pas_is_the_or_of_the_pvs_of_the_visible_leaves() {
        let map = ServerMap::with_bsp_for_test(Arc::new(strip_with_vis()));
        map.calc_pas(2);
        // PAS(1) = {2} + PVS(2) = {1, 2, 3}; PAS(2) = {1, 3} + {2} + {2, 4} = all;
        // PAS(3) = {2, 4} + {1, 3} + {3} = all; PAS(4) = {3} + {2, 4} = {2, 3, 4}.
        let expected = [
            [true, true, true, false],
            [true, true, true, true],
            [true, true, true, true],
            [false, true, true, true],
        ];
        for from in 1..=4 {
            for to in 1..=4 {
                assert_eq!(
                    map.pas_leaf_audible(from, to),
                    Some(expected[from - 1][to - 1]),
                    "{from} -> {to}"
                );
                assert_eq!(
                    map.can_hear_point(STRIP_POINTS[to - 1], STRIP_POINTS[from - 1]),
                    expected[to - 1][from - 1],
                    "sound in {to} heard in {from}"
                );
            }
        }
        // The box test agrees: an ear in leaf 1 does not hear a box in leaf 4.
        let far = STRIP_POINTS[3];
        assert!(!map.can_hear(STRIP_POINTS[0], far - Vec3::ONE, far + Vec3::ONE));
        assert!(map.can_hear(STRIP_POINTS[1], far - Vec3::ONE, far + Vec3::ONE));
    }

    #[test]
    fn no_pas_means_everyone_hears() {
        let far = Vec3::new(300.0, 0.0, 0.0);
        let hears = |map: &ServerMap| map.can_hear_point(far, STRIP_POINTS[0]);
        // Not built (before activation) and a single-client server.
        let map = ServerMap::with_bsp_for_test(Arc::new(strip_with_vis()));
        assert!(hears(&map));
        let single = ServerMap::with_bsp_for_test(Arc::new(strip_with_vis()));
        single.calc_pas(1);
        assert!(hears(&single));
        // A map whose visibility data is gone or lies about its leaf count.
        let mut bsp = strip_with_vis();
        bsp.visibility.clear();
        let no_vis = ServerMap::with_bsp_for_test(Arc::new(bsp));
        no_vis.calc_pas(2);
        assert!(hears(&no_vis));
        let mut bsp = strip_with_vis();
        bsp.models[0].vis_leafs = 0;
        let corrupt = ServerMap::with_bsp_for_test(Arc::new(bsp));
        corrupt.calc_pas(2);
        assert!(hears(&corrupt));
        // Built, for contrast: leaf 1 does not hear leaf 4.
        let built = ServerMap::with_bsp_for_test(Arc::new(strip_with_vis()));
        built.calc_pas(2);
        assert!(!hears(&built));
    }

    /// The strip map grown to `leafs` leaves, every one seeing all the others.
    fn dense_bsp(leafs: usize) -> BspMap {
        let mut bsp = strip_with_vis();
        let row = leafs.div_ceil(8);
        bsp.visibility = vec![0xFF; row];
        while bsp.leafs.len() <= leafs {
            bsp.leafs.push(bsp.leafs[2]);
        }
        for leaf in bsp.leafs.iter_mut().skip(1) {
            leaf.vis_offset = 0;
        }
        bsp.models[0].vis_leafs = leafs as i32;
        bsp
    }

    #[test]
    fn pas_build_is_capped_in_leafs_and_in_work() {
        // A small map is built; one leaf past the cap, and everyone hears.
        assert!(Pas::build(&strip_with_vis()).is_some());
        let over = dense_bsp(MAX_PAS_LEAFS + 1);
        assert!(Pas::build(&over).is_none(), "more leafs than the cap");
        // Every leaf of 4096 sees every other: 16.7 million row ORs of 512 bytes, far past the
        // work cap, so the build gives up part of the way.
        let dense = dense_bsp(MAX_PAS_LEAFS);
        assert!(Pas::build(&dense).is_none(), "more work than the cap");
        // The same density with few leaves is fine.
        let small = dense_bsp(64);
        let pas = Pas::build(&small).expect("small dense map");
        assert_eq!(pas.or_rows, 64 * 64);
    }

    #[test]
    fn pas_is_a_superset_of_pvs() {
        let Some((bsp, map)) = dust2() else {
            return;
        };
        let vis_leafs = bsp.models[0].vis_leafs as usize;
        assert!(vis_leafs > 200);
        let (mut seen, mut heard_only) = (0u64, 0u64);
        for i in 0..200usize {
            let from = 1 + (i * 7919) % vis_leafs;
            let pvs = bsp.decompress_pvs(from);
            for n in (0..vis_leafs).filter(|&n| pvs[n]) {
                seen += 1;
                assert_eq!(map.pas_leaf_audible(from, n + 1), Some(true), "leaf {from} sees {}", n + 1);
            }
            heard_only += (0..vis_leafs)
                .filter(|&n| !pvs[n] && map.pas_leaf_audible(from, n + 1) == Some(true))
                .count() as u64;
        }
        assert!(seen > 0);
        assert!(heard_only > 0, "the PAS must reach beyond the PVS");
    }

    #[test]
    fn pas_memory_is_bounded() {
        for name in ["de_dust2", "cs_office"] {
            let Some((bsp, map)) = real_map(name) else {
                continue;
            };
            let leafs = bsp.leafs.len();
            let (bytes, or_rows) = map.pas_size().expect("the map has visibility data");
            assert!(bytes <= leafs * leafs.div_ceil(8), "{name}: {bytes} bytes for {leafs} leafs");
            println!("{name} PAS: {leafs} leafs, {bytes} bytes, {or_rows} row ORs");
        }
    }

    #[test]
    fn pas_excludes_the_far_spawn_and_a_solid_point_is_heard_everywhere() {
        let Some((bsp, map)) = dust2() else {
            return;
        };
        let origin_of = |classname: &str| {
            map.entities
                .iter()
                .filter(|e| e.classname() == classname)
                .find_map(|e| e.get_vec3("origin"))
                .expect("spawn")
        };
        let t = origin_of("info_player_deathmatch");
        let ct = origin_of("info_player_start");
        // The PAS is not the whole map: the two spawns do not hear each other.
        assert!(!map.can_hear_point(t, ct), "T spawn sound reaches the CT spawn");
        assert!(!map.can_hear_point(ct, t));
        assert!(map.can_hear_point(t, t));
        // A point in solid (leaf 0) is heard everywhere.
        let solid = (-40..40)
            .flat_map(|x| (-40..40).map(move |y| Vec3::new(x as f32, y as f32, 0.0) * 128.0))
            .flat_map(|p| (-8..8).map(move |z| p + Vec3::Z * (z as f32 * 128.0)))
            .find(|&p| bsp.find_leaf(p) == 0)
            .expect("de_dust2 has solid space");
        assert!(map.pas_row(solid).is_none());
        assert!(map.can_hear_point(solid, ct));
        assert!(map.can_hear_point(solid, t));
    }
}
