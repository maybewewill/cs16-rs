//! WASM-compatible lobby directory and signaling queue for `?join=<host:port>&room=<code>`
//! invites and browser WebRTC/WebSocket matchmaking (`/lobbies`).

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;

use crate::registry::push_json_str;

/// Default lobby expiration TTL in seconds.
pub const DEFAULT_LOBBY_TTL_SECS: f64 = 180.0;
/// Default maximum active lobby rooms retained in memory.
pub const DEFAULT_MAX_LOBBIES: usize = 512;
/// Maximum signaling messages retained per lobby room.
pub const MAX_SIGNALS_PER_LOBBY: usize = 64;
/// Maximum payload size for a single signaling message (SDP/ICE).
pub const MAX_SIGNAL_PAYLOAD_BYTES: usize = 4096;

/// Errors returned by [`LobbyRegistry`] operations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LobbyError {
    #[error("invalid room code")]
    InvalidRoom,
    #[error("invalid lobby host")]
    InvalidHost,
    #[error("invalid lobby port")]
    InvalidPort,
    #[error("invalid lobby request payload: {0}")]
    InvalidPayload(&'static str),
    #[error("lobby room not found")]
    NotFound,
}

/// Validates a lobby room code (`[A-Za-z0-9_-]`, 1..=32 bytes).
pub fn valid_room_code(room: &str) -> bool {
    !room.is_empty()
        && room.len() <= 32
        && room
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Validates a lobby host/hostname (`[A-Za-z0-9._:-]`, 1..=253 bytes).
pub fn valid_lobby_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':'))
}

