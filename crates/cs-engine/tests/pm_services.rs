//! The services the player movement gets from the engine (`pmove` physents, player traces,
//! point contents, ladders, `PM_TraceModel`), on a synthetic map built here and on de_dust2
//! (the real-map test returns early when the game is not installed).

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use common::{Harness, TestGame};
use cs_assets::bsp::{BspMap, BspModel, ClipNode, Leaf, LightmapAtlas, Node, Plane};
use cs_engine::collision::Hull;
use cs_engine::consts::{
    FL_CLIENT, FL_MONSTERCLIP, MOVETYPE_PUSH, SOLID_BBOX, SOLID_BSP, SOLID_NOT, SOLID_SLIDEBOX,
    SOLID_TRIGGER,
};
use cs_engine::{
    CONTENTS_LADDER, EdictId, EngineApi, PM_GLASS_IGNORE, PM_NORMAL, PM_PLAYER_MAXS,
    PM_PLAYER_MINS, PM_WORLD_ONLY, PmPhysEnts, ServerConfig, ServerMap,
};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

const SOLID: i32 = -2;
const EMPTY: i32 = -1;
const WATER: i32 = -3;
/// `CONTENTS_CURRENT_90`: a current, which `PM_PointContents` reports as water.
const CURRENT_90: i32 = -10;

/// Leaves of the synthetic map; a leaf's index is the child `-(index + 1)` of a node.
const LEAF_CONTENTS: [i32; 4] = [SOLID, EMPTY, WATER, CURRENT_90];

// The brush models of the synthetic map (model 0 is the world).
const MODEL_LADDER: usize = 1;
const MODEL_WATER: usize = 2;
const MODEL_BLOCK: usize = 3;
const MODEL_SLAB: usize = 4;

/// A space partition the builder turns into the nodes of the point hull and the clipnodes of
/// the other hulls.
enum Tree {
    Leaf(i32),
    /// `front` is the side with `p[axis] >= dist`.
    Split {
        axis: usize,
        dist: f32,
        front: Box<Tree>,
        back: Box<Tree>,
    },
}

fn split(axis: usize, dist: f32, front: Tree, back: Tree) -> Tree {
    Tree::Split {
        axis,
        dist,
        front: Box::new(front),
        back: Box::new(back),
    }
}

/// The brush `mins..maxs` grown by `e` on every side (the hull of a box brush for a player
/// hull of half-size `e`), `inside` within, empty outside.
fn box_tree(mins: Vec3, maxs: Vec3, inside: i32, e: Vec3) -> Tree {
    let z = split(
        2,
        mins.z - e.z,
        split(2, maxs.z + e.z, Tree::Leaf(EMPTY), Tree::Leaf(inside)),
        Tree::Leaf(EMPTY),
    );
    let y = split(
        1,
        mins.y - e.y,
        split(1, maxs.y + e.y, Tree::Leaf(EMPTY), z),
        Tree::Leaf(EMPTY),
    );
    split(
        0,
        mins.x - e.x,
        split(0, maxs.x + e.x, Tree::Leaf(EMPTY), y),
        Tree::Leaf(EMPTY),
    )
}

/// The world: solid below `z = 0`, a slab `x 100..200, z 40..100` over every `y`, and a
/// current (contents -10, in every hull) for `y >= 500`.
fn world_tree(e: Vec3) -> Tree {
    let slab = split(
        0,
        100.0 - e.x,
        split(
            0,
            200.0 + e.x,
            Tree::Leaf(EMPTY),
            split(
                2,
                40.0 - e.z,
                split(2, 100.0 + e.z, Tree::Leaf(EMPTY), Tree::Leaf(SOLID)),
                Tree::Leaf(EMPTY),
            ),
        ),
        Tree::Leaf(EMPTY),
    );
    split(
        1,
        500.0,
        Tree::Leaf(CURRENT_90),
        split(2, e.z, slab, Tree::Leaf(SOLID)),
    )
}

#[derive(Default)]
struct Builder {
    planes: Vec<Plane>,
    nodes: Vec<Node>,
    clipnodes: Vec<ClipNode>,
}

impl Builder {
    fn plane(&mut self, axis: usize, dist: f32) -> usize {
        let mut normal = Vec3::ZERO;
        normal[axis] = 1.0;
        self.planes.push(Plane {
            normal,
            dist,
            plane_type: axis as i32,
        });
        self.planes.len() - 1
    }

    /// The tree as nodes of the point hull (pre-order, so the root is not the last node);
    /// returns the child reference of its root.
    fn emit_nodes(&mut self, tree: &Tree) -> i16 {
        match tree {
            Tree::Leaf(c) => {
                let leaf = LEAF_CONTENTS
                    .iter()
                    .position(|l| l == c)
                    .expect("known leaf");
                -(leaf as i16) - 1
            }
            Tree::Split {
                axis,
                dist,
                front,
                back,
            } => {
                let plane = self.plane(*axis, *dist) as u32;
                let index = self.nodes.len();
                self.nodes.push(Node {
                    plane,
                    children: [0, 0],
                    mins: [0; 3],
                    maxs: [0; 3],
                    first_face: 0,
                    num_faces: 0,
                });
                let f = self.emit_nodes(front);
                let b = self.emit_nodes(back);
                self.nodes[index].children = [f, b];
                index as i16
            }
        }
    }

