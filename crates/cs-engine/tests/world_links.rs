//! Linking entities into the world: absolute bounds, the area tree and the leaves an entity
//! touches for the visibility test.

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use common::{Harness, TestGame};
use cs_assets::bsp::BspMap;
use cs_engine::consts::{MOVETYPE_PUSH, SOLID_BBOX, SOLID_BSP, SOLID_NOT, SOLID_TRIGGER};
use cs_engine::{EdictId, EngineApi, ServerConfig, ServerMap};
use cs_probe::ProbeState;
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

fn dust2() -> Option<(ServerMap, ServerMap)> {
    let vfs = Vfs::discover(None, VfsConfig::default()).ok()?;
    let raw = vfs.read_bytes("maps/de_dust2.bsp").expect("de_dust2 reads");
    let bsp = Arc::new(BspMap::load(&vfs, "de_dust2").expect("de_dust2 parses"));
    let make = || ServerMap::from_bsp("de_dust2", &raw, bsp.clone()).expect("valid BSP header");
    Some((make(), make()))
}

#[test]
fn linked_entity_has_absolute_bounds() {
    let mut h = Harness::new(2);
    let v = run(&mut h, |eng| {
        let id = boxed(eng, SOLID_BBOX, Vec3::new(100.0, 0.0, 0.0), 16.0);
        let first = eng.ent(id).unwrap().clone();
        // Moving it links it again.
        eng.set_origin(id, Vec3::new(0.0, 50.0, 0.0));
        let second = eng.ent(id).unwrap().clone();
        // Not through `set_origin`: the bounds wait for the next link.
        eng.ent_mut(id).unwrap().origin = Vec3::new(7.0, 7.0, 7.0);
        let stale = eng.ent(id).unwrap().absmin;
        eng.link_entity(id, false);
        let third = eng.ent(id).unwrap().clone();
        (first, second, stale, third)
    });
    let (first, second, stale, third) = v;
    assert_eq!(first.absmin, Vec3::new(83.0, -17.0, -17.0));
    assert_eq!(first.absmax, Vec3::new(117.0, 17.0, 17.0));
    assert_eq!(first.size, Vec3::splat(32.0));
    assert_eq!(second.absmin, Vec3::new(-17.0, 33.0, -17.0));
    assert_eq!(second.absmax, Vec3::new(17.0, 67.0, 17.0));
    assert_eq!(stale, second.absmin);
    assert_eq!(third.absmin, Vec3::splat(-10.0));
    assert_eq!(third.absmax, Vec3::splat(24.0));
}

