//! HLTV time-delay ring buffer and late-join signon state tracker.
//!
//! Stores captured GoldSrc `svc_*` frames stamped with their capture timestamp
//! and releases them once `now >= frame.capture_time + delay_secs`. While
//! buffering, it parses critical signon and state-sync messages (`ServerInfo`,
//! `DeltaDescription`, `NewUserMsg`, `NewMoveVars`, `ResourceList`,
//! `SpawnBaseline`, `UpdateUserInfo`, `LightStyle`, `PacketEntities`) so that
//! spectators joining mid-match receive a complete, consistent initial state
//! aligned to `now - tv_delay`.

use std::collections::{BTreeMap, VecDeque};

use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::delta::{
    DeltaRecord, DeltaRegistry, GoldSrcClientData, GoldSrcEntityState, write_delta,
};
use cs_proto48::messages::{
    MoveVarsMsg, ResourceEntry, ServerInfoMsg, SvcMessage, SvcParseContext, drc,
    entity_schema_name, parse_svc_stream_partial, svc, wire_entity_type, write_svc_cdtrack,
    write_svc_clientdata, write_svc_deltadescriptions, write_svc_hltv, write_svc_lightstyle,
    write_svc_newmovevars, write_svc_packetentities_full, write_svc_print,
    write_svc_resourcelist_with_consistency, write_svc_resourcelocation, write_svc_resourcerequest,
    write_svc_sendextrainfo, write_svc_serverinfo, write_svc_setview, write_svc_signonnum,
    write_svc_time, write_svc_updateuserinfo, write_svc_userregs, write_svc_voiceinit,
};
use cs_proto48::usermsg::UserMsgRegistry;

/// Default HLTV broadcast delay (`tv_delay 30`).
pub const DEFAULT_TV_DELAY_SECS: f64 = 30.0;

/// Default upper bound on buffered frames in [`DelayBuffer`].
pub const DEFAULT_MAX_DELAY_FRAMES: usize = 16_384;

/// A single captured `svc_*` frame held in [`DelayBuffer`].
#[derive(Debug, Clone, PartialEq)]
pub struct DelayFrame {
    /// Capture timestamp in proxy clock seconds.
    pub capture_time: f64,
    /// Whether the frame belonged to the upstream sign-on phase.
    pub is_signon: bool,
    /// Raw concatenated `svc_*` wire payload.
    pub payload: Vec<u8>,
}

/// Internal entry pairing a public [`DelayFrame`] with optional upstream sequence.
#[derive(Debug, Clone)]
struct QueuedFrame {
    frame: DelayFrame,
    incoming_seq: Option<u8>,
}

/// Materialized sign-on and world-state snapshot at a point on the HLTV timeline.
#[derive(Debug, Clone)]
pub struct SignonSnapshot {
    pub server_info: Option<ServerInfoMsg>,
    pub send_extra_info: Option<(String, bool)>,
    pub deltas: DeltaRegistry,
    pub usermsgs: UserMsgRegistry,
    pub movevars: Option<MoveVarsMsg>,
    pub cd_track: Option<(i8, i8)>,
    pub set_view: Option<u16>,
    pub resource_request: Option<(i32, i32)>,
    pub resource_list: Option<(Vec<ResourceEntry>, Vec<u16>)>,
    pub resource_location: Option<String>,
    pub baselines: BTreeMap<u16, (u8, DeltaRecord)>,
    pub instance_baselines: Vec<DeltaRecord>,
    pub light_styles: BTreeMap<u8, String>,
    /// Latest userinfo per player slot: `(userid, info_string, cdkey_hash)`.
    pub userinfos: BTreeMap<u8, (i32, String, [u8; 16])>,
    pub voice_init: Option<(String, u8)>,
    pub server_time: f32,
    pub latest_entities: Vec<GoldSrcEntityState>,
}

