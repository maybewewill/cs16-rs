//! `cs-probe`: a headless Protocol 48 client. It performs the connectionless handshake and the
//! sign-on, sends user commands and tracks the entities the server sends. Integration tests
//! drive servers with it over a loopback transport; the binary connects to a real server over
//! UDP and records every datagram to a capture file (see [`capture`]).
//!
//! The sign-on sequence is a working assumption (recalled HLDS behaviour), not yet verified
//! against a reference server.

pub mod capture;

use std::collections::{BTreeMap, VecDeque};

use cs_proto48::CONNECTIONLESS_HEADER;
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::crypto::com_munge2;
use cs_proto48::delta::{GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd};
use cs_proto48::handshake::{
    OobPacket, default_cs16_protinfo, default_cs16_userinfo, encode_connect_48,
    encode_getchallenge, lan_steam_auth_ticket, parse_oob_packet,
};
use cs_proto48::messages::{
    PacketEntitiesMsg, ServerInfoMsg, SvcMessage, SvcParseContext, entity_schema_name,
    parse_svc_stream, write_clc_delta, write_clc_move, write_clc_resourcelist, write_clc_stringcmd,
};
use cs_proto48::netchan::{FRAGMENT_C2S_SIZE, NetChan, SplitPacketAssembler};
use cs_transport::{PacketTransport, PeerAddr};

#[cfg(not(target_arch = "wasm32"))]
pub use capture::CaptureWriter;
pub use capture::{CAPTURE_MAGIC, CaptureRecord, Direction, read_capture};

/// Minimum spacing between two netchan packets (at most 100 packets per second).
const SEND_INTERVAL: f64 = 0.01;
/// A connectionless handshake packet is resent when no answer arrived within this time.
const HANDSHAKE_RESEND: f64 = 1.0;
/// Previously sent commands repeated in every `clc_move` as backups.
const MAX_BACKUP_CMDS: usize = 2;
/// Queued commands sent as new commands in one packet; the rest wait for the next packet, so
/// the `clc_move` body stays far below its one-byte length limit.
const MAX_NEW_CMDS: usize = 16;
/// Reason given when the server demands file consistency checks the probe cannot answer.
const CONSISTENCY_REASON: &str = "consistency required: set mp_consistency 0";
/// Reason given when `svc_resourcelist` arrives before `svc_serverinfo`.
const NO_SERVERINFO_REASON: &str = "resourcelist before serverinfo";

