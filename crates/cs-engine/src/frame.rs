//! Per-client frames: when each active client is sent an update, what it contains and which
//! earlier update it is delta-encoded against.

use std::collections::BTreeMap;

use cs_proto48::bitbuf::MsgWriter;
use std::sync::Arc;

use cs_proto48::delta::{
    DeltaMarks, DeltaRecord, DeltaRegistry, GoldSrcClientData, GoldSrcEntityState,
    GoldSrcWeaponData,
};
use cs_proto48::event::{WireEvent, write_svc_event};
use cs_proto48::messages::{
    EntityEncodeFn, PreparedEntityOp, WeaponDelta, WeaponRecords, entity_schema_name,
    prepare_packetentities_delta_ops, prepare_packetentities_full_ops, weapon_records,
    wire_entity_type, write_svc_addangle, write_svc_choke, write_svc_clientdata_with_weapons,
    write_svc_packetentities_delta_encoded, write_svc_packetentities_delta_prepared,
    write_svc_packetentities_full_encoded, write_svc_packetentities_full_prepared, write_svc_pings,
    write_svc_setangle, write_svc_time,
};
use cs_transport::{MAX_UDP_PACKET, PacketTransport};

use crate::client::{Client, ClientState};
use crate::edict::{EdictId, EntVars};
use crate::game::{FullPackCtx, GameDll, entity_state_from_vars};
use crate::lifecycle::PINGS_INTERVAL_SECS;
use crate::world::LinkSnapshot;
use crate::server::{Server, ServerConfig};
use crate::unlag::userinfo_lw;
use crate::world::consts::EF_NODRAW;

/// `IN_SCORE` (`in_buttons.h` / ReHLDS `SV_ShouldUpdatePing`, `sv_main.cpp`, read 2026-11-25):
/// client scoreboard button (`+showscores` / `TAB`).
pub(crate) const IN_SCORE: u16 = 1 << 15;

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

/// One update sent to a client, kept as the base of later deltas.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientFrame {
    /// Netchan sequence of the datagram that carried the update.
    pub seq: u32,
    /// Entities sent, as `(wire entity type, record)` by entity number. The record of an
    /// entity whose state did not change from one update to the next is the same `Arc`, so a
    /// ring of 64 updates of static entities holds one record each.
    pub entities: BTreeMap<u16, (u8, Arc<DeltaRecord>)>,
    pub clientdata: DeltaRecord,
    /// The `weapon_data_t` list sent with `clientdata` (`client_frame_t::weapondata`), by weapon
    /// index; an index that is missing is the zero record. Empty for a client without `cl_lw`.
    /// (The original leaves the list of a frame it sent no weapons in as it was 64 frames
    /// earlier, so a later delta against that frame would use stale data; here it is empty.)
    pub weapondata: WeaponRecords,
    /// Real time the update was built (`client_frame_t::senttime`).
    pub sent_at: f64,
    /// Round trip of this update in seconds, set when the client acknowledges it
    /// (`client_frame_t::ping_time`); negative while it is not known.
    pub ping_time: f64,
    /// The players among the entities sent, as the client was given them: what lag
    /// compensation rewinds to (`client_frame_t::entities` of the original keeps whole entity
    /// states; only what `SV_FlagNonInterpolatedPlayers` and `SV_ApplyAdjustedPositions` read
    /// is kept here).
    pub players: Vec<FramePlayer>,
}

/// A player as one update showed it to a client.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FramePlayer {
    /// Entity number (slot + 1).
    pub number: u16,
    pub origin: glam::Vec3,
    /// `EntVars::effects` as sent.
    pub effects: u32,
    /// `EntVars::health` of the player when the update was built (`entity_state_t::health`,
    /// which the original's game fills from it; an assumption, not checked against the game).
    pub health: f32,
}

/// The state of an entity as last encoded for one client, kept to skip the encoding (and share
/// the record) when the next update shows the same state.
#[derive(Debug, Clone)]
pub(crate) struct EncodedEntity {
    state: GoldSrcEntityState,
    wire_type: u8,
    record: Arc<DeltaRecord>,
}

/// Minimum due active clients in one server frame before Stage-2 delta compression and `NetChan`
/// framing fans out across scoped worker threads.
#[cfg(not(target_arch = "wasm32"))]
const PARALLEL_SNAPSHOT_MIN_CLIENTS: usize = 4;

/// Precomputed Stage-1 entity delta operations when the game DLL provides conditional encoders
/// (`GameDll::delta_encode` requires `&mut self` and must execute sequentially in Stage 1).
enum PreparedEntityPlan {
    Full {
        ops: Vec<PreparedEntityOp>,
    },
    Delta {
        delta_seq: u8,
        expected_count: u16,
        ops: Vec<PreparedEntityOp>,
    },
}

/// Self-contained Stage-1 snapshot capture for one due active client slot.
///
/// Stage 2 consumes `ClientFrameJob` alongside its disjoint `&mut Client` without touching
/// `Server`'s game DLL, edict pool, or outbox, so multi-client delta compression and `NetChan`
/// framing can run safely in parallel via `std::thread::scope`.
struct ClientFrameJob {
    rate: u32,
    header_payload: MsgWriter,
    client_data: GoldSrcClientData,
    weapon_list: Option<Box<[GoldSrcWeaponData; 64]>>,
    entities: Vec<GoldSrcEntityState>,
    players: Vec<FramePlayer>,
    pings_block: MsgWriter,
    wire_events: Vec<WireEvent>,
    queued_unreliable: Vec<u8>,
    prepared_entities: Option<PreparedEntityPlan>,
}

/// Stage-2 encoded output for one active client slot, committed in deterministic `0..max_clients`
/// order during Stage 3.
struct ClientFrameOutcome {
    packet: Vec<u8>,
    entity_blocks_dropped: u64,
    unreliable_dropped: u64,
}

enum SlotWork {
    None,
    Signon {
        rate: u32,
        packet: Vec<u8>,
    },
    Active {
        job: Box<ClientFrameJob>,
        outcome: Option<ClientFrameOutcome>,
    },
}

