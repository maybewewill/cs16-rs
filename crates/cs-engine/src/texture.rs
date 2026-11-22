//! `TraceTexture`: the name of the texture a line hits on a brush model. Ported from ReHLDS
//! `rehlds/engine/pr_cmds.cpp` (`TraceTexture`, `SurfaceAtPoint`) and
//! `rehlds/engine/model.cpp` (`CalcSurfaceExtents`, for the `texturemins` and `extents`
//! the walk compares against), read 2026-11-22.
//!
//! The name is the one the map stores, up to 16 characters, with the `{`, `!`, `-0` and similar
//! prefixes still on: cutting them is the job of the caller (`pm_shared`'s
//! `PM_FindTextureType`).

use cs_assets::bsp::{BspMap, Face, TexInfo};
use glam::Vec3;

use crate::edict::EdictId;
use crate::game::EngineApi;
use crate::trace_ents::{angle_vectors, brush_model};

/// A face's `texturemins` and `extents` (`msurface_t`), as `CalcSurfaceExtents` makes them.
struct SurfaceExtents {
    texturemins: [i32; 2],
    extents: [i32; 2],
}

/// `CalcSurfaceExtents` (`rehlds/engine/model.cpp`, read 2026-11-22): the texture-space box of
/// the face, in whole 16-unit lumps. The engine does this at load for every face; here it is
/// done for the faces a trace looks at. The dot products are taken in double and stored as
/// `float`, as the source has it (the `FIXED: loss of floating point` there). `None` for a face
/// whose edges or vertices are outside the map's arrays.
fn surface_extents(bsp: &BspMap, face: &Face, tex: &TexInfo) -> Option<SurfaceExtents> {
    let mut mins = [999_999.0f32; 2];
    let mut maxs = [-99_999.0f32; 2];
    let first = usize::try_from(face.first_edge).ok()?;
    for i in 0..usize::from(face.num_edges) {
        let e = *bsp.surf_edges.get(first.checked_add(i)?)?;
        let vertex = if e >= 0 {
            bsp.edges.get(e as usize)?[0]
        } else {
            bsp.edges.get(e.unsigned_abs() as usize)?[1]
        };
        let p = *bsp.vertices_raw.get(usize::from(vertex))?;
        for (j, (vec, shift)) in [(tex.s_vec, tex.s_shift), (tex.t_vec, tex.t_shift)]
            .into_iter()
            .enumerate()
        {
            let val = (f64::from(p.x) * f64::from(vec.x)
                + f64::from(p.y) * f64::from(vec.y)
                + f64::from(p.z) * f64::from(vec.z)
                + f64::from(shift)) as f32;
            if val < mins[j] {
                mins[j] = val;
            }
            if val > maxs[j] {
                maxs[j] = val;
            }
        }
    }
    let mut out = SurfaceExtents {
        texturemins: [0; 2],
        extents: [0; 2],
    };
    for i in 0..2 {
        let bmins = (mins[i] / 16.0).floor() as i32;
        let bmaxs = (maxs[i] / 16.0).ceil() as i32;
        out.texturemins[i] = bmins.wrapping_mul(16);
        out.extents[i] = bmaxs.wrapping_sub(bmins).wrapping_mul(16);
    }
    Some(out)
}

