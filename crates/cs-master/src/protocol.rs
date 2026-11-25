//! GoldSrc Master Server Query (`0x31`) and Heartbeat (`q` / `0` / `b`) wire protocol.
//!
//! References:
//! - GoldSrc / HLDS `Master_Heartbeat` / `Master_Shutdown` (`q`, `0\n...`, `b\n`)
//! - Valve Master Server Query Protocol (`0x31` query, `0xFFFFFFFF 0x66 0x0A` reply)

use std::collections::BTreeMap;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddrV4};

use thiserror::Error;

/// Master server query request opcode (`'1'`).
pub const MASTER_QUERY_OPCODE: u8 = 0x31;

/// Master server query reply header: `\xFF\xFF\xFF\xFF\x66\x0A`.
pub const MASTER_REPLY_HEADER: [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, 0x66, 0x0A];

/// Master server heartbeat challenge reply header: `\xFF\xFF\xFF\xFFs\n`.
pub const HEARTBEAT_CHALLENGE_HEADER: [u8; 6] = [0xFF, 0xFF, 0xFF, 0xFF, b's', 0x0A];

/// Connectionless 4-byte OOB prefix (`0xFFFFFFFF`).
pub const OOB_PREFIX: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

/// GoldSrc master region codes.
pub const REGION_US_EAST: u8 = 0x00;
pub const REGION_US_WEST: u8 = 0x01;
pub const REGION_SOUTH_AMERICA: u8 = 0x02;
pub const REGION_EUROPE: u8 = 0x03;
pub const REGION_ASIA: u8 = 0x04;
pub const REGION_AUSTRALIA: u8 = 0x05;
pub const REGION_MIDDLE_EAST: u8 = 0x06;
pub const REGION_AFRICA: u8 = 0x07;
pub const REGION_ALL: u8 = 0xFF;

/// Errors produced while parsing, validating, or serving master server packets.
#[derive(Debug, Error)]
pub enum MasterError {
    #[error("packet is truncated")]
    Truncated,

    #[error("invalid packet opcode: 0x{0:02x}")]
    InvalidOpcode(u8),

    #[error("invalid packet header")]
    InvalidHeader,

    #[error("invalid packet format: {0}")]
    InvalidPacket(String),

    #[error("invalid UTF-8 string in packet: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),

    #[error("invalid IPv4 socket address: {0}")]
    InvalidAddress(String),

    #[error("invalid query filter: {0}")]
    InvalidFilter(String),

    #[error("missing or invalid heartbeat challenge")]
    InvalidChallenge,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Decoded `0x31` master server query request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryRequest {
    pub region: u8,
    pub cursor: SocketAddrV4,
    pub filter: String,
}

impl QueryRequest {
    /// Returns `true` when the cursor represents the start of the list (`0.0.0.0:0`).
    pub fn is_start_cursor(&self) -> bool {
        self.cursor == SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)
    }
}

/// Parsed backslash key-value metadata from a `0\n...` server heartbeat packet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeartbeatInfo {
    pub protocol: Option<u8>,
    pub challenge: Option<u32>,
    pub players: Option<u8>,
    pub max_players: Option<u8>,
    pub bots: Option<u8>,
    pub gamedir: Option<String>,
    pub map: Option<String>,
    pub server_type: Option<String>,
    pub password: Option<bool>,
    pub os: Option<String>,
    pub secure: Option<bool>,
    pub lan: Option<bool>,
    pub version: Option<String>,
    pub region: Option<u8>,
    pub product: Option<String>,
    pub gametype: Option<String>,
    pub app_id: Option<u32>,
    pub fields: BTreeMap<String, String>,
}

/// Decoded heartbeat packet sent by a game server to the master server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeartbeatPacket {
    ChallengeRequest,
    Update(HeartbeatInfo),
    Shutdown,
}

/// Any request packet received on the master server UDP socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterRequest {
    Query(QueryRequest),
    Heartbeat(HeartbeatPacket),
}

/// Encode a `0x31` master server query request datagram.
pub fn encode_query_request(region: u8, cursor: impl fmt::Display, filter: &str) -> Vec<u8> {
    let cursor_str = cursor.to_string();
    let mut out = Vec::with_capacity(2 + cursor_str.len() + 1 + filter.len() + 1);
    out.push(MASTER_QUERY_OPCODE);
    out.push(region);
    for &b in cursor_str.as_bytes() {
        if b != 0 {
            out.push(b);
        }
    }
    out.push(0);
    for &b in filter.as_bytes() {
        if b != 0 {
            out.push(b);
        }
    }
    out.push(0);
    out
}

