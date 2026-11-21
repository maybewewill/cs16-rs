//! Per-client frames of `Server<TestGame>` as `cs-probe` clients see them over the loopback:
//! delta compression, visibility, update rate and bandwidth.

mod common;

use common::{Harness, TestGame, big_info};
use cs_engine::{ClientState, MAX_PACKET_ENTITIES, ServerConfig};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_proto48::messages::svc;
use cs_transport::{LoopbackEndpoint, MAX_UDP_PACKET, PacketTransport, PeerAddr};
use glam::Vec3;

type Probe = ProbeClient<LoopbackEndpoint>;

/// Position tolerance of the project.
const TOLERANCE: f32 = 0.03125;

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

fn server_origin(h: &Harness, slot: u8) -> Vec3 {
    let edicts = h.server.edicts();
    edicts
        .get(edicts.client(slot))
        .expect("client edict")
        .vars
        .origin
}

fn entity_number(probe: &Probe) -> u16 {
    u16::from(probe.player_slot().expect("slot")) + 1
}

fn forward() -> GoldSrcUserCmd {
    GoldSrcUserCmd {
        msec: 10,
        forwardmove: 250.0,
        ..Default::default()
    }
}

#[test]
fn second_probe_sees_first_probe_entity() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    h.step(&mut [&mut a, &mut b], 20);

    let index = entity_number(&a);
    let seen = b.entities().get(&index).expect("B sees A");
    let actual = server_origin(&h, a.player_slot().unwrap());
    assert!(
        (seen.origin - actual).abs().max_element() <= TOLERANCE,
        "{:?} vs {actual:?}",
        seen.origin
    );
    assert_eq!(b.stats().parse_errors, 0);
}

#[test]
fn stationary_world_frames_are_small_deltas() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    let deltas_before = b.stats().delta_updates;
    h.step(&mut [&mut a, &mut b], 100);

    assert!(b.stats().delta_updates > deltas_before);
    assert!(
        b.stats().last_datagram < 64,
        "last datagram {} bytes",
        b.stats().last_datagram
    );
    assert!(b.entities().contains_key(&entity_number(&a)));
    assert_eq!(b.stats().parse_errors, 0);
}

#[test]
fn stale_delta_request_falls_back_to_full_update() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut probe], 20);
    let deltas = probe.stats().delta_updates;
    let fulls = probe.stats().full_updates;
    assert!(deltas > 0, "acknowledged frames are sent as deltas");

    let seq = probe.incoming_sequence().expect("connected");
    probe.force_delta_seq(Some(seq.wrapping_sub(70) as u8));
    h.step(&mut [&mut probe], 10);
    assert!(probe.stats().full_updates > fulls);
    assert!(probe.entities().contains_key(&entity_number(&probe)));

    // Asking for the latest frame again brings deltas back.
    probe.force_delta_seq(None);
    let deltas = probe.stats().delta_updates;
    h.step(&mut [&mut probe], 10);
    assert!(probe.stats().delta_updates > deltas);
    assert_eq!(probe.stats().parse_errors, 0);
}

#[test]
fn moved_entity_is_updated_and_removed_entity_disappears() {
    let mut h = Harness::new(4);
    let mut a = joined(&mut h, "A", &mut []);
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    let index = entity_number(&a);
    let slot = a.player_slot().unwrap();
    let start = b.entities()[&index].origin;

    for _ in 0..20 {
        a.queue_cmd(forward());
        h.step(&mut [&mut a, &mut b], 1);
    }
    h.step(&mut [&mut a, &mut b], 10);
    let moved = server_origin(&h, slot);
    assert!(moved.x > start.x + 10.0, "A moved on the server: {moved:?}");
    let seen = b.entities()[&index].origin;
    assert!(
        (seen - moved).abs().max_element() <= TOLERANCE,
        "{seen:?} vs {moved:?}"
    );

    a.send_stringcmd("disconnect");
    h.step(&mut [&mut a, &mut b], 5);
    assert_eq!(h.server.client_state(slot), ClientState::Free);
    h.step(&mut [&mut b], 10);
    assert!(!b.entities().contains_key(&index));
    assert!(b.entities().contains_key(&entity_number(&b)));
    assert_eq!(b.stats().parse_errors, 0);
}

