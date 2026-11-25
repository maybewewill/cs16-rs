//! The end of a client's life and the housekeeping around it, as `cs-probe` clients see it over
//! the loopback: kicks, timeouts, `disconnect`, shutdown, userinfo updates, pings and map
//! changes.

mod common;

use common::{FRAME, GAME_KICK_REASON, Harness, TestGame};
use cs_cvar::CVarRegistry;
use cs_engine::{
    ClientState, MAX_EDICTS_WIRE, MAX_RELIABLE_BACKLOG, PINGS_INTERVAL_SECS, PrecacheError, SETINFO_BURST,
    SETINFO_RATE_PER_SEC, Server, ServerConfig, ServerMap, SignonStage,
};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::delta::DeltaRegistry;
use cs_proto48::info::InfoString;
use cs_proto48::messages::svc;
use cs_proto48::netchan::{FRAGMENT_S2C_MAX_SIZE, MAX_MSGLEN};
use cs_proto48::query::{A2S_INFO, A2S_INFO_PAYLOAD};
use cs_transport::{LoopbackEndpoint, PacketTransport, PeerAddr};

type Probe = ProbeClient<LoopbackEndpoint>;

/// Seconds of silence after which the default configuration releases a client.
const TIMEOUT_SECS: f64 = 60.0;

/// Adds a probe and runs the harness, pumping `others` too, until the probe is in the game.
fn joined(h: &mut Harness, name: &str, others: &mut [&mut Probe]) -> Probe {
    let mut probe = h.add_probe(name);
    for _ in 0..300 {
        let mut all: Vec<&mut Probe> = others.iter_mut().map(|p| &mut **p).collect();
        all.push(&mut probe);
        h.step(&mut all, 1);
        if probe.state() == &ProbeState::InGame {
            return probe;
        }
    }
    panic!("{name} did not get in the game: {:?}", probe.state());
}

fn slot_of(probe: &Probe) -> u8 {
    probe.player_slot().expect("player slot")
}

fn opcodes(probe: &Probe, opcode: u8) -> usize {
    probe
        .opcode_log()
        .iter()
        .filter(|&&op| op == opcode)
        .count()
}

fn name_of(probe: &Probe, slot: u8) -> Option<String> {
    let info = probe.userinfos().get(&slot)?;
    InfoString::parse(info).get("name").map(str::to_string)
}

/// Frames in `secs` seconds.
fn frames(secs: f64) -> usize {
    (secs / FRAME).round() as usize
}

#[test]
fn silent_client_times_out_and_others_see_removal() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    h.step(&mut [&mut a, &mut b], 20);
    let slot_a = slot_of(&a);
    let entity_a = u16::from(slot_a) + 1;
    assert!(b.entities().contains_key(&entity_a));

    // A stops sending. Shortly before the timeout it is still there.
    h.step(&mut [&mut b], frames(TIMEOUT_SECS - 1.0));
    assert_eq!(h.server.client_state(slot_a), ClientState::Active);
    h.step(&mut [&mut b], frames(2.0));
    assert_eq!(h.server.client_state(slot_a), ClientState::Free);
    assert_eq!(h.server.game().client_disconnect_calls, 1);
    assert!(!b.entities().contains_key(&entity_a));
    assert_eq!(b.userinfos()[&slot_a], "");
    assert_eq!(h.server.client_state(slot_of(&b)), ClientState::Active);
    // Nothing is sent to a client that timed out.
    assert_eq!(a.state(), &ProbeState::InGame);
}

#[test]
fn rejected_datagrams_do_not_keep_a_client_alive() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    let slot_a = slot_of(&a);
    let rejected = h.server.net_stats().datagrams_rejected;

    // A's probe is not pumped, but its address keeps sending a stale datagram (sequence 1)
    // every frame, which the netchan refuses.
    let mut stale = vec![1u8; 16];
    stale[..4].copy_from_slice(&1u32.to_le_bytes());
    stale[4..8].copy_from_slice(&0u32.to_le_bytes());
    for _ in 0..frames(TIMEOUT_SECS + 1.0) {
        a.transport_mut()
            .send(PeerAddr::Loopback(0), &stale)
            .expect("loopback send");
        h.step(&mut [&mut b], 1);
    }
    assert!(h.server.net_stats().datagrams_rejected > rejected + 1000);
    assert_eq!(h.server.client_state(slot_a), ClientState::Free);
    assert_eq!(h.server.game().client_disconnect_calls, 1);
}

#[test]
fn kick_delivers_reason() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    h.server.kick(0, "bye");
    h.step(&mut [&mut a], 5);
    assert_eq!(a.state(), &ProbeState::Disconnected("bye".to_string()));
    assert_eq!(h.server.client_state(0), ClientState::Free);
    assert_eq!(h.server.game().client_disconnect_calls, 1);
    // A free slot cannot be kicked again.
    h.server.kick(0, "again");
    h.step(&mut [&mut a], 5);
    assert_eq!(h.server.game().client_disconnect_calls, 1);
}

