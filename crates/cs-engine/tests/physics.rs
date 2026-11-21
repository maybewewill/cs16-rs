//! The server frame order (packets, physics, time) and the physics pass: simulation time, the
//! think of every entity, and the `NONE`, `NOCLIP` and `FOLLOW` movetypes.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use common::{FRAME, Harness, TestGame, schedule_extra_thinks};
use cs_assets::bsp::BspMap;
use cs_engine::consts::{
    FL_KILLME, MOVETYPE_FOLLOW, MOVETYPE_NOCLIP, MOVETYPE_NONE, MOVETYPE_PUSH, SOLID_BBOX,
    SOLID_TRIGGER,
};
use cs_engine::{EdictId, EngineApi, SV_TIME_AT_MAP_START, ServerConfig, ServerMap};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_transport::LoopbackEndpoint;
use cs_vfs::{Vfs, VfsConfig};
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

    // It ran before the frame's time advance, but at the client's `svtimebase`: the frame's
    // simulation time plus the frame time minus the duration of the commands, plus the
    // command's own 10 ms (`SV_EstablishTimeBase`, `SV_RunCmd`). That frame is 10 ms long
    // and the command 10 ms, so the command's time is the end of the frame, and the frame's
    // `start_frame` (at the simulation time before the advance) is one frame earlier.
    let at_cmd = *game.usercmd_times.last().unwrap();
    let at_start = *game.start_frame_times.last().unwrap();
    assert!((at_cmd - at_start - FRAME).abs() < EPS, "{at_cmd} vs {at_start}");
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
        ..TestGame::default()
    };
    let mut h = harness(1, game);
    schedule_extra_thinks(&mut h, 0.0, vec![SV_TIME_AT_MAP_START + 0.004, 0.5]);
    let settle = h.server.game().start_frame_times.len();
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
    assert_eq!(
        h.server.game().start_frame_times[settle..],
        [SV_TIME_AT_MAP_START]
    );
}

#[test]
fn think_due_after_the_frame_waits() {
    // 11 ms into a 10 ms frame: not due in the first frame, due in the second.
    let game = TestGame {
        extra_entities: 1,
        ..TestGame::default()
    };
    let mut h = harness(1, game);
    schedule_extra_thinks(&mut h, SV_TIME_AT_MAP_START + 0.011, Vec::new());
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
    let settle = h.server.game().start_frame_times.len();
    h.step(&mut [], 1);
    // A 10 s stall: the simulation takes one 0.25 s step.
    h.now += 10.0;
    h.server.frame(&mut h.hub, h.now);
    h.step(&mut [], 1);
    let times = &h.server.game().start_frame_times[settle..];
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
        ..TestGame::default()
    };
    let mut h = harness(1, game);
    schedule_extra_thinks(&mut h, 0.01, Vec::new());
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

/// `NOCLIP`, `FOLLOW` and `PUSH` never run `SV_CheckVelocity`: a NaN or overflowing velocity
/// must not poison the origin or the angles (nor those of a follower).
#[test]
fn non_finite_and_huge_velocities_keep_noclip_follow_and_push_finite() {
    for bad in [f32::NAN, f32::INFINITY, 3e38] {
        let mut h = harness(1, TestGame::default());
        let (noclip, follower, pusher) = run(&mut h, move |eng| {
            let noclip = spawn_box(eng, Vec3::new(10.0, 20.0, 30.0));
            let v = eng.ent_mut(noclip).unwrap();
            v.movetype = MOVETYPE_NOCLIP;
            v.velocity = Vec3::new(bad, 1.0, bad);
            v.avelocity = Vec3::new(bad, bad, 0.0);
            let follower = eng.create_entity("test_follower").expect("free edict");
            let v = eng.ent_mut(follower).unwrap();
            v.movetype = MOVETYPE_FOLLOW;
            v.aiment = Some(noclip);
            v.v_angle = Vec3::new(bad, 0.0, 0.0);
            let pusher = spawn_box(eng, Vec3::new(500.0, 0.0, 0.0));
            let v = eng.ent_mut(pusher).unwrap();
            v.movetype = MOVETYPE_PUSH;
            v.velocity = Vec3::new(bad, bad, 0.0);
            v.avelocity = Vec3::new(0.0, bad, 0.0);
            v.nextthink = 1000.0;
            (noclip, follower, pusher)
        });
        // Enough frames for 3e38 per second to overflow an f32 origin.
        h.step(&mut [], 40);
        for id in [noclip, follower, pusher] {
            let v = vars(&h, id);
            assert!(v.origin.is_finite(), "{bad}: {:?} {:?}", id, v.origin);
            assert!(v.angles.is_finite(), "{bad}: {:?} {:?}", id, v.angles);
            assert!(v.velocity.is_finite() && v.avelocity.is_finite(), "{bad}: {id:?}");
        }
    }
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
        // An entity that is not a player: its think runs. (The world is a `PUSH` entity on its
        // own clock `ltime`, as in HLDS, so it is no control for this.)
        let other = eng.create_entity("control").unwrap();
        eng.ent_mut(other).unwrap().nextthink = t;
    });
    h.step(&mut [], 5);
    assert_eq!(vars(&h, player).origin, before);
    let log = &h.server.game().think_log;
    assert!(log.iter().all(|(index, _)| *index != 1), "{log:?}");
    assert!(log.iter().any(|(index, _)| *index > 1), "{log:?}");
    assert!(vars(&h, player).nextthink > 0.0, "left alone");
}

