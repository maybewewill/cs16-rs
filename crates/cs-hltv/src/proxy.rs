//! HLTV proxy core: upstream Protocol 48 client (`\hltv\1`), downstream
//! spectator server (`NetChan`, `A2S_INFO` with `server_type = b'p'`),
//! auto-director (`svc_director`), `clc_hltv` / `svc_hltv` control messages,
//! and optional `.dem` recording via [`cs_clientcore::DemoRecorder`].

use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, RandomState};
use std::path::PathBuf;

use cs_clientcore::DemoRecorder;
use cs_cvar::{CVarFlags, CVarRegistry};
use cs_proto48::bitbuf::{MsgWriter, ProtoError};
use cs_proto48::crypto::{com_munge2, md5_hex};
use cs_proto48::delta::{DeltaRecord, DeltaRegistry, DeltaSchema, GoldSrcEntityState};
use cs_proto48::demo::DemoNetMsgInfo;
use cs_proto48::event::{write_svc_event, write_svc_event_reliable};
use cs_proto48::handshake::{
    OobPacket, default_cs16_protinfo, default_cs16_userinfo, encode_a2a_ack, encode_connect_48,
    encode_getchallenge, encode_s2c_challenge, encode_s2c_connection, encode_s2c_reject,
    lan_steam_auth_ticket, parse_oob_packet, tokenize_goldsrc_line,
};
use cs_proto48::info::InfoString;
use cs_proto48::messages::{
    ClcMessage, ServerInfoMsg, SvcMessage, SvcParseContext, WeaponDelta, WeaponRecords, drc,
    entity_schema_name, parse_clc_stream, parse_svc_stream_partial, svc, wire_entity_type,
    write_clc_cvarvalue, write_clc_cvarvalue2, write_clc_delta, write_clc_fileconsistency,
    write_clc_hltv, write_clc_nop, write_clc_resourcelist, write_clc_stringcmd, write_svc_addangle,
    write_svc_cdtrack, write_svc_centerprint, write_svc_choke, write_svc_clientdata_with_weapons,
    write_svc_crosshairangle, write_svc_customization, write_svc_cutscene, write_svc_damage,
    write_svc_deltadescriptions, write_svc_director, write_svc_disconnect, write_svc_exec,
    write_svc_filetxferfailed, write_svc_finale, write_svc_foundsecret, write_svc_hltv,
    write_svc_intermission, write_svc_killedmonster, write_svc_lightstyle, write_svc_newmovevars,
    write_svc_nop, write_svc_packetentities_delta, write_svc_packetentities_full,
    write_svc_particle, write_svc_pings, write_svc_print,
    write_svc_resourcelist_with_consistency, write_svc_resourcelocation, write_svc_resourcerequest,
    write_svc_restore, write_svc_roomtype, write_svc_sendcvarvalue, write_svc_sendcvarvalue2,
    write_svc_sendextrainfo, write_svc_serverinfo, write_svc_setangle, write_svc_setpause,
    write_svc_setview, write_svc_signonnum, write_svc_sound, write_svc_soundfade,
    write_svc_spawnstatic, write_svc_spawnstaticsound, write_svc_stufftext, write_svc_temp_entity,
    write_svc_time, write_svc_timescale, write_svc_updateuserinfo, write_svc_usermsg,
    write_svc_version, write_svc_voiceinit, write_svc_voicedata,
};
use cs_proto48::netchan::{
    FRAGMENT_C2S_SIZE, FRAGMENT_S2C_MAX_SIZE, NetChan, SplitPacketAssembler,
};
use cs_proto48::query::{
    PlayerQueryEntry, ServerQueryInfo, encode_s2a_info_src, encode_s2a_players, encode_s2a_rules,
    encode_s2c_query_challenge,
};
use cs_proto48::usermsg::{CsUserMsg, UserMsgRegistry};
use cs_proto48::{CONNECTIONLESS_HEADER, PROTOCOL_VERSION};
use cs_transport::{PacketTransport, PeerAddr, TransportError};
use glam::Vec3;
use tracing::{info, warn};

#[cfg(not(target_arch = "wasm32"))]
use cs_transport::UdpTransport;

use crate::delay::{DEFAULT_TV_DELAY_SECS, DelayBuffer, SignonSnapshot};

const CHALLENGE_BUCKET_SECS: f64 = 30.0;
const HANDSHAKE_RESEND_SECS: f64 = 1.0;
const UPSTREAM_KEEPALIVE_SECS: f64 = 0.05;
const SPECTATOR_RETRANSMIT_SECS: f64 = 0.05;
const SPECTATOR_TIMEOUT_SECS: f64 = 60.0;
const MAX_DATAGRAMS_PER_STEP: usize = 2048;
const MAX_SPECTATOR_DELTA_HISTORY: usize = 64;
const MAX_INFO_STRING_LEN: usize = 256;
const MAX_UNRELIABLE_DATAGRAM: usize = 3800;
const DEFAULT_DIRECTOR_INTERVAL_SECS: f64 = 5.0;

/// Errors returned by HLTV proxy setup or persistence operations.
#[derive(Debug, thiserror::Error)]
pub enum HltvError {
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("protocol error: {0}")]
    Proto(#[from] ProtoError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid HLTV configuration: {0}")]
    InvalidConfig(String),
}

/// Configuration for an [`HltvProxy`] instance.
#[derive(Debug, Clone)]
pub struct HltvConfig {
    /// Player/proxy name sent in the upstream `connect` userinfo (`\name\...`).
    pub name: String,
    /// Public hostname advertised in `A2S_INFO` and spectator `svc_serverinfo`.
    pub hostname: String,
    /// Broadcast delay in seconds (`tv_delay`, default `30.0`).
    pub delay_secs: f64,
    /// Maximum concurrent spectators allowed on the proxy.
    pub max_spectators: u8,
    /// Optional password required in spectator `\password\...` userinfo.
    pub password: Option<String>,
    /// Upstream game server (or parent HLTV proxy) address.
    pub master_addr: Option<PeerAddr>,
    /// Optional `.dem` output file path for HLDEMO v5 recording.
    pub record_demo_path: Option<PathBuf>,
}

impl Default for HltvConfig {
    fn default() -> Self {
        Self {
            name: "HLTV Proxy".to_string(),
            hostname: "cs16-rs HLTV Proxy".to_string(),
            delay_secs: DEFAULT_TV_DELAY_SECS,
            max_spectators: 64,
            password: None,
            master_addr: None,
            record_demo_path: None,
        }
    }
}

/// Connection state of the proxy's upstream GoldSrc Protocol 48 client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamState {
    Idle,
    AwaitChallenge,
    AwaitConnection,
    Signon,
    InGame,
    Rejected(String),
    Disconnected(String),
}

/// Sign-on stage of a connected downstream spectator or child HLTV proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpectatorStage {
    AwaitNew,
    AwaitSendRes,
    AwaitSpawn,
    AwaitSendEnts,
    Active,
}

/// Monotonic counters exposed by [`HltvProxy::stats`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HltvStats {
    pub upstream_packets_in: u64,
    pub upstream_hltv_cmds: u64,
    pub downstream_hltv_status_cmds: u64,
    pub downstream_hltv_listen_cmds: u64,
    pub a2s_info_queries: u64,
    pub frames_forwarded: u64,
    pub director_cmds_sent: u64,
    pub spectators_joined: u64,
}

/// Automatic HLTV director emitting `svc_director` (`drc::CMD_*`) cues from
/// delayed game events and active player positions.
#[derive(Debug, Clone)]
pub struct AutoDirector {
    pub banner_text: String,
    pub switch_interval_secs: f64,
    pub current_target: u16,
    pub events_emitted: u64,
    next_switch_at: f64,
}

impl AutoDirector {
    pub fn new(banner_text: impl Into<String>) -> Self {
        Self {
            banner_text: banner_text.into(),
            switch_interval_secs: DEFAULT_DIRECTOR_INTERVAL_SECS,
            current_target: 0,
            events_emitted: 0,
            next_switch_at: 0.0,
        }
    }

    /// Encodes a `drc::CMD_EVENT` director cue (`ent1`, `ent2`, `DRC_FLAG_*`).
    pub fn encode_director_event(ent1: u16, ent2: u16, flags: u32) -> Vec<u8> {
        let mut w = MsgWriter::with_capacity(10);
        w.write_u8(drc::CMD_EVENT);
        w.write_u16(ent1);
        w.write_u16(ent2);
        w.write_u32(flags);
        w.into_bytes()
    }

    /// Encodes a `drc::CMD_CAMERA` director cue (`position`, `angles`, `fov`, `target`).
    pub fn encode_director_camera(
        position: Vec3,
        angles: Vec3,
        fov: u8,
        target_entity: u16,
    ) -> Vec<u8> {
        let mut w = MsgWriter::with_capacity(16);
        w.write_u8(drc::CMD_CAMERA);
        w.write_coord(position.x);
        w.write_coord(position.y);
        w.write_coord(position.z);
        w.write_coord(angles.x);
        w.write_coord(angles.y);
        w.write_coord(angles.z);
        w.write_u8(fov);
        w.write_u16(target_entity);
        w.into_bytes()
    }

    /// Encodes a `drc::CMD_MESSAGE` banner/HUD text cue (clamped to `<= 255` bytes).
    pub fn encode_director_message(
        text: &str,
        x: f32,
        y: f32,
        hold_secs: f32,
        color: [u8; 4],
    ) -> Vec<u8> {
        let mut w = MsgWriter::with_capacity(64);
        w.write_u8(drc::CMD_MESSAGE);
        w.write_u8(0); // fade effect
        w.write_bytes(&color);
        w.write_f32(x);
        w.write_f32(y);
        w.write_f32(0.5); // fadein
        w.write_f32(0.5); // fadeout
        w.write_f32(hold_secs.max(0.1));
        w.write_f32(0.0); // fxtime
        let max_text = 254usize.saturating_sub(w.len());
        let mut end = text.len().min(max_text);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        w.write_str(&text[..end]);
        w.into_bytes()
    }

    /// Encodes the welcome banner message for a newly active spectator.
    pub fn welcome_banner(&self) -> Vec<u8> {
        Self::encode_director_message(&self.banner_text, -1.0, 0.85, 6.0, [255, 180, 50, 255])
    }

