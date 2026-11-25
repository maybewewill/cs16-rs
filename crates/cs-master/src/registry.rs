//! GoldSrc Master Server registry, challenge-response table, query filtering, and JSON export.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::ops::Bound;

use tracing::debug;

use crate::protocol::{
    HeartbeatInfo, HeartbeatPacket, MasterError, REGION_ALL, encode_heartbeat_challenge,
    parse_heartbeat_packet,
};

/// Default server registration TTL in seconds (5 minutes).
pub const DEFAULT_TTL_SECS: f64 = 300.0;

/// Default maximum number of registered servers kept in memory.
pub const DEFAULT_MAX_ENTRIES: usize = 4096;

/// Default maximum number of outstanding heartbeat challenges.
pub const DEFAULT_MAX_CHALLENGES: usize = 8192;

/// Default lifetime of an issued heartbeat challenge in seconds.
pub const DEFAULT_CHALLENGE_TTL_SECS: f64 = 30.0;

/// Default maximum server entries returned in a single `0x31` query reply datagram.
pub const DEFAULT_MAX_PER_PAGE: usize = 200;

const MAX_FILTER_DEPTH: usize = 16;

/// A registered game server in the master server directory.
#[derive(Debug, Clone, PartialEq)]
pub struct MasterEntry {
    pub addr: SocketAddrV4,
    pub protocol: u8,
    pub gamedir: String,
    pub map: String,
    pub players: u8,
    pub max_players: u8,
    pub bots: u8,
    pub dedicated: bool,
    pub proxy: bool,
    pub secure: bool,
    pub password: bool,
    pub linux: bool,
    pub lan: bool,
    pub os: String,
    pub server_type: String,
    pub version: String,
    pub region: u8,
    pub product: String,
    pub app_id: u32,
    pub gametype: Vec<String>,
    pub last_heartbeat: f64,
}