/// Parse a `0x31` master server query request datagram.
pub fn parse_query_request(data: &[u8]) -> Result<QueryRequest, MasterError> {
    if data.len() < 2 {
        return Err(MasterError::Truncated);
    }
    if data[0] != MASTER_QUERY_OPCODE {
        return Err(MasterError::InvalidOpcode(data[0]));
    }
    let region = data[1];
    let rest = &data[2..];
    let Some(cursor_end) = rest.iter().position(|&b| b == 0) else {
        return Err(MasterError::Truncated);
    };
    let cursor_str = std::str::from_utf8(&rest[..cursor_end])?;
    let cursor = if cursor_str.is_empty() {
        SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)
    } else {
        cursor_str
            .parse::<SocketAddrV4>()
            .map_err(|_| MasterError::InvalidAddress(cursor_str.to_string()))?
    };

    let after_cursor = &rest[cursor_end + 1..];
    let Some(filter_end) = after_cursor.iter().position(|&b| b == 0) else {
        return Err(MasterError::Truncated);
    };
    let filter = std::str::from_utf8(&after_cursor[..filter_end])?.to_string();

    Ok(QueryRequest {
        region,
        cursor,
        filter,
    })
}

/// Encode a master query reply packet (`\xFF\xFF\xFF\xFF\x66\x0A` + 6-byte big-endian entries).
///
/// When `has_more` is `false`, appends the trailing `0.0.0.0:0` (`[0u8; 6]`) sentinel.
pub fn encode_query_reply(addrs: &[SocketAddrV4], has_more: bool) -> Vec<u8> {
    let sentinel = if has_more { 0 } else { 6 };
    let mut out = Vec::with_capacity(MASTER_REPLY_HEADER.len() + addrs.len() * 6 + sentinel);
    out.extend_from_slice(&MASTER_REPLY_HEADER);
    for addr in addrs {
        out.extend_from_slice(&addr.ip().octets());
        out.extend_from_slice(&addr.port().to_be_bytes());
    }
    if !has_more {
        out.extend_from_slice(&[0u8; 6]);
    }
    out
}

/// Parse a master query reply packet, returning `(addrs, has_more)`.
///
/// `has_more` is `false` when the `0.0.0.0:0` terminator was encountered.
pub fn parse_query_reply(data: &[u8]) -> Result<(Vec<SocketAddrV4>, bool), MasterError> {
    if data.len() < MASTER_REPLY_HEADER.len() {
        return Err(MasterError::Truncated);
    }
    if data[..MASTER_REPLY_HEADER.len()] != MASTER_REPLY_HEADER {
        return Err(MasterError::InvalidHeader);
    }
    let payload = &data[MASTER_REPLY_HEADER.len()..];
    if payload.len() % 6 != 0 {
        return Err(MasterError::InvalidPacket(format!(
            "query reply payload length {} is not a multiple of 6",
            payload.len()
        )));
    }

    let mut addrs = Vec::with_capacity(payload.len() / 6);
    let mut has_more = true;
    for chunk in payload.chunks_exact(6) {
        if chunk == [0u8; 6] {
            has_more = false;
            break;
        }
        let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
        let port = u16::from_be_bytes([chunk[4], chunk[5]]);
        addrs.push(SocketAddrV4::new(ip, port));
    }

    Ok((addrs, has_more))
}

/// Encode a master heartbeat challenge reply (`\xFF\xFF\xFF\xFFs\n` + LE `u32`).
pub fn encode_heartbeat_challenge(challenge: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    out.extend_from_slice(&HEARTBEAT_CHALLENGE_HEADER);
    out.extend_from_slice(&challenge.to_le_bytes());
    out
}

/// Parse a master heartbeat challenge reply (`\xFF\xFF\xFF\xFFs\n` + LE `u32`).
pub fn parse_heartbeat_challenge(data: &[u8]) -> Result<u32, MasterError> {
    if data.len() < 10 {
        return Err(MasterError::Truncated);
    }
    if data[..6] != HEARTBEAT_CHALLENGE_HEADER {
        return Err(MasterError::InvalidHeader);
    }
    if data.len() != 10 {
        return Err(MasterError::InvalidPacket(format!(
            "expected 10-byte heartbeat challenge packet, got {}",
            data.len()
        )));
    }
    Ok(u32::from_le_bytes([data[6], data[7], data[8], data[9]]))
}

