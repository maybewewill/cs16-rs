//! User commands and string commands of in-game `cs-probe` clients against `Server<TestGame>`
//! over the loopback transport.

mod common;

use common::Harness;
use cs_engine::{
    ClientState, MAX_CLIENT_INCOMING_STREAM, MAX_CONSECUTIVE_BAD_MOVES, MAX_NAME_BYTES,
    MAX_REFUSED_STREAMS, MAX_USERCMD_STEP_MS, STRINGCMD_BURST, STRINGCMD_RATE_PER_SEC,
};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::crypto::com_munge2;
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::info::InfoString;
use cs_proto48::netchan::{FRAGMENT_S2C_MAX_SIZE, make_frag_id};
use cs_transport::{FaultStats, Faults, LoopbackEndpoint, PacketTransport, PeerAddr};

type Probe = ProbeClient<LoopbackEndpoint>;

/// Adds a probe and runs the harness until it is in the game.
fn joined(h: &mut Harness, name: &str) -> Probe {
    let mut probe = h.add_probe(name);
    h.step(&mut [&mut probe], 200);
    assert_eq!(probe.state(), &ProbeState::InGame);
    probe
}

/// Entity updates the probe received, full and delta.
fn entity_updates(probe: &Probe) -> u64 {
    probe.stats().full_updates + probe.stats().delta_updates
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
    // (The idle frames' commands of 0 ms run as well; they are left out.)
    let game = h.server.game();
    let seeds: Vec<u32> = game.usercmd_msecs[seeds_before..]
        .iter()
        .zip(&game.random_seeds[seeds_before..])
        .filter(|(ms, _)| **ms != 0)
        .map(|(_, seed)| *seed)
        .collect();
    assert_eq!(seeds.len(), 100);
    assert!(seeds.windows(2).all(|w| w[1] == w[0] + 1), "{seeds:?}");
}

#[test]
fn long_cmd_reaches_the_game_in_steps_of_at_most_50_ms() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "Player");
    let start = origin_x(&h, 0);
    let before = h.server.game().usercmd_msecs.len();

    // The bank is full after sign-on, so all 200 ms are granted.
    drive(&mut h, &mut probe, &forward(200), 1);
    // (Commands of 0 ms from the idle frames run too and are left out.)
    let steps: Vec<u8> = h.server.game().usercmd_msecs[before..]
        .iter()
        .copied()
        .filter(|&ms| ms != 0)
        .collect();
    assert_eq!(steps, [50, 50, 50, 50]);
    assert!(steps.iter().all(|&ms| ms <= MAX_USERCMD_STEP_MS));
    let moved = origin_x(&h, 0) - start;
    assert!((moved - 50.0).abs() <= 0.01, "moved {moved}");
}

/// A kick the game queues in a step stops the move at once, as HLDS does: the later steps of
/// the same command (and the later commands) are not run for the kicked slot.
#[test]
fn a_kick_queued_by_a_step_stops_the_move() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "Player");
    let before = h.server.game().usercmd_msecs.len();
    h.server
        .game_mut()
        .usercmd_hook
        .set(|eng, slot| eng.kick(slot, "test kick"));
    // One 200 ms command is four steps; the first one kicks.
    drive(&mut h, &mut probe, &forward(200), 1);
    assert_eq!(h.server.game().usercmd_msecs[before..], [50]);
    assert_eq!(h.server.client_state(0), ClientState::Free);
}

