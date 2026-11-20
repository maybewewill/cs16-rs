//! Hostile traffic through `Server::frame` over the loopback: connectionless datagrams from
//! many forged addresses, and more datagrams than one frame reads.

mod common;

use common::{Harness, Tap};
use cs_engine::{ClientState, MAX_DATAGRAMS_PER_FRAME, OOB_GLOBAL_REPLIES_PER_SEC};
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::handshake::encode_getchallenge;
use cs_transport::{LoopbackEndpoint, PeerAddr};

type Probe = ProbeClient<LoopbackEndpoint>;

/// First loopback id used for forged addresses, far above the ids of real endpoints.
const FORGED: u32 = 100_000;

/// Adds a probe and runs the harness until it is in the game.
fn joined(h: &mut Harness) -> Probe {
    let mut probe = h.add_probe("Player");
    h.step(&mut [&mut probe], 200);
    assert_eq!(probe.state(), &ProbeState::InGame);
    probe
}

fn entity_updates(probe: &Probe) -> u64 {
    probe.stats().full_updates + probe.stats().delta_updates
}

/// Replies the tap saw going to forged addresses.
fn forged_replies(tap: &Tap) -> Vec<(PeerAddr, usize)> {
    tap.sent
        .iter()
        .copied()
        .filter(|(to, _)| matches!(to, PeerAddr::Loopback(id) if *id >= FORGED))
        .collect()
}

/// 64-bit LCG (Knuth's MMIX constants) seeded `0xC516`; each step yields the state's high byte.
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
fn random_connectionless_datagrams_from_many_addresses() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h);
    let updates_before = entity_updates(&probe);

    // 10,000 datagrams: the connectionless prefix and 0 to 64 random bytes, from 3,000
    // addresses, 500 per frame.
    let mut rng = Lcg(0xC516);
    let mut replies = Vec::new();
    for i in 0..10_000usize {
        let mut data = vec![0xFF; 4];
        data.extend((0..i % 65).map(|_| rng.next_byte()));
        h.hub.inject(FORGED + (i % 3_000) as u32, &data);
        if i % 500 == 499 {
            replies.extend(forged_replies(&h.step_tapped(&mut [&mut probe])));
        }
    }
    for _ in 0..10 {
        replies.extend(forged_replies(&h.step_tapped(&mut [&mut probe])));
    }

    assert!(!replies.is_empty(), "some datagram is answered");
    for (to, len) in &replies {
        assert!(*len <= 64, "{len} byte reply to {to:?}");
    }
    assert_eq!(h.server.client_count(), 1, "no slot is taken");
    assert_eq!(h.server.client_state(0), ClientState::Active);
    assert_eq!(probe.state(), &ProbeState::InGame);
    // 30 frames at the probe's `cl_updaterate` of 60 are 18 updates.
    assert!(entity_updates(&probe) > updates_before + 15);
    assert_eq!(probe.stats().parse_errors, 0);
}

#[test]
fn challenge_burst_from_one_address_is_limited() {
    let mut h = Harness::new(4);
    let from = FORGED;
    let getchallenge = encode_getchallenge(true);
    let denied = h.server.net_stats().oob_denied;
    for _ in 0..100 {
        h.hub.inject(from, &getchallenge);
    }
    let tap = h.step_tapped(&mut []);
    let replies = tap.sent_to(PeerAddr::Loopback(from));
    assert!(replies <= 20, "{replies} replies");
    assert!(replies > 0, "the burst is answered");
    assert_eq!(h.server.net_stats().oob_denied, denied + 100 - replies as u64);
}

#[test]
fn challenges_from_many_addresses_share_a_global_budget() {
    let mut h = Harness::new(4);
    let getchallenge = encode_getchallenge(true);
    for i in 0..5_000 {
        h.hub.inject(FORGED + i, &getchallenge);
    }
    // One frame reads at most MAX_DATAGRAMS_PER_FRAME of them, the next the rest.
    let first = forged_replies(&h.step_tapped(&mut [])).len();
    assert_eq!(first, OOB_GLOBAL_REPLIES_PER_SEC as usize);
    let second = forged_replies(&h.step_tapped(&mut [])).len();
    // The second frame's refill: one frame of the global rate.
    let refill = (OOB_GLOBAL_REPLIES_PER_SEC * common::FRAME).ceil() as usize;
    assert!(second <= refill, "{second} replies in the second frame");
    assert!(h.server.net_stats().oob_denied >= (5_000 - first - second) as u64);
}

#[test]
fn a_flood_is_read_over_several_frames() {
    let mut h = Harness::new(4);
    let mut probe = joined(&mut h);
    // An update every frame.
    probe.send_stringcmd("setinfo cl_updaterate 102");
    h.step(&mut [&mut probe], 20);
    let me = PeerAddr::Loopback(probe.transport().id());

    // Datagrams from addresses without a slot that are not connectionless: each is read and
    // ignored.
    for i in 0..10_000u32 {
        h.hub.inject(FORGED + i, &[0u8; 16]);
    }
    let first = h.step_tapped(&mut [&mut probe]);
    assert_eq!(first.received, MAX_DATAGRAMS_PER_FRAME);
    assert_eq!(first.sent_to(me), 1, "the update goes out in the first frame");
    let second = h.step_tapped(&mut [&mut probe]);
    assert_eq!(second.received, MAX_DATAGRAMS_PER_FRAME);
    let third = h.step_tapped(&mut [&mut probe]);
    // The rest of the flood, and the probe's own datagrams.
    assert!(third.received >= 10_000 - 2 * MAX_DATAGRAMS_PER_FRAME);
    assert!(third.received < MAX_DATAGRAMS_PER_FRAME, "all read by the third frame");
    assert_eq!(h.server.client_state(0), ClientState::Active);
    h.step(&mut [&mut probe], 10);
    assert_eq!(probe.state(), &ProbeState::InGame);
    assert_eq!(probe.stats().parse_errors, 0);
}