impl Default for SignonSnapshot {
    fn default() -> Self {
        Self {
            server_info: None,
            send_extra_info: None,
            deltas: DeltaRegistry::cs16_default(),
            usermsgs: UserMsgRegistry::new(),
            movevars: None,
            cd_track: None,
            set_view: None,
            resource_request: None,
            resource_list: None,
            resource_location: None,
            baselines: BTreeMap::new(),
            instance_baselines: Vec::new(),
            light_styles: BTreeMap::new(),
            userinfos: BTreeMap::new(),
            voice_init: None,
            server_time: 1.0,
            latest_entities: Vec::new(),
        }
    }
}

impl SignonSnapshot {
    /// Applies decoded `svc_*` messages onto this snapshot.
    pub fn apply_messages(&mut self, msgs: &[SvcMessage]) {
        for msg in msgs {
            match msg {
                SvcMessage::ServerInfo(info) => {
                    self.userinfos.clear();
                    self.baselines.clear();
                    self.instance_baselines.clear();
                    self.light_styles.clear();
                    self.latest_entities.clear();
                    self.resource_request = None;
                    self.resource_list = None;
                    self.resource_location = None;
                    self.server_info = Some((**info).clone());
                }
                SvcMessage::SendExtraInfo {
                    fallback_dir,
                    cheats,
                } => {
                    self.send_extra_info = Some((fallback_dir.clone(), *cheats));
                }
                SvcMessage::DeltaDescription(schema) => {
                    self.deltas.insert((**schema).clone());
                }
                SvcMessage::NewUserMsg(desc) => {
                    self.usermsgs.register(desc.id, desc.size, desc.name.clone());
                }
                SvcMessage::NewMoveVars(mv) => {
                    self.movevars = Some((**mv).clone());
                }
                SvcMessage::CdTrack { track, loop_track } => {
                    self.cd_track = Some((*track, *loop_track));
                }
                SvcMessage::SetView(ent) => {
                    self.set_view = Some(*ent);
                }
                SvcMessage::ResourceRequest {
                    spawn_count,
                    reserved,
                } => {
                    self.resource_request = Some((*spawn_count, *reserved));
                }
                SvcMessage::ResourceList {
                    resources,
                    consistency_indices,
                } => {
                    self.resource_list = Some((resources.clone(), consistency_indices.clone()));
                }
                SvcMessage::ResourceLocation(url) => {
                    self.resource_location = Some(url.clone());
                }
                SvcMessage::SpawnBaseline {
                    baselines,
                    instance_baselines,
                } => {
                    self.baselines = baselines.clone();
                    self.instance_baselines = instance_baselines.clone();
                }
                SvcMessage::LightStyle { index, pattern } => {
                    self.light_styles.insert(*index, pattern.clone());
                }
                SvcMessage::UpdateUserInfo {
                    slot,
                    userid,
                    userinfo,
                    cdkey_hash,
                } => {
                    if userinfo.is_empty() {
                        self.userinfos.remove(slot);
                    } else {
                        self.userinfos
                            .insert(*slot, (*userid, userinfo.clone(), *cdkey_hash));
                    }
                }
                SvcMessage::VoiceInit { codec, quality } => {
                    self.voice_init = Some((codec.clone(), *quality));
                }
                SvcMessage::Time(t) => {
                    self.server_time = *t;
                }
                SvcMessage::PacketEntities(pe) => {
                    let max_clients = self.server_info.as_ref().map_or(32, |i| i.max_clients);
                    self.latest_entities = pe
                        .entities
                        .iter()
                        .filter_map(|(&number, (ent_type, rec))| {
                            let wire_type = wire_entity_type(*ent_type);
                            let schema = self
                                .deltas
                                .get(entity_schema_name(number, wire_type, max_clients))?;
                            Some(GoldSrcEntityState::from_delta(
                                number, wire_type, rec, schema,
                            ))
                        })
                        .collect();
                }
                _ => {}
            }
        }
    }