#[test]
fn movetypes_hlds_refuses_only_think() {
    // 1 and 2 and 99 are movetypes HLDS refuses. They only think.
    let game = TestGame {
        extra_entities: 3,
        ..TestGame::default()
    };
    let mut h = harness(1, game);
    schedule_extra_thinks(&mut h, 0.01, Vec::new());
    run(&mut h, |eng| {
        for (index, movetype) in [(2, 1), (3, 2), (4, 99)] {
            let id = eng.entity_ids()[index];
            assert_eq!(id.index, index as u16);
            let v = eng.ent_mut(id).unwrap();
            v.movetype = movetype;
            v.velocity = Vec3::new(500.0, 0.0, 0.0);
        }
    });
    h.step(&mut [], 2);
    let log = &h.server.game().think_log;
    assert_eq!(log.len(), 3, "{log:?}");
    for index in 2..=4 {
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

#[test]
fn settle_passes_run_before_the_baselines_are_cut() {
    // One player slot: 2 passes of 0.1 s at activation, `sv_time` stays 1.0. A think due at
    // 1.05 runs in them; its `skin` is in the baseline a joining probe receives.
    let game = TestGame {
        extra_entities: 1,
        extra_nextthink: SV_TIME_AT_MAP_START + 0.05,
        think_sets_skin: Some(9),
        ..TestGame::default()
    };
    let mut h = harness(1, game);
    let log = &h.server.game().think_log;
    assert_eq!(log.len(), 1, "{log:?}");
    assert!((log[0].1 - (SV_TIME_AT_MAP_START + 0.05)).abs() < EPS);
    assert_eq!(h.server.game().start_frame_calls, 2);
    let probe = joined(&mut h);
    assert_eq!(probe.baselines()[&2].skin, 9);
    // The settle passes did not advance the simulation time.
    assert_eq!(h.server.game().start_frame_times[..3], [1.0, 1.0, 1.0]);
}

#[test]
fn think_that_sets_killme_is_freed_after_the_call() {
    let game = TestGame {
        extra_entities: 1,
        think_kills_self: true,
        ..TestGame::default()
    };
    let mut h = harness(1, game);
    schedule_extra_thinks(&mut h, 0.01, Vec::new());
    h.step(&mut [], 1);
    assert_eq!(h.server.game().think_log.len(), 1);
    assert!(h.server.edicts().by_index(2).is_none());
}

#[test]
fn rejected_movetype_is_reported_once_per_entity_per_map() {
    let game = TestGame {
        extra_entities: 2,
        ..TestGame::default()
    };
    let mut h = harness(1, game);
    let set_bad = |h: &mut Harness| {
        run(h, |eng| {
            for id in eng.entity_ids().into_iter().filter(|id| id.index >= 2) {
                eng.ent_mut(id).unwrap().movetype = 99;
            }
        });
    };
    set_bad(&mut h);
    h.step(&mut [], 5);
    assert_eq!(h.server.movetype_warnings(), 2);
    // A new map starts again.
    h.server.change_map(ServerMap::stub("second"));
    set_bad(&mut h);
    h.step(&mut [], 5);
    assert_eq!(h.server.movetype_warnings(), 4);
}

#[test]
fn follow_and_force_retouch_queue_touches() {
    let mut h = harness(1, TestGame::default());
    // A trigger at x = 1000 and a box resting in it, linked without touching.
    let (parent, follower) = run(&mut h, |eng| {
        let trigger = spawn_box(eng, Vec3::new(1000.0, 0.0, 0.0));
        eng.ent_mut(trigger).unwrap().solid = SOLID_TRIGGER;
        eng.set_origin(trigger, Vec3::new(1000.0, 0.0, 0.0));
        spawn_box(eng, Vec3::new(1000.0, 0.0, 0.0));
        let parent = spawn_box(eng, Vec3::new(-2000.0, 0.0, 0.0));
        let follower = spawn_box(eng, Vec3::new(-2000.0, 0.0, 0.0));
        (parent, follower)
    });
    h.step(&mut [], 3);
    assert!(h.server.game().touch_log.is_empty());

    // `force_retouch` re-links the resting box with touching; the touch is delivered in the
    // same frame.
    run(&mut h, |eng| eng.set_force_retouch(1.0));
    let after_retouch = h.server.game().touch_log.len();
    assert!(after_retouch >= 1, "{after_retouch}");
    assert_eq!(h.server.pending_touch_count(), 0);

    // A follower carried into the trigger touches it.
    run(&mut h, move |eng| {
        let v = eng.ent_mut(follower).unwrap();
        v.movetype = MOVETYPE_FOLLOW;
        v.aiment = Some(parent);
        eng.set_origin(parent, Vec3::new(1000.0, 0.0, 0.0));
    });
    assert!(h.server.game().touch_log.len() > after_retouch);
    assert_eq!(h.server.pending_touch_count(), 0);
}

fn dust2() -> Option<(ServerMap, ServerMap)> {
    let vfs = Vfs::discover(None, VfsConfig::default()).ok()?;
    let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("de_dust2 reads");
    let bsp = Arc::new(BspMap::load(&vfs, "de_dust2").expect("de_dust2 parses"));
    let make = || ServerMap::from_bsp("de_dust2", &raw, bsp.clone()).expect("valid BSP header");
    Some((make(), make()))
}

/// A noclip model entity that flies from a spot the viewer cannot see into its view is sent
/// to the viewer once it is there, and not before: the relink after each move keeps the
/// leaves it touches (what the visibility test reads) current.
#[test]
fn noclip_entity_enters_a_probes_view_on_a_real_map() {
    let Some((map, probe_map)) = dust2() else {
        return;
    };
    let origin_of = |classname: &str| {
        probe_map
            .entities
            .iter()
            .filter(|e| e.classname() == classname)
            .find_map(|e| e.get_vec3("origin"))
            .unwrap_or_else(|| panic!("de_dust2 has a {classname}"))
    };
    let t_spawn = origin_of("info_player_deathmatch");
    let ct_spawn = origin_of("info_player_start");
    let eye = ct_spawn + Vec3::new(0.0, 0.0, 17.0);
    assert!(!probe_map.can_see(eye, t_spawn - Vec3::ONE, t_spawn + Vec3::ONE));

    let game = TestGame {
        spawn_origins: vec![ct_spawn],
        ..TestGame::default()
    };
    let cfg = ServerConfig {
        max_clients: 2,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_map(cfg, map, game);
    let mut probe = h.add_probe("CT");
    h.step(&mut [&mut probe], 300);
    assert_eq!(probe.state(), &ProbeState::InGame);

    // 100 frames of 10 ms from the far spawn to the viewer's.
    let velocity = (ct_spawn - t_spawn) / (100.0 * FRAME as f32);
    let id = run(&mut h, move |eng| {
        let model = eng
            .precache_model("models/test.mdl")
            .ok()
            .or_else(|| eng.model_index("models/test.mdl"))
            .unwrap();
        let id = eng.create_entity("test_box").unwrap();
        let v = eng.ent_mut(id).unwrap();
        v.modelindex = model;
        v.solid = SOLID_BBOX;
        v.movetype = MOVETYPE_NOCLIP;
        v.velocity = velocity;
        eng.set_size(id, Vec3::splat(-16.0), Vec3::splat(16.0));
        eng.set_origin(id, t_spawn);
        id
    });
    // The first 10 frames take it a tenth of the way: still where the viewer cannot see.
    h.server.game_mut().add_to_full_pack_ents.clear();
    h.step(&mut [&mut probe], 10);
    assert!(
        !h.server.game().add_to_full_pack_ents.contains(&id.index),
        "listed before it crossed into view"
    );
    h.step(&mut [&mut probe], 100);
    assert!(
        h.server.game().add_to_full_pack_ents.contains(&id.index),
        "never listed after it crossed into view"
    );
}
