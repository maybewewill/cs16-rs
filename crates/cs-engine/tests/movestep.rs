//! Entity walking (`sv_move.cpp`): `walk_move`, `drop_to_floor` and `move_to_origin`.
//!
//! The stub-map tests build floors, steps and walls from box entities. A floor must be a
//! `MOVETYPE_PUSHSTEP` box: `SV_CheckBottom` traces with `MOVE_NOMONSTERS`, which ignores every
//! entity that is not a brush model or a pushable. The de_dust2 tests return early when the
//! game is not installed; their spots are named in the comments.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use common::{Harness, TestGame};
use cs_assets::bsp::BspMap;
use cs_engine::consts::{
    FL_FLY, FL_MONSTERCLIP, FL_ONGROUND, FL_PARTIALGROUND, FL_SWIM, MOVETYPE_PUSH,
    MOVETYPE_PUSHSTEP, SOLID_BBOX, SOLID_BSP, SOLID_SLIDEBOX,
};
use cs_engine::{EdictId, EngineApi, MOVE_NORMAL, ServerConfig, ServerMap, WalkMoveMode};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

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

/// A solid box (a pushable, so that floor searches that skip monsters still see it) spanning
/// `mins..maxs` in world coordinates.
fn slab(eng: &mut EngineApi<'_>, mins: Vec3, maxs: Vec3) -> EdictId {
    let id = eng.create_entity("test_slab").expect("free edict");
    let v = eng.ent_mut(id).unwrap();
    v.solid = SOLID_BBOX;
    v.movetype = MOVETYPE_PUSHSTEP;
    eng.set_size(id, mins, maxs);
    eng.set_origin(id, Vec3::ZERO);
    id
}

/// A standing-size monster at `origin`.
fn walker(eng: &mut EngineApi<'_>, origin: Vec3, flags: u32) -> EdictId {
    let id = eng.create_entity("test_walker").expect("free edict");
    let v = eng.ent_mut(id).unwrap();
    v.solid = SOLID_SLIDEBOX;
    v.flags = flags;
    eng.set_size(
        id,
        Vec3::new(-16.0, -16.0, -36.0),
        Vec3::new(16.0, 16.0, 36.0),
    );
    eng.set_origin(id, origin);
    id
}

fn origin(eng: &EngineApi<'_>, id: EdictId) -> Vec3 {
    eng.ent(id).unwrap().origin
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.1
}

fn stub_harness() -> Harness {
    let cfg = ServerConfig {
        max_clients: 2,
        max_edicts: 64,
        ..ServerConfig::default()
    };
    Harness::with_config(cfg, TestGame::default())
}

/// The map and the harness on de_dust2, `None` without the game.
fn dust2() -> Option<Harness> {
    let vfs = Vfs::discover(None, VfsConfig::default()).ok()?;
    let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("de_dust2 reads");
    let bsp = Arc::new(BspMap::load(&vfs, "de_dust2").expect("de_dust2 parses"));
    let map = ServerMap::from_bsp("de_dust2", &raw, bsp).expect("valid BSP header");
    let cfg = ServerConfig {
        max_clients: 2,
        ..ServerConfig::default()
    };
    Some(Harness::with_map(cfg, map, TestGame::default()))
}

#[test]
fn walk_move_steps_up_a_stair() {
    let mut h = stub_harness();
    let (low, high) = run(&mut h, |eng| {
        // A floor with its surface at z = 0, and a 16 high step from x = 50.
        slab(
            eng,
            Vec3::new(-200.0, -200.0, -16.0),
            Vec3::new(400.0, 400.0, 0.0),
        );
        slab(
            eng,
            Vec3::new(50.0, -200.0, -16.0),
            Vec3::new(400.0, 200.0, 16.0),
        );
        let w = walker(eng, Vec3::new(0.0, 0.0, 36.0), FL_ONGROUND);
        assert!(eng.walk_move(w, 0.0, 20.0, WalkMoveMode::Normal));
        assert!(near(origin(eng, w).x, 20.0) && near(origin(eng, w).z, 36.0));
        // The box now reaches x = 56 > 50 only after the second move: it climbs the step.
        assert!(eng.walk_move(w, 0.0, 20.0, WalkMoveMode::Normal));
        let low = origin(eng, w);

        // A 30 high step is more than sv_stepsize (18): refused, nothing moves.
        slab(
            eng,
            Vec3::new(200.0, 300.0, -16.0),
            Vec3::new(400.0, 400.0, 30.0),
        );
        let w2 = walker(eng, Vec3::new(100.0, 350.0, 36.0), FL_ONGROUND);
        let mut last = origin(eng, w2);
        while eng.walk_move(w2, 0.0, 20.0, WalkMoveMode::Normal) {
            last = origin(eng, w2);
        }
        assert_eq!(origin(eng, w2), last, "a refused step changes nothing");
        (low, last)
    });
    assert!(
        near(low.x, 40.0) && near(low.z, 52.0),
        "on the step: {low:?}"
    );
    assert!(
        high.x < 200.0 - 16.0 + 0.1 && near(high.z, 36.0),
        "stopped before the tall step: {high:?}"
    );
}