    /// Encodes the Protocol 48 `new` sign-on stage (`svc_serverinfo`,
    /// `svc_deltadescriptions`, `svc_newmovevars`, `svc_newusermsg`, active
    /// `svc_updateuserinfo`, etc.). Returns `None` if no `ServerInfo` exists.
    pub fn encode_new_stage(
        &self,
        player_slot_override: Option<u8>,
        hostname_override: Option<&str>,
    ) -> Option<Vec<u8>> {
        let mut info = self.server_info.clone()?;
        if let Some(slot) = player_slot_override {
            let max_idx = info.max_clients.saturating_sub(1);
            info.player_slot = slot.min(max_idx);
        }
        if let Some(host) = hostname_override
            && !host.is_empty()
        {
            info.hostname = host.to_string();
        }

        let mut w = MsgWriter::with_capacity(4096);
        write_svc_print(
            &mut w,
            &format!("\x02\nHLTV Proxy\nServer # {}\n", info.spawn_count),
        );
        write_svc_serverinfo(&mut w, &info);
        let (fallback, cheats) = self
            .send_extra_info
            .as_ref()
            .map_or(("", false), |(d, c)| (d.as_str(), *c));
        write_svc_sendextrainfo(&mut w, fallback, cheats);
        write_svc_deltadescriptions(&mut w, &self.deltas);
        let mv = self.movevars.clone().unwrap_or_default();
        write_svc_newmovevars(&mut w, &mv);
        let (track, loop_track) = self.cd_track.unwrap_or((0, 0));
        write_svc_cdtrack(&mut w, track, loop_track);
        let view_ent = self
            .set_view
            .unwrap_or_else(|| u16::from(info.player_slot) + 1);
        write_svc_setview(&mut w, view_ent);
        write_svc_userregs(&mut w, &self.usermsgs);
        for (&slot, (userid, userinfo, hash)) in &self.userinfos {
            write_svc_updateuserinfo(&mut w, slot, *userid, userinfo, hash);
        }
        Some(w.into_bytes())
    }

    /// Encodes the Protocol 48 `sendres` sign-on stage (`svc_resourcerequest`,
    /// `svc_resourcelist`, optional `svc_resourcelocation`).
    pub fn encode_sendres_stage(&self) -> Option<Vec<u8>> {
        let info = self.server_info.as_ref()?;
        let mut w = MsgWriter::with_capacity(1024);
        if let Some(url) = &self.resource_location {
            write_svc_resourcelocation(&mut w, url);
        }
        let spawn_count = self
            .resource_request
            .map_or(info.spawn_count as i32, |(sc, _)| sc);
        write_svc_resourcerequest(&mut w, spawn_count);
        match &self.resource_list {
            Some((res, consistency)) => {
                write_svc_resourcelist_with_consistency(&mut w, res, consistency);
            }
            None => {
                write_svc_resourcelist_with_consistency(&mut w, &[], &[]);
            }
        }
        Some(w.into_bytes())
    }

    /// Encodes the Protocol 48 `spawn` sign-on stage (`svc_spawnbaseline`,
    /// `svc_time`, `svc_updateuserinfo`, `svc_lightstyle`, `svc_clientdata`,
    /// optional `svc_hltv(HLTV_ACTIVE)`, `svc_signonnum(1)`, `svc_voiceinit`).
    pub fn encode_spawn_stage(&self, include_hltv_active: bool) -> Option<Vec<u8>> {
        let info = self.server_info.as_ref()?;
        let mut w = MsgWriter::with_capacity(4096);
        self.write_baselines(&mut w, info.max_clients);
        write_svc_time(&mut w, self.server_time);
        for (&slot, (userid, userinfo, hash)) in &self.userinfos {
            write_svc_updateuserinfo(&mut w, slot, *userid, userinfo, hash);
        }
        for (&idx, pattern) in &self.light_styles {
            write_svc_lightstyle(&mut w, idx, pattern);
        }
        if let Some(cd_schema) = self.deltas.get("clientdata_t") {
            let cd = GoldSrcClientData {
                fov: 90.0,
                maxspeed: 320.0,
                iuser1: 4, // OBS_ROAMING spectator mode
                ..Default::default()
            };
            write_svc_clientdata(&mut w, cd_schema, None, &cd, f64::from(self.server_time));
        }
        if include_hltv_active {
            write_svc_hltv(&mut w, drc::HLTV_ACTIVE);
        }
        write_svc_signonnum(&mut w, 1);
        let (codec, quality) = self
            .voice_init
            .as_ref()
            .map_or(("", 0), |(c, q)| (c.as_str(), *q));
        write_svc_voiceinit(&mut w, codec, quality);
        Some(w.into_bytes())
    }

