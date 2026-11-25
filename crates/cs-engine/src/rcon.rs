//! Server-side GoldSrc RCON (`challenge rcon` / `rcon <ch> "<pw>" <cmd>`) authentication,
//! per-IP brute-force protection (`sv_rcon_*`), and `0xFFFF_FFFF 'l'` chunked responses.
//!
//! Ported from ReHLDS `rehlds/engine/sv_main.cpp` (`SV_Rcon`, `SV_GetChallenge`,
//! `SV_FlushRedirect`, read 2026-11-25).

use std::collections::{BTreeMap, HashMap};

use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::handshake::{encode_a2a_print, encode_s2c_challenge_rcon, tokenize_goldsrc_line};
use cs_transport::PeerAddr;

const CHALLENGE_TTL_SECS: f64 = 60.0;
const MAX_CHALLENGE_ENTRIES: usize = 2048;
/// Maximum RCON packet body after the 4-byte `0xFFFF_FFFF` header (`swds.dll:FUN_10204010`).
pub const MAX_RCON_BODY_BYTES: usize = 512;
/// Maximum RCON command length in characters (`swds.dll:FUN_10204010`).
pub const MAX_RCON_COMMAND_CHARS: usize = 1023;
/// Maximum text bytes carried per `0xFFFF_FFFF 'l'` UDP response datagram.
pub const MAX_RCON_REPLY_CHUNK: usize = 1200;

#[derive(Debug, Clone)]
struct ChallengeEntry {
    challenge: u32,
    issued_at: f64,
    touch: u64,
}

#[derive(Debug, Clone, Default)]
struct FailureState {
    timestamps: Vec<f64>,
    total_failures: u32,
    banned_until: Option<f64>,
}

/// RCON policy parameters read from server cvars (`sv_rcon_*`).
#[derive(Debug, Clone, Copy)]
pub struct RconPolicy {
    pub max_failures: u32,
    pub min_failures: u32,
    pub min_failure_time: f64,
    pub ban_penalty_mins: f64,
}

impl Default for RconPolicy {
    fn default() -> Self {
        Self {
            max_failures: 10,
            min_failures: 5,
            min_failure_time: 30.0,
            ban_penalty_mins: 0.0,
        }
    }
}

impl RconPolicy {
    /// Clamp RCON brute-force policy to retail `swds.dll:FUN_1020aed0` bounds:
    /// `sv_rcon_minfailures` and `sv_rcon_maxfailures` in `[1, 20]` with `max >= min`,
    /// and `sv_rcon_minfailuretime >= 1`.
    pub fn clamped(self) -> Self {
        let min_failures = self.min_failures.clamp(1, 20);
        let max_failures = self.max_failures.clamp(1, 20).max(min_failures);
        let min_failure_time = if self.min_failure_time.is_finite() {
            self.min_failure_time.max(1.0)
        } else {
            1.0
        };
        let ban_penalty_mins = if self.ban_penalty_mins.is_finite() {
            self.ban_penalty_mins.max(0.0)
        } else {
            0.0
        };
        Self {
            max_failures,
            min_failures,
            min_failure_time,
            ban_penalty_mins,
        }
    }
}

/// Parsed RCON datagram after `0xFFFF_FFFF`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RconDatagram {
    ChallengeRequest,
    Command {
        challenge: u32,
        password: String,
        command: String,
    },
}

