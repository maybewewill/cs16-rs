//! Lag compensation (`sv_unlag`): while the game runs a command of a client with `cl_lw` and
//! `cl_lc`, the other players stand where that client saw them, and afterwards they are exactly
//! where they were.
//!
//! The rig: two players, the shooter (slot 0, at y = 500) and the target (slot 1) who runs along
//! the x axis at [`SPEED`] units per second, its place a function of the server's real time. The
//! shooter's ping is made by holding the server back from reading for `hold` frames while the
//! rest of the server runs: the command that was sent first is read `hold` frames later, with
//! the acknowledgement of the update it saw. The test game records what the shooter's command
//! saw.

mod common;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use common::{FRAME, Harness, TestGame};
use cs_engine::collision::Hull;
use cs_engine::{EngineApi, MoveFilter, ServerConfig};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_transport::{LoopbackEndpoint, PacketTransport, PeerAddr, TransportError};
use glam::Vec3;

type Probe = ProbeClient<LoopbackEndpoint>;

/// Units per second the target runs at.
const SPEED: f32 = 300.0;
/// How far the target jumps when it teleports.
const JUMP: f32 = 500.0;
/// Slack, in units, between where the rewind should put the target and where it did: a frame
/// and a half of the target's movement.
const TOLERANCE: f32 = 4.5;

/// A transport the server reads nothing from while `held`: what the clients send waits.
struct Holdable<'a> {
    hub: &'a mut cs_transport::LoopbackHub,
    held: bool,
}

impl PacketTransport for Holdable<'_> {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        if self.held { None } else { self.hub.recv() }
    }

    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), TransportError> {
        self.hub.send(to, data)
    }
}

#[derive(Clone, Default)]
struct Scenario {
    /// `lerp_msec` of the shooter's commands.
    lerp: i16,
    /// Frames the server reads nothing before the measured command (that many frames of ping).
    hold: usize,
    /// The shooter's `cl_updaterate`; 100 when 0.
    update_rate: u32,
    /// The target teleports this many seconds before the measured command.
    jump_before: Option<f64>,
    /// The target dies this many seconds before the measured command.
    die_before: Option<f64>,
    cvars: Vec<(&'static str, &'static str)>,
    shooter_info: Vec<(&'static str, &'static str)>,
}

/// What the measured command saw and what became of the target.
struct Outcome {
    /// Server real time of the measured command.
    time: f64,
    /// Where the target really was when the command ran.
    true_x: f32,
    /// The target's place (`origin`, `absmin`, `absmax`) during the command.
    seen: (Vec3, Vec3, Vec3),
    /// The game's `trace_line` across the target's seen place, and across its true place.
    line_hits_seen: bool,
    line_hits_true: bool,
    /// The game's movement trace across the target's seen place, and across its true place.
    pmove_hits_seen: bool,
    pmove_hits_true: bool,
    /// The target's place (as above) before and after the command.
    before: (Vec3, Vec3, Vec3),
    after: (Vec3, Vec3, Vec3),
    /// What `entities_in_box` finds around the true place and around the seen place afterwards.
    found_at_true: bool,
    found_at_seen: bool,
    /// The target's x in the update the shooter got after the command.
    shown_to_shooter_x: f32,
    /// The target's own x as seen by its own command, run in the same frame after the
    /// shooter's.
    target_saw_self_x: f32,
    /// `pos` of the scenario, to compute where the rewind should have put the target.
    pos: Rc<dyn Fn(f64) -> f32>,
}

fn joined(h: &mut Harness, name: &str) -> Probe {
    let mut probe = h.add_probe(name);
    h.step(&mut [&mut probe], 200);
    assert_eq!(probe.state(), &ProbeState::InGame);
    probe
}

fn cmd(msec: u8, lerp: i16) -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        msec,
        lerp_msec: lerp,
        ..Default::default()
    }
}