    /// Encodes `svc_time` + full `svc_packetentities` for the snapshot's latest
    /// known entity state.
    pub fn encode_entities_full(&self) -> Option<Vec<u8>> {
        let info = self.server_info.as_ref()?;
        let mut w = MsgWriter::with_capacity(1024);
        write_svc_time(&mut w, self.server_time);
        write_svc_packetentities_full(
            &mut w,
            &self.deltas,
            info.max_clients,
            &self.baselines,
            &self.latest_entities,
            f64::from(self.server_time),
        );
        Some(w.into_bytes())
    }

    /// Produces the full sequence of initial `svc_*` packets for a late-joining
    /// spectator (`new`, `sendres`, `spawn`, and if entities exist, initial `packetentities`).
    pub fn encode_signon_packets(
        &self,
        player_slot_override: Option<u8>,
        hostname_override: Option<&str>,
        include_hltv_active: bool,
    ) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        if let Some(p) = self.encode_new_stage(player_slot_override, hostname_override) {
            packets.push(p);
        } else {
            return packets;
        }
        if let Some(p) = self.encode_sendres_stage() {
            packets.push(p);
        }
        if let Some(p) = self.encode_spawn_stage(include_hltv_active) {
            packets.push(p);
        }
        if !self.latest_entities.is_empty()
            && let Some(p) = self.encode_entities_full()
        {
            packets.push(p);
        }
        packets
    }

    fn write_baselines(&self, w: &mut MsgWriter, max_clients: u8) {
        w.write_u8(svc::SPAWNBASELINE);
        w.start_bit_writing();
        let st = f64::from(self.server_time);
        for (&entnum, (ent_type, rec)) in &self.baselines {
            let wire_type = wire_entity_type(*ent_type);
            if let Some(schema) = self
                .deltas
                .get(entity_schema_name(entnum, wire_type, max_clients))
                && rec.values.len() == schema.fields.len()
            {
                let zero = schema.zero_record();
                w.write_bits(u32::from(entnum), 11);
                w.write_bits(u32::from(wire_type), 2);
                write_delta(w, schema, &zero, rec, true, st);
            }
        }
        w.write_bits(0xFFFF, 16);
        let num_inst = self.instance_baselines.len().min(63);
        if let Some(ent_schema) = self.deltas.get("entity_state_t") {
            let valid: Vec<&DeltaRecord> = self
                .instance_baselines
                .iter()
                .take(num_inst)
                .filter(|r| r.values.len() == ent_schema.fields.len())
                .collect();
            w.write_bits(valid.len() as u32, 6);
            let zero = ent_schema.zero_record();
            for rec in valid {
                write_delta(w, ent_schema, &zero, rec, true, st);
            }
        } else {
            w.write_bits(0, 6);
        }
        w.end_bit_writing();
    }
}

/// Bounded HLTV broadcast delay queue (`tv_delay`).
///
/// Frames are pushed at `capture_time = now` and withheld until
/// `now >= frame.capture_time + delay_secs`, while tracking sign-on headers and
/// incremental state so late-joining spectators receive an exact sign-on
/// snapshot at `now - delay_secs`.
#[derive(Debug, Clone)]
pub struct DelayBuffer {
    delay_secs: f64,
    max_frames: usize,
    frames: VecDeque<QueuedFrame>,
    dropped_frames: u64,
    /// Parser context at the live capture edge (handles incoming delta packets).
    capture_ctx: SvcParseContext,
    /// Initial sign-on snapshot captured for the current map before `tv_delay` has elapsed.
    initial_signon: SignonSnapshot,
    /// Latest live sign-on snapshot at the capture edge.
    captured_signon: SignonSnapshot,
    /// Parser context at the delayed release edge (`now - delay_secs`).
    delayed_ctx: SvcParseContext,
    /// Sign-on snapshot as of the last drained/released frame.
    delayed_signon: SignonSnapshot,
    synthetic_capture_seq: u8,
    synthetic_delayed_seq: u8,
}

