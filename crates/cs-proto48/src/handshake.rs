//! GoldSrc Protocol 48 connectionless (`0xFFFF_FFFF`) handshake packets
//! (`ReHLDS/engine/sv_main.cpp`: `SV_ConnectionlessPacket`, `SVC_GetChallenge`,
//! `SV_ConnectClient`).

use crate::bitbuf::{MsgReader, MsgWriter, ProtoError};
use crate::info::InfoString;
use crate::query::{A2A_ACK, A2A_PING, A2S_INFO, A2S_PLAYER, A2S_RULES};
use crate::{CONNECTIONLESS_HEADER, PROTOCOL_VERSION};

pub const S2C_CHALLENGE: u8 = b'A';
pub const S2C_CONNECTION: u8 = b'B';
pub const S2C_REJECT: u8 = b'9';
pub const A2A_PRINT: u8 = b'l';

/// Parsed connectionless (`0xFFFF_FFFF`) GoldSrc packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OobPacket {
    A2aPing,
    A2aAck,
    A2sInfo {
        challenge: Option<i32>,
    },
    A2sPlayer {
        challenge: i32,
    },
    A2sRules {
        challenge: i32,
    },
    GetChallenge {
        steam: bool,
    },
    S2cChallenge {
        challenge: u32,
        auth_protocol: u32,
        server_steam_id: u64,
        vac_secure: bool,
    },
    Connect {
        protocol: u32,
        challenge: u32,
        protinfo: InfoString,
        userinfo: InfoString,
        /// The userinfo argument exactly as received, for validity and length checks
        /// that the parsed [`InfoString`] cannot express.
        userinfo_raw: String,
        auth_ticket: Vec<u8>,
    },
    S2cConnection {
        userid: i32,
        remote_addr: String,
        vac_secure: bool,
        build: i32,
    },
    S2cReject(String),
    A2aPrint(String),
    ChallengeRcon,
    S2cChallengeRcon(u32),
    Rcon {
        challenge: u32,
        password: String,
        command: String,
        raw_tail: String,
    },
    Unknown(String),
}

/// Client → Server: `\xFF\xFF\xFF\xFFgetchallenge steam\n`.
pub fn encode_getchallenge(steam: bool) -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(32);
    w.write_i32(CONNECTIONLESS_HEADER);
    if steam {
        w.write_bytes(b"getchallenge steam\n");
    } else {
        w.write_bytes(b"getchallenge\n");
    }
    w.into_bytes()
}

/// Server → Client: `\xFF\xFF\xFF\xFFA00000000 <challenge> 3 <steamid> <vac>\n\0`.
pub fn encode_s2c_challenge(challenge: u32, server_steam_id: u64, vac_secure: bool) -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(80);
    w.write_i32(CONNECTIONLESS_HEADER);
    let text = format!(
        "A00000000 {challenge} 3 {server_steam_id} {}\n",
        u8::from(vac_secure)
    );
    w.write_str(&text);
    w.into_bytes()
}

/// Client → Server: `\xFF\xFF\xFF\xFFconnect 48 <challenge> "<protinfo>" "<userinfo>"\n<ticket>`.
pub fn encode_connect_48(
    challenge: u32,
    protinfo: &InfoString,
    userinfo: &InfoString,
    auth_ticket: &[u8],
) -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(512);
    w.write_i32(CONNECTIONLESS_HEADER);
    let line = format!(
        "connect {PROTOCOL_VERSION} {challenge} \"{}\" \"{}\"\n",
        protinfo.to_wire_string(),
        userinfo.to_wire_string()
    );
    w.write_bytes(line.as_bytes());
    w.write_bytes(auth_ticket);
    w.into_bytes()
}

/// Build standard CS 1.6 `protinfo` for Steam/LAN Protocol 48 connections.
pub fn default_cs16_protinfo(cdkey_md5_hex: &str) -> InfoString {
    let mut info = InfoString::new();
    info.set("prot", "3");
    info.set("unique", "-1");
    info.set("raw", "steam");
    info.set("cdkey", cdkey_md5_hex);
    info
}

/// Build standard CS 1.6 `userinfo` for Protocol 48 connections.
pub fn default_cs16_userinfo(player_name: &str) -> InfoString {
    let mut info = InfoString::new();
    info.set("_vgui_menus", "1");
    info.set("_ah", "1");
    info.set("cl_lw", "1");
    info.set("cl_lc", "1");
    info.set("cl_dlmax", "512");
    info.set("cl_updaterate", "60");
    info.set("rate", "30000");
    info.set("name", player_name);
    info.set("model", "gordon");
    info.set("topcolor", "30");
    info.set("bottomcolor", "6");
    info
}

