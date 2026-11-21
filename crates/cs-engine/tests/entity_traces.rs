//! Traces against linked entities (`SV_Move`): box entities on the stub map, brush entities and
//! the world's own answers on de_dust2 (those return early when the game is not installed).

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use common::{Harness, TestGame};
use cs_assets::bsp::BspMap;
use cs_engine::collision::Hull;
use cs_engine::consts::{
    FL_MONSTER, MOVETYPE_PUSH, MOVETYPE_PUSHSTEP, SOLID_BBOX, SOLID_BSP, SOLID_NOT, SOLID_SLIDEBOX,
    SOLID_TRIGGER,
};
use cs_engine::{EdictId, EngineApi, MoveFilter, ServerConfig, ServerMap};
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

/// A solid box entity of half-size `half` linked at `origin`.
fn boxed(eng: &mut EngineApi<'_>, solid: u8, origin: Vec3, half: f32) -> EdictId {
    let id = eng.create_entity("test_box").expect("free edict");
    eng.ent_mut(id).unwrap().solid = solid;
    eng.set_size(id, Vec3::splat(-half), Vec3::splat(half));
    eng.set_origin(id, origin);
    id
}

fn dust2_harness() -> Option<Harness> {
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

/// The world edict: what `trace_line` reports for a miss.
fn world() -> EdictId {
    EdictId {
        index: 0,
        serial: 0,
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.001
}

#[test]
fn line_hits_a_bbox_entity_and_reports_it() {
    let mut h = Harness::new(2);
    let (id, t) = run(&mut h, |eng| {
        let id = boxed(eng, SOLID_BBOX, Vec3::new(100.0, 0.0, 0.0), 16.0);
        let t = eng.trace_line(
            Vec3::ZERO,
            Vec3::new(200.0, 0.0, 0.0),
            MoveFilter::Normal,
            None,
        );
        (id, t)
    });
    assert_eq!(t.ent, Some(id));
    // The near face is at x = 84; the trace stops DIST_EPSILON short of it.
    assert!(
        near(t.trace.fraction, (84.0 - 0.03125) / 200.0),
        "{:?}",
        t.trace
    );
    assert!(near(t.trace.end_pos.x, 84.0 - 0.03125));
    assert_eq!(t.trace.plane_normal, Vec3::NEG_X);
    assert!(t.trace.hit_solid && !t.trace.start_solid && !t.trace.all_solid);

    // A line that misses the box reports nothing.
    let t = run(&mut h, |eng| {
        eng.trace_line(
            Vec3::new(0.0, 40.0, 0.0),
            Vec3::new(200.0, 40.0, 0.0),
            MoveFilter::Normal,
            None,
        )
    });
    assert_eq!(t.ent, Some(world()), "a missed line reports the world");
    assert_eq!(t.trace.fraction, 1.0);
    assert_eq!(t.trace.end_pos, Vec3::new(200.0, 40.0, 0.0));
}

#[test]
fn pass_and_owner_are_skipped() {
    fn trace(eng: &EngineApi<'_>, from_x: f32, pass: Option<EdictId>) -> Option<EdictId> {
        let (start, end) = (Vec3::new(from_x, 0.0, 0.0), Vec3::new(200.0, 0.0, 0.0));
        eng.trace_line(start, end, MoveFilter::Normal, pass).ent
    }
    let mut h = Harness::new(2);
    let (hits, ids) = run(&mut h, |eng| {
        let shooter = boxed(eng, SOLID_SLIDEBOX, Vec3::new(-100.0, 0.0, 0.0), 8.0);
        let target = boxed(eng, SOLID_BBOX, Vec3::new(100.0, 0.0, 0.0), 16.0);
        let missile = boxed(eng, SOLID_BBOX, Vec3::new(50.0, 0.0, 0.0), 4.0);
        eng.ent_mut(missile).unwrap().owner = Some(shooter);
        // Nothing passed: the first thing in the way is the missile.
        let plain = trace(eng, -50.0, None);
        // Passing the missile: the target.
        let pass_missile = trace(eng, -50.0, Some(missile));
        // Passing the shooter skips its own missile (`touch.owner == pass`).
        let pass_shooter = trace(eng, -50.0, Some(shooter));
        // Passing the target: the missile.
        let pass_target = trace(eng, -50.0, Some(target));
        // Starting before the shooter and passing the missile skips the shooter, the
        // missile's owner (`pass.owner == touch`); without a pass it is hit first.
        let from_behind = trace(eng, -150.0, None);
        let owner_of_pass = trace(eng, -150.0, Some(missile));
        // Owned by the passed entity, the target is skipped too.
        eng.ent_mut(target).unwrap().owner = Some(missile);
        let owned_by_pass = trace(eng, -50.0, Some(missile));
        (
            (
                plain,
                pass_missile,
                pass_shooter,
                pass_target,
                from_behind,
                owner_of_pass,
                owned_by_pass,
            ),
            (shooter, target, missile),
        )
    });
    let (shooter, target, missile) = ids;
    let (plain, pass_missile, pass_shooter, pass_target, from_behind, owner_of_pass, owned) = hits;
    assert_eq!(plain, Some(missile));
    assert_eq!(pass_missile, Some(target));
    assert_eq!(pass_shooter, Some(target), "own missile skipped");
    assert_eq!(pass_target, Some(missile));
    assert_eq!(from_behind, Some(shooter));
    assert_eq!(
        owner_of_pass,
        Some(target),
        "the passed entity's owner skipped"
    );
    assert_eq!(
        owned,
        Some(world()),
        "entity owned by the passed one skipped"
    );
}

#[test]
fn triggers_and_not_solid_are_ignored() {
    let mut h = Harness::new(2);
    let hit = run(&mut h, |eng| {
        boxed(eng, SOLID_TRIGGER, Vec3::new(50.0, 0.0, 0.0), 16.0);
        boxed(eng, SOLID_NOT, Vec3::new(100.0, 0.0, 0.0), 16.0);
        let (start, end) = (Vec3::ZERO, Vec3::new(200.0, 0.0, 0.0));
        let ignored = eng.trace_line(start, end, MoveFilter::Normal, None).ent;
        let slide = boxed(eng, SOLID_SLIDEBOX, Vec3::new(150.0, 0.0, 0.0), 8.0);
        let seen = eng.trace_line(start, end, MoveFilter::Normal, None).ent;
        (ignored, seen, slide)
    });
    assert_eq!(hit.0, Some(world()));
    assert_eq!(hit.1, Some(hit.2));
}

#[test]
fn no_monsters_skips_everything_but_brushes_and_pushables() {
    let mut h = Harness::new(2);
    let (plain, nomonsters, pushable, boxy) = run(&mut h, |eng| {
        let boxy = boxed(eng, SOLID_BBOX, Vec3::new(100.0, 0.0, 0.0), 16.0);
        let (start, end) = (Vec3::ZERO, Vec3::new(200.0, 0.0, 0.0));
        let plain = eng.trace_line(start, end, MoveFilter::Normal, None).ent;
        let nomonsters = eng.trace_line(start, end, MoveFilter::NoMonsters, None).ent;
        eng.ent_mut(boxy).unwrap().movetype = MOVETYPE_PUSHSTEP;
        let pushable = eng.trace_line(start, end, MoveFilter::NoMonsters, None).ent;
        (plain, nomonsters, pushable, boxy)
    });
    assert_eq!(plain, Some(boxy));
    assert_eq!(nomonsters, Some(world()));
    assert_eq!(pushable, Some(boxy));
}

#[test]
fn missile_widens_monsters() {
    let mut h = Harness::new(2);
    let (normal, missile, monster) = run(&mut h, |eng| {
        let monster = boxed(eng, SOLID_SLIDEBOX, Vec3::new(100.0, 30.0, 0.0), 16.0);
        eng.ent_mut(monster).unwrap().flags |= FL_MONSTER;
        let (start, end) = (Vec3::ZERO, Vec3::new(200.0, 0.0, 0.0));
        let normal = eng.trace_line(start, end, MoveFilter::Normal, None).ent;
        let missile = eng.trace_line(start, end, MoveFilter::Missile, None).ent;
        (normal, missile, monster)
    });
    assert_eq!(normal, Some(world()));
    assert_eq!(missile, Some(monster));
}

#[test]
fn hull_trace_against_entity_uses_expanded_box() {
    let mut h = Harness::new(2);
    let (id, standing, crouched, line) = run(&mut h, |eng| {
        let id = boxed(eng, SOLID_BBOX, Vec3::new(100.0, 0.0, 0.0), 16.0);
        let (start, end) = (Vec3::ZERO, Vec3::new(200.0, 0.0, 0.0));
        let standing = eng.trace_hull_ents(start, end, Hull::Standing, MoveFilter::Normal, None);
        let crouched = eng.trace_hull_ents(start, end, Hull::Crouched, MoveFilter::Normal, None);
        let line = eng.trace_line(start, end, MoveFilter::Normal, None);
        (id, standing, crouched, line)
    });
    // The point stops at the face, 84; a hull 16 wide stops 16 earlier.
    assert!(near(line.trace.end_pos.x, 84.0 - 0.03125));
    for t in [standing, crouched] {
        assert_eq!(t.ent, Some(id));
        assert!(near(t.trace.end_pos.x, 68.0 - 0.03125), "{:?}", t.trace);
    }
    // Vertically the standing hull is 36 tall, the crouched one 18, 16 of the box's own.
    let mut h = Harness::new(2);
    let (stand_z, crouch_z) = run(&mut h, |eng| {
        boxed(eng, SOLID_BBOX, Vec3::new(0.0, 0.0, 100.0), 16.0);
        let (start, end) = (Vec3::ZERO, Vec3::new(0.0, 0.0, 200.0));
        (
            eng.trace_hull_ents(start, end, Hull::Standing, MoveFilter::Normal, None)
                .trace
                .end_pos
                .z,
            eng.trace_hull_ents(start, end, Hull::Crouched, MoveFilter::Normal, None)
                .trace
                .end_pos
                .z,
        )
    });
    assert!(near(stand_z, 84.0 - 36.0 - 0.03125), "{stand_z}");
    assert!(near(crouch_z, 84.0 - 18.0 - 0.03125), "{crouch_z}");
}

#[test]
fn start_inside_an_entity_is_start_solid_and_hits_it() {
    let mut h = Harness::new(2);
    let (id, t, stuck) = run(&mut h, |eng| {
        let id = boxed(eng, SOLID_BBOX, Vec3::ZERO, 16.0);
        let walker = boxed(eng, SOLID_SLIDEBOX, Vec3::new(4.0, 0.0, 0.0), 8.0);
        let t = eng.trace_line(
            Vec3::ZERO,
            Vec3::new(100.0, 0.0, 0.0),
            MoveFilter::Normal,
            None,
        );
        (
            id,
            t,
            (
                eng.test_entity_position(walker),
                eng.test_entity_position(id),
            ),
        )
    });
    // Both entities hold the start; the one visited last is reported, as in the original.
    assert!(t.ent.is_some());
    assert!(t.trace.start_solid);
    assert_eq!(stuck.0, Some(id));
    assert!(stuck.1.is_some(), "the box stands in the walker too");
}

#[test]
fn hostile_input_is_refused() {
    let mut h = Harness::new(2);
    let results = run(&mut h, |eng| {
        let id = boxed(eng, SOLID_BBOX, Vec3::new(100.0, 0.0, 0.0), 16.0);
        let bad = [
            Vec3::splat(f32::NAN),
            Vec3::splat(f32::INFINITY),
            Vec3::splat(1.0e30),
        ];
        let mut out = Vec::new();
        for b in bad {
            out.push(eng.trace_line(Vec3::ZERO, b, MoveFilter::Normal, None));
            out.push(eng.trace_line(b, Vec3::ZERO, MoveFilter::Normal, None));
            out.push(eng.trace_box(Vec3::ZERO, b, b, Vec3::X, MoveFilter::Missile, None));
            out.push(eng.trace_hull_ents(b, b, Hull::Large, MoveFilter::NoMonsters, Some(id)));
            out.push(eng.trace_entity(id, b, Vec3::ZERO, Hull::Point));
        }
        out.push(eng.trace_entity(
            EdictId {
                index: 9,
                serial: 99,
            },
            Vec3::ZERO,
            Vec3::X,
            Hull::Point,
        ));
        assert_eq!(eng.point_contents_ents(bad[0]), -1);
        assert_eq!(
            eng.test_entity_position(EdictId {
                index: 9,
                serial: 99
            }),
            None
        );
        out
    });
    for t in results {
        assert_eq!(t.ent, None);
        assert_eq!(t.trace.fraction, 1.0);
        assert!(t.trace.end_pos.is_finite());
        assert!(t.trace.plane_normal.is_finite());
    }
}

#[test]
fn hostile_entities_trace_safely() {
    let mut h = Harness::new(2);
    let t = run(&mut h, |eng| {
        // As many entities as the pool gives, all in one place...
        let mut ids = Vec::new();
        while let Some(id) = eng.create_entity("crowd") {
            eng.ent_mut(id).unwrap().solid = SOLID_BBOX;
            eng.set_size(id, Vec3::splat(-8.0), Vec3::splat(8.0));
            eng.set_origin(id, Vec3::new(100.0, 0.0, 0.0));
            ids.push(id);
        }
        assert!(ids.len() > 500, "{}", ids.len());
        // ...a few of them with numbers nobody should have.
        let nasty = [
            (Vec3::splat(f32::NAN), Vec3::splat(f32::NAN)),
            (Vec3::splat(1.0e30), Vec3::splat(f32::INFINITY)),
            (Vec3::splat(f32::NEG_INFINITY), Vec3::splat(-1.0e30)),
        ];
        for (id, (a, b)) in ids.iter().zip(nasty) {
            let v = eng.ent_mut(*id).unwrap();
            v.origin = a;
            v.mins = b;
            v.maxs = a;
            v.angles = b;
            eng.link_entity(*id, true);
        }
        for filter in [
            MoveFilter::Normal,
            MoveFilter::NoMonsters,
            MoveFilter::Missile,
        ] {
            for hull in [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched] {
                let t =
                    eng.trace_hull_ents(Vec3::ZERO, Vec3::new(200.0, 0.0, 0.0), hull, filter, None);
                assert!(t.trace.fraction.is_finite() && t.trace.end_pos.is_finite());
            }
        }
        for id in ids.iter().take(8) {
            eng.test_entity_position(*id);
        }
        eng.trace_line(
            Vec3::ZERO,
            Vec3::new(200.0, 0.0, 0.0),
            MoveFilter::Normal,
            None,
        )
    });
    assert!(t.ent.is_some());
    assert!(t.trace.fraction < 1.0);
}

#[test]
fn world_answers_match_world_only_trace() {
    let Some(mut h) = dust2_harness() else {
        return;
    };
    let ok = run(&mut h, |eng| {
        let mut state: u64 = 0x5EED_0003;
        let mut unit = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 40) as f32 / 16_777_216.0
        };
        let hulls = [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched];
        let (mut hits, mut solids) = (0, 0);
        for i in 0..50 {
            let start = Vec3::new(
                -2000.0 + 4000.0 * unit(),
                -2000.0 + 4000.0 * unit(),
                -400.0 + 800.0 * unit(),
            );
            let end = start
                + Vec3::new(
                    (unit() * 2.0 - 1.0) * 512.0,
                    (unit() * 2.0 - 1.0) * 512.0,
                    (unit() * 2.0 - 1.0) * 512.0,
                );
            let hull = hulls[i % 4];
            let world = eng.trace_hull(start, end, hull);
            let ents = eng.trace_hull_ents(start, end, hull, MoveFilter::Normal, None);
            assert_eq!(
                ents.trace.fraction.to_bits(),
                world.fraction.to_bits(),
                "{i}"
            );
            assert_eq!(ents.trace.end_pos, world.end_pos, "{i}");
            assert_eq!(ents.trace.plane_normal, world.plane_normal, "{i}");
            assert_eq!(ents.trace.all_solid, world.all_solid, "{i}");
            assert_eq!(ents.trace.start_solid, world.start_solid, "{i}");
            assert_eq!(ents.trace.hit_sky, world.hit_sky, "{i}");
            assert_eq!(
                ents.ent,
                (world.fraction < 1.0 || world.start_solid).then(|| eng.world())
            );
            hits += usize::from(world.fraction < 1.0);
            solids += usize::from(world.start_solid);
        }
        (hits, solids)
    });
    assert!(ok.0 > 5, "the traces hit the map: {ok:?}");
}

