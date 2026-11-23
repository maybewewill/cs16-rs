//! The connection against a hand-played server (the state machine's details) and against
//! `cs-engine` over a loopback (the whole path).

use cs_clientcore::resources::{ResourceFiles, res_type};
use cs_clientcore::{
    ConnState, Connection, ConnectionConfig, ConsistencyPolicy, MAX_CMDRATE, MIN_CMDRATE,
};
use cs_proto48::bitbuf::{MsgReader, MsgWriter};
use cs_proto48::crypto::{com_munge2, com_unmunge, md5_hex};
use cs_proto48::delta::DeltaRegistry;
use cs_proto48::handshake::{encode_s2c_challenge, encode_s2c_connection};
use cs_proto48::messages::{
    ClcMessage, ResourceEntry, ServerInfoMsg, parse_clc_stream, svc, write_svc_print,
    write_svc_resourcerequest, write_svc_serverinfo, write_svc_signonnum,
    write_svc_stufftext,
};
use cs_proto48::netchan::{FRAGMENT_S2C_MAX_SIZE, NetChan};
use cs_transport::{LoopbackEndpoint, LoopbackHub, PacketTransport, PeerAddr};

const SPAWN_COUNT: u32 = 3;
const MAP_CRC: u32 = 0xDEAD_BEEF;
const STEP: f64 = 0.01;

/// The test plays the server by hand on the hub side of a loopback.
struct Server {
    hub: LoopbackHub,
    client: PeerAddr,
    chan: NetChan,
    reg: DeltaRegistry,
}

fn config() -> ConnectionConfig {
    ConnectionConfig::new("core", 76_561_197_960_265_729, &md5_hex(b"cs-clientcore test key"))
}

fn new_pair(cfg: ConnectionConfig) -> (Server, Connection<LoopbackEndpoint>) {
    let hub = LoopbackHub::new();
    let endpoint = hub.connect();
    let client = PeerAddr::Loopback(endpoint.id());
    let conn = Connection::new(endpoint, PeerAddr::Loopback(0), cfg);
    let server = Server {
        hub,
        client,
        chan: NetChan::new(FRAGMENT_S2C_MAX_SIZE),
        reg: DeltaRegistry::cs16_default(),
    };
    (server, conn)
}

impl Server {
    fn send_raw(&mut self, data: &[u8]) {
        self.hub.send(self.client, data).unwrap();
    }

    fn recv_raw(&mut self) -> Vec<Vec<u8>> {
        std::iter::from_fn(|| self.hub.recv().map(|(_, data)| data)).collect()
    }

    /// Every netchan packet the client sent since the last call, decoded.
    fn recv_clc(&mut self) -> Vec<Vec<ClcMessage>> {
        let schema = self.reg.get("usercmd_t").unwrap().clone();
        self.recv_raw()
            .iter()
            .filter_map(|d| self.chan.process_packet(d).unwrap())
            .map(|inc| parse_clc_stream(&inc.payload, &schema, inc.sequence).unwrap())
            .collect()
    }

    fn send_reliable(&mut self, reliable: &[u8]) {
        self.chan.write_reliable(reliable);
        let packet = self.chan.build_packet(&[]);
        self.send_raw(&packet);
    }
}