/// Build a compact LAN-compatible Steam auth certificate blob (`len > 0 && len < 1024`)
/// accepted by `sv_lan 1` HLDS/ReHLDS servers (`sv_main.cpp:2462-2488`).
pub fn lan_steam_auth_ticket(steam_id: u64) -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(32);
    // Minimal Steam2/Steam3 ticket header used on LAN (`sv_lan 1` bypasses Steam backend).
    w.write_u32(20);
    w.write_u64_le(steam_id);
    w.write_u32(10); // CS 1.6 AppID
    w.write_u32(0);
    w.write_u32(0);
    w.into_bytes()
}

trait WriteU64Ext {
    fn write_u64_le(&mut self, v: u64);
}

impl WriteU64Ext for MsgWriter {
    fn write_u64_le(&mut self, v: u64) {
        self.write_bytes(&v.to_le_bytes());
    }
}

/// Server → Client: `\xFF\xFF\xFF\xFFB <userid> "<addr>" <vac> <build>\n\0`.
pub fn encode_s2c_connection(
    userid: i32,
    remote_addr: &str,
    vac_secure: bool,
    build: i32,
) -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(80);
    w.write_i32(CONNECTIONLESS_HEADER);
    let text = format!(
        "B {userid} \"{remote_addr}\" {} {build}\n",
        u8::from(vac_secure)
    );
    w.write_str(&text);
    w.into_bytes()
}

/// Server → Client: `\xFF\xFF\xFF\xFF9<reason>\n\0`.
pub fn encode_s2c_reject(reason: &str) -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(reason.len() + 16);
    w.write_i32(CONNECTIONLESS_HEADER);
    w.write_u8(S2C_REJECT);
    w.write_str(reason);
    w.into_bytes()
}

/// Server → Client: `\xFF\xFF\xFF\xFFj\0`.
pub fn encode_a2a_ack() -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(6);
    w.write_i32(CONNECTIONLESS_HEADER);
    w.write_u8(A2A_ACK);
    w.write_u8(0);
    w.into_bytes()
}

/// Server → Client: `\xFF\xFF\xFF\xFFl<text>\0\0` (`SV_FlushRedirect`, `sv_main.cpp:3277`).
pub fn encode_a2a_print(text: &str) -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(text.len() + 8);
    w.write_i32(CONNECTIONLESS_HEADER);
    w.write_u8(A2A_PRINT);
    w.write_str(text);
    w.write_u8(0);
    w.into_bytes()
}

/// RCON client → server: `\xFF\xFF\xFF\xFFchallenge rcon\n`.
pub fn encode_challenge_rcon() -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(24);
    w.write_i32(CONNECTIONLESS_HEADER);
    w.write_bytes(b"challenge rcon\n");
    w.into_bytes()
}

/// Server → RCON client: `\xFF\xFF\xFF\xFFchallenge rcon <challenge>\n\0` (`sv_main.cpp:2703`).
pub fn encode_s2c_challenge_rcon(challenge: u32) -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(36);
    w.write_i32(CONNECTIONLESS_HEADER);
    w.write_str(&format!("challenge rcon {challenge}\n"));
    w.into_bytes()
}

/// RCON client → server: `\xFF\xFF\xFF\xFFrcon <challenge> "<password>" <command>\n`.
pub fn encode_rcon(challenge: u32, password: &str, command: &str) -> Vec<u8> {
    let mut w = MsgWriter::with_capacity(password.len() + command.len() + 32);
    w.write_i32(CONNECTIONLESS_HEADER);
    w.write_bytes(format!("rcon {challenge} \"{password}\" {command}\n").as_bytes());
    w.into_bytes()
}