/// The first brush model of de_dust2 (from 1) that a vertical line through the middle of its
/// box hits from above when it stands at the origin, with the height the hit is at.
fn vertical_target(eng: &mut EngineApi<'_>) -> (usize, EdictId, f32) {
    for model in 1..64 {
        let name = format!("*{model}");
        let id = eng.create_entity("func_wall").expect("free edict");
        if !eng.set_model(id, &name) {
            break;
        }
        {
            let v = eng.ent_mut(id).unwrap();
            v.solid = SOLID_BSP;
            v.movetype = MOVETYPE_PUSH;
        }
        eng.set_origin(id, Vec3::ZERO);
        let v = eng.ent(id).unwrap();
        let (mins, maxs) = (v.mins, v.maxs);
        let mid = (mins + maxs) * 0.5;
        let t = eng.trace_entity(
            id,
            Vec3::new(mid.x, mid.y, maxs.z + 64.0),
            Vec3::new(mid.x, mid.y, mins.z - 64.0),
            Hull::Point,
        );
        if t.ent == Some(id) && !t.trace.start_solid && t.trace.fraction > 0.0 {
            return (model, id, t.trace.end_pos.z);
        }
        eng.ent_mut(id).unwrap().solid = SOLID_NOT;
    }
    panic!("a brush model of de_dust2 whose middle a vertical line hits");
}

