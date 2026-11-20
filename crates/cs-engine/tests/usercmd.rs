//! User commands and string commands of in-game `cs-probe` clients against `Server<TestGame>`
//! over the loopback transport.

mod common;

use common::Harness;
use cs_engine::{ClientState, MAX_CONSECUTIVE_BAD_MOVES, STRINGCMD_BURST, STRINGCMD_RATE_PER_SEC};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::info::InfoString;
use cs_transport::{FaultStats, Faults, LoopbackEndpoint, PacketTransport, PeerAddr};

type Probe = ProbeClient<LoopbackEndpoint>;

/// Adds a probe and runs the harness until it is in the game.
fn joined(h: &mut Harness, name: &str) -> Probe {
    let mut probe = h.add_probe(name);
    h.step(&mut [&mut probe], 200);
    assert_eq!(probe.state(), &ProbeState::InGame);
    probe
}

fn origin_x(h: &Harness, slot: u8) -> f32 {
    let edicts = h.server.edicts();
    edicts
        .get(edicts.client(slot))
        .expect("client edict")
        .vars
        .origin
        .x
}

fn forward(msec: u8) -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        msec,
        forwardmove: 250.0,
        ..Default::default()
    }
}

/// Queues `cmd` once per frame for `frames` frames, then lets 10 idle frames pass.
fn drive(h: &mut Harness, probe: &mut Probe, cmd: &GoldSrcUserCmd, frames: usize) {
    for _ in 0..frames {
        probe.queue_cmd(cmd.clone());
        h.step(&mut [&mut *probe], 1);
    }
    h.step(&mut [probe], 10);
}

#[test]
fn forward_cmds_move_the_player() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "Player");
    let start = origin_x(&h, 0);
    let seeds_before = h.server.game().random_seeds.len();

    // 100 commands of 10 ms: one second of walking at 250 units per second.
    drive(&mut h, &mut probe, &forward(10), 100);
    let moved = origin_x(&h, 0) - start;
    assert!((moved - 250.0).abs() <= 1.0, "moved {moved}");

    // Working assumption: the seed is the packet's sequence minus the command's index from
    // the newest, so successive commands get successive seeds.
    let seeds = &h.server.game().random_seeds[seeds_before..];
    assert_eq!(seeds.len(), 100);
    assert!(seeds.windows(2).all(|w| w[1] == w[0] + 1), "{seeds:?}");
}

#[test]
fn speedhack_is_bounded_by_real_time() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "Player");
    let start = origin_x(&h, 0);

    // 100 commands per second claiming 20 ms each: twice real time, for two seconds.
    drive(&mut h, &mut probe, &forward(20), 200);
    let moved = origin_x(&h, 0) - start;
    assert!(moved <= 250.0 * (2.0 + 0.5), "moved {moved}");
    // Real time itself is granted.
    assert!(moved >= 250.0 * 2.0, "moved {moved}");
}

/// Distance walked in two seconds of 10 ms commands, with `faults` on the probe's endpoint.
fn walk_two_seconds(faults: Faults) -> (f32, FaultStats) {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "Player");
    probe.transport_mut().set_faults(faults);
    let start = origin_x(&h, 0);
    drive(&mut h, &mut probe, &forward(10), 200);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    (origin_x(&h, 0) - start, probe.transport().fault_stats())
}

#[test]
fn movement_continues_under_loss() {
    let (clean, _) = walk_two_seconds(Faults::default());
    let (lossy, stats) = walk_two_seconds(Faults {
        drop_every: 3,
        ..Faults::default()
    });
    assert!(stats.dropped > 0, "{stats:?}");
    assert!(
        (lossy - clean).abs() < clean * 0.05,
        "clean {clean}, lossy {lossy}"
    );
}

#[test]
fn bad_move_checksum_is_ignored_then_client_dropped() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "Player");
    let start = origin_x(&h, 0);
    let runs = h.server.game().run_usercmd_calls;

    // The only command that would move the player travels in the corrupted packet.
    probe.corrupt_next_moves(1);
    probe.queue_cmd(forward(10));
    h.step(&mut [&mut probe], 20);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(origin_x(&h, 0), start);
    assert_eq!(h.server.game().run_usercmd_calls, runs);

    // One short of the limit, then good moves: the count starts over.
    probe.corrupt_next_moves(MAX_CONSECUTIVE_BAD_MOVES - 1);
    h.step(&mut [&mut probe], 60);
    assert_eq!(h.server.client_state(0), ClientState::Active);

    probe.corrupt_next_moves(MAX_CONSECUTIVE_BAD_MOVES);
    h.step(&mut [&mut probe], 60);
    assert_eq!(h.server.client_state(0), ClientState::Free);
    assert_eq!(h.server.client_count(), 0);
    assert_eq!(h.server.game().client_disconnect_calls, 1);
}

/// 64-bit LCG (Knuth's MMIX constants); each step yields the state's high byte.
struct Lcg(u64);