fn places(h: &Harness, slot: u8) -> (Vec3, Vec3, Vec3) {
    let v = &h.server.edicts().get(h.server.edicts().client(slot)).unwrap().vars;
    (v.origin, v.absmin, v.absmax)
}

fn same_bits(a: (Vec3, Vec3, Vec3), b: (Vec3, Vec3, Vec3)) -> bool {
    let bits = |v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
    bits(a.0) == bits(b.0) && bits(a.1) == bits(b.1) && bits(a.2) == bits(b.2)
}

/// Sets the target's origin to `x` along its line, and its health.
fn put_target(h: &mut Harness, x: f32, health: f32) {
    h.server.game_mut().act(move |eng| {
        let id = eng.client_edict(1);
        eng.ent_mut(id).unwrap().health = health;
        eng.set_origin(id, Vec3::new(x, 0.0, 0.0));
    });
}

fn run(scn: &Scenario) -> Outcome {
    let game = TestGame {
        spawn_origins: vec![Vec3::new(0.0, 500.0, 0.0), Vec3::new(0.0, 0.0, 0.0)],
        ..TestGame::default()
    };
    let cfg = ServerConfig {
        max_clients: 2,
        max_edicts: 64,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_config(cfg, game);
    for &(name, value) in &scn.cvars {
        h.server.game_mut().act(move |eng| eng.set_cvar(name, value));
    }
    let mut shooter = joined(&mut h, "Shooter");
    let mut target = joined(&mut h, "Target");
    h.server.game_mut().act(|eng| {
        for slot in 0..2 {
            let id = eng.client_edict(slot);
            eng.set_size(id, Vec3::splat(-16.0), Vec3::splat(16.0));
        }
    });
    // An update for the shooter every frame, so that its history has no gaps.
    let rate = if scn.update_rate == 0 { 100 } else { scn.update_rate };
    shooter.send_stringcmd(&format!("setinfo cl_updaterate {rate}"));
    for (key, value) in &scn.shooter_info {
        shooter.send_stringcmd(&format!("setinfo {key} {value}"));
    }
    h.step(&mut [&mut shooter, &mut target], 5);

    // The schedule: the measured command is sent at the first pump after `now0` and read at
    // `time`; the target's place is a function of the time of the frame that sets it.
    let now0 = h.now + 40.0 * FRAME;
    let time = now0 + (scn.hold + 1) as f64 * FRAME;
    let t_start = h.now;
    let jump_at = scn.jump_before.map(|s| time - s);
    let die_at = scn.die_before.map(|s| time - s);
    let pos: Rc<dyn Fn(f64) -> f32> = Rc::new(move |t| {
        let jumped = jump_at.is_some_and(|j| t >= j);
        SPEED * (t - t_start) as f32 + if jumped { JUMP } else { 0.0 }
    });
    let health_at = move |t: f64| if die_at.is_some_and(|d| t >= d) { 0.0 } else { 100.0 };

    // Warm up: the target runs and the shooter sends commands with its `lerp_msec`.
    while h.now + FRAME < now0 - 1e-9 {
        let t = h.now + FRAME;
        put_target(&mut h, pos(t), health_at(t));
        shooter.queue_cmd(cmd(10, scn.lerp));
        h.step(&mut [&mut shooter, &mut target], 1);
    }
    h.now = now0 - FRAME;
    put_target(&mut h, pos(now0), health_at(now0));
    shooter.queue_cmd(cmd(10, scn.lerp));
    h.step(&mut [&mut shooter, &mut target], 1);
    assert!((h.now - now0).abs() < 1e-9);

    // The measured command.
    let true_x = pos(now0 + scn.hold as f64 * FRAME);
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&seen);
    let marks = Rc::new(Cell::new(0));
    let own_x = Rc::new(Cell::new(f32::NAN));
    h.server.game_mut().usercmd_hook.set({
        let marks = Rc::clone(&marks);
        let own_x = Rc::clone(&own_x);
        move |eng: &mut EngineApi<'_>, slot| {
            if slot == 1 {
                own_x.set(eng.ent(eng.client_edict(1)).unwrap().origin.x);
            }
            if slot != 0 {
                return;
            }
            marks.set(marks.get() + 1);
            let (me, other) = (eng.client_edict(0), eng.client_edict(1));
            let v = eng.ent(other).unwrap();
            let place = (v.origin, v.absmin, v.absmax);
            let across = |x: f32| (Vec3::new(x, -200.0, 0.0), Vec3::new(x, 200.0, 0.0));
            let line = |eng: &EngineApi<'_>, x: f32| {
                let (a, b) = across(x);
                eng.trace_line(a, b, MoveFilter::Normal, Some(me)).ent == Some(other)
            };
            let pmove = |eng: &EngineApi<'_>, x: f32| {
                let (a, b) = across(x);
                eng.trace_player_move(me, a, b, Hull::Point).ent == Some(other)
            };
            sink.borrow_mut().push((
                place,
                line(eng, place.0.x),
                line(eng, true_x),
                pmove(eng, place.0.x),
                pmove(eng, true_x),
            ));
        }
    });
    shooter.queue_cmd(cmd(10, scn.lerp));
    target.queue_cmd(cmd(10, 0));
    let mut before = places(&h, 1);
    let mut tail = 0;
    for i in 0..=scn.hold {
        let pump_at = now0 + (i + 1) as f64 * FRAME;
        let frame_at = pump_at;
        if i < scn.hold {
            put_target(&mut h, pos(frame_at), health_at(frame_at));
        } else {
            before = places(&h, 1);
        }
        h.now = pump_at;
        shooter.pump(pump_at);
        target.pump(pump_at);
        let mut net = Holdable {
            hub: &mut h.hub,
            held: i < scn.hold,
        };
        h.server.frame(&mut net, frame_at);
        h.now = frame_at;
        tail = i;
    }
    assert_eq!(tail, scn.hold);
    assert!((h.now - time).abs() < 1e-9, "{} {}", h.now, time);
    assert_eq!(marks.get(), 1, "the measured command ran once");

    let after = places(&h, 1);
    let found = Rc::new(Cell::new((false, false)));
    let sink = Rc::clone(&found);
    let (seen_place, line_seen, line_true, pmove_seen, pmove_true) = seen.borrow()[0];
    let (true_origin, seen_origin) = (before.0.x, seen_place.0.x);
    h.server.game_mut().act(move |eng| {
        let id = eng.client_edict(1);
        let around = |x: f32| {
            eng.entities_in_box(Vec3::new(x - 1.0, -1.0, -1.0), Vec3::new(x + 1.0, 1.0, 1.0))
                .contains(&id)
        };
        sink.set((around(true_origin), around(seen_origin)));
    });
    h.step(&mut [&mut shooter, &mut target], 2);
    Outcome {
        time,
        true_x,
        seen: seen_place,
        line_hits_seen: line_seen,
        line_hits_true: line_true,
        pmove_hits_seen: pmove_seen,
        pmove_hits_true: pmove_true,
        before,
        after,
        found_at_true: found.get().0,
        found_at_seen: found.get().1,
        shown_to_shooter_x: shooter.entities().get(&2).map_or(f32::NAN, |s| s.origin.x),
        target_saw_self_x: own_x.get(),
        pos,
    }
}
/// Where the rewind should put the target for a shooter whose ping is `latency` seconds, whose
/// `lerp_msec` is `lerp` and whose update interval is `interval` (the original's
/// `SV_GetUnlagTargetTime`, with `sv_maxunlag` 0.5).
fn expected_x(o: &Outcome, latency: f64, lerp: i16, interval: f64) -> f32 {
    let latency = latency.min(0.5);
    // `lerp_msec` is 9 bits on the wire: what does not fit arrives as a large number.
    let lerp = if (0..=511).contains(&lerp) { lerp } else { 511 };
    let interp = (f64::from(lerp) / 1000.0).min(0.1).max(interval);
    (o.pos)(o.time - latency - interp)
}


