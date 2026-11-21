//! The pushers (`MOVETYPE_PUSH`): movement on the pusher's own clock `ltime`, what they carry,
//! what blocks them and `GameDll::blocked`; plus the conveyor bookkeeping of
//! `SV_CheckMovingGround`.
//!
//! The pushers here are `SOLID_BBOX` boxes with `MOVETYPE_PUSH` (the stub map has no brush
//! models); the real-map test uses a brush model of de_dust2 and returns without assertions when
//! the game is not installed. Riders are `MOVETYPE_FLY` boxes with no velocity: they stay where
//! they are unless something moves them.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use common::{Harness, TestGame};
use cs_assets::bsp::BspMap;
use cs_engine::consts::{
    FL_CLIENT, FL_CONVEYOR, FL_ONGROUND, MOVETYPE_FLY, MOVETYPE_NONE, MOVETYPE_PUSH,
    MOVETYPE_WALK, SOLID_BBOX, SOLID_BSP, SOLID_SLIDEBOX,
};
use cs_engine::{EdictId, EngineApi, EntVars, ServerConfig, ServerMap};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

fn harness(game: TestGame) -> Harness {
    let cfg = ServerConfig {
        max_clients: 1,
        max_edicts: 64,
        ..ServerConfig::default()
    };
    Harness::with_config(cfg, game)
}

/// Runs `f` as the game at the start of the next frame (which also runs that frame's physics)
/// and returns what it returned.
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

fn vars(h: &Harness, id: EdictId) -> EntVars {
    h.server.edicts().get(id).expect("live").vars.clone()
}

fn near(a: Vec3, b: Vec3, eps: f32) -> bool {
    (a - b).abs().max_element() <= eps
}

/// A `MOVETYPE_PUSH` box of half-size `half` that moves by `velocity` until `nextthink` on its
/// own clock (which starts at 0).
fn pusher(
    eng: &mut EngineApi<'_>,
    origin: Vec3,
    half: f32,
    velocity: Vec3,
    nextthink: f64,
) -> EdictId {
    let id = eng.create_entity("pusher").expect("free edict");
    let v = eng.ent_mut(id).unwrap();
    v.solid = SOLID_BBOX;
    v.movetype = MOVETYPE_PUSH;
    v.velocity = velocity;
    v.nextthink = nextthink;
    eng.set_size(id, Vec3::splat(-half), Vec3::splat(half));
    eng.set_origin(id, origin);
    id
}

/// A box of half-size `half` that stays where it is.
fn body(eng: &mut EngineApi<'_>, class: &str, origin: Vec3, half: f32) -> EdictId {
    let id = eng.create_entity(class).expect("free edict");
    let v = eng.ent_mut(id).unwrap();
    v.solid = SOLID_BBOX;
    v.movetype = MOVETYPE_FLY;
    eng.set_size(id, Vec3::splat(-half), Vec3::splat(half));
    eng.set_origin(id, origin);
    id
}

/// `body`, standing on `ground`.
fn rider(eng: &mut EngineApi<'_>, origin: Vec3, ground: EdictId) -> EdictId {
    let id = body(eng, "rider", origin, 8.0);
    let v = eng.ent_mut(id).unwrap();
    v.flags |= FL_ONGROUND;
    v.groundentity = Some(ground);
    id
}

#[test]
fn pusher_moves_by_velocity_on_ltime() {
    let mut h = harness(TestGame::default());
    // Both are due to stop at ltime 0.055; only the first has a `nextthink` to run until.
    let (runs, idle) = run(&mut h, |eng| {
        let runs = pusher(eng, Vec3::ZERO, 16.0, Vec3::new(100.0, 0.0, 0.0), 0.055);
        let idle = pusher(eng, Vec3::new(0.0, 500.0, 0.0), 16.0, Vec3::new(100.0, 0.0, 0.0), 0.0);
        (runs, idle)
    });
    // The frame that created them ran their physics too: 5 more frames bring `ltime` to 0.06
    // minus what is left until `nextthink`, so the think is due only in the last of them.
    h.step(&mut [], 4);
    assert!(h.server.game().think_log.is_empty(), "{:?}", h.server.game().think_log);
    let before = vars(&h, runs);
    assert!((before.ltime - 0.05).abs() < 1e-6, "{}", before.ltime);
    h.step(&mut [], 1);

    let v = vars(&h, runs);
    assert!((v.ltime - 0.055).abs() < 1e-9, "ltime {}", v.ltime);
    assert!((v.origin.x - 5.5).abs() < 1e-3, "{:?}", v.origin);
    assert_eq!((v.origin.y, v.origin.z), (0.0, 0.0));
    // The think ran once, as the clock reached `nextthink`, and cleared it.
    let thinks: Vec<u16> = h.server.game().think_log.iter().map(|&(i, _)| i).collect();
    assert_eq!(thinks, [runs.index]);
    assert_eq!(v.nextthink, 0.0);

    // From here on it has no think pending and stays.
    h.step(&mut [], 5);
    let later = vars(&h, runs);
    assert_eq!((later.origin, later.ltime), (v.origin, v.ltime));
    assert_eq!(h.server.game().think_log.len(), 1);

    // The one that never had a `nextthink` never moved.
    let still = vars(&h, idle);
    assert_eq!(still.origin, Vec3::new(0.0, 500.0, 0.0));
    assert_eq!(still.ltime, 0.0);
}