impl Lcg {
    fn next_byte(&mut self) -> u8 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 56) as u8
    }
}

#[test]
fn garbage_from_connected_address_never_panics() {
    let mut h = Harness::new(4);
    let mut target = h.add_probe("Target");
    let mut other = h.add_probe("Other");
    h.step(&mut [&mut target, &mut other], 200);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.client_state(1), ClientState::Active);
    let updates_before = other.stats().full_updates;

    let mut rng = Lcg(0xC516);
    for i in 0..10_000usize {
        let len = i % 65;
        let mut data: Vec<u8> = (0..len).map(|_| rng.next_byte()).collect();
        if i % 3 == 0 && len >= 8 {
            // A netchan header with a sequence just above the client's, so the server's
            // channel accepts the packet and the payload parser sees the random body.
            let flags = u32::from(rng.next_byte() & 0xC0) << 24;
            let ahead = u32::from(rng.next_byte() % 4);
            let seq = target.outgoing_sequence().expect("connected") + ahead;
            let ack = target.incoming_sequence().expect("connected");
            data[..4].copy_from_slice(&(seq | flags).to_le_bytes());
            data[4..8].copy_from_slice(&ack.to_le_bytes());
        }
        target
            .transport_mut()
            .send(PeerAddr::Loopback(0), &data)
            .expect("loopback send");
        if i % 100 == 99 {
            h.step(&mut [&mut target, &mut other], 1);
        }
    }
    h.step(&mut [&mut target, &mut other], 50);

    // The target may have been dropped for unparsable payloads; either outcome is fine.
    assert!(
        matches!(
            h.server.client_state(0),
            ClientState::Active | ClientState::Free
        ),
        "{:?}",
        h.server.client_state(0)
    );
    assert_eq!(h.server.client_state(1), ClientState::Active);
    assert_eq!(other.state(), &ProbeState::InGame);
    assert!(other.stats().full_updates > updates_before + 100);
    assert_eq!(other.stats().parse_errors, 0);
}

#[test]
fn stringcmd_flood_is_throttled() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "Player");
    let before = h.server.game().client_command_calls;
    let burst = STRINGCMD_BURST as usize;

    let queued_at = h.now;
    for _ in 0..5000 {
        probe.send_stringcmd("say x");
    }
    // The commands travel as reliable fragments over several frames.
    for _ in 0..3000 {
        if h.server.game().client_command_calls - before >= burst {
            break;
        }
        h.step(&mut [&mut probe], 1);
    }
    h.step(&mut [&mut probe], 10);

    let elapsed = h.now - queued_at;
    let calls = h.server.game().client_command_calls - before;
    assert!(calls >= burst, "{calls} commands reached the game");
    assert!(
        calls as f64 <= STRINGCMD_BURST + STRINGCMD_RATE_PER_SEC * elapsed,
        "{calls} commands reached the game in {elapsed} s"
    );
    assert_eq!(h.server.client_state(0), ClientState::Active);
}

#[test]
fn unknown_stringcmd_reaches_game() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "Player");
    probe.send_stringcmd("jointeam 2");
    h.step(&mut [&mut probe], 20);
    let game = h.server.game();
    assert_eq!(
        game.commands,
        vec![vec!["jointeam".to_string(), "2".to_string()]]
    );
}

#[test]
fn setinfo_updates_userinfo_and_notifies_game() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "Player");
    probe.send_stringcmd("setinfo model gign");
    // Ignored: a protected key, a missing key, and a userinfo past MAX_INFO_STRING.
    probe.send_stringcmd("setinfo *sid 1");
    probe.send_stringcmd("setinfo");
    probe.send_stringcmd(&format!("setinfo big {}", "x".repeat(300)));
    h.step(&mut [&mut probe], 20);

    let game = h.server.game();
    assert_eq!(game.client_userinfo_changed_calls, 1);
    assert!(game.commands.is_empty(), "{:?}", game.commands);
    let info = InfoString::parse(probe.userinfos().get(&0).expect("own userinfo"));
    assert_eq!(info.get("model"), Some("gign"));
    assert_eq!(info.get("*sid"), None);
    assert_eq!(info.get("big"), None);
}

#[test]
fn disconnect_and_dropclient_free_the_slot() {
    let mut h = Harness::new(4);
    let mut first = h.add_probe("First");
    let mut second = h.add_probe("Second");
    h.step(&mut [&mut first, &mut second], 200);
    assert_eq!(h.server.client_count(), 2);

    first.send_stringcmd("disconnect");
    h.step(&mut [&mut first, &mut second], 20);
    assert_eq!(h.server.client_state(0), ClientState::Free);
    assert_eq!(h.server.client_state(1), ClientState::Active);

    second.send_stringcmd("dropclient");
    h.step(&mut [&mut first, &mut second], 20);
    assert_eq!(h.server.client_count(), 0);
    let game = h.server.game();
    assert_eq!(game.client_disconnect_calls, 2);
    assert!(game.commands.is_empty(), "{:?}", game.commands);
}