#[test]
fn walk_move_steps_up_a_stair_on_dust2() {
    let Some(mut h) = dust2() else {
        return;
    };
    run(&mut h, |eng| {
        // A 16 unit step on de_dust2 (found by scanning): standing at (-2016, 2400, 36.03125)
        // on the lower tread, walking east 16 ends at (-2000, 2400, 52.03125), and walking
        // back west from the upper tread goes down again.
        let w = walker(eng, Vec3::new(-2016.0, 2400.0, 36.03125), FL_ONGROUND);
        assert!(eng.walk_move(w, 0.0, 16.0, WalkMoveMode::Normal));
        let up = origin(eng, w);
        assert!(
            near(up.x, -2000.0) && near(up.y, 2400.0) && near(up.z, 52.03125),
            "{up:?}"
        );
        assert!(eng.walk_move(w, 180.0, 16.0, WalkMoveMode::Normal));
        assert!(near(origin(eng, w).z, 36.03125));
    });
}

#[test]
fn walk_move_refuses_to_walk_off_a_ledge() {
    let mut h = stub_harness();
    run(&mut h, |eng| {
        // A floor that ends at x = 100 with nothing under it.
        slab(
            eng,
            Vec3::new(-200.0, -200.0, -16.0),
            Vec3::new(100.0, 200.0, 0.0),
        );
        let w = walker(eng, Vec3::new(0.0, 0.0, 36.0), FL_ONGROUND);
        let mut steps = 0;
        while eng.walk_move(w, 0.0, 24.0, WalkMoveMode::Normal) {
            steps += 1;
            assert!(steps < 20, "the walker must stop at the ledge");
        }
        // At x = 72 the box reaches 88; the next step puts a corner past the edge.
        assert_eq!(steps, 3);
        assert!(near(origin(eng, w).x, 72.0));
        let before = origin(eng, w);
        assert!(!eng.walk_move(w, 0.0, 24.0, WalkMoveMode::Normal));
        assert_eq!(origin(eng, w), before);

        // A flier is not held back by the edge, and neither is a monster whose floor was
        // pulled out from under it (`FL_PARTIALGROUND`), which also loses `FL_ONGROUND`.
        eng.ent_mut(w).unwrap().flags |= FL_FLY;
        assert!(eng.walk_move(w, 0.0, 24.0, WalkMoveMode::Normal));
        // With the floor mostly gone the corner check is waived and the move goes ahead; with
        // the monster's middle off the floor as well it falls (loses `FL_ONGROUND`).
        eng.ent_mut(w).unwrap().flags = FL_ONGROUND | FL_PARTIALGROUND;
        eng.set_origin(w, before);
        assert!(eng.walk_move(w, 0.0, 24.0, WalkMoveMode::Normal));
        assert_ne!(eng.ent(w).unwrap().flags & FL_ONGROUND, 0);
        eng.set_origin(w, Vec3::new(140.0, 0.0, 36.0));
        assert!(eng.walk_move(w, 0.0, 24.0, WalkMoveMode::Normal));
        assert_eq!(eng.ent(w).unwrap().flags & FL_ONGROUND, 0);
    });
}