    /// Inspects delayed messages and entities to produce any due `svc_director` payloads.
    pub fn observe(
        &mut self,
        now: f64,
        max_clients: u8,
        msgs: &[SvcMessage],
        entities: &[GoldSrcEntityState],
    ) -> Vec<Vec<u8>> {
        let mut cues = Vec::new();
        let mut focused_this_tick = false;

        for msg in msgs {
            if let SvcMessage::UserMsg { decoded, .. } = msg {
                match decoded {
                    CsUserMsg::DeathMsg {
                        killer_id,
                        victim_id,
                        headshot,
                        ..
                    } => {
                        let flags = if *headshot { 0x40 | 0x100 } else { 0x40 };
                        cues.push(Self::encode_director_event(
                            u16::from(*killer_id),
                            u16::from(*victim_id),
                            flags,
                        ));
                        let target = if *killer_id != 0 {
                            u16::from(*killer_id)
                        } else {
                            u16::from(*victim_id)
                        };
                        if target != 0
                            && let Some(ent) = entities.iter().find(|e| e.number == target)
                        {
                            self.current_target = target;
                            cues.push(Self::encode_director_camera(
                                ent.origin + Vec3::new(0.0, 0.0, 48.0),
                                ent.angles,
                                90,
                                target,
                            ));
                            focused_this_tick = true;
                            self.next_switch_at = now + self.switch_interval_secs;
                        }
                    }
                    CsUserMsg::BombDrop { origin, .. } => {
                        cues.push(Self::encode_director_event(0, 0, 0x40));
                        cues.push(Self::encode_director_camera(
                            *origin + Vec3::new(0.0, 0.0, 64.0),
                            Vec3::new(25.0, 0.0, 0.0),
                            90,
                            0,
                        ));
                        focused_this_tick = true;
                        self.next_switch_at = now + self.switch_interval_secs;
                    }
                    _ => {}
                }
            }
        }

        if !focused_this_tick && (self.current_target == 0 || now >= self.next_switch_at) {
            let players: Vec<&GoldSrcEntityState> = entities
                .iter()
                .filter(|e| (1..=u16::from(max_clients)).contains(&e.number) && e.modelindex != 0)
                .collect();
            if !players.is_empty() {
                let next = players
                    .iter()
                    .find(|e| e.number > self.current_target)
                    .copied()
                    .unwrap_or(players[0]);
                self.current_target = next.number;
                self.next_switch_at = now + self.switch_interval_secs;
                cues.push(Self::encode_director_camera(
                    next.origin + Vec3::new(0.0, 0.0, 48.0),
                    next.angles,
                    90,
                    next.number,
                ));
            }
        }

        self.events_emitted = self.events_emitted.saturating_add(cues.len() as u64);
        cues
    }
}

type DeltaFrameEntities = BTreeMap<u16, (u8, DeltaRecord)>;

/// Connected downstream spectator or child HLTV proxy slot.
#[derive(Debug)]
pub struct SpectatorSlot {
    pub slot: u8,
    pub userid: i32,
    pub addr: PeerAddr,
    pub name: String,
    pub is_hltv: bool,
    pub listening: bool,
    pub stage: SpectatorStage,
    pub connected_at: f64,
    pub last_received: f64,
    chan: NetChan,
    delta_seq: Option<u8>,
    sent_frames: BTreeMap<u8, (u32, DeltaFrameEntities)>,
    pending_new: bool,
    next_retransmit_at: f64,
}

impl SpectatorSlot {
    fn new(slot: u8, userid: i32, addr: PeerAddr, name: String, is_hltv: bool, now: f64) -> Self {
        Self {
            slot,
            userid,
            addr,
            name,
            is_hltv,
            listening: true,
            stage: SpectatorStage::AwaitNew,
            connected_at: now,
            last_received: now,
            chan: NetChan::new(FRAGMENT_S2C_MAX_SIZE),
            delta_seq: None,
            sent_frames: BTreeMap::new(),
            pending_new: false,
            next_retransmit_at: now,
        }
    }

    pub fn outgoing_sequence(&self) -> u32 {
        self.chan.outgoing_sequence
    }

    pub fn incoming_sequence(&self) -> u32 {
        self.chan.incoming_sequence
    }

    fn record_sent_entities(
        &mut self,
        seq: u32,
        entities: DeltaFrameEntities,
    ) {
        let key = (seq & 0xFF) as u8;
        self.sent_frames.insert(key, (seq, entities));
        while self.sent_frames.len() > MAX_SPECTATOR_DELTA_HISTORY {
            if let Some(oldest_key) = self
                .sent_frames
                .iter()
                .min_by_key(|(_, (s, _))| *s)
                .map(|(k, _)| *k)
            {
                self.sent_frames.remove(&oldest_key);
            } else {
                break;
            }
        }
    }

    fn delta_base(&self, current_seq: u32) -> Option<(u8, &DeltaFrameEntities)> {
        let seq_byte = self.delta_seq?;
        let (full_seq, map) = self.sent_frames.get(&seq_byte)?;
        if *full_seq <= current_seq && current_seq - *full_seq < 63 {
            Some((seq_byte, map))
        } else {
            None
        }
    }
}

/// Keyed stateless challenge generator matching GoldSrc's 30-second window.
#[derive(Debug)]
struct ChallengeIssuer {
    key: RandomState,
}

impl Default for ChallengeIssuer {
    fn default() -> Self {
        Self {
            key: RandomState::new(),
        }
    }
}

impl ChallengeIssuer {
    fn issue(&self, addr: &PeerAddr, now: f64) -> u32 {
        self.for_window(addr, (now / CHALLENGE_BUCKET_SECS).floor() as i64)
    }

    fn check(&self, addr: &PeerAddr, challenge: u32, now: f64) -> bool {
        let w = (now / CHALLENGE_BUCKET_SECS).floor() as i64;
        challenge == self.for_window(addr, w)
            || challenge == self.for_window(addr, w.wrapping_sub(1))
    }

    fn for_window(&self, addr: &PeerAddr, window: i64) -> u32 {
        match self.key.hash_one((addr, window)) as u32 & 0x7FFF_FFFF {
            0 => 1,
            c => c,
        }
    }
}

struct UpstreamClient {
    master: Option<PeerAddr>,
    state: UpstreamState,
    challenge: u32,
    handshake_sent_at: Option<f64>,
    next_keepalive_at: f64,
    chan: Option<NetChan>,
    splits: SplitPacketAssembler,
    ctx: SvcParseContext,
    server_info: Option<ServerInfoMsg>,
    last_entity_seq: Option<u8>,
    steam_id: u64,
    cdkey_md5_hex: String,
    hltv_announced: bool,
}

impl UpstreamClient {
    fn new(master: Option<PeerAddr>, steam_id: u64, cdkey_md5_hex: String) -> Self {
        let mut ctx = SvcParseContext::new();
        ctx.usermsgs = UserMsgRegistry::cs16_default();
        let state = if master.is_some() {
            UpstreamState::AwaitChallenge
        } else {
            UpstreamState::Idle
        };
        Self {
            master,
            state,
            challenge: 0,
            handshake_sent_at: None,
            next_keepalive_at: 0.0,
            chan: None,
            splits: SplitPacketAssembler::default(),
            ctx,
            server_info: None,
            last_entity_seq: None,
            steam_id,
            cdkey_md5_hex,
            hltv_announced: false,
        }
    }

    fn demo_sequences(&self) -> [i32; 7] {
        match &self.chan {
            Some(c) => [
                c.incoming_sequence as i32,
                c.incoming_acknowledged as i32,
                c.incoming_reliable_acknowledged as i32,
                c.incoming_reliable_sequence as i32,
                c.outgoing_sequence as i32,
                c.reliable_sequence as i32,
                c.last_reliable_sequence as i32,
            ],
            None => [0; 7],
        }
    }
}

/// Step-driven HLTV proxy bridging an upstream GoldSrc server and downstream
/// spectators over generic [`PacketTransport`]s.
pub struct HltvProxy<U: PacketTransport, D: PacketTransport> {
    config: HltvConfig,
    cvars: CVarRegistry,
    upstream: U,
    downstream: D,
    upstream_client: UpstreamClient,
    delay: DelayBuffer,
    director: AutoDirector,
    recorder: Option<DemoRecorder>,
    spectators: Vec<SpectatorSlot>,
    spectator_by_addr: HashMap<PeerAddr, usize>,
    next_userid: i32,
    challenges: ChallengeIssuer,
    usercmd_schema: DeltaSchema,
    require_a2s_challenge: bool,
    last_upstream_spawn_count: Option<u32>,
    stats: HltvStats,
}

impl<U: PacketTransport, D: PacketTransport> HltvProxy<U, D> {
    /// Creates a new [`HltvProxy`] over generic upstream and downstream transports.
    ///
    /// If `config.master_addr` is `Some`, the upstream client starts handshaking
    /// on the first [`step`](Self::step). If `config.record_demo_path` is `Some`,
    /// `.dem` recording starts immediately.
    pub fn new(config: HltvConfig, upstream: U, downstream: D) -> Self {
        let delay = DelayBuffer::new(config.delay_secs);
        let banner = format!("HLTV: {}", config.hostname);
        let upstream_client = UpstreamClient::new(
            config.master_addr,
            76_561_197_960_265_900,
            md5_hex(config.name.as_bytes()),
        );
        let usercmd_schema = DeltaRegistry::cs16_default()
            .get("usercmd_t")
            .cloned()
            .expect("built-in usercmd_t schema");

        let mut cvars = CVarRegistry::new_empty();
        cvars.register(
            "tv_delay",
            &format!("{:.1}", delay.delay_secs()),
            CVarFlags::SERVER,
            "HLTV broadcast delay in seconds",
        );
        cvars.register(
            "tv_name",
            &config.name,
            CVarFlags::SERVER,
            "HLTV proxy client name",
        );
        cvars.register(
            "hostname",
            &config.hostname,
            CVarFlags::SERVER,
            "HLTV proxy hostname",
        );
        cvars.register(
            "tv_maxclients",
            &config.max_spectators.to_string(),
            CVarFlags::SERVER,
            "Maximum connected HLTV spectators",
        );
        cvars.register(
            "tv_password",
            config.password.as_deref().unwrap_or(""),
            CVarFlags::PROTECTED,
            "Spectator password for HLTV proxy",
        );

        let recorder = config
            .record_demo_path
            .as_ref()
            .map(|_| DemoRecorder::new("", "cstrike", 0, PROTOCOL_VERSION as i32));

        Self {
            config,
            cvars,
            upstream,
            downstream,
            upstream_client,
            delay,
            director: AutoDirector::new(banner),
            recorder,
            spectators: Vec::new(),
            spectator_by_addr: HashMap::new(),
            next_userid: 1,
            challenges: ChallengeIssuer::default(),
            usercmd_schema,
            require_a2s_challenge: false,
            last_upstream_spawn_count: None,
            stats: HltvStats::default(),
        }
    }