/// Stage-2 pure per-client snapshot compression and `NetChan` framing. Runs either inline or
/// concurrently across disjoint `(&mut Client, &mut ClientFrameJob)` pairs inside
/// `std::thread::scope`.
fn encode_client_frame_job(
    client: &mut Client,
    job: &mut ClientFrameJob,
    deltas: &DeltaRegistry,
    baseline_records: &BTreeMap<u16, (u8, DeltaRecord)>,
    max_clients: u8,
    sv_time: f64,
    real_time: f64,
) -> ClientFrameOutcome {
    let mut payload = std::mem::replace(&mut job.header_payload, MsgWriter::new());
    let pings_block = std::mem::replace(&mut job.pings_block, MsgWriter::new());
    let players = std::mem::take(&mut job.players);
    let queued = std::mem::take(&mut job.queued_unreliable);

    let seq = client.chan.outgoing_sequence;
    let base = client
        .delta_seq
        .and_then(|delta_seq| client.frames.delta_base(delta_seq, seq));

    let mut weapondata = WeaponRecords::new();
    let clientdata = match deltas.get("clientdata_t") {
        Some(schema) => {
            let weapon_schema = deltas.get("weapon_data_t");
            if let (Some(ws), Some(list)) = (weapon_schema, job.weapon_list.as_deref()) {
                weapondata = weapon_records(ws, list);
            }
            let no_weapons = WeaponRecords::new();
            let weapons = weapon_schema
                .zip(job.weapon_list.as_ref())
                .map(|(schema, _)| WeaponDelta {
                    schema,
                    from: base.map_or(&no_weapons, |f| &f.weapondata),
                    to: &weapondata,
                });
            write_svc_clientdata_with_weapons(
                &mut payload,
                schema,
                base.map(|f| (low_byte(f.seq), &f.clientdata)),
                &job.client_data,
                weapons,
                sv_time,
            )
        }
        None => DeltaRecord::default(),
    };

    let mut block = MsgWriter::new();
    match &job.prepared_entities {
        Some(PreparedEntityPlan::Delta {
            delta_seq,
            expected_count,
            ops,
        }) => write_svc_packetentities_delta_prepared(
            &mut block,
            deltas,
            max_clients,
            *delta_seq,
            *expected_count,
            &job.entities,
            ops,
            sv_time,
        ),
        Some(PreparedEntityPlan::Full { ops }) => write_svc_packetentities_full_prepared(
            &mut block,
            deltas,
            max_clients,
            &job.entities,
            ops,
            sv_time,
        ),
        None => match base {
            Some(from) => write_svc_packetentities_delta_encoded(
                &mut block,
                deltas,
                max_clients,
                low_byte(from.seq),
                baseline_records,
                &from.entities,
                &job.entities,
                sv_time,
                None,
            ),
            None => write_svc_packetentities_full_encoded(
                &mut block,
                deltas,
                max_clients,
                baseline_records,
                &job.entities,
                sv_time,
                None,
            ),
        },
    }

    let mut event_block = MsgWriter::new();
    if let Some(schema) = deltas.get("event_t") {
        write_svc_event(&mut event_block, schema, &job.wire_events, sv_time);
    }

    let reliable_len = client.chan.next_packet_reliable_len();
    let budget = MAX_DATAGRAM.min(MAX_UDP_PACKET.saturating_sub(reliable_len));
    let mut payload = payload.into_bytes();
    let with_entities =
        payload.len() + block.len() + event_block.len() + pings_block.len() <= budget;
    let mut entity_blocks_dropped = 0u64;
    let mut unreliable_dropped = 0u64;
    if with_entities {
        payload.extend_from_slice(block.as_slice());
        payload.extend_from_slice(event_block.as_slice());
        payload.extend_from_slice(pings_block.as_slice());
        if payload.len() + queued.len() <= budget {
            payload.extend_from_slice(&queued);
        } else if !queued.is_empty() {
            unreliable_dropped += 1;
        }
    } else {
        entity_blocks_dropped += 1;
        if !queued.is_empty() {
            unreliable_dropped += 1;
        }
        payload.clear();
    }

    if with_entities {
        // An entity in the same state as in the update before keeps its record (and the
        // encoding is skipped).
        let mut encoded = BTreeMap::new();
        let mut stored = BTreeMap::new();
        for state in &job.entities {
            let known = client
                .last_encoded
                .remove(&state.number)
                .filter(|known| known.state == *state);
            let known = match known {
                Some(known) => known,
                None => {
                    let wire_type = wire_entity_type(state.entity_type);
                    let Some(schema) = deltas.get(entity_schema_name(
                        state.number,
                        wire_type,
                        max_clients,
                    )) else {
                        continue;
                    };
                    EncodedEntity {
                        state: state.clone(),
                        wire_type,
                        record: Arc::new(state.to_delta(schema)),
                    }
                }
            };
            stored.insert(
                state.number,
                (known.wire_type, Arc::clone(&known.record)),
            );
            encoded.insert(state.number, known);
        }
        client.last_encoded = encoded;
        client.frames.store(ClientFrame {
            seq,
            entities: stored,
            clientdata,
            weapondata,
            sent_at: real_time,
            ping_time: -1.0,
            players,
        });
    }

    let packet = client.chan.build_packet(&payload);
    ClientFrameOutcome {
        packet,
        entity_blocks_dropped,
        unreliable_dropped,
    }
}

/// Reusable scratch buffers held on [`crate::world::WorldLinks`] across server frames so
/// `send_client_frames` avoids per-client heap allocations for candidate lists and entity arrays.
pub(crate) struct FrameScratch {
    /// Pre-filtered live modelled edicts for this server frame: `(edict_id, is_nodraw)`.
    pub(crate) live_edicts: Vec<(EdictId, bool)>,
    /// Reusable per-client `(edict_id, in_pvs)` candidate buffer.
    pub(crate) pvs_candidates: Vec<(EdictId, bool)>,
    /// Reusable `LinkSnapshot` for `FullPackCtx::viewer_leaves`.
    pub(crate) viewer_leaves: LinkSnapshot,
    /// Per-client accepted `GoldSrcEntityState` scratch vectors (indexed by slot).
    pub(crate) slot_accepted: Vec<Vec<GoldSrcEntityState>>,
}

impl Default for FrameScratch {
    fn default() -> Self {
        Self {
            live_edicts: Vec::new(),
            pvs_candidates: Vec::new(),
            viewer_leaves: LinkSnapshot {
                leafnums: Vec::new(),
                headnode: -1,
                area: None,
            },
            slot_accepted: Vec::new(),
        }
    }
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

    /// The frame stored for exactly `seq`, if it is still kept.
    pub fn get(&self, seq: u32) -> Option<&ClientFrame> {
        self.frames[seq as usize % UPDATE_BACKUP]
            .as_ref()
            .filter(|f| f.seq == seq)
    }