#[test]
fn walk_move_refuses_to_walk_off_a_ledge_on_dust2() {
    let Some(mut h) = dust2() else {
        return;
    };
    run(&mut h, |eng| {
        // A small platform south of the T spawn: standing at (-832, -656, 196.03125), every
        // step of 16 puts a corner over the edge (the floor beyond is 32 lower, z 164.03125),
        // so the walk is refused in all four directions and nothing moves.
        let w = walker(eng, Vec3::new(-832.0, -656.0, 196.0), 0);
        assert_eq!(eng.drop_to_floor(w), 1);
        let platform = origin(eng, w);
        assert!(near(platform.z, 196.03125), "{platform:?}");
        for yaw in [0.0, 90.0, 180.0, 270.0] {
            assert!(!eng.walk_move(w, yaw, 16.0, WalkMoveMode::Normal), "{yaw}");
            assert_eq!(origin(eng, w), platform);
        }
        // At the T spawn (-832, -704) the floor is that low one, and there it walks freely.
        let low = walker(eng, Vec3::new(-832.0, -704.0, 176.0), 0);
        assert_eq!(eng.drop_to_floor(low), 1);
        assert!(
            near(origin(eng, low).z, 164.03125),
            "{:?}",
            origin(eng, low)
        );
        assert!(eng.walk_move(low, 0.0, 16.0, WalkMoveMode::Normal));
        assert!(near(origin(eng, low).x, -816.0));
    });
}

#[test]
fn walk_move_modes() {
    let mut h = stub_harness();
    run(&mut h, |eng| {
        slab(
            eng,
            Vec3::new(-200.0, -200.0, -16.0),
            Vec3::new(400.0, 200.0, 0.0),
        );
        let w = walker(eng, Vec3::new(0.0, 0.0, 36.0), FL_ONGROUND);
        let linked = eng.ent(w).unwrap().absmin;

        // Normal moves and links.
        assert!(eng.walk_move(w, 0.0, 10.0, WalkMoveMode::Normal));
        assert!(near(origin(eng, w).x, 10.0));
        assert!(near(eng.ent(w).unwrap().absmin.x, linked.x + 10.0));

        // CheckOnly moves the origin as the source does but leaves the links alone.
        let linked = eng.ent(w).unwrap().absmin;
        assert!(eng.walk_move(w, 0.0, 10.0, WalkMoveMode::CheckOnly));
        assert!(near(origin(eng, w).x, 20.0));
        assert_eq!(eng.ent(w).unwrap().absmin, linked);

        // WorldOnly searches the world model alone: this stub map has none, so the entity
        // floor is not there and the step is off an edge.
        let before = origin(eng, w);
        assert!(!eng.walk_move(w, 0.0, 10.0, WalkMoveMode::WorldOnly));
        assert_eq!(origin(eng, w), before);

        // Without ground, flight or swimming nothing walks.
        eng.ent_mut(w).unwrap().flags = 0;
        assert!(!eng.walk_move(w, 0.0, 10.0, WalkMoveMode::Normal));
        assert_eq!(origin(eng, w), before);

        // Hostile numbers are refused.
        eng.ent_mut(w).unwrap().flags = FL_ONGROUND;
        assert!(!eng.walk_move(w, f32::NAN, 10.0, WalkMoveMode::Normal));
        assert!(!eng.walk_move(w, 0.0, f32::INFINITY, WalkMoveMode::Normal));
        assert!(!eng.walk_move(w, 0.0, 1.0e9, WalkMoveMode::Normal));
        assert_eq!(origin(eng, w), before);
    });
}

#[test]
fn fliers_and_swimmers_walk_through_the_air_or_not() {
    let mut h = stub_harness();
    run(&mut h, |eng| {
        // A flier moves in the open and is stopped by a wall; the wall is solid, the move
        // stops short without moving at all (the source's move is all or nothing).
        slab(
            eng,
            Vec3::new(60.0, -200.0, -100.0),
            Vec3::new(80.0, 200.0, 100.0),
        );
        let f = walker(eng, Vec3::new(0.0, 0.0, 50.0), FL_FLY);
        assert!(eng.walk_move(f, 0.0, 20.0, WalkMoveMode::Normal));
        assert!(near(origin(eng, f).x, 20.0));
        assert!(!eng.walk_move(f, 0.0, 100.0, WalkMoveMode::Normal));
        assert!(near(origin(eng, f).x, 20.0));

        // Nothing here is water, so a swimmer that would leave "water" is refused.
        let s = walker(eng, Vec3::new(0.0, 100.0, 50.0), FL_SWIM);
        assert!(!eng.walk_move(s, 90.0, 10.0, WalkMoveMode::Normal));
        assert!(near(origin(eng, s).y, 100.0));
    });
}