#[test]
fn rider_moves_with_the_pusher() {
    let mut h = harness(TestGame::default());
    let (p, r, bystander) = run(&mut h, |eng| {
        let p = pusher(eng, Vec3::ZERO, 32.0, Vec3::new(100.0, 0.0, 0.0), 1.0);
        let r = rider(eng, Vec3::new(0.0, 0.0, 40.0), p);
        let bystander = body(eng, "bystander", Vec3::new(500.0, 0.0, 40.0), 8.0);
        (p, r, bystander)
    });
    let moved = vars(&h, p).origin;
    assert!(moved.x > 0.5, "the pusher moved: {moved:?}");
    let rv = vars(&h, r);
    assert!(near(rv.origin, moved + Vec3::new(0.0, 0.0, 40.0), 1e-4), "{:?}", rv.origin);
    // A non-player that was pushed is no longer on the ground.
    assert_eq!(rv.flags & FL_ONGROUND, 0);
    assert_eq!(vars(&h, bystander).origin, Vec3::new(500.0, 0.0, 40.0));
    assert!(h.server.game().blocked_log.is_empty());

    // It keeps being carried while the game keeps it on the pusher.
    for _ in 0..4 {
        h.server.game_mut().act(move |eng| {
            let v = eng.ent_mut(r).unwrap();
            v.flags |= FL_ONGROUND;
            v.groundentity = Some(p);
        });
        h.step(&mut [], 1);
    }
    let moved = vars(&h, p).origin;
    let rv = vars(&h, r);
    assert!(moved.x > 4.0, "{moved:?}");
    assert!(near(rv.origin, moved + Vec3::new(0.0, 0.0, 40.0), 1e-3), "{:?}", rv.origin);
}

/// A pusher at the origin moving along +x; a free rider on top, a rider whose way is shut by a
/// wall, in that index order.
fn crush_scene(h: &mut Harness) -> (EdictId, EdictId, EdictId, EdictId) {
    run(h, |eng| {
        let p = pusher(eng, Vec3::ZERO, 16.0, Vec3::new(100.0, 0.0, 0.0), 1.0);
        let free = rider(eng, Vec3::new(0.0, 0.0, 24.0), p);
        // Inside the pusher's final box and against the wall: it cannot go anywhere.
        let crushed = body(eng, "crushed", Vec3::new(24.0, 0.0, 0.0), 8.0);
        let wall = body(eng, "wall", Vec3::new(40.0, 0.0, 0.0), 8.0);
        (p, free, crushed, wall)
    })
}

#[test]
fn crushed_rider_reverts_the_push() {
    let game = TestGame {
        blocked_stops_pusher: true,
        ..TestGame::default()
    };
    let mut h = harness(game);
    let (p, free, crushed, wall) = crush_scene(&mut h);

    assert_eq!(h.server.game().blocked_log, [(p.index, crushed.index)]);
    // The pusher is where it was, with the clock it had; nothing is NaN.
    let pv = vars(&h, p);
    assert_eq!(pv.origin, Vec3::ZERO);
    assert!(pv.ltime.abs() < 1e-12, "{}", pv.ltime);
    // The one that was blocked stays, and the one already carried is put back.
    assert_eq!(vars(&h, crushed).origin, Vec3::new(24.0, 0.0, 0.0));
    assert_eq!(vars(&h, free).origin, Vec3::new(0.0, 0.0, 24.0));
    assert_eq!(vars(&h, wall).origin, Vec3::new(40.0, 0.0, 0.0));
    for id in [p, free, crushed, wall] {
        let v = vars(&h, id);
        assert!(v.origin.is_finite() && v.absmin.is_finite() && v.absmax.is_finite(), "{v:?}");
    }
    // The game stopped it, so the next frames do not push again.
    h.step(&mut [], 5);
    assert_eq!(h.server.game().blocked_log.len(), 1);
    assert_eq!(vars(&h, p).origin, Vec3::ZERO);
}

