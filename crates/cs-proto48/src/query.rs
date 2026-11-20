//! Valve Source / GoldSrc Server Query Protocol (`A2S_INFO`, `A2S_PLAYER`, `A2S_RULES`,
//! `A2A_PING`, `S2A_INFO_SRC` `'I'`, legacy `S2A_INFO_DETAILED` `'m'`, and A2S challenge).

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::bitbuf::{MsgReader, MsgWriter, ProtoError};
use crate::{CONNECTIONLESS_HEADER, PROTOCOL_VERSION};

pub const A2S_INFO: u8 = b'T';
pub const A2S_PLAYER: u8 = b'U';
pub const A2S_RULES: u8 = b'V';
pub const A2A_PING: u8 = b'i';
pub const A2A_ACK: u8 = b'j';
pub const S2C_CHALLENGE_QUERY: u8 = b'A';
pub const S2A_INFO_SRC: u8 = b'I';
pub const S2A_INFO_DETAILED: u8 = b'm';
pub const S2A_PLAYERS: u8 = b'D';
pub const S2A_RULES: u8 = b'E';

pub const A2S_INFO_PAYLOAD: &[u8] = b"Source Engine Query\0";

/// Parsed server information from `S2A_INFO_SRC` (`'I'`) or legacy `S2A_INFO_DETAILED` (`'m'`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerQueryInfo {
    pub addr: String,
    pub protocol: u8,
    pub name: String,
    pub map: String,
    pub folder: String,
    pub game: String,
    pub app_id: u16,
    pub players: u8,
    pub max_players: u8,
    pub bots: u8,
    pub server_type: u8,
    pub environment: u8,
    pub password_protected: bool,
    pub vac_secure: bool,
    pub version: String,
    pub ping_ms: u32,
}

/// Player row returned by `S2A_PLAYERS` (`'D'`).
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerQueryEntry {
    pub index: u8,
    pub name: String,
    pub score: i32,
    pub duration_secs: f32,
}

/// Build an `A2S_INFO` query datagram (optionally appending a 4-byte A2S challenge).
pub fn encode_a2s_info_request(challenge: Option<i32>) -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(32);
    w.write_i32(CONNECTIONLESS_HEADER);
    w.write_u8(A2S_INFO);
    w.write_bytes(A2S_INFO_PAYLOAD);
    if let Some(ch) = challenge {
        w.write_i32(ch);
    }
    w.into_bytes()
}

/// Build an `A2S_PLAYER` query datagram (`challenge = -1` for initial request).
pub fn encode_a2s_player_request(challenge: i32) -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(12);
    w.write_i32(CONNECTIONLESS_HEADER);
    w.write_u8(A2S_PLAYER);
    w.write_i32(challenge);
    w.into_bytes()
}

/// Build an `A2A_PING` request datagram.
pub fn encode_a2a_ping() -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(6);
    w.write_i32(CONNECTIONLESS_HEADER);
    w.write_u8(A2A_PING);
    w.write_u8(0);
    w.into_bytes()
}

/// Encode a modern Valve/Steam `S2A_INFO_SRC` (`'I'`) response understood by both Steam
/// Server Browser and CS 1.6 `Find Servers -> LAN`.
pub fn encode_s2a_info_src(info: &ServerQueryInfo) -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(160);
    w.write_i32(CONNECTIONLESS_HEADER);
    w.write_u8(S2A_INFO_SRC);
    w.write_u8(info.protocol);
    w.write_str(&info.name);
    w.write_str(&info.map);
    w.write_str(&info.folder);
    w.write_str(&info.game);
    w.write_u16(info.app_id);
    w.write_u8(info.players);
    w.write_u8(info.max_players);
    w.write_u8(info.bots);
    w.write_u8(info.server_type);
    w.write_u8(info.environment);
    w.write_u8(u8::from(info.password_protected));
    w.write_u8(u8::from(info.vac_secure));
    w.write_str(&info.version);
    w.into_bytes()
}

/// Encode an `S2A_PLAYERS` (`'D'`) response.
pub fn encode_s2a_players(players: &[PlayerQueryEntry]) -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(16 + players.len() * 32);
    w.write_i32(CONNECTIONLESS_HEADER);
    w.write_u8(S2A_PLAYERS);
    w.write_u8(players.len().min(255) as u8);
    for p in players.iter().take(255) {
        w.write_u8(p.index);
        w.write_str(&p.name);
        w.write_i32(p.score);
        w.write_f32(p.duration_secs);
    }
    w.into_bytes()
}