#[test]
fn drop_to_floor_results() {
    let mut h = stub_harness();
    run(&mut h, |eng| {
        // Above a floor: lands on it.
        let floor = slab(
            eng,
            Vec3::new(-100.0, -100.0, -1000.0),
            Vec3::new(100.0, 100.0, 0.0),
        );
        let w = walker(eng, Vec3::new(0.0, 0.0, 200.0), 0);
        assert_eq!(eng.drop_to_floor(w), 1);
        assert!(near(origin(eng, w).z, 36.0), "{:?}", origin(eng, w));
        let v = eng.ent(w).unwrap();
        assert_ne!(v.flags & FL_ONGROUND, 0);
        assert_eq!(v.groundentity, Some(floor));

        // Over the void: no floor within 256 units, nothing changes.
        let a = walker(eng, Vec3::new(500.0, 0.0, 200.0), 0);
        assert_eq!(eng.drop_to_floor(a), 0);
        assert_eq!(origin(eng, a), Vec3::new(500.0, 0.0, 200.0));
        assert_eq!(eng.ent(a).unwrap().flags & FL_ONGROUND, 0);

        // Inside the thick floor: stuck.
        let s = walker(eng, Vec3::new(0.0, 0.0, -500.0), 0);
        assert_eq!(eng.drop_to_floor(s), -1);
        assert_eq!(origin(eng, s), Vec3::new(0.0, 0.0, -500.0));

        // A stale handle and a hostile origin are quiet.
        eng.remove_entity(a);
        assert_eq!(eng.drop_to_floor(a), 0);
        let n = walker(eng, Vec3::ZERO, 0);
        eng.ent_mut(n).unwrap().origin = Vec3::splat(f32::NAN);
        assert_eq!(eng.drop_to_floor(n), 0);
    });
}

#[test]
fn drop_to_floor_results_on_dust2() {
    let Some(mut h) = dust2() else {
        return;
    };
    run(&mut h, |eng| {
        // 40 units above the T spawn (-832, -704, 176): lands on the floor.
        let w = walker(eng, Vec3::new(-832.0, -704.0, 216.0), 0);
        assert_eq!(eng.drop_to_floor(w), 1);
        let landed = origin(eng, w);
        assert!(landed.z < 216.0 && landed.z > 160.0, "{landed:?}");
        let v = eng.ent(w).unwrap();
        assert_ne!(v.flags & FL_ONGROUND, 0);
        assert_eq!(v.groundentity, Some(eng.world()));

        // Inside the rock at (88, 2296, -68) (a wall near the CT spawn): stuck.
        let s = walker(eng, Vec3::new(88.0, 2296.0, -68.0), 0);
        assert_eq!(eng.drop_to_floor(s), -1);
        assert_eq!(origin(eng, s), Vec3::new(88.0, 2296.0, -68.0));
    });
}

#[test]
fn move_to_origin_approaches_the_goal() {
    fn trajectory() -> Vec<Vec3> {
        let mut h = stub_harness();
        run(&mut h, |eng| {
            slab(
                eng,
                Vec3::new(-300.0, -300.0, -16.0),
                Vec3::new(500.0, 400.0, 0.0),
            );
            // A wall in the way east of the walker; the goal is beyond it, to the north east.
            slab(
                eng,
                Vec3::new(20.0, -300.0, 0.0),
                Vec3::new(60.0, 100.0, 200.0),
            );
            let w = walker(eng, Vec3::new(0.0, 0.0, 36.0), FL_ONGROUND);
            let goal = Vec3::new(200.0, 250.0, 36.0);
            let start = origin(eng, w).distance(goal);
            let mut path = Vec::new();
            for _ in 0..40 {
                eng.move_to_origin(w, goal, 10.0, MOVE_NORMAL);
                path.push(origin(eng, w));
            }
            let end = *path.last().unwrap();
            assert!(end.distance(goal) < start - 100.0, "{end:?}");
            // The wall box spans x 20..60 for y < 100: the walker (half width 16) stays out.
            for p in &path {
                assert!(
                    !(p.x + 16.0 > 20.0 && p.x - 16.0 < 60.0 && p.y - 16.0 < 100.0),
                    "{p:?}"
                );
            }
            path
        })
    }
    // Same engine, same random numbers: the walk is reproducible.
    assert_eq!(trajectory(), trajectory());

    // Any other move type heads straight for the goal and keeps to the floor plane.
    let mut h = stub_harness();
    run(&mut h, |eng| {
        slab(
            eng,
            Vec3::new(-300.0, -300.0, -16.0),
            Vec3::new(500.0, 400.0, 0.0),
        );
        let w = walker(eng, Vec3::new(0.0, 0.0, 36.0), FL_ONGROUND);
        let goal = Vec3::new(30.0, 40.0, 500.0);
        eng.move_to_origin(w, goal, 10.0, 1);
        let p = origin(eng, w);
        assert!(near(p.x, 6.0) && near(p.y, 8.0) && near(p.z, 36.0), "{p:?}");

        // Off the ground, without flight or swimming, nothing happens.
        eng.ent_mut(w).unwrap().flags = 0;
        eng.move_to_origin(w, goal, 10.0, 1);
        eng.move_to_origin(w, goal, 10.0, MOVE_NORMAL);
        assert_eq!(origin(eng, w), p);

        // A flier follows the vertical part as well.
        eng.ent_mut(w).unwrap().flags = FL_FLY;
        eng.move_to_origin(w, Vec3::new(p.x, p.y, 400.0), 10.0, 1);
        assert!(near(origin(eng, w).z, 46.0));
    });
}