    /// The tree as clipnodes (children hold the contents of a leaf directly).
    fn emit_clip(&mut self, tree: &Tree) -> i16 {
        match tree {
            Tree::Leaf(c) => *c as i16,
            Tree::Split {
                axis,
                dist,
                front,
                back,
            } => {
                let plane = self.plane(*axis, *dist) as i32;
                let index = self.clipnodes.len();
                self.clipnodes.push(ClipNode {
                    plane,
                    children: [0, 0],
                });
                let f = self.emit_clip(front);
                let b = self.emit_clip(back);
                self.clipnodes[index].children = [f, b];
                index as i16
            }
        }
    }

    /// A model whose four hulls come from `tree_for(half extents of the hull)`.
    fn model(&mut self, mins: Vec3, maxs: Vec3, tree_for: impl Fn(Vec3) -> Tree) -> BspModel {
        let point = self.emit_nodes(&tree_for(Vec3::ZERO));
        let standing = self.emit_clip(&tree_for(Hull::Standing.half_extents()));
        let large = self.emit_clip(&tree_for(Hull::Large.half_extents()));
        let crouched = self.emit_clip(&tree_for(Hull::Crouched.half_extents()));
        BspModel {
            mins,
            maxs,
            origin: Vec3::ZERO,
            head_nodes: [
                i32::from(point),
                i32::from(standing),
                i32::from(large),
                i32::from(crouched),
            ],
            vis_leafs: 1,
            first_face: 0,
            num_faces: 0,
        }
    }
}

/// The half-extents of a brush model's box, by model.
fn brush_box(model: usize) -> (Vec3, Vec3) {
    match model {
        MODEL_LADDER => (Vec3::new(-8.0, -8.0, -32.0), Vec3::new(8.0, 8.0, 32.0)),
        MODEL_WATER => (Vec3::new(-32.0, -32.0, -32.0), Vec3::new(32.0, 32.0, 32.0)),
        MODEL_BLOCK => (Vec3::new(-16.0, -16.0, -32.0), Vec3::new(16.0, 16.0, 32.0)),
        MODEL_SLAB => (Vec3::new(-32.0, -8.0, -32.0), Vec3::new(32.0, 8.0, 32.0)),
        _ => unreachable!("no such model"),
    }
}

/// The synthetic map. The "ladder" brush is solid in every hull, standing in for the contents
/// the map compiler gives a `func_ladder`, so that `PM_TraceModel` has something to hit.
fn synthetic_bsp() -> BspMap {
    let mut b = Builder::default();
    let mut models = vec![b.model(Vec3::splat(-4096.0), Vec3::splat(4096.0), world_tree)];
    for (model, inside) in [
        (MODEL_LADDER, SOLID),
        (MODEL_WATER, WATER),
        (MODEL_BLOCK, SOLID),
        (MODEL_SLAB, SOLID),
    ] {
        let (mins, maxs) = brush_box(model);
        models.push(b.model(mins, maxs, |e| box_tree(mins, maxs, inside, e)));
    }
    let leaf = |contents: i32| Leaf {
        contents,
        vis_offset: -1,
        mins: [0; 3],
        maxs: [0; 3],
        first_mark_surface: 0,
        num_mark_surfaces: 0,
        ambient_levels: [0; 4],
    };
    BspMap {
        name: "pmtest".into(),
        skyname: String::new(),
        entities: Vec::new(),
        spawns: Vec::new(),
        hostage_spawns: Vec::new(),
        brush_entities: Vec::new(),
        planes: b.planes,
        vertices_raw: Vec::new(),
        visibility: Vec::new(),
        nodes: b.nodes,
        texinfos: Vec::new(),
        faces: Vec::new(),
        lighting: Vec::new(),
        clipnodes: b.clipnodes,
        leafs: LEAF_CONTENTS.iter().map(|&c| leaf(c)).collect(),
        mark_surfaces: Vec::new(),
        edges: Vec::new(),
        surf_edges: Vec::new(),
        models,
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

fn harness_on(name: &str, raw: &[u8], bsp: BspMap) -> Harness {
    let map = ServerMap::from_bsp(name, raw, Arc::new(bsp)).expect("valid BSP header");
    let cfg = ServerConfig {
        max_clients: 2,
        ..ServerConfig::default()
    };
    Harness::with_map(cfg, map, TestGame::default())
}

/// A BSP file that has nothing but a valid header (the checksum reads only that).
fn header_only() -> Vec<u8> {
    let mut raw = vec![0u8; 4 + 15 * 8];
    raw[..4].copy_from_slice(&30i32.to_le_bytes());
    raw
}

fn synthetic() -> Harness {
    harness_on("pmtest", &header_only(), synthetic_bsp())
}

fn dust2_harness() -> Option<Harness> {
    let vfs = Vfs::discover(None, VfsConfig::default()).ok()?;
    let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("de_dust2 reads");
    let bsp = BspMap::load(&vfs, "de_dust2").expect("de_dust2 parses");
    Some(harness_on("de_dust2", &raw, bsp))
}

/// Runs `f` as the game at the start of the next frame and returns what it returned.
fn run<T: 'static>(h: &mut Harness, f: impl FnOnce(&mut EngineApi<'_>) -> T + 'static) -> T {
    let out = Rc::new(RefCell::new(None));
    let sink = Rc::clone(&out);
    h.server
        .game_mut()
        .act(move |eng| *sink.borrow_mut() = Some(f(eng)));
    h.step(&mut [], 1);
    let result = out.borrow_mut().take();
    result.expect("the game ran the action")
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.001
}

