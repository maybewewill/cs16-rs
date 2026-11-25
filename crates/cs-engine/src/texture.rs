//! `TraceTexture`: the name of the texture a line hits on a brush model. Ported from ReHLDS
//! `rehlds/engine/pr_cmds.cpp` (`TraceTexture`, `SurfaceAtPoint`) and
//! `rehlds/engine/model.cpp` (`CalcSurfaceExtents` and the overrides `Mod_LoadFaces` applies
//! after it, for the `texturemins` and `extents` the walk compares against), read 2026-11-22.
//!
//! Delegates the BSP surface walk to [`cs_pm::collision::WorldCollision::trace_texture`] so
//! server and client prediction share the exact same surface lookup.
//!
//! The name is the one the map stores, up to 16 characters, with the `{`, `!`, `-0` and similar
//! prefixes still on: cutting them is the job of the caller (`pm_shared`'s
//! `PM_FindTextureType`).

use glam::Vec3;

use crate::edict::EdictId;
use crate::game::EngineApi;
use crate::trace_ents::{angle_vectors, brush_model};

impl EngineApi<'_> {
    /// `TraceTexture` (`pfnTraceTexture`, ReHLDS `rehlds/engine/pr_cmds.cpp`, read
    /// 2026-11-22): the name of the texture of the face of `ent`'s brush model the line
    /// `start` to `end` crosses first, as the map stores it. `ent` is the world or an entity
    /// whose model is `*N` of the map.
    ///
    /// As the source does, the line is moved by the entity's `origin` and, for an entity with
    /// `angles`, turned by them into the model's own frame, then walked down the model's
    /// first hull, the node tree of the model. A face is hit where the line crosses a node's
    /// plane inside the face's texture-space box (the box of whole 16-unit lumps of the
    /// face's extents), not by testing the face's polygon, so a hit can lie a little outside
    /// the polygon.
    ///
    /// `None` when there is no such face, for a stale `ent`, an entity without a brush model
    /// (a studio or sprite model, no model), a non-finite `start` or `end`, and a map without
    /// geometry (the stub map). A texture whose name the map could not read has the name the
    /// asset loader gives it (`missing`).
    pub fn trace_texture(&self, ent: EdictId, start: Vec3, end: Vec3) -> Option<String> {
        if !start.is_finite() || !end.is_finite() {
            return None;
        }
        let v = &self.edicts.get(ent)?.vars;
        let model = brush_model(v)?;
        let coll = self.map.collision()?;
        // `SV_HullForBsp` with a zero-size box: the point hull's offset is the entity's origin.
        let mut start = start - v.origin;
        let mut end = end - v.origin;
        if v.angles != Vec3::ZERO {
            let (forward, right, up) = angle_vectors(v.angles);
            let turn = |p: Vec3| Vec3::new(forward.dot(p), -right.dot(p), up.dot(p));
            start = turn(start);
            end = turn(end);
        }
        coll.trace_texture(model, start, end).map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use cs_assets::bsp::{BspMap, BspModel, Face, Node, Plane, TexInfo};
    use cs_assets::wad::DecodedTexture;
    use cs_pm::collision::{WorldCollision, calc_surface_extents};
    use glam::Vec3;

    use crate::world::tests::strip_bsp;

    const TEX_SPECIAL: i32 = 1;

    /// A map of one node: the plane `z = 0` with one 64 x 64 face of texture `"floor"`
    /// spanning x and y from 0 to 64 (in the texture space of `s = x`, `t = y`).
    fn one_face_bsp() -> BspMap {
        BspMap {
            planes: vec![Plane {
                normal: Vec3::Z,
                dist: 0.0,
                plane_type: 2,
            }],
            nodes: vec![Node {
                plane: 0,
                children: [-1, -1],
                mins: [0; 3],
                maxs: [0; 3],
                first_face: 0,
                num_faces: 1,
            }],
            models: vec![BspModel {
                mins: Vec3::splat(-64.0),
                maxs: Vec3::splat(64.0),
                origin: Vec3::ZERO,
                head_nodes: [0, -1, -1, -1],
                vis_leafs: 0,
                first_face: 0,
                num_faces: 1,
            }],
            texinfos: vec![TexInfo {
                s_vec: Vec3::X,
                s_shift: 0.0,
                t_vec: Vec3::Y,
                t_shift: 0.0,
                miptex: 0,
                flags: 0,
            }],
            faces: vec![Face {
                plane: 0,
                plane_side: 0,
                first_edge: 0,
                num_edges: 4,
                texinfo: 0,
                styles: [0; 4],
                lightmap_offset: -1,
            }],
            vertices_raw: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(64.0, 64.0, 0.0),
                Vec3::new(0.0, 64.0, 0.0),
            ],
            edges: vec![[0, 1], [1, 2], [2, 3], [3, 0]],
            surf_edges: vec![0, 1, 2, 3],
            textures: vec![DecodedTexture {
                name: "floor".to_string(),
                width: 16,
                height: 16,
                rgba: Vec::new(),
                alpha_test: false,
            }],
            ..strip_bsp()
        }
    }

    #[test]
    fn extents_are_whole_lumps_of_16() {
        let bsp = one_face_bsp();
        let e = calc_surface_extents(&bsp, &bsp.faces[0], &bsp.texinfos[0]).unwrap();
        assert_eq!((e.texturemins, e.extents), ([0, 0], [64, 64]));
    }

    #[test]
    fn the_walk_finds_the_face_the_line_crosses_inside_its_box() {
        let coll = WorldCollision::from_bsp(&one_face_bsp());
        let down = |x: f32, y: f32| {
            coll.trace_texture(0, Vec3::new(x, y, 10.0), Vec3::new(x, y, -10.0))
        };
        assert_eq!(down(32.0, 32.0), Some("floor"));
        assert_eq!(down(100.0, 32.0), None);
        // A line that stays on one side of the plane crosses nothing.
        let start = Vec3::new(32.0, 32.0, 10.0);
        assert_eq!(coll.trace_texture(0, start, start + Vec3::X), None);
    }

    #[test]
    fn a_tree_that_leads_back_to_itself_ends() {
        let mut bsp = one_face_bsp();
        bsp.nodes[0].children = [0, 0];
        let coll = WorldCollision::from_bsp(&bsp);
        let found = coll.trace_texture(
            0,
            Vec3::new(500.0, 500.0, 10.0),
            Vec3::new(500.0, 500.0, -10.0),
        );
        assert_eq!(found, None);
    }

    /// Runs the walk straight down at `(x, y)` over a copy of the one-face map whose texture is
    /// called `name` and is `size` x `size`, with `flags` on its texinfo.
    fn down_over(name: &str, size: u32, flags: i32, x: f32, y: f32) -> Option<String> {
        let mut bsp = one_face_bsp();
        bsp.textures[0].name = name.to_string();
        bsp.textures[0].width = size;
        bsp.textures[0].height = size;
        bsp.texinfos[0].flags = flags;
        let coll = WorldCollision::from_bsp(&bsp);
        coll.trace_texture(0, Vec3::new(x, y, 10.0), Vec3::new(x, y, -10.0))
            .map(str::to_owned)
    }

    #[test]
    fn water_faces_cover_the_whole_plane() {
        // The 64 x 64 polygon box does not hold (500, 500); a turbulent face's does.
        assert_eq!(down_over("floor", 16, 0, 500.0, 500.0), None);
        for name in ["!water", "!lava", "WATER_1", "laser1", "Laser2"] {
            assert_eq!(down_over(name, 16, 0, 500.0, 500.0).as_deref(), Some(name), "{name}");
            assert_eq!(
                down_over(name, 16, 0, -8000.0, 8000.0).as_deref(),
                Some(name),
                "{name}"
            );
            // 16384 wide from -8192: 8200 is past it.
            assert_eq!(down_over(name, 16, 0, 8200.0, 0.0), None, "{name}");
        }
        // Only a prefix counts.
        assert_eq!(down_over("a_water", 16, 0, 500.0, 500.0), None);
    }

    #[test]
    fn scroll_and_special_faces_take_the_texture_size() {
        // Texture space starts at 0 and spans the texture: 128 here, not the polygon's 64.
        for (name, flags) in [("scroll1", 0), ("floor", TEX_SPECIAL)] {
            assert_eq!(
                down_over(name, 128, flags, 100.0, 100.0).as_deref(),
                Some(name),
                "{name}"
            );
            assert_eq!(down_over(name, 128, flags, 140.0, 100.0), None, "{name}");
        }
        // `Scroll` with a capital is an ordinary face.
        assert_eq!(down_over("Scroll1", 128, 0, 100.0, 100.0), None);
        // `sky` keeps its computed box.
        assert_eq!(down_over("sky", 128, 0, 100.0, 100.0), None);
        assert_eq!(down_over("sky", 128, 0, 32.0, 32.0).as_deref(), Some("sky"));
    }

    #[test]
    fn a_very_deep_chain_of_nodes_is_cut_off() {
        let mut bsp = one_face_bsp();
        // A chain of nodes each passing the line on to the next, with the face at the end.
        let depth = 266usize;
        let tail = bsp.nodes[0];
        // A plane the line stays in front of: each chain node just passes it on.
        bsp.planes.push(Plane {
            normal: Vec3::X,
            dist: -1000.0,
            plane_type: 0,
        });
        bsp.nodes = (0..depth)
            .map(|i| Node {
                plane: 1,
                children: [(i + 1) as i16, (i + 1) as i16],
                num_faces: 0,
                ..tail
            })
            .collect();
        bsp.nodes.push(tail);
        let start = Vec3::new(32.0, 32.0, 10.0);
        let end = Vec3::new(32.0, 32.0, -10.0);
        assert_eq!(WorldCollision::from_bsp(&bsp).trace_texture(0, start, end), None);
        // The same line is found when the chain is short.
        bsp.nodes.drain(5..depth);
        for (i, n) in bsp.nodes.iter_mut().enumerate().take(5) {
            n.children = [(i + 1) as i16, (i + 1) as i16];
        }
        assert_eq!(
            WorldCollision::from_bsp(&bsp).trace_texture(0, start, end),
            Some("floor")
        );
    }
}
