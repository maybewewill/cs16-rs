//! Entity think scheduling: when `GameDll::think` is called, and what a think may do to the
//! entities around it.

mod common;

use common::{FRAME, Harness, TestGame, schedule_extra_thinks};
use cs_engine::{MAX_EDICTS_WIRE, SV_TIME_AT_MAP_START, ServerConfig};

/// A harness with one player slot and `extras` entities (indices 2, 3, ...) whose `nextthink`
/// is `nextthink`, in a pool of `max_edicts`.
fn harness(extras: usize, nextthink: f64, max_edicts: usize, game: TestGame) -> Harness {
    let cfg = ServerConfig {
        max_clients: 1,
        max_edicts,
        ..ServerConfig::default()
    };
    let overrides = game.extra_nextthink_overrides.clone();
    let game = TestGame {
        extra_entities: extras,
        extra_nextthink: 0.0,
        extra_nextthink_overrides: Vec::new(),
        ..game
    };
    let mut h = Harness::with_config(cfg, game);
    // Scheduled at the first frame: activation settles the map with passes that would run
    // thinks due that early (see `settle_physics`).
    schedule_extra_thinks(&mut h, nextthink, overrides);
    h
}

fn thinks_of(h: &Harness, index: u16) -> usize {
    h.server
        .game()
        .think_log
        .iter()
        .filter(|(i, _)| *i == index)
        .count()
}

#[test]
fn think_fires_once_when_due() {
    // Server time starts at `SV_TIME_AT_MAP_START`. A think is due when `nextthink <= sv_time
    // + frametime`: `sv_time` is 1.49 in the 50th frame, so the target is between the 49th
    // and the 50th frame's due time and the float sum of the steps does not matter.
    let target = SV_TIME_AT_MAP_START + 0.495;
    let mut h = harness(1, target, 64, TestGame::default());
    h.step(&mut [], 49);
    assert!(h.server.game().think_log.is_empty(), "not due yet");
    h.step(&mut [], 1);
    assert_eq!(h.server.game().think_log.len(), 1);
    let (index, time) = h.server.game().think_log[0];
    assert_eq!(index, 2);
    // `eng.time()` during the think is the time the think was due, not the frame's.
    assert!((time - target).abs() < 1e-9, "{time}");
    // The engine cleared the schedule: no second call.
    assert_eq!(h.server.edicts().by_index(2).unwrap().1.vars.nextthink, 0.0);
    h.step(&mut [], 100);
    assert_eq!(h.server.game().think_log.len(), 1);
}

#[test]
fn rescheduling_inside_think_waits_for_the_next_frame() {
    let game = TestGame {
        think_reschedules: true,
        ..TestGame::default()
    };
    // Due in the 5th frame (`sv_time` 1.04 + frame 0.01), 2 ms ahead of that frame's `sv_time`.
    let mut h = harness(1, SV_TIME_AT_MAP_START + 0.042, 64, game);
    h.step(&mut [], 4);
    assert!(h.server.game().think_log.is_empty());
    h.step(&mut [], 1);
    assert_eq!(h.server.game().think_log.len(), 1, "one call, not a loop");
    h.step(&mut [], 1);
    assert_eq!(h.server.game().think_log.len(), 2);
    h.step(&mut [], 10);
    // One call per frame.
    assert_eq!(h.server.game().think_log.len(), 12);
    // The first call runs at its due time, 8 ms before the next frame's time.
    let times: Vec<f64> = h.server.game().think_log.iter().map(|&(_, t)| t).collect();
    assert!(
        times.windows(2).all(|w| w[1] - w[0] > FRAME / 2.0),
        "{times:?}"
    );
}

#[test]
fn entity_removed_before_its_turn_is_skipped() {
    // Entities 2 (A) and 3 (B) are due in the same frame; A's think removes B.
    let game = TestGame {
        think_removes_index: Some(3),
        ..TestGame::default()
    };
    let mut h = harness(2, 0.01, 64, game);
    h.step(&mut [], 1);
    assert_eq!(h.server.game().think_log.len(), 1);
    assert_eq!(h.server.game().think_log[0].0, 2);
    assert!(h.server.edicts().by_index(3).is_none());
    h.step(&mut [], 5);
    assert_eq!(thinks_of(&h, 3), 0);
}

#[test]
fn entity_removing_itself_is_safe() {
    let game = TestGame {
        think_removes_self: true,
        ..TestGame::default()
    };
    let mut h = harness(2, 0.01, 64, game);
    h.step(&mut [], 5);
    assert_eq!(thinks_of(&h, 2), 1);
    assert_eq!(thinks_of(&h, 3), 1);
    assert!(h.server.edicts().by_index(2).is_none());
    assert!(h.server.edicts().by_index(3).is_none());
}