impl Default for DelayBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_TV_DELAY_SECS)
    }
}

impl DelayBuffer {
    /// Creates a new delay buffer with `delay_secs` (`>= 0.0`, default `30.0`).
    pub fn new(delay_secs: f64) -> Self {
        let mut capture_ctx = SvcParseContext::new();
        capture_ctx.usermsgs = UserMsgRegistry::cs16_default();
        let mut delayed_ctx = SvcParseContext::new();
        delayed_ctx.usermsgs = UserMsgRegistry::cs16_default();

        Self {
            delay_secs: sanitize_delay(delay_secs),
            max_frames: DEFAULT_MAX_DELAY_FRAMES,
            frames: VecDeque::new(),
            dropped_frames: 0,
            capture_ctx,
            initial_signon: SignonSnapshot::default(),
            captured_signon: SignonSnapshot::default(),
            delayed_ctx,
            delayed_signon: SignonSnapshot::default(),
            synthetic_capture_seq: 0,
            synthetic_delayed_seq: 0,
        }
    }

    /// Configures the maximum number of queued frames before oldest frames are evicted.
    pub fn with_max_frames(mut self, max_frames: usize) -> Self {
        self.max_frames = max_frames.max(1);
        self
    }

    pub fn delay_secs(&self) -> f64 {
        self.delay_secs
    }