fn valid_peer_label(peer: &str) -> bool {
    !peer.is_empty()
        && peer.len() <= 64
        && peer
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// One WebRTC/WebSocket signaling message queued in a lobby room.
#[derive(Debug, Clone, PartialEq)]
pub struct LobbySignal {
    pub id: u64,
    pub from: String,
    pub to: Option<String>,
    pub kind: String,
    pub payload: String,
    pub timestamp: f64,
}

impl LobbySignal {
    pub fn to_json(&self) -> String {
        let mut out = String::with_capacity(128 + self.payload.len());
        let _ = write!(out, "{{\"id\":{},\"from\":", self.id);
        push_json_str(&mut out, &self.from);
        out.push_str(",\"to\":");
        match self.to.as_deref() {
            Some(to) => push_json_str(&mut out, to),
            None => out.push_str("null"),
        }
        out.push_str(",\"kind\":");
        push_json_str(&mut out, &self.kind);
        out.push_str(",\"payload\":");
        push_json_str(&mut out, &self.payload);
        let _ = write!(out, ",\"timestamp\":{:.2}}}", self.timestamp.max(0.0));
        out
    }
}

/// Registered game lobby room entry.
#[derive(Debug, Clone, PartialEq)]
pub struct LobbyInfo {
    pub room: String,
    pub host: String,
    pub port: u16,
    pub map: String,
    pub players: u8,
    pub max_players: u8,
    pub mode: String,
    pub created_at: f64,
    pub last_seen: f64,
}

impl LobbyInfo {
    /// Returns the `?join=<host:port>&room=<code>` invite suffix for this lobby.
    pub fn invite_query(&self) -> String {
        format!("?join={}:{}&room={}", self.host, self.port, self.room)
    }

    pub fn to_json(&self) -> String {
        let mut out = String::with_capacity(192);
        out.push_str("{\"room\":");
        push_json_str(&mut out, &self.room);
        out.push_str(",\"host\":");
        push_json_str(&mut out, &self.host);
        let _ = write!(out, ",\"port\":{}", self.port);
        out.push_str(",\"map\":");
        push_json_str(&mut out, &self.map);
        let _ = write!(
            out,
            ",\"players\":{},\"max_players\":{},\"mode\":",
            self.players, self.max_players
        );
        push_json_str(&mut out, &self.mode);
        out.push_str(",\"invite_query\":");
        push_json_str(&mut out, &self.invite_query());
        let _ = write!(out, ",\"last_seen\":{:.2}}}", self.last_seen.max(0.0));
        out
    }
}

#[derive(Debug, Clone)]
struct LobbyRecord {
    info: LobbyInfo,
    next_signal_id: u64,
    signals: VecDeque<LobbySignal>,
}

/// Bounded in-memory lobby room & signaling registry.
#[derive(Debug, Clone)]
pub struct LobbyRegistry {
    rooms: BTreeMap<String, LobbyRecord>,
    ttl_secs: f64,
    max_lobbies: usize,
}

impl Default for LobbyRegistry {
    fn default() -> Self {
        Self::new(DEFAULT_LOBBY_TTL_SECS, DEFAULT_MAX_LOBBIES)
    }
}

impl LobbyRegistry {
    pub fn new(ttl_secs: f64, max_lobbies: usize) -> Self {
        Self {
            rooms: BTreeMap::new(),
            ttl_secs: ttl_secs.max(1.0),
            max_lobbies: max_lobbies.max(1),
        }
    }

    /// Removes expired lobby rooms and returns the number purged.
    pub fn expire_stale(&mut self, now_secs: f64) -> usize {
        let ttl = self.ttl_secs;
        let before = self.rooms.len();
        self.rooms.retain(|_, rec| {
            let age = now_secs - rec.info.last_seen;
            age >= 0.0 && age <= ttl
        });
        before - self.rooms.len()
    }

    /// Creates or refreshes a lobby entry.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert(
        &mut self,
        room: &str,
        host: &str,
        port: u16,
        map: &str,
        players: u8,
        max_players: u8,
        mode: &str,
        now_secs: f64,
    ) -> Result<LobbyInfo, LobbyError> {
        let room = room.trim();
        let host = host.trim();
        if !valid_room_code(room) {
            return Err(LobbyError::InvalidRoom);
        }
        if !valid_lobby_host(host) {
            return Err(LobbyError::InvalidHost);
        }
        if port == 0 {
            return Err(LobbyError::InvalidPort);
        }
        let clean_mode = match mode.trim() {
            "webrtc" => "webrtc",
            _ => "ws",
        };
        let clean_map = if map.trim().is_empty() {
            "de_dust2".to_string()
        } else {
            map.trim().chars().take(64).collect()
        };

        self.expire_stale(now_secs);
        if !self.rooms.contains_key(room)
            && self.rooms.len() >= self.max_lobbies
            && let Some(oldest) = self
                .rooms
                .iter()
                .min_by(|a, b| a.1.info.last_seen.total_cmp(&b.1.info.last_seen))
                .map(|(k, _)| k.clone())
        {
            self.rooms.remove(&oldest);
        }

        let created_at = self
            .rooms
            .get(room)
            .map_or(now_secs, |r| r.info.created_at);
        let info = LobbyInfo {
            room: room.to_string(),
            host: host.to_string(),
            port,
            map: clean_map,
            players,
            max_players: max_players.max(1),
            mode: clean_mode.to_string(),
            created_at,
            last_seen: now_secs,
        };
        if let Some(rec) = self.rooms.get_mut(room) {
            rec.info = info.clone();
        } else {
            self.rooms.insert(
                room.to_string(),
                LobbyRecord {
                    info: info.clone(),
                    next_signal_id: 1,
                    signals: VecDeque::new(),
                },
            );
        }
        Ok(info)
    }

    pub fn get(&self, room: &str, now_secs: f64) -> Option<&LobbyInfo> {
        if !valid_room_code(room) {
            return None;
        }
        self.rooms.get(room).and_then(|r| {
            let age = now_secs - r.info.last_seen;
            (age >= 0.0 && age <= self.ttl_secs).then_some(&r.info)
        })
    }

    pub fn remove(&mut self, room: &str) -> bool {
        valid_room_code(room) && self.rooms.remove(room).is_some()
    }

    pub fn len(&self) -> usize {
        self.rooms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rooms.is_empty()
    }

    pub fn list_json(&self, now_secs: f64) -> String {
        let mut out = String::from("[");
        let mut first = true;
        for rec in self.rooms.values() {
            let age = now_secs - rec.info.last_seen;
            if age < 0.0 || age > self.ttl_secs {
                continue;
            }
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&rec.info.to_json());
        }
        out.push(']');
        out
    }

    /// Appends a WebRTC/room signaling message to `room`.
    pub fn push_signal(
        &mut self,
        room: &str,
        from: &str,
        to: Option<&str>,
        kind: &str,
        payload: &str,
        now_secs: f64,
    ) -> Result<LobbySignal, LobbyError> {
        if !valid_room_code(room) {
            return Err(LobbyError::InvalidRoom);
        }
        if !valid_peer_label(from.trim()) {
            return Err(LobbyError::InvalidPayload("invalid 'from' peer"));
        }
        let clean_to = match to.map(str::trim).filter(|s| !s.is_empty()) {
            Some(t) if valid_peer_label(t) => Some(t.to_string()),
            Some(_) => return Err(LobbyError::InvalidPayload("invalid 'to' peer")),
            None => None,
        };
        let clean_kind = match kind.trim() {
            k @ ("offer" | "answer" | "ice" | "join") => k.to_string(),
            _ => return Err(LobbyError::InvalidPayload("invalid signal kind")),
        };
        if payload.len() > MAX_SIGNAL_PAYLOAD_BYTES {
            return Err(LobbyError::InvalidPayload("signal payload too large"));
        }

        self.expire_stale(now_secs);
        let rec = self.rooms.get_mut(room).ok_or(LobbyError::NotFound)?;
        rec.info.last_seen = now_secs;
        let id = rec.next_signal_id;
        rec.next_signal_id = rec.next_signal_id.saturating_add(1);
        let sig = LobbySignal {
            id,
            from: from.trim().to_string(),
            to: clean_to,
            kind: clean_kind,
            payload: payload.to_string(),
            timestamp: now_secs,
        };
        if rec.signals.len() >= MAX_SIGNALS_PER_LOBBY {
            rec.signals.pop_front();
        }
        rec.signals.push_back(sig.clone());
        Ok(sig)
    }

    /// Returns signaling messages in `room` with `id > since_id`, optionally filtered by `target`.
    pub fn poll_signals(
        &self,
        room: &str,
        since_id: u64,
        target: Option<&str>,
        now_secs: f64,
    ) -> Result<Vec<LobbySignal>, LobbyError> {
        if !valid_room_code(room) {
            return Err(LobbyError::InvalidRoom);
        }
        let rec = self.rooms.get(room).ok_or(LobbyError::NotFound)?;
        let age = now_secs - rec.info.last_seen;
        if age < 0.0 || age > self.ttl_secs {
            return Err(LobbyError::NotFound);
        }
        let list = rec
            .signals
            .iter()
            .filter(|s| {
                s.id > since_id
                    && match target {
                        Some(t) => s.to.as_deref().is_none_or(|dst| dst == t),
                        None => true,
                    }
            })
            .cloned()
            .collect();
        Ok(list)
    }

    /// Serializes signaling messages returned by [`Self::poll_signals`] into a JSON array.
    pub fn signals_to_json(signals: &[LobbySignal]) -> String {
        let mut out = String::from("[");
        for (idx, s) in signals.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            out.push_str(&s.to_json());
        }
        out.push(']');
        out
    }

    /// Parses a flat JSON object for `POST /lobbies` and registers the lobby.
    pub fn upsert_from_json(
        &mut self,
        body: &[u8],
        now_secs: f64,
    ) -> Result<LobbyInfo, LobbyError> {
        let fields = parse_flat_json_fields(body).map_err(LobbyError::InvalidPayload)?;
        let room = fields
            .get("room")
            .ok_or(LobbyError::InvalidPayload("missing room"))?;
        let host = fields
            .get("host")
            .ok_or(LobbyError::InvalidPayload("missing host"))?;
        let port = fields
            .get("port")
            .and_then(|v| v.parse::<u16>().ok())
            .ok_or(LobbyError::InvalidPort)?;
        let map = fields.get("map").map(String::as_str).unwrap_or("de_dust2");
        let players = fields
            .get("players")
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(0);
        let max_players = fields
            .get("max_players")
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(16);
        let mode = fields.get("mode").map(String::as_str).unwrap_or("ws");
        self.upsert(room, host, port, map, players, max_players, mode, now_secs)
    }

    /// Parses a flat JSON object for `POST /lobbies/<room>/signal` and pushes the signal.
    pub fn push_signal_from_json(
        &mut self,
        room: &str,
        body: &[u8],
        now_secs: f64,
    ) -> Result<LobbySignal, LobbyError> {
        let fields = parse_flat_json_fields(body).map_err(LobbyError::InvalidPayload)?;
        let from = fields
            .get("from")
            .ok_or(LobbyError::InvalidPayload("missing from"))?;
        let to = fields.get("to").map(String::as_str);
        let kind = fields
            .get("kind")
            .ok_or(LobbyError::InvalidPayload("missing kind"))?;
        let payload = fields.get("payload").map(String::as_str).unwrap_or("");
        self.push_signal(room, from, to, kind, payload, now_secs)
    }
}