#[test]
fn stuck_monster_gets_partial_ground_flag() {
    let mut h = stub_harness();
    run(&mut h, |eng| {
        // The floor is a small island; walls all round, so no direction works. The floor
        // under the monster is mostly gone (only the middle is left): `SV_CheckBottom` fails
        // and `SV_NewChaseDir2` marks it `FL_PARTIALGROUND`.
        slab(eng, Vec3::new(-8.0, -8.0, -16.0), Vec3::new(8.0, 8.0, 0.0));
        for (mins, maxs) in [
            (Vec3::new(-60.0, -60.0, 0.0), Vec3::new(-20.0, 60.0, 200.0)),
            (Vec3::new(20.0, -60.0, 0.0), Vec3::new(60.0, 60.0, 200.0)),
            (Vec3::new(-60.0, -60.0, 0.0), Vec3::new(60.0, -20.0, 200.0)),
            (Vec3::new(-60.0, 20.0, 0.0), Vec3::new(60.0, 60.0, 200.0)),
        ] {
            slab(eng, mins, maxs);
        }
        let w = walker(eng, Vec3::new(0.0, 0.0, 36.0), FL_ONGROUND);
        let before = origin(eng, w);
        eng.move_to_origin(w, Vec3::new(200.0, 200.0, 36.0), 10.0, MOVE_NORMAL);
        assert_eq!(origin(eng, w), before);
        assert_ne!(eng.ent(w).unwrap().flags & FL_PARTIALGROUND, 0);
    });
}

/// A flier in open air at (0, 0, 100), and a non-solid entity at `enemy_z` it has as `enemy`.
fn flier_with_enemy(eng: &mut EngineApi<'_>, enemy_z: f32) -> (EdictId, EdictId) {
    let f = walker(eng, Vec3::new(0.0, 0.0, 100.0), FL_FLY);
    let e = walker(eng, Vec3::new(500.0, 0.0, enemy_z), 0);
    eng.ent_mut(e).unwrap().solid = 0;
    eng.ent_mut(f).unwrap().enemy = Some(e);
    (f, e)
}

#[test]
fn flier_follows_the_enemy_height_and_retries_flat() {
    let mut h = stub_harness();
    run(&mut h, |eng| {
        // The enemy far below (dz 100 > 40): the move sinks 8.
        let (f, _) = flier_with_enemy(eng, 0.0);
        assert!(eng.walk_move(f, 0.0, 10.0, WalkMoveMode::Normal));
        let p = origin(eng, f);
        assert!(near(p.x, 10.0) && near(p.z, 92.0), "{p:?}");

        // The enemy far above (dz < 30): the move rises 8.
        let (f, _) = flier_with_enemy(eng, 200.0);
        assert!(eng.walk_move(f, 0.0, 10.0, WalkMoveMode::Normal));
        assert!(near(origin(eng, f).z, 108.0), "{:?}", origin(eng, f));

        // Between 30 and 40 below: level flight.
        let (f, _) = flier_with_enemy(eng, 65.0);
        assert!(eng.walk_move(f, 0.0, 10.0, WalkMoveMode::Normal));
        assert!(near(origin(eng, f).z, 100.0));

        // A stale enemy counts as none: no height change.
        let (f, e) = flier_with_enemy(eng, 0.0);
        eng.remove_entity(e);
        assert!(eng.walk_move(f, 0.0, 10.0, WalkMoveMode::Normal));
        assert!(near(origin(eng, f).z, 100.0));

        // A ceiling just above blocks the rising move; the second pass flies level.
        slab(eng, Vec3::new(-300.0, -300.0, 140.0), Vec3::new(300.0, 300.0, 200.0));
        let (f, _) = flier_with_enemy(eng, 400.0);
        eng.set_origin(f, Vec3::new(0.0, 100.0, 100.0));
        assert!(eng.walk_move(f, 0.0, 10.0, WalkMoveMode::Normal));
        let p = origin(eng, f);
        assert!(near(p.x, 10.0) && near(p.z, 100.0), "{p:?}");
    });
}

