//! Per-client frames: when each active client is sent an update, what it contains and which
//! earlier update it is delta-encoded against.

use std::collections::BTreeMap;

use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::delta::{DeltaRecord, GoldSrcEntityState};
use cs_proto48::messages::{
    entity_schema_name, wire_entity_type, write_svc_choke, write_svc_clientdata,
    write_svc_packetentities_delta, write_svc_packetentities_full, write_svc_time,
};
use cs_transport::{MAX_UDP_PACKET, PacketTransport};

use crate::client::ClientState;
use crate::edict::{EdictId, EntVars};
use crate::game::{GameDll, entity_state_from_vars};
use crate::server::{Server, ServerConfig};

/// Updates remembered per client as delta bases.
pub const UPDATE_BACKUP: usize = 64;
/// Limit on the unreliable part of a client's datagram.
pub const MAX_DATAGRAM: usize = 4000;
/// Entities one update carries at most: `MAX_PACKET_ENTITIES` (`rehlds/common/qlimits.h`, read
/// 2026-11-21).
pub const MAX_PACKET_ENTITIES: usize = 256;

/// A frame this many sequences older than the datagram being built is no longer a delta base.
const MAX_DELTA_AGE: u32 = UPDATE_BACKUP as u32 - 1;
/// `cl_updaterate` of a client whose userinfo has no usable value.
const DEFAULT_UPDATE_RATE: f64 = 20.0;
/// `rate` of a client whose userinfo has no usable value.
const DEFAULT_RATE: f64 = 30_000.0;
/// `EF_NODRAW` in `EntVars::effects`.
const EF_NODRAW: u32 = 128;

/// One update sent to a client, kept as the base of later deltas.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientFrame {
    /// Netchan sequence of the datagram that carried the update.
    pub seq: u32,
    /// Entities sent, as `(wire entity type, record)` by entity number.
    pub entities: BTreeMap<u16, (u8, DeltaRecord)>,
    pub clientdata: DeltaRecord,
}

/// The last [`UPDATE_BACKUP`] updates sent to one client, by netchan sequence.
#[derive(Debug, Clone)]
pub struct FrameRing {
    frames: Vec<Option<ClientFrame>>,
}

impl Default for FrameRing {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameRing {
    pub fn new() -> Self {
        Self {
            frames: vec![None; UPDATE_BACKUP],
        }
    }

    /// Keeps `f`, replacing the frame stored [`UPDATE_BACKUP`] sequences before it.
    pub fn store(&mut self, f: ClientFrame) {
        let slot = f.seq as usize % UPDATE_BACKUP;
        self.frames[slot] = Some(f);
    }

    /// Frames stored, at most [`UPDATE_BACKUP`].
    pub fn stored(&self) -> usize {
        self.frames.iter().filter(|f| f.is_some()).count()
    }

