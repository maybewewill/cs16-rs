//! Rolling NetChan telemetry ring buffer (`NET_TIMINGS = 256`) for GoldSrc `net_graph 1/2/3`.
//!
//! Ported from Xash3D FWGS (GPL-3.0-or-later; read 2026-11-25):
//! - `engine/client/cl_netgraph.c` (`NET_TIMINGS`, `NetGraph_GetFrameData`, `LATENCY_AVG_FRAC`,
//!   `PACKETLOSS_AVG_FRAC`, `PACKETCHOKE_AVG_FRAC`, `NUM_LATENCY_SAMPLES`)
//! - `engine/client/parse/cl_parse.c` (`CL_ParseClientData` latency & drop tracking, `svc_choke`)
//! - `engine/common/net_chan.c` (`Netchan_UpdateFlow`)

use std::collections::VecDeque;

/// Number of entries in the `net_graph` timing ring buffers (`cl_netgraph.c`).
pub const NET_TIMINGS: usize = 256;
pub const NET_TIMINGS_MASK: usize = NET_TIMINGS - 1;

/// `MULTIPLAYER_BACKUP` (`netchan.h` / `frames.rs`): window over which loss and choke % are computed.
const UPDATE_BACKUP: u32 = 64;

/// Smoothing fractions from `engine/client/cl_netgraph.c` (read 2026-11-25).
pub const PACKETLOSS_AVG_FRAC: f32 = 0.5;
pub const PACKETCHOKE_AVG_FRAC: f32 = 0.5;
pub const NUM_LATENCY_SAMPLES: u32 = 8;

/// Classification of an incoming frame slot in `NetGraphTelemetry`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PacketSampleStatus {
    #[default]
    Empty,
    Valid,
    Dropped,
    InvalidDelta,
    Skipped,
}

/// Telemetry for one incoming `NetChan` sequence slot (`sequence & NET_TIMINGS_MASK`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct IncomingTimingSample {
    pub sequence: u32,
    pub status: PacketSampleStatus,
    pub choked: bool,
    pub latency_sec: f32,
    pub bytes: u32,
    pub received_at: f64,
}

/// Telemetry for one outgoing `NetChan` sequence slot (`sequence & NET_TIMINGS_MASK`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OutgoingTimingSample {
    pub sequence: u32,
    pub sent_at: f64,
    pub bytes: u32,
    pub sent: bool,
    pub cmd_lerp: f32,
    pub lerp_msec: i16,
}

/// Rolling `NET_TIMINGS = 256` NetChan telemetry maintained by `Connection`.
#[derive(Debug, Clone)]
pub struct NetGraphTelemetry {
    incoming: [IncomingTimingSample; NET_TIMINGS],
    outgoing: [OutgoingTimingSample; NET_TIMINGS],
    in_window: VecDeque<(f64, usize)>,
    out_window: VecDeque<(f64, usize)>,
    latest_incoming_seq: u32,
    latest_outgoing_seq: u32,
    has_incoming: bool,
    last_in_bytes: u32,
    last_out_bytes: u32,
    ping_ms: f32,
    loss_percent: f32,
    choke_percent: f32,
    lerp_msec: i16,
    updaterate: f32,
    cmdrate: f32,
    max_msg_bytes: u32,
    total_dropped: u64,
    total_choked: u64,
    total_invalid_delta: u64,
}