impl MasterEntry {
    /// Construct a [`MasterEntry`] from a validated heartbeat update.
    pub fn from_heartbeat(addr: SocketAddrV4, info: &HeartbeatInfo, now_secs: f64) -> Self {
        let gamedir = info
            .gamedir
            .clone()
            .or_else(|| info.product.clone())
            .unwrap_or_default();
        let server_type = info.server_type.clone().unwrap_or_else(|| "d".to_string());
        let os = info.os.clone().unwrap_or_else(|| "l".to_string());
        let dedicated = server_type.eq_ignore_ascii_case("d");
        let proxy = server_type.eq_ignore_ascii_case("p")
            || info
                .fields
                .get("proxytarget")
                .is_some_and(|v| !v.is_empty());
        let linux = os.eq_ignore_ascii_case("l");
        let app_id = info
            .app_id
            .unwrap_or_else(|| default_app_id_for_gamedir(&gamedir));
        let gametype = info
            .gametype
            .as_deref()
            .map(|raw| {
                raw.split(',')
                    .map(|t| t.trim().to_ascii_lowercase())
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        Self {
            addr,
            protocol: info.protocol.unwrap_or(48),
            gamedir,
            map: info.map.clone().unwrap_or_default(),
            players: info.players.unwrap_or(0),
            max_players: info.max_players.unwrap_or(0),
            bots: info.bots.unwrap_or(0),
            dedicated,
            proxy,
            secure: info.secure.unwrap_or(false),
            password: info.password.unwrap_or(false),
            linux,
            lan: info.lan.unwrap_or(false),
            os,
            server_type,
            version: info.version.clone().unwrap_or_default(),
            region: info.region.unwrap_or(REGION_ALL),
            product: info.product.clone().unwrap_or_default(),
            app_id,
            gametype,
            last_heartbeat: now_secs,
        }
    }
}

fn default_app_id_for_gamedir(gamedir: &str) -> u32 {
    if gamedir.eq_ignore_ascii_case("cstrike") {
        10
    } else if gamedir.eq_ignore_ascii_case("tfc") {
        20
    } else if gamedir.eq_ignore_ascii_case("dod") {
        30
    } else if gamedir.eq_ignore_ascii_case("dmc") {
        40
    } else if gamedir.eq_ignore_ascii_case("gearbox") {
        50
    } else if gamedir.eq_ignore_ascii_case("ricochet") {
        60
    } else if gamedir.eq_ignore_ascii_case("valve") {
        70
    } else if gamedir.eq_ignore_ascii_case("czero") {
        80
    } else {
        0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FilterPredicate {
    GameDir(String),
    Map(String),
    NotEmpty(bool),
    NotFull(bool),
    NoPlayers(bool),
    Proxy(bool),
    Napp(u32),
    AppId(u32),
    Dedicated(bool),
    Secure(bool),
    Linux(bool),
    Password(bool),
    ServerType(String),
    GameType(Vec<String>),
    Region(u8),
    AlwaysTrue,
}

impl FilterPredicate {
    fn matches(&self, entry: &MasterEntry) -> bool {
        match self {
            Self::GameDir(v) => entry.gamedir.eq_ignore_ascii_case(v),
            Self::Map(v) => entry.map.eq_ignore_ascii_case(v),
            Self::NotEmpty(flag) => {
                if *flag {
                    entry.players > 0
                } else {
                    entry.players == 0
                }
            }
            Self::NotFull(flag) => {
                if *flag {
                    entry.players < entry.max_players
                } else {
                    entry.players >= entry.max_players
                }
            }
            Self::NoPlayers(flag) => {
                if *flag {
                    entry.players == 0
                } else {
                    entry.players > 0
                }
            }
            Self::Proxy(flag) => entry.proxy == *flag,
            Self::Napp(id) => entry.app_id != *id,
            Self::AppId(id) => entry.app_id == *id,
            Self::Dedicated(flag) => entry.dedicated == *flag,
            Self::Secure(flag) => entry.secure == *flag,
            Self::Linux(flag) => entry.linux == *flag,
            Self::Password(flag) => entry.password == *flag,
            Self::ServerType(t) => entry.server_type.eq_ignore_ascii_case(t),
            Self::GameType(tags) => tags
                .iter()
                .all(|req| entry.gametype.iter().any(|have| have == req)),
            Self::Region(r) => *r == REGION_ALL || entry.region == *r,
            Self::AlwaysTrue => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FilterExpr {
    Pred(FilterPredicate),
    Nor(Vec<FilterExpr>),
    Nand(Vec<FilterExpr>),
}

impl FilterExpr {
    fn matches(&self, entry: &MasterEntry) -> bool {
        match self {
            Self::Pred(p) => p.matches(entry),
            Self::Nor(children) => !children.iter().any(|c| c.matches(entry)),
            Self::Nand(children) => !children.iter().all(|c| c.matches(entry)),
        }
    }
}

/// Parsed GoldSrc Master Server Query filter (`region` + backslash filter expression).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MasterFilter {
    pub region: Option<u8>,
    exprs: Vec<FilterExpr>,
}

impl MasterFilter {
    /// Parse a GoldSrc `0x31` region byte and backslash filter string.
    ///
    /// Supports `\gamedir\`, `\map\`, `\empty\1`, `\full\1`, `\noplayers\1`, `\proxy\1`,
    /// `\napp\<id>`, `\appid\<id>`, `\dedicated\1`, `\secure\1`, `\linux\1`, `\password\0|1`,
    /// `\type\`, `\gametype\`, `\nor\<N>`, and `\nand\<N>`.
    pub fn parse(region: u8, filter_str: &str) -> Result<Self, MasterError> {
        let region_opt = if region == REGION_ALL {
            None
        } else {
            Some(region)
        };
        let trimmed = filter_str.trim_matches('\0');
        let stripped = trimmed.strip_prefix('\\').unwrap_or(trimmed);
        if stripped.is_empty() {
            return Ok(Self {
                region: region_opt,
                exprs: Vec::new(),
            });
        }

        let mut pairs = Vec::new();
        let mut parts = stripped.split('\\');
        while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
            if k.is_empty() {
                continue;
            }
            pairs.push((k.to_ascii_lowercase(), v.to_string()));
        }

        let mut idx = 0usize;
        let mut exprs = Vec::new();
        while idx < pairs.len() {
            exprs.push(parse_filter_expr(&pairs, &mut idx, 0)?);
        }

        Ok(Self {
            region: region_opt,
            exprs,
        })
    }

    /// Returns `true` if `entry` satisfies the region and every filter expression.
    pub fn matches(&self, entry: &MasterEntry) -> bool {
        if let Some(region) = self.region
            && region != REGION_ALL
            && entry.region != region
        {
            return false;
        }
        self.exprs.iter().all(|expr| expr.matches(entry))
    }
}

fn parse_filter_bool(key: &str, val: &str) -> Result<bool, MasterError> {
    match val.trim() {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        other => Err(MasterError::InvalidFilter(format!(
            "invalid boolean value '{other}' for filter '{key}'"
        ))),
    }
}

fn parse_filter_u32(key: &str, val: &str) -> Result<u32, MasterError> {
    val.trim().parse::<u32>().map_err(|_| {
        MasterError::InvalidFilter(format!("invalid integer value '{val}' for filter '{key}'"))
    })
}

fn parse_filter_expr(
    pairs: &[(String, String)],
    idx: &mut usize,
    depth: usize,
) -> Result<FilterExpr, MasterError> {
    if depth >= MAX_FILTER_DEPTH {
        return Err(MasterError::InvalidFilter(
            "filter nesting depth exceeded".to_string(),
        ));
    }
    if *idx >= pairs.len() {
        return Err(MasterError::InvalidFilter(
            "unexpected end of filter expression".to_string(),
        ));
    }

    let (key, val) = &pairs[*idx];
    *idx += 1;

    match key.as_str() {
        "nor" | "nand" => {
            let count = val.trim().parse::<usize>().map_err(|_| {
                MasterError::InvalidFilter(format!(
                    "invalid combinator count '{val}' for '\\{key}\\'"
                ))
            })?;
            let mut children = Vec::with_capacity(count.min(MAX_FILTER_DEPTH));
            for _ in 0..count {
                if *idx >= pairs.len() {
                    return Err(MasterError::InvalidFilter(format!(
                        "'\\{key}\\{count}' expects more filter terms"
                    )));
                }
                children.push(parse_filter_expr(pairs, idx, depth + 1)?);
            }
            if key == "nor" {
                Ok(FilterExpr::Nor(children))
            } else {
                Ok(FilterExpr::Nand(children))
            }
        }
        "gamedir" => Ok(FilterExpr::Pred(FilterPredicate::GameDir(val.clone()))),
        "map" => Ok(FilterExpr::Pred(FilterPredicate::Map(val.clone()))),
        "empty" => Ok(FilterExpr::Pred(FilterPredicate::NotEmpty(
            parse_filter_bool(key, val)?,
        ))),
        "full" => Ok(FilterExpr::Pred(FilterPredicate::NotFull(
            parse_filter_bool(key, val)?,
        ))),
        "noplayers" => Ok(FilterExpr::Pred(FilterPredicate::NoPlayers(
            parse_filter_bool(key, val)?,
        ))),
        "proxy" => Ok(FilterExpr::Pred(FilterPredicate::Proxy(parse_filter_bool(
            key, val,
        )?))),
        "napp" => Ok(FilterExpr::Pred(FilterPredicate::Napp(parse_filter_u32(
            key, val,
        )?))),
        "appid" => Ok(FilterExpr::Pred(FilterPredicate::AppId(parse_filter_u32(
            key, val,
        )?))),
        "dedicated" => Ok(FilterExpr::Pred(FilterPredicate::Dedicated(
            parse_filter_bool(key, val)?,
        ))),
        "secure" => Ok(FilterExpr::Pred(FilterPredicate::Secure(
            parse_filter_bool(key, val)?,
        ))),
        "linux" => Ok(FilterExpr::Pred(FilterPredicate::Linux(parse_filter_bool(
            key, val,
        )?))),
        "password" => Ok(FilterExpr::Pred(FilterPredicate::Password(
            parse_filter_bool(key, val)?,
        ))),
        "type" => Ok(FilterExpr::Pred(FilterPredicate::ServerType(val.clone()))),
        "gametype" => {
            let tags = val
                .split(',')
                .map(|t| t.trim().to_ascii_lowercase())
                .filter(|t| !t.is_empty())
                .collect();
            Ok(FilterExpr::Pred(FilterPredicate::GameType(tags)))
        }
        "region" => {
            let r = parse_filter_u32(key, val)?;
            let r_u8 = u8::try_from(r).map_err(|_| {
                MasterError::InvalidFilter(format!("region out of range: {r}"))
            })?;
            Ok(FilterExpr::Pred(FilterPredicate::Region(r_u8)))
        }
        other => {
            debug!(filter_key = other, filter_val = val, "ignoring unknown master filter key");
            Ok(FilterExpr::Pred(FilterPredicate::AlwaysTrue))
        }
    }
}

#[derive(Debug, Clone)]
struct ChallengeRecord {
    challenge: u32,
    issued_at: f64,
}

/// Bounded table of issued heartbeat challenges keyed by server IPv4 socket address.
#[derive(Debug, Clone)]
pub struct ChallengeTable {
    records: BTreeMap<SocketAddrV4, ChallengeRecord>,
    max_challenges: usize,
    ttl_secs: f64,
    prng_state: u64,
    counter: u64,
}

impl Default for ChallengeTable {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CHALLENGES, DEFAULT_CHALLENGE_TTL_SECS)
    }
}

impl ChallengeTable {
    /// Create a bounded challenge table with the given capacity and TTL in seconds.
    pub fn new(max_challenges: usize, ttl_secs: f64) -> Self {
        Self {
            records: BTreeMap::new(),
            max_challenges: max_challenges.max(1),
            ttl_secs: ttl_secs.max(0.0),
            prng_state: 0x6A09_E667_F3BC_C909,
            counter: 0,
        }
    }

    /// Issue and store a non-zero challenge for `addr` at `now_secs`.
    pub fn issue(&mut self, addr: SocketAddrV4, now_secs: f64) -> u32 {
        if !self.records.contains_key(&addr) && self.records.len() >= self.max_challenges {
            self.expire_stale(now_secs);
            if self.records.len() >= self.max_challenges
                && let Some(oldest_addr) = self
                    .records
                    .iter()
                    .min_by(|a, b| a.1.issued_at.total_cmp(&b.1.issued_at))
                    .map(|(k, _)| *k)
            {
                self.records.remove(&oldest_addr);
            }
        }

        let challenge = self.next_challenge(addr, now_secs);
        self.records.insert(
            addr,
            ChallengeRecord {
                challenge,
                issued_at: now_secs,
            },
        );
        challenge
    }

    /// Verify and consume a challenge for `addr` at `now_secs`.
    pub fn verify_and_consume(
        &mut self,
        addr: SocketAddrV4,
        challenge: u32,
        now_secs: f64,
    ) -> bool {
        let Some(record) = self.records.remove(&addr) else {
            return false;
        };
        let age = now_secs - record.issued_at;
        age >= 0.0 && age <= self.ttl_secs && record.challenge == challenge
    }

    /// Remove an outstanding challenge for `addr` if present.
    pub fn remove(&mut self, addr: &SocketAddrV4) {
        self.records.remove(addr);
    }

    /// Purge expired challenges and return the number removed.
    pub fn expire_stale(&mut self, now_secs: f64) -> usize {
        let before = self.records.len();
        self.records
            .retain(|_, rec| (now_secs - rec.issued_at) <= self.ttl_secs && now_secs >= rec.issued_at);
        before - self.records.len()
    }

    /// Number of currently tracked outstanding challenges.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Returns `true` when there are no outstanding challenges.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    fn next_challenge(&mut self, addr: SocketAddrV4, now_secs: f64) -> u32 {
        self.counter = self.counter.wrapping_add(1);
        let ip_bits = u32::from(*addr.ip()) as u64;
        let port_bits = addr.port() as u64;
        let mut z = self
            .prng_state
            .wrapping_add(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(self.counter)
            ^ (ip_bits << 16 | port_bits)
            ^ now_secs.to_bits();
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        self.prng_state = z;
        let ch = (z as u32) ^ ((z >> 32) as u32);
        if ch == 0 { 1 } else { ch }
    }
}

/// In-memory GoldSrc Master Server registry with bounded capacity and TTL expiration.
#[derive(Debug, Clone)]
pub struct MasterRegistry {
    entries: BTreeMap<SocketAddrV4, MasterEntry>,
    challenges: ChallengeTable,
    ttl_secs: f64,
    max_entries: usize,
}

impl Default for MasterRegistry {
    fn default() -> Self {
        Self::new(
            DEFAULT_TTL_SECS,
            DEFAULT_MAX_ENTRIES,
            DEFAULT_MAX_CHALLENGES,
        )
    }
}

impl MasterRegistry {
    /// Create a registry with custom TTL, max entries, and max outstanding challenges.
    pub fn new(ttl_secs: f64, max_entries: usize, max_challenges: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            challenges: ChallengeTable::new(max_challenges, DEFAULT_CHALLENGE_TTL_SECS),
            ttl_secs: ttl_secs.max(0.0),
            max_entries: max_entries.max(1),
        }
    }

    /// Configured registration TTL in seconds.
    pub fn ttl_secs(&self) -> f64 {
        self.ttl_secs
    }

    /// Issue a heartbeat challenge for `addr` at `now_secs`.
    pub fn issue_challenge(&mut self, addr: SocketAddrV4, now_secs: f64) -> u32 {
        self.challenges.issue(addr, now_secs)
    }

    /// Handle an incoming heartbeat datagram (`q`, `0\n...`, or `b`) from `addr`.
    ///
    /// Returns `Ok(Some(reply_bytes))` when a challenge packet should be sent back,
    /// `Ok(None)` when the update or shutdown succeeded without a reply, or an error
    /// if the packet or challenge is invalid.
    pub fn handle_heartbeat(
        &mut self,
        addr: SocketAddrV4,
        pkt: &[u8],
        now_secs: f64,
    ) -> Result<Option<Vec<u8>>, MasterError> {
        match parse_heartbeat_packet(pkt)? {
            HeartbeatPacket::ChallengeRequest => {
                let challenge = self.issue_challenge(addr, now_secs);
                Ok(Some(encode_heartbeat_challenge(challenge)))
            }
            HeartbeatPacket::Update(info) => {
                let challenge = info.challenge.ok_or(MasterError::InvalidChallenge)?;
                if !self.challenges.verify_and_consume(addr, challenge, now_secs) {
                    return Err(MasterError::InvalidChallenge);
                }
                self.expire_stale(now_secs);
                if !self.entries.contains_key(&addr)
                    && self.entries.len() >= self.max_entries
                    && let Some(oldest) = self
                        .entries
                        .iter()
                        .min_by(|a, b| a.1.last_heartbeat.total_cmp(&b.1.last_heartbeat))
                        .map(|(k, _)| *k)
                {
                    self.entries.remove(&oldest);
                }
                let entry = MasterEntry::from_heartbeat(addr, &info, now_secs);
                self.entries.insert(addr, entry);
                Ok(None)
            }
            HeartbeatPacket::Shutdown => {
                self.entries.remove(&addr);
                self.challenges.remove(&addr);
                Ok(None)
            }
        }
    }

    /// Remove expired server entries and challenges, returning removed server count.
    pub fn expire_stale(&mut self, now_secs: f64) -> usize {
        self.challenges.expire_stale(now_secs);
        let ttl_secs = self.ttl_secs;
        let before = self.entries.len();
        self.entries.retain(|_, entry| {
            let age = now_secs - entry.last_heartbeat;
            age >= 0.0 && age <= ttl_secs
        });
        before - self.entries.len()
    }

    /// Query active servers matching `filter`, paginated after `cursor`.
    ///
    /// Returns `(addrs, has_more)`. A `cursor` of `None` or `0.0.0.0:0` starts at the beginning.
    pub fn query(
        &self,
        filter: &MasterFilter,
        cursor: Option<SocketAddrV4>,
        max_per_page: usize,
        now_secs: f64,
    ) -> (Vec<SocketAddrV4>, bool) {
        if max_per_page == 0 {
            return (Vec::new(), false);
        }

        let start_bound = match cursor {
            Some(c) if c != SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0) => Bound::Excluded(c),
            _ => Bound::Unbounded,
        };

        let mut results = Vec::with_capacity(max_per_page.min(self.entries.len()));
        let mut has_more = false;
        for (addr, entry) in self.entries.range((start_bound, Bound::Unbounded)) {
            if !self.is_active(entry, now_secs) || !filter.matches(entry) {
                continue;
            }
            if results.len() == max_per_page {
                has_more = true;
                break;
            }
            results.push(*addr);
        }

        (results, has_more)
    }

    /// Serialize active server entries as a JSON array.
    pub fn to_json(&self, now_secs: f64) -> String {
        let mut out = String::from("[");
        let mut first = true;
        for entry in self.entries.values() {
            if !self.is_active(entry, now_secs) {
                continue;
            }
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str("{\"addr\":");
            push_json_str(&mut out, &entry.addr.to_string());
            out.push_str(",\"gamedir\":");
            push_json_str(&mut out, &entry.gamedir);
            out.push_str(",\"map\":");
            push_json_str(&mut out, &entry.map);
            let _ = write!(
                out,
                ",\"players\":{},\"max\":{},\"bots\":{},\"proxy\":{},\"secure\":{},\"dedicated\":{},\"password\":{},\"region\":{}}}",
                entry.players,
                entry.max_players,
                entry.bots,
                entry.proxy,
                entry.secure,
                entry.dedicated,
                entry.password,
                entry.region
            );
        }
        out.push(']');
        out
    }

    /// Look up a server entry by address.
    pub fn get(&self, addr: &SocketAddrV4) -> Option<&MasterEntry> {
        self.entries.get(addr)
    }

    /// Total number of stored entries (including not-yet-purged stale entries).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` when no entries are stored.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of active (non-expired) entries at `now_secs`.
    pub fn active_len(&self, now_secs: f64) -> usize {
        self.entries
            .values()
            .filter(|e| self.is_active(e, now_secs))
            .count()
    }

    fn is_active(&self, entry: &MasterEntry, now_secs: f64) -> bool {
        let age = now_secs - entry.last_heartbeat;
        age >= 0.0 && age <= self.ttl_secs
    }
}

pub(crate) fn push_json_str(out: &mut String, s: &str) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        encode_heartbeat_shutdown, encode_heartbeat_update, parse_heartbeat_challenge,
    };

    fn addr(last_octet: u8, port: u16) -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, last_octet), port)
    }

    fn register_server(
        reg: &mut MasterRegistry,
        server: SocketAddrV4,
        now: f64,
        extra: &[(&str, &str)],
    ) {
        let reply = reg.handle_heartbeat(server, b"q", now).unwrap().unwrap();
        let ch = parse_heartbeat_challenge(&reply).unwrap();
        let ch_str = ch.to_string();
        let mut kv = vec![
            ("protocol", "48"),
            ("challenge", ch_str.as_str()),
            ("gamedir", "cstrike"),
            ("map", "de_dust2"),
            ("players", "4"),
            ("max", "32"),
            ("bots", "0"),
            ("type", "d"),
            ("password", "0"),
            ("os", "l"),
            ("secure", "1"),
            ("region", "3"),
        ];
        kv.extend_from_slice(extra);
        let pkt = encode_heartbeat_update(&kv);
        assert!(reg.handle_heartbeat(server, &pkt, now).unwrap().is_none());
    }

    #[test]
    fn heartbeat_challenge_registration_and_shutdown() {
        let mut reg = MasterRegistry::default();
        let s1 = addr(1, 27015);
        register_server(&mut reg, s1, 10.0, &[]);
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.get(&s1).unwrap().map, "de_dust2");

        // Replaying the same challenge must be rejected.
        let replay = encode_heartbeat_update(&[("challenge", "1"), ("gamedir", "cstrike")]);
        assert!(matches!(
            reg.handle_heartbeat(s1, &replay, 11.0),
            Err(MasterError::InvalidChallenge)
        ));

        // Shutdown removes the entry.
        reg.handle_heartbeat(s1, &encode_heartbeat_shutdown(), 12.0)
            .unwrap();
        assert!(reg.is_empty());
    }

    #[test]
    fn ttl_expiry_and_bounded_capacity() {
        let mut reg = MasterRegistry::new(100.0, 2, 2);
        let s1 = addr(1, 27015);
        let s2 = addr(2, 27015);
        let s3 = addr(3, 27015);

        register_server(&mut reg, s1, 10.0, &[]);
        register_server(&mut reg, s2, 20.0, &[]);
        // Inserting s3 at capacity 2 evicts oldest (s1).
        register_server(&mut reg, s3, 30.0, &[]);
        assert_eq!(reg.len(), 2);
        assert!(reg.get(&s1).is_none());
        assert!(reg.get(&s2).is_some());
        assert!(reg.get(&s3).is_some());

        // At t = 125.0, s2 (last=20.0, age=105.0 > 100.0) expires while s3 remains active.
        assert_eq!(reg.expire_stale(125.0), 1);
        assert_eq!(reg.active_len(125.0), 1);
        assert!(reg.get(&s3).is_some());
    }

    #[test]
    fn challenge_table_is_bounded_and_expires() {
        let mut table = ChallengeTable::new(2, 15.0);
        let c1 = table.issue(addr(1, 27015), 1.0);
        let c2 = table.issue(addr(2, 27015), 2.0);
        assert_eq!(table.len(), 2);

        // Issuing a 3rd evicts oldest (addr 1).
        let c3 = table.issue(addr(3, 27015), 3.0);
        assert_eq!(table.len(), 2);
        assert!(!table.verify_and_consume(addr(1, 27015), c1, 4.0));
        assert!(table.verify_and_consume(addr(2, 27015), c2, 10.0));
        // c3 expires after 15.0s (issued at 3.0, checked at 20.0).
        assert!(!table.verify_and_consume(addr(3, 27015), c3, 20.0));
    }

    #[test]
    fn filters_and_nor_nand_work_correctly() {
        let mut reg = MasterRegistry::default();
        let active = addr(1, 27015);
        let empty = addr(2, 27015);
        let full = addr(3, 27015);
        let proxy = addr(4, 27020);
        let win_cz = addr(5, 27015);

        register_server(
            &mut reg,
            active,
            10.0,
            &[("gametype", "5on5,eac"), ("players", "10"), ("max", "12")],
        );
        register_server(&mut reg, empty, 10.0, &[("players", "0"), ("max", "32")]);
        register_server(&mut reg, full, 10.0, &[("players", "32"), ("max", "32")]);
        register_server(
            &mut reg,
            proxy,
            10.0,
            &[("type", "p"), ("players", "0"), ("max", "64")],
        );
        register_server(
            &mut reg,
            win_cz,
            10.0,
            &[
                ("gamedir", "czero"),
                ("map", "de_inferno"),
                ("os", "w"),
                ("secure", "0"),
                ("password", "1"),
                ("region", "1"),
            ],
        );

        let f1 = MasterFilter::parse(
            3,
            "\\gamedir\\cstrike\\map\\de_dust2\\empty\\1\\full\\1\\dedicated\\1\\secure\\1\\linux\\1\\napp\\500\\gametype\\5on5",
        )
        .unwrap();
        assert_eq!(reg.query(&f1, None, 100, 10.0).0, vec![active]);

        let f_empty_only = MasterFilter::parse(REGION_ALL, "\\noplayers\\1\\nor\\1\\proxy\\1").unwrap();
        assert_eq!(reg.query(&f_empty_only, None, 100, 10.0).0, vec![empty]);

        let f_proxy = MasterFilter::parse(REGION_ALL, "\\proxy\\1").unwrap();
        assert_eq!(reg.query(&f_proxy, None, 100, 10.0).0, vec![proxy]);

        // Exclude servers that are BOTH gamedir=czero AND password=1 using \nand\2.
        let f_nand = MasterFilter::parse(REGION_ALL, "\\nand\\2\\gamedir\\czero\\password\\1").unwrap();
        let res_nand = reg.query(&f_nand, None, 100, 10.0).0;
        assert!(!res_nand.contains(&win_cz));
        assert_eq!(res_nand.len(), 4);
    }

    #[test]
    fn query_pagination_uses_cursor_deterministically() {
        let mut reg = MasterRegistry::default();
        let addrs: Vec<SocketAddrV4> = (1..=5).map(|i| addr(i, 27015)).collect();
        for &a in &addrs {
            register_server(&mut reg, a, 10.0, &[]);
        }

        let filter = MasterFilter::default();
        let (p1, m1) = reg.query(&filter, None, 2, 10.0);
        assert_eq!(p1, vec![addrs[0], addrs[1]]);
        assert!(m1);

        let (p2, m2) = reg.query(&filter, p1.last().copied(), 2, 10.0);
        assert_eq!(p2, vec![addrs[2], addrs[3]]);
        assert!(m2);

        let (p3, m3) = reg.query(&filter, p2.last().copied(), 2, 10.0);
        assert_eq!(p3, vec![addrs[4]]);
        assert!(!m3);
    }

    #[test]
    fn to_json_formats_and_escapes_correctly() {
        let mut reg = MasterRegistry::default();
        let s = addr(10, 27015);
        register_server(
            &mut reg,
            s,
            5.0,
            &[("map", "de_dust2"), ("bots", "2"), ("players", "8")],
        );
        reg.entries.get_mut(&s).unwrap().map = "de_dust2\"test\\map".to_string();
        let json = reg.to_json(5.0);
        assert_eq!(
            json,
            "[{\"addr\":\"192.0.2.10:27015\",\"gamedir\":\"cstrike\",\"map\":\"de_dust2\\\"test\\\\map\",\"players\":8,\"max\":32,\"bots\":2,\"proxy\":false,\"secure\":true,\"dedicated\":true,\"password\":false,\"region\":3}]"
        );
    }
}
