//! The server frame order (packets, physics, time) and the physics pass: simulation time, the
//! think of every entity, and the `NONE`, `NOCLIP` and `FOLLOW` movetypes.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{FRAME, Harness, TestGame};
use cs_engine::consts::{
    FL_KILLME, MOVETYPE_FOLLOW, MOVETYPE_NOCLIP, MOVETYPE_NONE, MOVETYPE_TOSS, SOLID_BBOX,
};
use cs_engine::{EdictId, EngineApi, SV_TIME_AT_MAP_START, ServerConfig, ServerMap};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_transport::LoopbackEndpoint;
use glam::Vec3;

type Probe = ProbeClient<LoopbackEndpoint>;

const EPS: f64 = 1e-9;

fn harness(max_clients: u8, game: TestGame) -> Harness {
    let cfg = ServerConfig {
        max_clients,
        max_edicts: 64,
        ..ServerConfig::default()
    };
    Harness::with_config(cfg, game)
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

fn joined(h: &mut Harness) -> Probe {
    let mut probe = h.add_probe("Player");
    h.step(&mut [&mut probe], 200);
    assert_eq!(probe.state(), &ProbeState::InGame);
    probe
}

fn vars(h: &Harness, id: EdictId) -> cs_engine::EntVars {
    h.server.edicts().get(id).expect("live").vars.clone()
}

/// A box entity at `origin` that is in the area tree.
fn spawn_box(eng: &mut EngineApi<'_>, origin: Vec3) -> EdictId {
    let id = eng.create_entity("test_box").expect("free edict");
    eng.ent_mut(id).unwrap().solid = SOLID_BBOX;
    eng.set_size(id, Vec3::splat(-8.0), Vec3::splat(8.0));
    eng.set_origin(id, origin);
    id
}

#[test]
fn frame_order_is_packets_then_physics_then_time() {
    let game = TestGame {
        extra_entities: 1,
        extra_nextthink: 0.01,
        think_reschedules: true,
        ..TestGame::default()
    };
    let mut h = harness(1, game);
    let mut probe = joined(&mut h);
    let x_before = vars(&h, h.server.edicts().client(0)).origin.x;

    // 10 ms of forward movement: 2.5 units.
    probe.queue_cmd(GoldSrcUserCmd {
        msec: 10,
        forwardmove: 250.0,
        ..Default::default()
    });
    let cmds_before = h.server.game().usercmd_times.len();
    h.step(&mut [&mut probe], 1);
    let game = h.server.game();
    assert_eq!(game.usercmd_times.len(), cmds_before + 1);

    // The command ran before this frame's physics: the think of this frame saw its result.
    let seen = *game.think_player_x.last().unwrap();
    assert!((seen - (x_before + 2.5)).abs() < 1e-3, "{seen} vs {x_before}");

    // It ran at the simulation time, which the frame advances only after the physics: the
    // frame's `start_frame` had the same time.
    let at_cmd = *game.usercmd_times.last().unwrap();
    let at_start = *game.start_frame_times.last().unwrap();
    assert_eq!(at_cmd, at_start);
    h.step(&mut [&mut probe], 1);
    let next_start = *h.server.game().start_frame_times.last().unwrap();
    assert!((next_start - at_start - FRAME).abs() < EPS, "{next_start}");
    // Not the host's clock.
    assert!((at_start - h.now).abs() > 0.5);
}

#[test]
fn think_sees_its_own_time() {
    // Entity 2 is due 4 ms into the first frame (10 ms long); entity 3 was due long ago.
    let game = TestGame {
        extra_entities: 2,
        extra_nextthink_overrides: vec![SV_TIME_AT_MAP_START + 0.004, 0.5],
        ..TestGame::default()
    };
    let mut h = harness(1, game);
    h.step(&mut [], 1);
    let log = &h.server.game().think_log;
    assert_eq!(log.len(), 2, "{log:?}");
    // Due within the frame: called in it, with its own time.
    assert_eq!(log[0].0, 2);
    assert!((log[0].1 - (SV_TIME_AT_MAP_START + 0.004)).abs() < EPS);
    // Due in the past: runs at the server time.
    assert_eq!(log[1].0, 3);
    assert!((log[1].1 - SV_TIME_AT_MAP_START).abs() < EPS);
    // `start_frame` saw the server time of the frame.
    assert_eq!(h.server.game().start_frame_times, vec![SV_TIME_AT_MAP_START]);
}

#[test]
fn think_due_after_the_frame_waits() {
    // 11 ms into a 10 ms frame: not due in the first frame, due in the second.
    let game = TestGame {
        extra_entities: 1,
        extra_nextthink: SV_TIME_AT_MAP_START + 0.011,
        ..TestGame::default()
    };
    let mut h = harness(1, game);
    h.step(&mut [], 1);
    assert!(h.server.game().think_log.is_empty());
    h.step(&mut [], 1);
    let log = &h.server.game().think_log;
    assert_eq!(log.len(), 1);
    assert!((log[0].1 - (SV_TIME_AT_MAP_START + 0.011)).abs() < EPS);
}

#[test]
fn long_frames_are_clamped_to_a_quarter_second() {
    let mut h = harness(1, TestGame::default());
    h.step(&mut [], 1);
    // A 10 s stall: the simulation takes one 0.25 s step.
    h.now += 10.0;
    h.server.frame(&mut h.hub, h.now);
    h.step(&mut [], 1);
    let times = &h.server.game().start_frame_times;
    assert_eq!(times.len(), 3);
    assert!((times[2] - times[1] - 0.25).abs() < EPS, "{times:?}");
}

#[test]
fn sv_time_restarts_at_each_map() {
    let mut h = harness(1, TestGame::default());
    h.step(&mut [], 30);
    let times = &h.server.game().start_frame_times;
    assert!(times.last().unwrap() > &1.2);
    h.server.change_map(ServerMap::stub("second"));
    assert_eq!(
        h.server.game().activate_times,
        vec![SV_TIME_AT_MAP_START, SV_TIME_AT_MAP_START]
    );
    h.step(&mut [], 1);
    assert_eq!(*h.server.game().start_frame_times.last().unwrap(), 1.0);
    h.step(&mut [], 1);
    let times = &h.server.game().start_frame_times;
    assert!((times[times.len() - 1] - 1.0 - FRAME).abs() < EPS);
}

#[test]
fn killme_frees_after_physics() {
    let game = TestGame {
        extra_entities: 2,
        extra_nextthink: 0.01,
        ..TestGame::default()
    };
    let mut h = harness(1, game);
    // Entity 2 is marked at the start of the frame, before the pass reaches it.
    h.server.game_mut().act(|eng| {
        let victim = eng.find_by_classname(None, "test_extra").unwrap();
        eng.ent_mut(victim).unwrap().flags |= FL_KILLME;
    });
    h.step(&mut [], 1);
    assert!(h.server.edicts().by_index(2).is_none());
    // Its think did not run; the other entity's did.
    let log = &h.server.game().think_log;
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0].0, 3);
    assert!(h.server.edicts().by_index(3).is_some());
}

