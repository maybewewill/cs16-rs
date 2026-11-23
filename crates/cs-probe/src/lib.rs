//! `cs-probe`: a headless Protocol 48 client. It performs the connectionless handshake and the
//! sign-on, sends user commands and tracks the entities the server sends. Integration tests
//! drive servers with it over a loopback transport; the binary connects to a real server over
//! UDP and records every datagram to a capture file (see [`capture`]).
//!
//! The connection itself is `cs-clientcore`'s [`Connection`](cs_clientcore::Connection): the
//! probe is that client with a configuration that suits a tool (no game files, so a server
//! that demands file consistency is left) and the capture recording on top. Its sign-on
//! sequence is verified against stock HLDS build 10210 (`docs/dev/reference-stand.md`).

pub mod capture;
pub mod diff;
pub mod dump;
#[cfg(test)]
mod test_support;

use std::collections::{BTreeMap, VecDeque};

use cs_clientcore::{Connection, ConnectionConfig, ConsistencyPolicy};
#[cfg(not(target_arch = "wasm32"))]
use cs_clientcore::DatagramTap;
use cs_proto48::delta::{GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd, GoldSrcWeaponData};
use cs_proto48::messages::{ServerInfoMsg, SvcMessage};
use cs_transport::{PacketTransport, PeerAddr};

#[cfg(not(target_arch = "wasm32"))]
pub use capture::CaptureWriter;
pub use capture::{CAPTURE_MAGIC, CaptureRecord, Direction, read_capture};
pub use cs_clientcore::ConnState as ProbeState;
pub use cs_clientcore::ConnStats as ProbeStats;
pub use cs_clientcore::MESSAGE_LOG_CAPACITY;

#[cfg(test)]
use cs_clientcore::{CONSISTENCY_REASON, NO_SERVERINFO_REASON};

/// Minimum spacing between two netchan packets (at most 100 packets per second): the interval
/// of the default `cl_cmdrate` 100.
#[cfg(test)]
const SEND_INTERVAL: f64 = 0.01;

/// Identity the probe connects with.
#[derive(Debug, Clone)]
pub struct ProbeConfig {
    pub name: String,
    pub steam_id: u64,
    pub cdkey_md5_hex: String,
}

/// Writes the datagrams of a connection to a capture file.
#[cfg(not(target_arch = "wasm32"))]
struct CaptureTap(CaptureWriter);

#[cfg(not(target_arch = "wasm32"))]
impl DatagramTap for CaptureTap {
    fn record(&mut self, sent: bool, time: f64, data: &[u8]) -> std::io::Result<()> {
        self.0.push(&CaptureRecord {
            dir: if sent { Direction::Sent } else { Direction::Received },
            time,
            data: data.to_vec(),
        })
    }
}

/// Headless client over any [`PacketTransport`]. It never reads the system clock: the caller
/// passes its time to [`ProbeClient::pump`].
pub struct ProbeClient<T: PacketTransport> {
    core: Connection<T>,
}

impl<T: PacketTransport> ProbeClient<T> {
    pub fn new(transport: T, server: PeerAddr, cfg: ProbeConfig) -> Self {
        let mut core_cfg = ConnectionConfig::new(&cfg.name, cfg.steam_id, &cfg.cdkey_md5_hex);
        // The probe has no game files to hash.
        core_cfg.consistency = ConsistencyPolicy::Refuse;
        // It observes: a stuffed `kill` must not make its player act.
        core_cfg.forward_stufftext = false;
        Self {
            core: Connection::new(transport, server, core_cfg),
        }
    }

    /// The client core under the probe.
    pub fn core(&self) -> &Connection<T> {
        &self.core
    }

    pub fn core_mut(&mut self) -> &mut Connection<T> {
        &mut self.core
    }

    /// Starts the handshake; `getchallenge` goes out on the next [`pump`](Self::pump).
    pub fn connect(&mut self) {
        self.core.connect();
    }

    /// Receives everything waiting, advances the state, then sends: a connectionless handshake
    /// packet when one is due, or one netchan packet when `now` is at least 0.01 s past the
    /// previous one.
    pub fn pump(&mut self, now: f64) {
        self.core.pump(now);
    }

    /// Sets a userinfo key sent in the `connect` packet in place of the default (call before
    /// [`connect`](Self::connect)), e.g. `_vgui_menus` `0` for a client that wants text menus.
    pub fn set_userinfo(&mut self, key: &str, value: &str) {
        self.core.set_userinfo(key, value);
    }