#[test]
fn a_pusher_the_game_leaves_going_is_blocked_every_frame() {
    let mut h = harness(TestGame::default());
    let (p, _, crushed, _) = crush_scene(&mut h);
    h.step(&mut [], 4);
    let log = h.server.game().blocked_log.clone();
    assert_eq!(log, vec![(p.index, crushed.index); 5]);
    assert_eq!(vars(&h, p).origin, Vec3::ZERO);
    assert_eq!(vars(&h, crushed).origin, Vec3::new(24.0, 0.0, 0.0));
}

#[test]
fn blocked_may_remove_the_pusher() {
    let game = TestGame {
        blocked_removes_pusher: true,
        ..TestGame::default()
    };
    let mut h = harness(game);
    let (p, free, crushed, wall) = crush_scene(&mut h);

    assert_eq!(h.server.game().blocked_log, [(p.index, crushed.index)]);
    assert!(h.server.edicts().get(p).is_none(), "the pusher was removed");
    // What was moved is put back all the same.
    assert_eq!(vars(&h, free).origin, Vec3::new(0.0, 0.0, 24.0));
    assert_eq!(vars(&h, crushed).origin, Vec3::new(24.0, 0.0, 0.0));

    // The next frames run without it.
    h.step(&mut [], 5);
    assert_eq!(h.server.game().blocked_log.len(), 1);
    for id in [free, crushed, wall] {
        assert!(h.server.edicts().get(id).is_some());
    }
}

#[test]
fn rotating_pusher_carries_and_rotates() {
    let mut h = harness(TestGame::default());
    let origin = Vec3::new(100.0, 0.0, 0.0);
    let (p, r) = run(&mut h, move |eng| {
        let p = pusher(eng, origin, 32.0, Vec3::ZERO, 10.0);
        eng.ent_mut(p).unwrap().avelocity = Vec3::new(0.0, 90.0, 0.0);
        let r = rider(eng, origin + Vec3::new(20.0, 0.0, 40.0), p);
        (p, r)
    });
    for _ in 0..9 {
        h.server.game_mut().act(move |eng| {
            let v = eng.ent_mut(r).unwrap();
            v.flags |= FL_ONGROUND;
            v.groundentity = Some(p);
        });
        h.step(&mut [], 1);
    }
    let pv = vars(&h, p);
    let yaw = pv.angles.y;
    assert!((yaw - 90.0 * pv.ltime as f32).abs() < 1e-3, "{yaw} at ltime {}", pv.ltime);
    assert!(yaw > 8.0, "it turned: {yaw}");
    assert_eq!(pv.origin, origin, "a rotation does not translate");

    // The rider is where the turn put it, around the pusher's origin, and faces as it turned.
    let (sin, cos) = yaw.to_radians().sin_cos();
    let expect = origin + Vec3::new(20.0 * cos, 20.0 * sin, 40.0);
    let rv = vars(&h, r);
    assert!(near(rv.origin, expect, 0.02), "{:?} not {expect:?}", rv.origin);
    assert!((rv.angles.y - yaw).abs() < 1e-3, "{} not {yaw}", rv.angles.y);
    assert!(h.server.game().blocked_log.is_empty());
}

