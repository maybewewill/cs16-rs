//! Connectionless (`0xFFFF_FFFF`) packets: challenge, connect request and
//! server-info queries (`ReHLDS/engine/sv_main.cpp`: `SV_ConnectionlessPacket`).
//!
//! [`handle_oob`] is the one entry point: it parses a datagram and says what to do with it
//! ([`OobOutcome`]) and whether the datagram carried a challenge issued to its sender. Anyone
//! can send these datagrams with a forged source address, so it never panics and, unless the
//! datagram carries such a challenge, never replies with more than 64 bytes. A reply may still
//! be larger than the request that prompted it (a 23-byte `getchallenge` is answered with
//! up to 32 bytes), which is why the caller charges replies to unchallenged datagrams to the
//! per-address [`OobLimiter`] and to a global budget of [`OOB_GLOBAL_REPLIES_PER_SEC`].

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::hash::{BuildHasher, RandomState};
use std::net::IpAddr;
use std::sync::Mutex;

use cs_proto48::PROTOCOL_VERSION;
use cs_proto48::handshake::{
    OobPacket, encode_a2a_ack, encode_s2c_challenge, encode_s2c_reject, parse_oob_packet,
};
use cs_proto48::info::InfoString;
use cs_proto48::query::{
    PlayerQueryEntry, ServerQueryInfo, encode_s2a_info_src, encode_s2a_players, encode_s2a_rules,
    encode_s2c_query_challenge,
};
use cs_transport::PeerAddr;

/// Length of one challenge window; a challenge stays valid for its window and the next.
pub const CHALLENGE_BUCKET_SECS: f64 = 30.0;
/// Longest userinfo string a connect request may carry.
pub const MAX_INFO_STRING: usize = 256;
/// Longest player name, in bytes.
pub const MAX_NAME_BYTES: usize = 31;

/// Replies per second, all addresses together, to connectionless datagrams that carry no
/// challenge issued to their sender; also the largest burst of such replies.
pub const OOB_GLOBAL_REPLIES_PER_SEC: f64 = 1000.0;

const LIMITER_TOKENS_PER_SEC: f64 = 10.0;
const LIMITER_CAPACITY: f64 = 20.0;
const LIMITER_MAX_ADDRS: usize = 4096;

/// Result of validating a client connect challenge (`swds.dll:FUN_10205640`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeStatus {
    Ok,
    Missing,
    Bad,
}

#[derive(Debug)]
struct IssuedChallenge {
    challenge: u32,
    issued_at: f64,
    touch: u64,
}

#[derive(Debug, Default)]
struct ChallengeTable {
    by_addr: HashMap<PeerAddr, IssuedChallenge>,
    by_touch: BTreeMap<u64, PeerAddr>,
    next_touch: u64,
}

/// Issues and checks challenges for sender addresses (`swds.dll:FUN_10205640`), keeping a
/// bounded table so missing challenges (`"No challenge for your address.\n"`) are
/// distinguished from mismatched challenges (`"Bad challenge.\n"`).
#[derive(Debug)]
pub struct ChallengeIssuer {
    key: RandomState,
    table: Mutex<ChallengeTable>,
}

impl Default for ChallengeIssuer {
    fn default() -> Self {
        Self::new()
    }
}

impl ChallengeIssuer {
    pub fn new() -> Self {
        Self {
            key: RandomState::new(),
            table: Mutex::new(ChallengeTable::default()),
        }
    }

    pub fn issue(&self, addr: &PeerAddr, now: f64) -> u32 {
        let challenge = self.challenge_for(addr, window(now));
        let mut table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let touch = table.next_touch;
        table.next_touch += 1;
        if !table.by_addr.contains_key(addr)
            && table.by_addr.len() >= LIMITER_MAX_ADDRS
            && let Some((_, oldest)) = table.by_touch.pop_first()
        {
            table.by_addr.remove(&oldest);
        }
        if let Some(prev) = table.by_addr.get(addr) {
            let old_touch = prev.touch;
            table.by_touch.remove(&old_touch);
        }
        table.by_touch.insert(touch, *addr);
        table.by_addr.insert(
            *addr,
            IssuedChallenge {
                challenge,
                issued_at: now,
                touch,
            },
        );
        challenge
    }

