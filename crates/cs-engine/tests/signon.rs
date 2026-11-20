//! Sign-on of `cs-probe` clients against `Server<TestGame>` over the loopback transport.

mod common;

use common::{Harness, TestGame};
use cs_engine::{ClientState, SignonStage};
use cs_probe::{Direction, ProbeClient, ProbeState, read_capture};
use cs_proto48::CONNECTIONLESS_HEADER;
use cs_proto48::delta::DeltaRegistry;
use cs_proto48::info::InfoString;
use cs_proto48::messages::{SvcParseContext, parse_svc_stream, svc};
use cs_proto48::netchan::{FRAGMENT_C2S_SIZE, NetChan, SplitPacketAssembler};
use cs_proto48::usermsg::{USERMSG_BASE_ID, UserMsgRegistry};
use cs_transport::{Faults, LoopbackEndpoint};

type Probe = ProbeClient<LoopbackEndpoint>;

/// Steps one frame at a time until `done` holds; panics after `max_frames`.
fn step_until(
    h: &mut Harness,
    probes: &mut [&mut Probe],
    max_frames: usize,
    what: &str,
    mut done: impl FnMut(&Harness, &[&mut Probe]) -> bool,
) {
    for _ in 0..max_frames {
        if done(h, probes) {
            return;
        }
        h.step(probes, 1);
    }
    assert!(
        done(h, probes),
        "{what} not reached within {max_frames} frames"
    );
}

/// The probe's opcode log without netchan padding.
fn opcodes(probe: &Probe) -> Vec<u8> {
    probe
        .opcode_log()
        .iter()
        .copied()
        .filter(|&op| op != svc::NOP)
        .collect()
}

/// Whether `needles` occur in `hay` in this order (not necessarily adjacent).
fn contains_in_order(hay: &[u8], needles: &[u8]) -> bool {
    let mut rest = hay.iter();
    needles.iter().all(|n| rest.any(|h| h == n))
}

#[test]
fn probe_reaches_in_game_within_200_frames() {
    let mut h = Harness::new(4);
    let mut probe = h.add_probe("Player");
    h.step(&mut [&mut probe], 200);
    assert_eq!(probe.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.client_count(), 1);
    assert_eq!(h.server.game().client_connect_calls, 1);
    assert_eq!(h.server.game().client_put_in_server_calls, 1);
    assert_eq!(probe.player_slot(), Some(0));
    assert_eq!(
        probe.server_info().map(|i| i.spawn_count),
        Some(h.server.spawn_count())
    );
    // The per-frame update carries client data and the player entity the game placed.
    assert_eq!(probe.client_data().map(|c| c.health), Some(100.0));
    assert_eq!(probe.entities().get(&1).map(|e| e.modelindex), Some(2));
    assert_eq!(probe.stats().parse_errors, 0);
}

#[test]
fn signon_opcode_order_matches_working_assumption() {
    let mut h = Harness::new(4);
    let mut probe = h.add_probe("Player");
    h.step(&mut [&mut probe], 200);
    assert_eq!(probe.state(), &ProbeState::InGame);

    let schemas = DeltaRegistry::cs16_default().iter().count();
    let usermsgs = UserMsgRegistry::cs16_default().iter().count();
    let mut expected = vec![svc::PRINT, svc::SERVERINFO, svc::SENDEXTRAINFO];
    expected.extend(std::iter::repeat_n(svc::DELTADESCRIPTION, schemas));
    expected.extend([svc::NEWMOVEVARS, svc::CDTRACK, svc::SETVIEW]);
    expected.extend(std::iter::repeat_n(svc::NEWUSERMSG, usermsgs));

    let log = opcodes(&probe);
    assert!(
        log.starts_with(&expected),
        "sign-on starts with {:?}",
        &log[..expected.len().min(log.len())]
    );
    assert!(
        contains_in_order(
            &log[expected.len()..],
            &[
                svc::RESOURCEREQUEST,
                svc::RESOURCELIST,
                svc::SPAWNBASELINE,
                svc::SIGNONNUM,
                svc::PACKETENTITIES,
            ]
        ),
        "{log:?}"
    );
}