#[test]
fn update_rate_follows_userinfo() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "A", &mut []);
    probe.send_stringcmd("setinfo cl_updaterate 20");
    h.step(&mut [&mut probe], 100);
    h.step(&mut [&mut probe], 100);
    let frames = probe.stats().frames_in_last_second;
    assert!((19..=21).contains(&frames), "{frames} updates per second");
}

#[test]
fn no_update_is_sent_before_it_is_due() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "A", &mut []);
    probe.send_stringcmd("setinfo cl_updaterate 10");
    h.step(&mut [&mut probe], 200);
    let frames = probe.stats().frames_in_last_second;
    assert!((9..=11).contains(&frames), "{frames} updates per second");
}

/// A datagram whose entity block would not fit `MAX_DATAGRAM` carries no unreliable data at
/// all, as in HLDS: `SV_SendClientDatagram` clears an overflowed message, so the client gets
/// neither client data nor entities (`rehlds/engine/sv_main.cpp`, read 2026-11-21).
#[test]
fn oversized_frame_drops_the_whole_unreliable_part_as_hlds() {
    let game = TestGame {
        extra_entities: 800,
        churn_extras: true,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut probe = h.add_probe("A");
    for _ in 0..300 {
        h.step(&mut [&mut probe], 1);
        if h.server.client_state(0) == ClientState::Active {
            break;
        }
    }
    assert_eq!(h.server.client_state(0), ClientState::Active);
    let dropped = h.server.net_stats().entity_blocks_dropped;
    for _ in 0..30 {
        probe.queue_cmd(forward());
        h.step(&mut [&mut probe], 1);
    }
    h.step(&mut [&mut probe], 10);
    assert!(h.server.net_stats().entity_blocks_dropped > dropped + 10);
    assert!(probe.entities().is_empty(), "the entity block never fits");
    assert!(probe.client_data().is_none(), "client data goes with it");
    assert!(probe.stats().largest_datagram <= MAX_UDP_PACKET);
    assert_eq!(h.server.net_stats().send_failures, 0);
    assert_eq!(probe.stats().parse_errors, 0);
}

/// More visible entities than `MAX_PACKET_ENTITIES`: the update carries exactly that many
/// (the viewer's player edict and the lowest-numbered others), and the game is never asked
/// about the rest, as in HLDS `SV_WriteEntitiesToClient` (`rehlds/engine/sv_main.cpp`, read
/// 2026-11-21: "Too many entities in visible packet list.").
#[test]
fn packet_entities_follow_the_original_overflow_rule() {
    assert_eq!(MAX_PACKET_ENTITIES, 256, "rehlds/common/qlimits.h");
    let game = TestGame {
        extra_entities: 600,
        ..TestGame::default()
    };
    let cfg = ServerConfig {
        max_clients: 4,
        max_edicts: 700,
        ..ServerConfig::default()
    };
    let mut h = Harness::with_config(cfg, game);
    let mut probe = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut probe], 30);
    assert_eq!(h.server.net_stats().entity_blocks_dropped, 0);
    let own = entity_number(&probe);
    let seen: Vec<u16> = probe.entities().keys().copied().collect();
    assert_eq!(seen.len(), MAX_PACKET_ENTITIES, "{} entities", seen.len());
    assert!(seen.contains(&own));
    // Players come first, then the extras from the first edict after the four player slots.
    let last = 4 + (MAX_PACKET_ENTITIES as u16 - 1);
    assert!(seen.contains(&last), "{seen:?}");
    assert!(!seen.contains(&(last + 1)));
    let game = h.server.game();
    assert!(game.add_to_full_pack_ents.contains(&last));
    assert!(!game.add_to_full_pack_ents.contains(&(last + 1)));
    assert_eq!(probe.stats().parse_errors, 0);
}