    /// Distinguishes an accepted challenge, a missing/expired challenge for `addr`, and a
    /// mismatched challenge (`swds.dll:FUN_10205640`).
    pub fn verify(&self, addr: &PeerAddr, challenge: u32, now: f64) -> ChallengeStatus {
        if !now.is_finite() {
            return ChallengeStatus::Missing;
        }
        let table = self.table.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = table.by_addr.get(addr) else {
            return ChallengeStatus::Missing;
        };
        if !entry.issued_at.is_finite()
            || now < entry.issued_at
            || now - entry.issued_at > 2.0 * CHALLENGE_BUCKET_SECS
        {
            return ChallengeStatus::Missing;
        }
        if entry.challenge == challenge {
            ChallengeStatus::Ok
        } else {
            ChallengeStatus::Bad
        }
    }

    /// Accepts the challenge if it was issued for `addr` and has not expired.
    pub fn check(&self, addr: &PeerAddr, challenge: u32, now: f64) -> bool {
        self.verify(addr, challenge, now) == ChallengeStatus::Ok
    }

    /// Never 0, so that "no challenge" and "challenge 0" cannot be confused, and never above
    /// `i32::MAX`: the original client keeps the challenge in an `int` (found with the original
    /// client on 2026-11-22: a challenge with the top bit set came back as a negative number and
    /// the connect was refused with "Bad challenge.").
    fn challenge_for(&self, addr: &PeerAddr, window: i64) -> u32 {
        match self.key.hash_one((addr, window)) as u32 & 0x7FFF_FFFF {
            0 => 1,
            challenge => challenge,
        }
    }
}

/// Number of the challenge window containing `now` (saturating for non-finite `now`).
fn window(now: f64) -> i64 {
    (now / CHALLENGE_BUCKET_SECS).floor() as i64
}

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    refilled_at: f64,
    touch: u64,
}

/// Token bucket per address: 10 replies per second, bursts of 20, at most 4096
/// addresses (the least recently touched one is evicted).
#[derive(Debug, Default)]
pub struct OobLimiter {
    buckets: HashMap<PeerAddr, Bucket>,
    by_touch: BTreeMap<u64, PeerAddr>,
    next_touch: u64,
}

impl OobLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consumes one token for `addr`; `false` when its bucket is empty.
    pub fn allow(&mut self, addr: &PeerAddr, now: f64) -> bool {
        let touch = self.next_touch;
        self.next_touch += 1;
        if !self.buckets.contains_key(addr)
            && self.buckets.len() >= LIMITER_MAX_ADDRS
            && let Some((_, oldest)) = self.by_touch.pop_first()
        {
            self.buckets.remove(&oldest);
        }
        let bucket = self.buckets.entry(*addr).or_insert(Bucket {
            tokens: LIMITER_CAPACITY,
            refilled_at: now,
            touch,
        });
        self.by_touch.remove(&bucket.touch);
        bucket.touch = touch;
        self.by_touch.insert(touch, *addr);
        // A clock that steps back (or NaN) refills nothing.
        if now > bucket.refilled_at {
            bucket.tokens = (bucket.tokens + (now - bucket.refilled_at) * LIMITER_TOKENS_PER_SEC)
                .min(LIMITER_CAPACITY);
            bucket.refilled_at = now;
        }
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum QueryIpKey {
    Ip(IpAddr),
    Loopback(u32),
    Web(u32),
    WebTransport(u32),
}

impl QueryIpKey {
    fn from_peer(addr: &PeerAddr) -> Self {
        match addr {
            PeerAddr::Udp(sa) => Self::Ip(sa.ip()),
            PeerAddr::Loopback(id) => Self::Loopback(*id),
            PeerAddr::Web(id) => Self::Web(*id),
            PeerAddr::WebTransport(id) => Self::WebTransport(*id),
        }
    }
}

/// Retail A2S query rate limiter (`swds.dll:FUN_1020b250`): enforces `max_queries_sec`,
/// `max_queries_sec_global`, and `max_queries_window` over a sliding time window.
#[derive(Debug, Default)]
pub struct QueryRateLimiter {
    per_ip: HashMap<QueryIpKey, (VecDeque<f64>, u64)>,
    by_touch: BTreeMap<u64, QueryIpKey>,
    global: VecDeque<f64>,
    next_touch: u64,
}

impl QueryRateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow(
        &mut self,
        addr: &PeerAddr,
        now: f64,
        max_queries_sec: f64,
        max_queries_sec_global: f64,
        window_secs: f64,
    ) -> bool {
        if !now.is_finite() {
            return false;
        }
        let window = if window_secs.is_finite() && window_secs > 0.0 {
            window_secs.max(1.0)
        } else {
            60.0
        };
        let cutoff = now - window;
        while let Some(&front) = self.global.front() {
            if front <= cutoff || front > now {
                self.global.pop_front();
            } else {
                break;
            }
        }
        if max_queries_sec_global.is_finite() && max_queries_sec_global > 0.0 {
            let global_limit = (max_queries_sec_global * window).floor().max(1.0) as usize;
            if self.global.len() >= global_limit {
                return false;
            }
        }

        let key = QueryIpKey::from_peer(addr);
        let touch = self.next_touch;
        self.next_touch += 1;
        if !self.per_ip.contains_key(&key)
            && self.per_ip.len() >= LIMITER_MAX_ADDRS
            && let Some((_, oldest)) = self.by_touch.pop_first()
        {
            self.per_ip.remove(&oldest);
        }
        let (times, old_touch) = self.per_ip.entry(key).or_insert_with(|| (VecDeque::new(), touch));
        self.by_touch.remove(old_touch);
        *old_touch = touch;
        self.by_touch.insert(touch, key);
        while let Some(&front) = times.front() {
            if front <= cutoff || front > now {
                times.pop_front();
            } else {
                break;
            }
        }
        if max_queries_sec.is_finite() && max_queries_sec > 0.0 {
            let per_ip_limit = (max_queries_sec * window).floor().max(1.0) as usize;
            if times.len() >= per_ip_limit {
                return false;
            }
        }
        times.push_back(now);
        self.global.push_back(now);
        true
    }
}

