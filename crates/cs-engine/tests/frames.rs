//! Per-client frames of `Server<TestGame>` as `cs-probe` clients see them over the loopback:
//! delta compression, visibility, update rate and bandwidth.

mod common;

use common::{Harness, TestGame};
use cs_engine::{ClientState, ServerConfig};
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

#[test]
fn oversized_frame_drops_entities_but_keeps_clientdata() {
    let game = TestGame {
        extra_entities: 800,
        churn_extras: true,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut probe = h.add_probe("A");
    for _ in 0..300 {
        h.step(&mut [&mut probe], 1);
        if h.server.client_state(0) == ClientState::Active && probe.client_data().is_some() {
            break;
        }
    }
    assert_eq!(h.server.client_state(0), ClientState::Active);
    let first = probe.client_data().expect("client data").origin;

    for _ in 0..30 {
        probe.queue_cmd(forward());
        h.step(&mut [&mut probe], 1);
    }
    h.step(&mut [&mut probe], 10);
    let later = probe.client_data().expect("client data").origin;
    assert!(later.x > first.x + 10.0, "{first:?} -> {later:?}");
    assert!(
        (later - server_origin(&h, 0)).abs().max_element() <= TOLERANCE,
        "client data follows the server"
    );
    assert!(probe.entities().is_empty(), "the entity block never fits");
    assert!(probe.stats().largest_datagram <= MAX_UDP_PACKET);
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

#[test]
fn choke_is_sent_when_rate_is_exhausted() {
    let game = TestGame {
        extra_entities: 100,
        churn_extras: true,
        ..TestGame::default()
    };
    let mut h = Harness::with_game(4, game);
    let mut probe = joined(&mut h, "A", &mut []);
    probe.send_stringcmd("setinfo rate 1");
    h.step(&mut [&mut probe], 100);

    let min_rate = u64::from(ServerConfig::default().min_rate);
    let chokes_before = choke_count(&probe);
    let bytes_before = probe.stats().bytes_in;
    h.step(&mut [&mut probe], 500);
    let bytes = probe.stats().bytes_in - bytes_before;
    assert!(choke_count(&probe) > chokes_before, "svc_choke received");
    assert!(
        bytes <= 5 * min_rate + min_rate,
        "{bytes} bytes in five seconds at {min_rate} bytes per second"
    );
    assert_eq!(probe.stats().parse_errors, 0);
}

fn choke_count(probe: &Probe) -> usize {
    probe
        .opcode_log()
        .iter()
        .filter(|&&op| op == svc::CHOKE)
        .count()
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