#[test]
fn speedhack_is_bounded_by_real_time() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "Player");
    let start = origin_x(&h, 0);
    let t0 = h.now;

    // 100 commands per second claiming 20 ms each: twice real time, for two seconds.
    drive(&mut h, &mut probe, &forward(20), 200);
    let moved = f64::from(origin_x(&h, 0) - start);
    // The bank (full since sign-on) plus the real time that passed.
    let bound = 250.0 * (0.5 + (h.now - t0));
    assert!(moved <= bound, "moved {moved}, bound {bound}");
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
    let timed_runs =
        |h: &Harness| h.server.game().usercmd_msecs.iter().filter(|&&ms| ms != 0).count();
    let runs = timed_runs(&h);

    // The only command that would move the player travels in the corrupted packet.
    probe.corrupt_next_moves(1);
    probe.queue_cmd(forward(10));
    h.step(&mut [&mut probe], 20);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(origin_x(&h, 0), start);
    // (Commands of 0 ms from the idle frames are not moves and run.)
    assert_eq!(timed_runs(&h), runs);

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
    let updates_before = entity_updates(&other);
    let stats_before = h.server.net_stats();

    // Headers carry strictly increasing sequences, so the server's channel accepts each one
    // and the payload parser sees its random body. The random datagrams carry random 30-bit
    // sequences, so the headers start near the top of that range, where no random
    // sequence overtakes them. Only the reliable bit is random: the fragment flag is cleared,
    // as it would make most of them fail in the netchan.
    let mut next_seq = 0x3FFF_FFFF - 4_000;
    assert!(next_seq > target.outgoing_sequence().expect("connected"));
    let ack = target.incoming_sequence().expect("connected");
    let mut rng = Lcg(0xC516);
    for i in 0..10_000usize {
        let len = i % 65;
        let mut data: Vec<u8> = (0..len).map(|_| rng.next_byte()).collect();
        if i % 3 == 0 && len >= 8 {
            let reliable = u32::from(rng.next_byte() & 0x80) << 24;
            data[..4].copy_from_slice(&(next_seq | reliable).to_le_bytes());
            data[4..8].copy_from_slice(&ack.to_le_bytes());
            next_seq += 1;
        }
        // Garbage never sets the fragment flag (bit 30 of the first word).
        if len >= 4 {
            data[3] &= !0x40;
        }
        target
            .transport_mut()
            .send(PeerAddr::Loopback(0), &data)
            .expect("loopback send");
        // The target's own probe is not pumped, so no good move resets its bad-move count.
        if i % 100 == 99 {
            h.step(&mut [&mut other], 1);
        }
    }
    h.step(&mut [&mut other], 50);

    let stats = h.server.net_stats();
    assert!(stats.datagrams_rejected > 0, "{stats:?}");
    // The target is dropped by the bad-move rule; after that its address has no slot and
    // its garbage is no longer counted.
    assert!(
        stats.payloads_rejected
            >= stats_before.payloads_rejected + u64::from(MAX_CONSECUTIVE_BAD_MOVES),
        "{stats:?}"
    );
    assert_eq!(h.server.client_state(0), ClientState::Free);
    assert_eq!(h.server.game().client_disconnect_calls, 1);
    assert_eq!(h.server.client_state(1), ClientState::Active);
    assert_eq!(other.state(), &ProbeState::InGame);
    // 150 frames at the probe's `cl_updaterate` of 60 are 90 updates.
    assert!(entity_updates(&other) > updates_before + 80);
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

/// Name in the userinfo `probe` received for `slot`.
fn name_of(probe: &Probe, slot: u8) -> Option<String> {
    let info = probe.userinfos().get(&slot)?;
    InfoString::parse(info).get("name").map(str::to_string)
}

#[test]
fn setinfo_name_is_sanitised_and_bounded() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "Player");
    let long = "N".repeat(MAX_NAME_BYTES + 20);
    probe.send_stringcmd(&format!("setinfo name \x01{long}\x7F"));
    h.step(&mut [&mut probe], 20);
    assert_eq!(name_of(&probe, 0), Some("N".repeat(MAX_NAME_BYTES)));
    assert_eq!(h.server.game().client_userinfo_changed_calls, 1);
}

#[test]
fn setinfo_name_of_another_client_gets_a_prefix() {
    let mut h = Harness::new(4);
    let mut first = h.add_probe("First");
    let mut second = h.add_probe("Second");
    h.step(&mut [&mut first, &mut second], 200);

    second.send_stringcmd("setinfo name First");
    // Re-setting one's own name adds no prefix.
    first.send_stringcmd("setinfo name First");
    h.step(&mut [&mut first, &mut second], 20);
    assert_eq!(name_of(&first, 0).as_deref(), Some("First"));
    assert_eq!(name_of(&first, 1).as_deref(), Some("(1)First"));
}

#[test]
fn setinfo_name_without_value_keeps_the_name() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "Player");
    probe.send_stringcmd("setinfo name");
    probe.send_stringcmd("setinfo name \"\"");
    h.step(&mut [&mut probe], 20);
    assert_eq!(name_of(&probe, 0).as_deref(), Some("Player"));
    assert_eq!(h.server.game().client_userinfo_changed_calls, 0);
}