/// Parses an OOB `0xFFFF_FFFF` payload if it is an RCON packet (`challenge rcon` or `rcon ...`).
pub fn parse_rcon_datagram(raw: &[u8]) -> Option<RconDatagram> {
    if raw.len() < 5 || !raw.starts_with(&[0xFF, 0xFF, 0xFF, 0xFF]) {
        return None;
    }
    let body = &raw[4..];
    if body.len() > MAX_RCON_BODY_BYTES {
        return None;
    }
    let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
    let text = std::str::from_utf8(&body[..end]).ok()?.trim();
    if text.eq_ignore_ascii_case("challenge rcon")
        || text
            .split_whitespace()
            .take(2)
            .eq(["challenge", "rcon"])
            && text.split_whitespace().count() == 2
    {
        return Some(RconDatagram::ChallengeRequest);
    }
    let after_rcon = text
        .strip_prefix("rcon")
        .filter(|s| s.starts_with(char::is_whitespace))?
        .trim_start();
    let idx = after_rcon.find(char::is_whitespace)?;
    let (ch_str, rest) = (&after_rcon[..idx], after_rcon[idx..].trim_start());
    let challenge = ch_str.parse::<u32>().ok()?;
    let (password, command) = if let Some(after_q) = rest.strip_prefix('"') {
        let close = after_q.find('"')?;
        let pw = &after_q[..close];
        let cmd = after_q[close + 1..].trim_start();
        (pw.to_string(), cmd.to_string())
    } else {
        let mut tokens = tokenize_goldsrc_line(rest);
        if tokens.is_empty() {
            return None;
        }
        let pw = tokens.remove(0);
        (pw, tokens.join(" "))
    };
    Some(RconDatagram::Command {
        challenge,
        password,
        command,
    })
}

/// Outcome of handling an RCON packet before A2S/connect dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RconDecision {
    /// Reply directly with these OOB datagrams (`challenge rcon ...` or `l` error).
    Reply(Vec<Vec<u8>>),
    /// Authenticated RCON command ready to run on `Server`.
    Execute(String),
}

/// Per-server RCON challenge + brute-force state.
#[derive(Debug)]
pub struct RconState {
    challenges: HashMap<PeerAddr, ChallengeEntry>,
    by_touch: BTreeMap<u64, PeerAddr>,
    next_touch: u64,
    next_challenge: u32,
    failures: HashMap<PeerAddr, FailureState>,
}

impl Default for RconState {
    fn default() -> Self {
        Self::new()
    }
}

impl RconState {
    pub fn new() -> Self {
        Self {
            challenges: HashMap::new(),
            by_touch: BTreeMap::new(),
            next_touch: 1,
            next_challenge: 0x5243_4F4E,
            failures: HashMap::new(),
        }
    }

    pub fn reset(&mut self) {
        self.failures.clear();
        self.challenges.clear();
        self.by_touch.clear();
    }

    pub fn is_ip_banned(&mut self, addr: &PeerAddr, now: f64) -> bool {
        let Some(state) = self.failures.get_mut(addr) else {
            return false;
        };
        match state.banned_until {
            Some(until) if until.is_infinite() || now < until => true,
            Some(_) => {
                state.banned_until = None;
                state.total_failures = 0;
                state.timestamps.clear();
                false
            }
            None => false,
        }
    }

    fn issue_challenge(&mut self, addr: PeerAddr, now: f64) -> u32 {
        self.next_challenge = self.next_challenge.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let challenge = self.next_challenge.max(1);
        let touch = self.next_touch;
        self.next_touch += 1;

        if !self.challenges.contains_key(&addr)
            && self.challenges.len() >= MAX_CHALLENGE_ENTRIES
            && let Some((_, oldest)) = self.by_touch.pop_first()
        {
            self.challenges.remove(&oldest);
        }
        if let Some(prev) = self.challenges.get(&addr) {
            self.by_touch.remove(&prev.touch);
        }
        self.by_touch.insert(touch, addr);
        self.challenges.insert(
            addr,
            ChallengeEntry {
                challenge,
                issued_at: now,
                touch,
            },
        );
        challenge
    }

    fn validate_challenge(&self, addr: &PeerAddr, challenge: u32, now: f64) -> bool {
        self.challenges
            .get(addr)
            .is_some_and(|e| e.challenge == challenge && now >= e.issued_at && now - e.issued_at <= CHALLENGE_TTL_SECS)
    }

