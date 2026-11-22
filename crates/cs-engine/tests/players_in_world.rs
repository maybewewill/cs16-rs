//! Players as world entities: the edict of a player is linked after every command (and its
//! trigger touches found), the player movement trace follows the collision rules of
//! `SV_AddLinksToPM`, the touches the movement reports reach the game as impacts, and a
//! command runs at its client's own time base.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::{FRAME, Harness, TestGame};
use cs_engine::consts::{
    FL_CLIENT, FL_MONSTERCLIP, MOVETYPE_WALK, SOLID_BBOX, SOLID_BSP, SOLID_NOT, SOLID_SLIDEBOX,
    SOLID_TRIGGER,
};
use cs_engine::collision::Hull;
use cs_engine::{EdictId, EngineApi, EntTrace, ServerConfig};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_transport::LoopbackEndpoint;
use glam::Vec3;

type Probe = ProbeClient<LoopbackEndpoint>;

fn harness(game: TestGame) -> Harness {
    let cfg = ServerConfig {
        max_clients: 1,
        max_edicts: 64,
        ..ServerConfig::default()
    };
    Harness::with_config(cfg, game)
}

fn joined(h: &mut Harness) -> Probe {
    let mut probe = h.add_probe("Player");
    h.step(&mut [&mut probe], 200);
    assert_eq!(probe.state(), &ProbeState::InGame);
    probe
}

/// A command of `msec` walking forward at 250 units per second.
fn forward(msec: u8) -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        msec,
        forwardmove: 250.0,
        ..Default::default()
    }
}

fn player_id(h: &Harness) -> EdictId {
    h.server.edicts().client(0)
}

fn player_origin(h: &Harness) -> Vec3 {
    h.server
        .edicts()
        .get(player_id(h))
        .expect("player")
        .vars
        .origin
}

/// Entity with the class `class`, if there is one.
fn find(h: &Harness, class: &str) -> Option<EdictId> {
    (0..64).find_map(|i| {
        h.server
            .edicts()
            .by_index(i)
            .filter(|(_, e)| e.vars.classname == class)
            .map(|(id, _)| id)
    })
}

fn solid_box(eng: &mut EngineApi<'_>, class: &str, solid: u8, origin: Vec3, half: f32) -> EdictId {
    let id = eng.create_entity(class).expect("free edict");
    eng.ent_mut(id).unwrap().solid = solid;
    eng.set_size(id, Vec3::splat(-half), Vec3::splat(half));
    eng.set_origin(id, origin);
    id
}

#[test]
fn player_edict_is_linked_after_each_command() {
    let game = TestGame {
        spawn_origins: vec![Vec3::new(100.0, 0.0, 0.0)],
        ..TestGame::default()
    };
    let mut h = harness(game);
    let mut probe = joined(&mut h);
    // The engine's defaults for a player the game did not change.
    let vars = h.server.edicts().get(player_id(&h)).unwrap().vars.clone();
    assert_eq!((vars.solid, vars.movetype), (SOLID_SLIDEBOX, MOVETYPE_WALK));

    // The test game moves the player 2.5 units per command without linking it itself.
    let seen = Rc::new(RefCell::new(Vec::new()));
    let me = player_id(&h);
    let start = player_origin(&h);
    for n in 1..=6u8 {
        let expected = start + Vec3::new(2.5 * f32::from(n), 0.0, 0.0);
        let sink = Rc::clone(&seen);
        probe.queue_cmd(forward(10));
        // Runs at this frame's `start_frame`, after the command.
        h.server.game_mut().act(move |eng| {
            let found = eng.entities_in_box(expected - Vec3::splat(0.1), expected + Vec3::splat(0.1));
            sink.borrow_mut().push((expected, found));
        });
        h.step(&mut [&mut probe], 1);
    }
    assert_eq!(seen.borrow().len(), 6);
    for (expected, found) in seen.borrow().iter() {
        assert!(found.contains(&me), "{expected:?}: {found:?}");
    }

    // The player left the first place of the line: it is not found there any more.
    let first = seen.borrow()[0].0;
    let left = Rc::new(RefCell::new(None));
    let sink = Rc::clone(&left);
    h.server.game_mut().act(move |eng| {
        *sink.borrow_mut() =
            Some(eng.entities_in_box(first - Vec3::splat(0.1), first + Vec3::splat(0.1)));
    });
    h.step(&mut [], 1);
    let found = left.borrow_mut().take().expect("ran");
    assert!(!found.contains(&me), "{found:?}");
}

