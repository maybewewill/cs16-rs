//! The physent traces of `cs_pm::physent` on a hand-made BSP, with no engine, server or game
//! behind them: what a client needs to predict. The world is solid below `z = 0` in every hull;
//! one box entity stands in the way.

use cs_assets::bsp::{BspMap, BspModel, ClipNode, LightmapAtlas, Leaf, Node, Plane};
use cs_pm::collision::{CONTENTS_EMPTY, CONTENTS_SOLID, Hull, WorldCollision};
use cs_pm::physent::{
    PM_NORMAL, PM_WORLD_ONLY, PhysEnt, PhysEntView, PhysModel, player_trace, point_contents,
    test_player_position, world_physent,
};
use glam::Vec3;

/// `SOLID_BBOX`.
const SOLID_BBOX: i32 = 2;

/// Leaf 0 is solid, leaf 1 empty; a node child `-(leaf + 1)` names a leaf.
fn leaf(contents: i32) -> Leaf {
    Leaf {
        contents,
        vis_offset: -1,
        mins: [0; 3],
        maxs: [0; 3],
        first_mark_surface: 0,
        num_mark_surfaces: 0,
        ambient_levels: [0; 4],
    }
}

fn z_plane(dist: f32) -> Plane {
    Plane {
        normal: Vec3::Z,
        dist,
        plane_type: 2,
    }
}

/// A world that is solid below `z = 0` (grown by the half height of each player hull).
fn floor_bsp() -> BspMap {
    let hulls = [Hull::Standing, Hull::Large, Hull::Crouched];
    let mut planes = vec![z_plane(0.0)];
    let mut clipnodes = Vec::new();
    let mut head_nodes = vec![0];
    for hull in hulls {
        planes.push(z_plane(hull.half_extents().z));
        head_nodes.push(clipnodes.len() as i32);
        clipnodes.push(ClipNode {
            plane: planes.len() as i32 - 1,
            children: [CONTENTS_EMPTY as i16, CONTENTS_SOLID as i16],
        });
    }
    // `hull_range` ends a hull at the last node, which therefore must not be a root: one spare
    // node and clipnode at the end, never reached.
    let spare_node = Node {
        plane: 0,
        children: [-1, -1],
        mins: [0; 3],
        maxs: [0; 3],
        first_face: 0,
        num_faces: 0,
    };
    clipnodes.push(ClipNode {
        plane: 0,
        children: [CONTENTS_SOLID as i16, CONTENTS_SOLID as i16],
    });
    BspMap {
        name: "pmtest".into(),
        skyname: String::new(),
        entities: Vec::new(),
        spawns: Vec::new(),
        hostage_spawns: Vec::new(),
        brush_entities: Vec::new(),
        planes,
        vertices_raw: Vec::new(),
        visibility: Vec::new(),
        nodes: vec![
            Node {
                plane: 0,
                children: [-2, -1],
                mins: [0; 3],
                maxs: [0; 3],
                first_face: 0,
                num_faces: 0,
            },
            spare_node,
        ],
        texinfos: Vec::new(),
        faces: Vec::new(),
        lighting: Vec::new(),
        clipnodes,
        leafs: vec![leaf(CONTENTS_SOLID), leaf(CONTENTS_EMPTY)],
        mark_surfaces: Vec::new(),
        edges: Vec::new(),
        surf_edges: Vec::new(),
        models: vec![BspModel {
            mins: Vec3::splat(-4096.0),
            maxs: Vec3::splat(4096.0),
            origin: Vec3::ZERO,
            head_nodes: [head_nodes[0], head_nodes[1], head_nodes[2], head_nodes[3]],
            vis_leafs: 1,
            first_face: 0,
            num_faces: 0,
        }],
        textures: Vec::new(),
        lightmap_atlas: LightmapAtlas {
            width: 0,
            height: 0,
            rgba: Vec::new(),
            packed_faces: 0,
        },
        render_vertices: Vec::new(),
        render_indices: Vec::new(),
        batches: Vec::new(),
    }
}

/// A `SOLID_BBOX` entity at `origin`, 32 units wide.
fn box_ent(info: i32, origin: Vec3) -> PhysEnt {
    PhysEnt {
        view: PhysEntView {
            info,
            player: 0,
            origin,
            angles: Vec3::ZERO,
            mins: Vec3::splat(-16.0),
            maxs: Vec3::splat(16.0),
            solid: SOLID_BBOX,
            skin: 0,
            rendermode: 0,
            movetype: 0,
            is_brush_model: false,
            model_mins: Vec3::ZERO,
            model_maxs: Vec3::ZERO,
        },
        model: PhysModel::None,
    }
}

#[test]
fn pm_trace_on_a_bsp_without_the_engine() {
    let coll = WorldCollision::from_bsp(&floor_bsp());
    let ents = [
        world_physent(Some(&coll)),
        box_ent(1, Vec3::new(200.0, 0.0, 100.0)),
    ];

    // Straight down onto the floor: the standing hull rests 36 units above it.
    let down = player_trace(
        &coll,
        &ents,
        Vec3::new(0.0, 0.0, 100.0),
        Vec3::new(0.0, 0.0, 0.0),
        0,
        PM_NORMAL,
        -1,
    );
    assert_eq!(down.ent, 0);
    assert_eq!(down.plane_normal, Vec3::Z);
    assert!((0.63..0.65).contains(&down.fraction), "{}", down.fraction);
    assert!((36.0..36.1).contains(&down.endpos.z), "{}", down.endpos.z);

    // Sideways into the box entity: it is grown by the player's half width (16 + 16).
    let side_start = Vec3::new(100.0, 0.0, 100.0);
    let side_end = Vec3::new(300.0, 0.0, 100.0);
    let side = player_trace(&coll, &ents, side_start, side_end, 0, PM_NORMAL, -1);
    assert_eq!(side.ent, 1);
    assert_eq!(side.plane_normal, -Vec3::X);
    assert!((side.endpos.x - 168.0).abs() < 0.1, "{}", side.endpos.x);

    // The same trace with the box ignored, or with the world alone, hits nothing.
    let ignored = player_trace(&coll, &ents, side_start, side_end, 0, PM_NORMAL, 1);
    assert_eq!((ignored.fraction, ignored.ent), (1.0, -1));
    let world_only = player_trace(&coll, &ents, side_start, side_end, 0, PM_WORLD_ONLY, -1);
    assert_eq!((world_only.fraction, world_only.ent), (1.0, -1));
    assert_eq!(world_only.plane_normal, Vec3::ZERO);

    // Contents and position tests.
    assert_eq!(point_contents(&coll, &ents, Vec3::new(0.0, 0.0, 50.0)).0, CONTENTS_EMPTY);
    assert_eq!(point_contents(&coll, &ents, Vec3::new(0.0, 0.0, -50.0)).0, CONTENTS_SOLID);
    let origin = Vec3::new(0.0, 0.0, 50.0);
    assert_eq!(test_player_position(&coll, &ents, origin, origin, 0).0, -1);
    let buried = Vec3::new(0.0, 0.0, 10.0);
    assert_eq!(test_player_position(&coll, &ents, origin, buried, 0).0, 0);
    let in_box = Vec3::new(200.0, 0.0, 100.0);
    assert_eq!(test_player_position(&coll, &ents, origin, in_box, 0).0, 1);
}