#[test]
fn moved_brush_entity_collides_at_its_origin() {
    let Some(mut h) = dust2_harness() else {
        return;
    };
    let (z0, z1, hit) = run(&mut h, |eng| {
        let (_, id, z0) = vertical_target(eng);
        let v = eng.ent(id).unwrap();
        let mid = (v.mins + v.maxs) * 0.5;
        let (top, bottom) = (v.maxs.z + 64.0, v.mins.z - 64.0);
        eng.set_origin(id, Vec3::new(0.0, 0.0, 64.0));
        let t = eng.trace_entity(
            id,
            Vec3::new(mid.x, mid.y, top + 64.0),
            Vec3::new(mid.x, mid.y, bottom + 64.0),
            Hull::Point,
        );
        // The same line, through `SV_Move`: the entity is in the way when nothing else is
        // (the world may be as well; the entity is what is asked for).
        let line = eng.trace_line(
            Vec3::new(mid.x, mid.y, top + 64.0),
            Vec3::new(mid.x, mid.y, bottom + 64.0),
            MoveFilter::Normal,
            None,
        );
        (
            z0,
            t.trace.end_pos.z,
            (t.ent == Some(id), line.trace.end_pos.z >= t.trace.end_pos.z),
        )
    });
    assert!(near(z1, z0 + 64.0), "{z0} {z1}");
    assert!(hit.0);
    assert!(hit.1);
}