/// The ping of a shooter held back for `hold` frames, with an update every frame.
fn ping(hold: usize) -> f64 {
    hold as f64 * FRAME
}

#[test]
fn shooter_sees_target_where_its_client_saw_it() {
    // 100 ms of ping and 50 ms of interpolation: the target is where it was 0.15 s before.
    let scn = Scenario {
        lerp: 50,
        hold: 10,
        ..Scenario::default()
    };
    let o = run(&scn);
    let expected = expected_x(&o, ping(10), 50, FRAME);
    assert!(
        (o.seen.0.x - expected).abs() <= TOLERANCE,
        "seen {} expected {expected} (true {})",
        o.seen.0.x,
        o.true_x
    );
    // It is a real rewind: about 0.16 s of running.
    assert!(o.true_x - o.seen.0.x > SPEED * 0.13, "{} {}", o.seen.0.x, o.true_x);
    // The game's traces hit where the target was seen, not where it is.
    assert!(o.line_hits_seen && !o.line_hits_true);
    // The movement code meets the target where it really is.
    assert!(o.pmove_hits_true && !o.pmove_hits_seen);
    // The box moved with the origin (linked: the same margin around the player's box).
    assert_eq!(o.seen.1.x, o.seen.0.x - 17.0);
    assert_eq!(o.seen.2.x, o.seen.0.x + 17.0);
    assert_eq!(o.before.1.x, o.before.0.x - 17.0);

    // A longer interpolation delay sees the target further back.
    let slower = run(&Scenario { lerp: 100, ..scn.clone() });
    assert!((slower.seen.0.x - expected_x(&slower, ping(10), 100, FRAME)).abs() <= TOLERANCE);
    assert!(slower.seen.0.x < o.seen.0.x - SPEED * 0.04, "{} {}", slower.seen.0.x, o.seen.0.x);
}