#[test]
fn follow_tracks_aiment() {
    let mut h = harness(1, TestGame::default());
    let (parent, follower) = run(&mut h, |eng| {
        let parent = spawn_box(eng, Vec3::new(100.0, 20.0, 30.0));
        eng.ent_mut(parent).unwrap().angles = Vec3::new(0.0, 90.0, 0.0);
        let follower = eng.create_entity("test_follower").expect("free edict");
        let v = eng.ent_mut(follower).unwrap();
        v.movetype = MOVETYPE_FOLLOW;
        v.aiment = Some(parent);
        v.v_angle = Vec3::new(1.0, 2.0, 3.0);
        (parent, follower)
    });
    h.step(&mut [], 1);
    let f = vars(&h, follower);
    assert_eq!(f.origin, Vec3::new(101.0, 22.0, 33.0));
    assert_eq!(f.angles, Vec3::new(0.0, 90.0, 0.0));

    // It follows when the parent moves.
    run(&mut h, move |eng| eng.set_origin(parent, Vec3::new(-50.0, 0.0, 0.0)));
    h.step(&mut [], 1);
    assert_eq!(vars(&h, follower).origin, Vec3::new(-49.0, 2.0, 3.0));

    // Without a parent it stops following and becomes MOVETYPE_NONE.
    run(&mut h, move |eng| eng.remove_entity(parent));
    h.step(&mut [], 1);
    let f = vars(&h, follower);
    assert_eq!(f.movetype, MOVETYPE_NONE);
    assert_eq!(f.origin, Vec3::new(-49.0, 2.0, 3.0));
}

#[test]
fn follow_is_relinked_where_it_moved() {
    let mut h = harness(1, TestGame::default());
    let follower = run(&mut h, |eng| {
        let parent = spawn_box(eng, Vec3::new(1000.0, 0.0, 0.0));
        let follower = spawn_box(eng, Vec3::ZERO);
        let v = eng.ent_mut(follower).unwrap();
        v.movetype = MOVETYPE_FOLLOW;
        v.aiment = Some(parent);
        follower
    });
    h.step(&mut [], 1);
    let origin = vars(&h, follower).origin;
    assert_eq!(origin, Vec3::new(1000.0, 0.0, 0.0));
    let found = run(&mut h, move |eng| {
        eng.entities_in_box(origin - Vec3::ONE, origin + Vec3::ONE)
            .contains(&follower)
    });
    assert!(found);
}

