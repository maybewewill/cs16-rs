//! The end of a client's life and the housekeeping around it, as `cs-probe` clients see it over
//! the loopback: kicks, timeouts, `disconnect`, shutdown, userinfo updates and pings.

mod common;

use common::{FRAME, GAME_KICK_REASON, Harness, TestGame};
use cs_cvar::CVarRegistry;
use cs_engine::{
    ClientState, MAX_RELIABLE_BACKLOG, PINGS_INTERVAL_SECS, SETINFO_BURST, SETINFO_RATE_PER_SEC,
    Server, ServerConfig, ServerMap,
};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::delta::DeltaRegistry;
use cs_proto48::info::InfoString;
use cs_proto48::messages::svc;
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
fn shutdown_refuses_new_connections() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    h.server.shutdown(&mut h.hub, "bye");
    h.step(&mut [&mut a], 2);

    let mut late = h.add_probe("Late");
    h.step(&mut [&mut late], 50);
    assert_eq!(
        late.state(),
        &ProbeState::Rejected("Server is shutting down.".to_string())
    );
    assert_eq!(h.server.client_count(), 0);

    // Queries are still answered.
    let mut query = vec![0xFF; 4];
    query.push(A2S_INFO);
    query.extend_from_slice(A2S_INFO_PAYLOAD);
    let mut asker = h.hub.connect();
    asker
        .send(PeerAddr::Loopback(0), &query)
        .expect("loopback send");
    h.step(&mut [], 1);
    assert!(asker.recv().is_some());
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
    let first_update = log.iter().position(|&op| op == svc::CLIENTDATA);
    assert!(shout_at < first_update, "{log:?}");
}

#[test]
fn pings_are_sent_every_two_seconds() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    let before = opcodes(&a, svc::PINGS);
    h.step(&mut [&mut a], frames(5.0));
    let pings = opcodes(&a, svc::PINGS) - before;
    assert!((2..=3).contains(&pings), "{pings} svc_pings in 5 s");
    assert_eq!(PINGS_INTERVAL_SECS, 2.0);
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