/// Encode a game server heartbeat challenge request (`"q"`).
pub fn encode_heartbeat_challenge_request() -> Vec<u8> {
    b"q".to_vec()
}

/// Encode a game server heartbeat update (`"0\n\\k\\v..."`).
pub fn encode_heartbeat_update(fields: &[(&str, &str)]) -> Vec<u8> {
    let mut out = String::from("0\n");
    for (k, v) in fields {
        if k.is_empty() {
            continue;
        }
        out.push('\\');
        out.push_str(&k.replace('\\', ""));
        out.push('\\');
        out.push_str(&v.replace('\\', ""));
    }
    out.push('\n');
    out.into_bytes()
}

/// Encode a game server shutdown packet (`"b\n"`).
pub fn encode_heartbeat_shutdown() -> Vec<u8> {
    b"b\n".to_vec()
}

/// Parse backslash-delimited key-value metadata into lowercase-keyed map.
pub fn parse_kv_pairs(raw: &str) -> BTreeMap<String, String> {
    let trimmed = raw.trim_matches(|c: char| c == '\0' || c == '\r' || c == '\n');
    let stripped = trimmed.strip_prefix('\\').unwrap_or(trimmed);
    let mut map = BTreeMap::new();
    if stripped.is_empty() {
        return map;
    }

    let mut iter = stripped.split('\\');
    while let (Some(key), Some(val)) = (iter.next(), iter.next()) {
        let key_clean = key.trim();
        if key_clean.is_empty() {
            continue;
        }
        let val_clean = val.trim_end_matches(['\0', '\r', '\n']);
        map.insert(key_clean.to_ascii_lowercase(), val_clean.to_string());
    }
    map
}