/// Parse a connectionless (`0xFFFF_FFFF`) UDP packet.
pub fn parse_oob_packet(raw: &[u8]) -> Result<OobPacket, ProtoError> {
    if raw.len() < 5 {
        return Err(ProtoError::UnexpectedEof);
    }
    let mut r = MsgReader::new(raw);
    let hdr = r.read_i32()?;
    if hdr != CONNECTIONLESS_HEADER {
        return Err(ProtoError::InvalidPayload(
            "not a connectionless packet".to_string(),
        ));
    }

    let first = raw[4];
    match first {
        A2S_INFO => {
            let _ = r.read_u8()?;
            let _ = r.read_str()?; // "Source Engine Query\0"
            let challenge = if r.remaining() >= 4 {
                Some(r.read_i32()?)
            } else {
                None
            };
            return Ok(OobPacket::A2sInfo { challenge });
        }
        A2S_PLAYER => {
            let _ = r.read_u8()?;
            let challenge = if r.remaining() >= 4 {
                r.read_i32()?
            } else {
                -1
            };
            return Ok(OobPacket::A2sPlayer { challenge });
        }
        A2S_RULES => {
            let _ = r.read_u8()?;
            let challenge = if r.remaining() >= 4 {
                r.read_i32()?
            } else {
                -1
            };
            return Ok(OobPacket::A2sRules { challenge });
        }
        A2A_PING if raw.len() <= 6 || raw[5] == 0 || raw[5] == b'\n' => {
            return Ok(OobPacket::A2aPing);
        }
        A2A_ACK if raw.len() <= 6 || raw[5] == 0 || raw[5] == b'\n' => {
            return Ok(OobPacket::A2aAck);
        }
        S2C_REJECT => {
            let _ = r.read_u8()?;
            return Ok(OobPacket::S2cReject(r.read_str()?));
        }
        A2A_PRINT => {
            let _ = r.read_u8()?;
            return Ok(OobPacket::A2aPrint(r.read_str()?));
        }
        _ => {}
    }

    let line = r.read_str_line()?;
    let trailing = r.remaining_slice().to_vec();
    let tokens = tokenize_goldsrc_line(&line);
    let Some(cmd) = tokens.first().map(String::as_str) else {
        return Ok(OobPacket::Unknown(line));
    };

    if cmd == "ping" {
        return Ok(OobPacket::A2aPing);
    }

    if cmd == "getchallenge" {
        let steam = tokens
            .get(1)
            .is_some_and(|t| t.eq_ignore_ascii_case("steam"));
        return Ok(OobPacket::GetChallenge { steam });
    }

    if cmd.eq_ignore_ascii_case("challenge")
        && tokens.get(1).is_some_and(|t| t.eq_ignore_ascii_case("rcon"))
    {
        if let Some(ch) = tokens.get(2).and_then(|s| s.parse::<u32>().ok()) {
            return Ok(OobPacket::S2cChallengeRcon(ch));
        }
        return Ok(OobPacket::ChallengeRcon);
    }

    if cmd == "rcon" {
        let challenge = tokens
            .get(1)
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|v| i32::try_from(v).ok().map(|v| v as u32).or_else(|| u32::try_from(v).ok()))
            .unwrap_or(0);
        let password = tokens.get(2).cloned().unwrap_or_default();
        let command = extract_rcon_command(&line);
        let raw_tail = line
            .strip_prefix("rcon")
            .unwrap_or("")
            .trim_end_matches(&['\n', '\r'][..])
            .to_string();
        return Ok(OobPacket::Rcon {
            challenge,
            password,
            command,
            raw_tail,
        });
    }

    if cmd.starts_with('A') && tokens.len() >= 3 {
        let challenge = tokens[1].parse::<u32>().unwrap_or(0);
        let auth_protocol = tokens[2].parse::<u32>().unwrap_or(2);
        let server_steam_id = tokens
            .get(3)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let vac_secure = tokens
            .get(4)
            .and_then(|s| s.parse::<u32>().ok())
            .is_some_and(|v| v != 0);
        return Ok(OobPacket::S2cChallenge {
            challenge,
            auth_protocol,
            server_steam_id,
            vac_secure,
        });
    }

    if cmd == "B" {
        let userid = tokens
            .get(1)
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(1);
        let remote_addr = tokens.get(2).cloned().unwrap_or_default();
        let vac_secure = tokens
            .get(3)
            .and_then(|s| s.parse::<u32>().ok())
            .is_some_and(|v| v != 0);
        let build = tokens
            .get(4)
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(6153);
        return Ok(OobPacket::S2cConnection {
            userid,
            remote_addr,
            vac_secure,
            build,
        });
    }

    if cmd == "connect" && tokens.len() >= 5 {
        let protocol = tokens[1].parse::<u32>().unwrap_or(0);
        // The client keeps the challenge in an `int` and prints it with `%i`, so a challenge
        // above `i32::MAX` comes back negative: read it as a signed number and keep its bits.
        let challenge = tokens[2]
            .parse::<i64>()
            .ok()
            .and_then(|v| i32::try_from(v).ok().map(|v| v as u32).or_else(|| u32::try_from(v).ok()))
            .unwrap_or(0);
        let protinfo = InfoString::parse(&tokens[3]);
        let userinfo = InfoString::parse(&tokens[4]);
        return Ok(OobPacket::Connect {
            protocol,
            challenge,
            protinfo,
            userinfo,
            userinfo_raw: tokens[4].clone(),
            auth_ticket: trailing,
        });
    }

    Ok(OobPacket::Unknown(line))
}

