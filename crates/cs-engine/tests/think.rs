//! Entity think scheduling: when `GameDll::think` is called, and what a think may do to the
//! entities around it.

mod common;

use common::{FRAME, Harness, TestGame};
use cs_engine::{MAX_THINKS_PER_FRAME, ServerConfig};

/// A harness with one player slot and `extras` entities (indices 2, 3, ...) whose `nextthink`
/// is `nextthink`, in a pool of `max_edicts`.
fn harness(extras: usize, nextthink: f64, max_edicts: usize, game: TestGame) -> Harness {
    let cfg = ServerConfig {
        max_clients: 1,
        max_edicts,
        ..ServerConfig::default()
    };
    let game = TestGame {
        extra_entities: extras,
        extra_nextthink: nextthink,
        ..game
    };
    Harness::with_config(cfg, game)
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
    let mut h = harness(1, 0.5, 64, TestGame::default());
    h.step(&mut [], 49);
    assert!(h.server.game().think_log.is_empty(), "0.49 s: not due yet");
    h.step(&mut [], 1);
    assert_eq!(h.server.game().think_log.len(), 1);
    let (index, time) = h.server.game().think_log[0];
    assert_eq!(index, 2);
    // `eng.time()` during the think is the frame's server time.
    assert!((time - h.now).abs() < 1e-9 && time >= 0.5 - 1e-9, "{time}");
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
    let mut h = harness(1, 0.05, 64, game);
    h.step(&mut [], 4);
    assert!(h.server.game().think_log.is_empty());
    h.step(&mut [], 1);
    assert_eq!(h.server.game().think_log.len(), 1, "one call, not a loop");
    h.step(&mut [], 1);
    assert_eq!(h.server.game().think_log.len(), 2);
    h.step(&mut [], 10);
    // One call per frame.
    assert_eq!(h.server.game().think_log.len(), 12);
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

#[test]
fn thinks_are_capped_per_frame() {
    let total = 5000;
    let mut h = harness(total, 0.001, 6000, TestGame::default());
    h.step(&mut [], 1);
    let first = h.server.game().think_log.len();
    assert_eq!(first, MAX_THINKS_PER_FRAME);
    // Lowest indices first.
    let log = &h.server.game().think_log;
    assert!(log.windows(2).all(|w| w[0].0 < w[1].0));
    assert_eq!(log[0].0, 2);
    h.step(&mut [], 1);
    assert_eq!(h.server.game().think_log.len(), total);
    let log = &h.server.game().think_log;
    assert!(
        log[first..]
            .iter()
            .all(|&(i, _)| usize::from(i) >= 2 + MAX_THINKS_PER_FRAME)
    );
    h.step(&mut [], 2);
    assert_eq!(h.server.game().think_log.len(), total);
}

#[test]
fn nan_nextthink_never_fires() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
        let mut h = harness(1, bad, 64, TestGame::default());
        h.step(&mut [], 20);
        assert!(h.server.game().think_log.is_empty(), "{bad}");
    }
}