#[test]
fn hidden_entity_is_not_sent() {
    let game = TestGame {
        extra_entities: 1,
        nodraw_extras: true,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut a = joined(&mut h, "A", &mut []);
    let mut b = joined(&mut h, "B", &mut [&mut a]);
    h.step(&mut [&mut a, &mut b], 20);
    // The first edict after the four player slots.
    let hidden = 5;
    for probe in [&a, &b] {
        assert!(!probe.entities().contains_key(&hidden));
        assert!(probe.entities().contains_key(&entity_number(probe)));
    }
    assert!(b.entities().contains_key(&entity_number(&a)));
    let game = h.server.game();
    assert!(!game.add_to_full_pack_ents.contains(&hidden));
    // The game was asked about the other player's entity, and each viewer's own.
    assert!(game.add_to_full_pack_ents.contains(&entity_number(&a)));
    for probe in [&a, &b] {
        let slot = probe.player_slot().unwrap();
        assert!(
            game.add_to_full_pack_pairs
                .contains(&(slot, entity_number(probe)))
        );
    }
}

#[test]
fn omitting_clc_delta_requests_a_full_update() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut probe], 20);
    assert!(probe.stats().delta_updates > 0);

    probe.suppress_delta(true);
    // Packets already on their way may still name a frame.
    h.step(&mut [&mut probe], 5);
    let (fulls, deltas) = (probe.stats().full_updates, probe.stats().delta_updates);
    h.step(&mut [&mut probe], 20);
    assert!(probe.stats().full_updates > fulls);
    assert_eq!(probe.stats().delta_updates, deltas);

    probe.suppress_delta(false);
    h.step(&mut [&mut probe], 20);
    assert!(probe.stats().delta_updates > deltas);
    assert_eq!(probe.stats().parse_errors, 0);
}

/// Entity updates the probe received, full and delta.
fn entity_updates(probe: &Probe) -> u64 {
    probe.stats().full_updates + probe.stats().delta_updates
}

/// Adds a probe that sets `rate` while it signs on and runs the harness until the server has
/// it `Active` (in slot 0); returns the probe.
fn active_at_rate(h: &mut Harness, rate: &str) -> Probe {
    let mut probe = h.add_probe("A");
    while probe.state() != &ProbeState::Signon {
        h.step(&mut [&mut probe], 1);
        assert!(h.now < 5.0, "no connection");
    }
    probe.send_stringcmd(&format!("setinfo rate {rate}"));
    // Sign-on datagrams wait for the rate too: at 1000 bytes per second this takes a while.
    while h.server.client_state(0) != ClientState::Active {
        h.step(&mut [&mut probe], 1);
        assert!(h.now < 15.0, "not active");
    }
    probe
}