/// Opcodes up to the first `svc_packetentities`, without user messages, padding, time and prints.
fn signon_skeleton(opcodes: impl IntoIterator<Item = u8>) -> Vec<u8> {
    opcodes
        .into_iter()
        .take_while(|&op| op != svc::PACKETENTITIES)
        .filter(|&op| op < USERMSG_BASE_ID && ![svc::NOP, svc::TIME, svc::PRINT].contains(&op))
        .collect()
}

#[test]
fn signon_opcode_order_matches_golden_capture() {
    let Some(dir) = std::env::var_os("CS16_GOLDEN_DIR") else {
        return;
    };
    let Ok(bytes) = std::fs::read(std::path::Path::new(&dir).join("hlds_dust2_walk.cs48cap"))
    else {
        return;
    };
    let records = read_capture(&bytes).expect("readable capture");

    let mut chan = NetChan::new(FRAGMENT_C2S_SIZE);
    let mut ctx = SvcParseContext::new();
    let mut splits = SplitPacketAssembler::default();
    let mut golden = Vec::new();
    for record in records.iter().filter(|r| r.dir == Direction::Received) {
        let Some(datagram) = splits.push(&record.data) else {
            continue;
        };
        if datagram.starts_with(&CONNECTIONLESS_HEADER.to_le_bytes()) {
            continue;
        }
        let Some(incoming) = chan.process_packet(&datagram).expect("netchan packet") else {
            continue;
        };
        ctx.last_incoming_seq = (incoming.sequence & 0xFF) as u8;
        for stream in std::iter::once(incoming.payload).chain(incoming.completed_normal_stream) {
            let messages = parse_svc_stream(&stream, &mut ctx).expect("svc stream");
            golden.extend(messages.iter().map(|m| m.opcode()));
        }
    }

    let mut h = Harness::new(4);
    let mut probe = h.add_probe("Player");
    h.step(&mut [&mut probe], 200);
    assert_eq!(probe.state(), &ProbeState::InGame);
    assert_eq!(
        signon_skeleton(probe.opcode_log().iter().copied()),
        signon_skeleton(golden)
    );
}

#[test]
fn signon_survives_every_third_packet_dropped() {
    let mut h = Harness::new(4);
    let mut probe = h.add_probe("Player");
    probe.transport_mut().set_faults(Faults {
        drop_every: 3,
        ..Faults::default()
    });
    step_until(&mut h, &mut [&mut probe], 3000, "InGame", |_, p| {
        p[0].state() == &ProbeState::InGame
    });
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.game().client_put_in_server_calls, 1);
    assert!(probe.transport().fault_stats().dropped > 0);
}

#[test]
fn signon_survives_duplicates_and_reordering() {
    let mut h = Harness::new(4);
    let mut probe = h.add_probe("Player");
    probe.transport_mut().set_faults(Faults {
        duplicate_every: 2,
        swap_every: 3,
        ..Faults::default()
    });
    step_until(&mut h, &mut [&mut probe], 1000, "InGame", |_, p| {
        p[0].state() == &ProbeState::InGame
    });
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.client_count(), 1);
    let stats = probe.transport().fault_stats();
    assert!(stats.duplicated > 0, "{stats:?}");
    assert!(stats.swapped > 0, "{stats:?}");
}

#[test]
fn second_client_on_full_server_is_rejected() {
    let mut h = Harness::new(1);
    let mut first = h.add_probe("First");
    h.step(&mut [&mut first], 200);
    assert_eq!(first.state(), &ProbeState::InGame);

    let mut second = h.add_probe("Second");
    h.step(&mut [&mut first, &mut second], 20);
    // The probe drops the reject text's trailing newline ("Server is full.\n").
    assert_eq!(
        second.state(),
        &ProbeState::Rejected("Server is full.".to_string())
    );
    assert_eq!(first.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_count(), 1);
    assert_eq!(h.server.game().client_connect_calls, 1);
}