/// The edict the tests move around as "the player": slot 0.
fn player(eng: &mut EngineApi<'_>, origin: Vec3) -> EdictId {
    let id = eng.client_edict(0);
    eng.set_origin(id, origin);
    id
}

/// A solid box entity of half-size `half` linked at `origin`.
fn boxed(eng: &mut EngineApi<'_>, solid: u8, origin: Vec3, half: f32) -> EdictId {
    let id = eng.create_entity("test_box").expect("free edict");
    eng.ent_mut(id).unwrap().solid = solid;
    eng.set_size(id, Vec3::splat(-half), Vec3::splat(half));
    eng.set_origin(id, origin);
    id
}

/// An entity with brush model `model` of the synthetic map, linked at `origin`.
fn brush(eng: &mut EngineApi<'_>, model: usize, solid: u8, skin: i16, origin: Vec3) -> EdictId {
    let id = eng.create_entity("test_brush").expect("free edict");
    assert!(eng.set_model(id, &format!("*{model}")));
    {
        let v = eng.ent_mut(id).unwrap();
        v.solid = solid;
        v.skin = skin;
        v.movetype = MOVETYPE_PUSH;
    }
    eng.set_origin(id, origin);
    id
}

/// The index in `pe`'s physents of the one for `id`.
fn index_of(pe: &PmPhysEnts, id: EdictId) -> Option<i32> {
    (0..pe.num_physents() as i32).find(|&i| pe.edict_of(i) == Some(id))
}

#[test]
fn physents_start_with_the_world_and_keep_ladders_apart() {
    let mut h = synthetic();
    run(&mut h, |eng| {
        let p = player(eng, Vec3::new(0.0, 0.0, 50.0));
        let ladder = brush(
            eng,
            MODEL_LADDER,
            SOLID_NOT,
            CONTENTS_LADDER as i16,
            Vec3::new(60.0, 0.0, 32.0),
        );
        let water = brush(eng, MODEL_WATER, SOLID_NOT, -3, Vec3::new(-80.0, 0.0, 32.0));
        let door = brush(eng, MODEL_BLOCK, SOLID_BSP, 0, Vec3::new(0.0, 80.0, 32.0));
        let crate_ = boxed(eng, SOLID_BBOX, Vec3::new(0.0, -80.0, 50.0), 16.0);

        let pe = eng.pm_physents(p);
        let world = pe.physent(0).expect("the world is physent 0");
        assert_eq!((world.info, world.player, world.solid), (0, 0, 4));
        assert_eq!(world.movetype, 0);
        assert!(world.is_brush_model);
        assert_eq!(world.model_maxs, Vec3::splat(4096.0));
        assert_eq!(pe.edict_of(0), Some(eng.world()));

        // world, water, door and crate are physents; the ladder is a moveent alone.
        assert_eq!(pe.num_physents(), 4);
        assert_eq!(pe.num_moveents(), 1);
        assert!(index_of(&pe, ladder).is_none());
        for id in [water, door, crate_] {
            let i = index_of(&pe, id).expect("a physent");
            let view = pe.physent(i).unwrap();
            assert_eq!(view.info, i32::from(id.index));
            assert_eq!(view.player, 0);
        }
        let l = pe.moveent(0).expect("the ladder");
        assert_eq!(l.info, i32::from(ladder.index));
        assert_eq!(l.skin, CONTENTS_LADDER);
        assert!(l.is_brush_model);
        assert_eq!(l.origin, Vec3::new(60.0, 0.0, 32.0));
        let w = pe.physent(index_of(&pe, water).unwrap()).unwrap();
        assert_eq!((w.solid, w.skin), (0, -3));
        let c = pe.physent(index_of(&pe, crate_).unwrap()).unwrap();
        assert!(!c.is_brush_model);
        assert_eq!((c.mins, c.maxs), (Vec3::splat(-16.0), Vec3::splat(16.0)));
        assert!(pe.moveent(1).is_none());
        assert!(pe.physent(4).is_none());
    });
}