#[test]
fn hostile_numbers_never_reach_a_flier() {
    let mut h = stub_harness();
    run(&mut h, |eng| {
        let f = walker(eng, Vec3::new(0.0, 0.0, 100.0), FL_FLY);
        let before = origin(eng, f);
        let linked = eng.ent(f).unwrap().absmin;
        assert!(!eng.walk_move(f, 0.0, 1.0e9, WalkMoveMode::Normal));
        assert!(!eng.walk_move(f, 0.0, 1.0e9, WalkMoveMode::CheckOnly));
        assert_eq!(origin(eng, f), before);
        assert_eq!(eng.ent(f).unwrap().absmin, linked);

        // The height adjustment (+8 for an enemy above) can carry the move over the edge of
        // the world: refused, nothing moves.
        let (edge, _) = flier_with_enemy(eng, 131_071.0);
        eng.set_origin(edge, Vec3::new(0.0, 0.0, 131_070.0));
        let at_edge = origin(eng, edge);
        assert_eq!(at_edge.z, 131_070.0);
        assert!(!eng.walk_move(edge, 0.0, 1.0, WalkMoveMode::Normal));
        assert_eq!(origin(eng, edge), at_edge);

        // A huge step through `move_to_origin` goes nowhere either.
        eng.move_to_origin(f, Vec3::new(100.0, 0.0, 100.0), 1.0e9, MOVE_NORMAL);
        eng.move_to_origin(f, Vec3::new(100.0, 0.0, 100.0), 1.0e9, 1);
        assert_eq!(origin(eng, f), before);

        // A NaN `ideal_yaw` is no success and writes nothing that is not finite.
        for flags in [FL_FLY, FL_ONGROUND] {
            slab(eng, Vec3::new(-300.0, -300.0, -16.0), Vec3::new(500.0, 400.0, 0.0));
            let w = walker(eng, Vec3::new(0.0, 200.0, 36.0), flags);
            eng.ent_mut(w).unwrap().ideal_yaw = f32::NAN;
            eng.move_to_origin(w, Vec3::new(100.0, 200.0, 36.0), 10.0, MOVE_NORMAL);
            let v = eng.ent(w).unwrap();
            assert!(v.origin.is_finite() && v.absmin.is_finite() && v.absmax.is_finite());
        }
    });
}

#[test]
fn monster_clip_brushes_stop_only_monsters_that_ask_for_it() {
    let Some(mut h) = dust2() else {
        return;
    };
    run(&mut h, |eng| {
        // The T spawn floor (-832, -704, 164.03125), where walking east 16 works. Then brush
        // model *1 with `FL_MONSTERCLIP` is put over the step's destination.
        let w = walker(eng, Vec3::new(-832.0, -704.0, 176.0), 0);
        assert_eq!(eng.drop_to_floor(w), 1);
        let p = origin(eng, w);
        let wall = eng.create_entity("func_wall").expect("free edict");
        assert!(eng.set_model(wall, "*1"));
        let v = eng.ent_mut(wall).unwrap();
        v.solid = SOLID_BSP;
        v.movetype = MOVETYPE_PUSH;
        v.flags = FL_MONSTERCLIP;
        let mid = (v.mins + v.maxs) * 0.5;
        eng.set_origin(wall, p + Vec3::new(16.0, 0.0, 0.0) - mid);

        // An ordinary monster walks through it.
        eng.ent_mut(w).unwrap().flags = FL_ONGROUND;
        assert!(eng.walk_move(w, 0.0, 16.0, WalkMoveMode::Normal));

        // One with `FL_MONSTERCLIP` is stopped by it and stays put.
        eng.set_origin(w, p);
        eng.ent_mut(w).unwrap().flags = FL_ONGROUND | FL_MONSTERCLIP;
        assert!(!eng.walk_move(w, 0.0, 16.0, WalkMoveMode::Normal));
        assert_eq!(origin(eng, w), p);
    });
}