#[test]
fn reconnect_from_same_address_reuses_slot() {
    let mut h = Harness::new(4);
    let mut probe = h.add_probe("Player");
    h.step(&mut [&mut probe], 200);
    assert_eq!(probe.state(), &ProbeState::InGame);

    probe.connect();
    h.step(&mut [&mut probe], 5);
    step_until(&mut h, &mut [&mut probe], 200, "InGame again", |_, p| {
        p[0].state() == &ProbeState::InGame
    });
    assert_eq!(h.server.client_count(), 1);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.client_state(1), ClientState::Free);
    let game = h.server.game();
    assert_eq!(game.client_connect_calls, 2);
    // The game hears about the old session ending before the new one is put in.
    assert_eq!(game.client_disconnect_calls, 1);
    assert_eq!(game.client_put_in_server_calls, 2);
}

fn name_of(probe: &Probe, slot: u8) -> Option<String> {
    let info = probe.userinfos().get(&slot)?;
    InfoString::parse(info).get("name").map(str::to_string)
}

#[test]
fn duplicate_name_gets_numeric_prefix() {
    let mut h = Harness::new(4);
    let mut first = h.add_probe("Player");
    h.step(&mut [&mut first], 200);
    let mut second = h.add_probe("Player");
    h.step(&mut [&mut first, &mut second], 200);
    assert_eq!(second.state(), &ProbeState::InGame);
    assert_eq!(second.player_slot(), Some(1));
    assert_eq!(name_of(&second, 0).as_deref(), Some("Player"));
    assert_eq!(name_of(&second, 1).as_deref(), Some("(1)Player"));

    let mut third = h.add_probe("Player");
    h.step(&mut [&mut first, &mut second, &mut third], 200);
    assert_eq!(third.state(), &ProbeState::InGame);
    assert_eq!(name_of(&third, 2).as_deref(), Some("(2)Player"));
}

#[test]
fn stale_spawn_count_restarts_signon() {
    let mut h = Harness::new(4);
    let mut probe = h.add_probe("Player");
    step_until(&mut h, &mut [&mut probe], 200, "AwaitSpawn", |h, _| {
        h.server.client_state(0) == ClientState::Signon(SignonStage::AwaitSpawn)
    });
    // Goes out ahead of the probe's own `spawn`, which then arrives out of stage.
    probe.send_stringcmd("spawn 999 0");
    h.step(&mut [&mut probe], 200);

    let serverinfos = opcodes(&probe)
        .iter()
        .filter(|&&op| op == svc::SERVERINFO)
        .count();
    assert_eq!(serverinfos, 2);
    assert_eq!(probe.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.game().client_put_in_server_calls, 1);
}

#[test]
fn game_can_refuse_connection() {
    let game = TestGame {
        refuse: Some("No entry for tests".to_string()),
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut probe = h.add_probe("Player");
    h.step(&mut [&mut probe], 20);
    assert_eq!(
        probe.state(),
        &ProbeState::Rejected("No entry for tests".to_string())
    );
    assert_eq!(h.server.client_state(0), ClientState::Free);
    assert_eq!(h.server.client_count(), 0);
    assert_eq!(h.server.game().client_connect_calls, 1);
}

#[test]
fn stalled_signon_times_out() {
    let mut h = Harness::new(4);
    let mut probe = h.add_probe("Player");
    step_until(&mut h, &mut [&mut probe], 20, "AwaitSendRes", |h, _| {
        h.server.client_state(0) == ClientState::Signon(SignonStage::AwaitSendRes)
    });
    // The probe has sent `new` and is never pumped again.
    h.step(&mut [], 11_900);
    assert_eq!(
        h.server.client_state(0),
        ClientState::Signon(SignonStage::AwaitSendRes)
    );
    h.step(&mut [], 200);
    assert_eq!(h.server.client_state(0), ClientState::Free);
    assert_eq!(h.server.client_count(), 0);
    // Never put in the server, so the game is not told about a disconnect.
    assert_eq!(h.server.game().client_disconnect_calls, 0);
}