#[test]
fn dead_and_distant_entities_are_not_physents() {
    let mut h = synthetic();
    run(&mut h, |eng| {
        let p = player(eng, Vec3::new(0.0, 0.0, 50.0));
        // Another player: in while it lives, out when dead.
        let other = eng.client_edict(1);
        {
            let v = eng.ent_mut(other).unwrap();
            v.flags |= FL_CLIENT;
            v.health = 100.0;
            v.solid = SOLID_SLIDEBOX;
        }
        eng.set_size(other, PM_PLAYER_MINS[0], PM_PLAYER_MAXS[0]);
        eng.set_origin(other, Vec3::new(40.0, 0.0, 50.0));
        let pe = eng.pm_physents(p);
        let i = index_of(&pe, other).expect("a living player is a physent");
        assert_eq!(pe.physent(i).unwrap().player, i32::from(other.index));
        eng.ent_mut(other).unwrap().health = 0.0;
        assert!(index_of(&eng.pm_physents(p), other).is_none());

        // Out of the 256-unit reach (and beyond the map's neighbours), or in it but not
        // colliding: all left out.
        let far = boxed(eng, SOLID_BBOX, Vec3::new(1000.0, 0.0, 50.0), 16.0);
        let edge = boxed(eng, SOLID_BBOX, Vec3::new(240.0, 0.0, 50.0), 8.0);
        let trigger = boxed(eng, SOLID_TRIGGER, Vec3::new(30.0, 30.0, 50.0), 8.0);
        let ghost = boxed(eng, SOLID_NOT, Vec3::new(-30.0, 30.0, 50.0), 8.0);
        let owned = boxed(eng, SOLID_BBOX, Vec3::new(-30.0, -30.0, 50.0), 8.0);
        eng.ent_mut(owned).unwrap().owner = Some(p);
        let flat = boxed(eng, SOLID_BBOX, Vec3::new(0.0, 60.0, 50.0), 8.0);
        {
            let v = eng.ent_mut(flat).unwrap();
            v.mins.z = 0.0;
            v.maxs.z = 1.0;
        }
        eng.set_origin(flat, Vec3::new(0.0, 60.0, 50.0));
        let points = boxed(eng, SOLID_BBOX, Vec3::new(0.0, -60.0, 50.0), 0.0);
        let monster_clip = brush(eng, MODEL_BLOCK, SOLID_BSP, 0, Vec3::new(60.0, 60.0, 32.0));
        eng.ent_mut(monster_clip).unwrap().flags |= FL_MONSTERCLIP;

        let pe = eng.pm_physents(p);
        assert!(index_of(&pe, edge).is_some(), "256 units is within reach");
        for (name, id) in [
            ("far", far),
            ("trigger", trigger),
            ("ghost", ghost),
            ("owned", owned),
            ("flat", flat),
            ("points", points),
            ("monster clip", monster_clip),
            ("the player itself", p),
        ] {
            assert!(index_of(&pe, id).is_none(), "{name} is not a physent");
        }
        assert_eq!(pe.num_physents(), 2, "the world and the edge box");
        assert_eq!(pe.num_moveents(), 0);
    });
}

#[test]
fn pm_trace_maps_usehull_to_bsp_hulls() {
    let mut h = synthetic();
    run(&mut h, |eng| {
        let p = player(eng, Vec3::new(0.0, 0.0, 50.0));
        let pe = eng.pm_physents(p);
        let go = |z: f32, usehull: i32| {
            eng.pm_player_trace(
                &pe,
                Vec3::new(0.0, 0.0, z),
                Vec3::new(300.0, 0.0, z),
                usehull,
                PM_NORMAL,
                -1,
            )
        };

        // The slab hangs 40 units over the floor. A ducking player (hull 1 of the player
        // tables, BSP hull 3) walks under it at z = 20 ...
        let ducking = go(20.0, 1);
        assert_eq!(ducking.fraction, 1.0);
        assert_eq!(ducking.ent, -1);
        // ... a standing one (usehull 0, BSP hull 1) cannot: at z = 38 he is stopped 16
        // units short of the slab, by the world.
        let standing = go(38.0, 0);
        assert!(standing.fraction < 1.0);
        assert_eq!(standing.ent, 0);
        assert!(near(standing.endpos.x, 83.96875), "{:?}", standing.endpos);
        assert_eq!(standing.plane_normal, Vec3::new(-1.0, 0.0, 0.0));
        assert_eq!(standing.plane_dist, -84.0);
        // The point hull (usehull 2, BSP hull 0) goes to the slab itself ...
        let point = go(45.0, 2);
        assert!(near(point.endpos.x, 99.96875), "{:?}", point.endpos);
        // ... and the large hull (usehull 3, BSP hull 2) stops 32 short of it.
        let large = go(33.0, 3);
        assert!(near(large.endpos.x, 67.96875), "{:?}", large.endpos);
        // A hull that does not exist hits nothing.
        for bad in [-1, 4, 99, i32::MIN, i32::MAX] {
            let t = go(38.0, bad);
            assert_eq!((t.fraction, t.ent), (1.0, -1), "usehull {bad}");
        }
    });
}

