//! Toss, bounce, fly and step physics (`SV_Physics_Toss`, `SV_Physics_Bounce`,
//! `SV_Physics_Step` and what they call). Tests that need a real map return early when the game
//! is not installed; the others run on the stub map with box entities for floors.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use common::{Harness, TestGame};
use cs_assets::bsp::BspMap;
use cs_engine::collision::{CONTENTS_EMPTY, CONTENTS_WATER, Hull};
use cs_engine::consts::{
    FL_ONGROUND, MOVETYPE_BOUNCE, MOVETYPE_BOUNCEMISSILE, MOVETYPE_FLY, MOVETYPE_PUSH,
    MOVETYPE_STEP, MOVETYPE_TOSS, SOLID_BBOX, SOLID_BSP, SOLID_NOT, SOLID_SLIDEBOX,
};
use cs_engine::{
    EdictId, EngineApi, EntVars, MoveFilter, PrecacheError, SV_TIME_AT_MAP_START, ServerConfig,
    ServerMap,
};
use cs_vfs::{Vfs, VfsConfig};
use glam::Vec3;

const WORLD: EdictId = EdictId {
    index: 0,
    serial: 0,
};

fn stub() -> Harness {
    let cfg = ServerConfig {
        max_clients: 1,
        max_edicts: 64,
        ..ServerConfig::default()
    };
    Harness::with_config(cfg, TestGame::default())
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

fn vars(h: &Harness, id: EdictId) -> EntVars {
    h.server.edicts().get(id).expect("live").vars.clone()
}

fn near(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance
}

/// A solid floor whose top face is at z = 0, 400 by 400 units.
fn floor(eng: &mut EngineApi<'_>) -> EdictId {
    let id = eng.create_entity("test_floor").expect("free edict");
    eng.ent_mut(id).unwrap().solid = SOLID_SLIDEBOX;
    eng.set_size(id, Vec3::new(-200.0, -200.0, -5.0), Vec3::new(200.0, 200.0, 5.0));
    eng.set_origin(id, Vec3::new(0.0, 0.0, -5.0));
    id
}

/// A small box entity of movetype `movetype` at `origin`, moving at `velocity`.
fn mover(
    eng: &mut EngineApi<'_>,
    movetype: u8,
    solid: u8,
    origin: Vec3,
    velocity: Vec3,
) -> EdictId {
    let id = eng.create_entity("test_mover").expect("free edict");
    let v = eng.ent_mut(id).unwrap();
    v.movetype = movetype;
    v.solid = solid;
    v.velocity = velocity;
    eng.set_size(id, Vec3::splat(-4.0), Vec3::splat(4.0));
    eng.set_origin(id, origin);
    id
}

fn dust2() -> Option<(Harness, ServerMap)> {
    let vfs = Vfs::discover(None, VfsConfig::default()).ok()?;
    let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("de_dust2 reads");
    let bsp = Arc::new(BspMap::load(&vfs, "de_dust2").expect("de_dust2 parses"));
    let make = || ServerMap::from_bsp("de_dust2", &raw, bsp.clone()).expect("valid BSP header");
    let cfg = ServerConfig {
        max_clients: 1,
        ..ServerConfig::default()
    };
    Some((Harness::with_map(cfg, make(), TestGame::default()), make()))
}

/// A spot of de_dust2 with open air above it and a floor below: the CT spawn point.
fn spawn_point(map: &ServerMap) -> Vec3 {
    map.entities
        .iter()
        .filter(|e| e.classname() == "info_player_start")
        .find_map(|e| e.get_vec3("origin"))
        .expect("de_dust2 has a CT spawn")
}

#[test]
fn world_is_a_pushing_brush_entity() {
    let h = stub();
    let world = vars(&h, WORLD);
    assert_eq!(world.solid, SOLID_BSP);
    assert_eq!(world.movetype, MOVETYPE_PUSH);
}

#[test]
fn toss_falls_and_lands() {
    let Some((mut h, map)) = dust2() else {
        return;
    };
    let spawn = spawn_point(&map);
    // The floor under the spawn and the room above it: fall from up to 200 units.
    let (floor_z, height) = run(&mut h, move |eng| {
        let down = eng.trace_line(spawn, spawn - Vec3::Z * 2000.0, MoveFilter::Normal, None);
        let up = eng.trace_line(spawn, spawn + Vec3::Z * 400.0, MoveFilter::Normal, None);
        let floor_z = down.trace.end_pos.z;
        let room = up.trace.end_pos.z - floor_z;
        (floor_z, (room - 8.0).min(200.0))
    });
    assert!(height >= 100.0, "{height} units of room above the spawn");
    let start = Vec3::new(spawn.x, spawn.y, floor_z + height);
    let id = run(&mut h, move |eng| {
        mover(eng, MOVETYPE_TOSS, SOLID_BBOX, start, Vec3::ZERO)
    });

    // 20 frames of free fall: 0.2 s at 800 units/s^2 is 16 units, not on the ground.
    h.step(&mut [], 19);
    let falling = vars(&h, id);
    assert!(near(falling.origin.z, start.z - 16.0, 1.5), "{:?}", falling.origin);
    assert_eq!(falling.flags & FL_ONGROUND, 0);

    // sqrt(2 * 200 / 800) = 0.71 s of fall; the rest is at rest.
    h.step(&mut [], 150);
    let landed = vars(&h, id);
    assert_ne!(landed.flags & FL_ONGROUND, 0);
    assert_eq!(landed.groundentity, Some(WORLD));
    assert_eq!(landed.velocity, Vec3::ZERO);
    assert!(near(landed.origin.z, floor_z, 0.1), "{landed:?} vs {floor_z}");
    assert!(near(landed.origin.x, start.x, 0.01) && near(landed.origin.y, start.y, 0.01));

    // The landing was an impact with the world: both sides were touched, once.
    let touches = &h.server.game().touch_log;
    assert_eq!(touches[..], [(id.index, 0), (0, id.index)], "{touches:?}");
}

#[test]
fn toss_on_a_box_floor_touches_both_and_stands_on_it() {
    let mut h = stub();
    let (floor_id, id) = run(&mut h, |eng| {
        let floor_id = floor(eng);
        let id = mover(
            eng,
            MOVETYPE_TOSS,
            SOLID_BBOX,
            Vec3::new(0.0, 0.0, 30.0),
            Vec3::ZERO,
        );
        (floor_id, id)
    });
    h.step(&mut [], 100);
    let v = vars(&h, id);
    assert_ne!(v.flags & FL_ONGROUND, 0);
    assert_eq!(v.groundentity, Some(floor_id));
    assert_eq!(v.velocity, Vec3::ZERO);
    assert!(near(v.origin.z, 4.0, 0.1), "{:?}", v.origin);
    // Landing queues SV_Impact(ent, floor): the entity's touch, then the floor's.
    assert_eq!(
        h.server.game().touch_log[..],
        [(id.index, floor_id.index), (floor_id.index, id.index)]
    );
    assert_eq!(h.server.pending_touch_count(), 0);
}

#[test]
fn bounce_reflects_with_overbounce() {
    let mut h = stub();
    // Two units over the floor, falling at 400 units/s. The frame adds 8 units/s of gravity
    // (408 in all), reaches the floor half way, and reflects by `2 - friction` = 1.5 for
    // `BOUNCE`: -408 + 1.5 * 408 = 204. `BOUNCEMISSILE` does not fall and reflects by 2.
    let (bounce, missile) = run(&mut h, |eng| {
        floor(eng);
        let bounce = mover(
            eng,
            MOVETYPE_BOUNCE,
            SOLID_BBOX,
            Vec3::new(-100.0, 0.0, 6.0),
            Vec3::new(0.0, 0.0, -400.0),
        );
        eng.ent_mut(bounce).unwrap().friction = 0.5;
        let missile = mover(
            eng,
            MOVETYPE_BOUNCEMISSILE,
            SOLID_BBOX,
            Vec3::new(100.0, 0.0, 6.0),
            Vec3::new(0.0, 0.0, -400.0),
        );
        (bounce, missile)
    });
    let b = vars(&h, bounce);
    assert!(near(b.velocity.z, 204.0, 0.01), "{:?}", b.velocity);
    assert_eq!(b.flags & FL_ONGROUND, 0, "thrown back up, not resting");
    let m = vars(&h, missile);
    assert!(near(m.velocity.z, 400.0, 0.01), "{:?}", m.velocity);
    // Both went up again for the rest of the frame.
    assert!(b.origin.z > 4.0 && m.origin.z > 4.0);

    // Slowly, a bounce comes to rest on the floor it hit.
    let mut h = stub();
    let (floor_id, slow) = run(&mut h, |eng| {
        let floor_id = floor(eng);
        let slow = mover(
            eng,
            MOVETYPE_BOUNCE,
            SOLID_BBOX,
            Vec3::new(0.0, 0.0, 4.2),
            Vec3::new(0.0, 0.0, -20.0),
        );
        eng.ent_mut(slow).unwrap().friction = 1.0;
        (floor_id, slow)
    });
    let s = vars(&h, slow);
    assert_ne!(s.flags & FL_ONGROUND, 0);
    assert_eq!(s.groundentity, Some(floor_id));
    assert_eq!(s.velocity, Vec3::ZERO);
}

#[test]
fn fly_ignores_gravity() {
    let mut h = stub();
    let (fly, toss) = run(&mut h, |eng| {
        let fly = mover(
            eng,
            MOVETYPE_FLY,
            SOLID_BBOX,
            Vec3::new(0.0, 0.0, 500.0),
            Vec3::new(100.0, 0.0, 0.0),
        );
        let toss = mover(
            eng,
            MOVETYPE_TOSS,
            SOLID_BBOX,
            Vec3::new(0.0, 100.0, 500.0),
            Vec3::new(100.0, 0.0, 0.0),
        );
        (fly, toss)
    });
    h.step(&mut [], 9);
    // 10 frames of 10 ms at 100 units/s: 10 units along x.
    let f = vars(&h, fly);
    assert!(near(f.origin.x, 10.0, 0.01), "{:?}", f.origin);
    assert_eq!(f.origin.z, 500.0);
    assert_eq!(f.velocity, Vec3::new(100.0, 0.0, 0.0));
    // The tossed one fell: 0.1 s of gravity is a velocity of -80 and a drop of about 4.
    let t = vars(&h, toss);
    assert!(near(t.velocity.z, -80.0, 0.01), "{:?}", t.velocity);
    assert!(t.origin.z < 497.0, "{:?}", t.origin);
}

#[test]
fn velocity_is_clamped_and_nan_zeroed() {
    let mut h = stub();
    let (fast, nan_origin) = run(&mut h, |eng| {
        let fast = mover(
            eng,
            MOVETYPE_FLY,
            SOLID_NOT,
            Vec3::ZERO,
            Vec3::new(1e9, f32::NAN, -1e9),
        );
        let nan_origin = mover(eng, MOVETYPE_FLY, SOLID_NOT, Vec3::ZERO, Vec3::ZERO);
        eng.ent_mut(nan_origin).unwrap().origin = Vec3::new(f32::NAN, 5.0, 7.0);
        (fast, nan_origin)
    });
    let v = vars(&h, fast);
    // sv_maxvelocity is 2000; the NaN component is 0.
    assert_eq!(v.velocity, Vec3::new(2000.0, 0.0, -2000.0));
    assert!(near(v.origin.x, 20.0, 0.01) && v.origin.y == 0.0 && near(v.origin.z, -20.0, 0.01));
    let n = vars(&h, nan_origin);
    assert_eq!(n.origin, Vec3::new(0.0, 5.0, 7.0));

    // An infinity is treated as a NaN, and a non-finite angular velocity never reaches the
    // angles.
    let (inf, spin) = run(&mut h, |eng| {
        let inf = mover(
            eng,
            MOVETYPE_FLY,
            SOLID_NOT,
            Vec3::ZERO,
            Vec3::new(f32::INFINITY, 0.0, 0.0),
        );
        let spin = mover(eng, MOVETYPE_FLY, SOLID_NOT, Vec3::ZERO, Vec3::ZERO);
        eng.ent_mut(spin).unwrap().avelocity = Vec3::new(f32::NAN, f32::INFINITY, 90.0);
        (inf, spin)
    });
    assert_eq!(vars(&h, inf).velocity, Vec3::ZERO);
    let s = vars(&h, spin);
    assert!(s.angles.is_finite() && s.avelocity.is_finite(), "{s:?}");

    // The cvar is read at the start of every physics pass.
    run(&mut h, move |eng| {
        eng.set_cvar("sv_maxvelocity", "500");
        eng.ent_mut(fast).unwrap().velocity = Vec3::new(-3000.0, 100.0, 0.0);
    });
    assert_eq!(vars(&h, fast).velocity, Vec3::new(-500.0, 100.0, 0.0));
}

#[test]
fn precache_is_refused_during_the_settle_passes() {
    // One slot: the settle passes run a think due 0.05 s after the map start.
    let game = TestGame {
        extra_entities: 1,
        extra_nextthink: SV_TIME_AT_MAP_START + 0.05,
        think_precaches: Some("models/settle.mdl".to_string()),
        ..TestGame::default()
    };
    let cfg = ServerConfig {
        max_clients: 1,
        max_edicts: 64,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_config(cfg, game);
    assert_eq!(
        h.server.game().think_precache_results,
        [Err(PrecacheError::TooLate)]
    );
    // Nothing was added, now or later.
    run(&mut h, |eng| assert_eq!(eng.model_index("models/settle.mdl"), None));
}

/// A water pool of `cs_militia` that has air above it and at least 30 units of water under the
/// surface: the point on its surface, found by walking up from the middle of a water leaf.
fn water_spot(eng: &EngineApi<'_>, bsp: &BspMap) -> Option<Vec3> {
    let contents = |p: Vec3| eng.point_contents_ents(p);
    for leaf in bsp.leafs.iter().filter(|l| l.contents == CONTENTS_WATER) {
        let mid = |i: usize| (f32::from(leaf.mins[i]) + f32::from(leaf.maxs[i])) * 0.5;
        let mut p = Vec3::new(mid(0), mid(1), mid(2));
        if contents(p) != CONTENTS_WATER {
            continue;
        }
        while contents(p + Vec3::Z) == CONTENTS_WATER && p.z < 4000.0 {
            p.z += 1.0;
        }
        let air = (1..=15).all(|i| contents(p + Vec3::Z * (8.0 * i as f32)) == CONTENTS_EMPTY);
        let deep = (1..=30).all(|i| contents(p - Vec3::Z * i as f32) == CONTENTS_WATER);
        if air && deep {
            return Some(p);
        }
    }
    None
}

/// Water on a real map: `cs_militia`, whose pool is made of water leaves. The levels at the
/// feet, the waist and the eyes, the crossing into the water (the vertical velocity is halved),
/// and the way out.
#[test]
fn water_level_and_transition() {
    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let raw = vfs.read_bytes("maps/cs_militia.bsp").expect("cs_militia reads");
    let bsp = Arc::new(BspMap::load(&vfs, "cs_militia").expect("cs_militia parses"));
    let map = ServerMap::from_bsp("cs_militia", &raw, bsp.clone()).expect("valid BSP header");
    let cfg = ServerConfig {
        max_clients: 1,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_map(cfg, map, TestGame::default());

    let probe_bsp = bsp.clone();
    let spot = run(&mut h, move |eng| water_spot(eng, &probe_bsp));
    let surface = spot.expect("cs_militia has a pool with air above and 30 units of water");

    // Boxes of 16 units with the view point 6 above the middle: level 1 with the feet in the
    // water only, 2 with the middle in it, 3 with the view point too.
    let levels = run(&mut h, move |eng| {
        [3.0, -2.0, -20.0].map(|dz| {
            let id = mover(
                eng,
                MOVETYPE_FLY,
                SOLID_NOT,
                surface + Vec3::new(0.0, 0.0, dz),
                Vec3::ZERO,
            );
            eng.set_size(id, Vec3::splat(-8.0), Vec3::splat(8.0));
            eng.ent_mut(id).unwrap().view_ofs = Vec3::new(0.0, 0.0, 6.0);
            eng.set_origin(id, surface + Vec3::new(0.0, 0.0, dz));
            id
        })
    });
    h.step(&mut [], 1);
    let seen: Vec<_> = levels
        .iter()
        .map(|&id| (vars(&h, id).waterlevel, vars(&h, id).watertype))
        .collect();
    assert_eq!(
        seen,
        [(1, CONTENTS_WATER), (2, CONTENTS_WATER), (3, CONTENTS_WATER)]
    );

    // A toss from 20 units above: the frame that enters the water halves the velocity.
    let tossed = run(&mut h, move |eng| {
        mover(
            eng,
            MOVETYPE_TOSS,
            SOLID_NOT,
            surface + Vec3::new(0.0, 0.0, 20.0),
            Vec3::ZERO,
        )
    });
    assert_eq!(vars(&h, tossed).watertype, CONTENTS_EMPTY);
    let mut before = vars(&h, tossed);
    let mut entered = None;
    for _ in 0..100 {
        h.step(&mut [], 1);
        let now = vars(&h, tossed);
        if now.watertype == CONTENTS_WATER {
            entered = Some((before.velocity.z, now.clone()));
            break;
        }
        before = now;
    }
    let (v_before, now) = entered.expect("the entity fell into the water");
    // One frame of gravity (800 * 0.01), then the crossing halves the result.
    assert!(
        near(now.velocity.z, 0.5 * (v_before - 8.0), 0.01),
        "{v_before} -> {}",
        now.velocity.z
    );
    assert!(now.waterlevel >= 1);

    // Out again: a flyer that starts deep and moves up 5 units a frame is dry within 20 frames.
    let rising = run(&mut h, move |eng| {
        mover(
            eng,
            MOVETYPE_FLY,
            SOLID_NOT,
            surface - Vec3::Z * 20.0,
            Vec3::new(0.0, 0.0, 500.0),
        )
    });
    assert_eq!(vars(&h, rising).watertype, CONTENTS_WATER);
    h.step(&mut [], 20);
    let out = vars(&h, rising);
    assert_eq!((out.watertype, out.waterlevel), (CONTENTS_EMPTY, 0));
}

#[test]
fn step_entity_falls_and_gets_friction() {
    let Some((mut h, map)) = dust2() else {
        return;
    };
    let spawn = spawn_point(&map);
    // A monster-sized box (the standing hull): where it rests on the floor under the spawn,
    // and how far it can fall from.
    let (rest_z, height) = run(&mut h, move |eng| {
        let down = eng.trace_hull_ents(
            spawn,
            spawn - Vec3::Z * 2000.0,
            Hull::Standing,
            MoveFilter::Normal,
            None,
        );
        let up = eng.trace_hull_ents(
            spawn,
            spawn + Vec3::Z * 400.0,
            Hull::Standing,
            MoveFilter::Normal,
            None,
        );
        let rest_z = down.trace.end_pos.z;
        (rest_z, (up.trace.end_pos.z - rest_z - 4.0).min(100.0))
    });
    assert!(height >= 50.0, "{height} units of room above the spawn");
    let start = Vec3::new(spawn.x, spawn.y, rest_z + height);
    let id = run(&mut h, move |eng| {
        let id = eng.create_entity("test_monster").expect("free edict");
        let v = eng.ent_mut(id).unwrap();
        v.movetype = MOVETYPE_STEP;
        v.solid = SOLID_SLIDEBOX;
        v.health = 100.0;
        v.friction = 1.0;
        eng.set_size(id, Vec3::new(-16.0, -16.0, -36.0), Vec3::new(16.0, 16.0, 36.0));
        eng.set_origin(id, start);
        id
    });

    // Falls: not held by the ground, gravity pulls it down.
    h.step(&mut [], 9);
    let falling = vars(&h, id);
    assert!(falling.origin.z < start.z - 1.0, "{:?}", falling.origin);
    assert!(falling.velocity.z < 0.0);
    assert_eq!(falling.flags & FL_ONGROUND, 0);

    // Lands on the world and stays there.
    h.step(&mut [], 100);
    let landed = vars(&h, id);
    assert_ne!(landed.flags & FL_ONGROUND, 0);
    assert_eq!(landed.groundentity, Some(WORLD));
    assert_eq!(landed.velocity, Vec3::ZERO);
    assert!(near(landed.origin.z, rest_z, 0.1), "{:?} vs {rest_z}", landed.origin);

    // Pushed along the floor at 120 units/s: the first frame loses `speed * 0.01 * 4` (the
    // friction is `sv_friction` 4 times its own 1), and it comes to a halt.
    run(&mut h, move |eng| {
        eng.ent_mut(id).unwrap().velocity = Vec3::new(120.0, 0.0, 0.0);
    });
    let mut speed = vars(&h, id).velocity.x;
    assert!(near(speed, 115.2, 0.01), "{speed}");
    for _ in 0..100 {
        h.step(&mut [], 1);
        let next = vars(&h, id).velocity.x;
        assert!(next <= speed, "{next} after {speed}");
        speed = next;
    }
    let stopped = vars(&h, id);
    assert_eq!(stopped.velocity, Vec3::ZERO);
    assert!(stopped.origin.x > start.x + 5.0 && stopped.origin.x < start.x + 60.0);
    assert!(near(stopped.origin.z, rest_z, 0.1));
    assert_ne!(stopped.flags & FL_ONGROUND, 0);
}

fn stub_with(game: TestGame) -> Harness {
    let cfg = ServerConfig {
        max_clients: 1,
        max_edicts: 64,
        ..ServerConfig::default()
    };
    Harness::with_config(cfg, game)
}

/// A `BOUNCE` mover with friction 0.5 two units over a floor, falling at 400 units/s.
fn bounce_onto_floor(eng: &mut EngineApi<'_>) -> (EdictId, EdictId) {
    let floor_id = floor(eng);
    let id = mover(
        eng,
        MOVETYPE_BOUNCE,
        SOLID_BBOX,
        Vec3::new(0.0, 0.0, 6.0),
        Vec3::new(0.0, 0.0, -400.0),
    );
    eng.ent_mut(id).unwrap().friction = 0.5;
    (floor_id, id)
}

#[test]
fn a_touch_at_impact_sees_the_velocity_before_the_clip() {
    let mut h = stub();
    let (_, id) = run(&mut h, bounce_onto_floor);
    let states = &h.server.game().mover_touch_states;
    // One impact: the mover's own touch, seen with the velocity it hit with (400 plus the
    // frame's gravity), not the reflected one, and not on the ground.
    assert_eq!(states.len(), 1, "{states:?}");
    let (index, origin, velocity, flags) = states[0];
    assert_eq!(index, id.index);
    assert!(near(velocity.z, -408.0, 0.01), "{velocity:?}");
    assert_eq!(flags & FL_ONGROUND, 0);
    assert!(near(origin.z, 4.0, 0.1), "{origin:?}");
    assert!(near(vars(&h, id).velocity.z, 204.0, 0.01));
}

#[test]
fn a_touch_that_changes_the_velocity_alters_the_outcome() {
    let game = TestGame {
        mover_touch_velocity: Some(Vec3::new(0.0, 0.0, -100.0)),
        ..TestGame::default()
    };
    let mut h = stub_with(game);
    let (_, id) = run(&mut h, bounce_onto_floor);
    // The clip works on what the touch left: -100 reflected by 1.5 is 50, not 204.
    let v = vars(&h, id).velocity;
    assert!(near(v.z, 50.0, 0.01), "{v:?}");
}

/// The source reads `movetype` after the impact: a touch that turns a `BOUNCE` entity into a
/// `FLY` one makes the clip a plain slide (no reflection) and the entity stops on the floor.
#[test]
fn a_touch_that_changes_the_movetype_changes_the_branch() {
    let game = TestGame {
        mover_touch_movetype: Some(MOVETYPE_FLY),
        ..TestGame::default()
    };
    let mut h = stub_with(game);
    let (_, id) = run(&mut h, bounce_onto_floor);
    let v = vars(&h, id);
    assert_eq!(v.movetype, MOVETYPE_FLY);
    // As a `BOUNCE` entity it would leave with +204 (see the test above).
    assert_eq!(v.velocity, Vec3::ZERO);
    assert_ne!(v.flags & FL_ONGROUND, 0);
}

#[test]
fn a_touch_that_removes_the_entity_ends_the_move() {
    for movetype in [MOVETYPE_BOUNCE, MOVETYPE_TOSS, MOVETYPE_STEP] {
        let game = TestGame {
            mover_touch_removes: true,
            ..TestGame::default()
        };
        let mut h = stub_with(game);
        let id = run(&mut h, move |eng| {
            floor(eng);
            mover(
                eng,
                movetype,
                SOLID_SLIDEBOX,
                Vec3::new(0.0, 0.0, 5.0),
                Vec3::new(0.0, 0.0, -400.0),
            )
        });
        assert!(h.server.edicts().get(id).is_none(), "movetype {movetype}");
        // The mover's touch only: the floor's half of the impact and every later impact of
        // the move are dropped.
        let log = &h.server.game().touch_log;
        assert_eq!(log.len(), 1, "movetype {movetype}: {log:?}");
        assert_eq!(log[0].0, id.index);
        h.step(&mut [], 3);
        assert_eq!(h.server.game().touch_log.len(), 1);
    }
}

#[test]
fn a_step_entity_touch_sees_the_velocity_before_the_clip() {
    let mut h = stub();
    let id = run(&mut h, |eng| {
        floor(eng);
        mover(
            eng,
            MOVETYPE_STEP,
            SOLID_BBOX,
            Vec3::new(0.0, 0.0, 4.2),
            Vec3::new(0.0, 0.0, -20.0),
        )
    });
    let states = &h.server.game().mover_touch_states;
    assert_eq!(states.len(), 1, "{states:?}");
    // 20 units/s and the frame's 8 of gravity, not yet clipped to the floor.
    assert!(near(states[0].2.z, -28.0, 0.01), "{:?}", states[0]);
    let v = vars(&h, id);
    assert_eq!(v.velocity.z, 0.0);
    assert_ne!(v.flags & FL_ONGROUND, 0);
}

#[test]
fn a_move_into_a_crease_slides_along_it() {
    let mut h = stub();
    // Two walls meeting at a corner: faces at x = 40 and y = 40. The box is flush against both
    // (4 units from the faces, less the clipping epsilon) and flies diagonally into them.
    let flush = 36.0 - 0.03125;
    let id = run(&mut h, move |eng| {
        for (origin, half) in [
            (Vec3::new(50.0, 0.0, 0.0), Vec3::new(10.0, 200.0, 200.0)),
            (Vec3::new(0.0, 50.0, 0.0), Vec3::new(200.0, 10.0, 200.0)),
        ] {
            let wall = eng.create_entity("test_wall").expect("free edict");
            eng.ent_mut(wall).unwrap().solid = SOLID_SLIDEBOX;
            eng.set_size(wall, -half, half);
            eng.set_origin(wall, origin);
        }
        let id = mover(
            eng,
            MOVETYPE_STEP,
            SOLID_BBOX,
            Vec3::new(flush, flush, 100.0),
            Vec3::new(100.0, 100.0, -50.0),
        );
        eng.ent_mut(id).unwrap().flags |= cs_engine::consts::FL_FLY;
        id
    });
    // The planes of both walls clip it away; what is left is along the crease, straight down.
    let v = vars(&h, id);
    assert!(v.velocity.x == 0.0 && v.velocity.y == 0.0, "{:?}", v.velocity);
    assert!(near(v.velocity.z, -50.0, 0.01), "{:?}", v.velocity);
    assert!(v.origin.z < 100.0 && v.origin.z > 99.0, "{:?}", v.origin);
    assert!(near(v.origin.x, flush, 0.05) && near(v.origin.y, flush, 0.05));
}

/// The wade sounds of a real pool: `player/pl_wade1.wav` when an entity crosses into the water
/// and `player/pl_wade2.wav` when it leaves, heard by a player in the same room.
#[test]
fn wade_sounds_play_on_entering_and_leaving_water() {
    use cs_proto48::messages::SvcMessage;

    let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
        return;
    };
    let raw = vfs.read_bytes("maps/cs_militia.bsp").expect("cs_militia reads");
    let bsp = Arc::new(BspMap::load(&vfs, "cs_militia").expect("cs_militia parses"));
    let make = || ServerMap::from_bsp("cs_militia", &raw, bsp.clone()).expect("valid BSP header");
    let cfg = ServerConfig {
        max_clients: 1,
        ..ServerConfig::default()
    };
    let mut first = Harness::with_map(cfg.clone(), make(), TestGame::default());
    let probe_bsp = bsp.clone();
    let surface = run(&mut first, move |eng| water_spot(eng, &probe_bsp))
        .expect("cs_militia has a pool with air above and 30 units of water");

    let game = TestGame {
        precache_sounds: vec![
            "player/pl_wade1.wav".to_string(),
            "player/pl_wade2.wav".to_string(),
        ],
        spawn_origins: vec![surface + Vec3::new(0.0, 0.0, 60.0)],
        ..TestGame::default()
    };
    let mut h = Harness::with_map(cfg, make(), game);
    let mut probe = h.add_probe("Player");
    h.step(&mut [&mut probe], 200);
    let heard = |probe: &cs_probe::ProbeClient<cs_transport::LoopbackEndpoint>| {
        probe
            .messages()
            .iter()
            .filter_map(|m| match m {
                SvcMessage::Sound(s) => Some((s.sound_index, s.entity)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert!(heard(&probe).is_empty());

    let tossed = run(&mut h, move |eng| {
        mover(
            eng,
            MOVETYPE_TOSS,
            SOLID_NOT,
            surface + Vec3::new(0.0, 0.0, 20.0),
            Vec3::ZERO,
        )
    });
    h.step(&mut [&mut probe], 40);
    // Sound 1 is `pl_wade1` (the order of the precache): the entry.
    assert_eq!(heard(&probe).first(), Some(&(1, tossed.index)));

    let rising = run(&mut h, move |eng| {
        mover(
            eng,
            MOVETYPE_FLY,
            SOLID_NOT,
            surface - Vec3::Z * 20.0,
            Vec3::new(0.0, 0.0, 500.0),
        )
    });
    h.step(&mut [&mut probe], 40);
    // The flyer began in the water (no entry) and leaves it: `pl_wade2`.
    let own: Vec<_> = heard(&probe)
        .into_iter()
        .filter(|&(_, entity)| entity == rising.index)
        .collect();
    assert_eq!(own, [(2, rising.index)]);
}
