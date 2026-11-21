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
    /// The potentially audible set of every leaf, built on the first
    /// [`can_hear`](Self::can_hear): a map load that never sends a sound does not pay for it,
    /// and the build is bounded by [`MAX_PAS_LEAFS`]. `None` inside for a map without
    /// visibility data or with more leaves than that, where everything is audible.
    pas: OnceLock<Option<Pas>>,
}

/// Most visible leaves a PAS is built for: the GoldSrc `MAX_MAP_LEAFS`. Its table is at most
/// `8192 * 1024` bytes (8 MiB); a hostile map with more leaves hears everything instead of
/// making the load allocate without bound.
const MAX_PAS_LEAFS: usize = 8192;

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
    /// visibility data or past [`MAX_PAS_LEAFS`].
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
                let src = (n + 1) * row_bytes;
                for (d, s) in rows[own..own + row_bytes].iter_mut().zip(&pvs[src..src + row_bytes]) {
                    *d |= s;
                }
                or_rows += 1;
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

    /// Whether a box with absolute corners `mins`/`maxs` is potentially audible from `ear`: any
    /// leaf the box touches is in the PAS of the ear's leaf (the leaves PVS-visible from any
    /// leaf the ear leaf sees). Everything is audible on a map without visibility data (or with
    /// too many leaves to build a PAS), and to an ear in a leaf without any (solid leaf 0,
    /// outside the world). ReHLDS `SV_ValidClientMulticast` tests the listener's origin leaf
    /// against the PAS of the sound's leaf; this tests the ear's leaf against the leaves of the
    /// box, as [`can_see`](Self::can_see) does for the PVS.
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

    /// The PAS, built on first use.
    fn pas(&self) -> Option<&Pas> {
        self.pas
            .get_or_init(|| self.vis.as_deref().and_then(Pas::build))
            .as_ref()
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
}