#[test]
fn game_kick_delivers_reason() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    a.send_stringcmd("kickme");
    h.step(&mut [&mut a], 10);
    assert_eq!(
        a.state(),
        &ProbeState::Disconnected(GAME_KICK_REASON.to_string())
    );
    assert_eq!(h.server.client_state(0), ClientState::Free);
    assert_eq!(h.server.game().client_disconnect_calls, 1);
}

#[test]
fn kick_with_a_reliable_backlog_still_delivers_reason() {
    let game = TestGame {
        // Just under MAX_MSGLEN, so it is queued as one regular reliable message that, with the
        // header and an `svc_disconnect`, would not fit one datagram.
        flood_bytes: MAX_MSGLEN - 5,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut a = joined(&mut h, "A", &mut []);
    // The flood and the kick arrive in one payload, so the kick finds the flood queued.
    a.send_stringcmd("flood");
    a.send_stringcmd("kickme");
    while h.server.client_state(0) != ClientState::Free {
        h.step(&mut [&mut a], 1);
        assert!(h.now < 5.0, "not kicked");
    }

    // What reached the probe after the kick, read raw, then handed to it.
    let farewells: Vec<Vec<u8>> = std::iter::from_fn(|| a.transport_mut().recv())
        .map(|(_, data)| data)
        .collect();
    assert_eq!(farewells.len(), 3, "{:?}", h.server.net_stats());
    for datagram in &farewells {
        assert!(
            datagram.len() <= 256 + GAME_KICK_REASON.len(),
            "{} byte farewell",
            datagram.len()
        );
        h.hub
            .send(PeerAddr::Loopback(a.transport().id()), datagram)
            .expect("loopback send");
    }
    assert_eq!(h.server.net_stats().send_failures, 0);
    a.pump(h.now);
    assert_eq!(
        a.state(),
        &ProbeState::Disconnected(GAME_KICK_REASON.to_string())
    );
}

#[test]
fn start_frame_is_called_once_per_frame() {
    let mut h = Harness::new(4);
    // Activation ran 16 settle passes (4 player slots), each with a `start_frame`.
    let settle = h.server.game().start_frame_calls;
    assert_eq!(settle, 16);
    let mut probe = h.add_probe("A");
    h.step(&mut [&mut probe], 30);
    assert_eq!(h.server.game().start_frame_calls, settle + 30);
    // A skipped frame does not start one.
    h.server.frame(&mut h.hub, f64::NAN);
    h.server.frame(&mut h.hub, h.now - 1.0);
    assert_eq!(h.server.net_stats().frames_skipped, 2);
    assert_eq!(h.server.game().start_frame_calls, settle + 30);
    h.step(&mut [&mut probe], 1);
    assert_eq!(h.server.game().start_frame_calls, settle + 31);
}

#[test]
fn client_disconnect_command_frees_slot() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    a.send_stringcmd("disconnect");
    h.step(&mut [&mut a], 10);
    assert_eq!(h.server.client_state(0), ClientState::Free);
    assert_eq!(h.server.game().client_disconnect_calls, 1);
    // A client leaving on its own is not told anything.
    assert_eq!(a.state(), &ProbeState::InGame);
}

#[test]
fn shutdown_notifies_everyone() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    h.server.shutdown(&mut h.hub, "Server shutting down");
    h.step(&mut [&mut a, &mut b], 2);
    let reason = ProbeState::Disconnected("Server shutting down".to_string());
    assert_eq!(a.state(), &reason);
    assert_eq!(b.state(), &reason);
    assert_eq!(h.server.client_count(), 0);
    assert_eq!(h.server.game().client_disconnect_calls, 2);
}

#[test]
fn shutdown_deactivates_the_game_once() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    assert_eq!(h.server.game().server_deactivate_calls, 0);
    h.server.shutdown(&mut h.hub, "bye");
    // Clients are released first, then the game deactivates.
    assert_eq!(h.server.game().client_disconnect_calls, 1);
    assert_eq!(h.server.game().server_deactivate_calls, 1);
    // A second shutdown does not call it again.
    h.server.shutdown(&mut h.hub, "bye again");
    assert_eq!(h.server.game().server_deactivate_calls, 1);
    h.step(&mut [&mut a], 2);
}

/// After `shutdown` a frame does nothing but count itself as skipped: the game's
/// `start_frame` is not called and the datagrams waiting for the server (a new connection, a
/// query) are not read, so no new client gets in.
#[test]
fn frame_after_shutdown_does_nothing() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    h.server.shutdown(&mut h.hub, "bye");
    h.step(&mut [&mut a], 2);
    assert_eq!(h.server.client_count(), 0);

    let start_frames = h.server.game().start_frame_calls;
    let skipped = h.server.net_stats().frames_skipped;
    let mut late = h.add_probe("Late");
    let mut query = vec![0xFF; 4];
    query.push(A2S_INFO);
    query.extend_from_slice(A2S_INFO_PAYLOAD);
    let mut asker = h.hub.connect();
    asker
        .send(PeerAddr::Loopback(0), &query)
        .expect("loopback send");
    h.step(&mut [&mut late], 50);
    assert_eq!(h.server.game().start_frame_calls, start_frames);
    assert_eq!(h.server.net_stats().frames_skipped, skipped + 50);
    assert_eq!(h.server.client_count(), 0);
    assert!(
        !matches!(late.state(), ProbeState::InGame | ProbeState::Rejected(_)),
        "{:?}",
        late.state()
    );
    assert!(asker.recv().is_none(), "the query was not read");
}