#[test]
fn rotated_brush_bounds_cover_the_rotation() {
    let Some((map, bounds_map)) = dust2() else {
        return;
    };
    let (mins, maxs) = bounds_map
        .collision()
        .and_then(|c| c.model_bounds(1))
        .expect("model 1");
    let cfg = ServerConfig {
        max_clients: 2,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_map(cfg, map, TestGame::default());
    let origin = Vec3::new(100.0, 200.0, 300.0);
    let (plain, rotated) = run(&mut h, move |eng| {
        let id = eng.create_entity("func_wall").unwrap();
        assert!(eng.set_model(id, "*1"));
        {
            let v = eng.ent_mut(id).unwrap();
            v.solid = SOLID_BSP;
            v.movetype = MOVETYPE_PUSH;
        }
        eng.set_origin(id, origin);
        let plain = (eng.ent(id).unwrap().absmin, eng.ent(id).unwrap().absmax);
        eng.ent_mut(id).unwrap().angles = Vec3::new(0.0, 45.0, 0.0);
        eng.link_entity(id, false);
        let v = eng.ent(id).unwrap();
        (plain, (v.absmin, v.absmax))
    });
    // Not rotated: the model's box moved to the origin, widened by 1.
    assert_eq!(
        plain,
        (origin + mins - Vec3::ONE, origin + maxs + Vec3::ONE)
    );
    // Rotated: the cube of the largest coordinate around the origin, widened by 1.
    let radius = mins.abs().max_element().max(maxs.abs().max_element());
    assert_eq!(
        rotated,
        (
            origin - Vec3::splat(radius + 1.0),
            origin + Vec3::splat(radius + 1.0)
        )
    );
    // The cube holds the box wherever the rotation puts it.
    assert!(rotated.0.cmple(plain.0).all() && rotated.1.cmpge(plain.1).all());
}

#[test]
fn entities_in_box_finds_linked_only() {
    let mut h = Harness::new(2);
    let (ids, found) = run(&mut h, |eng| {
        let a = boxed(eng, SOLID_BBOX, Vec3::ZERO, 8.0);
        let b = boxed(eng, SOLID_BBOX, Vec3::new(100.0, 0.0, 0.0), 8.0);
        // Not solid: linked (it has bounds) but not in the tree.
        let c = boxed(eng, SOLID_NOT, Vec3::new(10.0, 0.0, 0.0), 8.0);
        let d = boxed(eng, SOLID_TRIGGER, Vec3::new(50.0, 0.0, 0.0), 8.0);
        let far = boxed(eng, SOLID_BBOX, Vec3::new(5000.0, 0.0, 0.0), 8.0);
        // Solid but never linked: only its variables say so.
        let loose = eng.create_entity("loose").unwrap();
        eng.ent_mut(loose).unwrap().solid = SOLID_BBOX;
        eng.ent_mut(loose).unwrap().maxs = Vec3::splat(8.0);
        // Linked again in the reverse of their index order.
        for id in [far, d, c, b, a] {
            eng.link_entity(id, false);
        }
        let lo = Vec3::new(-20.0, -20.0, -20.0);
        let hi = Vec3::new(120.0, 20.0, 20.0);
        let found = (
            eng.entities_in_box(lo, hi),
            eng.entities_in_box(Vec3::new(4990.0, -1.0, -1.0), Vec3::new(5010.0, 1.0, 1.0)),
            eng.entities_in_box(Vec3::splat(1000.0), Vec3::splat(2000.0)),
            // Touching boxes count, as in `SV_AreaEdicts`.
            eng.entities_in_box(Vec3::new(109.0, 0.0, 0.0), Vec3::new(200.0, 1.0, 1.0)),
            eng.entities_in_box(Vec3::NAN, hi),
            eng.entities_in_box(lo, Vec3::INFINITY),
        );
        // The world and the players are not in the tree.
        assert!(
            eng.entities_in_box(Vec3::splat(-1e5), Vec3::splat(1e5))
                .len()
                >= 4
        );
        assert!(
            !eng.entities_in_box(Vec3::splat(-1e5), Vec3::splat(1e5))
                .contains(&eng.world())
        );
        // Now `c` becomes solid.
        eng.ent_mut(c).unwrap().solid = SOLID_BBOX;
        eng.link_entity(c, false);
        let with_c = eng.entities_in_box(lo, hi);
        ((a, b, c, d, far, with_c), found)
    });
    let ((a, b, c, d, far, with_c), (near, far_hits, none, touching, nan, inf)) = (ids, found);
    assert_eq!(near, [a, b, d]);
    assert_eq!(far_hits, [far]);
    assert!(none.is_empty());
    assert_eq!(touching, [b]);
    assert!(nan.is_empty() && inf.is_empty());
    assert_eq!(with_c, [a, b, c, d]);
}

#[test]
fn unlink_on_remove_and_on_map_change() {
    let mut h = Harness::new(2);
    let ids = run(&mut h, |eng| {
        let a = boxed(eng, SOLID_BBOX, Vec3::ZERO, 8.0);
        let b = boxed(eng, SOLID_BBOX, Vec3::new(20.0, 0.0, 0.0), 8.0);
        let all = |eng: &EngineApi<'_>| eng.entities_in_box(Vec3::splat(-1e4), Vec3::splat(1e4));
        assert_eq!(all(eng), [a, b]);
        eng.remove_entity(a);
        assert_eq!(all(eng), [b]);
        // Removing again, or removing what never was, is harmless.
        eng.remove_entity(a);
        eng.set_origin(a, Vec3::ZERO);
        eng.link_entity(a, true);
        eng.set_size(a, Vec3::splat(-1.0), Vec3::splat(1.0));
        assert_eq!(all(eng), [b]);
        b
    });
    h.server.change_map(ServerMap::stub("second"));
    let after = run(&mut h, move |eng| {
        let everything = eng.entities_in_box(Vec3::splat(-1e5), Vec3::splat(1e5));
        assert!(eng.ent(ids).is_none());
        // The tree of the new map takes entities again.
        let n = boxed(eng, SOLID_BBOX, Vec3::ZERO, 8.0);
        (
            everything,
            eng.entities_in_box(Vec3::splat(-1e4), Vec3::splat(1e4)),
            n,
        )
    });
    assert!(after.0.is_empty());
    assert_eq!(after.1, [after.2]);
}

#[test]
fn pvs_uses_entity_leaves() {
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
    let view = Vec3::new(0.0, 0.0, 17.0);
    let eye = ct_spawn + view;
    let middle = (ct_spawn + t_spawn) * 0.5;
    // The long box runs from the viewer's own leaf to the far spawn, which is hidden. (That a
    // box can be seen over a hidden centre and hidden corners is checked on a synthetic map in
    // the unit tests of `world.rs`.)
    assert!(!probe_map.can_see(eye, t_spawn - Vec3::ONE, t_spawn + Vec3::ONE));
    let half = (t_spawn - ct_spawn).abs() * 0.5;

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

    let ids = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&ids);
    h.server.game_mut().act(move |eng| {
        let model = eng.precache_model("models/test.mdl").ok();
        let model = model
            .or_else(|| eng.model_index("models/test.mdl"))
            .unwrap();
        let mut make = |origin: Vec3, half: Vec3| {
            let id = eng.create_entity("test_box").unwrap();
            let v = eng.ent_mut(id).unwrap();
            v.modelindex = model;
            v.solid = SOLID_BBOX;
            eng.set_size(id, -half, half);
            eng.set_origin(id, origin);
            id
        };
        let near = make(ct_spawn, Vec3::splat(16.0));
        let far = make(t_spawn, Vec3::splat(16.0));
        let long = make(middle, half);
        sink.borrow_mut().extend([near, far, long]);
    });
    h.step(&mut [&mut probe], 30);
    let ids = ids.borrow();
    let seen = &h.server.game().add_to_full_pack_ents;
    assert!(seen.contains(&ids[0].index), "the box next to the viewer");
    assert!(!seen.contains(&ids[1].index), "the box at the far spawn");
    assert!(
        seen.contains(&ids[2].index),
        "the long box touches the viewer's leaf at one end"
    );
}