#[test]
fn pm_trace_flags_world_only_and_glass() {
    let mut h = synthetic();
    run(&mut h, |eng| {
        let p = player(eng, Vec3::new(0.0, 0.0, 20.0));
        // Glass (a solid brush with a render mode) at x 24..56, a plain crate behind it.
        let glass = brush(eng, MODEL_BLOCK, SOLID_BSP, 0, Vec3::new(40.0, 0.0, 20.0));
        eng.ent_mut(glass).unwrap().rendermode = 2;
        let wood = boxed(eng, SOLID_BBOX, Vec3::new(80.0, 0.0, 20.0), 8.0);
        // The slab's face is at x = 100 for the point hull; trace at z = 45 for the world.
        let pe = eng.pm_physents(p);
        let glass_i = index_of(&pe, glass).unwrap();
        let wood_i = index_of(&pe, wood).unwrap();
        let from = Vec3::new(0.0, 0.0, 20.0);
        let to = Vec3::new(300.0, 0.0, 20.0);
        let trace = |flags: i32| eng.pm_player_trace(&pe, from, to, 2, flags, -1);

        let normal = trace(PM_NORMAL);
        assert_eq!(normal.ent, glass_i);
        assert!(near(normal.endpos.x, 23.96875), "{:?}", normal.endpos);
        let no_glass = trace(PM_GLASS_IGNORE);
        assert_eq!(no_glass.ent, wood_i, "the glass is looked through");
        assert!(near(no_glass.endpos.x, 71.96875));
        let world_only = trace(PM_WORLD_ONLY);
        assert_eq!((world_only.fraction, world_only.ent), (1.0, -1));
        // Flags combine, and a physent can be ignored by index.
        assert_eq!(trace(PM_GLASS_IGNORE | PM_WORLD_ONLY).ent, -1);
        let skip_glass = eng.pm_player_trace(&pe, from, to, 2, PM_NORMAL, glass_i);
        assert_eq!(skip_glass.ent, wood_i);
        let skip_both_idx = eng.pm_player_trace(&pe, from, to, 2, PM_GLASS_IGNORE, wood_i);
        assert_eq!(skip_both_idx.ent, -1, "nothing left but the empty world");
    });
}

#[test]
fn pm_trace_rotated_brush_uses_the_rotation() {
    let mut h = synthetic();
    run(&mut h, |eng| {
        let p = player(eng, Vec3::new(0.0, 0.0, 32.0));
        // A slab 64 wide in x and 16 thick in y, turned a quarter: now 16 thick in x.
        let slab = brush(eng, MODEL_SLAB, SOLID_BSP, 0, Vec3::new(200.0, 0.0, 32.0));
        let from = Vec3::new(0.0, 0.0, 32.0);
        let to = Vec3::new(300.0, 0.0, 32.0);
        let pe = eng.pm_physents(p);
        let straight = eng.pm_player_trace(&pe, from, to, 2, PM_NORMAL, -1);
        assert!(near(straight.endpos.x, 167.96875), "{:?}", straight.endpos);
        assert!(near(straight.plane_normal.x, -1.0));

        eng.ent_mut(slab).unwrap().angles = Vec3::new(0.0, 90.0, 0.0);
        eng.set_origin(slab, Vec3::new(200.0, 0.0, 32.0));
        let pe = eng.pm_physents(p);
        let turned = eng.pm_player_trace(&pe, from, to, 2, PM_NORMAL, -1);
        assert!(near(turned.endpos.x, 191.96875), "{:?}", turned.endpos);
        assert!(
            near(turned.plane_normal.x, -1.0),
            "{:?}",
            turned.plane_normal
        );
        assert!(turned.plane_normal.y.abs() < 1e-5 && turned.plane_normal.z.abs() < 1e-5);
        // A point hull test inside the turned slab (x 192..208) but outside the straight one.
        let (hit, _) = eng.pm_test_player_position(&pe, from, Vec3::new(204.0, 20.0, 32.0), 2);
        assert_eq!(hit, index_of(&pe, slab).unwrap());
        let (clear, _) = eng.pm_test_player_position(&pe, from, Vec3::new(220.0, 0.0, 32.0), 2);
        assert_eq!(clear, -1);
    });
}