#[test]
fn unlag_off_traces_current_positions() {
    let base = Scenario {
        lerp: 50,
        hold: 10,
        ..Scenario::default()
    };
    let off = [
        Scenario { cvars: vec![("sv_unlag", "0")], ..base.clone() },
        Scenario { shooter_info: vec![("cl_lc", "0")], ..base.clone() },
        Scenario { shooter_info: vec![("cl_lw", "0")], ..base.clone() },
    ];
    for scn in &off {
        let o = run(scn);
        assert_eq!(o.seen.0.x, o.true_x);
        assert!(o.line_hits_true && o.line_hits_seen);
        assert!(o.pmove_hits_true && o.pmove_hits_seen);
    }
    // With it on, the same rig does rewind.
    let on = run(&base);
    assert!(on.seen.0.x < on.true_x - SPEED * 0.1);
}

#[test]
fn unlag_bounds_and_restores_exactly() {
    let cases = [
        // No ping at all.
        Scenario { lerp: i16::MAX, ..Scenario::default() },
        Scenario { lerp: -1, ..Scenario::default() },
        // 100 ms of ping.
        Scenario { lerp: i16::MAX, hold: 10, ..Scenario::default() },
        Scenario { lerp: -1, hold: 10, ..Scenario::default() },
        // 5 s of ping, with updates slow enough (10 a second) for the frames to span it.
        Scenario { lerp: i16::MAX, hold: 500, update_rate: 10, ..Scenario::default() },
        Scenario { lerp: -1, hold: 500, update_rate: 10, ..Scenario::default() },
    ];
    for scn in &cases {
        let o = run(scn);
        let interval = match scn.update_rate {
            0 => FRAME,
            rate => 1.0 / f64::from(rate),
        };
        // Never behind `sv_maxunlag` (0.5 s) plus the longest interpolation (0.1 s), and
        // never ahead of the true place.
        let deepest = o.true_x - SPEED * 0.61 - TOLERANCE;
        assert!(o.seen.0.x >= deepest, "hold {}: seen {} true {}", scn.hold, o.seen.0.x, o.true_x);
        assert!(o.seen.0.x <= o.true_x, "seen {} true {}", o.seen.0.x, o.true_x);
        // The rewind is where the formula puts it: the ping capped, plus the interpolation
        // delay `lerp_msec` gives, capped at 0.1 s, but at least the update interval.
        let expected = expected_x(&o, ping(scn.hold), scn.lerp, interval);
        assert!(
            (o.seen.0.x - expected).abs() <= TOLERANCE,
            "hold {} lerp {}: seen {} expected {expected}",
            scn.hold,
            scn.lerp,
            o.seen.0.x
        );
        // Afterwards the target is exactly where it was, and linked there.
        assert!(same_bits(o.before, o.after), "{:?} {:?}", o.before, o.after);
        assert!(o.found_at_true);
        assert!(!o.found_at_seen || o.seen.0.x == o.before.0.x);
    }
}