#[test]
fn noclip_moves_by_velocity_without_collision() {
    let mut h = harness(1, TestGame::default());
    let id = run(&mut h, |eng| {
        // A solid box in the way at x = 50.
        spawn_box(eng, Vec3::new(50.0, 0.0, 0.0));
        let id = spawn_box(eng, Vec3::ZERO);
        let v = eng.ent_mut(id).unwrap();
        v.movetype = MOVETYPE_NOCLIP;
        v.velocity = Vec3::new(1000.0, 0.0, -100.0);
        v.avelocity = Vec3::new(0.0, 900.0, 0.0);
        id
    });
    // The spawning frame already ran the pass once, after the game's call: 10 ms.
    h.step(&mut [], 9);
    let v = vars(&h, id);
    assert!((v.origin.x - 100.0).abs() < 1e-2, "{:?}", v.origin);
    assert!((v.origin.z + 10.0).abs() < 1e-3, "{:?}", v.origin);
    assert!((v.angles.y - 90.0).abs() < 1e-2, "{:?}", v.angles);
    // Past the blocker, which it did not touch.
    assert!(v.origin.x > 50.0 + 8.0 + 8.0);
}

#[test]
fn noclip_entity_stays_visible_to_a_probe_while_it_crosses_the_world() {
    let mut h = harness(1, TestGame::default());
    let id = run(&mut h, |eng| {
        let id = spawn_box(eng, Vec3::new(-3000.0, 0.0, 0.0));
        let v = eng.ent_mut(id).unwrap();
        v.movetype = MOVETYPE_NOCLIP;
        // 20 units a frame: 300 frames cross the stub world's area tree.
        v.velocity = Vec3::new(2000.0, 0.0, 0.0);
        id
    });
    let mut last = vars(&h, id).origin.x;
    for frame in 0..300 {
        let at = vars(&h, id).origin;
        // The probe box is around where the last relink put the entity.
        let found = run(&mut h, move |eng| {
            eng.entities_in_box(at - Vec3::ONE, at + Vec3::ONE)
                .contains(&id)
        });
        assert!(found, "frame {frame}, origin {at:?}");
        let now = vars(&h, id).origin.x;
        assert!(now > last, "it moved");
        last = now;
    }
    assert!(last > 2900.0, "{last}");
}

#[test]
fn players_are_skipped_by_the_entity_pass() {
    let mut h = harness(1, TestGame::default());
    let _probe = joined(&mut h);
    let player = h.server.edicts().client(0);
    let before = vars(&h, player).origin;
    run(&mut h, move |eng| {
        let t = eng.time();
        let v = eng.ent_mut(player).unwrap();
        v.movetype = MOVETYPE_NOCLIP;
        v.velocity = Vec3::new(1000.0, 0.0, 0.0);
        v.nextthink = t;
        // The world is not a player: its think runs.
        let world = eng.world();
        eng.ent_mut(world).unwrap().nextthink = t;
    });
    h.step(&mut [], 5);
    assert_eq!(vars(&h, player).origin, before);
    let log = &h.server.game().think_log;
    assert!(log.iter().all(|(index, _)| *index != 1), "{log:?}");
    assert!(log.iter().any(|(index, _)| *index == 0), "{log:?}");
    assert!(vars(&h, player).nextthink > 0.0, "left alone");
}

#[test]
fn movetypes_not_ported_yet_only_think() {
    // 1 and 2 and 99 are movetypes HLDS refuses; TOSS is not ported yet. All think.
    let game = TestGame {
        extra_entities: 4,
        extra_nextthink: 0.01,
        ..TestGame::default()
    };
    let mut h = harness(1, game);
    run(&mut h, |eng| {
        for (index, movetype) in [(2, 1), (3, 2), (4, 99), (5, MOVETYPE_TOSS)] {
            let id = eng.entity_ids()[index];
            assert_eq!(id.index, index as u16);
            let v = eng.ent_mut(id).unwrap();
            v.movetype = movetype;
            v.velocity = Vec3::new(500.0, 0.0, 0.0);
        }
    });
    h.step(&mut [], 2);
    let log = &h.server.game().think_log;
    assert_eq!(log.len(), 4, "{log:?}");
    for index in 2..=5 {
        let (id, _) = h.server.edicts().by_index(index).unwrap();
        assert_eq!(vars(&h, id).origin.x, 0.0, "entity {index} did not move");
    }
}

#[test]
fn force_retouch_relinks_stationary_entities_and_counts_down() {
    let mut h = harness(1, TestGame::default());
    let id = run(&mut h, |eng| spawn_box(eng, Vec3::ZERO));
    // Moved without a link: the absolute box is stale.
    let move_to = |h: &mut Harness, x: f32| {
        run(h, move |eng| eng.ent_mut(id).unwrap().origin = Vec3::new(x, 0.0, 0.0));
    };
    move_to(&mut h, 500.0);
    assert!(vars(&h, id).absmax.x < 100.0, "stale without a retouch");

    run(&mut h, |eng| eng.set_force_retouch(2.0));
    assert!(
        (vars(&h, id).absmax.x - 509.0).abs() < 1e-3,
        "{:?}",
        vars(&h, id).absmax
    );
    // The second pass counts the value down to 0, so a third one does not link.
    h.step(&mut [], 1);
    move_to(&mut h, 900.0);
    assert!(vars(&h, id).absmax.x < 600.0, "linking stopped");
}