#[test]
fn a_blocked_rotation_turns_everything_back() {
    let game = TestGame {
        blocked_stops_pusher: true,
        ..TestGame::default()
    };
    let mut h = harness(game);
    // The pusher turns 90 degrees in one frame, which swings the rider (inside its box, so it
    // is carried) into a wall (`MOVETYPE_NONE`: a pusher never moves it).
    let (p, r, wall) = run(&mut h, |eng| {
        let p = pusher(eng, Vec3::ZERO, 32.0, Vec3::ZERO, 10.0);
        eng.ent_mut(p).unwrap().avelocity = Vec3::new(0.0, 9000.0, 0.0);
        let r = body(eng, "rider", Vec3::new(30.0, 0.0, 0.0), 8.0);
        let wall = body(eng, "wall", Vec3::new(15.0, 15.0, 0.0), 6.0);
        eng.ent_mut(wall).unwrap().movetype = MOVETYPE_NONE;
        (p, r, wall)
    });
    assert_eq!(h.server.game().blocked_log, [(p.index, r.index)]);
    let pv = vars(&h, p);
    assert_eq!(pv.angles, Vec3::ZERO);
    assert!(pv.ltime.abs() < 1e-12);
    let rv = vars(&h, r);
    assert_eq!(rv.origin, Vec3::new(30.0, 0.0, 0.0));
    assert_eq!(rv.angles, Vec3::ZERO);
    assert_eq!(vars(&h, wall).origin, Vec3::new(15.0, 15.0, 0.0));
}

#[test]
fn a_player_is_carried_and_keeps_its_ground() {
    let mut h = harness(TestGame::default());
    let (p, player) = run(&mut h, |eng| {
        let p = pusher(eng, Vec3::ZERO, 32.0, Vec3::new(100.0, 0.0, 0.0), 1.0);
        let player = eng.client_edict(0);
        let v = eng.ent_mut(player).unwrap();
        v.movetype = MOVETYPE_WALK;
        v.solid = SOLID_SLIDEBOX;
        v.flags = FL_CLIENT | FL_ONGROUND;
        v.groundentity = Some(p);
        eng.set_size(player, Vec3::new(-16.0, -16.0, -36.0), Vec3::new(16.0, 16.0, 36.0));
        eng.set_origin(player, Vec3::new(0.0, 0.0, 68.0));
        (p, player)
    });
    h.step(&mut [], 3);
    let moved = vars(&h, p).origin;
    assert!(moved.x > 2.0, "{moved:?}");
    let pv = vars(&h, player);
    assert!(near(pv.origin, moved + Vec3::new(0.0, 0.0, 68.0), 1e-3), "{:?}", pv.origin);
    // A walking entity keeps `FL_ONGROUND`: the player's own movement owns it.
    assert_ne!(pv.flags & FL_ONGROUND, 0);
    assert_eq!(pv.fixangle, 0);
    assert!(h.server.game().blocked_log.is_empty());
}

#[test]
fn a_rotating_pusher_turns_the_players_view() {
    let mut h = harness(TestGame::default());
    let (p, player) = run(&mut h, |eng| {
        let p = pusher(eng, Vec3::ZERO, 32.0, Vec3::ZERO, 10.0);
        eng.ent_mut(p).unwrap().avelocity = Vec3::new(0.0, 90.0, 0.0);
        let player = eng.client_edict(0);
        let v = eng.ent_mut(player).unwrap();
        v.movetype = MOVETYPE_WALK;
        v.solid = SOLID_SLIDEBOX;
        v.flags = FL_CLIENT | FL_ONGROUND;
        v.groundentity = Some(p);
        eng.set_size(player, Vec3::new(-16.0, -16.0, -36.0), Vec3::new(16.0, 16.0, 36.0));
        eng.set_origin(player, Vec3::new(0.0, 0.0, 68.0));
        (p, player)
    });
    let _ = p;
    let pv = vars(&h, player);
    // A client is not turned; its view is told to follow (`fixangle` 2) and the yaw goes
    // into its `avelocity`.
    assert_eq!(pv.fixangle, 2);
    assert!(pv.avelocity.y > 0.0, "{:?}", pv.avelocity);
    assert_eq!(pv.angles.y, 0.0);
}