#[test]
fn max_edicts_is_capped_by_the_wire() {
    let cfg = ServerConfig {
        max_edicts: 65_535,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_config(cfg, TestGame::default());
    assert_eq!(h.server.config().max_edicts, MAX_EDICTS_WIRE);
    let created = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let counter = std::rc::Rc::clone(&created);
    h.server.game_mut().act(move |eng| {
        while eng.create_entity("test_extra").is_some() {
            counter.set(counter.get() + 1);
        }
    });
    h.step(&mut [], 1);
    // The world and the 16 player slots take 17 of the 2048 numbers the wire can carry.
    assert_eq!(created.get(), MAX_EDICTS_WIRE - 17);
}

#[test]
fn name_change_is_broadcast_sanitized_and_rate_limited() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    let slot_a = slot_of(&a);

    a.send_stringcmd("setinfo name  X\x01Y ");
    h.step(&mut [&mut a, &mut b], 20);
    assert_eq!(name_of(&b, slot_a).as_deref(), Some("XY"));

    // A second change inside the same second is ignored.
    a.send_stringcmd("setinfo name Other");
    h.step(&mut [&mut a, &mut b], 20);
    assert_eq!(name_of(&b, slot_a).as_deref(), Some("XY"));
    assert_eq!(h.server.game().client_userinfo_changed_calls, 1);

    // Once the interval has passed, the name can change again.
    h.step(&mut [&mut a, &mut b], 100);
    a.send_stringcmd("setinfo name Other");
    h.step(&mut [&mut a, &mut b], 20);
    assert_eq!(name_of(&b, slot_a).as_deref(), Some("Other"));
}

#[test]
fn setinfo_flood_is_limited_and_coalesced() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    h.step(&mut [&mut a, &mut b], 20);
    let slot_b = slot_of(&b);
    let updates_before = opcodes(&b, svc::UPDATEUSERINFO);
    let changes_before = h.server.game().client_userinfo_changed_calls;
    let t0 = h.now;

    // 200 different `setinfo` commands over one second.
    let mut peak_backlog = 0;
    for i in 0..100 {
        a.send_stringcmd(&format!("setinfo key{i} {i}"));
        a.send_stringcmd(&format!("setinfo other{i} {i}"));
        h.step(&mut [&mut a, &mut b], 1);
        peak_backlog = peak_backlog.max(h.server.client_reliable_backlog(slot_b));
    }
    h.step(&mut [&mut a, &mut b], 20);
    let elapsed = h.now - t0;
    let bound = SETINFO_BURST + SETINFO_RATE_PER_SEC * elapsed;

    let accepted = h.server.game().client_userinfo_changed_calls - changes_before;
    assert!(accepted as f64 >= SETINFO_BURST, "accepted {accepted}");
    assert!(
        accepted as f64 <= bound,
        "accepted {accepted}, bound {bound}"
    );
    let updates = opcodes(&b, svc::UPDATEUSERINFO) - updates_before;
    assert!(updates >= 1);
    assert!(updates as f64 <= bound, "{updates} updates, bound {bound}");
    assert_eq!(b.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(slot_b), ClientState::Active);
    assert!(
        peak_backlog < MAX_RELIABLE_BACKLOG / 64,
        "peak backlog {peak_backlog}"
    );
}

#[test]
fn several_changes_in_one_frame_broadcast_once() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    h.step(&mut [&mut a, &mut b], 20);
    let updates_before = opcodes(&b, svc::UPDATEUSERINFO);

    // Three commands in one packet: the game changes A's userinfo three times in one frame.
    for _ in 0..3 {
        a.send_stringcmd("rekey");
    }
    h.step(&mut [&mut a, &mut b], 20);
    let rekeys = h.server.game().commands.iter().filter(|c| c[0] == "rekey");
    assert_eq!(rekeys.count(), 3);
    assert_eq!(opcodes(&b, svc::UPDATEUSERINFO) - updates_before, 1);
    let info = InfoString::parse(&b.userinfos()[&slot_of(&a)]);
    assert!(info.get("rekey").is_some());
}