/// A connect request that passed every check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectRequest {
    pub userinfo: InfoString,
    pub protinfo: InfoString,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OobOutcome {
    Reply(Vec<u8>),
    Connect(ConnectRequest),
    Ignore,
}

/// What [`handle_oob`] needs to know about the server.
pub struct OobContext<'a> {
    pub challenges: &'a ChallengeIssuer,
    pub query: &'a ServerQueryInfo,
    pub players: &'a [PlayerQueryEntry],
    pub rules: &'a [(&'a str, &'a str)],
    pub is_full: bool,
    pub sv_contact: &'a str,
}

/// Answers one connectionless datagram from `from`. The caller applies [`OobLimiter`] first.
/// The flag is `true` when the datagram is a connect request or an A2S query that carries a
/// challenge `ctx.challenges` accepts for `from` (a reply to it may exceed 64 bytes), `false`
/// for every other datagram.
pub fn handle_oob(
    raw: &[u8],
    from: &PeerAddr,
    now: f64,
    ctx: &OobContext<'_>,
) -> (OobOutcome, bool) {
    let Ok(packet) = parse_oob_packet(raw) else {
        return (OobOutcome::Ignore, false);
    };
    match packet {
        OobPacket::GetChallenge { .. } => (
            OobOutcome::Reply(encode_s2c_challenge(
                ctx.challenges.issue(from, now),
                0,
                false,
            )),
            false,
        ),
        OobPacket::Connect {
            protocol,
            challenge,
            protinfo,
            userinfo,
            userinfo_raw,
            ..
        } => {
            // The challenge goes first (`swds.dll:FUN_10205640`): missing vs mismatched.
            match ctx.challenges.verify(from, challenge, now) {
                ChallengeStatus::Ok => {}
                ChallengeStatus::Missing => {
                    return (reject("No challenge for your address.\n"), false);
                }
                ChallengeStatus::Bad => {
                    return (reject("Bad challenge.\n"), false);
                }
            }
            if protocol < PROTOCOL_VERSION {
                return (
                    reject(&format!(
                        "This server is using a newer protocol ( {PROTOCOL_VERSION} ) than your client ( {protocol} ).  You should check for updates to your client.\n"
                    )),
                    true,
                );
            }
            if protocol > PROTOCOL_VERSION {
                return (
                    reject(&format!(
                        "This server is using an older protocol ( {PROTOCOL_VERSION} ) than your client ( {protocol} ).  If you believe this server is outdated, you can contact the server administrator at {}.\n",
                        ctx.sv_contact
                    )),
                    true,
                );
            }
            // `InfoString::is_valid` tolerates a missing leading `\`; the original `Info_IsValid` does not.
            if !userinfo_raw.starts_with('\\')
                || !InfoString::is_valid(&userinfo_raw)
                || userinfo_raw.len() > MAX_INFO_STRING
            {
                return (reject("Invalid userinfo in connect command\n"), true);
            }
            if ctx.is_full {
                return (reject("Server is full.\n"), true);
            }
            let name = sanitize_name(userinfo.get("name").unwrap_or(""));
            (
                OobOutcome::Connect(ConnectRequest {
                    userinfo,
                    protinfo,
                    name,
                }),
                true,
            )
        }
        OobPacket::A2aPing => (OobOutcome::Reply(encode_a2a_ack()), false),
        OobPacket::A2sInfo { challenge } => {
            query_reply(challenge, from, now, ctx, || encode_s2a_info_src(ctx.query))
        }
        OobPacket::A2sPlayer { challenge } => query_reply(Some(challenge), from, now, ctx, || {
            encode_s2a_players(ctx.players)
        }),
        OobPacket::A2sRules { challenge } => query_reply(Some(challenge), from, now, ctx, || {
            encode_s2a_rules(ctx.rules)
        }),
        OobPacket::A2aAck
        | OobPacket::S2cChallenge { .. }
        | OobPacket::S2cConnection { .. }
        | OobPacket::S2cReject(_)
        | OobPacket::A2aPrint(_)
        | OobPacket::ChallengeRcon
        | OobPacket::S2cChallengeRcon(_)
        | OobPacket::Rcon { .. }
        | OobPacket::Unknown(_) => (OobOutcome::Ignore, false),
    }
}