#[test]
fn rotated_brush_entity_collides_rotated() {
    let Some(mut h) = dust2_harness() else {
        return;
    };
    let out = run(&mut h, |eng| {
        let (_, id, _) = vertical_target(eng);
        let v = eng.ent(id).unwrap();
        let (mins, maxs) = (v.mins, v.maxs);
        let origin = Vec3::new(0.0, 0.0, 16.0);
        eng.set_origin(id, origin);
        // A slanted ray through the middle of the unrotated model.
        let mid = (mins + maxs) * 0.5;
        let a = Vec3::new(mid.x - 40.0, mid.y - 20.0, maxs.z + 48.0) - origin;
        let b = Vec3::new(mid.x + 40.0, mid.y + 20.0, mins.z - 48.0) - origin;
        let plain = eng.trace_entity(id, a + origin, b + origin, Hull::Point);

        // Yaw 90 turns the model about its origin: (x, y, z) -> (-y, x, z).
        let turn = |p: Vec3| Vec3::new(-p.y, p.x, p.z);
        eng.ent_mut(id).unwrap().angles = Vec3::new(0.0, 90.0, 0.0);
        let rotated = eng.trace_entity(id, turn(a) + origin, turn(b) + origin, Hull::Point);
        (plain, rotated, turn(a) + origin)
    });
    let (plain, rotated, _) = out;
    assert!(
        plain.trace.fraction < 1.0,
        "the ray hits the model: {:?}",
        plain.trace
    );
    assert!(
        (plain.trace.fraction - rotated.trace.fraction).abs() < 1e-3,
        "{:?} {:?}",
        plain.trace,
        rotated.trace
    );
    let want = Vec3::new(
        -plain.trace.plane_normal.y,
        plain.trace.plane_normal.x,
        plain.trace.plane_normal.z,
    );
    assert!(
        (rotated.trace.plane_normal - want).length() < 1e-4,
        "{:?} vs {:?}",
        rotated.trace.plane_normal,
        want
    );
}

#[test]
fn a_move_inside_a_box_entity_keeps_the_source_fraction() {
    let mut h = Harness::new(2);
    let (id, t) = run(&mut h, |eng| {
        let id = boxed(eng, SOLID_BBOX, Vec3::ZERO, 16.0);
        let end = Vec3::new(5.0, 0.0, 0.0);
        (
            id,
            eng.trace_box(
                Vec3::ZERO,
                Vec3::ZERO,
                Vec3::ZERO,
                end,
                MoveFilter::Normal,
                None,
            ),
        )
    });
    // As `SV_RecursiveHullCheck` leaves it: all solid, but fraction 1 and the end point.
    assert_eq!(t.ent, Some(id));
    assert!(t.trace.all_solid && t.trace.start_solid);
    assert_eq!(t.trace.fraction, 1.0);
    assert_eq!(t.trace.end_pos, Vec3::new(5.0, 0.0, 0.0));
}