#[test]
fn pm_trace_quirks() {
    let mut h = synthetic();
    run(&mut h, |eng| {
        let p = player(eng, Vec3::new(0.0, 0.0, 50.0));
        // Two identical boxes: the one that comes first among the physents wins a tie.
        let a = boxed(eng, SOLID_BBOX, Vec3::new(100.0, 0.0, 50.0), 16.0);
        let b = boxed(eng, SOLID_BBOX, Vec3::new(100.0, 0.0, 50.0), 16.0);
        let pe = eng.pm_physents(p);
        let (ia, ib) = (index_of(&pe, a).unwrap(), index_of(&pe, b).unwrap());
        let from = Vec3::new(0.0, 0.0, 50.0);
        let to = Vec3::new(200.0, 0.0, 50.0);

        let tie = eng.pm_player_trace(&pe, from, to, 2, PM_NORMAL, -1);
        assert_eq!(tie.ent, ia.min(ib));
        assert!(near(tie.fraction, 83.96875 / 200.0), "{}", tie.fraction);
        assert_eq!(tie.endpos, Vec3::new(200.0, 0.0, 0.0) * tie.fraction + from);

        // A miss leaves the normal at zero and `ent` at -1.
        let miss = eng.pm_player_trace(&pe, from, Vec3::new(0.0, 0.0, 200.0), 2, PM_NORMAL, -1);
        assert_eq!(miss.fraction, 1.0);
        assert_eq!(miss.ent, -1);
        assert_eq!(miss.plane_normal, Vec3::ZERO);
        assert_eq!(miss.endpos, Vec3::new(0.0, 0.0, 200.0));
        assert!(!miss.startsolid && !miss.allsolid);

        // Starting in solid: fraction 0 and the trace is at the start. Never leaving it also
        // sets `allsolid`.
        let inside = Vec3::new(0.0, 0.0, -10.0);
        let all = eng.pm_player_trace(&pe, inside, inside + Vec3::X, 2, PM_NORMAL, -1);
        assert!(all.allsolid && all.startsolid);
        assert_eq!((all.fraction, all.ent, all.endpos), (0.0, 0, inside));
        let out = eng.pm_player_trace(&pe, inside, Vec3::new(0.0, 0.0, 40.0), 2, PM_NORMAL, -1);
        assert!(out.startsolid && !out.allsolid);
        assert_eq!((out.fraction, out.ent, out.endpos), (0.0, 0, inside));
        assert!(out.inopen, "it ends in the open");

        // Quirk: only a trace that stops somewhere is copied from the physent's, so a clear
        // one keeps `inopen` and `inwater` false, in air as through a current.
        assert!(!miss.inopen && !miss.inwater);
        let wet = eng.pm_player_trace(
            &pe,
            Vec3::new(0.0, 400.0, 50.0),
            Vec3::new(0.0, 600.0, 50.0),
            2,
            PM_NORMAL,
            -1,
        );
        assert!(!wet.inwater && !wet.inopen);
        assert_eq!(wet.fraction, 1.0);
    });
}

#[test]
fn test_player_position_traces_at_pm_origin() {
    let mut h = synthetic();
    run(&mut h, |eng| {
        let p = player(eng, Vec3::new(0.0, 0.0, 50.0));
        let pe = eng.pm_physents(p);
        let open = Vec3::new(0.0, 0.0, 50.0);
        let in_floor = Vec3::new(0.0, 0.0, -10.0);

        // The position in the floor is blocked by the world; the trace filled is the one at
        // the (free) pmove origin, not at the tested position.
        let (hit, tr) = eng.pm_test_player_position(&pe, open, in_floor, 2);
        assert_eq!(hit, 0);
        assert_eq!(tr.fraction, 1.0);
        assert!(!tr.startsolid && !tr.allsolid);
        assert_eq!(tr.endpos, open);

        // The other way round: a free position, a pmove origin in the floor.
        let (hit, tr) = eng.pm_test_player_position(&pe, in_floor, open, 2);
        assert_eq!(hit, -1);
        assert!(tr.startsolid && tr.allsolid);
        assert_eq!(tr.fraction, 0.0);

        // The hull counts: 20 up is inside the floor for a standing player (hull 0 is solid
        // below z = 36), not for a point or a ducking one (below z = 18).
        let low = Vec3::new(0.0, 0.0, 20.0);
        assert_eq!(eng.pm_test_player_position(&pe, open, low, 0).0, 0);
        assert_eq!(eng.pm_test_player_position(&pe, open, low, 1).0, -1);
        assert_eq!(eng.pm_test_player_position(&pe, open, low, 2).0, -1);
        // A physent entry is reported by its index: a box at the position.
        let blocker = boxed(eng, SOLID_BBOX, Vec3::new(0.0, 0.0, 120.0), 16.0);
        let pe = eng.pm_physents(p);
        let (hit, _) = eng.pm_test_player_position(&pe, open, Vec3::new(0.0, 0.0, 120.0), 2);
        assert_eq!(hit, index_of(&pe, blocker).unwrap());
    });
}