#[test]
fn reliable_broadcast_reaches_everyone_past_new() {
    let game = TestGame {
        shout_on_put_in_server: true,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    // The `new` stage's banner is an `svc_print` too: one each, then the game's broadcast.
    let mut a = joined(&mut h, "A", &mut []);
    assert_eq!(opcodes(&a, svc::PRINT), 2);

    // B is put in the server, and the game broadcasts, while B is still signing on.
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    h.step(&mut [&mut a, &mut b], 10);
    assert_eq!(opcodes(&a, svc::PRINT), 3);
    assert_eq!(opcodes(&b, svc::PRINT), 2);
    let log = b.opcode_log();
    let shout_at = log.iter().rposition(|&op| op == svc::PRINT);
    // The first client data is the `spawn` answer's (`SV_WriteSpawn`); the shout of the frame
    // that answered `spawn` follows it (`sv.reliable_datagram`, flushed at the end of the
    // frame) and comes before the first update.
    let first_update = log
        .iter()
        .enumerate()
        .filter(|(_, op)| **op == svc::CLIENTDATA)
        .nth(1)
        .map(|(i, _)| i);
    assert!(shout_at < first_update, "{log:?}");
}

#[test]
fn pings_only_sent_while_in_score_held_and_throttled() {
    const IN_SCORE: u16 = 1 << 15;
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);

    // `SV_ShouldUpdatePing` (`rehlds/engine/sv_main.cpp`, read 2026-11-25): zero unsolicited
    // `svc_pings` while `+showscores` (`IN_SCORE`) is not held.
    let before = opcodes(&a, svc::PINGS);
    h.step(&mut [&mut a], frames(5.0));
    assert_eq!(
        opcodes(&a, svc::PINGS) - before,
        0,
        "must not emit svc_pings when IN_SCORE is not pressed"
    );

    // Holding `IN_SCORE` for 5 seconds emits `svc_pings` throttled to `PINGS_INTERVAL_SECS` (2.0 s).
    let before_held = opcodes(&a, svc::PINGS);
    for _ in 0..frames(5.0) {
        a.queue_cmd(cs_proto48::delta::GoldSrcUserCmd {
            msec: 10,
            buttons: IN_SCORE,
            ..Default::default()
        });
        h.step(&mut [&mut a], 1);
    }
    let pings = opcodes(&a, svc::PINGS) - before_held;
    assert!((2..=3).contains(&pings), "{pings} svc_pings in 5 s while IN_SCORE held");
    assert_eq!(PINGS_INTERVAL_SECS, 2.0);

    // Releasing `IN_SCORE` immediately stops `svc_pings` emission again.
    a.queue_cmd(cs_proto48::delta::GoldSrcUserCmd {
        msec: 10,
        buttons: 0,
        ..Default::default()
    });
    h.step(&mut [&mut a], 5);
    let after_release = opcodes(&a, svc::PINGS);
    h.step(&mut [&mut a], frames(5.0));
    assert_eq!(
        opcodes(&a, svc::PINGS) - after_release,
        0,
        "must stop emitting svc_pings after IN_SCORE is released"
    );
}

#[test]
fn sendinfo_time_spaces_userinfo_broadcasts_by_one_second() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    h.step(&mut [&mut a, &mut b], 20);
    let slot_a = slot_of(&a);
    let updates_before = opcodes(&b, svc::UPDATEUSERINFO);

    // First `setinfo` applies immediately and arms the 1.0 s `sendinfo_time` cooldown.
    a.send_stringcmd("setinfo cl_updaterate 25");
    h.step(&mut [&mut a, &mut b], 10);
    assert_eq!(opcodes(&b, svc::UPDATEUSERINFO) - updates_before, 1);

    // Mid-second `setinfo` updates within `< 1.0 s` coalesce without broadcasting yet.
    a.send_stringcmd("setinfo model gign");
    h.step(&mut [&mut a, &mut b], 30);
    a.send_stringcmd("setinfo topcolor 77");
    h.step(&mut [&mut a, &mut b], 30);
    assert_eq!(
        opcodes(&b, svc::UPDATEUSERINFO) - updates_before,
        1,
        "mid-second setinfo updates must wait for 1.0 s sendinfo_time"
    );

    // Once 1.0 s has elapsed from the first broadcast, all coalesced keys flush in one update.
    h.step(&mut [&mut a, &mut b], 50);
    assert_eq!(opcodes(&b, svc::UPDATEUSERINFO) - updates_before, 2);
    let info = InfoString::parse(&b.userinfos()[&slot_a]);
    assert_eq!(info.get("cl_updaterate"), Some("25"));
    assert_eq!(info.get("model"), Some("gign"));
    assert_eq!(info.get("topcolor"), Some("77"));
}

#[test]
fn slot_is_reusable_after_disconnect() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    assert_eq!(slot_of(&a), 0);
    a.send_stringcmd("disconnect");
    h.step(&mut [&mut a], 10);
    assert_eq!(h.server.client_state(0), ClientState::Free);

    let c = joined(&mut h, "C", &mut []);
    assert_eq!(slot_of(&c), 0);
    assert_eq!(c.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
}

#[test]
fn frame_with_bad_time_is_skipped() {
    let mut h = Harness::new(4);
    let mut probe = h.add_probe("A");
    // The probe's `getchallenge` waits while the server skips bad frames.
    probe.pump(0.01);
    h.server.frame(&mut h.hub, f64::NAN);
    h.server.frame(&mut h.hub, f64::INFINITY);
    h.server.frame(&mut h.hub, -1.0);
    probe.pump(0.02);
    assert_eq!(probe.state(), &ProbeState::AwaitChallenge);
    assert_eq!(h.server.net_stats().frames_skipped, 3);

    h.server.frame(&mut h.hub, 0.03);
    probe.pump(0.04);
    assert_eq!(probe.state(), &ProbeState::AwaitConnection);

    // Time going backwards is skipped too.
    h.server.frame(&mut h.hub, 0.02);
    assert_eq!(h.server.net_stats().frames_skipped, 4);
    h.server.frame(&mut h.hub, 0.03);
    assert_eq!(h.server.net_stats().frames_skipped, 4);
}