    /// Initiates or redirects the upstream connection to `master_addr`.
    pub fn connect_master(&mut self, master_addr: PeerAddr) {
        self.config.master_addr = Some(master_addr);
        self.upstream_client = UpstreamClient::new(
            Some(master_addr),
            self.upstream_client.steam_id,
            self.upstream_client.cdkey_md5_hex.clone(),
        );
    }

    pub fn set_banner(&mut self, text: impl Into<String>) {
        self.director.banner_text = text.into();
    }

    pub fn set_director_interval(&mut self, secs: f64) {
        if secs.is_finite() && secs > 0.0 {
            self.director.switch_interval_secs = secs;
        }
    }

    pub fn set_require_a2s_challenge(&mut self, required: bool) {
        self.require_a2s_challenge = required;
    }

    pub fn set_steam_id(&mut self, steam_id: u64) {
        self.upstream_client.steam_id = steam_id;
    }

    pub fn cvars(&self) -> &CVarRegistry {
        &self.cvars
    }

    /// Updates an HLTV cvar (`tv_delay`, `hostname`, `tv_name`, `tv_maxclients`,
    /// `tv_password`) and synchronizes proxy state.
    pub fn set_cvar(&mut self, name: &str, value: &str) -> bool {
        if !self.cvars.set(name, value) {
            return false;
        }
        match name.to_ascii_lowercase().as_str() {
            "tv_delay" => {
                if let Ok(secs) = value.trim().parse::<f64>() {
                    self.delay.set_delay_secs(secs);
                    self.config.delay_secs = self.delay.delay_secs();
                }
            }
            "hostname" => {
                self.config.hostname = value.to_string();
                self.director.banner_text = format!("HLTV: {}", self.config.hostname);
            }
            "tv_name" => {
                self.config.name = value.to_string();
            }
            "tv_maxclients" => {
                if let Ok(max) = value.trim().parse::<u8>() {
                    self.config.max_spectators = max;
                }
            }
            "tv_password" => {
                self.config.password = (!value.is_empty()).then(|| value.to_string());
            }
            _ => {}
        }
        true
    }

    pub fn upstream_state(&self) -> &UpstreamState {
        &self.upstream_client.state
    }

    pub fn delay_buffer(&self) -> &DelayBuffer {
        &self.delay
    }

    pub fn delay_buffer_mut(&mut self) -> &mut DelayBuffer {
        &mut self.delay
    }

    pub fn director(&self) -> &AutoDirector {
        &self.director
    }

    pub fn spectators(&self) -> &[SpectatorSlot] {
        &self.spectators
    }

    pub fn spectator_count(&self) -> usize {
        self.spectators.len()
    }

    pub fn stats(&self) -> &HltvStats {
        &self.stats
    }

    pub fn upstream_transport(&self) -> &U {
        &self.upstream
    }

    pub fn upstream_transport_mut(&mut self) -> &mut U {
        &mut self.upstream
    }

    pub fn downstream_transport(&self) -> &D {
        &self.downstream
    }

    pub fn downstream_transport_mut(&mut self) -> &mut D {
        &mut self.downstream
    }

    /// Enables live `.dem` recording into `cs_clientcore::DemoRecorder`.
    pub fn start_recording(&mut self) {
        let snap = self.delay.latest_captured_snapshot();
        let (map, dir, crc) = match &snap.server_info {
            Some(info) => (info.map_path.as_str(), info.game_dir.as_str(), info.map_crc),
            None => ("", "cstrike", 0),
        };
        let mut rec = DemoRecorder::new(map, dir, crc, PROTOCOL_VERSION as i32);
        if self.upstream_client.state == UpstreamState::InGame {
            rec.mark_signon_complete(0.0);
        }
        self.recorder = Some(rec);
    }

    /// Finalizes any active `.dem` recording and returns valid `HLDEMO` v5 bytes.
    pub fn finish_recording(&mut self) -> Result<Option<Vec<u8>>, ProtoError> {
        let Some(rec) = self.recorder.take() else {
            return Ok(None);
        };
        rec.finish().map(Some)
    }

    /// Finalizes any active `.dem` recording and writes it to `config.record_demo_path`.
    pub fn save_recording(&mut self) -> Result<Option<PathBuf>, HltvError> {
        let Some(path) = self.config.record_demo_path.clone() else {
            return Ok(None);
        };
        let Some(bytes) = self.finish_recording()? else {
            return Ok(None);
        };
        std::fs::write(&path, bytes)?;
        Ok(Some(path))
    }

    /// Builds the `A2S_INFO` payload advertising this proxy (`server_type = b'p'`).
    pub fn query_info(&self) -> ServerQueryInfo {
        let snap = self.delay.delayed_snapshot();
        let info = snap
            .server_info
            .as_ref()
            .or(self.delay.latest_captured_snapshot().server_info.as_ref());
        let (map, folder) = match info {
            Some(si) => (clean_map_name(&si.map_path), si.game_dir.clone()),
            None => ("lobby".to_string(), "cstrike".to_string()),
        };
        ServerQueryInfo {
            addr: String::new(),
            protocol: PROTOCOL_VERSION as u8,
            name: self.config.hostname.clone(),
            map,
            folder,
            game: "Counter-Strike".to_string(),
            app_id: 10,
            players: self.spectators.len().min(255) as u8,
            max_players: self.config.max_spectators,
            bots: 0,
            server_type: b'p',
            environment: if cfg!(windows) { b'w' } else { b'l' },
            password_protected: self.config.password.is_some(),
            vac_secure: false,
            version: "1.1.2.7/Stdio".to_string(),
            ping_ms: 1,
        }
    }

    /// Directly injects a raw `svc_*` frame into the delay buffer and optional
    /// `.dem` recorder (useful for deterministic unit tests or relay bridges).
    pub fn feed_upstream_svc(&mut self, now: f64, is_signon: bool, raw_svc: &[u8]) {
        self.ingest_svc_stream(now, is_signon, raw_svc, None);
    }

    /// Advances the proxy by one deterministic tick at timestamp `now`.
    pub fn step(&mut self, now: f64) {
        if !now.is_finite() {
            return;
        }
        self.pump_upstream(now);
        self.pump_downstream(now);
        self.flush_pending_new_spectators(now);
        self.dispatch_delayed_frames(now);
        self.pump_spectator_transmits(now);
        self.evict_timed_out_spectators(now);
    }

    /// Disconnects all spectators and upstream cleanly.
    pub fn shutdown(&mut self, reason: &str) {
        let mut msg = MsgWriter::new();
        write_svc_disconnect(&mut msg, reason);
        let bytes = msg.into_bytes();
        for spec in &mut self.spectators {
            spec.chan.write_reliable(&bytes);
            let pkt = spec.chan.build_packet(&[]);
            let _ = self.downstream.send(spec.addr, &pkt);
        }
        self.spectators.clear();
        self.spectator_by_addr.clear();

        if let (Some(master), Some(chan)) =
            (self.upstream_client.master, self.upstream_client.chan.as_mut())
        {
            let mut cw = MsgWriter::new();
            write_clc_stringcmd(&mut cw, "disconnect");
            chan.write_reliable(cw.as_slice());
            let pkt = chan.build_packet(&[]);
            let _ = self.upstream.send(master, &pkt);
        }
        self.upstream_client.state = UpstreamState::Disconnected(reason.to_string());
    }

    // ---- upstream client --------------------------------------------------

    fn pump_upstream(&mut self, now: f64) {
        for _ in 0..MAX_DATAGRAMS_PER_STEP {
            let Some((from, data)) = self.upstream.recv() else {
                break;
            };
            if self.upstream_client.master != Some(from) {
                continue;
            }
            self.stats.upstream_packets_in += 1;
            self.handle_upstream_datagram(now, &data);
        }

        match self.upstream_client.state {
            UpstreamState::AwaitChallenge | UpstreamState::AwaitConnection => {
                if self
                    .upstream_client
                    .handshake_sent_at
                    .is_none_or(|t| now - t >= HANDSHAKE_RESEND_SECS)
                    && let Some(master) = self.upstream_client.master
                {
                    let pkt = if self.upstream_client.state == UpstreamState::AwaitChallenge {
                        encode_getchallenge(true)
                    } else {
                        let prot = default_cs16_protinfo(&self.upstream_client.cdkey_md5_hex);
                        let mut user = default_cs16_userinfo(&self.config.name);
                        user.set("hltv", "1");
                        user.set("rate", "100000");
                        user.set("cl_updaterate", "101");
                        encode_connect_48(
                            self.upstream_client.challenge,
                            &prot,
                            &user,
                            &lan_steam_auth_ticket(self.upstream_client.steam_id),
                        )
                    };
                    let _ = self.upstream.send(master, &pkt);
                    self.upstream_client.handshake_sent_at = Some(now);
                }
            }
            UpstreamState::Signon | UpstreamState::InGame => {
                let has_reliable = self
                    .upstream_client
                    .chan
                    .as_ref()
                    .is_some_and(NetChan::has_pending_reliable);
                if has_reliable || now >= self.upstream_client.next_keepalive_at {
                    self.send_upstream_netchan(now);
                    self.upstream_client.next_keepalive_at = now + UPSTREAM_KEEPALIVE_SECS;
                }
            }
            UpstreamState::Idle
            | UpstreamState::Rejected(_)
            | UpstreamState::Disconnected(_) => {}
        }
    }

    fn handle_upstream_datagram(&mut self, now: f64, data: &[u8]) {
        let Some(datagram) = self.upstream_client.splits.push(data) else {
            return;
        };
        if datagram.starts_with(&CONNECTIONLESS_HEADER.to_le_bytes()) {
            self.handle_upstream_oob(&datagram);
            return;
        }
        let Some(chan) = self.upstream_client.chan.as_mut() else {
            return;
        };
        let Ok(Some(incoming)) = chan.process_packet(&datagram) else {
            return;
        };
        let seq_u8 = (incoming.sequence & 0xFF) as u8;
        let is_signon = self.upstream_client.state != UpstreamState::InGame;
        for stream in std::iter::once(incoming.payload).chain(incoming.completed_normal_stream) {
            self.ingest_svc_stream(now, is_signon, &stream, Some(seq_u8));
        }
    }

    fn handle_upstream_oob(&mut self, datagram: &[u8]) {
        match parse_oob_packet(datagram) {
            Ok(OobPacket::S2cChallenge { challenge, .. })
                if self.upstream_client.state == UpstreamState::AwaitChallenge =>
            {
                self.upstream_client.challenge = challenge;
                self.upstream_client.state = UpstreamState::AwaitConnection;
                self.upstream_client.handshake_sent_at = None;
            }
            Ok(OobPacket::S2cConnection { .. })
                if self.upstream_client.state == UpstreamState::AwaitConnection =>
            {
                self.upstream_client.state = UpstreamState::Signon;
                self.upstream_client.chan = Some(NetChan::new(FRAGMENT_C2S_SIZE));
                self.upstream_client.next_keepalive_at = 0.0;
                self.upstream_queue_stringcmd("new");
            }
            Ok(OobPacket::S2cReject(reason))
                if matches!(
                    self.upstream_client.state,
                    UpstreamState::AwaitChallenge | UpstreamState::AwaitConnection
                ) =>
            {
                self.upstream_client.state =
                    UpstreamState::Rejected(reason.trim_end_matches('\n').to_string());
            }
            _ => {}
        }
    }