fn parse_flat_json_fields(body: &[u8]) -> Result<BTreeMap<String, String>, &'static str> {
    let text = std::str::from_utf8(body).map_err(|_| "body must be UTF-8 JSON")?;
    let mut rest = text
        .trim()
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or("expected JSON object")?
        .trim();
    let mut map = BTreeMap::new();
    while !rest.is_empty() {
        let (key, after_key) = parse_json_string(rest)?;
        let after_colon = after_key
            .trim_start()
            .strip_prefix(':')
            .ok_or("expected ':'")?
            .trim_start();
        if after_colon.starts_with('"') {
            let (val, after_val) = parse_json_string(after_colon)?;
            map.insert(key, val);
            rest = after_val.trim_start();
        } else {
            let end = after_colon.find(',').unwrap_or(after_colon.len());
            let token = after_colon[..end].trim();
            if token.is_empty() || token.starts_with('{') || token.starts_with('[') {
                return Err("unsupported JSON value");
            }
            if token != "null" {
                map.insert(key, token.to_string());
            }
            rest = after_colon[end..].trim_start();
        }
        if let Some(next) = rest.strip_prefix(',') {
            rest = next.trim_start();
            if rest.is_empty() {
                return Err("trailing comma");
            }
        } else if !rest.is_empty() {
            return Err("malformed JSON object");
        }
    }
    Ok(map)
}