/// Identity the probe connects with.
#[derive(Debug, Clone)]
pub struct ProbeConfig {
    pub name: String,
    pub steam_id: u64,
    pub cdkey_md5_hex: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProbeState {
    Idle,
    AwaitChallenge,
    AwaitConnection,
    Signon,
    InGame,
    Rejected(String),
    Disconnected(String),
}

impl ProbeState {
    fn is_terminal(&self) -> bool {
        matches!(self, ProbeState::Rejected(_) | ProbeState::Disconnected(_))
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct ProbeStats {
    pub datagrams_in: u64,
    pub bytes_in: u64,
    pub largest_datagram: usize,
    pub full_updates: u64,
    pub delta_updates: u64,
    /// Entity updates received during the last second of the caller's clock.
    pub frames_in_last_second: u32,
    /// Size in bytes of the most recent datagram received from the server.
    pub last_datagram: usize,
    /// Datagrams from the server that could not be parsed (or were not understood).
    pub parse_errors: u64,
}

/// Headless client over any [`PacketTransport`]. It never reads the system clock: the caller
/// passes its time to [`ProbeClient::pump`].
pub struct ProbeClient<T: PacketTransport> {
    transport: T,
    server: PeerAddr,
    cfg: ProbeConfig,
    state: ProbeState,
    chan: Option<NetChan>,
    ctx: SvcParseContext,
    splits: SplitPacketAssembler,
    challenge: u32,
    /// When the current connectionless handshake packet was last sent; `None` sends it on the
    /// next pump.
    handshake_sent_at: Option<f64>,
    /// Earliest time the next netchan packet may go out.
    next_send_at: f64,
    server_info: Option<ServerInfoMsg>,
    entities: BTreeMap<u16, GoldSrcEntityState>,
    client_data: Option<GoldSrcClientData>,
    userinfos: BTreeMap<u8, String>,
    opcode_log: Vec<u8>,
    stats: ProbeStats,
    /// Arrival times of entity updates within the last second.
    frame_times: VecDeque<f64>,
    cmd_queue: VecDeque<GoldSrcUserCmd>,
    /// Last sent commands, newest first, repeated as backups.
    sent_cmds: VecDeque<GoldSrcUserCmd>,
    /// Most recent command taken from the queue; repeated with `msec = 0` when the queue is empty.
    last_cmd: GoldSrcUserCmd,
    /// Low byte of the netchan sequence of the last entity update.
    last_entity_seq: Option<u8>,
    forced_delta_seq: Option<u8>,
    /// Test hook: `clc_delta` is left out of outgoing packets.
    delta_suppressed: bool,
    /// Test hook: the reliable acknowledgement bit outgoing packets carry instead of the
    /// channel's own.
    withheld_reliable_ack: Option<u32>,
    /// Outgoing `clc_move` messages still to be sent with a corrupted checksum (test hook).
    corrupt_moves: u32,
    #[cfg(not(target_arch = "wasm32"))]
    capture: Option<CaptureWriter>,
    #[cfg(not(target_arch = "wasm32"))]
    capture_error: Option<std::io::Error>,
}

impl<T: PacketTransport> ProbeClient<T> {
    pub fn new(transport: T, server: PeerAddr, cfg: ProbeConfig) -> Self {
        Self {
            transport,
            server,
            cfg,
            state: ProbeState::Idle,
            chan: None,
            ctx: SvcParseContext::new(),
            splits: SplitPacketAssembler::default(),
            challenge: 0,
            handshake_sent_at: None,
            next_send_at: f64::NEG_INFINITY,
            server_info: None,
            entities: BTreeMap::new(),
            client_data: None,
            userinfos: BTreeMap::new(),
            opcode_log: Vec::new(),
            stats: ProbeStats::default(),
            frame_times: VecDeque::new(),
            cmd_queue: VecDeque::new(),
            sent_cmds: VecDeque::new(),
            last_cmd: GoldSrcUserCmd::default(),
            last_entity_seq: None,
            forced_delta_seq: None,
            delta_suppressed: false,
            withheld_reliable_ack: None,
            corrupt_moves: 0,
            #[cfg(not(target_arch = "wasm32"))]
            capture: None,
            #[cfg(not(target_arch = "wasm32"))]
            capture_error: None,
        }
    }

    /// Starts the handshake; `getchallenge` goes out on the next [`pump`](Self::pump).
    pub fn connect(&mut self) {
        self.state = ProbeState::AwaitChallenge;
        self.handshake_sent_at = None;
    }

    /// Receives everything waiting, advances the state, then sends: a connectionless handshake
    /// packet when one is due, or one netchan packet when `now` is at least 0.01 s past the
    /// previous one.
    pub fn pump(&mut self, now: f64) {
        while let Some((from, data)) = self.transport.recv() {
            if from != self.server {
                continue;
            }
            self.record(Direction::Received, now, &data);
            self.stats.datagrams_in += 1;
            self.stats.bytes_in += data.len() as u64;
            self.stats.largest_datagram = self.stats.largest_datagram.max(data.len());
            self.stats.last_datagram = data.len();
            self.handle_datagram(&data, now);
        }

        while self.frame_times.front().is_some_and(|&t| t <= now - 1.0) {
            self.frame_times.pop_front();
        }
        self.stats.frames_in_last_second = self.frame_times.len() as u32;

        match self.state {
            ProbeState::AwaitChallenge | ProbeState::AwaitConnection => {
                if self
                    .handshake_sent_at
                    .is_none_or(|t| now - t >= HANDSHAKE_RESEND)
                {
                    let packet = if self.state == ProbeState::AwaitChallenge {
                        encode_getchallenge(true)
                    } else {
                        encode_connect_48(
                            self.challenge,
                            &default_cs16_protinfo(&self.cfg.cdkey_md5_hex),
                            &default_cs16_userinfo(&self.cfg.name),
                            &lan_steam_auth_ticket(self.cfg.steam_id),
                        )
                    };
                    self.send(&packet, now);
                    self.handshake_sent_at = Some(now);
                }
            }
            ProbeState::Signon | ProbeState::InGame => {
                if now >= self.next_send_at {
                    self.send_netchan_packet(now);
                    self.next_send_at = now + SEND_INTERVAL;
                }
            }
            ProbeState::Idle | ProbeState::Rejected(_) | ProbeState::Disconnected(_) => {}
        }
    }

    pub fn queue_cmd(&mut self, cmd: GoldSrcUserCmd) {
        self.cmd_queue.push_back(cmd);
    }

    /// Queues a reliable string command. Ignored before the server accepted the connection
    /// (there is no channel to carry it yet).
    pub fn send_stringcmd(&mut self, cmd: &str) {
        if let Some(chan) = self.chan.as_mut() {
            let mut w = MsgWriter::new();
            write_clc_stringcmd(&mut w, cmd);
            chan.write_reliable(&w.into_bytes());
        }
    }

    /// Overrides the sequence sent in `clc_delta`; `None` restores the last entity update's.
    pub fn force_delta_seq(&mut self, seq: Option<u8>) {
        self.forced_delta_seq = seq;
    }

    /// Test hook: while `true`, outgoing packets carry no `clc_delta`, which asks the server for
    /// full updates.
    pub fn suppress_delta(&mut self, suppress: bool) {
        self.delta_suppressed = suppress;
    }

    /// Test hook: while `true`, outgoing packets keep acknowledging the reliable message state
    /// of the moment the hook was set, so the server never sees later reliable messages
    /// acknowledged and keeps resending them. `None` before the server accepted the connection.
    pub fn withhold_reliable_ack(&mut self, withhold: bool) {
        self.withheld_reliable_ack = if withhold {
            self.chan.as_ref().map(|c| c.incoming_reliable_sequence)
        } else {
            None
        };
    }

    /// Test hook: flips the checksum byte of the next `n` outgoing `clc_move` messages, so the
    /// server cannot parse the packets that carry them.
    pub fn corrupt_next_moves(&mut self, n: u32) {
        self.corrupt_moves = n;
    }

    /// Test hook: sequence number the next netchan packet will carry; `None` before the server
    /// accepted the connection.
    pub fn outgoing_sequence(&self) -> Option<u32> {
        self.chan.as_ref().map(|c| c.outgoing_sequence)
    }

    /// Test hook: sequence number of the last netchan packet received from the server; `None`
    /// before the server accepted the connection.
    pub fn incoming_sequence(&self) -> Option<u32> {
        self.chan.as_ref().map(|c| c.incoming_sequence)
    }

    pub fn state(&self) -> &ProbeState {
        &self.state
    }

    pub fn server_info(&self) -> Option<&ServerInfoMsg> {
        self.server_info.as_ref()
    }

    pub fn player_slot(&self) -> Option<u8> {
        self.server_info.as_ref().map(|info| info.player_slot)
    }

    pub fn entities(&self) -> &BTreeMap<u16, GoldSrcEntityState> {
        &self.entities
    }

    /// Spawn baselines received for the current map, decoded, by entity number.
    pub fn baselines(&self) -> BTreeMap<u16, GoldSrcEntityState> {
        let deltas = &self.ctx.deltas;
        let max_clients = self.ctx.max_clients;
        self.ctx
            .baselines
            .iter()
            .filter_map(|(&number, (ent_type, rec))| {
                let schema = deltas.get(entity_schema_name(number, *ent_type, max_clients))?;
                Some((number, GoldSrcEntityState::from_delta(number, *ent_type, rec, schema)))
            })
            .collect()
    }

    pub fn client_data(&self) -> Option<&GoldSrcClientData> {
        self.client_data.as_ref()
    }

    pub fn userinfos(&self) -> &BTreeMap<u8, String> {
        &self.userinfos
    }

    /// Opcode of every decoded server message in arrival order; a user message by its id.
    pub fn opcode_log(&self) -> &[u8] {
        &self.opcode_log
    }

    pub fn stats(&self) -> ProbeStats {
        self.stats
    }

    /// The transport the probe talks through.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Records every datagram exchanged with the server from now on.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_capture(&mut self, w: CaptureWriter) {
        self.capture = Some(w);
    }

    /// The error that stopped the capture, if writing it failed.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture_error(&self) -> Option<&std::io::Error> {
        self.capture_error.as_ref()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn record(&mut self, dir: Direction, time: f64, data: &[u8]) {
        if let Some(w) = self.capture.as_mut() {
            let record = CaptureRecord {
                dir,
                time,
                data: data.to_vec(),
            };
            if let Err(e) = w.push(&record) {
                self.capture = None;
                self.capture_error = Some(e);
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn record(&mut self, _dir: Direction, _time: f64, _data: &[u8]) {}

    /// Sends one datagram to the server. A send error is treated like a datagram lost on the
    /// network: handshake packets are resent and netchan reliable data is retransmitted.
    fn send(&mut self, data: &[u8], now: f64) {
        self.record(Direction::Sent, now, data);
        let _ = self.transport.send(self.server, data);
    }

    fn handle_datagram(&mut self, data: &[u8], now: f64) {
        let Some(datagram) = self.splits.push(data) else {
            return;
        };
        if datagram.starts_with(&CONNECTIONLESS_HEADER.to_le_bytes()) {
            self.handle_connectionless(&datagram);
        } else {
            self.handle_netchan(&datagram, now);
        }
    }

    fn handle_connectionless(&mut self, datagram: &[u8]) {
        match parse_oob_packet(datagram) {
            Err(_) | Ok(OobPacket::Unknown(_)) => self.stats.parse_errors += 1,
            Ok(OobPacket::S2cChallenge { challenge, .. })
                if self.state == ProbeState::AwaitChallenge =>
            {
                self.challenge = challenge;
                self.state = ProbeState::AwaitConnection;
                self.handshake_sent_at = None;
            }
            Ok(OobPacket::S2cConnection { .. }) if self.state == ProbeState::AwaitConnection => {
                self.state = ProbeState::Signon;
                self.chan = Some(NetChan::new(FRAGMENT_C2S_SIZE));
                self.next_send_at = f64::NEG_INFINITY;
                self.send_stringcmd("new");
            }
            Ok(OobPacket::S2cReject(reason))
                if matches!(
                    self.state,
                    ProbeState::AwaitChallenge | ProbeState::AwaitConnection
                ) =>
            {
                self.state = ProbeState::Rejected(reason.trim_end_matches('\n').to_string());
            }
            // Late duplicates of handshake answers, prints and the like.
            Ok(_) => {}
        }
    }

    fn handle_netchan(&mut self, datagram: &[u8], now: f64) {
        let Some(chan) = self.chan.as_mut() else {
            self.stats.parse_errors += 1;
            return;
        };
        let incoming = match chan.process_packet(datagram) {
            Ok(Some(incoming)) => incoming,
            // Duplicate or out-of-order packet.
            Ok(None) => return,
            Err(_) => {
                self.stats.parse_errors += 1;
                return;
            }
        };
        self.ctx.last_incoming_seq = (incoming.sequence & 0xFF) as u8;

        let mut failed = false;
        let streams = std::iter::once(incoming.payload).chain(incoming.completed_normal_stream);
        for stream in streams {
            match parse_svc_stream(&stream, &mut self.ctx) {
                Ok(messages) => {
                    for message in messages {
                        self.opcode_log.push(message.opcode());
                        self.handle_svc(message, now);
                    }
                }
                Err(_) => failed = true,
            }
        }
        if failed {
            self.stats.parse_errors += 1;
        }
    }

    fn handle_svc(&mut self, message: SvcMessage, now: f64) {
        let signon = self.state == ProbeState::Signon;
        match message {
            SvcMessage::Disconnect(reason) if !self.state.is_terminal() => {
                self.state = ProbeState::Disconnected(reason.trim_end_matches('\n').to_string());
            }
            SvcMessage::ServerInfo(info) => {
                self.server_info = Some(*info);
                if signon {
                    self.send_stringcmd("sendres");
                }
            }
            SvcMessage::ResourceRequest { .. } if signon => {
                if let Some(chan) = self.chan.as_mut() {
                    let mut w = MsgWriter::new();
                    write_clc_resourcelist(&mut w, &[]);
                    chan.write_reliable(&w.into_bytes());
                }
            }
            SvcMessage::ResourceList {
                consistency_indices,
                ..
            } if signon => {
                if consistency_indices.is_empty() {
                    self.send_spawn();
                } else {
                    self.state = ProbeState::Disconnected(CONSISTENCY_REASON.to_string());
                }
            }
            SvcMessage::SignonNum(1) if signon => self.send_stringcmd("sendents"),
            SvcMessage::PacketEntities(update) => self.apply_entities(&update, now),
            SvcMessage::ClientData { parsed, .. } => self.client_data = Some(parsed),
            SvcMessage::UpdateUserInfo { slot, userinfo, .. } => {
                self.userinfos.insert(slot, userinfo);
            }
            SvcMessage::StuffText(text)
                if matches!(self.state, ProbeState::Signon | ProbeState::InGame)
                    && is_reconnect(&text) =>
            {
                self.state = ProbeState::Signon;
                self.forget_map();
                self.send_stringcmd("new");
            }
            _ => {}
        }
    }

    /// `spawn <spawn_count> <crc>`: the map CRC munged with `COM_Munge2` keyed by
    /// `(-1 - spawn_count) & 0xFF`, printed as a signed 32-bit integer.
    /// Without `svc_serverinfo` there is no spawn count or map CRC to answer with, so the probe
    /// stops instead of waiting in `Signon` forever.
    fn send_spawn(&mut self) {
        let Some(info) = self.server_info.as_ref() else {
            self.state = ProbeState::Disconnected(NO_SERVERINFO_REASON.to_string());
            return;
        };
        let spawn_count = info.spawn_count;
        let mut crc = info.map_crc.to_le_bytes();
        com_munge2(&mut crc, (-1i32).wrapping_sub(spawn_count as i32) & 0xFF);
        let crc = i32::from_le_bytes(crc);
        self.send_stringcmd(&format!("spawn {spawn_count} {crc}"));
    }

    /// Drops everything that belongs to the previous map before a `reconnect` sign-on. The
    /// channel, user infos, stats, opcode log and the caller's `force_delta_seq` stay.
    fn forget_map(&mut self) {
        self.entities.clear();
        self.ctx.baselines.clear();
        self.ctx.instance_baselines.clear();
        self.ctx.entity_frames.clear();
        self.ctx.clientdata_history.clear();
        self.server_info = None;
        self.client_data = None;
        self.sent_cmds.clear();
        self.last_entity_seq = None;
    }

    /// Replaces the entity view with the complete frame the parser rebuilt from this update.
    fn apply_entities(&mut self, update: &PacketEntitiesMsg, now: f64) {
        let deltas = &self.ctx.deltas;
        let max_clients = self.ctx.max_clients;
        self.entities = update
            .entities
            .iter()
            .filter_map(|(&number, (ent_type, rec))| {
                let schema = deltas.get(entity_schema_name(number, *ent_type, max_clients))?;
                let state = GoldSrcEntityState::from_delta(number, *ent_type, rec, schema);
                Some((number, state))
            })
            .collect();

        if update.is_delta {
            self.stats.delta_updates += 1;
        } else {
            self.stats.full_updates += 1;
        }
        self.frame_times.push_back(now);
        self.last_entity_seq = Some(self.ctx.last_incoming_seq);
        if self.state == ProbeState::Signon {
            self.state = ProbeState::InGame;
        }
    }

    /// One netchan packet: `clc_move` with the queued commands (or the last command repeated
    /// with `msec = 0`) plus up to two backups, then `clc_delta` when there is a delta base.
    fn send_netchan_packet(&mut self, now: f64) {
        let Some(chan) = self.chan.as_mut() else {
            return;
        };
        let mut unreliable = MsgWriter::new();

        if let Some(schema) = self.ctx.deltas.get("usercmd_t") {
            let take = self.cmd_queue.len().min(MAX_NEW_CMDS);
            let mut new_cmds: Vec<GoldSrcUserCmd> = self.cmd_queue.drain(..take).collect();
            match new_cmds.last() {
                Some(last) => self.last_cmd = last.clone(),
                None => new_cmds.push(GoldSrcUserCmd {
                    msec: 0,
                    ..self.last_cmd.clone()
                }),
            }

            let cmds_newest_first: Vec<GoldSrcUserCmd> = new_cmds
                .iter()
                .rev()
                .chain(self.sent_cmds.iter())
                .cloned()
                .collect();
            let mut mv = MsgWriter::new();
            write_clc_move(
                &mut mv,
                schema,
                chan.outgoing_sequence,
                0,
                self.sent_cmds.len() as u8,
                &cmds_newest_first,
            );
            let mut mv = mv.into_bytes();
            if self.corrupt_moves > 0 {
                self.corrupt_moves -= 1;
                // Opcode, length, then the checksum byte.
                mv[2] ^= 0xFF;
            }
            unreliable.write_bytes(&mv);
            for cmd in new_cmds {
                self.sent_cmds.push_front(cmd);
            }
            self.sent_cmds.truncate(MAX_BACKUP_CMDS);
        }

        if let Some(seq) = self.forced_delta_seq.or(self.last_entity_seq)
            && !self.delta_suppressed
        {
            write_clc_delta(&mut unreliable, seq);
        }
        let packet = match self.withheld_reliable_ack {
            Some(ack) => {
                let own = std::mem::replace(&mut chan.incoming_reliable_sequence, ack);
                let packet = chan.build_packet(&unreliable.into_bytes());
                chan.incoming_reliable_sequence = own;
                packet
            }
            None => chan.build_packet(&unreliable.into_bytes()),
        };
        self.send(&packet, now);
    }
}

/// Whether a `svc_stufftext` text contains the command `reconnect`.
fn is_reconnect(text: &str) -> bool {
    text.split(['\n', ';']).any(|cmd| cmd.trim() == "reconnect")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_proto48::crypto::{com_unmunge2, md5_hex};
    use cs_proto48::delta::DeltaRegistry;
    use cs_proto48::handshake::{encode_s2c_challenge, encode_s2c_connection, encode_s2c_reject};
    use cs_proto48::messages::{
        ClcMessage, ENTITY_NORMAL, parse_clc_stream, svc, write_svc_clientdata,
        write_svc_disconnect, write_svc_packetentities_full, write_svc_resourcelist,
        write_svc_resourcerequest, write_svc_serverinfo, write_svc_signonnum,
        write_svc_spawnbaseline, write_svc_stufftext, write_svc_updateuserinfo,
    };
    use cs_proto48::netchan::FRAGMENT_S2C_MAX_SIZE;
    use cs_transport::{LoopbackEndpoint, LoopbackHub};
    use glam::Vec3;

    const CHALLENGE: u32 = 0x1234_5678;
    const SPAWN_COUNT: u32 = 3;
    const MAP_CRC: u32 = 0xDEAD_BEEF;

    fn config() -> ProbeConfig {
        ProbeConfig {
            name: "probe".to_string(),
            steam_id: 76_561_197_960_265_729,
            cdkey_md5_hex: md5_hex(b"cs-probe test key"),
        }
    }

    /// The test plays the server by hand on the hub side of a loopback.
    struct ScriptedServer {
        hub: LoopbackHub,
        client: PeerAddr,
        chan: NetChan,
        reg: DeltaRegistry,
    }

    impl ScriptedServer {
        fn new() -> (Self, ProbeClient<LoopbackEndpoint>) {
            let hub = LoopbackHub::new();
            let endpoint = hub.connect();
            let client = PeerAddr::Loopback(endpoint.id());
            let probe = ProbeClient::new(endpoint, PeerAddr::Loopback(0), config());
            let server = Self {
                hub,
                client,
                chan: NetChan::new(FRAGMENT_S2C_MAX_SIZE),
                reg: DeltaRegistry::cs16_default(),
            };
            (server, probe)
        }

        fn send_raw(&mut self, data: &[u8]) {
            self.hub.send(self.client, data).unwrap();
        }

        fn recv_raw(&mut self) -> Vec<Vec<u8>> {
            std::iter::from_fn(|| self.hub.recv().map(|(_, data)| data)).collect()
        }

        fn recv_oob(&mut self) -> Vec<OobPacket> {
            self.recv_raw()
                .iter()
                .map(|d| parse_oob_packet(d).unwrap())
                .collect()
        }

        /// Every netchan packet the probe sent since the last call, decoded.
        fn recv_clc(&mut self) -> Vec<Vec<ClcMessage>> {
            let schema = self.reg.get("usercmd_t").unwrap().clone();
            self.recv_raw()
                .iter()
                .filter_map(|d| self.chan.process_packet(d).unwrap())
                .map(|inc| parse_clc_stream(&inc.payload, &schema, inc.sequence).unwrap())
                .collect()
        }

        /// Sends one netchan packet carrying `reliable` (may be empty).
        fn send_reliable(&mut self, reliable: &[u8]) {
            self.chan.write_reliable(reliable);
            let packet = self.chan.build_packet(&[]);
            self.send_raw(&packet);
        }
    }

    fn string_cmds(packets: &[Vec<ClcMessage>]) -> Vec<String> {
        packets
            .iter()
            .flatten()
            .filter_map(|m| match m {
                ClcMessage::StringCmd(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    fn server_info() -> ServerInfoMsg {
        ServerInfoMsg {
            protocol: 48,
            spawn_count: SPAWN_COUNT,
            map_crc: MAP_CRC,
            client_dll_md5: [0; 16],
            max_clients: 32,
            player_slot: 0,
            is_deathmatch: true,
            game_dir: "cstrike".to_string(),
            hostname: "test".to_string(),
            map_path: "maps/de_dust2.bsp".to_string(),
            map_cycle: String::new(),
            vac_secure: false,
        }
    }

    fn msg(write: impl FnOnce(&mut MsgWriter)) -> Vec<u8> {
        let mut w = MsgWriter::new();
        write(&mut w);
        w.into_bytes()
    }

    /// Plays the handshake until the probe is in `Signon`; returns the time of the last pump.
    /// The probe's first netchan packet (`new`) is left for the caller to receive.
    fn handshake(server: &mut ScriptedServer, probe: &mut ProbeClient<LoopbackEndpoint>) -> f64 {
        probe.connect();
        probe.pump(0.0);
        assert_eq!(
            server.recv_oob(),
            vec![OobPacket::GetChallenge { steam: true }]
        );
        server.send_raw(&encode_s2c_challenge(CHALLENGE, 0, false));
        probe.pump(0.01);
        assert!(matches!(
            server.recv_oob().as_slice(),
            [OobPacket::Connect {
                protocol: 48,
                challenge: CHALLENGE,
                ..
            }]
        ));
        server.send_raw(&encode_s2c_connection(1, "loopback", false, 6153));
        probe.pump(0.02);
        assert_eq!(probe.state(), &ProbeState::Signon);
        0.02
    }

    /// Sends `reliable` to the probe, lets it answer once and returns its string commands.
    fn exchange(
        server: &mut ScriptedServer,
        probe: &mut ProbeClient<LoopbackEndpoint>,
        now: &mut f64,
        reliable: &[u8],
    ) -> Vec<Vec<ClcMessage>> {
        server.send_reliable(reliable);
        *now += SEND_INTERVAL;
        probe.pump(*now);
        server.recv_clc()
    }

    #[test]
    fn scripted_handshake_reaches_signon_and_sends_new() {
        let (mut server, mut probe) = ScriptedServer::new();
        assert_eq!(probe.state(), &ProbeState::Idle);
        probe.connect();
        assert_eq!(probe.state(), &ProbeState::AwaitChallenge);

        probe.pump(0.0);
        assert_eq!(
            server.recv_oob(),
            vec![OobPacket::GetChallenge { steam: true }]
        );
        server.send_raw(&encode_s2c_challenge(CHALLENGE, 0, false));
        probe.pump(0.01);
        assert_eq!(probe.state(), &ProbeState::AwaitConnection);
        match server.recv_oob().as_slice() {
            [
                OobPacket::Connect {
                    protocol,
                    challenge,
                    userinfo,
                    auth_ticket,
                    ..
                },
            ] => {
                assert_eq!((*protocol, *challenge), (48, CHALLENGE));
                assert_eq!(userinfo.get("name"), Some("probe"));
                assert_eq!(auth_ticket, &lan_steam_auth_ticket(config().steam_id));
            }
            other => panic!("expected connect, got {other:?}"),
        }

        server.send_raw(&encode_s2c_connection(1, "loopback", false, 6153));
        probe.pump(0.02);
        assert_eq!(probe.state(), &ProbeState::Signon);
        let packets = server.recv_clc();
        assert_eq!(packets.len(), 1);
        assert_eq!(string_cmds(&packets), vec!["new".to_string()]);
        assert!(
            packets[0]
                .iter()
                .any(|m| matches!(m, ClcMessage::Move { .. })),
            "every netchan packet carries clc_move: {packets:?}"
        );
    }

    #[test]
    fn handshake_packets_are_resent_after_one_second() {
        let (mut server, mut probe) = ScriptedServer::new();
        probe.connect();
        probe.pump(0.0);
        assert_eq!(server.recv_raw().len(), 1);
        probe.pump(0.99);
        assert!(server.recv_raw().is_empty());
        probe.pump(1.0);
        assert_eq!(
            server.recv_oob(),
            vec![OobPacket::GetChallenge { steam: true }]
        );
    }

    #[test]
    fn connectionless_reject_yields_rejected() {
        let (mut server, mut probe) = ScriptedServer::new();
        probe.connect();
        probe.pump(0.0);
        server.send_raw(&encode_s2c_reject("Server is full.\n"));
        probe.pump(0.01);
        assert_eq!(
            probe.state(),
            &ProbeState::Rejected("Server is full.".to_string())
        );
        server.recv_raw();
        probe.pump(5.0);
        assert!(
            server.recv_raw().is_empty(),
            "a rejected probe sends nothing"
        );
    }

    #[test]
    fn garbage_datagrams_count_as_parse_errors() {
        let (mut server, mut probe) = ScriptedServer::new();
        probe.connect();
        probe.pump(0.0);
        let garbage: [&[u8]; 4] = [
            b"",
            b"\x01\x02\x03",
            b"\xFF\xFF\xFF\xFFnonsense\n",
            b"\x00\x00\x00\x80\x00\x00\x00\x00\xAA\xBB\xCC",
        ];
        for g in garbage {
            server.send_raw(g);
        }
        probe.pump(0.01);
        assert_eq!(probe.stats().parse_errors, 4);
        assert_eq!(probe.stats().datagrams_in, 4);
        assert_eq!(probe.state(), &ProbeState::AwaitChallenge);

        // In `Signon` a too-short netchan datagram is counted too and changes nothing.
        let mut now = handshake_after_connect(&mut server, &mut probe);
        server.send_raw(b"\x01\x02\x03");
        now += SEND_INTERVAL;
        probe.pump(now);
        assert_eq!(probe.stats().parse_errors, 5);
        assert_eq!(probe.state(), &ProbeState::Signon);
    }

    /// Finishes a handshake whose `getchallenge` was already sent.
    fn handshake_after_connect(
        server: &mut ScriptedServer,
        probe: &mut ProbeClient<LoopbackEndpoint>,
    ) -> f64 {
        server.recv_raw();
        server.send_raw(&encode_s2c_challenge(CHALLENGE, 0, false));
        probe.pump(0.02);
        server.recv_raw();
        server.send_raw(&encode_s2c_connection(1, "loopback", false, 6153));
        probe.pump(0.03);
        assert_eq!(probe.state(), &ProbeState::Signon);
        server.recv_clc();
        0.03
    }

    #[test]
    fn signon_chain_reaches_in_game() {
        let (mut server, mut probe) = ScriptedServer::new();
        let mut now = handshake(&mut server, &mut probe);
        server.recv_clc();

        let info = msg(|w| {
            write_svc_serverinfo(w, &server_info());
            write_svc_updateuserinfo(w, 0, 1, "\\name\\probe", &[0; 16]);
        });
        let reply = exchange(&mut server, &mut probe, &mut now, &info);
        assert_eq!(string_cmds(&reply), vec!["sendres".to_string()]);
        assert_eq!(probe.player_slot(), Some(0));
        assert_eq!(probe.server_info().unwrap().map_crc, MAP_CRC);
        assert_eq!(
            probe.userinfos().get(&0).map(String::as_str),
            Some("\\name\\probe")
        );

        let request = msg(|w| write_svc_resourcerequest(w, SPAWN_COUNT as i32));
        let reply = exchange(&mut server, &mut probe, &mut now, &request);
        assert!(
            reply
                .iter()
                .flatten()
                .any(|m| m == &ClcMessage::ResourceList(Vec::new())),
            "empty clc_resourcelist expected: {reply:?}"
        );

        let list = msg(|w| write_svc_resourcelist(w, &[]));
        let reply = exchange(&mut server, &mut probe, &mut now, &list);
        let cmds = string_cmds(&reply);
        assert_eq!(cmds.len(), 1, "{cmds:?}");
        let args: Vec<&str> = cmds[0].split(' ').collect();
        assert_eq!(args[..2], ["spawn", "3"]);
        let mut crc = args[2].parse::<i32>().unwrap().to_le_bytes();
        com_unmunge2(&mut crc, (-1 - SPAWN_COUNT as i32) & 0xFF);
        assert_eq!(u32::from_le_bytes(crc), MAP_CRC);

        let signon = msg(|w| write_svc_signonnum(w, 1));
        let reply = exchange(&mut server, &mut probe, &mut now, &signon);
        assert_eq!(string_cmds(&reply), vec!["sendents".to_string()]);
        assert_eq!(probe.state(), &ProbeState::Signon);

        let ent = GoldSrcEntityState {
            number: 40,
            entity_type: ENTITY_NORMAL,
            origin: Vec3::new(128.0, -256.0, 64.0),
            modelindex: 7,
            ..Default::default()
        };
        let entities = msg(|w| {
            write_svc_packetentities_full(w, &server.reg, 32, &BTreeMap::new(), &[ent], 0.0)
        });
        // Entity updates travel unreliable; the sequence of this packet becomes the delta base.
        let packet = server.chan.build_packet(&entities);
        let entity_seq = (server.chan.outgoing_sequence - 1) as u8;
        server.send_raw(&packet);
        now += SEND_INTERVAL;
        probe.pump(now);
        assert_eq!(probe.state(), &ProbeState::InGame);
        assert_eq!(probe.entities().len(), 1);
        assert_eq!(probe.entities()[&40].origin, Vec3::new(128.0, -256.0, 64.0));
        assert_eq!(probe.entities()[&40].modelindex, 7);
        assert_eq!(probe.stats().full_updates, 1);
        assert_eq!(probe.stats().frames_in_last_second, 1);
        let reply = server.recv_clc();
        assert!(
            reply
                .iter()
                .flatten()
                .any(|m| m == &ClcMessage::Delta(entity_seq))
        );

        probe.force_delta_seq(Some(7));
        let reply = exchange(&mut server, &mut probe, &mut now, &[]);
        assert!(reply.iter().flatten().any(|m| m == &ClcMessage::Delta(7)));
        probe.force_delta_seq(None);
        let reply = exchange(&mut server, &mut probe, &mut now, &[]);
        assert!(
            reply
                .iter()
                .flatten()
                .any(|m| m == &ClcMessage::Delta(entity_seq))
        );
        probe.suppress_delta(true);
        let reply = exchange(&mut server, &mut probe, &mut now, &[]);
        assert!(!reply.iter().flatten().any(|m| matches!(m, ClcMessage::Delta(_))));
        probe.suppress_delta(false);
        let reply = exchange(&mut server, &mut probe, &mut now, &[]);
        assert!(
            reply
                .iter()
                .flatten()
                .any(|m| m == &ClcMessage::Delta(entity_seq))
        );
        assert_eq!(probe.stats().frames_in_last_second, 1);
        now += 1.0;
        probe.pump(now);
        assert_eq!(probe.stats().frames_in_last_second, 0);

        let opcodes: Vec<u8> = probe
            .opcode_log()
            .iter()
            .copied()
            .filter(|&op| op != svc::NOP) // netchan padding
            .collect();
        assert_eq!(
            opcodes,
            [
                svc::SERVERINFO,
                svc::UPDATEUSERINFO,
                svc::RESOURCEREQUEST,
                svc::RESOURCELIST,
                svc::SIGNONNUM,
                svc::PACKETENTITIES,
            ]
        );
    }

    #[test]
    fn consistency_list_disconnects() {
        let (mut server, mut probe) = ScriptedServer::new();
        let mut now = handshake(&mut server, &mut probe);
        server.recv_clc();
        exchange(
            &mut server,
            &mut probe,
            &mut now,
            &msg(|w| write_svc_serverinfo(w, &server_info())),
        );
        let list = msg(|w| {
            w.write_u8(svc::RESOURCELIST);
            w.start_bit_writing();
            w.write_bits(0, 12); // no resources
            w.write_one_bit(true); // consistency list follows
            w.write_one_bit(true); // one entry
            w.write_one_bit(false); // absolute index
            w.write_bits(5, 10);
            w.write_one_bit(false); // end of list
            w.end_bit_writing();
        });
        let reply = exchange(&mut server, &mut probe, &mut now, &list);
        assert_eq!(
            probe.state(),
            &ProbeState::Disconnected(CONSISTENCY_REASON.to_string())
        );
        assert!(string_cmds(&reply).iter().all(|c| !c.starts_with("spawn")));
    }

    #[test]
    fn svc_disconnect_and_reconnect() {
        let (mut server, mut probe) = ScriptedServer::new();
        let mut now = handshake(&mut server, &mut probe);
        server.recv_clc();

        let reply = exchange(
            &mut server,
            &mut probe,
            &mut now,
            &msg(|w| write_svc_stufftext(w, "reconnect\n")),
        );
        assert_eq!(string_cmds(&reply), vec!["new".to_string()]);
        assert_eq!(probe.state(), &ProbeState::Signon);

        exchange(
            &mut server,
            &mut probe,
            &mut now,
            &msg(|w| write_svc_disconnect(w, "Kicked by console")),
        );
        assert_eq!(
            probe.state(),
            &ProbeState::Disconnected("Kicked by console".to_string())
        );
    }

    #[test]
    fn reconnect_forgets_the_previous_map() {
        let (mut server, mut probe) = ScriptedServer::new();
        let mut now = handshake(&mut server, &mut probe);
        server.recv_clc();
        exchange(
            &mut server,
            &mut probe,
            &mut now,
            &msg(|w| write_svc_serverinfo(w, &server_info())),
        );

        let ent = GoldSrcEntityState {
            number: 40,
            entity_type: ENTITY_NORMAL,
            origin: Vec3::new(8.0, 16.0, 32.0),
            modelindex: 3,
            ..Default::default()
        };
        let baseline =
            msg(|w| write_svc_spawnbaseline(w, &server.reg, 32, &[(40, ent.clone())], 0.0));
        exchange(&mut server, &mut probe, &mut now, &baseline);
        let mut baselines = BTreeMap::new();
        baselines.insert(
            40,
            (
                ENTITY_NORMAL,
                ent.to_delta(server.reg.get("entity_state_t").unwrap()),
            ),
        );
        let update = msg(|w| {
            let schema = server.reg.get("clientdata_t").unwrap();
            write_svc_clientdata(w, schema, None, &GoldSrcClientData::default(), 0.0);
            write_svc_packetentities_full(w, &server.reg, 32, &baselines, &[ent], 0.0);
        });
        let packet = server.chan.build_packet(&update);
        server.send_raw(&packet);
        now += SEND_INTERVAL;
        probe.pump(now);
        server.recv_clc();
        assert_eq!(probe.state(), &ProbeState::InGame);
        assert_eq!(probe.entities().len(), 1);
        assert!(probe.server_info().is_some());
        assert!(probe.client_data().is_some());
        assert!(!probe.ctx.clientdata_history.is_empty());
        probe.force_delta_seq(Some(9));

        let reply = exchange(
            &mut server,
            &mut probe,
            &mut now,
            &msg(|w| write_svc_stufftext(w, "reconnect\n")),
        );
        assert_eq!(string_cmds(&reply), vec!["new".to_string()]);
        assert_eq!(probe.state(), &ProbeState::Signon);
        assert!(probe.entities().is_empty());
        assert!(probe.server_info().is_none());
        assert!(probe.client_data().is_none());
        assert!(probe.ctx.baselines.is_empty());
        assert!(probe.ctx.entity_frames.is_empty());
        assert!(probe.ctx.clientdata_history.is_empty());
        assert!(probe.last_entity_seq.is_none());
        // The caller's override survives the reconnect.
        assert!(reply.iter().flatten().any(|m| m == &ClcMessage::Delta(9)));
    }

    #[test]
    fn resourcelist_before_serverinfo_disconnects() {
        let (mut server, mut probe) = ScriptedServer::new();
        let mut now = handshake(&mut server, &mut probe);
        server.recv_clc();
        exchange(
            &mut server,
            &mut probe,
            &mut now,
            &msg(|w| write_svc_resourcelist(w, &[])),
        );
        assert_eq!(
            probe.state(),
            &ProbeState::Disconnected(NO_SERVERINFO_REASON.to_string())
        );
    }

    #[test]
    fn corrupt_next_moves_breaks_only_that_many_moves() {
        let (mut server, mut probe) = ScriptedServer::new();
        let mut now = handshake(&mut server, &mut probe);
        server.recv_clc();
        assert_eq!(
            probe.outgoing_sequence(),
            Some(server.chan.incoming_sequence + 1)
        );

        probe.corrupt_next_moves(1);
        let schema = server.reg.get("usercmd_t").unwrap().clone();
        let mut parsed = Vec::new();
        for _ in 0..2 {
            now += SEND_INTERVAL;
            probe.pump(now);
            for d in server.recv_raw() {
                let inc = server.chan.process_packet(&d).unwrap().unwrap();
                parsed.push(parse_clc_stream(&inc.payload, &schema, inc.sequence).is_ok());
            }
        }
        assert_eq!(parsed, vec![false, true]);
    }

    #[test]
    fn user_commands_carry_backups_and_respect_the_send_rate() {
        let (mut server, mut probe) = ScriptedServer::new();
        let mut now = handshake(&mut server, &mut probe);
        server.recv_clc();

        let cmd = |msec: u8, forward: f32| GoldSrcUserCmd {
            msec,
            forwardmove: forward,
            ..Default::default()
        };
        let moves = |packets: Vec<Vec<ClcMessage>>| -> Vec<(u8, Vec<GoldSrcUserCmd>)> {
            packets
                .into_iter()
                .flatten()
                .filter_map(|m| match m {
                    ClcMessage::Move {
                        num_backup, cmds, ..
                    } => Some((num_backup, cmds)),
                    _ => None,
                })
                .collect()
        };

        for i in 1..=3 {
            probe.queue_cmd(cmd(10, i as f32 * 10.0));
        }
        now += SEND_INTERVAL;
        probe.pump(now);
        // A second pump within 0.01 s sends nothing.
        probe.pump(now + 0.005);
        let sent = moves(server.recv_clc());
        // The backup is the empty command the `new` packet carried.
        assert_eq!(
            sent,
            vec![(
                1,
                vec![cmd(10, 30.0), cmd(10, 20.0), cmd(10, 10.0), cmd(0, 0.0)]
            )]
        );

        probe.queue_cmd(cmd(10, 40.0));
        let sent = moves(exchange(&mut server, &mut probe, &mut now, &[]));
        assert_eq!(
            sent,
            vec![(2, vec![cmd(10, 40.0), cmd(10, 30.0), cmd(10, 20.0)])]
        );

        let sent = moves(exchange(&mut server, &mut probe, &mut now, &[]));
        assert_eq!(
            sent,
            vec![(2, vec![cmd(0, 40.0), cmd(10, 40.0), cmd(10, 30.0)])]
        );
    }
}