    fn record_failure(&mut self, addr: PeerAddr, now: f64, policy: RconPolicy) -> bool {
        let policy = policy.clamped();
        let entry = self.failures.entry(addr).or_default();
        entry.total_failures = entry.total_failures.saturating_add(1);
        let window = policy.min_failure_time;
        entry.timestamps.push(now);
        entry.timestamps.retain(|&t| now - t <= window);

        let min_exceeded = entry.timestamps.len() >= policy.min_failures as usize;
        let max_exceeded = entry.total_failures >= policy.max_failures;
        if min_exceeded || max_exceeded {
            entry.banned_until = if policy.ban_penalty_mins <= 0.0 {
                Some(f64::INFINITY)
            } else {
                Some(now + policy.ban_penalty_mins * 60.0)
            };
            true
        } else {
            false
        }
    }

    /// Validates an incoming parsed RCON packet. Returns either direct reply datagrams or the
    /// command string to execute on the server console (`swds.dll:FUN_10204010`,
    /// `FUN_1020aed0`, `FUN_10208a50`).
    pub fn evaluate(
        &mut self,
        from: PeerAddr,
        now: f64,
        dgram: RconDatagram,
        rcon_password: &str,
        policy: RconPolicy,
    ) -> RconDecision {
        let policy = policy.clamped();
        match dgram {
            RconDatagram::ChallengeRequest => {
                if self.is_ip_banned(&from, now) {
                    return RconDecision::Reply(vec![encode_a2a_print(
                        "You have been banned from this server.\n",
                    )]);
                }
                let ch = self.issue_challenge(from, now);
                RconDecision::Reply(vec![encode_s2c_challenge_rcon(ch)])
            }
            RconDatagram::Command {
                challenge,
                password,
                command,
            } => {
                if self.is_ip_banned(&from, now) {
                    return RconDecision::Reply(vec![encode_a2a_print(
                        "You have been banned from this server.\n",
                    )]);
                }
                if !self.validate_challenge(&from, challenge, now) {
                    return RconDecision::Reply(vec![encode_a2a_print("Bad challenge.\n")]);
                }
                if rcon_password.is_empty() {
                    return RconDecision::Reply(vec![encode_a2a_print(
                        "Bad rcon_password.\nNo password set for this server.\n",
                    )]);
                }
                if password != rcon_password {
                    let banned = self.record_failure(from, now, policy);
                    let msg = if banned {
                        "Bad rcon_password.\nYou have been banned from this server.\n"
                    } else {
                        "Bad rcon_password.\n"
                    };
                    return RconDecision::Reply(vec![encode_a2a_print(msg)]);
                }
                let capped: String = command.chars().take(MAX_RCON_COMMAND_CHARS).collect();
                RconDecision::Execute(capped)
            }
        }
    }
}

