//! Touches: when `GameDll::touch` is called for a trigger, what a touch may do to the entities
//! around it, and the bound on a frame's touches. (The impact side, which the physics calls, is
//! tested inside `src/touch.rs`.)

mod common;

use common::{Harness, TestGame};
use cs_engine::{EdictId, EngineApi, MAX_TOUCHES_PER_FRAME, ServerConfig, consts};
use glam::Vec3;

const SOLID_TRIGGER: u8 = consts::SOLID_TRIGGER;
const SOLID_BBOX: u8 = consts::SOLID_BBOX;

fn harness(game: TestGame) -> Harness {
    let cfg = ServerConfig {
        max_clients: 1,
        max_edicts: 2048,
        ..ServerConfig::default()
    };
    Harness::with_config(cfg, game)
}

/// A trigger box of half-size `half` around `origin`.
fn trigger(eng: &mut EngineApi<'_>, class: &str, origin: Vec3, half: f32) -> EdictId {
    let id = eng.create_entity(class).expect("free edict");
    eng.ent_mut(id).unwrap().solid = SOLID_TRIGGER;
    eng.set_size(id, Vec3::splat(-half), Vec3::splat(half));
    eng.set_origin(id, origin);
    id
}

/// A solid box of half-size 8 at `origin`, linked without asking for touches.
fn body(eng: &mut EngineApi<'_>, class: &str, origin: Vec3) -> EdictId {
    let id = eng.create_entity(class).expect("free edict");
    eng.ent_mut(id).unwrap().solid = SOLID_BBOX;
    eng.set_size(id, Vec3::splat(-8.0), Vec3::splat(8.0));
    eng.set_origin(id, origin);
    id
}

#[test]
fn linking_into_a_trigger_touches_it_once() {
    let mut h = harness(TestGame::default());
    h.server.game_mut().act(|eng| {
        trigger(eng, "trigger", Vec3::new(0.0, 0.0, 500.0), 64.0);
        let ent = body(eng, "ent", Vec3::new(0.0, 0.0, 500.0));
        eng.link_entity(ent, true);
    });
    h.step(&mut [], 1);
    let log = h.server.game().touch_log.clone();
    assert_eq!(log.len(), 1, "{log:?}");
    let (toucher, other) = log[0];
    let class = |i| h.server.edicts().by_index(i).unwrap().1.vars.classname.clone();
    assert_eq!((class(toucher).as_str(), class(other).as_str()), ("trigger", "ent"));
    // Nothing stays queued for later frames.
    h.step(&mut [], 5);
    assert_eq!(h.server.game().touch_log.len(), 1);
}

#[test]
fn removal_inside_touch_is_safe() {
    // Two triggers hold one entity; the first touch removes both sides, so the second pair
    // (the other trigger and the removed entity) is skipped.
    let game = TestGame {
        touch_removes_both: true,
        ..TestGame::default()
    };
    let mut h = harness(game);
    h.server.game_mut().act(|eng| {
        trigger(eng, "t1", Vec3::new(0.0, -500.0, 500.0), 600.0);
        trigger(eng, "t2", Vec3::new(0.0, 500.0, 500.0), 600.0);
        let ent = body(eng, "ent", Vec3::new(0.0, 0.0, 500.0));
        eng.link_entity(ent, true);
    });
    h.step(&mut [], 1);
    assert_eq!(h.server.game().touch_log.len(), 1);
    h.step(&mut [], 3);
    assert_eq!(h.server.game().touch_log.len(), 1);
}

#[test]
fn touches_inside_touch_are_dispatched_in_the_same_frame() {
    let game = TestGame {
        touch_links_class: Some("follower".to_string()),
        ..TestGame::default()
    };
    let mut h = harness(game);
    h.server.game_mut().act(|eng| {
        trigger(eng, "trigger", Vec3::new(0.0, 0.0, 500.0), 64.0);
        let mover = body(eng, "mover", Vec3::new(0.0, 0.0, 500.0));
        body(eng, "follower", Vec3::new(10.0, 0.0, 500.0));
        eng.link_entity(mover, true);
    });
    h.step(&mut [], 1);
    let log = h.server.game().touch_log.clone();
    assert_eq!(log.len(), 2, "{log:?}");
    let class = |i| h.server.edicts().by_index(i).unwrap().1.vars.classname.clone();
    assert_eq!(class(log[0].1), "mover");
    assert_eq!(class(log[1].1), "follower");
    assert_eq!(log[0].0, log[1].0, "both by the one trigger");
}

/// One trigger holding two entities; the first touch changes the trigger, so the pair already
/// queued for the second entity is skipped.
fn two_in_a_trigger(game: TestGame) -> Harness {
    let mut h = harness(game);
    h.server.game_mut().act(|eng| {
        trigger(eng, "trigger", Vec3::new(0.0, 0.0, 500.0), 64.0);
        for _ in 0..2 {
            let ent = body(eng, "ent", Vec3::new(0.0, 0.0, 500.0));
            eng.link_entity(ent, true);
        }
    });
    h.step(&mut [], 1);
    h
}

#[test]
fn killed_trigger_gets_no_further_touches() {
    let h = two_in_a_trigger(TestGame {
        touch_kills_toucher: true,
        ..TestGame::default()
    });
    assert_eq!(h.server.game().touch_log.len(), 1);
}

#[test]
fn trigger_made_non_solid_gets_no_further_touches() {
    let h = two_in_a_trigger(TestGame {
        touch_unsolids_toucher: true,
        ..TestGame::default()
    });
    assert_eq!(h.server.game().touch_log.len(), 1);
}

#[test]
fn both_triggers_still_touch_when_nothing_changes() {
    let h = two_in_a_trigger(TestGame::default());
    assert_eq!(h.server.game().touch_log.len(), 2);
}

#[test]
fn touch_storm_is_bounded() {
    // 2000 entities in one trigger, each relinking itself when touched: the chain would never
    // end without the per-frame bound.
    let game = TestGame {
        touch_relinks_other: true,
        ..TestGame::default()
    };
    let mut h = harness(game);
    h.server.game_mut().act(|eng| {
        trigger(eng, "trigger", Vec3::new(0.0, 0.0, 500.0), 64.0);
        for _ in 0..2000 {
            let ent = body(eng, "ent", Vec3::new(0.0, 0.0, 500.0));
            eng.link_entity(ent, true);
        }
    });
    h.step(&mut [], 1);
    assert_eq!(h.server.game().touch_log.len(), MAX_TOUCHES_PER_FRAME);
    let stats = h.server.net_stats();
    assert_eq!(stats.touch_cap_frames, 1, "one warning for the frame");
    assert!(stats.touches_dropped > 0);
    // The rest was dropped, not carried over.
    h.step(&mut [], 3);
    assert_eq!(h.server.game().touch_log.len(), MAX_TOUCHES_PER_FRAME);
    assert_eq!(h.server.net_stats().touch_cap_frames, 1);

    // A new frame has a fresh budget: the storm runs again, capped again, warned once more.
    h.server.game_mut().act(|eng| {
        let mut after = None;
        while let Some(id) = eng.find_by_classname(after, "ent") {
            eng.link_entity(id, true);
            after = Some(id);
        }
    });
    h.step(&mut [], 1);
    assert_eq!(h.server.game().touch_log.len(), 2 * MAX_TOUCHES_PER_FRAME);
    assert_eq!(h.server.net_stats().touch_cap_frames, 2);
}