/// A netchan packet that carries `stream` as a one-piece file stream (`FRAG_FILE_STREAM`).
fn file_packet(server: &mut Server, stream: &[u8]) -> Vec<u8> {
    let seq = server.chan.outgoing_sequence;
    server.chan.outgoing_sequence += 1;
    let w1 = seq | (1 << 30);
    let w2 = server.chan.incoming_sequence | (server.chan.incoming_reliable_sequence << 31);
    let mut w = MsgWriter::new();
    w.write_u8(0); // no normal stream piece
    w.write_u8(1);
    w.write_u32(cs_proto48::netchan::make_frag_id(1, 1));
    w.write_u16(0);
    w.write_u16(stream.len() as u16);
    w.write_bytes(stream);
    let mut body = w.into_bytes();
    com_munge2(&mut body, (seq & 0xFF) as i32);
    let mut packet = w1.to_le_bytes().to_vec();
    packet.extend_from_slice(&w2.to_le_bytes());
    packet.extend_from_slice(&body);
    packet
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

fn msg(write: impl FnOnce(&mut MsgWriter)) -> Vec<u8> {
    let mut w = MsgWriter::new();
    write(&mut w);
    w.into_bytes()
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

/// Plays the handshake until the client is in `Signon` and has sent `new`; returns the time.
fn handshake(server: &mut Server, conn: &mut Connection<LoopbackEndpoint>) -> f64 {
    conn.connect();
    conn.pump(0.0);
    server.recv_raw();
    server.send_raw(&encode_s2c_challenge(0x1234_5678, 0, false));
    conn.pump(STEP);
    server.recv_raw();
    server.send_raw(&encode_s2c_connection(1, "loopback", false, 6153));
    conn.pump(2.0 * STEP);
    assert_eq!(conn.state(), &ConnState::Signon);
    assert_eq!(string_cmds(&server.recv_clc()), vec!["new".to_string()]);
    2.0 * STEP
}

/// Sends `reliable`, lets the client answer once and returns what it sent.
fn exchange(
    server: &mut Server,
    conn: &mut Connection<LoopbackEndpoint>,
    now: &mut f64,
    reliable: &[u8],
) -> Vec<Vec<ClcMessage>> {
    server.send_reliable(reliable);
    *now += STEP;
    conn.pump(*now);
    server.recv_clc()
}

/// `svc_serverinfo` and the request for the resource list.
fn up_to_resource_request(
    server: &mut Server,
    conn: &mut Connection<LoopbackEndpoint>,
    now: &mut f64,
) {
    let reply = exchange(
        server,
        conn,
        now,
        &msg(|w| write_svc_serverinfo(w, &server_info())),
    );
    assert_eq!(string_cmds(&reply), vec!["sendres".to_string()]);
    let request = msg(|w| write_svc_resourcerequest(w, SPAWN_COUNT as i32));
    exchange(server, conn, now, &request);
}

fn entry(res_type: u8, name: &str, index: u16, size: u32, flags: u8) -> ResourceEntry {
    ResourceEntry {
        res_type,
        filename: name.to_string(),
        index,
        download_size: size,
        flags,
        md5: None,
    }
}

/// `svc_resourcelist` with a consistency list of the given resource positions.
fn resource_list(resources: &[ResourceEntry], consistency: &[u32]) -> Vec<u8> {
    msg(|w| {
        w.write_u8(svc::RESOURCELIST);
        w.start_bit_writing();
        w.write_bits(resources.len() as u32, 12);
        for r in resources {
            w.write_bits(u32::from(r.res_type), 4);
            w.write_bit_string(&r.filename);
            w.write_bits(u32::from(r.index), 12);
            w.write_bits(r.download_size, 24);
            w.write_bits(u32::from(r.flags & 7), 3);
            w.write_one_bit(false);
        }
        w.write_one_bit(!consistency.is_empty());
        for &index in consistency {
            w.write_one_bit(true);
            w.write_one_bit(false); // absolute index
            w.write_bits(index, 10);
        }
        if !consistency.is_empty() {
            w.write_one_bit(false);
        }
        w.end_bit_writing();
    })
}

struct GameFiles {
    have: Vec<&'static str>,
}

impl ResourceFiles for GameFiles {
    fn exists(&self, path: &str) -> bool {
        self.have.contains(&path)
    }
    fn md5(&self, path: &str) -> Option<[u8; 16]> {
        self.exists(path).then(|| {
            let mut h = [0u8; 16];
            h[..4].copy_from_slice(&(path.len() as u32).to_le_bytes());
            h
        })
    }
}

#[test]
fn fills_the_precache_tables_as_the_server_sent_them() {
    let (mut server, mut conn) = new_pair(config());
    let mut now = handshake(&mut server, &mut conn);
    up_to_resource_request(&mut server, &mut conn, &mut now);

    let list = [
        entry(res_type::MODEL, "maps/de_dust2.bsp", 1, 1024, 1),
        entry(res_type::MODEL, "models/p_ak47.mdl", 7, 100, 0),
        entry(res_type::SOUND, "weapons/ak47-1.wav", 4, 50, 0),
        entry(res_type::EVENTSCRIPT, "events/ak47.sc", 2, 10, 0),
        entry(res_type::GENERIC, "gfx/env/desertbk.tga", 1, 8, 0),
        entry(res_type::DECAL, "{shot1", 3, 0, 0),
    ];
    let reply = exchange(&mut server, &mut conn, &mut now, &resource_list(&list, &[]));
    let tables = conn.precache();
    assert_eq!(tables.world_model(), Some("maps/de_dust2.bsp"));
    assert_eq!(tables.model(7), Some("models/p_ak47.mdl"));
    assert_eq!(tables.sound(4), Some("weapons/ak47-1.wav"));
    assert_eq!(tables.event(2), Some("events/ak47.sc"));
    assert_eq!(tables.generic(1), Some("gfx/env/desertbk.tga"));
    assert_eq!(tables.decals[&3].name, "{shot1");
    assert_eq!(tables.models.len(), 2);
    // Every file was there (no file view attached), so the client goes on to spawn.
    let cmds = string_cmds(&reply);
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert!(cmds[0].starts_with("spawn 3 "));
    assert_eq!(conn.resources().needed().len(), 0);

    // A `reconnect` starts the next map with empty tables.
    exchange(&mut server, &mut conn, &mut now, &msg(|w| write_svc_stufftext(w, "reconnect\n")));
    assert!(conn.precache().models.is_empty());
}

#[test]
fn missing_files_are_requested_then_the_client_spawns_when_they_arrive() {
    let (mut server, mut conn) = new_pair(config());
    conn.set_files(Box::new(GameFiles {
        have: vec!["maps/de_dust2.bsp"],
    }));
    let mut now = handshake(&mut server, &mut conn);
    up_to_resource_request(&mut server, &mut conn, &mut now);

    let list = [
        entry(res_type::MODEL, "maps/de_dust2.bsp", 1, 1024, 1),
        entry(res_type::MODEL, "models/extra.mdl", 7, 100, 0),
        entry(res_type::SOUND, "extra/boom.wav", 4, 50, 0),
    ];
    let reply = exchange(&mut server, &mut conn, &mut now, &resource_list(&list, &[]));
    assert_eq!(
        string_cmds(&reply),
        vec!["dlfile models/extra.mdl", "dlfile sound/extra/boom.wav"]
    );
    assert_eq!(conn.resources().pending_downloads(), 2);

    // The files come as file streams: name, compressor, size, data.
    for (name, _) in [("models/extra.mdl", 0), ("sound/extra/boom.wav", 1)] {
        let mut stream = format!("{name}\0\0").into_bytes();
        stream.extend_from_slice(&4u32.to_le_bytes());
        stream.extend_from_slice(b"data");
        let packet = file_packet(&mut server, &stream);
        server.send_raw(&packet);
        now += STEP;
        conn.pump(now);
    }
    let cmds = string_cmds(&server.recv_clc());
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    assert!(cmds[0].starts_with("spawn 3 "), "{cmds:?}");
}

#[test]
fn a_fatal_missing_file_with_downloads_off_ends_the_connection_with_its_name() {
    let (mut server, mut conn) = new_pair(config());
    conn.set_files(Box::new(GameFiles { have: vec![] }));
    conn.set_cvar("cl_allowdownload", "0");
    let mut now = handshake(&mut server, &mut conn);
    up_to_resource_request(&mut server, &mut conn, &mut now);
    let list = [entry(res_type::MODEL, "maps/de_dust2.bsp", 1, 1024, 1)];
    let reply = exchange(&mut server, &mut conn, &mut now, &resource_list(&list, &[]));
    assert!(string_cmds(&reply).is_empty());
    let ConnState::Disconnected(reason) = conn.state() else {
        panic!("expected Disconnected, got {:?}", conn.state());
    };
    assert!(reason.contains("maps/de_dust2.bsp"), "{reason}");
}

#[test]
fn answers_consistency_as_the_original() {
    let (mut server, mut conn) = new_pair(config());
    conn.set_files(Box::new(GameFiles {
        have: vec!["maps/de_dust2.bsp", "models/p_ak47.mdl"],
    }));
    let mut now = handshake(&mut server, &mut conn);
    up_to_resource_request(&mut server, &mut conn, &mut now);

    let list = [
        entry(res_type::MODEL, "maps/de_dust2.bsp", 1, 1024, 1),
        entry(res_type::MODEL, "models/p_ak47.mdl", 7, 100, 0),
    ];
    let reply = exchange(&mut server, &mut conn, &mut now, &resource_list(&list, &[1]));
    let flat: Vec<&ClcMessage> = reply.iter().flatten().collect();
    let consistency = flat
        .iter()
        .position(|m| matches!(m, ClcMessage::FileConsistency(_)))
        .expect("clc_fileconsistency expected");
    let spawn = flat
        .iter()
        .position(|m| matches!(m, ClcMessage::StringCmd(s) if s.starts_with("spawn")))
        .expect("spawn expected");
    assert!(consistency < spawn, "the answer goes before spawn: {flat:?}");

    let ClcMessage::FileConsistency(payload) = flat[consistency] else {
        unreachable!();
    };
    // Munged with the spawn count; bits: 1, index (10), md5 prefix (32), 0.
    let mut payload = payload.clone();
    com_unmunge(&mut payload, SPAWN_COUNT as i32);
    let mut r = MsgReader::new(&payload);
    r.start_bit_reading();
    assert!(r.read_one_bit());
    assert_eq!(r.read_bits(10), 1);
    assert_eq!(r.read_bits(32), "models/p_ak47.mdl".len() as u32);
    assert!(!r.read_one_bit());
}

#[test]
fn the_probe_policy_leaves_a_server_that_demands_consistency() {
    let mut cfg = config();
    cfg.consistency = ConsistencyPolicy::Refuse;
    let (mut server, mut conn) = new_pair(cfg);
    let mut now = handshake(&mut server, &mut conn);
    up_to_resource_request(&mut server, &mut conn, &mut now);
    let list = [entry(res_type::MODEL, "maps/de_dust2.bsp", 1, 1024, 1)];
    let reply = exchange(&mut server, &mut conn, &mut now, &resource_list(&list, &[0]));
    assert!(string_cmds(&reply).iter().all(|c| !c.starts_with("spawn")));
    assert!(matches!(conn.state(), ConnState::Disconnected(_)));
}

#[test]
fn stufftext_runs_allowed_commands_only() {
    let (mut server, mut conn) = new_pair(config());
    conn.register_command("spk", 0);
    let mut now = handshake(&mut server, &mut conn);

    let text = "echo hello; spk \"vox/ready\"; say_team hi; quit; cl_lw 0; name stolen; \
                setinfo evil 1; fullserverinfo \"\\\\hostname\\\\x\"; sv_gravity 800\n";
    let reply = exchange(&mut server, &mut conn, &mut now, &msg(|w| write_svc_stufftext(w, text)));

    // `echo` ran, `spk` went to the shell, unknown commands were forwarded to the server.
    assert!(conn.console_log().iter().any(|l| l == "hello"));
    let shell = conn.take_shell_commands();
    assert_eq!(shell.len(), 1);
    assert_eq!(shell[0].name, "spk");
    assert_eq!(shell[0].args, vec!["vox/ready"]);
    let cmds = string_cmds(&reply);
    assert!(cmds.contains(&"say_team hi".to_string()), "{cmds:?}");
    assert!(cmds.contains(&"sv_gravity 800".to_string()), "{cmds:?}");
    // Restricted and filtered commands and cvars did nothing.
    assert!(!cmds.iter().any(|c| c.contains("quit") || c.contains("evil")));
    assert_eq!(conn.cvars().string("cl_lw"), "1");
    assert_eq!(conn.cvars().string("name"), "core");
    assert!(conn.userinfo().get("evil").is_none());
    let log: Vec<&String> = conn.console_log().iter().collect();
    assert!(log.iter().any(|l| l.contains("privileged command quit")), "{log:?}");
    assert!(log.iter().any(|l| l.contains("cl_lw is a privileged variable")), "{log:?}");
    assert!(log.iter().any(|l| l.contains("name is a privileged variable")), "{log:?}");
    assert!(log.iter().any(|l| l.contains("privileged command setinfo")), "{log:?}");
    assert_eq!(conn.full_server_info(), "\\hostname\\x");

    // With the filter off the filterable cvar goes through; privileged things still do not.
    conn.set_cvar("cl_filterstuffcmd", "0");
    let reply = exchange(
        &mut server,
        &mut conn,
        &mut now,
        &msg(|w| write_svc_stufftext(w, "name renamed; cl_lw 0; quit\n")),
    );
    assert_eq!(conn.cvars().string("name"), "renamed");
    assert_eq!(conn.cvars().string("cl_lw"), "0");
    // The userinfo cvars told the server.
    let cmds = string_cmds(&reply);
    assert!(cmds.contains(&"setinfo \"name\" \"renamed\"\n".to_string()), "{cmds:?}");
    assert!(cmds.contains(&"setinfo \"cl_lw\" \"0\"\n".to_string()), "{cmds:?}");
    assert!(conn.take_shell_commands().is_empty());
}

#[test]
fn server_prints_and_cvar_queries_are_answered() {
    let (mut server, mut conn) = new_pair(config());
    let mut now = handshake(&mut server, &mut conn);
    let query = msg(|w| {
        write_svc_print(w, "welcome\n");
        w.write_u8(svc::SENDCVARVALUE2);
        w.write_i32(77);
        w.write_str("cl_updaterate");
        w.write_u8(svc::SENDCVARVALUE2);
        w.write_i32(78);
        w.write_str("cl_filterstuffcmd");
        w.write_u8(svc::SENDCVARVALUE2);
        w.write_i32(79);
        w.write_str("no_such_cvar");
        w.write_u8(svc::SENDCVARVALUE);
        w.write_str("sv_version");
    });
    let reply = exchange(&mut server, &mut conn, &mut now, &query);
    assert!(conn.console_log().iter().any(|l| l == "welcome"));
    let answers: Vec<&ClcMessage> = reply
        .iter()
        .flatten()
        .filter(|m| matches!(m, ClcMessage::CvarValue2 { .. } | ClcMessage::CvarValue(_)))
        .collect();
    let cvar2 = |id: i32, name: &str, value: &str| ClcMessage::CvarValue2 {
        request_id: id,
        cvar_name: name.to_string(),
        value: value.to_string(),
    };
    assert_eq!(answers[0], &cvar2(77, "cl_updaterate", "60"));
    assert_eq!(answers[1], &cvar2(78, "cl_filterstuffcmd", "CVAR is privileged"));
    assert_eq!(answers[2], &cvar2(79, "no_such_cvar", "Bad CVAR request"));
    assert_eq!(answers[3], &ClcMessage::CvarValue("1.1.2.2/Stdio,48,10211".to_string()));
}

/// Netchan packets the client sends over `seconds` of 1 ms steps with the given `cl_cmdrate`.
fn packets_over(rate: &str, seconds: usize) -> usize {
    let (mut server, mut conn) = new_pair(config());
    let mut now = handshake(&mut server, &mut conn);
    server.recv_raw();
    conn.set_cvar("cl_cmdrate", rate);
    let mut count = 0;
    for _ in 0..seconds * 1000 {
        now += 0.001;
        conn.pump(now);
        count += server.recv_raw().len();
    }
    count
}

#[test]
fn cmd_rate_bounds_clc_move() {
    // A rate under 10 is raised to 10, one over 100 lowered to 100, a rate in range is kept.
    // The window opens after the first packet, so a count is the rate give or take one.
    let within = |count: usize, rate: usize| count + 1 >= rate && count <= rate + 1;
    let low = packets_over("1", 2);
    assert!(within(low, 20), "{low}");
    let high = packets_over("1000", 1);
    assert!(within(high, MAX_CMDRATE as usize), "{high}");
    let mid = packets_over("30", 1);
    assert!(within(mid, 30), "{mid}");
    assert_eq!(MIN_CMDRATE as usize, 10);
}

#[test]
fn cmd_rate_is_clamped_in_the_cvar_too() {
    let (mut server, mut conn) = new_pair(config());
    let mut now = handshake(&mut server, &mut conn);
    conn.set_cvar("cl_cmdrate", "5000");
    now += 0.5;
    conn.pump(now);
    assert_eq!(conn.cvars().string("cl_cmdrate"), "100");
    conn.set_cvar("cl_cmdrate", "0");
    now += 0.5;
    conn.pump(now);
    assert_eq!(conn.cvars().string("cl_cmdrate"), "10");
}

#[test]
fn signon_number_is_tracked_and_userinfo_follows_the_cvars() {
    let (mut server, mut conn) = new_pair(config());
    let mut now = handshake(&mut server, &mut conn);
    exchange(&mut server, &mut conn, &mut now, &msg(|w| write_svc_signonnum(w, 1)));
    assert_eq!(conn.signon_num(), 1);
    assert_eq!(conn.userinfo().get("name"), Some("core"));
    assert_eq!(conn.userinfo().get("cl_updaterate"), Some("60"));
    assert_eq!(conn.userinfo().get("_vgui_menus"), Some("1"));
}