#[test]
fn walking_into_a_trigger_touches_it() {
    let game = TestGame {
        spawn_origins: vec![Vec3::new(0.0, 0.0, 0.0)],
        ..TestGame::default()
    };
    let mut h = harness(game);
    let mut probe = joined(&mut h);
    // The trigger spans x 40..60; the player (a box of 2 around its origin) starts at 0.
    h.server
        .game_mut()
        .act(|eng| {
            solid_box(eng, "test_trigger", SOLID_TRIGGER, Vec3::new(50.0, 0.0, 0.0), 10.0);
        });
    h.step(&mut [], 1);
    let trigger = find(&h, "test_trigger").expect("trigger");
    let me = player_id(&h);

    // 100 ms: 25 units, still short of the trigger.
    for _ in 0..10 {
        probe.queue_cmd(forward(10));
        h.step(&mut [&mut probe], 1);
    }
    assert!(h.server.game().touch_log.is_empty(), "{:?}", h.server.game().touch_log);

    // Another 200 ms carries the player through it.
    for _ in 0..20 {
        probe.queue_cmd(forward(10));
        h.step(&mut [&mut probe], 1);
    }
    h.step(&mut [&mut probe], 5);
    let log = &h.server.game().touch_log;
    assert!(!log.is_empty(), "the trigger was never touched");
    assert!(
        log.iter().all(|&(t, o)| (t, o) == (trigger.index, me.index)),
        "{log:?}"
    );
    // It was touched while the player was inside, not afterwards: every command inside the
    // trigger touches once (about 8 of the 10 ms steps in 20 units), none outside.
    assert!((1..=12).contains(&log.len()), "{log:?}");
    let before = log.len();
    h.step(&mut [&mut probe], 10);
    assert_eq!(h.server.game().touch_log.len(), before);
}

#[test]
fn player_move_trace_ignores_triggers_and_self() {
    let mut h = harness(TestGame::default());
    let _probe = joined(&mut h);
    let me = player_id(&h);
    let report = Rc::new(RefCell::new(None));
    let sink = Rc::clone(&report);
    h.server.game_mut().act(move |eng| {
        // The player is a box of 16 around the origin; a line along x passes through it.
        eng.set_size(me, Vec3::splat(-16.0), Vec3::splat(16.0));
        let at = |x| Vec3::new(x, 0.0, 0.0);
        let trace = |eng: &EngineApi<'_>| {
            let t = eng.trace_player_move(me, at(-200.0), at(200.0), Hull::Point);
            (t.trace.fraction, t.ent)
        };
        let mut out = Vec::new();

        // Nothing but the player itself and a trigger in the way: a clear move.
        eng.ent_mut(me).unwrap().origin = at(0.0);
        eng.link_entity(me, false);
        solid_box(eng, "trigger", SOLID_TRIGGER, at(-100.0), 20.0);
        out.push(("self and trigger", trace(eng)));

        // Entities that never collide with a player's movement.
        let ghost = solid_box(eng, "not", SOLID_NOT, at(-80.0), 10.0);
        eng.ent_mut(ghost).unwrap().skin = -3;
        eng.link_entity(ghost, false);
        let owned = solid_box(eng, "owned", SOLID_BBOX, at(-70.0), 10.0);
        eng.ent_mut(owned).unwrap().owner = Some(me);
        let dead = solid_box(eng, "dead", SOLID_BBOX, at(-60.0), 10.0);
        eng.ent_mut(dead).unwrap().flags |= FL_CLIENT;
        eng.ent_mut(dead).unwrap().health = 0.0;
        let flat = solid_box(eng, "flat", SOLID_BBOX, at(-50.0), 10.0);
        eng.set_size(flat, Vec3::new(-10.0, -10.0, 0.0), Vec3::new(10.0, 10.0, 1.0));
        let point = eng.create_entity("point").unwrap();
        eng.ent_mut(point).unwrap().solid = SOLID_BBOX;
        eng.set_origin(point, at(-40.0));
        // `SOLID_BSP` with `FL_MONSTERCLIP` is skipped before its model is looked at.
        let clip = solid_box(eng, "monsterclip", SOLID_BSP, at(-30.0), 10.0);
        eng.ent_mut(clip).unwrap().flags |= FL_MONSTERCLIP;
        out.push(("ignored kinds", trace(eng)));

        // A live solid box does stop the move, and so does another client-like box.
        let wall = solid_box(eng, "wall", SOLID_BBOX, at(-120.0), 10.0);
        out.push(("wall", trace(eng)));
        let ids = [ghost, owned, dead, flat, point, clip, wall];
        *sink.borrow_mut() = Some((out, ids));
    });
    h.step(&mut [], 1);
    let (out, ids) = report.borrow_mut().take().expect("ran");
    let by = |name: &str| out.iter().find(|(n, _)| *n == name).unwrap().1;
    assert_eq!(by("self and trigger"), (1.0, None));
    assert_eq!(by("ignored kinds"), (1.0, None));
    // The wall spans x -130..-110; the point trace starts at -200: 70 of 400 units.
    let (fraction, ent) = by("wall");
    assert!((fraction - 70.0 / 400.0).abs() < 1e-4, "{fraction}");
    assert_eq!(ent, Some(ids[6]));
}