fn server_with(cfg: ServerConfig) -> Server<TestGame> {
    Server::new(
        cfg,
        ServerMap::stub("test"),
        TestGame::default(),
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    )
}

#[test]
fn config_is_sanitised() {
    let defaults = ServerConfig::default();
    let server = server_with(ServerConfig {
        max_clients: 0,
        timeout_secs: f64::NAN,
        signon_timeout_secs: -1.0,
        min_update_rate: 0,
        max_update_rate: 0,
        min_rate: 0,
        max_rate: 10,
        ..ServerConfig::default()
    });
    let cfg = server.config();
    assert_eq!(cfg.max_clients, 1);
    assert_eq!(cfg.timeout_secs, defaults.timeout_secs);
    assert_eq!(cfg.signon_timeout_secs, defaults.signon_timeout_secs);
    assert_eq!((cfg.min_update_rate, cfg.max_update_rate), (1, 1));
    assert_eq!((cfg.min_rate, cfg.max_rate), (1000, 1000));

    let server = server_with(ServerConfig {
        max_clients: 200,
        timeout_secs: f64::INFINITY,
        min_update_rate: 30,
        max_update_rate: 20,
        ..ServerConfig::default()
    });
    let cfg = server.config();
    assert_eq!(cfg.max_clients, 32);
    assert_eq!(cfg.timeout_secs, defaults.timeout_secs);
    assert_eq!((cfg.min_update_rate, cfg.max_update_rate), (30, 30));
    assert_eq!(server.edicts().max_clients(), 32);

    // A valid configuration is kept as it is.
    let server = server_with(ServerConfig {
        timeout_secs: 5.0,
        ..ServerConfig::default()
    });
    assert_eq!(server.config().timeout_secs, 5.0);
    assert_eq!(server.config().max_clients, defaults.max_clients);
}

/// Runs the harness until every probe is in the game on the map whose path contains `map`.
fn rejoined(h: &mut Harness, probes: &mut [&mut Probe], map: &str) {
    let on_map = |p: &Probe| {
        p.state() == &ProbeState::InGame
            && p.server_info().is_some_and(|i| i.map_path.contains(map))
    };
    for _ in 0..500 {
        h.step(probes, 1);
        if probes.iter().all(|p| on_map(p)) {
            return;
        }
    }
    let states: Vec<_> = probes.iter().map(|p| p.state().clone()).collect();
    panic!("probes did not get in the game on {map}: {states:?}");
}

/// Entity numbers above the player slots that `probe` holds.
fn non_player_entities(h: &Harness, probe: &Probe) -> Vec<u16> {
    let max_clients = u16::from(h.server.config().max_clients);
    probe
        .entities()
        .keys()
        .copied()
        .filter(|&n| n > max_clients)
        .collect()
}

/// Entity numbers of the server's live `test_extra` edicts.
fn server_extras(h: &Harness) -> Vec<u16> {
    h.server
        .edicts()
        .iter_live()
        .filter(|(_, e)| e.vars.classname == "test_extra")
        .map(|(id, _)| id.index)
        .collect()
}

#[test]
fn probe_survives_changelevel() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut a], 20);
    let spawn_count = h.server.spawn_count();
    let slot = slot_of(&a);

    h.server.change_map(ServerMap::stub("second"));
    rejoined(&mut h, &mut [&mut a], "second");
    assert_eq!(h.server.spawn_count(), spawn_count + 1);
    let info = a.server_info().expect("server info");
    assert!(info.map_path.contains("second"), "{}", info.map_path);
    assert_eq!(info.spawn_count, spawn_count + 1);
    assert_eq!(h.server.client_state(slot), ClientState::Active);
    assert_eq!(h.server.game().client_disconnect_calls, 0);
}

#[test]
fn old_entities_are_gone_after_changelevel() {
    let game = TestGame {
        extra_entities: 5,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut a = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut a], 20);
    assert_eq!(non_player_entities(&h, &a).len(), 5);

    h.server.game_mut().extra_entities = 0;
    h.server.change_map(ServerMap::stub("second"));
    rejoined(&mut h, &mut [&mut a], "second");
    h.step(&mut [&mut a], 20);
    assert!(non_player_entities(&h, &a).is_empty());
    assert!(server_extras(&h).is_empty());
}

#[test]
fn precache_after_activate_is_too_late() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    h.server.game_mut().late_precache = None;
    h.server.change_map(ServerMap::stub("second"));
    rejoined(&mut h, &mut [&mut a], "second");
    assert_eq!(
        h.server.game().late_precache,
        Some(Err(PrecacheError::TooLate))
    );
    assert_eq!(h.server.game().client_put_in_server_calls, 2);
}

