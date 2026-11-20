//! Endurance runs of `Server<TestGame>` with eight `cs-probe` clients over a lossy loopback:
//! every endpoint drops every 17th datagram while the probes send random commands.

mod common;

use std::time::Instant;

use common::Harness;
use cs_engine::ClientState;
use cs_probe::{ProbeClient, ProbeState};
use cs_proto48::delta::GoldSrcUserCmd;
use cs_transport::{Faults, LoopbackEndpoint};
use glam::Vec3;

type Probe = ProbeClient<LoopbackEndpoint>;

/// Position tolerance of the project.
const TOLERANCE: f32 = 0.03125;
/// Clients in every run.
const CLIENTS: u8 = 8;
/// Frames pumped without new commands before the final checks, so both sides are at rest.
const SETTLE_FRAMES: usize = 50;

/// 64-bit LCG (Knuth's MMIX constants).
struct Lcg(u64);

impl Lcg {
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    /// A value in `lo..hi` from the state's high 24 bits.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let unit = (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32;
        lo + (hi - lo) * unit
    }

    /// A random 10 ms command.
    fn cmd(&mut self) -> GoldSrcUserCmd {
        GoldSrcUserCmd {
            msec: 10,
            forwardmove: self.range(-250.0, 250.0),
            sidemove: self.range(-250.0, 250.0),
            viewangles: Vec3::new(self.range(-89.0, 89.0), self.range(0.0, 360.0), 0.0),
            ..Default::default()
        }
    }
}

/// Connects eight probes with `drop_every = 17` on each endpoint, runs `frames` frames of random
/// commands, lets both sides settle, then checks every client.
fn soak(frames: usize) {
    let started = Instant::now();
    let mut h = Harness::new(CLIENTS);
    let mut probes: Vec<Probe> = (0..CLIENTS)
        .map(|i| {
            let mut probe = h.add_probe(&format!("Soak{i}"));
            probe.transport_mut().set_faults(Faults {
                drop_every: 17,
                ..Faults::default()
            });
            probe
        })
        .collect();

    for _ in 0..1000 {
        if probes.iter().all(|p| p.state() == &ProbeState::InGame) {
            break;
        }
        step(&mut h, &mut probes, 1);
    }
    let states: Vec<_> = probes.iter().map(|p| p.state().clone()).collect();
    assert!(
        states.iter().all(|s| s == &ProbeState::InGame),
        "{states:?}"
    );

    let mut rng = Lcg(0xC516);
    for _ in 0..frames {
        for probe in &mut probes {
            probe.queue_cmd(rng.cmd());
        }
        step(&mut h, &mut probes, 1);
    }
    step(&mut h, &mut probes, SETTLE_FRAMES);

    let stats = h.server.net_stats();
    assert_eq!(stats.frames_skipped, 0, "{stats:?}");
    // Every datagram the server built was within the transport's limit.
    assert_eq!(stats.send_failures, 0, "{stats:?}");
    for probe in &probes {
        let slot = probe.player_slot().expect("slot");
        assert_eq!(probe.state(), &ProbeState::InGame, "slot {slot}");
        assert_eq!(
            h.server.client_state(slot),
            ClientState::Active,
            "slot {slot}"
        );

        let edicts = h.server.edicts();
        let server = edicts.get(edicts.client(slot)).expect("edict").vars.origin;
        let client = probe.client_data().expect("client data").origin;
        assert!(
            (client - server).abs().max_element() <= TOLERANCE,
            "slot {slot}: client {client:?} server {server:?}"
        );

        let probe_stats = probe.stats();
        assert!(
            probe_stats.largest_datagram <= 4010,
            "slot {slot}: {probe_stats:?}"
        );
        assert_eq!(probe_stats.parse_errors, 0, "slot {slot}");
        let ring = h.server.client_frame_count(slot).expect("connected slot");
        assert!((1..=64).contains(&ring), "slot {slot}: {ring} frames");
        let faults = probe.transport().fault_stats();
        assert!(faults.dropped > 0, "slot {slot}: {faults:?}");
    }
    println!(
        "{frames} frames with {CLIENTS} clients in {:.1} s; {stats:?}",
        started.elapsed().as_secs_f64()
    );
}

fn step(h: &mut Harness, probes: &mut [Probe], frames: usize) {
    let mut all: Vec<&mut Probe> = probes.iter_mut().collect();
    h.step(&mut all, frames);
}

#[test]
fn soak_3000_frames_8_clients() {
    soak(3000);
}

/// Ten minutes of model time.
#[test]
#[ignore]
fn soak_60000_frames_8_clients() {
    soak(60_000);
}
