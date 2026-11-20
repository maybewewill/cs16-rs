//! What the server needs from a BSP map: name, checksum, brush-model count,
//! entity list, visibility (PVS) queries and hull collision.

use std::sync::Arc;

use cs_assets::bsp::{BspMap, Entity};
use cs_proto48::crypto::crc_map_bsp;
use glam::Vec3;

use crate::collision::WorldCollision;

pub struct ServerMap {
    pub name: String,
    pub path: String,
    pub crc: u32,
    pub submodel_count: usize,
    pub entities: Vec<Entity>,
    vis: Option<Arc<BspMap>>,
    collision: Option<WorldCollision>,
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

    /// Whether a box with absolute corners `mins`/`maxs` is potentially
    /// visible from `eye`: the leaf of the box centre or of any of its 8
    /// corners is in the PVS of the eye's leaf. An eye in a leaf without
    /// visibility data (solid leaf 0, outside the world) sees everything.
    ///
    /// Approximation: HLDS tests the entity's full leaf list.
    pub fn can_see(&self, eye: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        let Some(bsp) = &self.vis else {
            return true;
        };
        let eye_leaf = bsp.find_leaf(eye);
        let has_vis = eye_leaf != 0
            && !bsp.visibility.is_empty()
            && bsp.leafs.get(eye_leaf).is_some_and(|l| l.vis_offset >= 0);
        if !has_vis {
            return true;
        }
        let pvs = bsp.decompress_pvs(eye_leaf);
        let corners = (0..8).map(|i| {
            Vec3::new(
                if i & 1 == 0 { mins.x } else { maxs.x },
                if i & 2 == 0 { mins.y } else { maxs.y },
                if i & 4 == 0 { mins.z } else { maxs.z },
            )
        });
        std::iter::once((mins + maxs) * 0.5)
            .chain(corners)
            .any(|point| {
                // PVS bit `n` stands for leaf `n + 1`; leaf 0 is solid.
                let leaf = bsp.find_leaf(point);
                leaf != 0 && pvs.get(leaf - 1).copied().unwrap_or(false)
            })
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
}