#[test]
fn low_rate_client_still_receives_updates() {
    let game = TestGame {
        // A full update of about 2000 bytes: at 1000 bytes per second it costs two of the ten
        // seconds, leaving room for the deltas after it.
        extra_entities: 80,
        scatter_extras: true,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut probe = active_at_rate(&mut h, "1000");
    let rate = u64::from(ServerConfig::default().min_rate);
    let bytes_before = probe.stats().bytes_in;

    let mut origins = Vec::new();
    for _ in 0..10 {
        for _ in 0..100 {
            probe.queue_cmd(forward());
            h.step(&mut [&mut probe], 1);
        }
        origins.push(probe.client_data().map(|c| c.origin.x));
    }
    let bytes = probe.stats().bytes_in - bytes_before;
    assert!(
        probe.stats().largest_datagram > 3 * rate as usize / 2,
        "the full update is about 2 KB: {}",
        probe.stats().largest_datagram
    );
    assert!(entity_updates(&probe) >= 3, "{:?}", probe.stats());
    // Client data kept following the moving player, second by second.
    let distinct: std::collections::BTreeSet<u32> =
        origins.iter().flatten().map(|x| x.to_bits()).collect();
    assert!(distinct.len() >= 5, "{origins:?}");
    let seen = probe.entities()[&entity_number(&probe)].origin;
    assert!(seen.x > 100.0, "the entity view follows too: {seen:?}");
    let limit = rate * 10 + MAX_UDP_PACKET as u64;
    assert!(bytes <= limit, "{bytes} bytes in ten seconds, limit {limit}");
    assert_eq!(probe.stats().parse_errors, 0);
}

#[test]
fn choked_updates_are_announced_in_the_next_update() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "A", &mut []);
    probe.send_stringcmd("setinfo rate 1000");
    probe.send_stringcmd("setinfo cl_updaterate 102");
    h.step(&mut [&mut probe], 100);

    let log_start = probe.opcode_log().len();
    let datagrams = probe.stats().datagrams_in;
    let updates = entity_updates(&probe);
    h.step(&mut [&mut probe], 300);
    let log = &probe.opcode_log()[log_start..];
    let chokes = log.iter().filter(|&&op| op == svc::CHOKE).count();
    assert!(chokes > 0, "updates were choked");
    // Every run of chokes leads into an update.
    for (i, w) in log.windows(2).enumerate() {
        if w[0] == svc::CHOKE {
            assert!(
                matches!(w[1], svc::CHOKE | svc::TIME),
                "choke at {i} followed by {}",
                w[1]
            );
        }
    }
    assert_ne!(log.last(), Some(&svc::CHOKE));
    // No reliable traffic is left by now, so every datagram is an update: none carries only
    // chokes.
    let new_datagrams = probe.stats().datagrams_in - datagrams;
    assert_eq!(new_datagrams, entity_updates(&probe) - updates);
    assert!(new_datagrams < 150, "{new_datagrams} updates in three seconds at 1000 bytes/s");
    assert_eq!(probe.stats().parse_errors, 0);
}

#[test]
fn signon_at_minimum_rate_reaches_updates_promptly() {
    let mut h = Harness::new(4);
    let mut probe = active_at_rate(&mut h, "1000");
    let active_at = h.now;
    while entity_updates(&probe) == 0 {
        h.step(&mut [&mut probe], 1);
        assert!(
            h.now - active_at <= MAX_UDP_PACKET as f64 / 1000.0 + 1.0,
            "no entity update {} s after becoming active",
            h.now - active_at
        );
    }
    assert_eq!(probe.state(), &ProbeState::InGame);
}