#[test]
fn two_clients_survive_changelevel() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    h.step(&mut [&mut a, &mut b], 20);
    let (slot_a, slot_b) = (slot_of(&a), slot_of(&b));
    let game = h.server.game();
    assert_eq!(
        (game.server_activate_calls, game.server_deactivate_calls),
        (1, 0)
    );

    h.server.change_map(ServerMap::stub("second"));
    // Frame by frame, so that an empty userinfo is seen before anything could replace it.
    for _ in 0..300 {
        h.step(&mut [&mut a, &mut b], 1);
        assert_ne!(a.userinfos().get(&slot_b).map(String::as_str), Some(""));
        assert_ne!(b.userinfos().get(&slot_a).map(String::as_str), Some(""));
    }
    rejoined(&mut h, &mut [&mut a, &mut b], "second");
    assert_eq!(h.server.client_state(slot_a), ClientState::Active);
    assert_eq!(h.server.client_state(slot_b), ClientState::Active);
    assert!(a.entities().contains_key(&(u16::from(slot_b) + 1)));
    assert!(b.entities().contains_key(&(u16::from(slot_a) + 1)));
    assert_eq!(name_of(&a, slot_b).as_deref(), Some("B"));
    assert_eq!(name_of(&b, slot_a).as_deref(), Some("A"));
    let game = h.server.game();
    // Nobody is in the server while the new map activates.
    assert!(game.active_at_activate.is_empty(), "{:?}", game.active_at_activate);
    assert_eq!(game.server_activate_calls, 2);
    assert_eq!(game.server_deactivate_calls, 1);
    assert_eq!(game.client_disconnect_calls, 0);
    assert_eq!(game.client_put_in_server_calls, 4);
}

#[test]
fn client_in_signon_survives_changelevel() {
    let mut h = Harness::new(4);
    let mut a = h.add_probe("A");
    // Until the server has sent the `new` stage, before the probe is in the game.
    for _ in 0..300 {
        h.step(&mut [&mut a], 1);
        if h.server.client_state(0) == ClientState::Signon(SignonStage::AwaitSendRes) {
            break;
        }
    }
    assert_eq!(
        h.server.client_state(0),
        ClientState::Signon(SignonStage::AwaitSendRes)
    );
    assert_ne!(a.state(), &ProbeState::InGame);

    h.server.change_map(ServerMap::stub("second"));
    assert_eq!(
        h.server.client_state(0),
        ClientState::Signon(SignonStage::AwaitNew)
    );
    rejoined(&mut h, &mut [&mut a], "second");
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.game().client_put_in_server_calls, 1);
}

#[test]
fn stale_spawn_after_changelevel_restarts_signon() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut a], 20);
    let slot = slot_of(&a);
    let old_count = h.server.spawn_count();
    h.server.change_map(ServerMap::stub("second"));

    // Once the server waits for `spawn` on the new map, a `spawn` with the old count goes out
    // ahead of the probe's own.
    for _ in 0..300 {
        h.step(&mut [&mut a], 1);
        if h.server.client_state(slot) == ClientState::Signon(SignonStage::AwaitSpawn) {
            break;
        }
    }
    assert_eq!(
        h.server.client_state(slot),
        ClientState::Signon(SignonStage::AwaitSpawn)
    );
    let put_in = h.server.game().client_put_in_server_calls;
    a.send_stringcmd(&format!("spawn {old_count} 0"));
    for _ in 0..50 {
        h.step(&mut [&mut a], 1);
        if h.server.client_state(slot) != ClientState::Signon(SignonStage::AwaitSpawn) {
            break;
        }
    }
    assert_eq!(h.server.game().client_put_in_server_calls, put_in);
    assert_eq!(
        h.server.client_state(slot),
        ClientState::Signon(SignonStage::AwaitNew)
    );

    rejoined(&mut h, &mut [&mut a], "second");
    assert_eq!(h.server.client_state(slot), ClientState::Active);
    assert_eq!(h.server.game().client_put_in_server_calls, put_in + 1);
}

#[test]
fn old_reliable_data_does_not_cross_the_change() {
    let game = TestGame {
        print_on_deactivate: true,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut a = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut a], 20);
    let prints = opcodes(&a, svc::PRINT);

    h.server.change_map(ServerMap::stub("second"));
    rejoined(&mut h, &mut [&mut a], "second");
    h.step(&mut [&mut a], 50);
    // Only the banner of the new `new` stage: the game's message to the old map is gone.
    assert_eq!(opcodes(&a, svc::PRINT), prints + 1);
}