#[test]
fn rewind_stops_at_the_history() {
    // 0.62 s of ping and no `sv_maxunlag`: with 0.1 s of interpolation the target time is
    // 0.72 s back, older than the 64 updates kept (0.64 s), so nothing is rewound.
    let o = run(&Scenario {
        lerp: 100,
        hold: 62,
        cvars: vec![("sv_maxunlag", "0")],
        ..Scenario::default()
    });
    assert_eq!(o.seen.0.x, o.true_x);
    assert!(same_bits(o.before, o.after));
    // A little less ping and it is within the history: rewound by all of it.
    let o = run(&Scenario {
        lerp: 100,
        hold: 50,
        cvars: vec![("sv_maxunlag", "0")],
        ..Scenario::default()
    });
    assert!((o.seen.0.x - (o.pos)(o.time - ping(50) - 0.1)).abs() <= TOLERANCE);
    assert!(o.true_x - o.seen.0.x > SPEED * 0.55);
}

#[test]
fn teleported_or_dead_targets_are_not_interpolated() {
    let base = Scenario {
        lerp: 50,
        hold: 10,
        ..Scenario::default()
    };
    // A jump after the rewind's target time: the frames the rewind walks span it, so the
    // target stays where it is.
    let o = run(&Scenario { jump_before: Some(0.08), ..base.clone() });
    assert_eq!(o.seen.0.x, o.true_x, "teleported");
    assert!(same_bits(o.before, o.after));
    // A jump long before it is behind every frame the rewind looks at.
    let o = run(&Scenario { jump_before: Some(0.5), ..base.clone() });
    let expected = expected_x(&o, ping(10), 50, FRAME);
    assert!((o.seen.0.x - expected).abs() <= TOLERANCE, "{} {expected}", o.seen.0.x);
    assert!(o.true_x - o.seen.0.x > SPEED * 0.13);
    // A dead target is left alone.
    let o = run(&Scenario { die_before: Some(0.08), ..base.clone() });
    assert_eq!(o.seen.0.x, o.true_x, "dead");
}

#[test]
fn the_rewind_does_not_reach_packets_or_other_commands() {
    let o = run(&Scenario {
        lerp: 50,
        hold: 10,
        ..Scenario::default()
    });
    assert!(o.true_x - o.seen.0.x > SPEED * 0.13);
    // What the shooter is sent after the command is where the target really is.
    assert!(
        (o.shown_to_shooter_x - o.after.0.x).abs() < 0.2,
        "{} {}",
        o.shown_to_shooter_x,
        o.after.0.x
    );
    // The target's own command, run right after the shooter's, finds the target at home.
    assert_eq!(o.target_saw_self_x, o.true_x);
}