    fn ingest_svc_stream(
        &mut self,
        now: f64,
        is_signon: bool,
        raw_svc: &[u8],
        seq_u8: Option<u8>,
    ) {
        if raw_svc.is_empty() {
            return;
        }
        if let Some(seq) = seq_u8 {
            self.upstream_client.ctx.last_incoming_seq = seq;
        } else {
            self.upstream_client.ctx.last_incoming_seq =
                self.upstream_client.ctx.last_incoming_seq.wrapping_add(1);
        }

        if let Some(rec) = self.recorder.as_mut() {
            let seqs = self.upstream_client.demo_sequences();
            rec.record_svc_packet(now, is_signon, raw_svc, DemoNetMsgInfo::default(), seqs);
        }

        self.delay.push_frame_with_seq(now, is_signon, raw_svc, seq_u8);

        let (msgs, _) = parse_svc_stream_partial(raw_svc, &mut self.upstream_client.ctx);
        for msg in msgs {
            self.handle_upstream_svc(now, msg, seq_u8);
        }
    }

    fn handle_upstream_svc(&mut self, now: f64, msg: SvcMessage, seq_u8: Option<u8>) {
        let signon = self.upstream_client.state == UpstreamState::Signon;
        match msg {
            SvcMessage::Disconnect(reason) => {
                let clean = reason.trim_end_matches('\n').to_string();
                warn!(reason = %clean, "upstream disconnected");
                self.upstream_client.state = UpstreamState::Disconnected(clean);
            }
            SvcMessage::ServerInfo(info) => {
                if let Some(rec) = self.recorder.as_mut() {
                    rec.set_map_info(&info.map_path, &info.game_dir, info.map_crc);
                }
                self.upstream_client.server_info = Some(*info);
                if signon {
                    self.upstream_queue_stringcmd("sendres");
                }
            }
            SvcMessage::ResourceRequest { .. } if signon => {
                let mut w = MsgWriter::new();
                write_clc_resourcelist(&mut w, &[]);
                self.upstream_queue_reliable(w.as_slice());
            }
            SvcMessage::ResourceList {
                consistency_indices,
                ..
            } if signon => {
                if !consistency_indices.is_empty() {
                    let mut w = MsgWriter::new();
                    write_clc_fileconsistency(&mut w, &[]);
                    self.upstream_queue_reliable(w.as_slice());
                }
                if let Some(info) = &self.upstream_client.server_info {
                    let spawn_count = info.spawn_count;
                    let mut crc = info.map_crc.to_le_bytes();
                    com_munge2(&mut crc, (-1i32).wrapping_sub(spawn_count as i32) & 0xFF);
                    let crc = i32::from_le_bytes(crc);
                    self.upstream_queue_stringcmd(&format!("spawn {spawn_count} {crc}"));
                }
            }
            SvcMessage::SignonNum(1) if signon => {
                self.upstream_queue_stringcmd("sendents");
                if !self.upstream_client.hltv_announced {
                    self.upstream_client.hltv_announced = true;
                    let mut w = MsgWriter::new();
                    write_clc_hltv(&mut w, drc::HLTV_ACTIVE);
                    self.upstream_queue_reliable(w.as_slice());
                }
            }
            SvcMessage::PacketEntities(_) => {
                self.upstream_client.last_entity_seq =
                    seq_u8.or(Some(self.upstream_client.ctx.last_incoming_seq));
                if signon {
                    info!(
                        map = ?self.upstream_client.server_info.as_ref().map(|i| i.map_path.as_str()),
                        "upstream HLTV client in-game"
                    );
                    self.upstream_client.state = UpstreamState::InGame;
                    if let Some(rec) = self.recorder.as_mut() {
                        rec.mark_signon_complete(now);
                    }
                }
            }
            SvcMessage::Hltv(subcmd) => {
                self.stats.upstream_hltv_cmds += 1;
                let reply = match subcmd {
                    drc::HLTV_ACTIVE => Some(drc::HLTV_LISTEN),
                    drc::HLTV_STATUS => Some(drc::HLTV_STATUS),
                    drc::HLTV_LISTEN => Some(drc::HLTV_LISTEN),
                    _ => None,
                };
                if let Some(cmd) = reply {
                    let mut w = MsgWriter::new();
                    write_clc_hltv(&mut w, cmd);
                    self.upstream_queue_reliable(w.as_slice());
                }
            }
            SvcMessage::SendCvarValue(_) => {
                let mut w = MsgWriter::new();
                write_clc_cvarvalue(&mut w, "");
                self.upstream_queue_reliable(w.as_slice());
            }
            SvcMessage::SendCvarValue2 {
                request_id,
                cvar_name,
            } => {
                let mut w = MsgWriter::new();
                write_clc_cvarvalue2(&mut w, request_id, &cvar_name, "");
                self.upstream_queue_reliable(w.as_slice());
            }
            SvcMessage::StuffText(text) => {
                for line in text.split(['\n', ';']) {
                    if line.trim().eq_ignore_ascii_case("reconnect") {
                        self.upstream_client.state = UpstreamState::Signon;
                        self.upstream_client.last_entity_seq = None;
                        self.upstream_queue_stringcmd("new");
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    fn upstream_queue_stringcmd(&mut self, cmd: &str) {
        let mut w = MsgWriter::new();
        write_clc_stringcmd(&mut w, cmd);
        self.upstream_queue_reliable(w.as_slice());
    }

    fn upstream_queue_reliable(&mut self, bytes: &[u8]) {
        if let Some(chan) = self.upstream_client.chan.as_mut() {
            chan.write_reliable(bytes);
        }
    }

    fn send_upstream_netchan(&mut self, _now: f64) {
        let (Some(master), Some(chan)) =
            (self.upstream_client.master, self.upstream_client.chan.as_mut())
        else {
            return;
        };
        let mut unreliable = MsgWriter::new();
        if let Some(seq) = self.upstream_client.last_entity_seq {
            write_clc_delta(&mut unreliable, seq);
        } else {
            write_clc_nop(&mut unreliable);
        }
        let pkt = chan.build_packet(unreliable.as_slice());
        let _ = self.upstream.send(master, &pkt);
    }

    // ---- downstream server ------------------------------------------------

    fn pump_downstream(&mut self, now: f64) {
        for _ in 0..MAX_DATAGRAMS_PER_STEP {
            let Some((from, data)) = self.downstream.recv() else {
                break;
            };
            if data.starts_with(&CONNECTIONLESS_HEADER.to_le_bytes()) {
                self.handle_downstream_oob(from, &data, now);
            } else if let Some(&idx) = self.spectator_by_addr.get(&from) {
                self.handle_spectator_netchan(idx, &data, now);
            }
        }
    }

    fn handle_downstream_oob(&mut self, from: PeerAddr, data: &[u8], now: f64) {
        let Ok(packet) = parse_oob_packet(data) else {
            return;
        };
        let reply = match packet {
            OobPacket::A2aPing => Some(encode_a2a_ack()),
            OobPacket::GetChallenge { .. } => {
                let ch = self.challenges.issue(&from, now);
                Some(encode_s2c_challenge(ch, 0, false))
            }
            OobPacket::A2sInfo { challenge } => {
                self.stats.a2s_info_queries += 1;
                if !self.require_a2s_challenge
                    || challenge.is_some_and(|c| self.challenges.check(&from, c as u32, now))
                {
                    Some(encode_s2a_info_src(&self.query_info()))
                } else {
                    let ch = self.challenges.issue(&from, now) as i32;
                    Some(encode_s2c_query_challenge(ch))
                }
            }
            OobPacket::A2sPlayer { challenge } => {
                if !self.require_a2s_challenge
                    || self.challenges.check(&from, challenge as u32, now)
                {
                    let rows: Vec<PlayerQueryEntry> = self
                        .spectators
                        .iter()
                        .map(|s| PlayerQueryEntry {
                            index: s.slot,
                            name: s.name.clone(),
                            score: 0,
                            duration_secs: (now - s.connected_at).max(0.0) as f32,
                        })
                        .collect();
                    Some(encode_s2a_players(&rows))
                } else {
                    let ch = self.challenges.issue(&from, now) as i32;
                    Some(encode_s2c_query_challenge(ch))
                }
            }
            OobPacket::A2sRules { challenge } => {
                if !self.require_a2s_challenge
                    || self.challenges.check(&from, challenge as u32, now)
                {
                    let delay_str = format!("{:.0}", self.delay.delay_secs());
                    Some(encode_s2a_rules(&[
                        ("tv_delay", delay_str.as_str()),
                        ("tv_name", self.config.name.as_str()),
                    ]))
                } else {
                    let ch = self.challenges.issue(&from, now) as i32;
                    Some(encode_s2c_query_challenge(ch))
                }
            }
            OobPacket::Connect {
                protocol,
                challenge,
                userinfo,
                userinfo_raw,
                ..
            } => Some(self.handle_spectator_connect(
                from,
                protocol,
                challenge,
                &userinfo,
                &userinfo_raw,
                now,
            )),
            _ => None,
        };
        if let Some(bytes) = reply {
            let _ = self.downstream.send(from, &bytes);
        }
    }

    fn handle_spectator_connect(
        &mut self,
        from: PeerAddr,
        protocol: u32,
        challenge: u32,
        userinfo: &InfoString,
        userinfo_raw: &str,
        now: f64,
    ) -> Vec<u8> {
        if !self.challenges.check(&from, challenge, now) {
            return encode_s2c_reject("Bad challenge.\n");
        }
        if protocol != PROTOCOL_VERSION {
            return encode_s2c_reject(&format!(
                "This HLTV proxy uses protocol {PROTOCOL_VERSION}, client sent {protocol}.\n"
            ));
        }
        if !userinfo_raw.starts_with('\\')
            || !InfoString::is_valid(userinfo_raw)
            || userinfo_raw.len() > MAX_INFO_STRING_LEN
        {
            return encode_s2c_reject("Invalid userinfo in connect command\n");
        }
        if let Some(expected_pw) = &self.config.password
            && userinfo.get("password") != Some(expected_pw.as_str())
        {
            return encode_s2c_reject("Bad password.\n");
        }

        if let Some(&existing_idx) = self.spectator_by_addr.get(&from) {
            self.remove_spectator_at(existing_idx);
        }
        if self.spectators.len() >= usize::from(self.config.max_spectators) {
            return encode_s2c_reject("HLTV proxy is full.\n");
        }

        let name = userinfo
            .get("name")
            .filter(|s| !s.is_empty())
            .unwrap_or("Spectator")
            .to_string();
        let is_hltv = userinfo
            .get("hltv")
            .is_some_and(|v| !v.is_empty() && v != "0");
        let slot_num = self.next_free_spectator_slot();
        let userid = self.next_userid;
        self.next_userid = self.next_userid.wrapping_add(1);

        let slot = SpectatorSlot::new(slot_num, userid, from, name.clone(), is_hltv, now);
        let idx = self.spectators.len();
        self.spectators.push(slot);
        self.spectator_by_addr.insert(from, idx);
        self.stats.spectators_joined += 1;
        info!(slot = slot_num, userid, %from, name = %name, is_hltv, "spectator connected");

        encode_s2c_connection(userid, &from.to_string(), false, 8684)
    }

    fn next_free_spectator_slot(&self) -> u8 {
        for candidate in 0..=u8::MAX {
            if !self.spectators.iter().any(|s| s.slot == candidate) {
                return candidate;
            }
        }
        0
    }

    fn remove_spectator_at(&mut self, idx: usize) {
        if idx >= self.spectators.len() {
            return;
        }
        let removed = self.spectators.swap_remove(idx);
        self.spectator_by_addr.remove(&removed.addr);
        if let Some(moved) = self.spectators.get(idx) {
            self.spectator_by_addr.insert(moved.addr, idx);
        }
    }

    fn handle_spectator_netchan(&mut self, idx: usize, data: &[u8], now: f64) {
        let Ok(Some(incoming)) = self.spectators[idx].chan.process_packet(data) else {
            return;
        };
        self.spectators[idx].last_received = now;
        self.spectators[idx].delta_seq = None;

        let mut disconnect = false;
        let payloads = std::iter::once(incoming.payload).chain(incoming.completed_normal_stream);
        for payload in payloads {
            let Ok(msgs) = parse_clc_stream(&payload, &self.usercmd_schema, incoming.sequence)
            else {
                continue;
            };
            for msg in msgs {
                match msg {
                    ClcMessage::Delta(seq) => {
                        self.spectators[idx].delta_seq = Some(seq);
                    }
                    ClcMessage::Hltv(subcmd) => {
                        self.handle_spectator_hltv(idx, subcmd);
                    }
                    ClcMessage::StringCmd(line) => {
                        if self.handle_spectator_stringcmd(idx, &line, now) {
                            disconnect = true;
                            break;
                        }
                    }
                    ClcMessage::Move { .. } => {}
                    _ => {}
                }
            }
        }

        if disconnect {
            self.remove_spectator_at(idx);
        } else {
            // Answer signon / ack immediately on the same step for low latency.
            self.send_spectator_reliable_if_due(idx, now);
        }
    }

    fn handle_spectator_hltv(&mut self, idx: usize, subcmd: u8) {
        let num_spectators = self.spectators.len();
        let delay_secs = self.delay.delay_secs();
        let spec = &mut self.spectators[idx];
        match subcmd {
            drc::HLTV_ACTIVE => {
                spec.is_hltv = true;
            }
            drc::HLTV_STATUS => {
                self.stats.downstream_hltv_status_cmds += 1;
                let mut w = MsgWriter::new();
                write_svc_hltv(&mut w, drc::HLTV_ACTIVE);
                write_svc_print(
                    &mut w,
                    &format!(
                        "HLTV Status: {num_spectators} spectators, delay {delay_secs:.1}s\n"
                    ),
                );
                spec.chan.write_reliable(w.as_slice());
            }
            drc::HLTV_LISTEN => {
                self.stats.downstream_hltv_listen_cmds += 1;
                spec.listening = true;
                let mut w = MsgWriter::new();
                write_svc_hltv(&mut w, drc::HLTV_ACTIVE);
                spec.chan.write_reliable(w.as_slice());
            }
            _ => {}
        }
    }

    /// Returns `true` if the spectator requested disconnection.
    fn handle_spectator_stringcmd(&mut self, idx: usize, line: &str, now: f64) -> bool {
        let argv = tokenize_goldsrc_line(line);
        let Some(cmd) = argv.first().map(String::as_str) else {
            return false;
        };
        match cmd {
            "disconnect" | "dropclient" => return true,
            "new" => {
                let spec = &mut self.spectators[idx];
                spec.stage = SpectatorStage::AwaitNew;
                spec.sent_frames.clear();
                spec.delta_seq = None;
                let snap = self.delay.snapshot_at(now);
                if let Some(bytes) =
                    snap.encode_new_stage(Some(spec.slot), Some(&self.config.hostname))
                {
                    spec.chan.write_reliable(&bytes);
                    spec.stage = SpectatorStage::AwaitSendRes;
                    spec.pending_new = false;
                } else {
                    spec.pending_new = true;
                }
            }
            "sendres" if self.spectators[idx].stage == SpectatorStage::AwaitSendRes => {
                let snap = self.delay.snapshot_at(now);
                if let Some(bytes) = snap.encode_sendres_stage() {
                    let spec = &mut self.spectators[idx];
                    spec.chan.write_reliable(&bytes);
                    spec.stage = SpectatorStage::AwaitSpawn;
                }
            }
            "spawn" if self.spectators[idx].stage == SpectatorStage::AwaitSpawn => {
                let snap = self.delay.snapshot_at(now);
                if let Some(bytes) = snap.encode_spawn_stage(true) {
                    let spec = &mut self.spectators[idx];
                    spec.chan.write_reliable(&bytes);
                    spec.stage = SpectatorStage::AwaitSendEnts;
                }
            }
            "sendents" if self.spectators[idx].stage == SpectatorStage::AwaitSendEnts => {
                let snap = self.delay.snapshot_at(now);
                let banner = self.director.welcome_banner();
                let mut w = MsgWriter::new();
                write_svc_hltv(&mut w, drc::HLTV_ACTIVE);
                write_svc_director(&mut w, &banner);
                if self.director.current_target != 0
                    && let Some(ent) = snap
                        .latest_entities
                        .iter()
                        .find(|e| e.number == self.director.current_target)
                {
                    let cam = AutoDirector::encode_director_camera(
                        ent.origin + Vec3::new(0.0, 0.0, 48.0),
                        ent.angles,
                        90,
                        ent.number,
                    );
                    write_svc_director(&mut w, &cam);
                    self.stats.director_cmds_sent += 1;
                }
                self.stats.director_cmds_sent += 1;

                let spec = &mut self.spectators[idx];
                spec.stage = SpectatorStage::Active;
                spec.chan.write_reliable(w.as_slice());

                // Send an immediate full entity snapshot so the spectator enters InGame right away.
                if let Some(ent_bytes) = snap.encode_entities_full() {
                    let budget = MAX_UNRELIABLE_DATAGRAM
                        .saturating_sub(spec.chan.next_packet_reliable_len());
                    if ent_bytes.len() <= budget {
                        let seq = spec.chan.outgoing_sequence;
                        let ent_map = entity_record_map(&snap, &snap.latest_entities);
                        spec.record_sent_entities(seq, ent_map);
                        let pkt = spec.chan.build_packet(&ent_bytes);
                        let _ = self.downstream.send(spec.addr, &pkt);
                    } else {
                        let pkt = spec.chan.build_packet(&[]);
                        let _ = self.downstream.send(spec.addr, &pkt);
                    }
                    spec.next_retransmit_at = now + SPECTATOR_RETRANSMIT_SECS;
                }
            }
            _ => {}
        }
        false
    }

    fn flush_pending_new_spectators(&mut self, now: f64) {
        if !self
            .spectators
            .iter()
            .any(|s| s.pending_new && s.stage == SpectatorStage::AwaitNew)
        {
            return;
        }
        let snap = self.delay.snapshot_at(now);
        for spec in &mut self.spectators {
            if spec.pending_new
                && spec.stage == SpectatorStage::AwaitNew
                && let Some(bytes) =
                    snap.encode_new_stage(Some(spec.slot), Some(&self.config.hostname))
            {
                spec.chan.write_reliable(&bytes);
                spec.stage = SpectatorStage::AwaitSendRes;
                spec.pending_new = false;
            }
        }
    }

    fn dispatch_delayed_frames(&mut self, now: f64) {
        let due = self.delay.drain_due_decoded(now);
        if due.is_empty() {
            return;
        }

        for (frame, msgs) in due {
            // Detect delayed map change (`ServerInfo` with new `spawn_count`) and ask
            // active spectators to reconnect so they sign on cleanly for the new map.
            let mut new_spawn = None;
            for msg in &msgs {
                if let SvcMessage::ServerInfo(info) = msg
                    && self.last_upstream_spawn_count != Some(info.spawn_count)
                {
                    new_spawn = Some(info.spawn_count);
                }
            }
            if let Some(sc) = new_spawn {
                let first_map = self.last_upstream_spawn_count.is_none();
                self.last_upstream_spawn_count = Some(sc);
                if !first_map {
                    let mut w = MsgWriter::new();
                    write_svc_stufftext(&mut w, "reconnect\n");
                    for spec in &mut self.spectators {
                        if spec.stage == SpectatorStage::Active {
                            spec.chan.write_reliable(w.as_slice());
                            spec.stage = SpectatorStage::AwaitNew;
                            spec.sent_frames.clear();
                            spec.delta_seq = None;
                        }
                    }
                    continue;
                }
            }

            if frame.is_signon {
                continue;
            }

            let snap = self.delay.delayed_snapshot();
            let usermsgs = self.delay.delayed_usermsgs();
            let max_clients = snap.server_info.as_ref().map_or(32, |i| i.max_clients);

            // Decode entity snapshot if this frame carried PacketEntities / DeltaPacketEntities.
            let mut latest_pe_entities: Option<Vec<GoldSrcEntityState>> = None;
            let mut non_entity_msgs: Vec<&SvcMessage> = Vec::with_capacity(msgs.len());
            for msg in &msgs {
                if let SvcMessage::PacketEntities(pe) = msg {
                    let decoded: Vec<GoldSrcEntityState> = pe
                        .entities
                        .iter()
                        .filter_map(|(&number, (ent_type, rec))| {
                            let wire_type = wire_entity_type(*ent_type);
                            let schema = snap
                                .deltas
                                .get(entity_schema_name(number, wire_type, max_clients))?;
                            Some(GoldSrcEntityState::from_delta(
                                number, wire_type, rec, schema,
                            ))
                        })
                        .collect();
                    latest_pe_entities = Some(decoded);
                } else {
                    non_entity_msgs.push(msg);
                }
            }

            let entity_slice = latest_pe_entities
                .as_deref()
                .unwrap_or(&snap.latest_entities);
            let director_cues = self.director.observe(now, max_clients, &msgs, entity_slice);

            let mut common_prefix = MsgWriter::with_capacity(frame.payload.len() + 64);
            for msg in &non_entity_msgs {
                write_svc_message(
                    &mut common_prefix,
                    msg,
                    &snap.deltas,
                    usermsgs,
                    f64::from(snap.server_time),
                );
            }
            for cue in &director_cues {
                write_svc_director(&mut common_prefix, cue);
                self.stats.director_cmds_sent += 1;
            }
            let prefix_bytes = common_prefix.into_bytes();

            self.stats.frames_forwarded += 1;

            for spec in &mut self.spectators {
                if spec.stage != SpectatorStage::Active || !spec.listening {
                    continue;
                }
                let mut unreliable = MsgWriter::with_capacity(prefix_bytes.len() + 512);
                unreliable.write_bytes(&prefix_bytes);

                let mut sent_map = None;
                if let Some(entities) = latest_pe_entities.as_deref() {
                    let current_seq = spec.chan.outgoing_sequence;
                    if let Some((delta_seq, prev_map)) = spec.delta_base(current_seq) {
                        write_svc_packetentities_delta(
                            &mut unreliable,
                            &snap.deltas,
                            max_clients,
                            delta_seq,
                            &snap.baselines,
                            prev_map,
                            entities,
                            f64::from(snap.server_time),
                        );
                    } else {
                        write_svc_packetentities_full(
                            &mut unreliable,
                            &snap.deltas,
                            max_clients,
                            &snap.baselines,
                            entities,
                            f64::from(snap.server_time),
                        );
                    }
                    sent_map = Some((current_seq, entity_record_map(snap, entities)));
                }

                let payload = unreliable.into_bytes();
                let budget = MAX_UNRELIABLE_DATAGRAM
                    .saturating_sub(spec.chan.next_packet_reliable_len());
                let send_unreliable = if payload.len() <= budget {
                    if let Some((seq, map)) = sent_map {
                        spec.record_sent_entities(seq, map);
                    }
                    payload
                } else {
                    Vec::new()
                };

                let pkt = spec.chan.build_packet(&send_unreliable);
                let _ = self.downstream.send(spec.addr, &pkt);
                spec.next_retransmit_at = now + SPECTATOR_RETRANSMIT_SECS;
            }
        }
    }

    fn send_spectator_reliable_if_due(&mut self, idx: usize, now: f64) {
        let spec = &mut self.spectators[idx];
        if spec.chan.has_pending_reliable() {
            let pkt = spec.chan.build_packet(&[]);
            let _ = self.downstream.send(spec.addr, &pkt);
            spec.next_retransmit_at = now + SPECTATOR_RETRANSMIT_SECS;
        }
    }

    fn pump_spectator_transmits(&mut self, now: f64) {
        for spec in &mut self.spectators {
            if spec.chan.has_pending_reliable() && now >= spec.next_retransmit_at {
                let pkt = spec.chan.build_packet(&[]);
                let _ = self.downstream.send(spec.addr, &pkt);
                spec.next_retransmit_at = now + SPECTATOR_RETRANSMIT_SECS;
            }
        }
    }

    fn evict_timed_out_spectators(&mut self, now: f64) {
        let mut idx = 0;
        while idx < self.spectators.len() {
            if now - self.spectators[idx].last_received > SPECTATOR_TIMEOUT_SECS {
                self.remove_spectator_at(idx);
            } else {
                idx += 1;
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl HltvProxy<UdpTransport, UdpTransport> {
    /// Binds non-blocking UDP sockets for upstream and downstream (`bind_addr`, e.g. `"0.0.0.0:27020"`).
    pub fn bind_udp(config: HltvConfig, bind_addr: &str) -> Result<Self, HltvError> {
        let upstream = UdpTransport::bind("0.0.0.0:0")?;
        let downstream = UdpTransport::bind(bind_addr)?;
        Ok(Self::new(config, upstream, downstream))
    }
}

fn clean_map_name(map_path: &str) -> String {
    let s = map_path.strip_prefix("maps/").unwrap_or(map_path);
    s.strip_suffix(".bsp").unwrap_or(s).to_string()
}

fn entity_record_map(
    snap: &SignonSnapshot,
    entities: &[GoldSrcEntityState],
) -> BTreeMap<u16, (u8, DeltaRecord)> {
    let max_clients = snap.server_info.as_ref().map_or(32, |i| i.max_clients);
    entities
        .iter()
        .filter_map(|ent| {
            let wire_type = wire_entity_type(ent.entity_type);
            let schema = snap
                .deltas
                .get(entity_schema_name(ent.number, wire_type, max_clients))?;
            Some((ent.number, (wire_type, ent.to_delta(schema))))
        })
        .collect()
}

/// Re-serializes a decoded `SvcMessage` (other than `PacketEntities`) onto `w`.
fn write_svc_message(
    w: &mut MsgWriter,
    msg: &SvcMessage,
    deltas: &DeltaRegistry,
    usermsgs: &UserMsgRegistry,
    server_time: f64,
) {
    match msg {
        SvcMessage::Nop => write_svc_nop(w),
        SvcMessage::Disconnect(r) => write_svc_disconnect(w, r),
        SvcMessage::Version(v) => write_svc_version(w, *v),
        SvcMessage::SetView(e) => write_svc_setview(w, *e),
        SvcMessage::Sound(s) => write_svc_sound(w, s),
        SvcMessage::Time(t) => write_svc_time(w, *t),
        SvcMessage::Print(t) => write_svc_print(w, t),
        SvcMessage::StuffText(t) => write_svc_stufftext(w, t),
        SvcMessage::SetAngle(a) => write_svc_setangle(w, *a),
        SvcMessage::AddAngle(y) => write_svc_addangle(w, *y),
        SvcMessage::ServerInfo(info) => write_svc_serverinfo(w, info),
        SvcMessage::LightStyle { index, pattern } => write_svc_lightstyle(w, *index, pattern),
        SvcMessage::UpdateUserInfo {
            slot,
            userid,
            userinfo,
            cdkey_hash,
        } => write_svc_updateuserinfo(w, *slot, *userid, userinfo, cdkey_hash),
        SvcMessage::DeltaDescription(_) => write_svc_deltadescriptions(w, deltas),
        SvcMessage::ClientData {
            parsed,
            weapon_deltas,
            ..
        } => {
            if let Some(cd_schema) = deltas.get("clientdata_t") {
                let weapons_map: WeaponRecords = weapon_deltas.iter().cloned().collect();
                let empty_map = WeaponRecords::new();
                let weapon_delta = deltas.get("weapon_data_t").map(|ws| WeaponDelta {
                    schema: ws,
                    from: &empty_map,
                    to: &weapons_map,
                });
                write_svc_clientdata_with_weapons(w, cd_schema, None, parsed, weapon_delta, server_time);
            }
        }
        SvcMessage::StopSound(raw) => {
            w.write_u8(svc::STOPSOUND);
            w.write_u16(*raw);
        }
        SvcMessage::Pings(pings) => write_svc_pings(w, pings),
        SvcMessage::TempEntity { te_type, raw } => write_svc_temp_entity(w, *te_type, raw),
        SvcMessage::SetPause(p) => write_svc_setpause(w, *p),
        SvcMessage::SignonNum(n) => write_svc_signonnum(w, *n),
        SvcMessage::CenterPrint(t) => write_svc_centerprint(w, t),
        SvcMessage::CdTrack { track, loop_track } => write_svc_cdtrack(w, *track, *loop_track),
        SvcMessage::WeaponAnim { anim, body } => {
            w.write_u8(svc::WEAPONANIM);
            w.write_u8(*anim);
            w.write_u8(*body);
        }
        SvcMessage::DecalName { index, name } => {
            w.write_u8(svc::DECALNAME);
            w.write_u8(*index);
            w.write_str(name);
        }
        SvcMessage::RoomType(rt) => write_svc_roomtype(w, *rt),
        SvcMessage::NewUserMsg(desc) => {
            w.write_u8(svc::NEWUSERMSG);
            w.write_u8(desc.id);
            w.write_i8(desc.size);
            w.write_fixed_str(&desc.name, 16);
        }
        SvcMessage::Choke => write_svc_choke(w),
        SvcMessage::ResourceList {
            resources,
            consistency_indices,
        } => write_svc_resourcelist_with_consistency(w, resources, consistency_indices),
        SvcMessage::NewMoveVars(mv) => write_svc_newmovevars(w, mv),
        SvcMessage::ResourceRequest { spawn_count, .. } => {
            write_svc_resourcerequest(w, *spawn_count);
        }
        SvcMessage::ResourceLocation(url) => write_svc_resourcelocation(w, url),
        SvcMessage::SendExtraInfo {
            fallback_dir,
            cheats,
        } => write_svc_sendextrainfo(w, fallback_dir, *cheats),
        SvcMessage::VoiceInit { codec, quality } => write_svc_voiceinit(w, codec, *quality),
        SvcMessage::VoiceData { player_slot, data } => write_svc_voicedata(w, *player_slot, data),
        SvcMessage::SendCvarValue(name) => write_svc_sendcvarvalue(w, name),
        SvcMessage::SendCvarValue2 {
            request_id,
            cvar_name,
        } => write_svc_sendcvarvalue2(w, *request_id, cvar_name),
        SvcMessage::UserMsg { id, decoded, .. } => match decoded {
            CsUserMsg::Unknown { payload, .. } => {
                if let Some(desc) = usermsgs.get_by_id(*id) {
                    w.write_u8(*id);
                    if desc.size < 0 {
                        let len = payload.len().min(255);
                        w.write_u8(len as u8);
                        w.write_bytes(&payload[..len]);
                    } else {
                        w.write_bytes(payload);
                    }
                }
            }
            other => {
                let _ = write_svc_usermsg(w, usermsgs, other);
            }
        },
        SvcMessage::Event(events) => {
            if let Some(schema) = deltas.get("event_t") {
                write_svc_event(w, schema, events, server_time);
            }
        }
        SvcMessage::EventReliable {
            index,
            args,
            fire_time,
        } => {
            if let Some(schema) = deltas.get("event_t") {
                write_svc_event_reliable(w, schema, *index, args, *fire_time, server_time);
            }
        }
        SvcMessage::Particle(p) => write_svc_particle(w, p),
        SvcMessage::Damage(d) => write_svc_damage(w, d),
        SvcMessage::SpawnStatic(s) => write_svc_spawnstatic(w, s),
        SvcMessage::SpawnStaticSound(s) => write_svc_spawnstaticsound(w, s),
        SvcMessage::Customization(c) => write_svc_customization(w, c),
        SvcMessage::CrosshairAngle { pitch, yaw } => write_svc_crosshairangle(w, *pitch, *yaw),
        SvcMessage::SoundFade(f) => write_svc_soundfade(w, f),
        SvcMessage::TimeScale(s) => write_svc_timescale(w, *s),
        SvcMessage::KilledMonster => write_svc_killedmonster(w),
        SvcMessage::FoundSecret => write_svc_foundsecret(w),
        SvcMessage::Intermission => write_svc_intermission(w),
        SvcMessage::Finale(t) => write_svc_finale(w, t),
        SvcMessage::Restore { save_name, maps } => write_svc_restore(w, save_name, maps),
        SvcMessage::Cutscene(t) => write_svc_cutscene(w, t),
        SvcMessage::FileTxferFailed(f) => write_svc_filetxferfailed(w, f),
        SvcMessage::Hltv(sub) => write_svc_hltv(w, *sub),
        SvcMessage::Director(payload) => write_svc_director(w, payload),
        SvcMessage::Exec(cmd) => write_svc_exec(w, cmd),
        SvcMessage::SpawnBaseline { .. } | SvcMessage::PacketEntities(_) | SvcMessage::Ignored(_) => {
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_clientcore::{ConnState, Connection, ConnectionConfig};
    use cs_proto48::demo::parse_demo;
    use cs_proto48::messages::{ENTITY_NORMAL, MoveVarsMsg, ResourceEntry, write_svc_userregs};
    use cs_proto48::query::{A2sInfoReply, decode_a2s_info_reply, encode_a2s_info_request};
    use cs_transport::{LoopbackEndpoint, LoopbackHub};

    struct MockUpstreamServer {
        hub: LoopbackHub,
        client_addr: Option<PeerAddr>,
        chan: Option<NetChan>,
        challenge: u32,
        deltas: DeltaRegistry,
        usermsgs: UserMsgRegistry,
        usercmd_schema: DeltaSchema,
        spawn_count: u32,
        map_crc: u32,
        connect_userinfo: Option<InfoString>,
        clc_hltv_received: Vec<u8>,
        client_active: bool,
        sent_frames: BTreeMap<u8, BTreeMap<u16, (u8, DeltaRecord)>>,
        client_delta_seq: Option<u8>,
    }

    impl MockUpstreamServer {
        fn new() -> Self {
            let deltas = DeltaRegistry::cs16_default();
            let usercmd_schema = deltas.get("usercmd_t").unwrap().clone();
            Self {
                hub: LoopbackHub::new(),
                client_addr: None,
                chan: None,
                challenge: 424_242,
                deltas,
                usermsgs: UserMsgRegistry::cs16_default(),
                usercmd_schema,
                spawn_count: 1,
                map_crc: 0x55AA_1234,
                connect_userinfo: None,
                clc_hltv_received: Vec::new(),
                client_active: false,
                sent_frames: BTreeMap::new(),
                client_delta_seq: None,
            }
        }

        fn connect_endpoint(&self) -> LoopbackEndpoint {
            self.hub.connect()
        }

        fn step(&mut self) {
            while let Some((from, data)) = self.hub.recv() {
                if data.starts_with(&CONNECTIONLESS_HEADER.to_le_bytes()) {
                    if let Ok(oob) = parse_oob_packet(&data) {
                        match oob {
                            OobPacket::GetChallenge { .. } => {
                                let reply = encode_s2c_challenge(self.challenge, 0, false);
                                let _ = self.hub.send(from, &reply);
                            }
                            OobPacket::Connect {
                                challenge,
                                userinfo,
                                ..
                            } if challenge == self.challenge => {
                                self.connect_userinfo = Some(userinfo);
                                self.client_addr = Some(from);
                                self.chan = Some(NetChan::new(FRAGMENT_S2C_MAX_SIZE));
                                let reply = encode_s2c_connection(1, &from.to_string(), false, 8684);
                                let _ = self.hub.send(from, &reply);
                            }
                            _ => {}
                        }
                    }
                    continue;
                }

                if self.client_addr == Some(from)
                    && let Some(inc) = self
                        .chan
                        .as_mut()
                        .and_then(|chan| chan.process_packet(&data).ok().flatten())
                {
                    let mut replies = MsgWriter::new();
                    for stream in std::iter::once(inc.payload).chain(inc.completed_normal_stream) {
                        if let Ok(msgs) =
                            parse_clc_stream(&stream, &self.usercmd_schema, inc.sequence)
                        {
                            for msg in msgs {
                                match msg {
                                    ClcMessage::Delta(s) => self.client_delta_seq = Some(s),
                                    ClcMessage::Hltv(sub) => self.clc_hltv_received.push(sub),
                                    ClcMessage::StringCmd(cmd) => {
                                        if cmd == "new" {
                                            let info = ServerInfoMsg {
                                                protocol: 48,
                                                spawn_count: self.spawn_count,
                                                map_crc: self.map_crc,
                                                client_dll_md5: [0; 16],
                                                max_clients: 16,
                                                player_slot: 0,
                                                is_deathmatch: true,
                                                game_dir: "cstrike".to_string(),
                                                hostname: "Mock Upstream".to_string(),
                                                map_path: "maps/de_dust2.bsp".to_string(),
                                                map_cycle: String::new(),
                                                vac_secure: false,
                                            };
                                            write_svc_serverinfo(&mut replies, &info);
                                            write_svc_deltadescriptions(&mut replies, &self.deltas);
                                            write_svc_newmovevars(
                                                &mut replies,
                                                &MoveVarsMsg::default(),
                                            );
                                            write_svc_userregs(&mut replies, &self.usermsgs);
                                            write_svc_updateuserinfo(
                                                &mut replies,
                                                1,
                                                55,
                                                "\\name\\PlayerOne",
                                                &[0; 16],
                                            );
                                        } else if cmd == "sendres" {
                                            write_svc_resourcerequest(
                                                &mut replies,
                                                self.spawn_count as i32,
                                            );
                                            write_svc_resourcelist_with_consistency(
                                                &mut replies,
                                                &[ResourceEntry {
                                                    res_type: 2,
                                                    filename: "maps/de_dust2.bsp".to_string(),
                                                    index: 1,
                                                    download_size: 0,
                                                    flags: 0,
                                                    md5: None,
                                                    reserved: None,
                                                }],
                                                &[],
                                            );
                                        } else if cmd.starts_with("spawn ") {
                                            let bl = GoldSrcEntityState {
                                                number: 2,
                                                entity_type: ENTITY_NORMAL,
                                                modelindex: 4,
                                                ..Default::default()
                                            };
                                            cs_proto48::messages::write_svc_spawnbaseline(
                                                &mut replies,
                                                &self.deltas,
                                                16,
                                                &[(2, bl)],
                                                1.0,
                                            );
                                            write_svc_hltv(&mut replies, drc::HLTV_ACTIVE);
                                            write_svc_signonnum(&mut replies, 1);
                                        } else if cmd == "sendents" {
                                            self.client_active = true;
                                            self.send_entities(
                                                1.0,
                                                &[GoldSrcEntityState {
                                                    number: 2,
                                                    entity_type: ENTITY_NORMAL,
                                                    origin: Vec3::new(10.0, 20.0, 30.0),
                                                    modelindex: 4,
                                                    ..Default::default()
                                                }],
                                                None,
                                            );
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    if !replies.is_empty()
                        && let Some(chan) = self.chan.as_mut()
                    {
                        chan.write_reliable(replies.as_slice());
                    }
                }
            }

            if let (Some(addr), Some(chan)) = (self.client_addr, self.chan.as_mut())
                && chan.has_pending_reliable()
            {
                let pkt = chan.build_packet(&[]);
                let _ = self.hub.send(addr, &pkt);
            }
        }

        fn send_entities(
            &mut self,
            time: f32,
            entities: &[GoldSrcEntityState],
            extra: Option<&CsUserMsg>,
        ) {
            let (Some(addr), Some(chan)) = (self.client_addr, self.chan.as_mut()) else {
                return;
            };
            let mut w = MsgWriter::new();
            write_svc_time(&mut w, time);
            if let Some(um) = extra {
                assert!(write_svc_usermsg(&mut w, &self.usermsgs, um));
            }
            let seq = chan.outgoing_sequence;
            if let Some(delta_seq) = self.client_delta_seq
                && let Some(prev) = self.sent_frames.get(&delta_seq)
            {
                write_svc_packetentities_delta(
                    &mut w,
                    &self.deltas,
                    16,
                    delta_seq,
                    &BTreeMap::new(),
                    prev,
                    entities,
                    f64::from(time),
                );
            } else {
                write_svc_packetentities_full(
                    &mut w,
                    &self.deltas,
                    16,
                    &BTreeMap::new(),
                    entities,
                    f64::from(time),
                );
            }
            let map: BTreeMap<u16, (u8, DeltaRecord)> = entities
                .iter()
                .map(|e| {
                    let schema = self
                        .deltas
                        .get(entity_schema_name(e.number, ENTITY_NORMAL, 16))
                        .unwrap();
                    (e.number, (ENTITY_NORMAL, e.to_delta(schema)))
                })
                .collect();
            self.sent_frames.insert((seq & 0xFF) as u8, map);
            let pkt = chan.build_packet(w.as_slice());
            let _ = self.hub.send(addr, &pkt);
        }
    }

    #[test]
    fn upstream_hltv_handshake_a2s_info_director_delay_and_demo_recording() {
        let mut upstream_srv = MockUpstreamServer::new();
        let upstream_ep = upstream_srv.connect_endpoint();
        let downstream_hub = LoopbackHub::new();

        let cfg = HltvConfig {
            name: "TestProxy".to_string(),
            hostname: "cs16-rs HLTV Live".to_string(),
            delay_secs: 5.0,
            max_spectators: 32,
            password: None,
            master_addr: Some(PeerAddr::Loopback(0)),
            record_demo_path: None,
        };
        let mut proxy = HltvProxy::new(cfg, upstream_ep, downstream_hub);
        proxy.start_recording();

        // 1. Pump until proxy upstream reaches InGame.
        let mut now = 0.0;
        for _ in 0..200 {
            now += 0.05;
            proxy.step(now);
            upstream_srv.step();
            if *proxy.upstream_state() == UpstreamState::InGame && !upstream_srv.clc_hltv_received.is_empty() {
                break;
            }
        }
        assert_eq!(*proxy.upstream_state(), UpstreamState::InGame);
        assert_eq!(
            upstream_srv
                .connect_userinfo
                .as_ref()
                .and_then(|u| u.get("hltv")),
            Some("1")
        );
        assert!(
            upstream_srv.clc_hltv_received.contains(&drc::HLTV_ACTIVE)
                || upstream_srv.clc_hltv_received.contains(&drc::HLTV_LISTEN)
        );
        assert!(proxy.stats().upstream_hltv_cmds >= 1);

        // 2. Verify A2S_INFO returns server_type == b'p' and accurate map/counts.
        let mut query_ep = proxy.downstream_transport().connect();
        query_ep
            .send(PeerAddr::Loopback(0), &encode_a2s_info_request(None))
            .unwrap();
        proxy.step(now);
        let (_, a2s_bytes) = query_ep.recv().expect("A2S_INFO reply");
        match decode_a2s_info_reply(&a2s_bytes, "127.0.0.1:27020", 1).unwrap() {
            A2sInfoReply::Info(info) => {
                assert_eq!(info.server_type, b'p');
                assert_eq!(info.name, "cs16-rs HLTV Live");
                assert_eq!(info.map, "de_dust2");
                assert_eq!(info.players, 0);
                assert_eq!(info.max_players, 32);
            }
            other => panic!("unexpected A2S reply: {other:?}"),
        }

        // 3. Connect a real cs_clientcore::Connection spectator to the proxy.
        let spec_ep = proxy.downstream_transport().connect();
        let mut spec_conn = Connection::new(
            spec_ep,
            PeerAddr::Loopback(0),
            ConnectionConfig::new("Spectator", 76_561_197_960_265_800, &md5_hex(b"spec")),
        );
        spec_conn.connect();

        for _ in 0..200 {
            now += 0.05;
            spec_conn.pump(now);
            proxy.step(now);
            upstream_srv.step();
            if *spec_conn.state() == ConnState::InGame {
                break;
            }
        }
        assert_eq!(*spec_conn.state(), ConnState::InGame);
        assert_eq!(spec_conn.stats().parse_errors, 0);
        assert_eq!(proxy.spectator_count(), 1);
        assert!(
            spec_conn
                .messages()
                .iter()
                .any(|m| matches!(m, SvcMessage::Hltv(drc::HLTV_ACTIVE))),
            "spectator must receive svc_hltv(HLTV_ACTIVE)"
        );
        assert!(
            spec_conn.messages().iter().any(|m| matches!(
                m,
                SvcMessage::Director(bytes) if bytes.first() == Some(&drc::CMD_MESSAGE)
            )),
            "spectator must receive auto-director CMD_MESSAGE banner"
        );

        // Verify A2S_INFO reflects 1 connected spectator.
        query_ep
            .send(PeerAddr::Loopback(0), &encode_a2s_info_request(None))
            .unwrap();
        proxy.step(now);
        let (_, a2s_bytes2) = query_ep.recv().expect("A2S_INFO reply");
        let A2sInfoReply::Info(info2) = decode_a2s_info_reply(&a2s_bytes2, "127.0.0.1:27020", 1).unwrap() else {
            panic!("expected Info");
        };
        assert_eq!(info2.players, 1);

        // 4. Upstream emits a moved entity + DeathMsg at `capture_time = now`.
        let capture_t = now;
        let moved_ent = GoldSrcEntityState {
            number: 2,
            entity_type: ENTITY_NORMAL,
            origin: Vec3::new(450.0, -120.0, 64.0),
            modelindex: 4,
            ..Default::default()
        };
        let death = CsUserMsg::DeathMsg {
            killer_id: 2,
            victim_id: 1,
            headshot: true,
            weapon: "ak47".to_string(),
        };
        upstream_srv.send_entities(capture_t as f32, &[moved_ent], Some(&death));

        // Pump before delay expires: spectator MUST NOT see x = 450.0 yet!
        for _ in 0..20 {
            now += 0.1; // +2.0s < 5.0s delay
            proxy.step(now);
            spec_conn.pump(now);
            upstream_srv.step();
        }
        assert!(now < capture_t + 5.0);
        let early_x = spec_conn
            .entities()
            .get(&2)
            .map(|e| e.origin.x)
            .unwrap_or(0.0);
        assert!(
            (early_x - 450.0).abs() > 10.0,
            "frame released early before tv_delay! x={early_x}"
        );

        // Advance past `capture_t + 5.0`: delayed frame + director CMD_EVENT + CMD_CAMERA arrive!
        while now < capture_t + 5.5 {
            now += 0.1;
            proxy.step(now);
            spec_conn.pump(now);
            upstream_srv.step();
        }
        // Pump a couple extra steps for delivery.
        for _ in 0..5 {
            now += 0.05;
            proxy.step(now);
            spec_conn.pump(now);
        }

        assert_eq!(spec_conn.stats().parse_errors, 0);
        let delayed_x = spec_conn
            .entities()
            .get(&2)
            .expect("entity 2 present after delay")
            .origin
            .x;
        assert!(
            (delayed_x - 450.0).abs() < 1.0,
            "expected delayed origin.x ~ 450.0, got {delayed_x}"
        );
        assert!(
            spec_conn.messages().iter().any(|m| matches!(
                m,
                SvcMessage::Director(bytes) if bytes.first() == Some(&drc::CMD_EVENT)
            )),
            "auto-director should emit CMD_EVENT on DeathMsg"
        );
        assert!(
            spec_conn.messages().iter().any(|m| matches!(
                m,
                SvcMessage::Director(bytes) if bytes.first() == Some(&drc::CMD_CAMERA)
            )),
            "auto-director should emit CMD_CAMERA"
        );

        // 5. Verify downstream clc_hltv (HLTV_STATUS & HLTV_LISTEN) handling.
        let mut spec2_chan = NetChan::new(FRAGMENT_C2S_SIZE);
        let mut spec2_ep = proxy.downstream_transport().connect();
        spec2_ep
            .send(PeerAddr::Loopback(0), &encode_getchallenge(true))
            .unwrap();
        proxy.step(now);
        let (_, ch_pkt) = spec2_ep.recv().unwrap();
        let OobPacket::S2cChallenge { challenge, .. } = parse_oob_packet(&ch_pkt).unwrap() else {
            panic!("expected challenge");
        };
        let mut hltv_user = default_cs16_userinfo("ChildProxy");
        hltv_user.set("hltv", "1");
        let conn_pkt = encode_connect_48(
            challenge,
            &default_cs16_protinfo(&md5_hex(b"child")),
            &hltv_user,
            &lan_steam_auth_ticket(76_561_197_960_265_801),
        );
        spec2_ep.send(PeerAddr::Loopback(0), &conn_pkt).unwrap();
        proxy.step(now);
        let (_, acc_pkt) = spec2_ep.recv().unwrap();
        assert!(matches!(
            parse_oob_packet(&acc_pkt).unwrap(),
            OobPacket::S2cConnection { .. }
        ));

        let mut clc_w = MsgWriter::new();
        write_clc_hltv(&mut clc_w, drc::HLTV_STATUS);
        write_clc_hltv(&mut clc_w, drc::HLTV_LISTEN);
        spec2_chan.write_reliable(clc_w.as_slice());
        let pkt = spec2_chan.build_packet(&[]);
        spec2_ep.send(PeerAddr::Loopback(0), &pkt).unwrap();
        proxy.step(now);

        assert_eq!(proxy.stats().downstream_hltv_status_cmds, 1);
        assert_eq!(proxy.stats().downstream_hltv_listen_cmds, 1);
        let (_, reply_pkt) = spec2_ep.recv().expect("reply to clc_hltv");
        let inc = spec2_chan.process_packet(&reply_pkt).unwrap().unwrap();
        let mut ctx2 = SvcParseContext::new();
        let reply_msgs = cs_proto48::messages::parse_svc_stream(&inc.payload, &mut ctx2).unwrap();
        assert!(
            reply_msgs
                .iter()
                .any(|m| matches!(m, SvcMessage::Hltv(drc::HLTV_ACTIVE)))
        );
        assert!(
            reply_msgs
                .iter()
                .any(|m| matches!(m, SvcMessage::Print(t) if t.contains("HLTV Status")))
        );

        // 6. Verify DemoRecorder output parses cleanly as HLDEMO v5.
        let dem_bytes = proxy
            .finish_recording()
            .expect("finish_recording ok")
            .expect("recording active");
        let parsed_demo = parse_demo(&dem_bytes).expect("valid HLDEMO v5 bytes");
        assert_eq!(parsed_demo.header.map_name, "maps/de_dust2.bsp");
        assert!(!parsed_demo.directory.is_empty());
        let total_frames: usize = parsed_demo
            .directory
            .iter()
            .map(|d| d.frames.len())
            .sum();
        assert!(total_frames >= 3, "expected recorded demo frames, got {total_frames}");
    }

    #[test]
    fn password_and_capacity_limits_reject_unauthorized_spectators() {
        let upstream_hub = LoopbackHub::new();
        let downstream_hub = LoopbackHub::new();
        let cfg = HltvConfig {
            password: Some("secret".to_string()),
            max_spectators: 1,
            ..Default::default()
        };
        let mut proxy = HltvProxy::new(cfg, upstream_hub.connect(), downstream_hub);
        let mut ep = proxy.downstream_transport().connect();
        let now = 1.0;

        ep.send(PeerAddr::Loopback(0), &encode_getchallenge(true))
            .unwrap();
        proxy.step(now);
        let (_, ch_pkt) = ep.recv().unwrap();
        let OobPacket::S2cChallenge { challenge, .. } = parse_oob_packet(&ch_pkt).unwrap() else {
            panic!("expected challenge");
        };

        // Wrong password rejected.
        let mut bad_user = default_cs16_userinfo("BadSpec");
        bad_user.set("password", "wrong");
        let bad_conn = encode_connect_48(
            challenge,
            &default_cs16_protinfo("00"),
            &bad_user,
            &lan_steam_auth_ticket(1),
        );
        ep.send(PeerAddr::Loopback(0), &bad_conn).unwrap();
        proxy.step(now);
        let (_, rej) = ep.recv().unwrap();
        assert!(matches!(parse_oob_packet(&rej).unwrap(), OobPacket::S2cReject(_)));

        // Right password accepted.
        let mut ok_user = default_cs16_userinfo("GoodSpec");
        ok_user.set("password", "secret");
        let ok_conn = encode_connect_48(
            challenge,
            &default_cs16_protinfo("00"),
            &ok_user,
            &lan_steam_auth_ticket(1),
        );
        ep.send(PeerAddr::Loopback(0), &ok_conn).unwrap();
        proxy.step(now);
        let (_, acc) = ep.recv().unwrap();
        assert!(matches!(
            parse_oob_packet(&acc).unwrap(),
            OobPacket::S2cConnection { .. }
        ));
        assert_eq!(proxy.spectator_count(), 1);
    }
}
