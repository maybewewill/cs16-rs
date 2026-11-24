//! Live loopback integration tests for `cs-relay`: allowlist enforcement (403) and full
//! Protocol 48 sign-on through `WsClientTransport -> RelayServer -> UdpTransport -> cs-engine`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use cs_clientcore::{ConnState, Connection, ConnectionConfig};
use cs_cvar::CVarRegistry;
use cs_engine::{Server, ServerConfig, ServerMap};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::DeltaRegistry;
use cs_relay::{RelayPolicy, RelayServer, TargetAllowlist};
use cs_sim::pmove::MoveParams;
use cs_sim::walk_game::WalkGame;
use cs_transport::{PeerAddr, UdpTransport, WsClientTransport};

#[test]
fn relay_rejects_disallowed_target_with_403() {
    let allow: std::net::SocketAddr = "127.0.0.1:27901".parse().unwrap();
    let policy = RelayPolicy {
        allowlist: TargetAllowlist::from_addrs([allow]),
        ..RelayPolicy::default()
    };
    let relay = RelayServer::bind("127.0.0.1:0", policy).unwrap();
    let relay_addr = relay.local_addr();

    let mut tcp = TcpStream::connect(relay_addr).unwrap();
    let req = "GET /relay?target=127.0.0.1:27999 HTTP/1.1\r\n\
               Host: localhost\r\n\
               Upgrade: websocket\r\n\
               Connection: Upgrade\r\n\
               Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
    tcp.write_all(req.as_bytes()).unwrap();
    tcp.flush().unwrap();
    let mut resp = String::new();
    tcp.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 403"), "{resp}");
}

#[test]
fn relay_bridges_ws_client_to_udp_protocol48_server() {
    let mut udp = UdpTransport::bind("127.0.0.1:0").unwrap();
    let udp_addr = udp.local_addr();
    let mut server = Server::new(
        ServerConfig {
            max_clients: 4,
            ..ServerConfig::default()
        },
        ServerMap::stub("de_dust2"),
        WalkGame::new(MoveParams::default()),
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );

    let policy = RelayPolicy {
        allowlist: TargetAllowlist::from_addrs([udp_addr]),
        ..RelayPolicy::default()
    };
    let relay = RelayServer::bind("127.0.0.1:0", policy).unwrap();
    let relay_addr = relay.local_addr();

    let url = format!("ws://{relay_addr}/relay?target={udp_addr}");
    let ws = WsClientTransport::connect(&url).unwrap();
    let cfg = ConnectionConfig::new(
        "relay-player",
        76_561_197_960_265_731,
        &md5_hex(b"cs-relay test"),
    );
    let mut conn = Connection::new(ws, PeerAddr::Web(0), cfg);
    conn.connect();

    let mut now = 0.0;
    for _ in 0..800 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut udp, now);
        if conn.state() == &ConnState::InGame {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    assert_eq!(conn.state(), &ConnState::InGame, "{:?}", conn.messages());
    assert_eq!(conn.stats().parse_errors, 0);
    assert!(conn.stats().full_updates >= 1);
}