    pub fn queue_cmd(&mut self, cmd: GoldSrcUserCmd) {
        self.core.queue_cmd(cmd);
    }

    /// Queues a reliable string command. Ignored before the server accepted the connection
    /// (there is no channel to carry it yet).
    pub fn send_stringcmd(&mut self, cmd: &str) {
        self.core.send_stringcmd(cmd);
    }

    /// Overrides the sequence sent in `clc_delta`; `None` restores the last entity update's.
    pub fn force_delta_seq(&mut self, seq: Option<u8>) {
        self.core.force_delta_seq(seq);
    }

    /// Test hook: while `true`, outgoing packets carry no `clc_delta`, which asks the server for
    /// full updates.
    pub fn suppress_delta(&mut self, suppress: bool) {
        self.core.suppress_delta(suppress);
    }

    /// Test hook: while `true`, outgoing packets keep acknowledging the reliable message state
    /// of the moment the hook was set, so the server never sees later reliable messages
    /// acknowledged and keeps resending them. `None` before the server accepted the connection.
    pub fn withhold_reliable_ack(&mut self, withhold: bool) {
        self.core.withhold_reliable_ack(withhold);
    }

    /// Test hook: flips the checksum byte of the next `n` outgoing `clc_move` messages, so the
    /// server cannot parse the packets that carry them.
    pub fn corrupt_next_moves(&mut self, n: u32) {
        self.core.corrupt_next_moves(n);
    }

    /// Test hook: while `suppressed`, outgoing packets carry no `clc_move` at all (by default
    /// every packet has one: the queued commands, or the last one again with `msec` 0), so the
    /// server runs no command of this client. Queued commands wait.
    pub fn suppress_moves(&mut self, suppressed: bool) {
        self.core.suppress_moves(suppressed);
    }

    /// Test hook: sequence number the next netchan packet will carry; `None` before the server
    /// accepted the connection.
    pub fn outgoing_sequence(&self) -> Option<u32> {
        self.core.outgoing_sequence()
    }

    /// Test hook: sequence number of the last netchan packet received from the server; `None`
    /// before the server accepted the connection.
    pub fn incoming_sequence(&self) -> Option<u32> {
        self.core.incoming_sequence()
    }

    pub fn state(&self) -> &ProbeState {
        self.core.state()
    }

    pub fn server_info(&self) -> Option<&ServerInfoMsg> {
        self.core.server_info()
    }

    pub fn player_slot(&self) -> Option<u8> {
        self.core.player_slot()
    }

    pub fn entities(&self) -> &BTreeMap<u16, GoldSrcEntityState> {
        self.core.entities()
    }

    /// Spawn baselines received for the current map, decoded, by entity number.
    pub fn baselines(&self) -> BTreeMap<u16, GoldSrcEntityState> {
        self.core.baselines()
    }

    pub fn client_data(&self) -> Option<&GoldSrcClientData> {
        self.core.client_data()
    }

    /// The `weapon_data_t` list of the last `svc_clientdata` the server sent, as the client
    /// rebuilds it from the deltas, by weapon index; an index not listed is all zero.
    pub fn weapon_data(&self) -> BTreeMap<u8, GoldSrcWeaponData> {
        self.core.weapon_data()
    }

    pub fn userinfos(&self) -> &BTreeMap<u8, String> {
        self.core.userinfos()
    }

    /// Forgets the logged messages, so a test can tell what arrives from now on.
    pub fn clear_messages(&mut self) {
        self.core.clear_messages();
    }

    /// The last 1024 messages received, oldest first, without `svc_nop`, `svc_time`,
    /// `svc_clientdata` and the entity updates that every frame carries. User messages are
    /// decoded. A map change does not clear it.
    pub fn messages(&self) -> &VecDeque<SvcMessage> {
        self.core.messages()
    }

    /// Opcode of every decoded server message in arrival order; a user message by its id.
    pub fn opcode_log(&self) -> &[u8] {
        self.core.opcode_log()
    }

    pub fn stats(&self) -> ProbeStats {
        self.core.stats()
    }

    /// The transport the probe talks through.
    pub fn transport(&self) -> &T {
        self.core.transport()
    }

    pub fn transport_mut(&mut self) -> &mut T {
        self.core.transport_mut()
    }