impl Default for NetGraphTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl NetGraphTelemetry {
    pub fn new() -> Self {
        Self {
            incoming: [IncomingTimingSample::default(); NET_TIMINGS],
            outgoing: [OutgoingTimingSample::default(); NET_TIMINGS],
            in_window: VecDeque::new(),
            out_window: VecDeque::new(),
            latest_incoming_seq: 0,
            latest_outgoing_seq: 0,
            has_incoming: false,
            last_in_bytes: 0,
            last_out_bytes: 0,
            ping_ms: 0.0,
            loss_percent: 0.0,
            choke_percent: 0.0,
            lerp_msec: 0,
            updaterate: 60.0,
            cmdrate: 60.0,
            max_msg_bytes: 0,
            total_dropped: 0,
            total_choked: 0,
            total_invalid_delta: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Prune 1-second rolling traffic windows at `now`.
    pub fn prune_windows(&mut self, now: f64) {
        while self.in_window.front().is_some_and(|&(t, _)| t <= now - 1.0) {
            self.in_window.pop_front();
        }
        while self.out_window.front().is_some_and(|&(t, _)| t <= now - 1.0) {
            self.out_window.pop_front();
        }
    }

    /// Record a newly accepted incoming `NetChan` packet (`CL_ReadNetMessage` / `CL_ParseClientData`).
    pub fn record_incoming(
        &mut self,
        sequence: u32,
        prev_sequence: u32,
        sequence_ack: u32,
        bytes: usize,
        now: f64,
        updaterate: f32,
    ) {
        if updaterate.is_finite() && updaterate > 0.0 {
            self.updaterate = updaterate;
        }

        // Mark any sequence gap as dropped (`cl_parse.c:1007-1018`).
        // Skip the very first packet when `prev_sequence == 0` so an arbitrary initial server seq
        // is not miscounted as dropped packets.
        if self.has_incoming && prev_sequence > 0 && sequence > prev_sequence.saturating_add(1) {
            let gap = sequence - prev_sequence - 1;
            let mark_count = gap.min(NET_TIMINGS as u32);
            let start = sequence - mark_count;
            for missed in start..sequence {
                let slot = &mut self.incoming[(missed as usize) & NET_TIMINGS_MASK];
                *slot = IncomingTimingSample {
                    sequence: missed,
                    status: PacketSampleStatus::Dropped,
                    choked: false,
                    latency_sec: 0.0,
                    bytes: 0,
                    received_at: now,
                };
            }
            self.total_dropped += u64::from(gap);
        }

        self.has_incoming = true;
        self.latest_incoming_seq = sequence;
        let byte_u32 = bytes.min(u32::MAX as usize) as u32;
        self.last_in_bytes = byte_u32;
        self.max_msg_bytes = self.max_msg_bytes.max(byte_u32).min(1000);
        self.in_window.push_back((now, bytes));
        self.prune_windows(now);

        // RTT from matching outgoing packet (`cl_parse.c:1076-1090`).
        let mut latency_sec = 0.0f32;
        if sequence_ack > 0 {
            let ack_slot = self.outgoing[(sequence_ack as usize) & NET_TIMINGS_MASK];
            if ack_slot.sent && ack_slot.sequence == sequence_ack && now >= ack_slot.sent_at {
                let rtt = (now - ack_slot.sent_at) as f32;
                if rtt.is_finite() && (0.0..=2.0).contains(&rtt) {
                    latency_sec = rtt;
                }
            }
        }

        let slot = &mut self.incoming[(sequence as usize) & NET_TIMINGS_MASK];
        *slot = IncomingTimingSample {
            sequence,
            status: PacketSampleStatus::Valid,
            choked: false,
            latency_sec,
            bytes: byte_u32,
            received_at: now,
        };

        self.recompute_averages();
    }

    /// Mark an incoming sequence as `svc_choke` (`cl_parse.c:2480-2483`).
    pub fn mark_choked(&mut self, sequence: u32) {
        if sequence == 0 {
            return;
        }
        let slot = &mut self.incoming[(sequence as usize) & NET_TIMINGS_MASK];
        if slot.sequence == sequence && !slot.choked {
            slot.choked = true;
            self.total_choked += 1;
            self.recompute_averages();
        }
    }

    /// Mark an incoming sequence as having an invalid/refused delta frame (`cl_netgraph.c:197-199`).
    pub fn mark_invalid_delta(&mut self, sequence: u32) {
        if sequence == 0 {
            return;
        }
        let slot = &mut self.incoming[(sequence as usize) & NET_TIMINGS_MASK];
        if slot.sequence == sequence && slot.status != PacketSampleStatus::InvalidDelta {
            slot.status = PacketSampleStatus::InvalidDelta;
            self.total_invalid_delta += 1;
            self.recompute_averages();
        }
    }

    /// Record an outgoing `NetChan` packet (`CL_WritePacket` / `Netchan_Transmit`).
    #[allow(clippy::too_many_arguments)]
    pub fn record_outgoing(
        &mut self,
        sequence: u32,
        bytes: usize,
        now: f64,
        sent: bool,
        cmd_lerp: f32,
        lerp_msec: i16,
        cmdrate: f32,
    ) {
        if cmdrate.is_finite() && cmdrate > 0.0 {
            self.cmdrate = cmdrate;
        }
        self.lerp_msec = lerp_msec;
        self.latest_outgoing_seq = sequence;
        let byte_u32 = bytes.min(u32::MAX as usize) as u32;
        if sent {
            self.last_out_bytes = byte_u32;
            self.out_window.push_back((now, bytes));
        }
        self.prune_windows(now);

        let slot = &mut self.outgoing[(sequence as usize) & NET_TIMINGS_MASK];
        *slot = OutgoingTimingSample {
            sequence,
            sent_at: now,
            bytes: byte_u32,
            sent,
            cmd_lerp: if cmd_lerp.is_finite() { cmd_lerp } else { 0.0 },
            lerp_msec,
        };
    }

    fn recompute_averages(&mut self) {
        if !self.has_incoming {
            return;
        }
        let head = self.latest_incoming_seq;
        let start = head.saturating_sub(UPDATE_BACKUP - 1).max(1);
        let mut choke_count = 0u32;
        let mut loss_count = 0u32;
        let mut lat_sum_ms = 0.0f32;
        let mut lat_count = 0u32;
        let lat_start = head.saturating_sub(NUM_LATENCY_SAMPLES - 1).max(1);

        for seq in start..=head {
            let s = self.incoming[(seq as usize) & NET_TIMINGS_MASK];
            if s.sequence != seq {
                continue;
            }
            if s.choked {
                choke_count += 1;
            }
            match s.status {
                PacketSampleStatus::Dropped => loss_count += 1,
                PacketSampleStatus::Valid if seq >= lat_start && s.latency_sec > 0.0 => {
                    lat_sum_ms += s.latency_sec * 1000.0;
                    lat_count += 1;
                }
                _ => {}
            }
        }

        if lat_count > 0 {
            self.ping_ms = lat_sum_ms / (lat_count as f32);
        }

        // `NetGraph_GetFrameData` (`cl_netgraph.c:239-244`)
        let raw_loss = 100.0 * (loss_count as f32) / (UPDATE_BACKUP as f32);
        self.loss_percent = PACKETLOSS_AVG_FRAC * self.loss_percent + (1.0 - PACKETLOSS_AVG_FRAC) * raw_loss;

        let raw_choke = 100.0 * (choke_count as f32) / (UPDATE_BACKUP as f32);
        self.choke_percent =
            PACKETCHOKE_AVG_FRAC * self.choke_percent + (1.0 - PACKETCHOKE_AVG_FRAC) * raw_choke;
    }

    pub fn latest_incoming_seq(&self) -> u32 {
        self.latest_incoming_seq
    }

    pub fn latest_outgoing_seq(&self) -> u32 {
        self.latest_outgoing_seq
    }

    pub fn incoming_sample(&self, sequence: u32) -> IncomingTimingSample {
        let s = self.incoming[(sequence as usize) & NET_TIMINGS_MASK];
        if s.sequence == sequence {
            s
        } else {
            IncomingTimingSample::default()
        }
    }

    pub fn outgoing_sample(&self, sequence: u32) -> OutgoingTimingSample {
        let s = self.outgoing[(sequence as usize) & NET_TIMINGS_MASK];
        if s.sequence == sequence {
            s
        } else {
            OutgoingTimingSample::default()
        }
    }

    pub fn last_in_bytes(&self) -> u32 {
        self.last_in_bytes
    }

    pub fn last_out_bytes(&self) -> u32 {
        self.last_out_bytes
    }

    pub fn in_bytes_per_sec(&self) -> f32 {
        self.in_window.iter().map(|(_, b)| *b as f32).sum()
    }

    pub fn in_kbs(&self) -> f32 {
        self.in_bytes_per_sec() / 1024.0
    }

    pub fn in_packets_per_sec(&self) -> f32 {
        self.in_window.len() as f32
    }

    pub fn out_bytes_per_sec(&self) -> f32 {
        self.out_window.iter().map(|(_, b)| *b as f32).sum()
    }

    pub fn out_kbs(&self) -> f32 {
        self.out_bytes_per_sec() / 1024.0
    }

    pub fn out_packets_per_sec(&self) -> f32 {
        self.out_window.len() as f32
    }

    pub fn ping_ms(&self) -> f32 {
        self.ping_ms
    }

    pub fn loss_percent(&self) -> f32 {
        self.loss_percent
    }

    pub fn choke_percent(&self) -> f32 {
        self.choke_percent
    }

    pub fn lerp_msec(&self) -> i16 {
        self.lerp_msec
    }

    pub fn updaterate(&self) -> f32 {
        self.updaterate
    }

    pub fn cmdrate(&self) -> f32 {
        self.cmdrate
    }

    pub fn max_msg_bytes(&self) -> u32 {
        self.max_msg_bytes
    }

    pub fn total_dropped(&self) -> u64 {
        self.total_dropped
    }

    pub fn total_choked(&self) -> u64 {
        self.total_choked
    }

    pub fn total_invalid_delta(&self) -> u64 {
        self.total_invalid_delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_records_drops_choke_invalid_delta_latency_and_rolling_rates() {
        let mut ng = NetGraphTelemetry::new();
        ng.record_outgoing(10, 80, 1.000, true, 0.5, 50, 100.0);
        ng.record_outgoing(11, 96, 1.010, true, 1.2, 50, 100.0);
        assert_eq!(ng.last_out_bytes(), 96);
        assert_eq!(ng.lerp_msec(), 50);
        assert_eq!(ng.out_bytes_per_sec(), 176.0);
        assert_eq!(ng.out_packets_per_sec(), 2.0);

        // First packet seq 1 acks outgoing 10 at 1.045 -> 45 ms RTT.
        ng.record_incoming(1, 0, 10, 200, 1.045, 60.0);
        assert!((ng.ping_ms() - 45.0).abs() < 0.1);
        assert_eq!(ng.last_in_bytes(), 200);
        assert_eq!(ng.total_dropped(), 0);

        // Jump from seq 1 to seq 4 -> seq 2 and 3 dropped!
        ng.record_incoming(4, 1, 11, 300, 1.060, 60.0);
        assert_eq!(ng.total_dropped(), 2);
        assert_eq!(ng.incoming_sample(2).status, PacketSampleStatus::Dropped);
        assert_eq!(ng.incoming_sample(3).status, PacketSampleStatus::Dropped);
        assert!(ng.loss_percent() > 0.0);

        ng.mark_choked(4);
        assert_eq!(ng.total_choked(), 1);
        assert!(ng.incoming_sample(4).choked);
        assert!(ng.choke_percent() > 0.0);

        ng.mark_invalid_delta(4);
        assert_eq!(ng.total_invalid_delta(), 1);
        assert_eq!(ng.incoming_sample(4).status, PacketSampleStatus::InvalidDelta);

        // Pruning past 1.0 second removes old samples from rolling 1s window.
        ng.prune_windows(2.100);
        assert_eq!(ng.in_bytes_per_sec(), 0.0);
        assert_eq!(ng.in_packets_per_sec(), 0.0);
        assert_eq!(ng.out_bytes_per_sec(), 0.0);
        assert_eq!(ng.out_packets_per_sec(), 0.0);
    }

    #[test]
    fn ring_wraps_cleanly_at_net_timings_256() {
        let mut ng = NetGraphTelemetry::new();
        for seq in 1..=300u32 {
            ng.record_incoming(seq, seq - 1, 0, 128, f64::from(seq) * 0.01, 60.0);
        }
        assert_eq!(ng.latest_incoming_seq(), 300);
        // Slot 44 (300 & 255) now holds seq 300, not seq 44.
        assert_eq!(ng.incoming_sample(300).sequence, 300);
        assert_eq!(ng.incoming_sample(44).status, PacketSampleStatus::Empty);
    }
}