fn reject(text: &str) -> OobOutcome {
    OobOutcome::Reply(encode_s2c_reject(text))
}

/// The full A2S reply for an accepted challenge, otherwise the 9-byte challenge reply; the
/// flag tells which.
fn query_reply(
    challenge: Option<i32>,
    from: &PeerAddr,
    now: f64,
    ctx: &OobContext<'_>,
    full_reply: impl FnOnce() -> Vec<u8>,
) -> (OobOutcome, bool) {
    match challenge {
        Some(c) if ctx.challenges.check(from, c as u32, now) => {
            (OobOutcome::Reply(full_reply()), true)
        }
        _ => (
            OobOutcome::Reply(encode_s2c_query_challenge(
                ctx.challenges.issue(from, now) as i32
            )),
            false,
        ),
    }
}

/// Strips control characters and surrounding spaces and truncates to
/// [`MAX_NAME_BYTES`] on a character boundary; an empty result becomes `"unnamed"`.
pub fn sanitize_name(raw: &str) -> String {
    let printable: String = raw
        .chars()
        .filter(|&c| !(c < '\u{20}' || c == '\u{7F}'))
        .collect();
    let trimmed = printable.trim_matches(' ');
    let mut end = trimmed.len().min(MAX_NAME_BYTES);
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    let name = trimmed[..end].trim_end_matches(' ');
    if name.is_empty() {
        "unnamed".to_string()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use cs_proto48::crypto::md5_hex;
    use cs_proto48::handshake::{
        default_cs16_protinfo, default_cs16_userinfo, encode_connect_48, encode_getchallenge,
        lan_steam_auth_ticket,
    };
    use cs_proto48::query::{
        A2S_RULES, A2sInfoReply, cs16_server_query_info, decode_a2s_info_reply,
        encode_a2s_info_request, encode_a2s_player_request,
    };

    use super::*;

    const NOW: f64 = 1000.0;
    const RULES: &[(&str, &str)] = &[("mp_timelimit", "20"), ("sv_gravity", "800")];

    fn addr(n: u8) -> PeerAddr {
        PeerAddr::Udp(SocketAddr::from(([10, 0, 0, n], 27005)))
    }

    fn query() -> ServerQueryInfo {
        cs16_server_query_info("cs16-rs test", "de_dust2", 3, 32, 0)
    }

    fn players() -> Vec<PlayerQueryEntry> {
        vec![PlayerQueryEntry {
            index: 0,
            name: "Player".to_string(),
            score: 2,
            duration_secs: 12.5,
        }]
    }

    fn context<'a>(
        challenges: &'a ChallengeIssuer,
        query: &'a ServerQueryInfo,
        players: &'a [PlayerQueryEntry],
        is_full: bool,
    ) -> OobContext<'a> {
        OobContext {
            challenges,
            query,
            players,
            rules: RULES,
            is_full,
            sv_contact: "admin@example.com",
        }
    }

    fn protinfo() -> InfoString {
        default_cs16_protinfo(&md5_hex(b"cs16-rs-key"))
    }

    /// The outcome of [`handle_oob`], without the challenge flag.
    fn outcome(raw: &[u8], from: &PeerAddr, now: f64, ctx: &OobContext<'_>) -> OobOutcome {
        handle_oob(raw, from, now, ctx).0
    }

    #[test]
    fn challenge_flag_is_set_only_for_a_valid_challenge() {
        let issuer = ChallengeIssuer::new();
        let q = query();
        let list = players();
        let ctx = context(&issuer, &q, &list, false);
        let from = addr(1);
        let good = issuer.issue(&from, NOW);
        let bad = (1..).find(|c| !issuer.check(&from, *c, NOW)).unwrap();
        let info = default_cs16_userinfo("P").to_wire_string();
        let a2s = |c: i32| encode_a2s_player_request(c);
        let flag = |raw: &[u8]| handle_oob(raw, &from, NOW, &ctx).1;
        // A connect or a query with the issued challenge.
        assert!(flag(&connect_datagram(48, good, &info)));
        assert!(flag(&connect_datagram(47, good, &info)));
        assert!(flag(&a2s(good as i32)));
        assert!(flag(&encode_a2s_info_request(Some(good as i32))));
        // The same with a wrong challenge, a challenge issued to someone else, and requests
        // that carry none.
        assert!(!flag(&connect_datagram(48, bad, &info)));
        assert!(!flag(&a2s(bad as i32)));
        assert!(!flag(&encode_a2s_info_request(None)));
        assert!(!flag(&encode_a2s_info_request(Some(bad as i32))));
        assert!(!handle_oob(&connect_datagram(48, good, &info), &addr(2), NOW, &ctx).1);
        assert!(!flag(&encode_getchallenge(true)));
        assert!(!flag(b"garbage"));
    }

    /// A connect datagram with an arbitrary protocol number and raw userinfo text.
    fn connect_datagram(protocol: u32, challenge: u32, userinfo_raw: &str) -> Vec<u8> {
        let mut d = vec![0xFF; 4];
        d.extend_from_slice(
            format!(
                "connect {protocol} {challenge} \"{}\" \"{userinfo_raw}\"\n",
                protinfo().to_wire_string()
            )
            .as_bytes(),
        );
        d.extend_from_slice(&lan_steam_auth_ticket(76561198000000001));
        d
    }

    #[test]
    fn getchallenge_reply_parses_and_validates() {
        let issuer = ChallengeIssuer::new();
        let q = query();
        let ctx = context(&issuer, &q, &[], false);
        let OobOutcome::Reply(reply) = outcome(&encode_getchallenge(true), &addr(1), NOW, &ctx)
        else {
            panic!("getchallenge must be answered");
        };
        assert!(reply.len() <= 64);
        match parse_oob_packet(&reply).unwrap() {
            OobPacket::S2cChallenge { challenge, .. } => {
                assert_ne!(challenge, 0);
                assert!(issuer.check(&addr(1), challenge, NOW));
            }
            other => panic!("unexpected reply: {other:?}"),
        }
    }

    #[test]
    fn challenges_fit_the_client_int() {
        let issuer = ChallengeIssuer::new();
        for port in 0..2000u16 {
            let a = PeerAddr::Udp(SocketAddr::from(([10, 0, 0, 1], port)));
            for t in [0.0, 30.0, 1e6] {
                let c = issuer.issue(&a, t);
                assert!(c != 0 && c <= i32::MAX as u32, "{c}");
            }
        }
    }

    #[test]
    fn challenge_is_bound_to_address_and_expires() {
        let issuer = ChallengeIssuer::new();
        let challenge = issuer.issue(&addr(1), NOW);
        assert_ne!(challenge, 0);
        assert!(issuer.check(&addr(1), challenge, NOW));
        assert!(!issuer.check(&addr(2), challenge, NOW));
        assert!(!issuer.check(&addr(1), challenge, NOW + 61.0));
        assert!(issuer.check(&addr(1), challenge, NOW + 29.0));
    }

    #[test]
    fn connect_with_bad_or_missing_challenge_is_rejected() {
        let issuer = ChallengeIssuer::new();
        let q = query();
        let ctx = context(&issuer, &q, &[], false);
        let bad = 12345u32;
        // Without a prior getchallenge for this address -> "No challenge for your address.\n".
        for protocol in [48, 47] {
            let raw = connect_datagram(protocol, bad, &default_cs16_userinfo("P").to_wire_string());
            assert_eq!(
                outcome(&raw, &addr(1), NOW, &ctx),
                reject("No challenge for your address.\n")
            );
        }
        // With an issued challenge for this address and a mismatched number -> "Bad challenge.\n".
        let issued = issuer.issue(&addr(1), NOW);
        let mismatched = if issued == 1 { 2 } else { 1 };
        for protocol in [48, 47] {
            let raw = connect_datagram(
                protocol,
                mismatched,
                &default_cs16_userinfo("P").to_wire_string(),
            );
            assert_eq!(
                outcome(&raw, &addr(1), NOW, &ctx),
                reject("Bad challenge.\n")
            );
        }
    }

    #[test]
    fn connect_with_wrong_protocol_is_rejected() {
        let issuer = ChallengeIssuer::new();
        let q = query();
        let ctx = context(&issuer, &q, &[], false);
        let challenge = issuer.issue(&addr(1), NOW);
        let raw = connect_datagram(47, challenge, &default_cs16_userinfo("P").to_wire_string());
        let OobOutcome::Reply(reply) = outcome(&raw, &addr(1), NOW, &ctx) else {
            panic!("wrong protocol must be rejected");
        };
        let OobPacket::S2cReject(text) = parse_oob_packet(&reply).unwrap() else {
            panic!("expected a reject");
        };
        assert_eq!(
            text,
            "This server is using a newer protocol ( 48 ) than your client ( 47 ).  You should check for updates to your client.\n"
        );

        let raw_newer = connect_datagram(49, challenge, &default_cs16_userinfo("P").to_wire_string());
        let OobOutcome::Reply(reply_newer) = outcome(&raw_newer, &addr(1), NOW, &ctx) else {
            panic!("newer client protocol must be rejected");
        };
        let OobPacket::S2cReject(text_newer) = parse_oob_packet(&reply_newer).unwrap() else {
            panic!("expected a reject");
        };
        assert_eq!(
            text_newer,
            "This server is using an older protocol ( 48 ) than your client ( 49 ).  If you believe this server is outdated, you can contact the server administrator at admin@example.com.\n"
        );
    }

    #[test]
    fn query_rate_limiter_enforces_per_ip_and_global_windows() {
        let mut q = QueryRateLimiter::new();
        let a1 = PeerAddr::Udp(SocketAddr::from(([10, 0, 0, 1], 27005)));
        let a1_alt = PeerAddr::Udp(SocketAddr::from(([10, 0, 0, 1], 27006)));
        let a2 = PeerAddr::Udp(SocketAddr::from(([10, 0, 0, 2], 27005)));
        // 3.0 * 60 = 180 per IP in 60s window (shared across ports on same IP).
        for i in 0..180 {
            let p = if i % 2 == 0 { &a1 } else { &a1_alt };
            assert!(q.allow(p, NOW, 3.0, 30.0, 60.0));
        }
        assert!(!q.allow(&a1, NOW, 3.0, 30.0, 60.0));
        assert!(q.allow(&a2, NOW, 3.0, 30.0, 60.0));
        // After window elapses, allowed again.
        assert!(q.allow(&a1, NOW + 61.0, 3.0, 30.0, 60.0));
    }

    #[test]
    fn connect_with_oversized_or_invalid_userinfo_is_rejected() {
        let issuer = ChallengeIssuer::new();
        let q = query();
        let ctx = context(&issuer, &q, &[], false);
        let challenge = issuer.issue(&addr(1), NOW);
        let invalid = reject("Invalid userinfo in connect command\n");

        let oversized = format!("\\name\\{}", "a".repeat(251));
        assert_eq!(oversized.len(), 257);
        let raw = connect_datagram(48, challenge, &oversized);
        assert_eq!(outcome(&raw, &addr(1), NOW, &ctx), invalid);

        let at_limit = format!("\\name\\{}", "a".repeat(250));
        assert_eq!(at_limit.len(), MAX_INFO_STRING);
        let raw = connect_datagram(48, challenge, &at_limit);
        assert!(matches!(
            outcome(&raw, &addr(1), NOW, &ctx),
            OobOutcome::Connect(_)
        ));

        for bad in ["name\\Bob", "", "\\name", "\\\\Bob"] {
            let raw = connect_datagram(48, challenge, bad);
            assert_eq!(outcome(&raw, &addr(1), NOW, &ctx), invalid, "{bad:?}");
        }
    }

    #[test]
    fn connect_when_full_is_rejected() {
        let issuer = ChallengeIssuer::new();
        let q = query();
        let ctx = context(&issuer, &q, &[], true);
        let challenge = issuer.issue(&addr(1), NOW);
        let raw = encode_connect_48(
            challenge,
            &protinfo(),
            &default_cs16_userinfo("P"),
            &lan_steam_auth_ticket(1),
        );
        assert_eq!(
            outcome(&raw, &addr(1), NOW, &ctx),
            reject("Server is full.\n")
        );
    }

    #[test]
    fn valid_connect_yields_request_with_sanitized_name() {
        let issuer = ChallengeIssuer::new();
        let q = query();
        let ctx = context(&issuer, &q, &[], false);
        let challenge = issuer.issue(&addr(1), NOW);
        let userinfo = default_cs16_userinfo("  A\x01B  ");
        let raw = encode_connect_48(challenge, &protinfo(), &userinfo, &lan_steam_auth_ticket(1));
        assert_eq!(
            outcome(&raw, &addr(1), NOW, &ctx),
            OobOutcome::Connect(ConnectRequest {
                userinfo,
                protinfo: protinfo(),
                name: "AB".to_string(),
            })
        );
    }

    #[test]
    fn a2s_without_challenge_gets_nine_byte_challenge() {
        let issuer = ChallengeIssuer::new();
        let q = query();
        let ctx = context(&issuer, &q, &[], false);
        let mut rules_request = vec![0xFF; 4];
        rules_request.push(A2S_RULES);
        rules_request.extend_from_slice(&(-1i32).to_le_bytes());
        for request in [
            encode_a2s_info_request(None),
            encode_a2s_player_request(-1),
            rules_request,
        ] {
            let OobOutcome::Reply(reply) = outcome(&request, &addr(1), NOW, &ctx) else {
                panic!("A2S query must be answered");
            };
            assert_eq!(reply.len(), 9);
            let A2sInfoReply::Challenge(challenge) = decode_a2s_info_reply(&reply, "", 0).unwrap()
            else {
                panic!("expected a challenge");
            };
            assert!(issuer.check(&addr(1), challenge as u32, NOW));
        }
    }

    #[test]
    fn a2s_with_accepted_challenge_returns_server_info() {
        let issuer = ChallengeIssuer::new();
        let q = query();
        let p = players();
        let ctx = context(&issuer, &q, &p, false);
        let challenge = issuer.issue(&addr(1), NOW) as i32;

        let request = encode_a2s_info_request(Some(challenge));
        let OobOutcome::Reply(reply) = outcome(&request, &addr(1), NOW, &ctx) else {
            panic!("A2S_INFO must be answered");
        };
        assert_eq!(
            decode_a2s_info_reply(&reply, &q.addr, q.ping_ms).unwrap(),
            A2sInfoReply::Info(q.clone())
        );

        assert_eq!(
            outcome(&encode_a2s_player_request(challenge), &addr(1), NOW, &ctx),
            OobOutcome::Reply(encode_s2a_players(&p))
        );
        let mut rules_request = vec![0xFF; 4];
        rules_request.push(A2S_RULES);
        rules_request.extend_from_slice(&challenge.to_le_bytes());
        assert_eq!(
            outcome(&rules_request, &addr(1), NOW, &ctx),
            OobOutcome::Reply(encode_s2a_rules(RULES))
        );

        let foreign = issuer.issue(&addr(2), NOW) as i32;
        let OobOutcome::Reply(reply) =
            outcome(&encode_a2s_info_request(Some(foreign)), &addr(1), NOW, &ctx)
        else {
            panic!("A2S_INFO must be answered");
        };
        assert_eq!(reply.len(), 9);
        assert!(matches!(
            decode_a2s_info_reply(&reply, "", 0).unwrap(),
            A2sInfoReply::Challenge(_)
        ));
    }

    #[test]
    fn server_to_client_packets_are_ignored() {
        let issuer = ChallengeIssuer::new();
        let q = query();
        let ctx = context(&issuer, &q, &[], false);
        for raw in [
            encode_s2c_challenge(5, 0, false),
            encode_s2c_reject("nope\n"),
            encode_a2a_ack(),
            cs_proto48::handshake::encode_s2c_connection(1, "x", false, 1),
            b"\xFF\xFF\xFF\xFFrcon 1 pass status\n".to_vec(),
            b"not connectionless".to_vec(),
        ] {
            assert_eq!(outcome(&raw, &addr(1), NOW, &ctx), OobOutcome::Ignore);
        }
        assert_eq!(
            outcome(&cs_proto48::query::encode_a2a_ping(), &addr(1), NOW, &ctx),
            OobOutcome::Reply(encode_a2a_ack())
        );
    }

    #[test]
    fn sanitize_name_rules() {
        assert_eq!(sanitize_name(""), "unnamed");
        assert_eq!(sanitize_name("    "), "unnamed");
        assert_eq!(sanitize_name("\x01\x7F\n"), "unnamed");
        assert_eq!(sanitize_name("  A\x01B  "), "AB");
        assert_eq!(sanitize_name(&format!("a{}b", " ".repeat(30))), "a");
        let long = "я".repeat(40);
        let name = sanitize_name(&long);
        assert!(name.len() <= MAX_NAME_BYTES);
        assert_eq!(name, "я".repeat(15));
    }

    #[test]
    fn limiter_caps_burst() {
        let mut limiter = OobLimiter::new();
        let allowed = (0..100).filter(|_| limiter.allow(&addr(1), NOW)).count();
        assert_eq!(allowed, 20);
        let allowed = (0..100)
            .filter(|_| limiter.allow(&addr(1), NOW + 1.0))
            .count();
        assert_eq!(allowed, 10);
        assert!(limiter.allow(&addr(2), NOW + 1.0));
    }

    #[test]
    fn limiter_evicts_least_recently_touched_address() {
        let mut limiter = OobLimiter::new();
        let peer = |i: u32| PeerAddr::Loopback(i);
        while limiter.allow(&peer(0), NOW) {}
        for i in 1..LIMITER_MAX_ADDRS as u32 {
            assert!(limiter.allow(&peer(i), NOW));
        }
        // Touch the exhausted address so address 1 becomes the oldest.
        assert!(!limiter.allow(&peer(0), NOW));
        assert!(limiter.allow(&peer(LIMITER_MAX_ADDRS as u32), NOW));
        assert_eq!(limiter.buckets.len(), LIMITER_MAX_ADDRS);
        assert_eq!(limiter.by_touch.len(), LIMITER_MAX_ADDRS);
        assert!(!limiter.buckets.contains_key(&peer(1)));
        assert!(
            !limiter.allow(&peer(0), NOW),
            "address 0 must keep its empty bucket"
        );
    }

    #[test]
    fn random_and_truncated_datagrams_never_panic_or_amplify() {
        let issuer = ChallengeIssuer::new();
        let q = query();
        let p = players();
        let ctx = context(&issuer, &q, &p, false);
        let from = addr(7);

        let mut inputs: Vec<Vec<u8>> = Vec::new();
        let mut state: u64 = 0xC516;
        let mut next_byte = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 56) as u8
        };
        for i in 0..10_000usize {
            let len = i % 65;
            let mut d: Vec<u8> = (0..len).map(|_| next_byte()).collect();
            if i % 2 == 0 {
                for b in d.iter_mut().take(4) {
                    *b = 0xFF;
                }
            }
            inputs.push(d);
        }
        let getchallenge = encode_getchallenge(true);
        let connect = encode_connect_48(
            issuer.issue(&from, NOW),
            &protinfo(),
            &default_cs16_userinfo("Player"),
            &lan_steam_auth_ticket(76561198000000001),
        );
        for valid in [&getchallenge, &connect] {
            for end in 0..=valid.len() {
                inputs.push(valid[..end].to_vec());
            }
        }

        let mut replies = 0;
        let mut connects = 0;
        for raw in &inputs {
            let (result, challenged) = handle_oob(raw, &from, NOW, &ctx);
            match result {
                OobOutcome::Reply(reply) => {
                    replies += 1;
                    if !challenged {
                        assert!(reply.len() <= 64, "{} byte reply to {raw:?}", reply.len());
                    }
                }
                OobOutcome::Connect(_) => connects += 1,
                OobOutcome::Ignore => {}
            }
        }
        // The generator must actually reach the connectionless handlers.
        assert!(replies > 0);
        assert!(connects > 0);
    }
}