/// Extracts the raw command tail after `rcon <challenge> <password> ` so spaces and quotes in the
/// executed console command are preserved verbatim.
fn extract_rcon_command(line: &str) -> String {
    let trimmed = line.trim_end_matches(&['\n', '\r'][..]);
    let mut rest = trimmed.trim_start();
    // Skip 3 tokens (`rcon`, challenge, password).
    for _ in 0..3 {
        rest = rest.trim_start();
        if rest.is_empty() {
            return String::new();
        }
        if let Some(after_open) = rest.strip_prefix('"') {
            match after_open.find('"') {
                Some(end) => rest = &after_open[end + 1..],
                None => return String::new(),
            }
        } else {
            match rest.find(|c: char| c.is_ascii_whitespace()) {
                Some(end) => rest = &rest[end..],
                None => return String::new(),
            }
        }
    }
    rest.trim_start().to_string()
}

/// GoldSrc `Cmd_TokenizeString`-compatible splitter handling quoted arguments.
pub fn tokenize_goldsrc_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'"' {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            out.push(String::from_utf8_lossy(&bytes[start..i]).into_owned());
            if i < bytes.len() {
                i += 1;
            }
        } else {
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            out.push(String::from_utf8_lossy(&bytes[start..i]).into_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::md5_hex;

    #[test]
    fn connect_accepts_a_challenge_printed_as_a_negative_int() {
        let raw = b"\xFF\xFF\xFF\xFFconnect 48 -123 \"\\prot\\2\" \"\\name\\P\"\n";
        match parse_oob_packet(raw).unwrap() {
            OobPacket::Connect { challenge, .. } => assert_eq!(challenge, (-123i32) as u32),
            other => panic!("unexpected: {other:?}"),
        }
        let raw = b"\xFF\xFF\xFF\xFFconnect 48 3000000000 \"\\prot\\2\" \"\\name\\P\"\n";
        match parse_oob_packet(raw).unwrap() {
            OobPacket::Connect { challenge, .. } => assert_eq!(challenge, 3_000_000_000),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn handshake_packets_roundtrip_exactly() {
        let gc = encode_getchallenge(true);
        assert_eq!(
            parse_oob_packet(&gc).unwrap(),
            OobPacket::GetChallenge { steam: true }
        );

        let sc = encode_s2c_challenge(123_456_789, 90071992547409920, false);
        assert_eq!(
            parse_oob_packet(&sc).unwrap(),
            OobPacket::S2cChallenge {
                challenge: 123_456_789,
                auth_protocol: 3,
                server_steam_id: 90071992547409920,
                vac_secure: false,
            }
        );

        let prot = default_cs16_protinfo(&md5_hex(b"cs16-rs-key"));
        let user = default_cs16_userinfo("Player");
        let ticket = lan_steam_auth_ticket(76561198000000001);
        let conn = encode_connect_48(123_456_789, &prot, &user, &ticket);
        match parse_oob_packet(&conn).unwrap() {
            OobPacket::Connect {
                protocol,
                challenge,
                protinfo,
                userinfo,
                userinfo_raw,
                auth_ticket,
            } => {
                assert_eq!(protocol, 48);
                assert_eq!(challenge, 123_456_789);
                assert_eq!(protinfo.get("raw"), Some("steam"));
                assert_eq!(userinfo.get("name"), Some("Player"));
                assert_eq!(userinfo_raw, user.to_wire_string());
                assert_eq!(auth_ticket, ticket);
            }
            other => panic!("unexpected packet: {other:?}"),
        }

        let ok = encode_s2c_connection(7, "127.0.0.1:27005", false, 6153);
        assert_eq!(
            parse_oob_packet(&ok).unwrap(),
            OobPacket::S2cConnection {
                userid: 7,
                remote_addr: "127.0.0.1:27005".to_string(),
                vac_secure: false,
                build: 6153,
            }
        );
    }

    #[test]
    fn rcon_and_a2a_print_packets_roundtrip() {
        let cr = encode_challenge_rcon();
        assert_eq!(parse_oob_packet(&cr).unwrap(), OobPacket::ChallengeRcon);

        let scr = encode_s2c_challenge_rcon(987654321);
        assert_eq!(
            parse_oob_packet(&scr).unwrap(),
            OobPacket::S2cChallengeRcon(987654321)
        );

        let rc = encode_rcon(987654321, "s3cr3t pw", "changelevel \"de_dust2\"; status");
        assert_eq!(
            parse_oob_packet(&rc).unwrap(),
            OobPacket::Rcon {
                challenge: 987654321,
                password: "s3cr3t pw".to_string(),
                command: "changelevel \"de_dust2\"; status".to_string(),
                raw_tail: " 987654321 \"s3cr3t pw\" changelevel \"de_dust2\"; status".to_string(),
            }
        );

        let ap = encode_a2a_print("hostname: test\n");
        assert_eq!(
            parse_oob_packet(&ap).unwrap(),
            OobPacket::A2aPrint("hostname: test\n".to_string())
        );
    }
}