    /// The ping `SV_ExecuteClientMessage` gives the frame the client just acknowledged (ReHLDS
    /// `rehlds/engine/sv_user.cpp`, read 2026-11-21): `now - sent_at - update_interval`, or 0
    /// when that is not positive or the client connected less than 2 s ago. Nothing for a frame
    /// that is not kept.
    pub fn note_ack(&mut self, seq: u32, now: f64, update_interval: f64, since_connect: f64) {
        let Some(frame) = self.frames[seq as usize % UPDATE_BACKUP]
            .as_mut()
            .filter(|f| f.seq == seq)
        else {
            return;
        };
        let ping = now - frame.sent_at - update_interval;
        frame.ping_time = if ping <= 0.0 || since_connect < 2.0 {
            0.0
        } else {
            ping
        };
    }

    /// Takes `seconds` off the ping of frame `seq` (never below 0), if it is kept and has one.
    pub fn trim_ping(&mut self, seq: u32, seconds: f64) {
        if let Some(frame) = self.frames[seq as usize % UPDATE_BACKUP]
            .as_mut()
            .filter(|f| f.seq == seq && f.ping_time >= 0.0)
        {
            frame.ping_time = (frame.ping_time - seconds).max(0.0);
        }
    }

    /// `SV_CalcClientTime` (ReHLDS `rehlds/engine/sv_user.cpp`, read 2026-11-21): the average
    /// ping of the frames up to the acknowledged `ack`, `samples` of them (clamped to 1..=16),
    /// that have one; 0 without any, and 0 when the pings of the latest 4 differ by more than
    /// 0.2 s.
    pub fn client_time(&self, ack: u32, samples: i32) -> f64 {
        let samples = samples.clamp(1, 16) as u32;
        let ping_of = |i: u32| {
            self.get(ack.wrapping_sub(i))
                .map(|f| f.ping_time)
                .filter(|&p| p > 0.0)
        };
        let known: Vec<f64> = (0..samples).filter_map(ping_of).collect();
        if known.is_empty() {
            return 0.0;
        }
        let average = known.iter().sum::<f64>() / known.len() as f64;
        let recent: Vec<f64> = (0..4).filter_map(ping_of).collect();
        let min = recent.iter().copied().fold(9999.0, f64::min);
        let max = recent.iter().copied().fold(-9999.0, f64::max);
        if max < min || (max - min).abs() <= 0.2 {
            average
        } else {
            0.0
        }
    }