#[test]
fn player_move_trace_is_refused_for_bad_input() {
    let mut h = harness(TestGame::default());
    let _probe = joined(&mut h);
    let me = player_id(&h);
    let stale = EdictId {
        index: me.index,
        serial: me.serial.wrapping_add(1),
    };
    let report = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&report);
    h.server.game_mut().act(move |eng| {
        let ok = Vec3::new(10.0, 0.0, 0.0);
        let put = |t: EntTrace| sink.borrow_mut().push((t.trace.fraction, t.ent));
        put(eng.trace_player_move(stale, Vec3::ZERO, ok, Hull::Point));
        put(eng.trace_player_move(me, Vec3::NAN, ok, Hull::Point));
        put(eng.trace_player_move(me, Vec3::ZERO, Vec3::splat(1.0e9), Hull::Point));
    });
    h.step(&mut [], 1);
    assert_eq!(*report.borrow(), vec![(1.0, None); 3]);
}

#[test]
fn touches_the_movement_reports_arrive_as_impacts() {
    let mut h = harness(TestGame::default());
    let mut probe = joined(&mut h);
    h.server
        .game_mut()
        .act(|eng| {
            solid_box(eng, "wall", SOLID_BBOX, Vec3::new(0.0, 500.0, 0.0), 8.0);
        });
    h.step(&mut [&mut probe], 1);
    let wall = find(&h, "wall").expect("wall");
    let me = player_id(&h);

    // Reported for a command: `SV_Impact(wall, player)` touches the wall, then the player.
    h.server.game_mut().usercmd_touches = vec![wall];
    probe.queue_cmd(forward(10));
    h.step(&mut [&mut probe], 1);
    assert_eq!(
        h.server.game().touch_log,
        vec![(wall.index, me.index), (me.index, wall.index)]
    );

    // A player that is not solid gets no impacts.
    h.server.game_mut().act(move |eng| eng.ent_mut(me).unwrap().solid = SOLID_NOT);
    h.step(&mut [&mut probe], 1);
    // The 0 ms command of that idle frame ran before the action made the player not solid, so
    // it reported its touch like any command (it no longer skipped, see `handle_move`).
    let logged = h.server.game().touch_log.len();
    probe.queue_cmd(forward(10));
    h.step(&mut [&mut probe], 1);
    assert_eq!(h.server.game().touch_log.len(), logged);

    // Touches reported with no command running are dropped, not kept for the next one.
    h.server.game_mut().usercmd_touches.clear();
    h.server.game_mut().act(move |eng| {
        eng.ent_mut(me).unwrap().solid = SOLID_SLIDEBOX;
        eng.player_touched(me, wall);
        // Not a client's edict, and a player touching itself: refused.
        eng.player_touched(wall, me);
        eng.player_touched(me, me);
    });
    h.step(&mut [&mut probe], 1);
    probe.queue_cmd(forward(10));
    h.step(&mut [&mut probe], 1);
    assert_eq!(h.server.game().touch_log.len(), logged);
}

#[test]
fn commands_run_at_their_clients_time_base() {
    let mut h = harness(TestGame::default());
    let mut probe = joined(&mut h);
    let before = h.server.game().usercmd_times.len();

    // One packet with commands of 10, 20 and 30 ms: the frame is 10 ms long, so the commands
    // start 50 ms before the end of the frame's time, `SV_EstablishTimeBase`.
    for msec in [10, 20, 30] {
        probe.queue_cmd(forward(msec));
    }
    h.step(&mut [&mut probe], 1);
    let game = h.server.game();
    let sim_time_at_frame = *game.start_frame_times.last().unwrap();
    let times = &game.usercmd_times[before..];
    let frametimes = &game.usercmd_frametimes[before..];
    assert_eq!(times.len(), 3, "{times:?}");
    let base = sim_time_at_frame + FRAME - 0.060;
    for (got, want) in times.iter().zip([base + 0.010, base + 0.030, base + 0.060]) {
        assert!((got - want).abs() < 1e-6, "{times:?} vs base {base}");
    }
    // Each command's own frame time, not the host's.
    for (got, want) in frametimes.iter().zip([0.010f32, 0.020, 0.030]) {
        assert!((got - want).abs() < 1e-6, "{frametimes:?}");
    }
}