#[test]
fn point_contents_sees_water_brushes() {
    let mut h = synthetic();
    run(&mut h, |eng| {
        let p = player(eng, Vec3::new(0.0, 0.0, 50.0));
        let water = brush(eng, MODEL_WATER, SOLID_NOT, -3, Vec3::new(0.0, 0.0, 100.0));
        brush(
            eng,
            MODEL_LADDER,
            SOLID_NOT,
            CONTENTS_LADDER as i16,
            Vec3::new(100.0, -100.0, 32.0),
        );
        let pe = eng.pm_physents(p);
        // Air, the floor (solid wins), a current (water, but the true contents stay).
        assert_eq!(
            eng.pm_point_contents(&pe, Vec3::new(0.0, 0.0, 50.0)),
            (-1, -1)
        );
        assert_eq!(
            eng.pm_point_contents(&pe, Vec3::new(0.0, 0.0, -5.0)),
            (-2, -2)
        );
        assert_eq!(
            eng.pm_point_contents(&pe, Vec3::new(0.0, 550.0, 50.0)),
            (WATER, CURRENT_90)
        );
        // Inside the water brush: its own `skin`; the world there is empty.
        assert_eq!(
            eng.pm_point_contents(&pe, Vec3::new(0.0, 0.0, 100.0)),
            (-3, -1)
        );
        eng.ent_mut(water).unwrap().skin = -4;
        let pe = eng.pm_physents(p);
        assert_eq!(
            eng.pm_point_contents(&pe, Vec3::new(0.0, 0.0, 100.0)),
            (-4, -1)
        );
        // Just outside the brush.
        assert_eq!(
            eng.pm_point_contents(&pe, Vec3::new(40.0, 0.0, 100.0)),
            (-1, -1)
        );
        // A water brush half in the floor: solid still wins.
        eng.set_origin(water, Vec3::new(0.0, 0.0, 10.0));
        let pe = eng.pm_physents(p);
        assert_eq!(
            eng.pm_point_contents(&pe, Vec3::new(0.0, 0.0, -5.0)),
            (-2, -2)
        );
        assert_eq!(
            eng.pm_point_contents(&pe, Vec3::new(0.0, 0.0, 5.0)),
            (-4, -1)
        );
        // A ladder is not part of the contents (it is not a physent).
        assert_eq!(
            eng.pm_point_contents(&pe, Vec3::new(100.0, -100.0, 32.0)),
            (-1, -1)
        );
    });
}