    /// Records every datagram exchanged with the server from now on.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_capture(&mut self, w: CaptureWriter) {
        self.core.set_tap(Box::new(CaptureTap(w)));
    }

    /// The error that stopped the capture, if writing it failed.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn capture_error(&self) -> Option<&std::io::Error> {
        self.core.tap_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_proto48::bitbuf::MsgWriter;
    use cs_proto48::handshake::{OobPacket, lan_steam_auth_ticket, parse_oob_packet};
    use cs_proto48::netchan::NetChan;
    use cs_proto48::crypto::{com_unmunge2, md5_hex};
    use cs_proto48::delta::DeltaRegistry;
    use cs_proto48::handshake::{encode_s2c_challenge, encode_s2c_connection, encode_s2c_reject};
    use cs_proto48::messages::{
        ClcMessage, ENTITY_NORMAL, parse_clc_stream, svc, write_svc_clientdata,
        write_svc_disconnect, write_svc_packetentities_full, write_svc_resourcelist,
        write_svc_resourcerequest, write_svc_serverinfo, write_svc_signonnum,
        write_svc_print, write_svc_spawnbaseline, write_svc_stufftext, write_svc_time,
        write_svc_updateuserinfo,
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
        assert!(!probe.core().parse_context().clientdata_history.is_empty());
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
        assert!(probe.core().parse_context().baselines.is_empty());
        assert!(probe.core().parse_context().entity_frames.is_empty());
        assert!(probe.core().parse_context().clientdata_history.is_empty());
        assert!(probe.core().last_entity_seq().is_none());
        // The caller's override survives the reconnect.
        assert!(reply.iter().flatten().any(|m| m == &ClcMessage::Delta(9)));
    }

    #[test]
    fn message_log_skips_per_update_messages() {
        let (mut server, mut probe) = ScriptedServer::new();
        let mut now = handshake(&mut server, &mut probe);
        server.recv_clc();
        let stream = msg(|w| {
            w.write_u8(svc::NOP);
            write_svc_time(w, 1.0);
            write_svc_print(w, "hello\n");
            write_svc_clientdata(
                w,
                server.reg.get("clientdata_t").unwrap(),
                None,
                &GoldSrcClientData::default(),
                1.0,
            );
            write_svc_stufftext(w, "echo hi\n");
        });
        exchange(&mut server, &mut probe, &mut now, &stream);
        assert_eq!(
            probe.messages().iter().cloned().collect::<Vec<_>>(),
            vec![
                SvcMessage::Print("hello\n".to_string()),
                SvcMessage::StuffText("echo hi\n".to_string()),
            ]
        );
    }

    #[test]
    fn message_log_keeps_the_last_1024() {
        let (_server, mut probe) = ScriptedServer::new();
        for i in 0..MESSAGE_LOG_CAPACITY + 10 {
            probe.core_mut().log_message(&SvcMessage::Print(i.to_string()));
        }
        probe.core_mut().log_message(&SvcMessage::Nop);
        probe.core_mut().log_message(&SvcMessage::Time(2.0));
        assert_eq!(probe.messages().len(), MESSAGE_LOG_CAPACITY);
        assert_eq!(probe.messages().front(), Some(&SvcMessage::Print("10".to_string())));
        assert_eq!(
            probe.messages().back(),
            Some(&SvcMessage::Print((MESSAGE_LOG_CAPACITY + 9).to_string()))
        );
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
    fn suppressed_moves_leave_the_packets_without_a_clc_move() {
        let (mut server, mut probe) = ScriptedServer::new();
        let mut now = handshake(&mut server, &mut probe);
        server.recv_clc();
        let schema = server.reg.get("usercmd_t").unwrap().clone();
        let mut moves_in_packets = |probe: &mut ProbeClient<_>, now: &mut f64| {
            *now += SEND_INTERVAL;
            probe.pump(*now);
            let mut count = 0;
            for d in server.recv_raw() {
                let inc = server.chan.process_packet(&d).unwrap().unwrap();
                let msgs = parse_clc_stream(&inc.payload, &schema, inc.sequence).unwrap();
                count += msgs
                    .iter()
                    .filter(|m| matches!(m, ClcMessage::Move { .. }))
                    .count();
            }
            count
        };
        assert_eq!(moves_in_packets(&mut probe, &mut now), 1);
        probe.suppress_moves(true);
        assert_eq!(moves_in_packets(&mut probe, &mut now), 0);
        probe.suppress_moves(false);
        assert_eq!(moves_in_packets(&mut probe, &mut now), 1);
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