/// A netchan packet with sequence `seq` whose only content is a `piece` sent as fragment `idx`
/// of `total` in the normal stream, munged like the sender does.
fn fragment_packet(seq: u32, idx: u16, total: u16, piece: &[u8]) -> Vec<u8> {
    let mut w = MsgWriter::new();
    w.write_u32(seq | (1 << 30));
    w.write_u32(0);
    w.write_u8(1);
    w.write_u32(make_frag_id(idx, total));
    w.write_u16(0);
    w.write_u16(piece.len() as u16);
    w.write_u8(0);
    w.write_bytes(piece);
    let mut bytes = w.into_bytes();
    com_munge2(&mut bytes[8..], (seq as u8) as i32);
    bytes
}

/// `b"BZ2\0"` followed by `body`: a compressed fragment stream.
fn bz2_stream(body: &[u8]) -> Vec<u8> {
    let mut stream = b"BZ2\0".to_vec();
    stream.extend_from_slice(body);
    stream
}

#[test]
fn compressed_stream_bombs_are_refused() {
    let mut h = Harness::new(4);
    let mut target = h.add_probe("Target");
    let mut other = h.add_probe("Other");
    h.step(&mut [&mut target, &mut other], 200);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    let updates_before = entity_updates(&other);

    // Each a one-piece stream that completes in one datagram: 8 MiB of zeros, and 1 MiB of
    // `clc_nop`. The target's own probe is not pumped, so its sequences do not clash.
    let bombs = [
        bz2_stream(include_bytes!("../../cs-proto48/tests/fixtures/bomb.bz2")),
        bz2_stream(include_bytes!("../../cs-proto48/tests/fixtures/nops.bz2")),
    ];
    let first_seq = target.outgoing_sequence().expect("connected");
    for (bomb, seq) in bombs.iter().zip(first_seq..) {
        let rejected = h.server.net_stats().datagrams_rejected;
        let packet = fragment_packet(seq, 1, 1, bomb);
        target
            .transport_mut()
            .send(PeerAddr::Loopback(0), &packet)
            .expect("loopback send");
        h.step(&mut [&mut other], 1);
        assert_eq!(
            h.server.net_stats().datagrams_rejected,
            rejected + 1,
            "{} byte bomb",
            bomb.len()
        );
    }
    h.step(&mut [&mut other], 20);

    // The sender stays connected: a refused datagram is not a bad move.
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.client_state(1), ClientState::Active);
    assert_eq!(other.state(), &ProbeState::InGame);
    // 22 frames at the probe's `cl_updaterate` of 60 are 13 updates.
    assert!(entity_updates(&other) > updates_before + 10);
    assert_eq!(other.stats().parse_errors, 0);
}

#[test]
fn fragment_flood_stays_bounded() {
    let mut h = Harness::new(4);
    let mut target = h.add_probe("Target");
    let mut other = h.add_probe("Other");
    h.step(&mut [&mut target, &mut other], 200);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    let updates_before = entity_updates(&other);

    // 2000 maximum-size pieces of a 65535-piece message that is never completed, each with
    // a new piece index. The target's own probe is not pumped, so its sequences do not clash.
    let first_seq = target.outgoing_sequence().expect("connected");
    let piece = vec![0x5A; FRAGMENT_S2C_MAX_SIZE];
    let mut peak = 0;
    for (i, seq) in (0..2_000u16).zip(first_seq..) {
        let packet = fragment_packet(seq, i + 1, u16::MAX, &piece);
        target
            .transport_mut()
            .send(PeerAddr::Loopback(0), &packet)
            .expect("loopback send");
        if i % 20 == 19 {
            h.step(&mut [&mut other], 1);
            let buffered = h.server.client_incoming_buffered_bytes(0);
            assert!(
                buffered <= 2 * MAX_CLIENT_INCOMING_STREAM,
                "{buffered} bytes"
            );
            peak = peak.max(buffered);
        }
    }
    h.step(&mut [&mut other], 20);

    // Pieces were buffered up to the limit, then discarded.
    assert!(peak > MAX_CLIENT_INCOMING_STREAM / 2, "peak {peak}");
    assert!(h.server.net_stats().datagrams_rejected > 0);
    assert_eq!(h.server.client_state(1), ClientState::Active);
    assert_eq!(other.state(), &ProbeState::InGame);
    // 120 frames at the probe's `cl_updaterate` of 60 are 72 updates.
    assert!(entity_updates(&other) > updates_before + 60);
}