/// Encode an `S2A_RULES` (`'E'`) response.
pub fn encode_s2a_rules(rules: &[(&str, &str)]) -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(32 + rules.len() * 24);
    w.write_i32(CONNECTIONLESS_HEADER);
    w.write_u8(S2A_RULES);
    w.write_u16(rules.len().min(u16::MAX as usize) as u16);
    for (k, v) in rules {
        w.write_str(k);
        w.write_str(v);
    }
    w.into_bytes()
}

/// Decoded response to an `A2S_INFO` probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A2sInfoReply {
    Challenge(i32),
    Info(ServerQueryInfo),
}

/// Decode either an A2S challenge (`'A'`), `S2A_INFO_SRC` (`'I'`), or legacy
/// `S2A_INFO_DETAILED` (`'m'`).
pub fn decode_a2s_info_reply(
    packet: &[u8],
    addr: &str,
    ping_ms: u32,
) -> Result<A2sInfoReply, ProtoError> {
    let mut r = MsgReader::new(packet);
    let hdr = r.read_i32()?;
    if hdr != CONNECTIONLESS_HEADER {
        return Err(ProtoError::InvalidPayload(
            "missing 0xFFFFFFFF connectionless header".to_string(),
        ));
    }
    let kind = r.read_u8()?;
    match kind {
        S2C_CHALLENGE_QUERY => Ok(A2sInfoReply::Challenge(r.read_i32()?)),
        S2A_INFO_SRC => {
            let protocol = r.read_u8()?;
            let name = r.read_str()?;
            let map = r.read_str()?;
            let folder = r.read_str()?;
            let game = r.read_str()?;
            let app_id = r.read_u16()?;
            let players = r.read_u8()?;
            let max_players = r.read_u8()?;
            let bots = r.read_u8()?;
            let server_type = r.read_u8()?;
            let environment = r.read_u8()?;
            let password_protected = r.read_u8()? != 0;
            let vac_secure = r.read_u8()? != 0;
            let version = r.read_str().unwrap_or_default();
            Ok(A2sInfoReply::Info(ServerQueryInfo {
                addr: addr.to_string(),
                protocol,
                name,
                map,
                folder,
                game,
                app_id,
                players,
                max_players,
                bots,
                server_type,
                environment,
                password_protected,
                vac_secure,
                version,
                ping_ms,
            }))
        }
        S2A_INFO_DETAILED => {
            let _reported_addr = r.read_str()?;
            let name = r.read_str()?;
            let map = r.read_str()?;
            let folder = r.read_str()?;
            let game = r.read_str()?;
            let players = r.read_u8()?;
            let max_players = r.read_u8()?;
            let protocol = r.read_u8()?;
            let server_type = r.read_u8().unwrap_or(b'd');
            let environment = r.read_u8().unwrap_or(b'w');
            let password_protected = r.read_u8().unwrap_or(0) != 0;
            let is_mod = r.read_u8().unwrap_or(0) != 0;
            if is_mod {
                let _ = r.read_str();
                let _ = r.read_str();
                let _ = r.read_str();
                let _ = r.read_i32();
                let _ = r.read_i32();
                let _ = r.read_u8();
                let _ = r.read_u8();
            }
            let vac_secure = r.read_u8().unwrap_or(0) != 0;
            let bots = r.read_u8().unwrap_or(0);
            Ok(A2sInfoReply::Info(ServerQueryInfo {
                addr: addr.to_string(),
                protocol,
                name,
                map,
                folder,
                game,
                app_id: 10,
                players,
                max_players,
                bots,
                server_type,
                environment,
                password_protected,
                vac_secure,
                version: "1.1.2.7/Stdio".to_string(),
                ping_ms,
            }))
        }
        other => Err(ProtoError::UnknownOpcode(other)),
    }
}

