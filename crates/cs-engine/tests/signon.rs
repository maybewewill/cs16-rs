//! Sign-on of `cs-probe` clients against `Server<TestGame>` over the loopback transport.

mod common;

use common::{Harness, TestGame};
use cs_engine::{
    ClientState, INIT_BUFFER_BUDGET, MAX_NAME_BYTES, MAX_RELIABLE_BACKLOG, MAX_SIGNON_BUFFER,
    MsgDest, SPAWN_TAIL_MAX, ServerConfig, ServerMap, SignonStage,
};
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
    assert_eq!(h.server.game().client_put_in_server_calls, 1);
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
    // The silence timeout is set past the sign-on timeout, which is what is tested here.
    let cfg = ServerConfig {
        max_clients: 4,
        timeout_secs: 1000.0,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_config(cfg, TestGame::default());
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

fn serverinfo_count(probe: &Probe) -> usize {
    opcodes(probe)
        .iter()
        .filter(|&&op| op == svc::SERVERINFO)
        .count()
}

#[test]
fn repeated_new_while_stage_pending_is_ignored() {
    let mut h = Harness::new(4);
    let mut probe = h.add_probe("Player");
    step_until(&mut h, &mut [&mut probe], 20, "AwaitSendRes", |h, _| {
        h.server.client_state(0) == ClientState::Signon(SignonStage::AwaitSendRes)
    });
    // One reliable payload of 300 `new` commands while the client awaits `sendres`.
    for _ in 0..300 {
        probe.send_stringcmd("new");
    }
    h.step(&mut [&mut probe], 200);
    assert_eq!(serverinfo_count(&probe), 1);
    assert_eq!(probe.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
}

#[test]
fn repeated_new_followed_by_sendres_in_one_payload() {
    let mut h = Harness::new(4);
    let mut other = h.add_probe("Other");
    h.step(&mut [&mut other], 200);
    assert_eq!(other.state(), &ProbeState::InGame);

    let mut probe = h.add_probe("Player");
    step_until(
        &mut h,
        &mut [&mut other, &mut probe],
        200,
        "first stage delivered",
        |_, p| p[1].server_info().is_some(),
    );
    // One reliable payload: 300 `new`, then `sendres`.
    for _ in 0..300 {
        probe.send_stringcmd("new");
    }
    probe.send_stringcmd("sendres");
    h.step(&mut [&mut other, &mut probe], 400);

    // Either sign-on completes or the client was dropped by the backlog cap.
    match h.server.client_state(1) {
        ClientState::Active => assert_eq!(probe.state(), &ProbeState::InGame),
        state => assert_eq!(state, ClientState::Free),
    }
    assert_eq!(other.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(other.stats().parse_errors, 0);
}

#[test]
fn new_is_ignored_awaiting_sendres_and_restarts_awaiting_spawn() {
    let mut h = Harness::new(4);
    let mut probe = h.add_probe("Player");
    step_until(&mut h, &mut [&mut probe], 20, "AwaitSendRes", |h, _| {
        h.server.client_state(0) == ClientState::Signon(SignonStage::AwaitSendRes)
    });
    probe.send_stringcmd("new");
    h.step(&mut [&mut probe], 200);
    assert_eq!(serverinfo_count(&probe), 1);
    assert_eq!(probe.state(), &ProbeState::InGame);

    let mut h = Harness::new(4);
    let mut probe = h.add_probe("Player");
    step_until(&mut h, &mut [&mut probe], 200, "AwaitSpawn", |h, _| {
        h.server.client_state(0) == ClientState::Signon(SignonStage::AwaitSpawn)
    });
    // Goes out ahead of the probe's own `spawn`, which then arrives out of stage.
    probe.send_stringcmd("new");
    h.step(&mut [&mut probe], 200);
    assert_eq!(serverinfo_count(&probe), 2);
    assert_eq!(probe.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    let game = h.server.game();
    assert_eq!(game.client_put_in_server_calls, 1);
    assert_eq!(game.client_disconnect_calls, 0);
}

#[test]
fn new_and_sendres_in_one_payload_honour_only_new() {
    let mut h = Harness::new(4);
    let mut probe = h.add_probe("Player");
    step_until(&mut h, &mut [&mut probe], 200, "AwaitSpawn", |h, _| {
        h.server.client_state(0) == ClientState::Signon(SignonStage::AwaitSpawn)
    });
    // One reliable payload: `new`, then `sendres` (the probe may append its own replies to the
    // `sendres` stage, which arrive out of stage as well).
    probe.send_stringcmd("new");
    probe.send_stringcmd("sendres");
    step_until(
        &mut h,
        &mut [&mut probe],
        200,
        "second svc_serverinfo",
        |_, p| serverinfo_count(p[0]) == 2,
    );
    // Had `sendres` been honoured too, its `svc_resourcerequest` would have gone out in the
    // same reliable message as the `new` stage and arrived with it.
    let requests = opcodes(&probe)
        .iter()
        .filter(|&&op| op == svc::RESOURCEREQUEST)
        .count();
    assert_eq!(requests, 1);
    assert_eq!(serverinfo_count(&probe), 2);
}

#[test]
fn signon_completes_while_game_broadcasts_every_frame() {
    let game = TestGame {
        broadcast_every_frame: true,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut first = h.add_probe("First");
    h.step(&mut [&mut first], 200);
    assert_eq!(first.state(), &ProbeState::InGame);

    // Every frame the game broadcasts a reliable message and a userinfo change, so the
    // joining client's channel almost always has reliable data in flight during sign-on.
    let mut second = h.add_probe("Second");
    step_until(
        &mut h,
        &mut [&mut first, &mut second],
        400,
        "second InGame",
        |_, p| p[1].state() == &ProbeState::InGame,
    );
    assert_eq!(h.server.client_state(1), ClientState::Active);
    assert_eq!(first.state(), &ProbeState::InGame);
    assert_eq!(first.stats().parse_errors, 0);
    assert_eq!(second.stats().parse_errors, 0);
}

#[test]
fn reliable_backlog_over_the_cap_drops_the_client() {
    let game = TestGame {
        flood_bytes: MAX_RELIABLE_BACKLOG + 1,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut flooder = h.add_probe("Flooder");
    let mut other = h.add_probe("Other");
    h.step(&mut [&mut flooder, &mut other], 200);
    assert_eq!(h.server.client_count(), 2);

    flooder.send_stringcmd("flood");
    h.step(&mut [&mut flooder, &mut other], 20);
    assert_eq!(h.server.client_state(0), ClientState::Free);
    assert_eq!(h.server.client_state(1), ClientState::Active);
    assert_eq!(h.server.client_count(), 1);
    assert_eq!(h.server.game().client_disconnect_calls, 1);
    assert_eq!(other.state(), &ProbeState::InGame);
}

#[test]
fn new_after_spawn_restarts_cleanly() {
    let mut h = Harness::new(4);
    let mut probe = h.add_probe("Player");
    step_until(&mut h, &mut [&mut probe], 200, "AwaitSendEnts", |h, _| {
        h.server.client_state(0) == ClientState::Signon(SignonStage::AwaitSendEnts)
    });
    // Goes out ahead of the probe's own `sendents`, which then arrives out of stage.
    probe.send_stringcmd("new");
    h.step(&mut [&mut probe], 200);
    assert_eq!(serverinfo_count(&probe), 2);
    assert_eq!(probe.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    let game = h.server.game();
    assert_eq!(game.client_put_in_server_calls, 2);
    assert_eq!(game.client_disconnect_calls, 1);
}

#[test]
fn kick_during_connect_rejects() {
    let game = TestGame {
        kick_on_connect: Some("Kicked while connecting".to_string()),
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut probe = h.add_probe("Player");
    h.step(&mut [&mut probe], 20);
    assert_eq!(
        probe.state(),
        &ProbeState::Rejected("Kicked while connecting".to_string())
    );
    assert_eq!(h.server.client_count(), 0);
    assert_eq!(h.server.game().client_connect_calls, 1);

    // The rejected connect did not use up a userid.
    h.server.game_mut().kick_on_connect = None;
    let mut next = h.add_probe("Next");
    h.step(&mut [&mut next], 200);
    assert_eq!(next.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_userid(0), Some(1));
}

#[test]
fn backlog_overflow_during_connect_rejects_with_reason() {
    let game = TestGame {
        flood_on_connect: true,
        flood_bytes: MAX_RELIABLE_BACKLOG + 1,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut probe = h.add_probe("Player");
    h.step(&mut [&mut probe], 20);
    assert_eq!(
        probe.state(),
        &ProbeState::Rejected("Reliable channel overflowed".to_string())
    );
    assert_eq!(h.server.client_count(), 0);
}

#[test]
fn prefixed_duplicate_name_fits_the_name_limit() {
    let long = "N".repeat(MAX_NAME_BYTES);
    let mut h = Harness::new(4);
    let mut first = h.add_probe(&long);
    h.step(&mut [&mut first], 200);
    let mut second = h.add_probe(&long);
    h.step(&mut [&mut first, &mut second], 200);
    assert_eq!(second.state(), &ProbeState::InGame);
    assert_eq!(name_of(&second, 0).as_deref(), Some(long.as_str()));
    let prefixed = name_of(&second, 1).expect("second client's name");
    assert_eq!(prefixed, format!("(1){}", &long[..MAX_NAME_BYTES - 3]));
    assert_eq!(prefixed.len(), MAX_NAME_BYTES);
}

/// Bytes of the `svc_spawnbaseline` message for `baselines` of a server with `max_clients`.
fn baseline_message_len(
    max_clients: u8,
    baselines: &[(u16, cs_proto48::delta::GoldSrcEntityState)],
) -> usize {
    let mut w = cs_proto48::bitbuf::MsgWriter::new();
    cs_proto48::messages::write_svc_spawnbaseline(
        &mut w,
        &DeltaRegistry::cs16_default(),
        max_clients,
        baselines,
        0.0,
    );
    w.len()
}

/// The init buffer is full, 1500 entities have every baseline field set: the baselines that
/// fit the rest of the sign-on budget are sent (the lowest entity numbers), the others are
/// skipped, and the client still gets in the game.
#[test]
fn init_and_baselines_share_the_signon_budget() {
    let entities = 1500;
    let game = TestGame {
        extra_entities: entities,
        rich_extras: true,
        activate_sends: (0..INIT_BUFFER_BUDGET / 1000 + 2)
            .map(|_| (MsgDest::Init, vec![svc::NOP; 1000]))
            .collect(),
        ..TestGame::default()
    };
    let cfg = ServerConfig {
        max_clients: 4,
        max_edicts: entities + 10,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_config(cfg, game);
    let init = h.server.init_buffer_len();
    assert!(
        init > INIT_BUFFER_BUDGET - 1000 && init <= INIT_BUFFER_BUDGET,
        "{init}"
    );
    let mut probe = h.add_probe("A");
    step_until(&mut h, &mut [&mut probe], 3000, "in game", |_, p| {
        p[0].state() == &ProbeState::InGame
    });
    assert_eq!(h.server.client_state(0), ClientState::Active);

    let baselines = probe.baselines();
    let kept: Vec<_> = baselines.iter().map(|(n, s)| (*n, s.clone())).collect();
    assert!(
        !kept.is_empty() && kept.len() < entities,
        "{} baselines",
        kept.len()
    );
    assert_eq!(kept.len(), h.server.baseline_count());
    // The highest numbers are the ones skipped: the world (edict 0, which has a model, so
    // `SV_CreateBaseline` makes one for it) and the lowest of the others are all there.
    let mut numbers = baselines.keys().copied();
    assert_eq!(numbers.next(), Some(0));
    assert!(numbers.eq(5..5 + kept.len() as u16 - 1));
    // Together with the init buffer they fit the sign-on buffer less the spawn tail.
    let room = MAX_SIGNON_BUFFER - SPAWN_TAIL_MAX - init;
    let size = baseline_message_len(4, &kept);
    assert!(size <= room, "{size} bytes of {room}");
    eprintln!(
        "{} of {entities} baselines kept: {size} bytes of {room}",
        kept.len()
    );
}

/// Sizes of the spawn baselines (for the choice of `SIGNON_BASELINE_RESERVE`): a stub of 1500
/// entities with a model, an origin and angles, and the brush entities of the installed maps.
/// Reported with `--nocapture`; the real maps are skipped without the installed game.
#[test]
fn signon_baseline_sizes_are_reported() {
    let entities = 1500;
    let game = TestGame {
        extra_entities: entities,
        ..TestGame::default()
    };
    let cfg = ServerConfig {
        max_clients: 16,
        max_edicts: entities + 20,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_config(cfg, game);
    let mut probe = h.add_probe("A");
    step_until(&mut h, &mut [&mut probe], 3000, "in game", |_, p| {
        p[0].state() == &ProbeState::InGame
    });
    let kept: Vec<_> = probe.baselines().into_iter().collect();
    let size = baseline_message_len(16, &kept);
    eprintln!(
        "stub, {entities} entities with a model (origin): {} baselines, {size} bytes",
        kept.len()
    );
    // The entities and the world, which has a model.
    assert_eq!(kept.len(), entities + 1);

    let Ok(vfs) = cs_vfs::Vfs::discover(None, cs_vfs::VfsConfig::default()) else {
        return;
    };
    for name in ["de_dust2", "cs_office"] {
        let Ok(raw) = vfs.read_bytes(&format!("maps/{name}.bsp")) else {
            continue;
        };
        let bsp = std::sync::Arc::new(cs_assets::bsp::BspMap::load(&vfs, name).expect("map parses"));
        let mut map = ServerMap::from_bsp(name, &raw, bsp).expect("valid BSP header");
        // The test game keeps the entities it knows by class: give the brush entities that.
        for e in &mut map.entities {
            if e.get("model").is_some_and(|m| m.starts_with('*')) {
                e.properties
                    .insert("classname".to_string(), "test_keep_brush".to_string());
            }
        }
        let cfg = ServerConfig {
            max_clients: 32,
            ..ServerConfig::default()
        };
        let mut h = Harness::with_map(cfg, map, TestGame::default());
        let mut probe = h.add_probe("A");
        step_until(&mut h, &mut [&mut probe], 3000, "in game", |_, p| {
            p[0].state() == &ProbeState::InGame
        });
        let kept: Vec<_> = probe.baselines().into_iter().collect();
        let size = baseline_message_len(32, &kept);
        eprintln!("{name}: {} brush-entity baselines, {size} bytes", kept.len());
    }
}