#[test]
fn baselines_are_rebuilt_for_the_new_map() {
    let game = TestGame {
        extra_entities: 5,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut a = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut a], 20);
    let old_extras = server_extras(&h);
    assert_eq!(non_player_entities(&h, &a), old_extras);
    assert_eq!(baseline_skins(&h, &a), vec![0; 5]);

    // The new map's extras take the first two old indices, with another skin.
    h.server.game_mut().extra_entities = 2;
    h.server.game_mut().extra_skin = 7;
    h.server.change_map(ServerMap::stub("second"));
    let new_extras = server_extras(&h);
    assert_eq!(new_extras, old_extras[..2]);
    rejoined(&mut h, &mut [&mut a], "second");
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    h.step(&mut [&mut a, &mut b], 20);
    for probe in [&a, &b] {
        assert_eq!(non_player_entities(&h, probe), new_extras);
        let max_clients = u16::from(h.server.config().max_clients);
        let baselines: Vec<u16> = probe
            .baselines()
            .keys()
            .copied()
            .filter(|&n| n > max_clients)
            .collect();
        assert_eq!(baselines, new_extras);
        assert_eq!(baseline_skins(&h, probe), vec![7, 7]);
        assert!(new_extras.iter().all(|n| probe.entities()[n].skin == 7));
    }
}

/// `skin` of each spawn baseline above the player slots that `probe` holds.
fn baseline_skins(h: &Harness, probe: &Probe) -> Vec<i16> {
    let max_clients = u16::from(h.server.config().max_clients);
    probe
        .baselines()
        .iter()
        .filter(|&(&n, _)| n > max_clients)
        .map(|(_, state)| state.skin)
        .collect()
}

#[test]
fn stale_edict_id_does_not_resolve_after_changelevel() {
    let game = TestGame {
        extra_entities: 1,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let (old_id, _) = h
        .server
        .edicts()
        .iter_live()
        .find(|(_, e)| e.vars.classname == "test_extra")
        .expect("extra entity");
    h.server.change_map(ServerMap::stub("second"));
    let (new_id, _) = h
        .server
        .edicts()
        .iter_live()
        .find(|(_, e)| e.vars.classname == "test_extra")
        .expect("extra entity");
    assert_eq!(new_id.index, old_id.index);
    assert!(h.server.edicts().get(old_id).is_none());
    assert!(h.server.edicts().get(new_id).is_some());
}

#[test]
fn changelevel_twice_before_signon() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut a], 20);
    let signon_stufftexts = opcodes(&a, svc::STUFFTEXT);
    let spawn_count = h.server.spawn_count();
    let put_in = h.server.game().client_put_in_server_calls;

    h.server.change_map(ServerMap::stub("second"));
    h.server.change_map(ServerMap::stub("third"));
    rejoined(&mut h, &mut [&mut a], "third");
    h.step(&mut [&mut a], 50);
    assert_eq!(a.state(), &ProbeState::InGame);
    assert_eq!(h.server.spawn_count(), spawn_count + 2);
    assert_eq!(a.server_info().map(|i| i.spawn_count), Some(spawn_count + 2));
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.game().client_put_in_server_calls, put_in + 1);
    // One `reconnect`, then the stuffed text of one sign-on.
    assert_eq!(
        opcodes(&a, svc::STUFFTEXT),
        signon_stufftexts * 2 + 1,
        "{:?}",
        a.opcode_log()
    );
}

#[test]
fn changelevel_without_clients() {
    let mut h = Harness::new(4);
    h.step(&mut [], 5);
    h.server.change_map(ServerMap::stub("second"));
    assert_eq!(h.server.spawn_count(), 2);
    assert_eq!(h.server.client_count(), 0);
    let game = h.server.game();
    assert_eq!(
        (game.server_deactivate_calls, game.server_activate_calls),
        (1, 2)
    );
    let mut a = joined(&mut h, "A", &mut []);
    let info = a.server_info().expect("server info");
    assert!(info.map_path.contains("second"), "{}", info.map_path);
    assert_eq!(info.spawn_count, 2);
    h.step(&mut [&mut a], 5);
    assert_eq!(a.state(), &ProbeState::InGame);
}

#[test]
fn client_stalled_after_changelevel_is_dropped() {
    let cfg = ServerConfig {
        max_clients: 4,
        signon_timeout_secs: 2.0,
        timeout_secs: 1000.0,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_config(cfg, TestGame::default());
    let mut a = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut a], 20);

    h.server.change_map(ServerMap::stub("second"));
    // A is no longer pumped.
    h.step(&mut [], frames(2.0) - 5);
    assert_eq!(
        h.server.client_state(0),
        ClientState::Signon(SignonStage::AwaitNew)
    );
    h.step(&mut [], 10);
    assert_eq!(h.server.client_state(0), ClientState::Free);
    // It was not in the server on the new map.
    assert_eq!(h.server.game().client_disconnect_calls, 0);
}

#[test]
fn changelevel_during_a_fragmented_reliable_stream() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut a], 20);
    let prints = opcodes(&a, svc::PRINT);

    a.send_stringcmd("bigprint");
    let mut peak = 0;
    let mut partly_sent = false;
    for _ in 0..200 {
        h.step(&mut [&mut a], 1);
        let backlog = h.server.client_reliable_backlog(0);
        peak = peak.max(backlog);
        if peak > 0 && backlog > 0 && backlog + 2 * FRAGMENT_S2C_MAX_SIZE <= peak {
            partly_sent = true;
            break;
        }
    }
    assert!(partly_sent, "peak {peak}");
    assert!(peak > MAX_MSGLEN, "peak {peak}");
    assert_eq!(opcodes(&a, svc::PRINT), prints);

    h.server.change_map(ServerMap::stub("second"));
    rejoined(&mut h, &mut [&mut a], "second");
    h.step(&mut [&mut a], 100);
    assert_eq!(a.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    // Only the banner of the new `new` stage: no part of the old stream is delivered.
    assert_eq!(opcodes(&a, svc::PRINT), prints + 1);
}