    /// `(entity records, distinct allocations among them)` over the frames stored.
    pub(crate) fn record_counts(&self) -> (usize, usize) {
        let mut seen = std::collections::BTreeSet::new();
        let mut total = 0;
        for frame in self.frames.iter().flatten() {
            for (_, record) in frame.entities.values() {
                total += 1;
                seen.insert(Arc::as_ptr(record));
            }
        }
        (total, seen.len())
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

/// Resolves live server rate and updaterate bounds from `cvars` (`sv_minupdaterate`,
/// `sv_maxupdaterate`, `sv_minrate`, `sv_maxrate`, `sv_lan`, `sv_lan_rate`, per
/// `swds.dll:FUN_102081e0` and ReHLDS `SV_CheckUpdateRate` / `SV_CheckRate` in `sv_main.cpp`,
/// read 2026-11-25), falling back to `base_min_rate` / `base_max_rate` / `fallback` when unset
/// or `0`.
pub(crate) fn effective_rate_config(
    cvars: &cs_cvar::CVarRegistry,
    base_min_rate: u32,
    base_max_rate: u32,
    fallback: &ServerConfig,
) -> ServerConfig {
    let mut cfg = fallback.clone();
    if let Some(c) = cvars.get("sv_minupdaterate") {
        let v = c.as_f32();
        if v.is_finite() {
            cfg.min_update_rate = if v > 0.001 { (v as u32).max(1) } else { 1 };
        }
    }
    if let Some(c) = cvars.get("sv_maxupdaterate") {
        let v = c.as_f32();
        if v.is_finite() {
            let raw = if v > 0.001 { (v as u32).max(1) } else { 30 };
            cfg.max_update_rate = raw.max(cfg.min_update_rate);
        }
    }
    if let Some(c) = cvars.get("sv_minrate") {
        let v = c.as_f32();
        cfg.min_rate = if v.is_finite() && v > 0.0 {
            (v as u32).max(1000)
        } else {
            base_min_rate
        };
    }
    if let Some(c) = cvars.get("sv_maxrate") {
        let v = c.as_f32();
        cfg.max_rate = if v.is_finite() && v > 0.0 {
            (v as u32).max(cfg.min_rate)
        } else {
            base_max_rate.max(cfg.min_rate)
        };
    }
    if cvars.get_f32("sv_lan") > 0.0 {
        let lan_rate = cvars.get_f32("sv_lan_rate");
        if lan_rate.is_finite() && lan_rate > 0.0 {
            let r = (lan_rate as u32).max(1000);
            cfg.min_rate = r;
            cfg.max_rate = r;
        }
    }
    cfg
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
    /// Live rate/updaterate config resolved from `self.cvars`.
    pub(crate) fn live_rate_config(&self) -> ServerConfig {
        effective_rate_config(&self.cvars, self.base_min_rate, self.base_max_rate, &self.cfg)
    }

    /// Live update interval in seconds for `slot`, clamping `cl_updaterate` against live
    /// `sv_minupdaterate` / `sv_maxupdaterate` from `self.cvars`.
    pub(crate) fn client_update_interval(&self, slot: u8) -> f64 {
        let cfg = self.live_rate_config();
        update_interval(self.views[usize::from(slot)].userinfo.get("cl_updaterate"), &cfg)
    }

    /// Live rate cap in bytes/sec for `slot`, clamping `rate` against live `sv_minrate` /
    /// `sv_maxrate` (or `sv_lan_rate` under `sv_lan`) from `self.cvars`.
    pub(crate) fn client_rate(&self, slot: u8) -> u32 {
        let cfg = self.live_rate_config();
        clamp_rate(self.views[usize::from(slot)].userinfo.get("rate"), &cfg)
    }

    /// Sends each connected client its datagram for this frame, if any. No datagram goes before
    /// the client's clear time, and every one moves it (see [`advance_clear_time`]). An
    /// `Active` client is sent only its due updates, which carry its pending reliable data; a
    /// due update the rate holds back is choked: counted, and announced with `svc_choke` in the
    /// next update sent. A client still signing on is sent one datagram per frame, so that the
    /// packets it acknowledges let a lost reliable message be resent.
    /// Pre-filters dead, world, unmodelled, and non-client `EF_NODRAW` edicts once per server
    /// frame before iterating `0..max_clients`.
    fn prefilter_frame_edicts(&mut self) {
        let max_clients = u16::from(self.cfg.max_clients);
        let out = &mut self.world.frame_scratch.live_edicts;
        out.clear();
        for (id, edict) in self.edicts.iter_live() {
            let v = &edict.vars;
            if id.index == 0 || v.modelindex == 0 {
                continue;
            }
            let nodraw = (v.effects & EF_NODRAW) != 0;
            if !nodraw || id.index <= max_clients {
                out.push((id, nodraw));
            }
        }
    }

    pub(crate) fn send_client_frames(&mut self, net: &mut dyn PacketTransport) {
        self.send_client_frames_with_mode(net, true);
    }

    /// Two-stage snapshot pipeline:
    /// - **Stage 1 (sequential, `0..max_clients`):** pre-filters live edicts once, executes game
    ///   hooks (`UpdateClientData`, `GetWeaponData`, `SetupVisibility`, `AddToFullPack`, optional
    ///   `DeltaEncode`) and captures each due client's snapshot job.
    /// - **Stage 2 (parallel when `allow_parallel && due_active >= 4` on native targets):** runs
    ///   Protocol 48 delta bit-packing, `ClientFrame` storage, and `NetChan` packet framing
    ///   concurrently across disjoint `(&mut Client, &mut ClientFrameJob)` pairs.
    /// - **Stage 3 (sequential, `0..max_clients`):** recycles per-client scratch buffers,
    ///   accumulates `net_stats`, advances `clear_time`/ping trackers, and transmits packets in
    ///   deterministic slot order.
    pub(crate) fn send_client_frames_with_mode(
        &mut self,
        net: &mut dyn PacketTransport,
        allow_parallel: bool,
    ) {
        let max_clients = self.cfg.max_clients;
        let any_due_active = (0..usize::from(max_clients)).any(|index| {
            let client = &self.clients[index];
            !client.is_free()
                && !client.fake
                && client.state == ClientState::Active
                && self.real_time >= client.next_update_at
                && self.real_time >= client.clear_time
        });
        if any_due_active {
            self.prefilter_frame_edicts();
        }

        // Stage 1: sequential game/engine capture in exact `0..max_clients` order.
        let mut works = Vec::with_capacity(usize::from(max_clients));
        for slot in 0..max_clients {
            let index = usize::from(slot);
            let client = &self.clients[index];
            // `SV_SendClientMessages` skips `fakeclient` (`sv_main.cpp` 5293): no update is
            // built, no event queue is kept, nothing is sent.
            if client.is_free() || client.fake {
                works.push(SlotWork::None);
                continue;
            }
            let can_send = self.real_time >= client.clear_time;
            if client.state != ClientState::Active {
                // A client that is not in the game is sent no events, and keeps none.
                self.outbox.events[index].clear();
                if can_send {
                    let packet = self.build_signon_packet(slot);
                    let rate = self.client_rate(slot);
                    works.push(SlotWork::Signon { rate, packet });
                } else {
                    works.push(SlotWork::None);
                }
            } else if self.real_time < client.next_update_at {
                works.push(SlotWork::None);
            } else if can_send {
                match self.capture_client_frame_job(slot) {
                    Some(job) => works.push(SlotWork::Active {
                        job: Box::new(job),
                        outcome: None,
                    }),
                    None => works.push(SlotWork::None),
                }
            } else {
                self.choke_update(slot);
                works.push(SlotWork::None);
            }
        }

        // Stage 2: delta compression + `NetChan` framing across disjoint client slots.
        {
            let deltas = &self.deltas;
            let baseline_records = &self.baseline_records;
            let sv_time = self.sv_time;
            let real_time = self.real_time;
            #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
            let mut active_tasks: Vec<(
                &mut Client,
                &mut ClientFrameJob,
                &mut Option<ClientFrameOutcome>,
            )> = self
                .clients
                .iter_mut()
                .zip(works.iter_mut())
                .filter_map(|(client, work)| match work {
                    SlotWork::Active { job, outcome } => Some((client, job.as_mut(), outcome)),
                    _ => None,
                })
                .collect();

            #[cfg(not(target_arch = "wasm32"))]
            if allow_parallel && active_tasks.len() >= PARALLEL_SNAPSHOT_MIN_CLIENTS {
                let workers = std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(4)
                    .clamp(2, 8)
                    .min(active_tasks.len());
                let chunk_size = active_tasks.len().div_ceil(workers);
                std::thread::scope(|scope| {
                    let mut chunks = active_tasks.chunks_mut(chunk_size);
                    let first = chunks.next();
                    for chunk in chunks {
                        scope.spawn(move || {
                            for (client, job, outcome) in chunk {
                                **outcome = Some(encode_client_frame_job(
                                    client,
                                    job,
                                    deltas,
                                    baseline_records,
                                    max_clients,
                                    sv_time,
                                    real_time,
                                ));
                            }
                        });
                    }
                    if let Some(chunk) = first {
                        for (client, job, outcome) in chunk {
                            **outcome = Some(encode_client_frame_job(
                                client,
                                job,
                                deltas,
                                baseline_records,
                                max_clients,
                                sv_time,
                                real_time,
                            ));
                        }
                    }
                });
            } else {
                for (client, job, outcome) in active_tasks {
                    *outcome = Some(encode_client_frame_job(
                        client,
                        job,
                        deltas,
                        baseline_records,
                        max_clients,
                        sv_time,
                        real_time,
                    ));
                }
            }

            #[cfg(target_arch = "wasm32")]
            {
                let _ = allow_parallel;
                for (client, job, outcome) in active_tasks {
                    *outcome = Some(encode_client_frame_job(
                        client,
                        job,
                        deltas,
                        baseline_records,
                        max_clients,
                        sv_time,
                        real_time,
                    ));
                }
            }
        }

        // Stage 3: deterministic commit and UDP transmit in `slot 0..max_clients` order.
        for (index, work) in works.into_iter().enumerate() {
            let (rate, packet) = match work {
                SlotWork::None => continue,
                SlotWork::Signon { rate, packet } => (rate, packet),
                SlotWork::Active { job, outcome } => {
                    let rate = job.rate;
                    self.recycle_slot_entities(index, job.entities);
                    let Some(outcome) = outcome else {
                        continue;
                    };
                    self.net_stats.entity_blocks_dropped += outcome.entity_blocks_dropped;
                    self.net_stats.unreliable_dropped += outcome.unreliable_dropped;
                    (rate, outcome.packet)
                }
            };
            let client = &mut self.clients[index];
            let Some(addr) = client.addr else {
                continue;
            };
            client.clear_time =
                advance_clear_time(client.clear_time, self.real_time, packet.len(), rate);
            let seq = client.chan.outgoing_sequence.wrapping_sub(1);
            client.pings.record_sent(seq, self.real_time);
            self.net_stats.send(net, addr, &packet);
        }
    }

    /// Skips the due update of `slot`: its due time advances as if it had been sent.
    fn choke_update(&mut self, slot: u8) {
        let index = usize::from(slot);
        let interval = self.client_update_interval(slot);
        let client = &mut self.clients[index];
        client.next_update_at = next_due(client.next_update_at, interval, self.real_time);
        client.choke_count = client.choke_count.saturating_add(1);
    }

    /// The datagram of a client still signing on: its pending reliable data, if any. Unreliable
    /// messages queued for it are dropped.
    fn build_signon_packet(&mut self, slot: u8) -> Vec<u8> {
        let client = &mut self.clients[usize::from(slot)];
        client.unreliable.clear();
        client.chan.build_packet(&[])
    }

    /// Stage 1 capture for one due active client slot: runs game hooks (`UpdateClientData`,
    /// `GetWeaponData`, `SetupVisibility`, `AddToFullPack`, optional `DeltaEncode`), drains the
    /// outbox/event/unreliable queues, and returns a self-contained [`ClientFrameJob`] ready for
    /// Stage 2 encoding. Returns `None` if the game dropped the client meanwhile.
    fn capture_client_frame_job(&mut self, slot: u8) -> Option<ClientFrameJob> {
        let index = usize::from(slot);
        let interval = self.client_update_interval(slot);
        let client = &mut self.clients[index];
        client.next_update_at = next_due(client.next_update_at, interval, self.real_time);

        // `SV_SendClientDatagram`: `svc_time`, then `SV_WriteClientdataToMessage`: the choke
        // byte and the forced view first, and only then the game is asked for the client data.
        let mut header_payload = MsgWriter::new();
        write_svc_time(&mut header_payload, self.sv_time as f32);
        self.write_choke_and_fixangle(slot, &mut header_payload);
        let client_data = self.call_game(|game, eng| game.update_client_data(eng, slot));
        // The weapon list is asked for only when the client has `cl_lw`, and sent only if the
        // game filled it.
        let weapon_list = if self.client_has_lw(slot) {
            let mut list = Box::new([GoldSrcWeaponData::default(); 64]);
            self.call_game_synced(|game, eng| game.get_weapon_data(eng, slot, &mut list))
                .then_some(list)
        } else {
            None
        };
        let entities = self.visible_entities(slot);
        let players = self.frame_players(&entities);
        self.drain_outbox();
        if self.clients[index].state != ClientState::Active {
            self.recycle_slot_entities(index, entities);
            return None;
        }

        // `SV_ShouldUpdatePing` + `SV_EmitPings_internal` (`rehlds/engine/sv_main.cpp:4805-4929,
        // 4955, 5060`, read 2026-11-25): only emit `svc_pings` while `IN_SCORE` (`+showscores` /
        // `TAB`) is held or for an HLTV proxy, throttled per slot to `PINGS_INTERVAL_SECS`.
        let mut pings_block = MsgWriter::new();
        let tab_held = (self.clients[index].last_cmd.buttons & IN_SCORE) != 0;
        let is_proxy = self.views[index].userinfo.get("*hltv").is_some();
        if (tab_held || is_proxy) && self.real_time >= self.next_pings_at[index] {
            let pings: Vec<(u8, u16, u8)> = self
                .clients
                .iter()
                .enumerate()
                .filter(|(_, c)| c.state == ClientState::Active)
                .map(|(s, c)| (s as u8, c.pings.ping_ms(), 0))
                .collect();
            if !pings.is_empty() {
                self.next_pings_at[index] = self.real_time + PINGS_INTERVAL_SECS;
                write_svc_pings(&mut pings_block, &pings);
            }
        }

        // `SV_EmitEvents` (ReHLDS `rehlds/engine/sv_main.cpp`, read 2026-11-22): the events
        // queued for the client follow the packet entities. The queue is emptied whether or not
        // the datagram goes out, as there.
        let numbers: Vec<u16> = entities.iter().map(|s| s.number).collect();
        let wire_events = self.outbox.events[index].take_wire_events(&numbers);
        let queued_unreliable = std::mem::take(&mut self.clients[index].unreliable);
        let rate = self.client_rate(slot);

        // When the game DLL defines conditional encoders (`GameDll::delta_encode`), evaluate the
        // entity delta masks sequentially here so Stage 2 can bit-pack without `&mut self.game`.
        let prepared_entities = if self.game.has_delta_encoder() {
            let max_clients = self.cfg.max_clients;
            let client = &self.clients[index];
            let seq = client.chan.outgoing_sequence;
            let base = client
                .delta_seq
                .and_then(|delta_seq| client.frames.delta_base(delta_seq, seq));
            let game = &mut self.game;
            let mut encode = |name: &str,
                              from: &GoldSrcEntityState,
                              to: &GoldSrcEntityState,
                              marks: &mut DeltaMarks<'_>| {
                game.delta_encode(name, slot, from, to, marks);
            };
            let encode = Some(&mut encode as EntityEncodeFn<'_>);
            let plan = match base {
                Some(from) => {
                    let (expected_count, ops) = prepare_packetentities_delta_ops(
                        &self.deltas,
                        max_clients,
                        &self.baseline_records,
                        &from.entities,
                        &entities,
                        encode,
                    );
                    PreparedEntityPlan::Delta {
                        delta_seq: low_byte(from.seq),
                        expected_count,
                        ops,
                    }
                }
                None => {
                    let ops = prepare_packetentities_full_ops(
                        &self.deltas,
                        max_clients,
                        &self.baseline_records,
                        &entities,
                        encode,
                    );
                    PreparedEntityPlan::Full { ops }
                }
            };
            Some(plan)
        } else {
            None
        };

        Some(ClientFrameJob {
            rate,
            header_payload,
            client_data,
            weapon_list,
            entities,
            players,
            pings_block,
            wire_events,
            queued_unreliable,
            prepared_entities,
        })
    }

    fn recycle_slot_entities(&mut self, index: usize, mut entities: Vec<GoldSrcEntityState>) {
        entities.clear();
        let slots = &mut self.world.frame_scratch.slot_accepted;
        if slots.len() <= index {
            slots.resize_with(index + 1, Vec::new);
        }
        slots[index] = entities;
    }

    /// The start of `SV_WriteClientdataToMessage` (ReHLDS `sv_main.cpp`, read 2026-11-22): one
    /// `svc_choke` if any update was choked since the last one sent (however many: the source
    /// writes the byte once and clears the count), then the view the game forced on the client
    /// (`fixangle`): `svc_addangle` of the yaw turn in `avelocity[1]` for 2 (which it zeroes), a
    /// `svc_setangle` of `angles` for any other non-zero value; `fixangle` is cleared either way.
    pub(crate) fn write_choke_and_fixangle(&mut self, slot: u8, w: &mut MsgWriter) {
        let client = &mut self.clients[usize::from(slot)];
        if std::mem::take(&mut client.choke_count) != 0 {
            write_svc_choke(w);
        }
        let id = self.edicts.client(slot);
        let Some(edict) = self.edicts.get_mut(id) else {
            return;
        };
        let vars = &mut edict.vars;
        if vars.fixangle == 0 {
            return;
        }
        if vars.fixangle == 2 {
            write_svc_addangle(w, vars.avelocity.y);
            vars.avelocity.y = 0.0;
        } else {
            write_svc_setangle(w, vars.angles);
        }
        vars.fixangle = 0;
    }

    /// The client data of `slot` as `svc_clientdata` against the zero baseline (the spawn answer
    /// of `SV_WriteSpawn` sends it before the client has acknowledged any update), preceded by
    /// the choke and `fixangle` messages of [`write_choke_and_fixangle`](Self::write_choke_and_fixangle).
    /// Asks the game for the client data (the engine's `UpdateClientData` export) and, for a
    /// client with `cl_lw`, the weapon data. Not kept as a delta base. Does not drain the
    /// outbox: what the game sent while it ran goes to the client's reliable stream after the
    /// answer, as `SV_WriteSpawn` leaves it in `netchan.message` behind the answer (the caller
    /// drains once the answer is written).
    pub(crate) fn write_spawn_clientdata(&mut self, slot: u8, w: &mut MsgWriter) {
        self.write_choke_and_fixangle(slot, w);
        let client_data = self.call_game(|game, eng| game.update_client_data(eng, slot));
        let weapon_list = if self.client_has_lw(slot) {
            let mut list = [GoldSrcWeaponData::default(); 64];
            self.call_game_synced(|game, eng| game.get_weapon_data(eng, slot, &mut list))
                .then_some(list)
        } else {
            None
        };
        let Some(schema) = self.deltas.get("clientdata_t") else {
            return;
        };
        let weapon_schema = self.deltas.get("weapon_data_t");
        let weapondata = match (weapon_schema, &weapon_list) {
            (Some(ws), Some(list)) => weapon_records(ws, list),
            _ => WeaponRecords::new(),
        };
        let no_weapons = WeaponRecords::new();
        let weapons = weapon_schema
            .zip(weapon_list.as_ref())
            .map(|(schema, _)| WeaponDelta {
                schema,
                from: &no_weapons,
                to: &weapondata,
            });
        write_svc_clientdata_with_weapons(w, schema, None, &client_data, weapons, self.sv_time);
    }

    /// `host_client->lw`: the client's `cl_lw`, see `EngineApi::client_lw`.
    fn client_has_lw(&self, slot: u8) -> bool {
        userinfo_lw(&self.views[usize::from(slot)].userinfo)
    }

    /// The players among `states`, as lag compensation will need them later.
    fn frame_players(&self, states: &[GoldSrcEntityState]) -> Vec<FramePlayer> {
        let max_clients = u16::from(self.cfg.max_clients);
        states
            .iter()
            .filter(|s| (1..=max_clients).contains(&s.number))
            .map(|s| FramePlayer {
                number: s.number,
                origin: s.origin,
                effects: s.effects,
                health: self
                    .edicts
                    .by_index(s.number)
                    .map_or(0.0, |(_, e)| e.vars.health),
            })
            .collect()
    }

    /// Whether entity `id` is potentially visible in `pvs` (`None`: the viewer sees all): by the
    /// leaves it was linked with, or, for an entity that was never linked, by the leaves its
    /// box `origin + mins..origin + maxs` touches.
    fn entity_in_pvs(&self, pvs: Option<&[u64]>, id: EdictId, v: &EntVars) -> bool {
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
    /// the rest are not offered to the game. Client slots are the lowest edict numbers, so
    /// they come first and the cap never cuts one. Candidates are the live edicts with a model
    /// (not the world), without `EF_NODRAW` unless the viewer's own; for each the engine works
    /// out whether it is potentially visible from the viewer's eye (the game's
    /// `setup_visibility` may move it) and the game's `add_to_full_pack` decides, with the
    /// state it may edit ([`FullPackCtx`]). The caller drains the outbox.
    fn visible_entities(&mut self, slot: u8) -> Vec<GoldSrcEntityState> {
        let index = usize::from(slot);
        let viewer = self.edicts.client(slot);
        let own_eye = self
            .edicts
            .get(viewer)
            .map(|e| e.vars.origin + e.vars.view_ofs)
            .unwrap_or_default();
        // `SetupVisibility`: the game may move the viewpoint (a spectator sees through the
        // player it follows). The sign-on states are copied once, for this call and the ones below.
        self.sync_views();
        let eye = self
            .call_game_synced(|game, eng| game.setup_visibility(eng, slot))
            .filter(|p| p.is_finite())
            .unwrap_or(own_eye);
        let max_clients_u8 = self.cfg.max_clients;

        let mut candidates = std::mem::take(&mut self.world.frame_scratch.pvs_candidates);
        candidates.clear();
        {
            let pvs = self.map.viewer_pvs_row(eye);
            for &(id, nodraw) in &self.world.frame_scratch.live_edicts {
                if nodraw && id != viewer {
                    continue;
                }
                let Some(edict) = self.edicts.get(id) else {
                    continue;
                };
                if edict.vars.modelindex == 0 {
                    continue;
                }
                candidates.push((id, self.entity_in_pvs(pvs, id, &edict.vars)));
            }
        }

        let mut viewer_leaves = std::mem::replace(
            &mut self.world.frame_scratch.viewer_leaves,
            LinkSnapshot {
                leafnums: Vec::new(),
                headnode: -1,
                area: None,
            },
        );
        self.world.fill_link_snapshot(viewer, &mut viewer_leaves);

        let hostflags = u8::from(self.client_has_lw(slot));
        let max_clients = u16::from(self.cfg.max_clients);
        let mut accepted = if index < self.world.frame_scratch.slot_accepted.len() {
            std::mem::take(&mut self.world.frame_scratch.slot_accepted[index])
        } else {
            Vec::new()
        };
        accepted.clear();

        for &(ent_id, in_pvs) in &candidates {
            let ent = ent_id.index;
            if ent > max_clients && accepted.len() >= MAX_PACKET_ENTITIES {
                tracing::debug!(slot, "too many entities in the visible packet list");
                break;
            }
            // The state is copied only for an entity the game is asked about: the game may edit
            // it, so it cannot be built after the answer (the first two checks and the cap
            // stop the rest from being copied at all).
            let Some(edict) = self.edicts.get(ent_id) else { continue };
            let mut state = entity_state_from_vars(ent, &edict.vars, max_clients_u8);
            let ctx = FullPackCtx {
                ent,
                ent_id,
                viewer: slot,
                viewer_id: viewer,
                hostflags,
                in_pvs,
                viewer_leaves: &viewer_leaves,
            };
            if self.call_game_synced(|game, eng| game.add_to_full_pack(eng, &ctx, &mut state)) {
                accepted.push(state);
            }
        }

        self.world.frame_scratch.pvs_candidates = candidates;
        self.world.frame_scratch.viewer_leaves = viewer_leaves;
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
            weapondata: WeaponRecords::new(),
            sent_at: 0.0,
            ping_time: -1.0,
            players: Vec::new(),
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
        assert!(close(update_interval(Some("1000"), &cfg), 1.0 / 30.0));
        assert!(close(update_interval(None, &cfg), 0.05));
        assert!(close(update_interval(Some("abc"), &cfg), 0.05));
        assert!(close(update_interval(Some("1"), &cfg), 0.1));
        let high_cap = ServerConfig {
            max_update_rate: 102,
            ..cfg
        };
        assert!(close(update_interval(Some("1000"), &high_cap), 1.0 / 102.0));
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

    #[derive(Default)]
    struct CaptureTransport {
        sent: Vec<(cs_transport::PeerAddr, Vec<u8>)>,
    }

    impl PacketTransport for CaptureTransport {
        fn send(
            &mut self,
            to: cs_transport::PeerAddr,
            data: &[u8],
        ) -> Result<(), cs_transport::TransportError> {
            self.sent.push((to, data.to_vec()));
            Ok(())
        }

        fn recv(&mut self) -> Option<(cs_transport::PeerAddr, Vec<u8>)> {
            None
        }
    }

    struct DeterministicGame {
        use_encoder: bool,
        tick: u32,
    }

    impl GameDll for DeterministicGame {
        fn usermsgs(&self) -> cs_proto48::usermsg::UserMsgRegistry {
            cs_proto48::usermsg::UserMsgRegistry::cs16_default()
        }

        fn server_activate(&mut self, _eng: &mut crate::game::EngineApi<'_>) {}
        fn server_deactivate(&mut self, _eng: &mut crate::game::EngineApi<'_>) {}
        fn spawn_entity(
            &mut self,
            _eng: &mut crate::game::EngineApi<'_>,
            _id: EdictId,
            _classname: &str,
            _keys: &[(String, String)],
        ) -> bool {
            true
        }
        fn client_connect(
            &mut self,
            _eng: &mut crate::game::EngineApi<'_>,
            _slot: u8,
        ) -> Result<(), String> {
            Ok(())
        }
        fn client_put_in_server(&mut self, _eng: &mut crate::game::EngineApi<'_>, _slot: u8) {}
        fn client_disconnect(&mut self, _eng: &mut crate::game::EngineApi<'_>, _slot: u8) {}
        fn client_command(
            &mut self,
            _eng: &mut crate::game::EngineApi<'_>,
            _slot: u8,
            _argv: &[String],
        ) {
        }
        fn client_userinfo_changed(&mut self, _eng: &mut crate::game::EngineApi<'_>, _slot: u8) {}
        fn run_usercmd(
            &mut self,
            _eng: &mut crate::game::EngineApi<'_>,
            _slot: u8,
            _cmd: &cs_proto48::delta::GoldSrcUserCmd,
            _random_seed: u32,
        ) {
        }
        fn start_frame(&mut self, _eng: &mut crate::game::EngineApi<'_>, _frametime: f32) {}

        fn update_client_data(
            &mut self,
            _eng: &mut crate::game::EngineApi<'_>,
            slot: u8,
        ) -> GoldSrcClientData {
            GoldSrcClientData {
                origin: glam::Vec3::new(
                    f32::from(slot) * 64.0 + self.tick as f32 * 3.5,
                    -120.0 + f32::from(slot) * 16.0,
                    36.0,
                ),
                velocity: glam::Vec3::new(150.0, -25.0 * f32::from(slot), 0.0),
                view_ofs: glam::Vec3::new(0.0, 0.0, 17.0),
                health: 100.0 - f32::from(slot) * 5.0,
                weapons: 0x0003_0005,
                ammo_nails: 30 - self.tick as i32,
                ..GoldSrcClientData::default()
            }
        }

        fn get_weapon_data(
            &mut self,
            _eng: &mut crate::game::EngineApi<'_>,
            slot: u8,
            weapons: &mut [GoldSrcWeaponData; 64],
        ) -> bool {
            weapons[1] = GoldSrcWeaponData {
                m_i_id: 1,
                m_i_clip: 12 - (self.tick as i32 % 5),
                m_fl_next_primary_attack: 0.25 + f32::from(slot) * 0.01,
                ..GoldSrcWeaponData::default()
            };
            weapons[28] = GoldSrcWeaponData {
                m_i_id: 28,
                m_i_clip: 30 - (self.tick as i32 % 10),
                m_fl_next_primary_attack: 0.5 + f32::from(slot) * 0.02,
                ..GoldSrcWeaponData::default()
            };
            true
        }

        fn add_to_full_pack(
            &mut self,
            _eng: &mut crate::game::EngineApi<'_>,
            ctx: &FullPackCtx<'_>,
            state: &mut GoldSrcEntityState,
        ) -> bool {
            // Vary visibility per viewer and tick so full/delta removals, additions, and
            // unchanged entities are all exercised across clients.
            if ctx.ent > 12
                && (ctx.ent + u16::from(ctx.viewer) + self.tick as u16).is_multiple_of(3)
            {
                return false;
            }
            state.frame = (self.tick as f32 * 4.0) % 256.0;
            true
        }

        fn has_delta_encoder(&self) -> bool {
            self.use_encoder
        }

        fn delta_encode(
            &mut self,
            schema_name: &str,
            viewer: u8,
            _from: &GoldSrcEntityState,
            to: &GoldSrcEntityState,
            marks: &mut DeltaMarks<'_>,
        ) {
            if schema_name == "entity_state_player_t" && to.number == u16::from(viewer) + 1 {
                marks.unset_mask(marks.entity_animtime_mask());
            }
        }
    }

    fn build_deterministic_server(use_encoder: bool) -> Server<DeterministicGame> {
        let cfg = ServerConfig {
            max_clients: 10,
            ..ServerConfig::default()
        };
        let mut server = Server::new(
            cfg,
            crate::map::ServerMap::stub("de_dust2"),
            DeterministicGame {
                use_encoder,
                tick: 0,
            },
            DeltaRegistry::cs16_default(),
            cs_cvar::CVarRegistry::new_with_defaults(),
        );
        // 8 active human clients (slots 0..8), 1 signon client (slot 8), 1 fake bot (slot 9).
        for slot in 0..8u8 {
            let idx = usize::from(slot);
            let client = &mut server.clients[idx];
            client.state = ClientState::Active;
            client.addr = Some(cs_transport::PeerAddr::Loopback(u32::from(slot) + 1));
            client.chan.outgoing_sequence = 10 + u32::from(slot);
            client.next_update_at = 0.0;
            client.clear_time = 0.0;
            client.choke_count = u32::from(slot % 2);
            if slot.is_multiple_of(2) {
                client.last_cmd.buttons = IN_SCORE;
            }
            client.unreliable = vec![0x08, slot, 0xAA, 0xBB];
            client.chan.write_reliable(&[0x08, b'r', b'e', b'l', slot]);
            server.views[idx].active = true;
            server.views[idx].userinfo =
                cs_proto48::info::InfoString::parse("\\cl_lw\\1\\cl_updaterate\\20\\rate\\30000");

            let eid = server.edicts.client(slot);
            if let Some(ent) = server.edicts.get_mut(eid) {
                ent.vars.modelindex = 2;
                ent.vars.origin = glam::Vec3::new(f32::from(slot) * 96.0, 48.0, 0.0);
                ent.vars.angles = glam::Vec3::new(0.0, f32::from(slot) * 30.0, 0.0);
                ent.vars.health = 100.0;
                ent.vars.fixangle = if slot == 3 { 1 } else if slot == 5 { 2 } else { 0 };
                ent.vars.avelocity.y = 12.5;
            }
        }
        // Slot 8: signing on.
        server.clients[8].state = ClientState::Signon(crate::client::SignonStage::AwaitSpawn);
        server.clients[8].addr = Some(cs_transport::PeerAddr::Loopback(9));
        server.clients[8].chan.write_reliable(&[0x09, b's', b'i', b'g', b'n']);
        // Slot 9: fake client (bot, skipped).
        server.clients[9] = Client::fake_client(99, 0.0);

        // Spawn 20 world / custom entities so each snapshot compresses a realistic entity set.
        for i in 0..20u16 {
            let id = server.edicts.alloc(1.0).expect("free edict");
            if let Some(ent) = server.edicts.get_mut(id) {
                ent.vars.modelindex = 3 + (i % 4);
                ent.vars.origin = glam::Vec3::new(f32::from(i) * 32.0, -64.0, 12.0);
                ent.vars.angles = glam::Vec3::new(0.0, f32::from(i) * 15.0, 0.0);
                if i.is_multiple_of(5) {
                    ent.vars.flags |= crate::world::consts::FL_CUSTOMENTITY;
                }
            }
        }
        server
    }

    fn advance_world_and_ack(server: &mut Server<DeterministicGame>, tick: u32) {
        server.game.tick = tick;
        server.real_time = 1.0 + f64::from(tick) * 0.1;
        server.sv_time = 1.0 + f64::from(tick) * 0.1;
        for slot in 0..8u8 {
            let idx = usize::from(slot);
            if tick > 0 {
                // Acknowledge the previous snapshot so delta compression (`svc_deltapacketentities`)
                // activates on frames 1 and 2.
                let last_seq = server.clients[idx].chan.outgoing_sequence.wrapping_sub(1);
                server.clients[idx].delta_seq = Some(low_byte(last_seq));
                server.clients[idx].unreliable = vec![0x08, slot, tick as u8];
            }
            let eid = server.edicts.client(slot);
            if let Some(ent) = server.edicts.get_mut(eid) {
                ent.vars.origin.x += (tick as f32) * 12.0;
                ent.vars.angles.y = (ent.vars.angles.y + 7.5) % 360.0;
            }
        }
        for n in 11..=30u16 {
            if n.is_multiple_of(2)
                && let Some(id) = server.edicts.by_index(n).map(|(id, _)| id)
                && let Some(ent) = server.edicts.get_mut(id)
            {
                ent.vars.origin.z += 1.25;
            }
        }
    }

    #[test]
    fn parallel_snapshot_compression_matches_sequential_byte_for_byte() {
        for use_encoder in [false, true] {
            let mut seq_srv = build_deterministic_server(use_encoder);
            let mut par_srv = build_deterministic_server(use_encoder);

            for tick in 0..4u32 {
                advance_world_and_ack(&mut seq_srv, tick);
                advance_world_and_ack(&mut par_srv, tick);

                let mut seq_cap = CaptureTransport::default();
                let mut par_cap = CaptureTransport::default();
                seq_srv.send_client_frames_with_mode(&mut seq_cap, false);
                par_srv.send_client_frames_with_mode(&mut par_cap, true);

                assert_eq!(
                    seq_cap.sent.len(),
                    9,
                    "expected 8 active + 1 signon datagrams (use_encoder={use_encoder}, tick={tick})"
                );
                assert_eq!(
                    seq_cap.sent, par_cap.sent,
                    "outgoing UDP packets diverged (use_encoder={use_encoder}, tick={tick})"
                );
                assert_eq!(seq_srv.net_stats, par_srv.net_stats);
                for slot in 0..8usize {
                    assert_eq!(
                        seq_srv.clients[slot].clear_time.to_bits(),
                        par_srv.clients[slot].clear_time.to_bits()
                    );
                    assert_eq!(
                        seq_srv.clients[slot].chan.outgoing_sequence,
                        par_srv.clients[slot].chan.outgoing_sequence
                    );
                    let last_seq = seq_srv.clients[slot].chan.outgoing_sequence.wrapping_sub(1);
                    assert_eq!(
                        seq_srv.clients[slot].frames.get(last_seq),
                        par_srv.clients[slot].frames.get(last_seq)
                    );
                }
            }
        }
    }
}