#[test]
fn reliable_message_and_large_frame_share_the_datagram_budget() {
    let game = TestGame {
        // A full update of about 3000 bytes.
        extra_entities: 120,
        scatter_extras: true,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut probe = joined(&mut h, "A", &mut []);
    // Frequent updates, each a full one with a large entity block.
    probe.send_stringcmd("setinfo rate 100000");
    probe.send_stringcmd("setinfo cl_updaterate 102");
    probe.suppress_delta(true);
    h.step(&mut [&mut probe], 50);
    let frame_size = probe.stats().last_datagram;
    let info = big_info();
    assert!(
        frame_size + info.len() > MAX_UDP_PACKET,
        "frame {frame_size} + message {} must not fit one datagram",
        info.len()
    );

    let dropped = h.server.net_stats().entity_blocks_dropped;
    let origin = probe.client_data().expect("client data").origin.x;
    probe.send_stringcmd("biginfo");
    while !probe.userinfos().contains_key(&15) {
        probe.queue_cmd(forward());
        h.step(&mut [&mut probe], 1);
        assert!(h.now < 30.0, "the reliable message never arrived");
    }
    // The message travelled in an update whose unreliable part gave way to it (HLDS drops the
    // whole of it, `Netchan_Transmit`: "Unreliable would overfow, ignoring"); the updates after
    // it bring client data again.
    assert!(h.server.net_stats().entity_blocks_dropped > dropped);
    for _ in 0..20 {
        probe.queue_cmd(forward());
        h.step(&mut [&mut probe], 1);
    }
    assert!(probe.client_data().expect("client data").origin.x > origin);
    assert_eq!(probe.userinfos()[&15], info);
    let fulls = probe.stats().full_updates;
    h.step(&mut [&mut probe], 20);
    assert!(probe.stats().full_updates > fulls, "entity updates resume");
    assert!(probe.stats().largest_datagram <= MAX_UDP_PACKET);
    assert_eq!(h.server.net_stats().send_failures, 0);
    assert_eq!(probe.stats().parse_errors, 0);
}

#[test]
fn reliable_flood_does_not_exceed_the_rate() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "A", &mut []);
    let rate = 30_000; // the probe's userinfo
    let bytes_before = probe.stats().bytes_in;
    let origin = probe.client_data().expect("client data").origin.x;
    h.server.game_mut().flood_per_usercmd = 2000;

    // 2000 reliable bytes per frame against 30000 bytes per second: the backlog passes
    // MAX_RELIABLE_BACKLOG within the three seconds and the client is dropped.
    let mut dropped_at = None;
    let mut last_origin = origin;
    for _ in 0..300 {
        probe.queue_cmd(forward());
        h.step(&mut [&mut probe], 1);
        if dropped_at.is_none() {
            if h.server.client_state(0) == ClientState::Free {
                dropped_at = Some(h.now);
            } else {
                last_origin = probe.client_data().expect("client data").origin.x;
            }
        }
    }
    let bytes = probe.stats().bytes_in - bytes_before;
    let limit = rate * 3 + MAX_UDP_PACKET as u64;
    assert!(bytes <= limit, "{bytes} bytes in three seconds, limit {limit}");
    assert!(dropped_at.is_some(), "the reliable backlog cap drops the client");
    assert!(last_origin > origin + 10.0, "client data kept coming: {origin} -> {last_origin}");
    assert_eq!(probe.stats().parse_errors, 0);
}

#[test]
fn withheld_reliable_ack_is_not_amplified() {
    let mut h = Harness::new(4);
    let mut probe = h.add_probe("A");
    while probe.state() != &ProbeState::Signon {
        h.step(&mut [&mut probe], 1);
        assert!(h.now < 5.0, "no connection");
    }
    // From its first packet on, the client asks for a low rate and never acknowledges a
    // reliable message: the server's sign-on data is resent again and again.
    let rate = 5_000;
    probe.send_stringcmd(&format!("setinfo rate {rate}"));
    probe.withhold_reliable_ack(true);
    h.step(&mut [&mut probe], 50);
    assert!(h.server.client_state(0) != ClientState::Active);

    let bytes_before = probe.stats().bytes_in;
    h.step(&mut [&mut probe], 200);
    let bytes = probe.stats().bytes_in - bytes_before;
    assert!(bytes > 0, "the server keeps resending");
    let limit = rate * 2 + MAX_UDP_PACKET as u64;
    assert!(bytes <= limit, "{bytes} bytes in two seconds, limit {limit}");
}

#[test]
fn rejected_datagram_keeps_the_delta_request() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h, "A", &mut []);
    h.step(&mut [&mut probe], 20);
    let (fulls, deltas) = (probe.stats().full_updates, probe.stats().delta_updates);

    // The probe is not pumped: its last packet asked for a delta. Each frame the server gets
    // only a stale datagram (sequence 1) from its address, which the netchan refuses.
    let rejected = h.server.net_stats().datagrams_rejected;
    let mut stale = vec![1u8; 16];
    stale[..4].copy_from_slice(&1u32.to_le_bytes());
    stale[4..8].copy_from_slice(&0u32.to_le_bytes());
    for _ in 0..10 {
        probe
            .transport_mut()
            .send(PeerAddr::Loopback(0), &stale)
            .expect("loopback send");
        h.step(&mut [], 1);
    }
    assert!(h.server.net_stats().datagrams_rejected >= rejected + 10);
    h.step(&mut [&mut probe], 1);
    assert_eq!(probe.stats().full_updates, fulls);
    assert!(probe.stats().delta_updates > deltas);
    assert_eq!(probe.stats().parse_errors, 0);
}