#[test]
fn duplicated_reconnect_is_harmless() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut a], 20);
    let put_in = h.server.game().client_put_in_server_calls;
    let stufftexts = opcodes(&a, svc::STUFFTEXT);

    // A receives a reliable message whose acknowledgement the server never sees.
    a.withhold_reliable_ack(true);
    a.send_stringcmd("biginfo");
    for _ in 0..100 {
        h.step(&mut [&mut a], 1);
        if a.userinfos().contains_key(&15) {
            break;
        }
    }
    assert!(a.userinfos().contains_key(&15));
    assert!(h.server.client_reliable_backlog(0) > 0);

    h.server.change_map(ServerMap::stub("second"));
    a.withhold_reliable_ack(false);
    let mut entered = 0;
    let mut was_in_game = a.state() == &ProbeState::InGame;
    for _ in 0..500 {
        h.step(&mut [&mut a], 1);
        let in_game = a.state() == &ProbeState::InGame
            && a.server_info().is_some_and(|i| i.map_path.contains("second"));
        if in_game && !was_in_game {
            entered += 1;
        }
        was_in_game = a.state() == &ProbeState::InGame;
    }
    assert_eq!(entered, 1);
    assert_eq!(a.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(h.server.game().client_put_in_server_calls, put_in + 1);
    // The `reconnect` did arrive twice: the stuffed text of one sign-on plus two.
    assert_eq!(opcodes(&a, svc::STUFFTEXT), stufftexts * 2 + 2);
}

#[test]
fn changelevel_restarts_the_signon_timeout() {
    let cfg = ServerConfig {
        max_clients: 4,
        signon_timeout_secs: 2.0,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_config(cfg, TestGame::default());
    let mut a = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut a], frames(3.0));
    assert_eq!(h.server.client_state(0), ClientState::Active);

    h.server.change_map(ServerMap::stub("second"));
    h.step(&mut [&mut a], 1);
    assert_ne!(h.server.client_state(0), ClientState::Free);
    rejoined(&mut h, &mut [&mut a], "second");
    assert_eq!(h.server.client_state(0), ClientState::Active);
}

#[test]
fn reconnect_from_a_kicked_address_drops_stale_farewells() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut a], 20);

    // The probe starts over: `getchallenge`, then `connect`.
    a.connect();
    for _ in 0..50 {
        h.now += FRAME;
        a.pump(h.now);
        if a.state() == &ProbeState::AwaitConnection {
            break;
        }
        h.server.frame(&mut h.hub, h.now);
    }
    assert_eq!(a.state(), &ProbeState::AwaitConnection);
    // Kicked between frames: its farewells go out at the end of the frame that also accepts
    // the probe's `connect`.
    h.server.kick(0, "stale");
    h.server.frame(&mut h.hub, h.now);
    for _ in 0..300 {
        h.step(&mut [&mut a], 1);
        if a.state() == &ProbeState::InGame {
            break;
        }
    }
    assert_eq!(a.state(), &ProbeState::InGame);
    assert_eq!(h.server.client_state(0), ClientState::Active);
}

#[test]
fn game_cannot_set_userinfo_of_a_free_slot() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    let slot_a = slot_of(&a);
    a.send_stringcmd("disconnect");
    h.step(&mut [&mut a, &mut b], 20);
    assert_eq!(h.server.client_state(slot_a), ClientState::Free);
    assert_eq!(b.userinfos()[&slot_a], "");
    let updates = opcodes(&b, svc::UPDATEUSERINFO);

    b.send_stringcmd(&format!("setkey {slot_a} name Ghost"));
    h.step(&mut [&mut b], 20);
    assert!(h.server.game().commands.iter().any(|c| c[0] == "setkey"));
    assert_eq!(b.userinfos()[&slot_a], "");
    assert_eq!(opcodes(&b, svc::UPDATEUSERINFO), updates);
}

/// F19: a map change the game asks for while it deactivates the old map or activates the new one
/// (or before the switch) does not survive `change_map`.
#[test]
fn a_change_level_asked_during_the_switch_is_forgotten() {
    let game = TestGame {
        change_level_on_deactivate: Some("from_deactivate".to_string()),
        change_level_on_activate: Some("from_activate".to_string()),
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    // The first activation (before any switch) asked as well; nobody has taken it yet.
    h.server.change_map(ServerMap::stub("second"));
    assert_eq!(h.server.take_pending_change_level(), None);
    // After the switch the game can ask again, and that request stands.
    h.server.game_mut().change_level_on_deactivate = None;
    h.server.game_mut().change_level_on_activate = None;
    h.server.game_mut().act(|eng| {
        eng.change_level("later");
    });
    h.step(&mut [], 1);
    assert_eq!(
        h.server.take_pending_change_level(),
        Some("later".to_string())
    );
}