    pub fn set_delay_secs(&mut self, delay_secs: f64) {
        self.delay_secs = sanitize_delay(delay_secs);
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn max_frames(&self) -> usize {
        self.max_frames
    }

    pub fn dropped_frames(&self) -> u64 {
        self.dropped_frames
    }

    /// Returns the most recently captured sign-on snapshot (at the live edge).
    pub fn latest_captured_snapshot(&self) -> &SignonSnapshot {
        &self.captured_signon
    }

    /// Returns the sign-on snapshot at the delayed release edge.
    pub fn delayed_snapshot(&self) -> &SignonSnapshot {
        &self.delayed_signon
    }

    /// Returns the `UserMsg` registry at the delayed release edge.
    pub fn delayed_usermsgs(&self) -> &UserMsgRegistry {
        &self.delayed_ctx.usermsgs
    }

    /// Pushes a raw `svc_*` frame captured at `now`.
    pub fn push_frame(&mut self, now: f64, is_signon: bool, raw_svc: &[u8]) {
        self.push_frame_with_seq(now, is_signon, raw_svc, None);
    }

    /// Pushes a raw `svc_*` frame captured at `now` with an optional upstream
    /// netchan sequence byte (used as the delta-compression index).
    pub fn push_frame_with_seq(
        &mut self,
        now: f64,
        is_signon: bool,
        raw_svc: &[u8],
        incoming_seq: Option<u8>,
    ) {
        if raw_svc.is_empty() || !now.is_finite() {
            return;
        }
        let seq = incoming_seq.unwrap_or_else(|| {
            self.synthetic_capture_seq = self.synthetic_capture_seq.wrapping_add(1);
            self.synthetic_capture_seq
        });
        self.capture_ctx.last_incoming_seq = seq;

        let (msgs, _err) = parse_svc_stream_partial(raw_svc, &mut self.capture_ctx);
        let has_new_serverinfo = msgs
            .iter()
            .any(|m| matches!(m, SvcMessage::ServerInfo(_)));
        self.captured_signon.apply_messages(&msgs);

        if has_new_serverinfo {
            self.initial_signon = self.captured_signon.clone();
        } else if is_signon {
            self.initial_signon.apply_messages(&msgs);
        }

        while self.frames.len() >= self.max_frames {
            if let Some(evicted) = self.frames.pop_front() {
                self.dropped_frames = self.dropped_frames.saturating_add(1);
                let eseq = evicted.incoming_seq.unwrap_or_else(|| {
                    self.synthetic_delayed_seq = self.synthetic_delayed_seq.wrapping_add(1);
                    self.synthetic_delayed_seq
                });
                self.delayed_ctx.last_incoming_seq = eseq;
                let (emsgs, _) =
                    parse_svc_stream_partial(&evicted.frame.payload, &mut self.delayed_ctx);
                self.delayed_signon.apply_messages(&emsgs);
            }
        }

        self.frames.push_back(QueuedFrame {
            frame: DelayFrame {
                capture_time: now,
                is_signon,
                payload: raw_svc.to_vec(),
            },
            incoming_seq: Some(seq),
        });
    }

    /// Releases and returns all buffered frames whose `capture_time + delay_secs <= now`,
    /// preserving capture order.
    pub fn drain_due(&mut self, now: f64) -> Vec<DelayFrame> {
        self.drain_due_decoded(now)
            .into_iter()
            .map(|(frame, _)| frame)
            .collect()
    }

    /// Releases all due frames along with their parsed `SvcMessage` list using
    /// the delay buffer's sequence-accurate `SvcParseContext`.
    pub fn drain_due_decoded(&mut self, now: f64) -> Vec<(DelayFrame, Vec<SvcMessage>)> {
        if !now.is_finite() {
            return Vec::new();
        }
        let mut out = Vec::new();
        while let Some(front) = self.frames.front() {
            if now < front.frame.capture_time + self.delay_secs {
                break;
            }
            if let Some(entry) = self.frames.pop_front() {
                let seq = entry.incoming_seq.unwrap_or_else(|| {
                    self.synthetic_delayed_seq = self.synthetic_delayed_seq.wrapping_add(1);
                    self.synthetic_delayed_seq
                });
                self.delayed_ctx.last_incoming_seq = seq;
                let (msgs, _) =
                    parse_svc_stream_partial(&entry.frame.payload, &mut self.delayed_ctx);
                self.delayed_signon.apply_messages(&msgs);
                out.push((entry.frame, msgs));
            }
        }
        out
    }

    /// Computes the sign-on snapshot visible at the delayed timeline (`now - delay_secs`),
    /// falling back to the initial sign-on headers if `now` is still within the initial
    /// warm-up delay window.
    pub fn snapshot_at(&self, now: f64) -> SignonSnapshot {
        let mut snap = self.delayed_signon.clone();
        let mut ctx = self.delayed_ctx.clone();
        let mut synth = self.synthetic_delayed_seq;

        if now.is_finite() {
            for entry in &self.frames {
                if now < entry.frame.capture_time + self.delay_secs {
                    break;
                }
                let seq = entry.incoming_seq.unwrap_or_else(|| {
                    synth = synth.wrapping_add(1);
                    synth
                });
                ctx.last_incoming_seq = seq;
                let (msgs, _) = parse_svc_stream_partial(&entry.frame.payload, &mut ctx);
                snap.apply_messages(&msgs);
            }
        }

        if snap.server_info.is_none() && self.initial_signon.server_info.is_some() {
            return self.initial_signon.clone();
        }
        snap
    }

    /// Produces the initial `svc_*` sign-on packets for a spectator connecting at `now`
    /// (aligned to `now - tv_delay`).
    pub fn late_join_signon(&self, now: f64) -> Vec<Vec<u8>> {
        self.snapshot_at(now)
            .encode_signon_packets(None, None, true)
    }
}

fn sanitize_delay(secs: f64) -> f64 {
    if secs.is_finite() && secs >= 0.0 {
        secs
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_proto48::messages::{ENTITY_NORMAL, parse_svc_stream};
    use glam::Vec3;

    fn sample_server_info(map: &str, spawn_count: u32) -> ServerInfoMsg {
        ServerInfoMsg {
            protocol: 48,
            spawn_count,
            map_crc: 0xABCD_1234,
            client_dll_md5: [0x11; 16],
            max_clients: 16,
            player_slot: 0,
            is_deathmatch: true,
            game_dir: "cstrike".to_string(),
            hostname: "Test HLDS".to_string(),
            map_path: format!("maps/{map}.bsp"),
            map_cycle: String::new(),
            vac_secure: false,
        }
    }

    fn build_signon_bytes(map: &str, spawn_count: u32) -> Vec<u8> {
        let reg = DeltaRegistry::cs16_default();
        let ureg = UserMsgRegistry::cs16_default();
        let mut w = MsgWriter::new();
        write_svc_serverinfo(&mut w, &sample_server_info(map, spawn_count));
        write_svc_deltadescriptions(&mut w, &reg);
        write_svc_newmovevars(&mut w, &MoveVarsMsg::default());
        write_svc_userregs(&mut w, &ureg);
        write_svc_resourcerequest(&mut w, spawn_count as i32);
        write_svc_resourcelist_with_consistency(
            &mut w,
            &[ResourceEntry {
                res_type: 2,
                filename: format!("maps/{map}.bsp"),
                index: 1,
                download_size: 2048,
                flags: 0,
                md5: None,
                reserved: None,
            }],
            &[],
        );
        let player_bl = GoldSrcEntityState {
            number: 1,
            entity_type: ENTITY_NORMAL,
            modelindex: 3,
            ..Default::default()
        };
        cs_proto48::messages::write_svc_spawnbaseline(&mut w, &reg, 16, &[(1, player_bl)], 1.0);
        write_svc_updateuserinfo(&mut w, 0, 101, "\\name\\Alice\\model\\gordon", &[0; 16]);
        write_svc_lightstyle(&mut w, 0, "m");
        write_svc_signonnum(&mut w, 1);
        w.into_bytes()
    }

    #[test]
    fn withholds_frames_until_delay_expires_and_releases_in_order() {
        let mut buf = DelayBuffer::new(30.0);
        assert_eq!(buf.delay_secs(), 30.0);

        let mut f1 = MsgWriter::new();
        write_svc_print(&mut f1, "frame-1");
        let mut f2 = MsgWriter::new();
        write_svc_print(&mut f2, "frame-2");
        let mut f3 = MsgWriter::new();
        write_svc_print(&mut f3, "frame-3");

        buf.push_frame(1.0, false, f1.as_slice());
        buf.push_frame(2.5, false, f2.as_slice());
        buf.push_frame(5.0, false, f3.as_slice());
        assert_eq!(buf.len(), 3);

        assert!(buf.drain_due(10.0).is_empty());
        assert!(buf.drain_due(30.999).is_empty());

        let due1 = buf.drain_due(31.0);
        assert_eq!(due1.len(), 1);
        assert_eq!(due1[0].capture_time, 1.0);
        assert_eq!(due1[0].payload, f1.as_slice());

        let due23 = buf.drain_due(35.0);
        assert_eq!(due23.len(), 2);
        assert_eq!(due23[0].capture_time, 2.5);
        assert_eq!(due23[1].capture_time, 5.0);
        assert!(buf.is_empty());
    }

    #[test]
    fn late_join_signon_replays_headers_and_time_aligned_userinfo() {
        let mut buf = DelayBuffer::new(30.0);
        let signon = build_signon_bytes("de_dust2", 7);
        buf.push_frame(0.0, true, &signon);

        // At t=10s Bob joins; at t=12s entity updates; at t=40s Alice renames to Alice2.
        let mut u_bob = MsgWriter::new();
        write_svc_updateuserinfo(&mut u_bob, 1, 102, "\\name\\Bob", &[0; 16]);
        buf.push_frame(10.0, false, u_bob.as_slice());

        // Entity update at t=12s.
        let reg = DeltaRegistry::cs16_default();
        let ent = GoldSrcEntityState {
            number: 1,
            entity_type: ENTITY_NORMAL,
            origin: Vec3::new(100.0, 200.0, 32.0),
            modelindex: 3,
            ..Default::default()
        };
        let mut ent_msg = MsgWriter::new();
        write_svc_time(&mut ent_msg, 12.0);
        write_svc_packetentities_full(&mut ent_msg, &reg, 16, &BTreeMap::new(), &[ent], 12.0);
        buf.push_frame(12.0, false, ent_msg.as_slice());

        let mut u_alice2 = MsgWriter::new();
        write_svc_updateuserinfo(&mut u_alice2, 0, 101, "\\name\\Alice2", &[0; 16]);
        buf.push_frame(40.0, false, u_alice2.as_slice());

        // Drain up to t=45s (delayed playhead = 15s: includes t=0, t=10, t=12, NOT t=40).
        let drained = buf.drain_due(45.0);
        assert_eq!(drained.len(), 3);

        let packets = buf.late_join_signon(45.0);
        assert!(packets.len() >= 3);

        let mut ctx = SvcParseContext::new();
        let mut all_msgs = Vec::new();
        for pkt in &packets {
            all_msgs.extend(parse_svc_stream(pkt, &mut ctx).expect("valid late-join packet"));
        }

        assert!(all_msgs.iter().any(|m| matches!(
            m,
            SvcMessage::ServerInfo(si) if si.map_path == "maps/de_dust2.bsp" && si.spawn_count == 7
        )));
        assert!(
            all_msgs
                .iter()
                .any(|m| matches!(m, SvcMessage::DeltaDescription(_)))
        );
        assert!(
            all_msgs
                .iter()
                .any(|m| matches!(m, SvcMessage::NewUserMsg(_)))
        );
        assert!(
            all_msgs
                .iter()
                .any(|m| matches!(m, SvcMessage::NewMoveVars(_)))
        );
        assert!(
            all_msgs
                .iter()
                .any(|m| matches!(m, SvcMessage::ResourceList { .. }))
        );
        assert!(
            all_msgs
                .iter()
                .any(|m| matches!(m, SvcMessage::SpawnBaseline { .. }))
        );
        assert!(
            all_msgs
                .iter()
                .any(|m| matches!(m, SvcMessage::Hltv(drc::HLTV_ACTIVE)))
        );

        // Userinfo snapshot at t_delayed=15s must have Alice (not Alice2) and Bob.
        let mut slot_names = BTreeMap::new();
        for m in &all_msgs {
            if let SvcMessage::UpdateUserInfo { slot, userinfo, .. } = m {
                slot_names.insert(*slot, userinfo.clone());
            }
        }
        assert_eq!(
            slot_names.get(&0).map(String::as_str),
            Some("\\name\\Alice\\model\\gordon")
        );
        assert_eq!(slot_names.get(&1).map(String::as_str), Some("\\name\\Bob"));

        // Advance to t=75s (delayed playhead = 45s): now Alice2 must be active.
        let _ = buf.drain_due(75.0);
        let snap_75 = buf.snapshot_at(75.0);
        assert_eq!(
            snap_75.userinfos.get(&0).map(|(_, u, _)| u.as_str()),
            Some("\\name\\Alice2")
        );
    }

    #[test]
    fn bounded_capacity_evicts_oldest_and_preserves_state() {
        let mut buf = DelayBuffer::new(30.0).with_max_frames(2);
        buf.push_frame(0.0, true, &build_signon_bytes("de_inferno", 2));
        let mut u1 = MsgWriter::new();
        write_svc_updateuserinfo(&mut u1, 2, 200, "\\name\\Charlie", &[0; 16]);
        buf.push_frame(1.0, false, u1.as_slice());

        let mut p = MsgWriter::new();
        write_svc_print(&mut p, "overflow");
        buf.push_frame(2.0, false, p.as_slice());

        assert_eq!(buf.len(), 2);
        assert_eq!(buf.dropped_frames(), 1);
        assert_eq!(
            buf.delayed_snapshot()
                .server_info
                .as_ref()
                .map(|s| s.map_path.as_str()),
            Some("maps/de_inferno.bsp")
        );
    }

    #[test]
    fn negative_or_nan_delay_clamps_to_zero() {
        let mut buf = DelayBuffer::new(-5.0);
        assert_eq!(buf.delay_secs(), 0.0);
        buf.set_delay_secs(f64::NAN);
        assert_eq!(buf.delay_secs(), 0.0);

        let mut p = MsgWriter::new();
        write_svc_print(&mut p, "instant");
        buf.push_frame(10.0, false, p.as_slice());
        assert_eq!(buf.drain_due(10.0).len(), 1);
    }
}