/// `SurfaceAtPoint` (`rehlds/engine/pr_cmds.cpp`, read 2026-11-22): walks the node tree from
/// `node` along the line `start`..`end`, nearest side first, and gives the first face whose
/// texture-space box holds the point where the line crosses the node's plane (the source's
/// order: the near child, then the node's own faces, then the far child). `None` in a leaf.
///
/// `budget` counts the nodes visited: a valid tree is walked in at most as many steps as it has
/// nodes, so a map whose nodes lead back to themselves ends the walk instead of the process.
fn surface_at_point(
    bsp: &BspMap,
    node: i32,
    start: Vec3,
    end: Vec3,
    budget: &mut usize,
) -> Option<usize> {
    if node < 0 {
        // `node->contents < 0`: a leaf.
        return None;
    }
    *budget = budget.checked_sub(1)?;
    let n = bsp.nodes.get(node as usize)?;
    let plane = bsp.planes.get(n.plane as usize)?;
    let front = start.dot(plane.normal) - plane.dist;
    let back = end.dot(plane.normal) - plane.dist;
    let s = usize::from(front < 0.0);
    let t = usize::from(back < 0.0);
    if t == s {
        return surface_at_point(bsp, i32::from(n.children[s]), start, end, budget);
    }
    let frac = front / (front - back);
    let mid = Vec3::new(
        (end.x - start.x) * frac + start.x,
        (end.y - start.y) * frac + start.y,
        (end.z - start.z) * frac + start.z,
    );
    if let Some(face) = surface_at_point(bsp, i32::from(n.children[s]), start, mid, budget) {
        return Some(face);
    }
    let first = usize::from(n.first_face);
    for i in 0..usize::from(n.num_faces) {
        let index = first + i;
        let Some(face) = bsp.faces.get(index) else {
            continue;
        };
        let Some(tex) = bsp.texinfos.get(usize::from(face.texinfo)) else {
            continue;
        };
        let Some(extents) = surface_extents(bsp, face, tex) else {
            continue;
        };
        // `(int)` truncates toward zero.
        let ds = (mid.dot(tex.s_vec) + tex.s_shift) as i32;
        let dt = (mid.dot(tex.t_vec) + tex.t_shift) as i32;
        if ds >= extents.texturemins[0]
            && dt >= extents.texturemins[1]
            && ds - extents.texturemins[0] <= extents.extents[0]
            && dt - extents.texturemins[1] <= extents.extents[1]
        {
            return Some(index);
        }
    }
    surface_at_point(bsp, i32::from(n.children[s ^ 1]), mid, end, budget)
}

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
        let bsp = self.map.bsp()?;
        let root = bsp.models.get(model)?.head_nodes[0];
        // `SV_HullForBsp` with a zero-size box: the point hull's offset is the entity's origin.
        let mut start = start - v.origin;
        let mut end = end - v.origin;
        if v.angles != Vec3::ZERO {
            let (forward, right, up) = angle_vectors(v.angles);
            let turn = |p: Vec3| Vec3::new(forward.dot(p), -right.dot(p), up.dot(p));
            start = turn(start);
            end = turn(end);
        }
        let mut budget = bsp.nodes.len();
        let face = surface_at_point(bsp, root, start, end, &mut budget)?;
        let tex = bsp.texinfos.get(usize::from(bsp.faces.get(face)?.texinfo))?;
        let texture = bsp.textures.get(usize::try_from(tex.miptex).ok()?)?;
        Some(texture.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use cs_assets::bsp::{BspMap, Face, Node, Plane, TexInfo};
    use cs_assets::wad::DecodedTexture;

    use crate::world::tests::strip_bsp;

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
        let e = surface_extents(&bsp, &bsp.faces[0], &bsp.texinfos[0]).unwrap();
        assert_eq!((e.texturemins, e.extents), ([0, 0], [64, 64]));
    }

    #[test]
    fn the_walk_finds_the_face_the_line_crosses_inside_its_box() {
        let bsp = one_face_bsp();
        let down = |x: f32, y: f32| {
            let mut budget = bsp.nodes.len();
            surface_at_point(
                &bsp,
                0,
                Vec3::new(x, y, 10.0),
                Vec3::new(x, y, -10.0),
                &mut budget,
            )
        };
        assert_eq!(down(32.0, 32.0), Some(0));
        assert_eq!(down(100.0, 32.0), None);
        // A line that stays on one side of the plane crosses nothing.
        let mut budget = bsp.nodes.len();
        let start = Vec3::new(32.0, 32.0, 10.0);
        assert_eq!(
            surface_at_point(&bsp, 0, start, start + Vec3::X, &mut budget),
            None
        );
    }

    #[test]
    fn a_tree_that_leads_back_to_itself_ends() {
        let mut bsp = one_face_bsp();
        bsp.nodes[0].children = [0, 0];
        let mut budget = bsp.nodes.len();
        let found = surface_at_point(
            &bsp,
            0,
            Vec3::new(500.0, 500.0, 10.0),
            Vec3::new(500.0, 500.0, -10.0),
            &mut budget,
        );
        assert_eq!(found, None);
    }
}