/// Query a single GoldSrc / CS 1.6 server via `A2S_INFO`, automatically handling
/// Valve's 2020 A2S challenge-response retry.
pub fn query_server_info(addr: SocketAddr, timeout: Duration) -> Result<ServerQueryInfo, ProtoError> {
    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_read_timeout(Some(timeout))?;

    let req = encode_a2s_info_request(None);
    let start = Instant::now();
    sock.send_to(&req, addr)?;

    let mut buf = [0u8; 2048];
    for _ in 0..2 {
        let (len, _) = sock.recv_from(&mut buf)?;
        let ping_ms = start.elapsed().as_millis().max(1) as u32;
        match decode_a2s_info_reply(&buf[..len], &addr.to_string(), ping_ms)? {
            A2sInfoReply::Info(info) => return Ok(info),
            A2sInfoReply::Challenge(ch) => {
                let retry = encode_a2s_info_request(Some(ch));
                sock.send_to(&retry, addr)?;
            }
        }
    }

    Err(ProtoError::InvalidPayload(
        "server requested A2S challenge twice".to_string(),
    ))
}

/// Broadcast `A2S_INFO` across localhost and LAN ports `27015..=27020` to discover both
/// real Valve HLDS / ReHLDS CS 1.6 servers and `cs-server` instances.
pub fn discover_goldsrc_lan_servers(timeout: Duration) -> Vec<ServerQueryInfo> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
        return Vec::new();
    };
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(25)));

    let req = encode_a2s_info_request(None);
    let start = Instant::now();
    for port in 27015..=27020u16 {
        let _ = sock.send_to(&req, format!("127.0.0.1:{port}"));
        let _ = sock.send_to(&req, format!("255.255.255.255:{port}"));
    }

    let mut found: HashMap<String, ServerQueryInfo> = HashMap::new();
    let mut buf = [0u8; 2048];
    while start.elapsed() < timeout {
        if let Ok((len, src)) = sock.recv_from(&mut buf) {
            let ping_ms = start.elapsed().as_millis().max(1) as u32;
            match decode_a2s_info_reply(&buf[..len], &src.to_string(), ping_ms) {
                Ok(A2sInfoReply::Info(info)) => {
                    found.insert(info.addr.clone(), info);
                }
                Ok(A2sInfoReply::Challenge(ch)) => {
                    let retry = encode_a2s_info_request(Some(ch));
                    let _ = sock.send_to(&retry, src);
                }
                Err(_) => {}
            }
        }
    }

    let mut list: Vec<ServerQueryInfo> = found.into_values().collect();
    list.sort_by_key(|s| (s.ping_ms, s.addr.clone()));
    list
}

/// Build a default CS 1.6 `ServerQueryInfo` template for `cs-server`.
pub fn cs16_server_query_info(
    hostname: &str,
    map: &str,
    players: usize,
    max_players: usize,
    bots: usize,
) -> ServerQueryInfo {
    ServerQueryInfo {
        addr: String::new(),
        protocol: PROTOCOL_VERSION as u8,
        name: hostname.to_string(),
        map: map.to_string(),
        folder: "cstrike".to_string(),
        game: "Counter-Strike".to_string(),
        app_id: 10,
        players: players.min(255) as u8,
        max_players: max_players.min(255) as u8,
        bots: bots.min(255) as u8,
        server_type: b'd',
        environment: if cfg!(windows) { b'w' } else { b'l' },
        password_protected: false,
        vac_secure: false,
        version: "1.1.2.7/Stdio".to_string(),
        ping_ms: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a2s_info_src_encodes_and_decodes_cleanly() {
        let info = cs16_server_query_info("cs16-rs Dedicated", "de_dust2", 10, 32, 9);
        let wire = encode_s2a_info_src(&info);
        let reply = decode_a2s_info_reply(&wire, "127.0.0.1:27015", 3).unwrap();
        match reply {
            A2sInfoReply::Info(decoded) => {
                assert_eq!(decoded.protocol, 48);
                assert_eq!(decoded.name, "cs16-rs Dedicated");
                assert_eq!(decoded.map, "de_dust2");
                assert_eq!(decoded.folder, "cstrike");
                assert_eq!(decoded.app_id, 10);
                assert_eq!(decoded.players, 10);
                assert_eq!(decoded.max_players, 32);
                assert_eq!(decoded.bots, 9);
            }
            other => panic!("unexpected reply: {other:?}"),
        }
    }
}