/// Linking with hostile numbers neither panics nor leaves a bound that is not finite.
fn hostile_bounds_link_safely_on(mut h: Harness) {
    run(&mut h, |eng| {
        let id = boxed(eng, SOLID_BBOX, Vec3::new(10.0, 20.0, 30.0), 4.0);
        let good = eng.ent(id).unwrap().clone();
        let nan = Vec3::NAN;
        let bad = [
            (nan, nan),
            (Vec3::splat(-1e30), Vec3::splat(1e30)),
            (Vec3::splat(f32::NEG_INFINITY), Vec3::splat(f32::INFINITY)),
            (Vec3::splat(5.0), Vec3::splat(-5.0)),
            (Vec3::new(0.0, 5.0, 0.0), Vec3::new(1.0, 4.0, 1.0)),
            (Vec3::new(-1.0, -1.0, f32::NAN), Vec3::ONE),
        ];
        for (mins, maxs) in bad {
            eng.set_size(id, mins, maxs);
            let v = eng.ent(id).unwrap();
            assert_eq!((v.mins, v.maxs, v.size), (good.mins, good.maxs, good.size));
            assert_eq!((v.absmin, v.absmax), (good.absmin, good.absmax));
        }
        for origin in [nan, Vec3::splat(1e30), Vec3::splat(f32::INFINITY)] {
            eng.set_origin(id, origin);
            assert_eq!(eng.ent(id).unwrap().origin, good.origin);
        }

        // Values the game wrote itself, past `set_origin`/`set_size`.
        for origin in [
            nan,
            Vec3::splat(1e30),
            Vec3::splat(f32::NEG_INFINITY),
            Vec3::splat(f32::MAX),
        ] {
            for (mins, maxs) in [
                (Vec3::splat(-4.0), Vec3::splat(4.0)),
                (nan, Vec3::splat(4.0)),
                (Vec3::splat(-1e38), Vec3::splat(1e38)),
            ] {
                {
                    let v = eng.ent_mut(id).unwrap();
                    v.origin = origin;
                    v.mins = mins;
                    v.maxs = maxs;
                    v.angles = Vec3::new(0.0, nan.x, 0.0);
                    v.solid = SOLID_BSP;
                    v.modelindex = 1;
                }
                eng.link_entity(id, true);
                let v = eng.ent(id).unwrap();
                assert!(
                    v.absmin.is_finite() && v.absmax.is_finite(),
                    "{origin} {mins} {maxs}"
                );
                let _ = eng.entities_in_box(v.absmin, v.absmax);
            }
        }
        // Hostile queries.
        for q in [nan, Vec3::splat(1e30), Vec3::splat(f32::INFINITY)] {
            let _ = eng.entities_in_box(q, q);
            let _ = eng.entities_in_box(-q, q);
        }
    });
}

#[test]
fn hostile_bounds_link_safely() {
    hostile_bounds_link_safely_on(Harness::new(2));
    if let Some((map, _)) = dust2() {
        let cfg = ServerConfig {
            max_clients: 2,
            ..ServerConfig::default()
        };
        hostile_bounds_link_safely_on(Harness::with_map(cfg, map, TestGame::default()));
    }
}