/// Sends `target` one-piece compressed streams that its netchan refuses, with sequences from
/// `*seq` on, one datagram per frame.
fn send_bombs(h: &mut Harness, target: &mut Probe, other: &mut Probe, seq: &mut u32, count: u32) {
    let bomb = bz2_stream(include_bytes!("../../cs-proto48/tests/fixtures/bomb.bz2"));
    for _ in 0..count {
        let packet = fragment_packet(*seq, 1, 1, &bomb);
        *seq += 1;
        target
            .transport_mut()
            .send(PeerAddr::Loopback(0), &packet)
            .expect("loopback send");
        h.step(&mut [&mut *other], 1);
    }
}

#[test]
fn repeated_refused_streams_drop_the_sender() {
    let mut h = Harness::new(4);
    let mut target = h.add_probe("Target");
    let mut other = h.add_probe("Other");
    h.step(&mut [&mut target, &mut other], 200);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    let updates_before = entity_updates(&other);

    // The target's own probe is not pumped, so its sequences do not clash.
    let mut seq = target.outgoing_sequence().expect("connected");
    send_bombs(&mut h, &mut target, &mut other, &mut seq, MAX_REFUSED_STREAMS - 1);
    assert_eq!(h.server.client_state(0), ClientState::Active, "one short");
    send_bombs(&mut h, &mut target, &mut other, &mut seq, 1);
    assert_eq!(h.server.client_state(0), ClientState::Free);
    assert_eq!(h.server.client_count(), 1);
    h.step(&mut [&mut other], 20);

    assert_eq!(h.server.client_state(1), ClientState::Active);
    assert_eq!(other.state(), &ProbeState::InGame);
    // 28 frames at the probe's `cl_updaterate` of 60 are 16 updates.
    assert!(entity_updates(&other) > updates_before + 12);
    assert_eq!(other.stats().parse_errors, 0);
}

#[test]
fn an_accepted_stream_resets_the_refused_count() {
    let mut h = Harness::new(4);
    let mut target = h.add_probe("Target");
    let mut other = h.add_probe("Other");
    h.step(&mut [&mut target, &mut other], 200);
    assert_eq!(h.server.client_state(0), ClientState::Active);

    let mut seq = target.outgoing_sequence().expect("connected");
    send_bombs(&mut h, &mut target, &mut other, &mut seq, MAX_REFUSED_STREAMS - 1);
    // A completed, uncompressed one-piece stream (`clc_nop`).
    let packet = fragment_packet(seq, 1, 1, &[1]);
    seq += 1;
    target
        .transport_mut()
        .send(PeerAddr::Loopback(0), &packet)
        .expect("loopback send");
    h.step(&mut [&mut other], 1);
    send_bombs(&mut h, &mut target, &mut other, &mut seq, MAX_REFUSED_STREAMS - 1);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    send_bombs(&mut h, &mut target, &mut other, &mut seq, 1);
    assert_eq!(h.server.client_state(0), ClientState::Free);
}

#[test]
fn spoofed_fragment_headers_do_not_drop_the_client() {
    let mut h = Harness::new(4);
    let mut target = h.add_probe("Target");
    let mut other = h.add_probe("Other");
    h.step(&mut [&mut target, &mut other], 200);
    assert_eq!(h.server.client_state(0), ClientState::Active);

    // Anyone who knows the address can send these: the fragment flag and a header that ends
    // before the first stream's entry.
    for _ in 0..20 {
        target
            .transport_mut()
            .send(PeerAddr::Loopback(0), &[0, 0, 0, 0x40, 0, 0, 0, 0])
            .expect("loopback send");
    }
    let rejected = h.server.net_stats().datagrams_rejected;
    h.step(&mut [&mut other], 5);

    assert!(h.server.net_stats().datagrams_rejected >= rejected + 20);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.client_state(1), ClientState::Active);
}