    /// The frame the client names with `delta_seq` (the low byte of its sequence), when it is
    /// stored and fewer than 63 sequences older than `current_seq`.
    pub fn delta_base(&self, delta_seq: u8, current_seq: u32) -> Option<&ClientFrame> {
        // `UPDATE_BACKUP` divides 256, so the low byte alone picks the slot.
        let frame = self.frames[usize::from(delta_seq) % UPDATE_BACKUP].as_ref()?;
        let recent = frame.seq <= current_seq && current_seq - frame.seq < MAX_DELTA_AGE;
        (frame.seq & 0xFF == u32::from(delta_seq) && recent).then_some(frame)
    }
}

/// Seconds between updates: `1 / cl_updaterate`, the rate defaulting to 20 and clamped to
/// `min_update_rate..=max_update_rate`.
pub fn update_interval(cl_updaterate: Option<&str>, cfg: &ServerConfig) -> f64 {
    let rate = parse_number(cl_updaterate).unwrap_or(DEFAULT_UPDATE_RATE);
    let min = f64::from(cfg.min_update_rate).max(1.0);
    1.0 / rate.max(min).min(f64::from(cfg.max_update_rate).max(min))
}

/// Bytes per second a client may be sent: `rate`, defaulting to 30000 and clamped to
/// `min_rate..=max_rate`.
pub fn clamp_rate(rate: Option<&str>, cfg: &ServerConfig) -> u32 {
    let rate = parse_number(rate).unwrap_or(DEFAULT_RATE);
    let min = f64::from(cfg.min_rate);
    rate.max(min).min(f64::from(cfg.max_rate).max(min)) as u32
}

fn parse_number(value: Option<&str>) -> Option<f64> {
    value?.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// When the update after one due at `due` and sent at `now` is due: one `interval` after
/// `due`, or after `now` when the client fell a whole interval behind, so it never bursts.
fn next_due(due: f64, interval: f64, now: f64) -> f64 {
    let next = due + interval;
    if next <= now { now + interval } else { next }
}

/// When a client may be sent its next datagram, after one of `bytes` went to it at `now` with
/// `rate` bytes per second: the datagram's transmission time added to the later of
/// `clear_time` and `now`. Every datagram waits for the clear time, so the debt never exceeds
/// one datagram's worth.
pub(crate) fn advance_clear_time(clear_time: f64, now: f64, bytes: usize, rate: u32) -> f64 {
    clear_time.max(now) + bytes as f64 / f64::from(rate.max(1))
}

impl<G: GameDll> Server<G> {
    /// Sends each connected client its datagram for this frame, if any. No datagram goes before
    /// the client's clear time, and every one moves it (see [`advance_clear_time`]). An
    /// `Active` client is sent only its due updates, which carry its pending reliable data; a
    /// due update the rate holds back is choked: counted, and announced with `svc_choke` in the
    /// next update sent. A client still signing on is sent one datagram per frame, so that the
    /// packets it acknowledges let a lost reliable message be resent.
    pub(crate) fn send_client_frames(&mut self, net: &mut dyn PacketTransport) {
        for slot in 0..self.cfg.max_clients {
            let index = usize::from(slot);
            let client = &self.clients[index];
            if client.is_free() {
                continue;
            }
            let can_send = self.time >= client.clear_time;
            let packet = if client.state != ClientState::Active {
                can_send.then(|| self.build_signon_packet(slot))
            } else if self.time < client.next_update_at {
                None
            } else if can_send {
                self.build_update(slot)
            } else {
                self.choke_update(slot);
                None
            };
            let rate = clamp_rate(self.views[index].userinfo.get("rate"), &self.cfg);
            let client = &mut self.clients[index];
            let (Some(packet), Some(addr)) = (packet, client.addr) else {
                continue;
            };
            client.clear_time =
                advance_clear_time(client.clear_time, self.time, packet.len(), rate);
            let seq = client.chan.outgoing_sequence.wrapping_sub(1);
            client.pings.record_sent(seq, self.time);
            self.net_stats.send(net, addr, &packet);
        }
    }

    /// Skips the due update of `slot`: its due time advances as if it had been sent.
    fn choke_update(&mut self, slot: u8) {
        let index = usize::from(slot);
        let interval = update_interval(self.views[index].userinfo.get("cl_updaterate"), &self.cfg);
        let client = &mut self.clients[index];
        client.next_update_at = next_due(client.next_update_at, interval, self.time);
        client.choke_count = client.choke_count.saturating_add(1);
    }

    /// The datagram of a client still signing on: its pending reliable data, if any. Unreliable
    /// messages queued for it are dropped.
    fn build_signon_packet(&mut self, slot: u8) -> Vec<u8> {
        let client = &mut self.clients[usize::from(slot)];
        client.unreliable.clear();
        client.chan.build_packet(&[])
    }

    /// The due update of `slot`: one `svc_choke` per update choked since the last one sent,
    /// `svc_time`, `svc_clientdata` and the entity block, delta-encoded against the frame the
    /// client acknowledged when there is one, followed by the queued unreliable messages. The
    /// entity block counts at most [`MAX_PACKET_ENTITIES`] entities. When the block does not
    /// fit the datagram the whole unreliable part is dropped (nothing is stored as a delta
    /// base, the client gets the reliable data alone), as HLDS does: a `SV_SendClientDatagram`
    /// (`rehlds/engine/sv_main.cpp`, read 2026-11-21) whose message overflowed is cleared
    /// (`WARNING: msg overflowed`) and its netchan sends the reliable data alone; and
    /// `Netchan_Transmit` (`net_chan.cpp`, read 2026-11-21) leaves the unreliable payload out
    /// when it does not fit next to the reliable one (`Unreliable would overfow, ignoring`).
    /// Queued unreliable messages that do not fit next to the block are dropped alone (the
    /// `REHLDS_FIXES` rule). `None` when the game dropped the client meanwhile.
    fn build_update(&mut self, slot: u8) -> Option<Vec<u8>> {
        let index = usize::from(slot);
        let interval = update_interval(self.views[index].userinfo.get("cl_updaterate"), &self.cfg);
        let client = &mut self.clients[index];
        client.next_update_at = next_due(client.next_update_at, interval, self.time);

        let client_data = self.call_game(|game, eng| game.update_client_data(eng, slot));
        let entities = self.visible_entities(slot);
        self.drain_outbox();
        if self.clients[index].state != ClientState::Active {
            return None;
        }

        let max_clients = self.cfg.max_clients;
        let client = &mut self.clients[index];
        let seq = client.chan.outgoing_sequence;
        let base = client
            .delta_seq
            .and_then(|delta_seq| client.frames.delta_base(delta_seq, seq));

        let mut payload = MsgWriter::new();
        for _ in 0..std::mem::take(&mut client.choke_count).min(255) {
            write_svc_choke(&mut payload);
        }
        write_svc_time(&mut payload, self.time as f32);
        let clientdata = match self.deltas.get("clientdata_t") {
            Some(schema) => write_svc_clientdata(
                &mut payload,
                schema,
                base.map(|f| (low_byte(f.seq), &f.clientdata)),
                &client_data,
                self.time,
            ),
            None => DeltaRecord::default(),
        };
        let mut block = MsgWriter::new();
        match base {
            Some(from) => write_svc_packetentities_delta(
                &mut block,
                &self.deltas,
                max_clients,
                low_byte(from.seq),
                &self.baseline_records,
                &from.entities,
                &entities,
                self.time,
            ),
            None => write_svc_packetentities_full(
                &mut block,
                &self.deltas,
                max_clients,
                &self.baseline_records,
                &entities,
                self.time,
            ),
        }

        let reliable_len = client.chan.next_packet_reliable_len();
        let budget = MAX_DATAGRAM.min(MAX_UDP_PACKET.saturating_sub(reliable_len));
        let mut payload = payload.into_bytes();
        let queued = std::mem::take(&mut client.unreliable);
        let with_entities = payload.len() + block.len() <= budget;
        if with_entities {
            payload.extend_from_slice(block.as_slice());
            if payload.len() + queued.len() <= budget {
                payload.extend_from_slice(&queued);
            } else if !queued.is_empty() {
                self.net_stats.unreliable_dropped += 1;
            }
        } else {
            self.net_stats.entity_blocks_dropped += 1;
            if !queued.is_empty() {
                self.net_stats.unreliable_dropped += 1;
            }
            payload.clear();
        }

        if with_entities {
            let entities = entities
                .iter()
                .filter_map(|state| {
                    let wire_type = wire_entity_type(state.entity_type);
                    let schema = self.deltas.get(entity_schema_name(
                        state.number,
                        wire_type,
                        max_clients,
                    ))?;
                    Some((state.number, (wire_type, state.to_delta(schema))))
                })
                .collect();
            client.frames.store(ClientFrame {
                seq,
                entities,
                clientdata,
            });
        }
        Some(client.chan.build_packet(&payload))
    }

    /// Whether entity `id` is potentially visible in `pvs` (`None`: the viewer sees all): by the
    /// leaves it was linked with, or, for an entity that was never linked, by the leaves its
    /// box `origin + mins..origin + maxs` touches.
    fn entity_in_pvs(&self, pvs: Option<&[bool]>, id: EdictId, v: &EntVars) -> bool {
        let (Some(bsp), Some(pvs)) = (self.map.bsp(), pvs) else {
            return true;
        };
        self.world.leaves_visible(bsp, pvs, id).unwrap_or_else(|| {
            crate::world::find_touched_leafs(bsp, v.origin + v.mins, v.origin + v.maxs)
                .visible(bsp, pvs)
        })
    }

    /// Entity states `slot` is sent, at most [`MAX_PACKET_ENTITIES`]: as in HLDS
    /// (`SV_WriteEntitiesToClient`, `rehlds/engine/sv_main.cpp`, read 2026-11-21) the client
    /// slots are always asked, the other edicts in index order until the list is full, and
    /// the rest are not offered to the game. Candidates are the viewer's own player edict, and every
    /// other live edict with a model and without `EF_NODRAW` that is potentially visible from
    /// the viewer's eye; each goes out when the game's `add_to_full_pack` accepts it, with the
    /// game's edits. The caller drains the outbox.
    fn visible_entities(&mut self, slot: u8) -> Vec<GoldSrcEntityState> {
        let viewer = self.edicts.client(slot);
        let eye = self
            .edicts
            .get(viewer)
            .map(|e| e.vars.origin + e.vars.view_ofs)
            .unwrap_or_default();
        let pvs = self.map.viewer_pvs(eye);
        let candidates: Vec<GoldSrcEntityState> = self
            .edicts
            .iter_live()
            .filter(|(id, edict)| {
                let v = &edict.vars;
                *id == viewer
                    || (v.modelindex != 0
                        && v.effects & EF_NODRAW == 0
                        && self.entity_in_pvs(pvs.as_deref(), *id, v))
            })
            .map(|(id, edict)| entity_state_from_vars(id.index, &edict.vars))
            .collect();
        // One copy of the sign-on states for all the calls below: nothing changes them here.
        self.sync_views();
        let max_clients = u16::from(self.cfg.max_clients);
        let mut accepted = Vec::new();
        for mut state in candidates {
            let ent = state.number;
            if ent > max_clients && accepted.len() >= MAX_PACKET_ENTITIES {
                tracing::debug!(slot, "too many entities in the visible packet list");
                break;
            }
            if self.call_game_synced(|game, eng| game.add_to_full_pack(eng, slot, ent, &mut state))
            {
                accepted.push(state);
            }
        }
        accepted
    }
}

/// The low byte of a netchan sequence, as `clc_delta` and delta messages carry it.
fn low_byte(seq: u32) -> u8 {
    (seq & 0xFF) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(seq: u32) -> ClientFrame {
        ClientFrame {
            seq,
            entities: BTreeMap::new(),
            clientdata: DeltaRecord::default(),
        }
    }

    #[test]
    fn delta_base_requires_matching_recent_frame() {
        let mut ring = FrameRing::new();
        ring.store(frame(100));
        assert_eq!(ring.delta_base(100, 101).map(|f| f.seq), Some(100));
        assert!(ring.delta_base(100, 163).is_none());
        assert!(ring.delta_base(99, 101).is_none());
        // 62 sequences old is still recent enough.
        assert_eq!(ring.delta_base(100, 162).map(|f| f.seq), Some(100));
        // A frame stored 64 sequences later takes the place of the old one.
        ring.store(frame(164));
        assert!(ring.delta_base(100, 165).is_none());
        assert_eq!(ring.delta_base(164, 165).map(|f| f.seq), Some(164));
        // Low bytes wrap: seq 300 is asked for as 44.
        ring.store(frame(300));
        assert_eq!(ring.delta_base(44, 301).map(|f| f.seq), Some(300));
        // A frame from the future of the current sequence is never a base.
        assert!(ring.delta_base(44, 299).is_none());
        assert!(FrameRing::new().delta_base(0, 0).is_none());
    }

    #[test]
    fn update_interval_clamps() {
        let cfg = ServerConfig::default();
        let close = |a: f64, b: f64| (a - b).abs() < 1e-12;
        assert!(close(update_interval(Some("20"), &cfg), 0.05));
        assert!(close(update_interval(Some("1000"), &cfg), 1.0 / 102.0));
        assert!(close(update_interval(None, &cfg), 0.05));
        assert!(close(update_interval(Some("abc"), &cfg), 0.05));
        assert!(close(update_interval(Some("1"), &cfg), 0.1));
    }

    #[test]
    fn next_due_advances_by_interval_and_never_bursts() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-12;
        // Sent a little late: the next update keeps the schedule.
        assert!(close(next_due(1.0, 0.1, 1.01), 1.1));
        // More than one interval behind: the schedule restarts from now.
        assert!(close(next_due(0.0, 0.1, 2.0), 2.1));
        assert!(close(next_due(1.0, 0.1, 1.1), 1.2));
    }

    #[test]
    fn clear_time_advances_by_transmission_time() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        // Idle client: counted from now.
        assert!(close(advance_clear_time(0.0, 5.0, 500, 1000), 5.5));
        // Still busy: counted from the clear time.
        assert!(close(advance_clear_time(5.5, 5.2, 200, 1000), 5.7));
        // No cap: a large datagram owes its whole transmission time.
        assert!(close(advance_clear_time(0.0, 5.0, 4010, 1000), 9.01));
    }