fn parse_json_string(input: &str) -> Result<(String, &str), &'static str> {
    let after_open = input.strip_prefix('"').ok_or("expected JSON string")?;
    let mut chars = after_open.char_indices();
    let mut out = String::new();
    while let Some((idx, ch)) = chars.next() {
        match ch {
            '"' => return Ok((out, &after_open[idx + 1..])),
            '\\' => {
                let (_, esc) = chars.next().ok_or("unterminated JSON escape")?;
                match esc {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '/' => out.push('/'),
                    'b' => out.push('\u{08}'),
                    'f' => out.push('\u{0C}'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    _ => return Err("unsupported JSON escape"),
                }
            }
            c if (c as u32) < 0x20 => return Err("control character in JSON string"),
            c => out.push(c),
        }
    }
    Err("unterminated JSON string")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lobby_registration_signaling_ttl_and_validation() {
        let mut reg = LobbyRegistry::new(60.0, 4);
        assert!(reg
            .upsert("../bad", "127.0.0.1", 27016, "de_dust2", 1, 16, "ws", 10.0)
            .is_err());
        assert!(reg
            .upsert("dust-01", "play.example.com", 0, "de_dust2", 1, 16, "ws", 10.0)
            .is_err());

        let info = reg
            .upsert_from_json(
                br#"{"room":"dust-01","host":"play.example.com","port":27016,"map":"de_dust2","players":3,"max_players":16,"mode":"webrtc"}"#,
                10.0,
            )
            .unwrap();
        assert_eq!(info.invite_query(), "?join=play.example.com:27016&room=dust-01");
        assert!(reg.list_json(15.0).contains("\"room\":\"dust-01\""));

        let s1 = reg
            .push_signal_from_json(
                "dust-01",
                br#"{"from":"host","to":"guest1","kind":"offer","payload":"v=0"}"#,
                12.0,
            )
            .unwrap();
        assert_eq!(s1.id, 1);

        let polled = reg.poll_signals("dust-01", 0, Some("guest1"), 12.0).unwrap();
        assert_eq!(polled.len(), 1);
        assert_eq!(polled[0].payload, "v=0");
        assert!(reg.poll_signals("dust-01", 1, Some("guest1"), 12.0).unwrap().is_empty());

        assert_eq!(reg.expire_stale(80.0), 1);
        assert!(reg.is_empty());
    }
}