#[test]
fn ladder_contains_and_trace_model() {
    let mut h = synthetic();
    run(&mut h, |eng| {
        let p = player(eng, Vec3::new(0.0, 0.0, 50.0));
        let at = Vec3::new(200.0, 0.0, 32.0);
        brush(eng, MODEL_LADDER, SOLID_NOT, CONTENTS_LADDER as i16, at);
        let pe = eng.pm_physents(p);
        assert_eq!((pe.num_physents(), pe.num_moveents()), (1, 1));

        // The ladder box is 16 wide; the standing hull grows it by 16 each way.
        let near_ladder =
            |dx: f32, hull: i32| eng.pm_ladder_contains(&pe, 0, at + Vec3::new(dx, 0.0, 0.0), hull);
        assert!(near_ladder(20.0, 0));
        assert!(!near_ladder(30.0, 0));
        assert!(near_ladder(-23.0, 0));
        // The ducking hull grows it by 16 too, but is shorter; the point hull not at all.
        assert!(near_ladder(20.0, 1));
        assert!(!near_ladder(10.0, 2));
        assert!(near_ladder(4.0, 2));
        // Above the ladder: the standing hull's 36 units of reach, not more.
        assert!(eng.pm_ladder_contains(&pe, 0, at + Vec3::new(0.0, 0.0, 66.0), 0));
        assert!(!eng.pm_ladder_contains(&pe, 0, at + Vec3::new(0.0, 0.0, 70.0), 0));
        // No such moveent, no such hull.
        assert!(!eng.pm_ladder_contains(&pe, 1, at, 0));
        assert!(!eng.pm_ladder_contains(&pe, usize::MAX, at, 0));
        assert!(!eng.pm_ladder_contains(&pe, 0, at, 7));

        // `PM_TraceModel`: from in front of the ladder to its centre, the point hull is hit
        // at the face (8 units from the centre) and the normal points back at the player.
        let (fraction, normal) = eng.pm_trace_model(&pe, 0, at + Vec3::new(40.0, 0.0, 0.0), at);
        assert!(near(fraction, 31.96875 / 40.0), "{fraction}");
        assert_eq!(normal, Vec3::X);
        // Missing the ladder gives fraction 1; so does a trace that never leaves solid.
        let miss = eng.pm_trace_model(
            &pe,
            0,
            at + Vec3::new(40.0, 0.0, 0.0),
            at + Vec3::new(20.0, 0.0, 0.0),
        );
        assert_eq!(miss, (1.0, Vec3::ZERO));
        let inside = eng.pm_trace_model(&pe, 0, at, at + Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(inside, (1.0, Vec3::ZERO));
        assert_eq!(eng.pm_trace_model(&pe, 1, at, at), (1.0, Vec3::ZERO));
    });
}

#[test]
fn pm_services_refuse_bad_input() {
    let mut h = synthetic();
    run(&mut h, |eng| {
        let p = player(eng, Vec3::new(0.0, 0.0, 50.0));
        let ghost = eng.create_entity("gone").expect("free edict");
        eng.remove_entity(ghost);
        // A stale edict has no physents at all.
        let none = eng.pm_physents(ghost);
        assert_eq!((none.num_physents(), none.num_moveents()), (0, 0));
        assert!(none.physent(0).is_none());
        // An origin outside the world has the world only.
        let lost = eng.client_edict(1);
        eng.ent_mut(lost).unwrap().origin = Vec3::splat(f32::NAN);
        assert_eq!(eng.pm_physents(lost).num_physents(), 1);

        let pe = eng.pm_physents(p);
        // Bad indices.
        assert_eq!(pe.num_physents(), 1);
        for i in [-1, -1000, 1, 2, i32::MAX, i32::MIN] {
            assert!(pe.physent(i).is_none(), "{i}");
            assert!(pe.edict_of(i).is_none(), "{i}");
        }
        assert!(pe.moveent(0).is_none() && pe.moveent(usize::MAX).is_none());

        // Bad numbers: nothing hit, nothing panics.
        let bad = [
            Vec3::splat(f32::NAN),
            Vec3::new(f32::INFINITY, 0.0, 0.0),
            Vec3::new(0.0, 1e30, 0.0),
        ];
        for b in bad {
            let t = eng.pm_player_trace(&pe, b, Vec3::ZERO, 0, PM_NORMAL, -1);
            assert_eq!((t.fraction, t.ent), (1.0, -1));
            assert!(t.endpos.is_finite());
            let t = eng.pm_player_trace(&pe, Vec3::ZERO, b, 0, PM_NORMAL, -1);
            assert_eq!((t.fraction, t.ent), (1.0, -1));
            assert!(t.endpos.is_finite());
            assert_eq!(eng.pm_test_player_position(&pe, b, Vec3::ZERO, 0).0, -1);
            assert_eq!(eng.pm_test_player_position(&pe, Vec3::ZERO, b, 0).0, -1);
            assert_eq!(eng.pm_point_contents(&pe, b), (-1, -1));
            assert_eq!(eng.pm_trace_model(&pe, 0, b, Vec3::ZERO), (1.0, Vec3::ZERO));
            assert!(!eng.pm_ladder_contains(&pe, 0, b, 0));
        }
        // An entity with hostile numbers (set behind the linking's back) is never hit.
        let boom = boxed(eng, SOLID_BBOX, Vec3::new(100.0, 0.0, 50.0), 16.0);
        let pe = eng.pm_physents(p);
        eng.ent_mut(boom).unwrap().mins = Vec3::splat(f32::NAN);
        let pe_bad = eng.pm_physents(p);
        for pe in [&pe, &pe_bad] {
            let t = eng.pm_player_trace(
                pe,
                Vec3::new(0.0, 0.0, 50.0),
                Vec3::new(200.0, 0.0, 50.0),
                2,
                PM_NORMAL,
                -1,
            );
            assert!(t.fraction.is_finite() && t.endpos.is_finite());
            let _ = eng.pm_test_player_position(pe, Vec3::ZERO, Vec3::new(100.0, 0.0, 50.0), 0);
        }
        // The default (empty) list traces nothing and has no contents.
        let empty = PmPhysEnts::default();
        let t = eng.pm_player_trace(&empty, Vec3::ZERO, Vec3::X, 0, PM_NORMAL, -1);
        assert_eq!((t.fraction, t.ent), (1.0, -1));
        assert_eq!(eng.pm_point_contents(&empty, Vec3::ZERO), (-1, -1));
    });
}

/// On a real map the traces of the world physent agree with the engine's own world traces
/// wherever those start in the open (the two walks differ only when a trace starts in solid
/// or backs up out of it).
#[test]
fn world_player_trace_matches_hull_trace_on_dust2() {
    let Some(mut h) = dust2_harness() else {
        return;
    };
    let (compared, mismatched) = run(&mut h, |eng| {
        let p = eng.client_edict(0);
        let pe = eng.pm_physents(p);
        let (mins, maxs) = (
            Vec3::new(-1500.0, -1500.0, -400.0),
            Vec3::new(1500.0, 3000.0, 400.0),
        );
        let mut state: u64 = 0xC516;
        let mut unit = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 40) as f32 / 16_777_216.0
        };
        // (usehull of the player tables, the engine hull it maps to)
        let hulls = [
            (0, Hull::Standing),
            (1, Hull::Crouched),
            (2, Hull::Point),
            (3, Hull::Large),
        ];
        let (mut compared, mut mismatched, mut hits) = (0, 0, 0);
        for i in 0..20_000 {
            let start = mins + (maxs - mins) * Vec3::new(unit(), unit(), unit());
            let end = start + (Vec3::new(unit(), unit(), unit()) * 2.0 - Vec3::ONE) * 600.0;
            let (usehull, hull) = hulls[i % 4];
            let engine = eng.trace_hull(start, end, hull);
            if engine.start_solid || engine.all_solid {
                continue;
            }
            let pm = eng.pm_player_trace(&pe, start, end, usehull, PM_WORLD_ONLY, -1);
            compared += 1;
            hits += usize::from(pm.fraction < 1.0);
            assert!(!pm.startsolid && !pm.allsolid, "{i}");
            assert_eq!(pm.ent, if pm.fraction < 1.0 { 0 } else { -1 }, "{i}");
            if (pm.fraction - engine.fraction).abs() > 0.002
                || (pm.fraction < 1.0 && pm.plane_normal.dot(engine.plane_normal) < 0.999)
            {
                mismatched += 1;
            }
        }
        assert!(hits > 50, "the traces hit the map: {hits}");
        (compared, mismatched)
    });
    assert!(compared > 200, "traces were compared: {compared}");
    assert_eq!(mismatched, 0, "of {compared} traces");
}