    #[test]
    fn clear_time_handles_hostile_inputs() {
        let cfg = ServerConfig::default();
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        let min = clamp_rate(Some("0"), &cfg);
        let max = clamp_rate(Some("1e30"), &cfg);
        assert_eq!((min, max), (cfg.min_rate, cfg.max_rate));
        assert!(close(advance_clear_time(0.0, 1.0, 1000, min), 2.0));
        assert!(close(advance_clear_time(0.0, 1.0, 4000, max), 1.04));
        // A zero-byte datagram costs nothing.
        assert!(close(advance_clear_time(0.0, 1.0, 0, min), 1.0));
        assert!(close(advance_clear_time(1.3, 1.0, 0, min), 1.3));
        // `now` earlier than the clear time: counted from the clear time.
        assert!(close(advance_clear_time(1.5, 1.0, 100, min), 1.6));
        assert!(close(advance_clear_time(3.0, 1.0, 100, min), 3.1));
        // A zero rate (not reachable through clamp_rate) does not divide by zero.
        assert!(advance_clear_time(0.0, 1.0, 10, 0).is_finite());
    }

    #[test]
    fn clamp_rate_defaults_and_clamps() {
        let cfg = ServerConfig::default();
        assert_eq!(clamp_rate(None, &cfg), 30_000);
        assert_eq!(clamp_rate(Some("abc"), &cfg), 30_000);
        assert_eq!(clamp_rate(Some("25000"), &cfg), 25_000);
        assert_eq!(clamp_rate(Some("1"), &cfg), cfg.min_rate);
        assert_eq!(clamp_rate(Some("-5"), &cfg), cfg.min_rate);
        assert_eq!(clamp_rate(Some("9999999"), &cfg), cfg.max_rate);
    }
}
