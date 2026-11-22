//! The engine side of the player movement of the original: what `SV_RunCmd` does around the
//! game's `run_usercmd` (commands of 0 ms, the conveyor under the feet), `PM_SV_PlaySound`
//! (a sound the mover does not hear), and `TraceTexture`.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use common::{Harness, TestGame};
use cs_assets::bsp::{
    BspMap, BspModel, Face, Leaf, LightmapAtlas, Node, Plane, TexInfo,
};
use cs_assets::wad::DecodedTexture;
use cs_engine::consts::{FL_BASEVELOCITY, FL_CONVEYOR, FL_ONGROUND};
use cs_engine::{EdictId, EngineApi, ServerConfig, ServerMap};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::{SoundMsg, SvcMessage};
use cs_transport::LoopbackEndpoint;
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

type Probe = ProbeClient<LoopbackEndpoint>;

fn joined(h: &mut Harness, name: &str) -> Probe {
    let mut probe = h.add_probe(name);
    h.step(&mut [&mut probe], 200);
    assert_eq!(probe.state(), &ProbeState::InGame);
    probe
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

fn sounds(p: &Probe) -> Vec<SoundMsg> {
    p.messages()
        .iter()
        .filter_map(|m| match m {
            SvcMessage::Sound(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

// --- SV_RunCmd -------------------------------------------------------------------------

#[test]
fn zero_msec_commands_reach_the_game() {
    let mut h = Harness::new(2);
    let mut probe = joined(&mut h, "Player");
    let before = h.server.game().run_usercmd_calls;
    // The probe has nothing queued, so every packet it sends carries the last command again
    // with `msec` 0, as the original client does when it has no movement to send.
    h.step(&mut [&mut probe], 30);

    let game = h.server.game();
    let calls = game.run_usercmd_calls - before;
    assert!(calls > 0, "no command reached the game");
    assert!(game.usercmd_msecs[game.usercmd_msecs.len() - calls..]
        .iter()
        .all(|&ms| ms == 0));
    assert!(game.usercmd_frametimes[game.usercmd_frametimes.len() - calls..]
        .iter()
        .all(|&ft| ft == 0.0));
}

#[test]
fn a_zero_msec_command_takes_nothing_from_the_budget() {
    let mut h = Harness::new(2);
    let mut probe = joined(&mut h, "Player");
    h.step(&mut [&mut probe], 30);
    let start = h.server.edicts().get(h.server.edicts().client(0)).unwrap().vars.origin.x;
    // The idle zero-ms commands left the bank alone: 100 ms in one go all run.
    let before = h.server.game().usercmd_msecs.len();
    probe.queue_cmd(GoldSrcUserCmd {
        msec: 100,
        forwardmove: 250.0,
        ..Default::default()
    });
    h.step(&mut [&mut probe], 1);
    let ran: u32 = h.server.game().usercmd_msecs[before..]
        .iter()
        .map(|&ms| u32::from(ms))
        .sum();
    assert_eq!(ran, 100);
    let moved = h.server.edicts().get(h.server.edicts().client(0)).unwrap().vars.origin.x - start;
    assert!((moved - 25.0).abs() <= 0.01, "moved {moved}");
}

/// What the game saw of the player: base velocity, flags, velocity.
type Seen = Rc<RefCell<Option<(Vec3, u32, Vec3)>>>;

fn with_conveyor() -> (Harness, Probe, Seen) {
    let mut h = Harness::new(2);
    let mut probe = joined(&mut h, "Player");
    run(&mut h, |eng| {
        let belt = eng.create_entity("test_conveyor").expect("free edict");
        let v = eng.ent_mut(belt).unwrap();
        v.flags |= FL_CONVEYOR;
        v.speed = 100.0;
        v.movedir = Vec3::new(0.0, 1.0, 0.0);
        let me = eng.client_edict(0);
        let v = eng.ent_mut(me).unwrap();
        v.flags |= FL_ONGROUND;
        v.groundentity = Some(belt);
    });
    let seen = Rc::new(RefCell::new(None));
    let sink = Rc::clone(&seen);
    h.server.game_mut().usercmd_hook.set(move |eng, slot| {
        let v = eng.ent(eng.client_edict(slot)).unwrap();
        *sink.borrow_mut() = Some((v.basevelocity, v.flags, v.velocity));
    });
    probe.queue_cmd(GoldSrcUserCmd {
        msec: 10,
        ..Default::default()
    });
    h.step(&mut [&mut probe], 1);
    (h, probe, seen)
}

#[test]
fn a_conveyor_moves_a_standing_player() {
    let (_h, _probe, seen) = with_conveyor();
    let (basevelocity, flags, _) = seen.borrow().expect("the game ran");
    // `SV_CheckMovingGround` ran before the game: the belt's speed is the base velocity, and
    // `FL_BASEVELOCITY` is cleared again at its end.
    assert_eq!(basevelocity, Vec3::new(0.0, 100.0, 0.0));
    assert_eq!(flags & FL_BASEVELOCITY, 0);
    assert_ne!(flags & FL_ONGROUND, 0);
}

#[test]
fn a_base_velocity_nothing_renewed_is_folded_into_the_velocity() {
    let mut h = Harness::new(2);
    let mut probe = joined(&mut h, "Player");
    run(&mut h, |eng| {
        let me = eng.client_edict(0);
        let v = eng.ent_mut(me).unwrap();
        v.basevelocity = Vec3::new(10.0, 0.0, 0.0);
        v.velocity = Vec3::new(0.0, 5.0, 0.0);
    });
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    h.server.game_mut().usercmd_hook.set(move |eng, slot| {
        let v = eng.ent(eng.client_edict(slot)).unwrap();
        sink.borrow_mut().push((v.basevelocity, v.velocity));
    });
    probe.queue_cmd(GoldSrcUserCmd {
        msec: 10,
        ..Default::default()
    });
    h.step(&mut [&mut probe], 1);
    let first = seen.borrow()[0];
    // `velocity += basevelocity * (frametime * 0.5 + 1)`, the base velocity cleared.
    let scale = 0.01f32 * 0.5 + 1.0;
    assert_eq!(first.0, Vec3::ZERO);
    assert!((first.1 - Vec3::new(10.0 * scale, 5.0, 0.0)).abs().max_element() < 1e-4, "{first:?}");
}

// --- PM_SV_PlaySound ---------------------------------------------------------------------

const STEP: &str = "player/pl_step1.wav";

fn two_players() -> (Harness, Probe, Probe) {
    let game = TestGame {
        precache_sounds: vec![STEP.to_string()],
        spawn_origins: vec![Vec3::new(100.0, -50.0, 24.0), Vec3::new(64.0, 0.0, 0.0)],
        ..TestGame::default()
    };
    let mut h = Harness::with_game(2, game);
    let a = joined(&mut h, "A");
    let b = joined(&mut h, "B");
    (h, a, b)
}

#[test]
fn pm_sound_skips_the_mover() {
    let (mut h, mut a, mut b) = two_players();
    let results = run(&mut h, |eng| {
        let me = eng.client_edict(0);
        // 0.5 * 255 is 127.5: the byte is cut, not rounded.
        let heard = eng.pm_play_sound(me, 4, STEP, 0.5, 1.0, 0, 100);
        // Out of range arguments are held to the wire, as `SV_BuildSoundMsg` does.
        let clamped = eng.pm_play_sound(me, 99, STEP, 9.0, 1.0, 0, -5);
        // Not precached, and a non-finite volume: refused.
        let refused = (
            eng.pm_play_sound(me, 1, "player/nope.wav", 1.0, 1.0, 0, 100),
            eng.pm_play_sound(me, 1, STEP, f32::NAN, 1.0, 0, 100),
        );
        (heard, clamped, refused)
    });
    assert_eq!(results, (true, true, (false, false)));
    h.step(&mut [&mut a, &mut b], 30);

    assert!(sounds(&a).is_empty(), "the mover's own client got {:?}", sounds(&a));
    let heard = sounds(&b);
    assert_eq!(heard.len(), 2, "{heard:?}");
    assert_eq!((heard[0].entity, heard[0].channel, heard[0].volume), (1, 4, 127));
    assert_eq!(heard[0].pitch, 100);
    assert!((heard[0].origin - Vec3::new(100.0, -50.0, 24.0)).abs().max_element() <= 0.125);
    // Channel 99 is 4, volume 9.0 is 255, pitch -5 is 0.
    assert_eq!((heard[1].channel, heard[1].volume, heard[1].pitch), (4, 255, 0));
}

#[test]
fn pm_sound_on_the_static_channel_skips_the_mover_too() {
    let (mut h, mut a, mut b) = two_players();
    run(&mut h, |eng| {
        let me = eng.client_edict(1);
        assert!(eng.pm_play_sound(me, 6, STEP, 1.0, 1.0, 0, 100));
    });
    h.step(&mut [&mut a, &mut b], 30);
    assert_eq!(sounds(&a).len(), 1);
    assert!(sounds(&b).is_empty());
}

/// One client on the server: a static-channel sound is broadcast instead of sent reliably to
/// everyone, and `MSG_FL_ONE` still leaves the mover out.
#[test]
fn pm_sound_on_a_single_client_server_skips_the_mover() {
    let game = TestGame {
        precache_sounds: vec![STEP.to_string()],
        ..TestGame::default()
    };
    let mut h = Harness::with_game(1, game);
    let mut a = joined(&mut h, "A");
    run(&mut h, |eng| {
        let me = eng.client_edict(0);
        assert!(eng.pm_play_sound(me, 6, STEP, 1.0, 1.0, 0, 100));
    });
    h.step(&mut [&mut a], 30);
    assert!(sounds(&a).is_empty(), "{:?}", sounds(&a));
    // The same call from an emitter that is not the mover reaches it.
    run(&mut h, |eng| {
        let me = eng.client_edict(0);
        assert!(eng.emit_sound(me, 6, STEP, 1.0, 1.0, 100));
    });
    h.step(&mut [&mut a], 30);
    assert_eq!(sounds(&a).len(), 1);
}

#[test]
fn an_ordinary_sound_still_reaches_the_emitter() {
    let (mut h, mut a, mut b) = two_players();
    run(&mut h, |eng| {
        let me = eng.client_edict(0);
        assert!(eng.emit_sound(me, 4, STEP, 0.5, 1.0, 100));
    });
    h.step(&mut [&mut a, &mut b], 30);
    // `emit_sound` rounds the volume: 127.5 is 128.
    assert_eq!(sounds(&a).iter().map(|s| s.volume).collect::<Vec<_>>(), vec![128]);
    assert_eq!(sounds(&b).iter().map(|s| s.volume).collect::<Vec<_>>(), vec![128]);
}

#[test]
fn a_sound_from_something_that_is_not_a_player_skips_nobody() {
    let (mut h, mut a, mut b) = two_players();
    run(&mut h, |eng| {
        let thing = eng.create_entity("test_thing").expect("free edict");
        eng.set_origin(thing, Vec3::new(80.0, 0.0, 0.0));
        assert!(eng.pm_play_sound(thing, 4, STEP, 1.0, 1.0, 0, 100));
    });
    h.step(&mut [&mut a, &mut b], 30);
    assert_eq!((sounds(&a).len(), sounds(&b).len()), (1, 1));
}

// --- TraceTexture ---------------------------------------------------------------------------

/// A map with a floor (the plane z = 0, a 64 x 64 face named "floor") in the world model and
/// a wall (the plane x = 0, a 64 x 64 face named "wall" in y and z) in the brush model `*1`.
fn two_face_map() -> ServerMap {
    let leaf = |contents: i32| Leaf {
        contents,
        vis_offset: -1,
        mins: [0; 3],
        maxs: [0; 3],
        first_mark_surface: 0,
        num_mark_surfaces: 0,
        ambient_levels: [0; 4],
    };
    let plane = |normal: Vec3| Plane {
        normal,
        dist: 0.0,
        plane_type: 0,
    };
    let node = |plane: u32, first_face: u16| Node {
        plane,
        children: [-2, -2],
        mins: [0; 3],
        maxs: [0; 3],
        first_face,
        num_faces: 1,
    };
    let texinfo = |s_vec: Vec3, t_vec: Vec3, miptex: i32| TexInfo {
        s_vec,
        s_shift: 0.0,
        t_vec,
        t_shift: 0.0,
        miptex,
        flags: 0,
    };
    let face = |texinfo: u16, first_edge: i32| Face {
        plane: 0,
        plane_side: 0,
        first_edge,
        num_edges: 4,
        texinfo,
        styles: [0; 4],
        lightmap_offset: -1,
    };
    let model = |head: i32| BspModel {
        mins: Vec3::splat(-100.0),
        maxs: Vec3::splat(100.0),
        origin: Vec3::ZERO,
        head_nodes: [head, -1, -1, -1],
        vis_leafs: 0,
        first_face: 0,
        num_faces: 0,
    };
    let texture = |name: &str| DecodedTexture {
        name: name.to_string(),
        width: 16,
        height: 16,
        rgba: Vec::new(),
        alpha_test: false,
    };
    let bsp = BspMap {
        name: "two_faces".into(),
        skyname: String::new(),
        entities: Vec::new(),
        spawns: Vec::new(),
        hostage_spawns: Vec::new(),
        brush_entities: Vec::new(),
        planes: vec![plane(Vec3::Z), plane(Vec3::X)],
        vertices_raw: vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(64.0, 0.0, 0.0),
            Vec3::new(64.0, 64.0, 0.0),
            Vec3::new(0.0, 64.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 64.0, 0.0),
            Vec3::new(0.0, 64.0, 64.0),
            Vec3::new(0.0, 0.0, 64.0),
        ],
        visibility: Vec::new(),
        nodes: vec![node(0, 0), node(1, 1)],
        // The floor's texture space is (x, y), the wall's is (y, z).
        texinfos: vec![
            texinfo(Vec3::X, Vec3::Y, 0),
            texinfo(Vec3::Y, Vec3::Z, 1),
        ],
        faces: vec![face(0, 0), face(1, 4)],
        lighting: Vec::new(),
        clipnodes: Vec::new(),
        leafs: vec![leaf(-2), leaf(-1)],
        mark_surfaces: Vec::new(),
        edges: vec![
            [0, 1],
            [1, 2],
            [2, 3],
            [3, 0],
            [4, 5],
            [5, 6],
            [6, 7],
            [7, 4],
        ],
        surf_edges: (0..8).collect(),
        models: vec![model(0), model(1)],
        textures: vec![texture("floor"), texture("wall")],
        lightmap_atlas: LightmapAtlas {
            width: 0,
            height: 0,
            rgba: Vec::new(),
            packed_faces: 0,
        },
        render_vertices: Vec::new(),
        render_indices: Vec::new(),
        batches: Vec::new(),
    };
    // A header the checksum accepts: version 30 and empty lumps.
    let mut raw = vec![0u8; 4 + 15 * 8];
    raw[0] = 30;
    ServerMap::from_bsp("two_faces", &raw, Arc::new(bsp)).expect("valid BSP header")
}

fn two_face_harness() -> Harness {
    let cfg = ServerConfig {
        max_clients: 1,
        ..ServerConfig::default()
    };
    Harness::with_map(cfg, two_face_map(), TestGame::default())
}

fn world_of(h: &Harness) -> EdictId {
    h.server.edicts().world()
}

#[test]
fn trace_texture_names_the_face_the_line_crosses() {
    let mut h = two_face_harness();
    let world = world_of(&h);
    let (down, beside) = run(&mut h, move |eng| {
        (
            eng.trace_texture(world, Vec3::new(32.0, 32.0, 10.0), Vec3::new(32.0, 32.0, -10.0)),
            // Past the 64 x 64 face of the floor.
            eng.trace_texture(world, Vec3::new(100.0, 32.0, 10.0), Vec3::new(100.0, 32.0, -10.0)),
        )
    });
    assert_eq!(down.as_deref(), Some("floor"));
    assert_eq!(beside, None);
}

#[test]
fn trace_texture_follows_the_origin_and_angles_of_a_brush_entity() {
    let mut h = two_face_harness();
    let (moved, turned_by_yaw, same_line_unturned) = run(&mut h, |eng| {
        let wall = eng.create_entity("func_wall").expect("free edict");
        assert!(eng.set_model(wall, "*1"));
        eng.set_origin(wall, Vec3::new(1000.0, 0.0, 0.0));
        let moved = eng.trace_texture(
            wall,
            Vec3::new(1010.0, 32.0, 32.0),
            Vec3::new(990.0, 32.0, 32.0),
        );
        // At the origin the world line along x misses the wall's box.
        eng.set_origin(wall, Vec3::ZERO);
        let same_line_unturned = eng.trace_texture(
            wall,
            Vec3::new(-32.0, 10.0, 32.0),
            Vec3::new(-32.0, -10.0, 32.0),
        );
        // Turned 90 degrees in yaw, model x is world y and model y is world -x.
        eng.ent_mut(wall).unwrap().angles = Vec3::new(0.0, 90.0, 0.0);
        let turned = eng.trace_texture(
            wall,
            Vec3::new(-32.0, 10.0, 32.0),
            Vec3::new(-32.0, -10.0, 32.0),
        );
        (moved, turned, same_line_unturned)
    });
    assert_eq!(moved.as_deref(), Some("wall"));
    assert_eq!(same_line_unturned, None);
    assert_eq!(turned_by_yaw.as_deref(), Some("wall"));
}

#[test]
fn trace_texture_refuses_what_has_no_brush_model_or_no_finite_line() {
    let mut h = two_face_harness();
    let world = world_of(&h);
    let results = run(&mut h, move |eng| {
        let boxed = eng.create_entity("test_box").expect("free edict");
        eng.set_size(boxed, Vec3::splat(-8.0), Vec3::splat(8.0));
        let gone = eng.create_entity("test_gone").expect("free edict");
        eng.remove_entity(gone);
        let down = (Vec3::new(32.0, 32.0, 10.0), Vec3::new(32.0, 32.0, -10.0));
        [
            // An entity without a brush model.
            eng.trace_texture(boxed, down.0, down.1),
            // A stale entity.
            eng.trace_texture(gone, down.0, down.1),
            eng.trace_texture(world, Vec3::NAN, down.1),
            eng.trace_texture(world, down.0, Vec3::new(0.0, f32::INFINITY, 0.0)),
        ]
    });
    assert_eq!(results, [None, None, None, None]);
}

#[test]
fn trace_texture_on_the_stub_map_finds_nothing() {
    let mut h = Harness::new(1);
    let world = world_of(&h);
    let found = run(&mut h, move |eng| {
        eng.trace_texture(world, Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.0, 0.0, -10.0))
    });
    assert_eq!(found, None);
}

/// de_dust2, when the game is installed: a line straight down through a spawn point hits a
/// face, and the name is one of the map's textures, as the map stores it.
#[test]
fn trace_texture_names_the_floor() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("de_dust2 reads");
    let bsp = Arc::new(BspMap::load(&vfs, "de_dust2").expect("de_dust2 parses"));
    let Some(spawn) = bsp.spawns.first().map(|s| s.position) else {
        return;
    };
    let names: Vec<String> = bsp.textures.iter().map(|t| t.name.clone()).collect();
    let map = ServerMap::from_bsp("de_dust2", &raw, bsp).expect("valid BSP header");
    let cfg = ServerConfig {
        max_clients: 2,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_map(cfg, map, TestGame::default());
    let world = world_of(&h);
    let found = run(&mut h, move |eng| {
        eng.trace_texture(world, spawn + Vec3::Z * 8.0, spawn - Vec3::Z * 128.0)
    });
    let name = found.expect("the floor under a spawn has a texture");
    assert!(!name.is_empty() && name.len() <= 15, "{name:?}");
    assert!(names.contains(&name), "{name:?} is not a texture of the map");
}