fn parse_bool_flag(s: &str) -> Option<bool> {
    match s.trim() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

fn parse_challenge_value(s: &str) -> Option<u32> {
    let trimmed = s.trim();
    if let Ok(u) = trimmed.parse::<u32>() {
        Some(u)
    } else if let Ok(i) = trimmed.parse::<i32>() {
        Some(i as u32)
    } else {
        None
    }
}

/// Parse a `0\n...` heartbeat update payload (after the leading `0` byte).
pub fn parse_heartbeat_update(kv_str: &str) -> HeartbeatInfo {
    let fields = parse_kv_pairs(kv_str);
    HeartbeatInfo {
        protocol: fields.get("protocol").and_then(|v| v.parse().ok()),
        challenge: fields.get("challenge").and_then(|v| parse_challenge_value(v)),
        players: fields.get("players").and_then(|v| v.parse().ok()),
        max_players: fields.get("max").and_then(|v| v.parse().ok()),
        bots: fields.get("bots").and_then(|v| v.parse().ok()),
        gamedir: fields.get("gamedir").cloned(),
        map: fields.get("map").cloned(),
        server_type: fields.get("type").cloned(),
        password: fields.get("password").and_then(|v| parse_bool_flag(v)),
        os: fields.get("os").cloned(),
        secure: fields.get("secure").and_then(|v| parse_bool_flag(v)),
        lan: fields.get("lan").and_then(|v| parse_bool_flag(v)),
        version: fields.get("version").cloned(),
        region: fields.get("region").and_then(|v| v.parse().ok()),
        product: fields.get("product").cloned(),
        gametype: fields.get("gametype").cloned(),
        app_id: fields.get("appid").and_then(|v| v.parse().ok()),
        fields,
    }
}

/// Parse a heartbeat datagram (`q`, `0\n...`, or `b` with optional `\xFF\xFF\xFF\xFF` prefix).
pub fn parse_heartbeat_packet(data: &[u8]) -> Result<HeartbeatPacket, MasterError> {
    if data.is_empty() {
        return Err(MasterError::Truncated);
    }

    let body = data.strip_prefix(&OOB_PREFIX).unwrap_or(data);
    if body.is_empty() {
        return Err(MasterError::Truncated);
    }

    match body[0] {
        b'q' => {
            let rest = &body[1..];
            if rest.iter().all(|&b| matches!(b, b'\0' | b'\r' | b'\n' | b' ')) {
                Ok(HeartbeatPacket::ChallengeRequest)
            } else {
                Err(MasterError::InvalidPacket(
                    "unexpected trailing bytes in challenge request".to_string(),
                ))
            }
        }
        b'b' => {
            let rest = &body[1..];
            if rest.iter().all(|&b| matches!(b, b'\0' | b'\r' | b'\n' | b' ')) {
                Ok(HeartbeatPacket::Shutdown)
            } else {
                Err(MasterError::InvalidPacket(
                    "unexpected trailing bytes in shutdown packet".to_string(),
                ))
            }
        }
        b'0' => {
            let rest = &body[1..];
            let kv_raw = std::str::from_utf8(rest)?;
            Ok(HeartbeatPacket::Update(parse_heartbeat_update(kv_raw)))
        }
        op => Err(MasterError::InvalidOpcode(op)),
    }
}

/// Parse any incoming master server UDP packet (either a `0x31` query or a heartbeat).
pub fn parse_master_request(data: &[u8]) -> Result<MasterRequest, MasterError> {
    if data.is_empty() {
        return Err(MasterError::Truncated);
    }
    if data[0] == MASTER_QUERY_OPCODE {
        Ok(MasterRequest::Query(parse_query_request(data)?))
    } else {
        Ok(MasterRequest::Heartbeat(parse_heartbeat_packet(data)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_request_roundtrip() {
        let filter = "\\gamedir\\cstrike\\map\\de_dust2\\empty\\1\\full\\1";
        let pkt = encode_query_request(REGION_EUROPE, "0.0.0.0:0", filter);
        assert_eq!(pkt[0], MASTER_QUERY_OPCODE);
        assert_eq!(pkt[1], REGION_EUROPE);

        let parsed = parse_query_request(&pkt).unwrap();
        assert_eq!(parsed.region, REGION_EUROPE);
        assert!(parsed.is_start_cursor());
        assert_eq!(parsed.filter, filter);
    }

    #[test]
    fn query_request_with_cursor_addr_and_empty_filter() {
        let cursor = "192.168.1.50:27015".parse::<SocketAddrV4>().unwrap();
        let pkt = encode_query_request(REGION_ALL, cursor, "");
        let parsed = parse_query_request(&pkt).unwrap();
        assert_eq!(parsed.region, REGION_ALL);
        assert_eq!(parsed.cursor, cursor);
        assert_eq!(parsed.filter, "");
        assert!(!parsed.is_start_cursor());
    }

    #[test]
    fn query_request_rejects_invalid_packets() {
        assert!(matches!(parse_query_request(&[]), Err(MasterError::Truncated)));
        assert!(matches!(
            parse_query_request(&[0x32, 0xFF]),
            Err(MasterError::InvalidOpcode(0x32))
        ));
        assert!(matches!(
            parse_query_request(b"1\xFF0.0.0.0:0"),
            Err(MasterError::Truncated)
        ));
        assert!(matches!(
            parse_query_request(b"1\xFF0.0.0.0:0\0\\gamedir\\cstrike"),
            Err(MasterError::Truncated)
        ));
        assert!(matches!(
            parse_query_request(b"1\xFFbad-addr\0\0"),
            Err(MasterError::InvalidAddress(_))
        ));
    }

    #[test]
    fn query_reply_roundtrip_end_and_paginated() {
        let addrs = vec![
            "10.0.0.1:27015".parse::<SocketAddrV4>().unwrap(),
            "192.168.0.2:27016".parse::<SocketAddrV4>().unwrap(),
        ];

        let final_pkt = encode_query_reply(&addrs, false);
        assert_eq!(&final_pkt[..6], &MASTER_REPLY_HEADER);
        assert_eq!(final_pkt.len(), 6 + 2 * 6 + 6);
        assert_eq!(&final_pkt[final_pkt.len() - 6..], &[0u8; 6]);
        let (decoded, has_more) = parse_query_reply(&final_pkt).unwrap();
        assert_eq!(decoded, addrs);
        assert!(!has_more);

        let page_pkt = encode_query_reply(&addrs, true);
        assert_eq!(page_pkt.len(), 6 + 2 * 6);
        let (decoded_page, has_more_page) = parse_query_reply(&page_pkt).unwrap();
        assert_eq!(decoded_page, addrs);
        assert!(has_more_page);

        let empty_pkt = encode_query_reply(&[], false);
        assert_eq!(empty_pkt.len(), 12);
        let (decoded_empty, has_more_empty) = parse_query_reply(&empty_pkt).unwrap();
        assert!(decoded_empty.is_empty());
        assert!(!has_more_empty);
    }

    #[test]
    fn query_reply_rejects_bad_header_or_length() {
        assert!(matches!(
            parse_query_reply(&[0xFF, 0xFF]),
            Err(MasterError::Truncated)
        ));
        assert!(matches!(
            parse_query_reply(&[0xFF, 0xFF, 0xFF, 0xFF, 0x65, 0x0A, 0, 0, 0, 0, 0, 0]),
            Err(MasterError::InvalidHeader)
        ));
        assert!(matches!(
            parse_query_reply(&[0xFF, 0xFF, 0xFF, 0xFF, 0x66, 0x0A, 1, 2, 3]),
            Err(MasterError::InvalidPacket(_))
        ));
    }

    #[test]
    fn heartbeat_challenge_roundtrip() {
        let ch = 0xDEAD_BEEF;
        let pkt = encode_heartbeat_challenge(ch);
        assert_eq!(pkt, [0xFF, 0xFF, 0xFF, 0xFF, b's', 0x0A, 0xEF, 0xBE, 0xAD, 0xDE]);
        assert_eq!(parse_heartbeat_challenge(&pkt).unwrap(), ch);
    }

    #[test]
    fn heartbeat_packets_parse_variants() {
        assert_eq!(
            parse_heartbeat_packet(b"q").unwrap(),
            HeartbeatPacket::ChallengeRequest
        );
        assert_eq!(
            parse_heartbeat_packet(b"q\n").unwrap(),
            HeartbeatPacket::ChallengeRequest
        );
        assert_eq!(
            parse_heartbeat_packet(b"q\0").unwrap(),
            HeartbeatPacket::ChallengeRequest
        );

        assert_eq!(
            parse_heartbeat_packet(b"b").unwrap(),
            HeartbeatPacket::Shutdown
        );
        assert_eq!(
            parse_heartbeat_packet(b"b\n").unwrap(),
            HeartbeatPacket::Shutdown
        );
        assert_eq!(
            parse_heartbeat_packet(b"\xFF\xFF\xFF\xFFb\n").unwrap(),
            HeartbeatPacket::Shutdown
        );

        let update = encode_heartbeat_update(&[
            ("protocol", "48"),
            ("challenge", "123456"),
            ("players", "14"),
            ("max", "32"),
            ("bots", "2"),
            ("gamedir", "cstrike"),
            ("map", "de_dust2"),
            ("type", "d"),
            ("password", "0"),
            ("os", "l"),
            ("secure", "1"),
            ("lan", "0"),
            ("version", "1.1.2.7/Stdio"),
            ("region", "255"),
            ("product", "cstrike"),
        ]);
        match parse_heartbeat_packet(&update).unwrap() {
            HeartbeatPacket::Update(info) => {
                assert_eq!(info.protocol, Some(48));
                assert_eq!(info.challenge, Some(123456));
                assert_eq!(info.players, Some(14));
                assert_eq!(info.max_players, Some(32));
                assert_eq!(info.bots, Some(2));
                assert_eq!(info.gamedir.as_deref(), Some("cstrike"));
                assert_eq!(info.map.as_deref(), Some("de_dust2"));
                assert_eq!(info.server_type.as_deref(), Some("d"));
                assert_eq!(info.password, Some(false));
                assert_eq!(info.os.as_deref(), Some("l"));
                assert_eq!(info.secure, Some(true));
                assert_eq!(info.lan, Some(false));
                assert_eq!(info.version.as_deref(), Some("1.1.2.7/Stdio"));
                assert_eq!(info.region, Some(255));
                assert_eq!(info.product.as_deref(), Some("cstrike"));
            }
            other => panic!("expected update, got {other:?}"),
        }
    }

    #[test]
    fn heartbeat_negative_signed_challenge_parses_as_u32() {
        let raw = b"0\n\\challenge\\-1\\gamedir\\cstrike\\\n";
        match parse_heartbeat_packet(raw).unwrap() {
            HeartbeatPacket::Update(info) => {
                assert_eq!(info.challenge, Some(u32::MAX));
                assert_eq!(info.gamedir.as_deref(), Some("cstrike"));
            }
            other => panic!("expected update, got {other:?}"),
        }
    }
}