/// The pass is bounded by the pool, which stops at the wire limit (`MAX_EDICTS_WIRE`): a full
/// pool whose every entity is due thinks in one frame, lowest indices first, and no think
/// repeats.
#[test]
fn a_full_pool_thinks_in_one_frame() {
    // The world and the one client slot take two edicts.
    let total = MAX_EDICTS_WIRE - 2;
    let mut h = harness(total, 0.001, 6000, TestGame::default());
    h.step(&mut [], 1);
    assert_eq!(h.server.game().think_log.len(), total);
    // Lowest indices first.
    let log = &h.server.game().think_log;
    assert!(log.windows(2).all(|w| w[0].0 < w[1].0));
    assert_eq!(log[0].0, 2);
    h.step(&mut [], 3);
    assert_eq!(h.server.game().think_log.len(), total);
}

#[test]
fn nan_nextthink_never_fires() {
    // Entities 2..=5 hold bad values; entity 6 has a finite due think, so the frames do run
    // thinks.
    let bad = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0];
    let mut overrides = bad.to_vec();
    overrides.push(0.01);
    let game = TestGame {
        extra_nextthink_overrides: overrides,
        ..TestGame::default()
    };
    let mut h = harness(5, 0.0, 64, game);
    h.step(&mut [], 20);
    let log = &h.server.game().think_log;
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0].0, 6);
    // The bad values were not rewritten.
    let value = |index| h.server.edicts().by_index(index).unwrap().1.vars.nextthink;
    assert!(value(2).is_nan());
    assert_eq!(value(3), f64::INFINITY);
    assert_eq!(value(4), f64::NEG_INFINITY);
    assert_eq!(value(5), -1.0);
    assert_eq!(value(6), 0.0);
}

#[test]
fn entity_created_above_the_running_one_is_called_in_the_same_pass() {
    // HLDS reads the entity count at every step of the pass, so a child at a higher index than
    // its parent is run in the same pass, and its think (due at once) is called in this frame.
    let game = TestGame {
        think_creates_child: true,
        ..TestGame::default()
    };
    let mut h = harness(1, 0.01, 64, game);
    h.step(&mut [], 1);
    let child = h.server.game().children[0];
    assert_eq!(child, 3);
    let log = &h.server.game().think_log;
    assert_eq!(log.len(), 2, "{log:?}");
    assert_eq!((log[0].0, log[1].0), (2, child));
    h.step(&mut [], 3);
    assert_eq!(h.server.game().think_log.len(), 2);
}

#[test]
fn entity_created_below_the_running_one_waits_for_the_next_frame() {
    // Entities 2 (A) and 3 (B) are due together. A's think removes A itself (so it creates no
    // child); B's think creates a child that takes A's freed slot 2, which this pass has
    // already left.
    let game = TestGame {
        think_removes_index: Some(2),
        think_creates_child: true,
        ..TestGame::default()
    };
    let mut h = harness(2, 0.01, 64, game);
    h.step(&mut [], 1);
    let child = h.server.game().children[0];
    assert_eq!(child, 2, "the child takes A's slot");
    let log = &h.server.game().think_log;
    assert_eq!(log.len(), 2, "{log:?}");
    assert_eq!((log[0].0, log[1].0), (2, 3));
    h.step(&mut [], 1);
    let log = &h.server.game().think_log;
    assert_eq!(log.len(), 3, "{log:?}");
    assert_eq!(log[2].0, child);
    h.step(&mut [], 3);
    assert_eq!(h.server.game().think_log.len(), 3);
}

#[test]
fn slot_reused_during_the_pass_is_not_called_for_the_old_entity() {
    // Entities 2 (A) and 3 (B) are due together. A's think removes B and creates a due child;
    // a slot freed at server time 2.0 or earlier is reused at once, so the child takes B's
    // slot. B is not called, and the child, run when the pass reaches slot 3, is called once.
    let game = TestGame {
        think_removes_index: Some(3),
        think_creates_child: true,
        ..TestGame::default()
    };
    let mut h = harness(2, 0.01, 64, game);
    h.step(&mut [], 1);
    let child = h.server.game().children[0];
    assert_eq!(child, 3, "the child takes B's slot");
    let log = &h.server.game().think_log;
    assert_eq!(log.len(), 2, "{log:?}");
    assert_eq!((log[0].0, log[1].0), (2, 3));
    h.step(&mut [], 3);
    assert_eq!(h.server.game().think_log.len(), 2);
}