#[test]
fn a_conveyor_gives_base_velocity_that_is_folded_in_afterwards() {
    let mut h = harness(TestGame::default());
    let (belt, rider) = run(&mut h, |eng| {
        let belt = body(eng, "belt", Vec3::ZERO, 32.0);
        let v = eng.ent_mut(belt).unwrap();
        v.flags |= FL_CONVEYOR;
        v.speed = 100.0;
        v.movedir = Vec3::new(1.0, 0.0, 0.0);
        let rider = body(eng, "item", Vec3::new(0.0, 0.0, 50.0), 8.0);
        let v = eng.ent_mut(rider).unwrap();
        v.movetype = MOVETYPE_NONE;
        v.flags |= FL_ONGROUND;
        v.groundentity = Some(belt);
        (belt, rider)
    });
    // On the belt: the belt's speed is its base velocity, which is not folded in yet.
    let v = vars(&h, rider);
    assert_eq!(v.basevelocity, Vec3::new(100.0, 0.0, 0.0));
    assert_eq!(v.velocity, Vec3::ZERO);

    // Off the belt the next frame folds it into the velocity, with the half-frame correction.
    h.server.game_mut().act(move |eng| {
        eng.ent_mut(rider).unwrap().flags &= !FL_ONGROUND;
    });
    h.step(&mut [], 1);
    let v = vars(&h, rider);
    assert_eq!(v.basevelocity, Vec3::ZERO);
    assert!((v.velocity.x - 100.0 * (0.01 * 0.5 + 1.0)).abs() < 1e-3, "{:?}", v.velocity);
    let _ = belt;
}

/// de_dust2 loaded twice (the server consumes one), or `None` without the game.
fn dust2() -> Option<(ServerMap, Arc<BspMap>)> {
    let vfs = Vfs::discover(None, VfsConfig::default()).ok()?;
    let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("de_dust2 reads");
    let bsp = Arc::new(BspMap::load(&vfs, "de_dust2").expect("de_dust2 parses"));
    let map = ServerMap::from_bsp("de_dust2", &raw, bsp.clone()).expect("valid BSP header");
    Some((map, bsp))
}

/// A brush model of de_dust2 as a pusher carries a player standing on it, and the player is
/// not carried by a pusher it is not on. The brush is the one the map gives to a `func_*`
/// entity, moved up and sideways by the pusher's `velocity`.
#[test]
fn brush_pusher_of_a_real_map_carries_its_rider() {
    let Some((map, bsp)) = dust2() else {
        return;
    };
    // The largest brush model: the one a rider is most likely to stand on.
    let (model, extent) = bsp
        .models
        .iter()
        .enumerate()
        .skip(1)
        .map(|(i, m)| (i, m.maxs - m.mins))
        .max_by(|a, b| (a.1.x * a.1.y).total_cmp(&(b.1.x * b.1.y)))
        .expect("de_dust2 has brush entities");
    let top = bsp.models[model].maxs.z;
    let center = (bsp.models[model].mins + bsp.models[model].maxs) * 0.5;
    assert!(extent.x > 32.0 && extent.y > 32.0, "{extent:?}");

    let cfg = ServerConfig {
        max_clients: 1,
        max_edicts: 256,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_map(cfg, map, TestGame::default());
    let name = format!("*{model}");
    let (p, player) = run(&mut h, move |eng| {
        let p = eng.create_entity("func_test").expect("free edict");
        assert!(eng.set_model(p, &name), "{name} is a model of the map");
        let v = eng.ent_mut(p).unwrap();
        v.solid = SOLID_BSP;
        v.movetype = MOVETYPE_PUSH;
        // Straight up, so the world beside the brush cannot be in the way of the rider.
        v.velocity = Vec3::new(0.0, 0.0, 100.0);
        v.nextthink = 1.0;
        eng.set_origin(p, Vec3::ZERO);

        let player = eng.client_edict(0);
        let v = eng.ent_mut(player).unwrap();
        v.movetype = MOVETYPE_WALK;
        v.solid = SOLID_SLIDEBOX;
        v.flags = FL_CLIENT | FL_ONGROUND;
        v.groundentity = Some(p);
        eng.set_size(player, Vec3::new(-16.0, -16.0, -36.0), Vec3::new(16.0, 16.0, 36.0));
        eng.set_origin(player, Vec3::new(center.x, center.y, top + 36.0 + 1.0));
        (p, player)
    });
    h.step(&mut [], 2);
    let pv = vars(&h, p);
    let player_v = vars(&h, player);
    assert!(pv.origin.z > 1.0, "the brush rose: {:?}", pv.origin);
    assert_eq!(pv.origin.x, 0.0);
    // Carried by the full distance the brush moved, while it was not blocked.
    let blocked = h.server.game().blocked_log.clone();
    assert!(blocked.is_empty(), "{blocked:?}");
    assert!(
        (player_v.origin.z - (top + 37.0 + pv.origin.z)).abs() < 1e-3,
        "{:?} after the brush rose {}",
        player_v.origin,
        pv.origin.z
    );
}