/// Splits RCON console output into `0xFFFF_FFFF 'l' <chunk>\0` datagrams (`SV_FlushRedirect`).
pub fn encode_rcon_reply(text: &str) -> Vec<Vec<u8>> {
    if text.is_empty() {
        return vec![encode_a2a_print("")];
    }
    let mut packets = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + MAX_RCON_REPLY_CHUNK).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            break;
        }
        let chunk = &text[start..end];
        let mut w = MsgWriter::with_capacity(chunk.len() + 8);
        w.write_i32(-1);
        w.write_u8(b'l');
        w.write_str(chunk);
        packets.push(w.into_bytes());
        start = end;
    }
    packets
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_proto48::handshake::{OobPacket, encode_challenge_rcon, encode_rcon, parse_oob_packet};

    fn peer(n: u32) -> PeerAddr {
        PeerAddr::Loopback(n)
    }

    #[test]
    fn rcon_challenge_auth_and_chunking_work() {
        let mut rcon = RconState::new();
        let req = parse_rcon_datagram(&encode_challenge_rcon()).unwrap();
        let RconDecision::Reply(ch_pkt) =
            rcon.evaluate(peer(1), 10.0, req, "secret", RconPolicy::default())
        else {
            panic!("expected challenge reply");
        };
        assert!(ch_pkt[0].len() <= 64);
        let OobPacket::S2cChallengeRcon(ch) = parse_oob_packet(&ch_pkt[0]).unwrap() else {
            panic!("expected S2cChallengeRcon");
        };

        let cmd_pkt = encode_rcon(ch, "secret", "status");
        let parsed = parse_rcon_datagram(&cmd_pkt).unwrap();
        assert_eq!(
            rcon.evaluate(peer(1), 10.1, parsed, "secret", RconPolicy::default()),
            RconDecision::Execute("status".into())
        );

        let long = "x".repeat(3000);
        let chunks = encode_rcon_reply(&long);
        assert_eq!(chunks.len(), 3);
        for c in chunks {
            assert!(c.len() <= MAX_RCON_REPLY_CHUNK + 8);
        }
    }

    #[test]
    fn rcon_brute_force_triggers_ban_and_empty_password_rejects() {
        let mut rcon = RconState::new();
        let req = parse_rcon_datagram(&encode_challenge_rcon()).unwrap();
        let RconDecision::Reply(pkt) = rcon.evaluate(peer(2), 1.0, req, "", RconPolicy::default())
        else {
            panic!();
        };
        let OobPacket::S2cChallengeRcon(ch) = parse_oob_packet(&pkt[0]).unwrap() else {
            panic!();
        };
        let parsed = parse_rcon_datagram(&encode_rcon(ch, "any", "status")).unwrap();
        let RconDecision::Reply(r) =
            rcon.evaluate(peer(2), 1.1, parsed, "", RconPolicy::default())
        else {
            panic!("empty rcon_password must fail");
        };
        assert!(String::from_utf8_lossy(&r[0]).contains("No password set for this server.\n"));

        // Bad challenge must NOT count toward record_failure (`swds.dll:FUN_10204010`).
        let policy = RconPolicy {
            max_failures: 3,
            min_failures: 3,
            min_failure_time: 10.0,
            ban_penalty_mins: 5.0,
        };
        for attempt in 0..5 {
            let p = parse_rcon_datagram(&encode_rcon(ch.wrapping_add(1), "wrong", "status")).unwrap();
            let RconDecision::Reply(rep) =
                rcon.evaluate(peer(2), 1.5 + f64::from(attempt) * 0.01, p, "secret", policy)
            else {
                panic!("bad challenge must reply");
            };
            assert!(String::from_utf8_lossy(&rep[0]).contains("Bad challenge."));
        }
        assert!(!rcon.is_ip_banned(&peer(2), 1.8));

        for attempt in 0..3 {
            let p = parse_rcon_datagram(&encode_rcon(ch, "wrong", "status")).unwrap();
            let _ = rcon.evaluate(peer(2), 2.0 + f64::from(attempt), p, "secret", policy);
        }
        assert!(rcon.is_ip_banned(&peer(2), 5.0));
        // Even correct password is rejected while banned.
        let good = parse_rcon_datagram(&encode_rcon(ch, "secret", "status")).unwrap();
        assert!(matches!(
            rcon.evaluate(peer(2), 5.5, good, "secret", policy),
            RconDecision::Reply(_)
        ));
        // Expires after 5 minutes.
        assert!(!rcon.is_ip_banned(&peer(2), 400.0));

        // 512-byte body cap and RconPolicy clamping.
        let oversized = vec![0xFF, 0xFF, 0xFF, 0xFF]
            .into_iter()
            .chain(std::iter::repeat_n(b'a', MAX_RCON_BODY_BYTES + 1))
            .collect::<Vec<_>>();
        assert!(parse_rcon_datagram(&oversized).is_none());

        let clamped = RconPolicy {
            max_failures: 1,
            min_failures: 25,
            min_failure_time: 0.0,
            ban_penalty_mins: -1.0,
        }
        .clamped();
        assert_eq!(clamped.min_failures, 20);
        assert_eq!(clamped.max_failures, 20);
        assert_eq!(clamped.min_failure_time, 1.0);
        assert_eq!(clamped.ban_penalty_mins, 0.0);
    }
}
