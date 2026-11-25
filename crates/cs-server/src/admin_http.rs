//! Pure-Rust HTTP/1.1 router for the `cs-server` Web Admin Panel and JSON/replay API.
//!
//! Endpoints:
//! - `GET  /admin` — Web Admin HTML shell
//! - `GET  /api/status` — live server status snapshot (token-protected)
//! - `GET  /api/players` — connected players & bots (token-protected)
//! - `GET  /api/stats` — match leaderboard & round history (token-protected)
//! - `GET  /api/replays` — recorded HLDEMO v5 round index (token-protected)
//! - `GET  /api/replays/<id>.dem` — download `.dem` bytes (token-protected)
//! - `POST /api/rcon` — execute server console/RCON command (token-protected)
//! - `GET  /api/invite` — generate `?join=<host:port>&room=<code>` lobby link (token-protected)
//!
//! Zero `unsafe` code; constant-time token comparison against `rcon_password`; strict JSON
//! escaping and bounded request sizes.

use std::fmt::Write as _;
use std::io::{self, Read, Write};

use cs_transport::ws_proto::{HttpRequest, MAX_HTTP_HEAD, parse_http_request};

use crate::replays::{escape_json_str, valid_replay_id};

/// Maximum HTTP request body accepted by `/api/*` endpoints (8 KiB).
pub const MAX_API_BODY_BYTES: usize = 8192;
/// Maximum RCON command length accepted via `POST /api/rcon`.
pub const MAX_RCON_CMD_BYTES: usize = 1024;
/// Maximum accepted admin bearer/header token length.
pub const MAX_AUTH_TOKEN_BYTES: usize = 256;

/// Constant-time byte slice equality (no early return on content mismatch).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff: usize = a.len() ^ b.len();
    let max_len = a.len().max(b.len());
    for i in 0..max_len {
        let ba = *a.get(i).unwrap_or(&0);
        let bb = *b.get(i).unwrap_or(&0);
        diff |= usize::from(ba ^ bb);
    }
    diff == 0
}

/// Server snapshot returned by `GET /api/status`.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerStatusSnapshot {
    pub hostname: String,
    pub map: String,
    pub players: u8,
    pub bots: u8,
    pub max_players: u8,
    pub uptime_secs: f64,
    pub tick_rate: u32,
    pub round_number: u32,
    pub score_ct: u32,
    pub score_t: u32,
    pub game_port: u16,
    pub web_port: Option<u16>,
    pub bot_quota: u32,
    pub bot_difficulty: u8,
    pub available_maps: Vec<String>,
}

/// Connected client or bot snapshot returned by `GET /api/players`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSnapshot {
    pub slot: u8,
    pub userid: i32,
    pub name: String,
    pub team: String,
    pub frags: i32,
    pub deaths: i32,
    pub ping_ms: u32,
    pub state: String,
    pub addr: String,
    pub is_bot: bool,
}

/// Defaults used when `GET /api/invite` builds a lobby/join URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteConfig {
    pub default_host: String,
    pub web_port: u16,
    pub game_port: u16,
    pub default_room: String,
}

impl Default for InviteConfig {
    fn default() -> Self {
        Self {
            default_host: "127.0.0.1".to_string(),
            web_port: 27016,
            game_port: 27015,
            default_room: "alpha".to_string(),
        }
    }
}

/// Abstraction over live `cs-server` state so `AdminRouter` stays pure and testable.
pub trait AdminBackend {
    fn rcon_password(&self) -> &str;
    fn status(&self) -> ServerStatusSnapshot;
    fn players(&self) -> Vec<PlayerSnapshot>;
    fn stats_json(&self) -> String;
    fn replays_index_json(&self) -> String;
    fn replay_dem(&self, id: &str) -> Option<Vec<u8>>;
    fn execute_rcon(&mut self, command: &str) -> Result<String, String>;
    fn admin_html(&self) -> Option<&[u8]> {
        None
    }
    fn invite_config(&self) -> InviteConfig {
        InviteConfig::default()
    }
}

/// Incoming HTTP/1.1 request view passed to [`AdminRouter::handle`].
#[derive(Debug, Clone)]
pub struct AdminHttpRequest<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: Option<&'a str>,
    pub authorization: Option<&'a str>,
    pub x_admin_token: Option<&'a str>,
    pub host: Option<&'a str>,
    pub body: &'a [u8],
}

impl<'a> AdminHttpRequest<'a> {
    pub fn from_http_request(head: &'a HttpRequest, body: &'a [u8]) -> Self {
        Self {
            method: head.method.as_str(),
            path: head.path.as_str(),
            query: head.query.as_deref(),
            authorization: head.header("authorization"),
            x_admin_token: head.header("x-admin-token"),
            host: head.header("host"),
            body,
        }
    }
}

/// Owned HTTP/1.1 request head + body parsed from a stream.
#[derive(Debug, Clone)]
pub struct OwnedHttpRequest {
    pub head: HttpRequest,
    pub body: Vec<u8>,
}

/// Errors returned by [`read_http_request_with_body`].
#[derive(Debug)]
pub enum HttpReadError {
    Io(io::Error),
    InvalidHead(&'static str),
    PayloadTooLarge,
    UnsupportedTransferEncoding,
}

/// Reads an HTTP/1.1 request head (max `MAX_HTTP_HEAD`) and optional `Content-Length` body
/// (max `max_body` bytes) from `stream`.
pub fn read_http_request_with_body(
    stream: &mut impl Read,
    max_body: usize,
) -> Result<OwnedHttpRequest, HttpReadError> {
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 512];
    let (head, head_len) = loop {
        match parse_http_request(&buf) {
            Ok(Some(parsed)) => break parsed,
            Ok(None) => {}
            Err(_) => return Err(HttpReadError::InvalidHead("malformed HTTP request")),
        }
        let n = stream.read(&mut tmp).map_err(HttpReadError::Io)?;
        if n == 0 {
            return Err(HttpReadError::InvalidHead("unexpected EOF"));
        }
        let room = MAX_HTTP_HEAD.saturating_sub(buf.len());
        if n > room {
            return Err(HttpReadError::InvalidHead("header exceeds limit"));
        }
        buf.extend_from_slice(&tmp[..n]);
    };

    if head.header("transfer-encoding").is_some() {
        return Err(HttpReadError::UnsupportedTransferEncoding);
    }

    let content_len = match head.header("content-length") {
        None => 0usize,
        Some(raw) => raw
            .trim()
            .parse::<usize>()
            .map_err(|_| HttpReadError::InvalidHead("invalid Content-Length"))?,
    };
    if content_len > max_body {
        return Err(HttpReadError::PayloadTooLarge);
    }

    let mut body = Vec::with_capacity(content_len);
    let leftover = &buf[head_len..];
    if leftover.len() >= content_len {
        body.extend_from_slice(&leftover[..content_len]);
    } else {
        body.extend_from_slice(leftover);
        let remaining = content_len - body.len();
        let mut tail = vec![0u8; remaining];
        stream.read_exact(&mut tail).map_err(HttpReadError::Io)?;
        body.extend_from_slice(&tail);
    }

    Ok(OwnedHttpRequest { head, body })
}

/// Serialized HTTP/1.1 response produced by [`AdminRouter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminHttpResponse {
    pub status_code: u16,
    pub status_text: &'static str,
    pub content_type: &'static str,
    pub content_disposition: Option<String>,
    pub body: Vec<u8>,
}

impl AdminHttpResponse {
    pub fn json(status_code: u16, status_text: &'static str, body: String) -> Self {
        Self {
            status_code,
            status_text,
            content_type: "application/json; charset=utf-8",
            content_disposition: None,
            body: body.into_bytes(),
        }
    }

    pub fn json_error(status_code: u16, status_text: &'static str, message: &str) -> Self {
        Self::json(
            status_code,
            status_text,
            format!("{{\"error\":{}}}", escape_json_str(message)),
        )
    }

    pub fn html(body: Vec<u8>) -> Self {
        Self {
            status_code: 200,
            status_text: "OK",
            content_type: "text/html; charset=utf-8",
            content_disposition: None,
            body,
        }
    }

    pub fn binary_download(filename: &str, body: Vec<u8>) -> Self {
        Self {
            status_code: 200,
            status_text: "OK",
            content_type: "application/octet-stream",
            content_disposition: Some(format!("attachment; filename=\"{filename}\"")),
            body,
        }
    }

    pub fn write_to(&self, writer: &mut impl Write, head_only: bool) -> io::Result<()> {
        let mut head = format!(
            "HTTP/1.1 {} {}\r\n\
             Content-Type: {}\r\n\
             Content-Length: {}\r\n\
             X-Content-Type-Options: nosniff\r\n\
             Cache-Control: no-store\r\n\
             Connection: close\r\n",
            self.status_code,
            self.status_text,
            self.content_type,
            self.body.len(),
        );
        if let Some(disp) = &self.content_disposition {
            let _ = write!(head, "Content-Disposition: {disp}\r\n");
        }
        head.push_str("\r\n");
        writer.write_all(head.as_bytes())?;
        if !head_only && !self.body.is_empty() {
            writer.write_all(&self.body)?;
        }
        writer.flush()
    }
}

/// Pure-Rust HTTP/1.1 router for `/admin` and `/api/*`.
pub struct AdminRouter;

impl AdminRouter {
    /// Returns `true` if `path` belongs to the admin UI or JSON/replay API.
    pub fn handles_path(path: &str) -> bool {
        path == "/admin" || path == "/admin.html" || path.starts_with("/api/")
    }

    /// Dispatches one HTTP/1.1 request against `backend`.
    pub fn handle(req: &AdminHttpRequest<'_>, backend: &mut dyn AdminBackend) -> AdminHttpResponse {
        let method = req.method;
        if req.path == "/admin" || req.path == "/admin.html" {
            if method != "GET" && method != "HEAD" {
                return AdminHttpResponse::json_error(
                    405,
                    "Method Not Allowed",
                    "method not allowed",
                );
            }
            if let Some(html) = backend.admin_html() {
                return AdminHttpResponse::html(html.to_vec());
            }
            return AdminHttpResponse::html(DEFAULT_ADMIN_FALLBACK_HTML.as_bytes().to_vec());
        }

        if !req.path.starts_with("/api/") {
            return AdminHttpResponse::json_error(404, "Not Found", "unknown route");
        }

        if let Err(resp) = authenticate(req, backend.rcon_password()) {
            return resp;
        }

        match (method, req.path) {
            ("GET" | "HEAD", "/api/status") => {
                let s = backend.status();
                AdminHttpResponse::json(200, "OK", encode_status_json(&s))
            }
            ("GET" | "HEAD", "/api/players") => {
                let list = backend.players();
                AdminHttpResponse::json(200, "OK", encode_players_json(&list))
            }
            ("GET" | "HEAD", "/api/stats") => {
                AdminHttpResponse::json(200, "OK", backend.stats_json())
            }
            ("GET" | "HEAD", "/api/replays") => {
                AdminHttpResponse::json(200, "OK", backend.replays_index_json())
            }
            ("GET" | "HEAD", "/api/invite") => {
                let cfg = backend.invite_config();
                match build_invite_json(req.query, req.host, &cfg) {
                    Ok(json) => AdminHttpResponse::json(200, "OK", json),
                    Err(msg) => AdminHttpResponse::json_error(400, "Bad Request", msg),
                }
            }
            ("GET" | "HEAD", path) if path.starts_with("/api/replays/") => {
                let Some(rest) = path.strip_prefix("/api/replays/") else {
                    return AdminHttpResponse::json_error(404, "Not Found", "not found");
                };
                let Some(id) = rest.strip_suffix(".dem") else {
                    return AdminHttpResponse::json_error(404, "Not Found", "expected .dem suffix");
                };
                if !valid_replay_id(id) {
                    return AdminHttpResponse::json_error(400, "Bad Request", "invalid replay id");
                }
                match backend.replay_dem(id) {
                    Some(bytes) => {
                        let filename = format!("{id}.dem");
                        AdminHttpResponse::binary_download(&filename, bytes)
                    }
                    None => AdminHttpResponse::json_error(404, "Not Found", "replay not found"),
                }
            }
            ("POST", "/api/rcon") => handle_rcon_post(req.body, backend),
            (_, "/api/rcon") => {
                AdminHttpResponse::json_error(405, "Method Not Allowed", "expected POST")
            }
            ("GET" | "HEAD", _) => {
                AdminHttpResponse::json_error(404, "Not Found", "unknown API endpoint")
            }
            _ => AdminHttpResponse::json_error(405, "Method Not Allowed", "method not allowed"),
        }
    }
}

fn extract_auth_token<'a>(req: &'a AdminHttpRequest<'a>) -> Option<&'a str> {
    if let Some(auth) = req.authorization {
        let trimmed = auth.trim();
        if trimmed.len() > 7 && trimmed[..7].eq_ignore_ascii_case("bearer ") {
            return Some(trimmed[7..].trim());
        }
    }
    req.x_admin_token.map(str::trim).filter(|t| !t.is_empty())
}

fn authenticate(req: &AdminHttpRequest<'_>, expected_password: &str) -> Result<(), AdminHttpResponse> {
    if expected_password.is_empty() {
        return Err(AdminHttpResponse::json_error(
            401,
            "Unauthorized",
            "rcon_password is not configured on server",
        ));
    }
    let Some(token) = extract_auth_token(req) else {
        return Err(AdminHttpResponse::json_error(
            401,
            "Unauthorized",
            "missing admin bearer token",
        ));
    };
    if token.len() > MAX_AUTH_TOKEN_BYTES
        || !constant_time_eq(token.as_bytes(), expected_password.as_bytes())
    {
        return Err(AdminHttpResponse::json_error(
            403,
            "Forbidden",
            "invalid admin token",
        ));
    }
    Ok(())
}

fn handle_rcon_post(body: &[u8], backend: &mut dyn AdminBackend) -> AdminHttpResponse {
    if body.len() > MAX_API_BODY_BYTES {
        return AdminHttpResponse::json_error(413, "Payload Too Large", "request body too large");
    }
    let command = match extract_json_string_field(body, "command") {
        Ok(cmd) => cmd,
        Err(msg) => return AdminHttpResponse::json_error(400, "Bad Request", msg),
    };
    let trimmed = command.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_RCON_CMD_BYTES {
        return AdminHttpResponse::json_error(
            400,
            "Bad Request",
            "command must be 1..=1024 bytes",
        );
    }
    if trimmed
        .chars()
        .any(|c| c == '\0' || (c < ' ' && c != '\t') || c == '\u{7F}')
    {
        return AdminHttpResponse::json_error(
            400,
            "Bad Request",
            "command contains disallowed control characters",
        );
    }

    match backend.execute_rcon(trimmed) {
        Ok(output) => AdminHttpResponse::json(
            200,
            "OK",
            format!(
                "{{\"ok\":true,\"command\":{},\"output\":{}}}",
                escape_json_str(trimmed),
                escape_json_str(&output),
            ),
        ),
        Err(err) => AdminHttpResponse::json_error(400, "Bad Request", &err),
    }
}

fn encode_status_json(s: &ServerStatusSnapshot) -> String {
    let uptime = if s.uptime_secs.is_finite() && s.uptime_secs >= 0.0 {
        s.uptime_secs
    } else {
        0.0
    };
    let web_port_json = match s.web_port {
        Some(p) => p.to_string(),
        None => "null".to_string(),
    };
    let mut out = String::with_capacity(256 + s.available_maps.len() * 20);
    let _ = write!(
        out,
        "{{\"hostname\":{},\"map\":{},\"players\":{},\"bots\":{},\"max_players\":{},\"uptime_secs\":{uptime:.2},\"tick_rate\":{},\"round_number\":{},\"score_ct\":{},\"score_t\":{},\"game_port\":{},\"web_port\":{web_port_json},\"bot_quota\":{},\"bot_difficulty\":{},\"available_maps\":[",
        escape_json_str(&s.hostname),
        escape_json_str(&s.map),
        s.players,
        s.bots,
        s.max_players,
        s.tick_rate,
        s.round_number,
        s.score_ct,
        s.score_t,
        s.game_port,
        s.bot_quota,
        s.bot_difficulty,
    );
    for (idx, m) in s.available_maps.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(&escape_json_str(m));
    }
    out.push_str("]}");
    out
}

fn encode_players_json(players: &[PlayerSnapshot]) -> String {
    let mut out = String::with_capacity(64 + players.len() * 160);
    let _ = write!(out, "{{\"count\":{},\"players\":[", players.len());
    for (idx, p) in players.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"slot\":{},\"userid\":{},\"name\":{},\"team\":{},\"frags\":{},\"deaths\":{},\"ping_ms\":{},\"state\":{},\"addr\":{},\"is_bot\":{}}}",
            p.slot,
            p.userid,
            escape_json_str(&p.name),
            escape_json_str(&p.team),
            p.frags,
            p.deaths,
            p.ping_ms,
            escape_json_str(&p.state),
            escape_json_str(&p.addr),
            p.is_bot,
        );
    }
    out.push_str("]}");
    out
}

/// Generates a lobby invite JSON payload containing `?join=<host:port>&room=<code>`.
pub fn build_invite_json(
    query: Option<&str>,
    host_header: Option<&str>,
    cfg: &InviteConfig,
) -> Result<String, &'static str> {
    let params = parse_query_params(query.unwrap_or(""));
    let host_from_header = host_header
        .and_then(|h| h.split(':').next())
        .map(str::trim)
        .filter(|h| !h.is_empty());

    let raw_host = param_value(&params, "host")
        .or(host_from_header)
        .unwrap_or(cfg.default_host.as_str());
    let host = validate_invite_host(raw_host)?;

    let port = match param_value(&params, "port") {
        Some(raw) => raw
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or("invalid invite port")?,
        None => cfg.web_port,
    };

    let room = match param_value(&params, "room") {
        Some(raw) => validate_room_code(raw)?,
        None => cfg.default_room.as_str(),
    };
    let room = validate_room_code(room)?;

    let base_url = match param_value(&params, "base") {
        Some(raw) => validate_base_url(raw)?,
        None => format!("http://{host}:{port}/"),
    };
    let join_target = format!("{host}:{port}");
    let query_suffix = format!(
        "?join={}&room={}",
        url_encode_component(&join_target),
        url_encode_component(room)
    );
    let invite_url = format!("{base_url}{query_suffix}");
    let ws_url = format!("ws://{host}:{port}/ws");
    let connect_cmd = format!("connect {host}:{}", cfg.game_port);

    Ok(format!(
        "{{\"invite_url\":{},\"query\":{},\"host\":{},\"port\":{},\"room\":{},\"ws_url\":{},\"connect_cmd\":{}}}",
        escape_json_str(&invite_url),
        escape_json_str(&query_suffix),
        escape_json_str(host),
        port,
        escape_json_str(room),
        escape_json_str(&ws_url),
        escape_json_str(&connect_cmd),
    ))
}

fn validate_invite_host(host: &str) -> Result<&str, &'static str> {
    let trimmed = host.trim();
    if trimmed.is_empty()
        || trimmed.len() > 253
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err("invalid invite host");
    }
    Ok(trimmed)
}

fn validate_room_code(room: &str) -> Result<&str, &'static str> {
    let trimmed = room.trim();
    if trimmed.is_empty()
        || trimmed.len() > 32
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("invalid room code");
    }
    Ok(trimmed)
}

fn validate_base_url(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim();
    if trimmed.len() > 256
        || !(trimmed.starts_with("http://") || trimmed.starts_with("https://"))
        || trimmed.contains(['?', '#', '\r', '\n', '"', '\'', '<', '>'])
    {
        return Err("invalid base URL");
    }
    Ok(trimmed.to_string())
}

fn parse_query_params(query: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if let (Some(dk), Some(dv)) = (url_decode(k), url_decode(v)) {
            out.push((dk, dv));
        }
    }
    out
}

fn param_value<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn url_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hi = *bytes.get(i + 1)?;
                let lo = *bytes.get(i + 2)?;
                let val = (hex_val(hi)? << 4) | hex_val(lo)?;
                out.push(val);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn url_encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Minimal strict JSON parser extracting a single top-level string field (e.g. `"command"`).
pub fn extract_json_string_field(body: &[u8], target_key: &str) -> Result<String, &'static str> {
    let text = std::str::from_utf8(body).map_err(|_| "request body must be UTF-8 JSON")?;
    let mut rest = text.trim();
    rest = rest
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or("expected JSON object")?
        .trim();
    if rest.is_empty() {
        return Err("missing required JSON field");
    }

    while !rest.is_empty() {
        let (key, after_key) = parse_json_string(rest)?;
        let after_colon = after_key
            .trim_start()
            .strip_prefix(':')
            .ok_or("expected ':' after JSON key")?
            .trim_start();
        if after_colon.starts_with('"') {
            let (val, after_val) = parse_json_string(after_colon)?;
            if key == target_key {
                return Ok(val);
            }
            rest = after_val.trim_start();
        } else {
            // Skip primitive value (number, bool, null) safely up to comma/end.
            let end = after_colon
                .find(',')
                .unwrap_or(after_colon.len());
            let token = after_colon[..end].trim();
            if token.is_empty() || token.starts_with('{') || token.starts_with('[') {
                return Err("unsupported JSON value");
            }
            rest = after_colon[end..].trim_start();
        }
        if let Some(next) = rest.strip_prefix(',') {
            rest = next.trim_start();
            if rest.is_empty() {
                return Err("trailing comma in JSON");
            }
        } else if !rest.is_empty() {
            return Err("malformed JSON object");
        }
    }

    Err("missing required JSON field")
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
                    'u' => {
                        let mut hex = String::with_capacity(4);
                        for _ in 0..4 {
                            let (_, h) = chars.next().ok_or("incomplete \\uXXXX escape")?;
                            hex.push(h);
                        }
                        let code = u32::from_str_radix(&hex, 16)
                            .map_err(|_| "invalid \\uXXXX hex escape")?;
                        let decoded = char::from_u32(code).ok_or("invalid Unicode scalar")?;
                        out.push(decoded);
                    }
                    _ => return Err("unsupported JSON escape"),
                }
            }
            c if (c as u32) < 0x20 => return Err("unescaped control character in JSON string"),
            c => out.push(c),
        }
    }
    Err("unterminated JSON string")
}

const DEFAULT_ADMIN_FALLBACK_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>CS 1.6 Admin</title></head><body><p>Admin UI asset <code>web/admin.html</code> not loaded; use <code>--web-dir web</code>.</p></body></html>";

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct StubBackend {
        rcon_password: String,
        last_cmd: Option<String>,
        replay: Vec<u8>,
    }

    impl AdminBackend for StubBackend {
        fn rcon_password(&self) -> &str {
            &self.rcon_password
        }

        fn status(&self) -> ServerStatusSnapshot {
            ServerStatusSnapshot {
                hostname: "CS 1.6 \"Test\"\nServer".to_string(),
                map: "de_dust2".to_string(),
                players: 2,
                bots: 1,
                max_players: 16,
                uptime_secs: 123.456,
                tick_rate: 100,
                round_number: 4,
                score_ct: 3,
                score_t: 1,
                game_port: 27015,
                web_port: Some(27016),
                bot_quota: 4,
                bot_difficulty: 1,
                available_maps: vec!["de_dust2".into(), "de_inferno".into()],
            }
        }

        fn players(&self) -> Vec<PlayerSnapshot> {
            vec![PlayerSnapshot {
                slot: 1,
                userid: 7,
                name: "Alice<script>".to_string(),
                team: "CT".to_string(),
                frags: 5,
                deaths: 2,
                ping_ms: 18,
                state: "active".to_string(),
                addr: "127.0.0.1:27005".to_string(),
                is_bot: false,
            }]
        }

        fn stats_json(&self) -> String {
            "{\"players\":[]}".to_string()
        }

        fn replays_index_json(&self) -> String {
            "{\"count\":1,\"rounds\":[{\"id\":\"r0001-de_dust2\"}]}".to_string()
        }

        fn replay_dem(&self, id: &str) -> Option<Vec<u8>> {
            (id == "r0001-de_dust2").then(|| self.replay.clone())
        }

        fn execute_rcon(&mut self, command: &str) -> Result<String, String> {
            self.last_cmd = Some(command.to_string());
            Ok(format!("ran: {command}"))
        }
    }

    #[test]
    fn constant_time_compare_and_auth_guards() {
        assert!(constant_time_eq(b"topsecret", b"topsecret"));
        assert!(!constant_time_eq(b"topsecret", b"topsecrex"));
        assert!(!constant_time_eq(b"topsecret", b"top"));
        assert!(!constant_time_eq(b"", b"topsecret"));

        let mut backend = StubBackend {
            rcon_password: "secret-pw".to_string(),
            last_cmd: None,
            replay: b"HLDEMO\0\0demo".to_vec(),
        };

        let unauth = AdminHttpRequest {
            method: "GET",
            path: "/api/status",
            query: None,
            authorization: None,
            x_admin_token: None,
            host: None,
            body: b"",
        };
        assert_eq!(AdminRouter::handle(&unauth, &mut backend).status_code, 401);

        let bad_auth = AdminHttpRequest {
            authorization: Some("Bearer wrong"),
            ..unauth.clone()
        };
        assert_eq!(AdminRouter::handle(&bad_auth, &mut backend).status_code, 403);

        // Empty configured rcon_password must fail closed.
        backend.rcon_password.clear();
        let empty_pw = AdminHttpRequest {
            authorization: Some("Bearer "),
            ..unauth
        };
        assert_eq!(AdminRouter::handle(&empty_pw, &mut backend).status_code, 401);
    }

    #[test]
    fn routes_status_players_replays_rcon_and_invite() {
        let mut backend = StubBackend {
            rcon_password: "hunter2".to_string(),
            last_cmd: None,
            replay: b"HLDEMO\0\0payload".to_vec(),
        };

        // /admin works without token.
        let admin_req = AdminHttpRequest {
            method: "GET",
            path: "/admin",
            query: None,
            authorization: None,
            x_admin_token: None,
            host: None,
            body: b"",
        };
        assert_eq!(AdminRouter::handle(&admin_req, &mut backend).status_code, 200);

        // Status JSON escapes quotes and newlines.
        let status_req = AdminHttpRequest {
            path: "/api/status",
            authorization: Some("Bearer hunter2"),
            ..admin_req.clone()
        };
        let status_resp = AdminRouter::handle(&status_req, &mut backend);
        assert_eq!(status_resp.status_code, 200);
        let status_body = String::from_utf8(status_resp.body).unwrap();
        assert!(status_body.contains("CS 1.6 \\\"Test\\\"\\nServer"));

        // Players JSON escapes angle brackets.
        let players_resp = AdminRouter::handle(
            &AdminHttpRequest {
                path: "/api/players",
                ..status_req.clone()
            },
            &mut backend,
        );
        let players_body = String::from_utf8(players_resp.body).unwrap();
        assert!(players_body.contains("Alice\\u003cscript\\u003e"));

        // Replay download enforces valid ID and returns .dem bytes.
        let dem_resp = AdminRouter::handle(
            &AdminHttpRequest {
                path: "/api/replays/r0001-de_dust2.dem",
                ..status_req.clone()
            },
            &mut backend,
        );
        assert_eq!(dem_resp.status_code, 200);
        assert_eq!(dem_resp.body, b"HLDEMO\0\0payload");
        assert_eq!(
            dem_resp.content_disposition.as_deref(),
            Some("attachment; filename=\"r0001-de_dust2.dem\"")
        );

        let bad_dem = AdminRouter::handle(
            &AdminHttpRequest {
                path: "/api/replays/..%2Fsecret.dem",
                ..status_req.clone()
            },
            &mut backend,
        );
        assert_eq!(bad_dem.status_code, 400);

        // POST /api/rcon runs command and rejects oversized bodies.
        let rcon_resp = AdminRouter::handle(
            &AdminHttpRequest {
                method: "POST",
                path: "/api/rcon",
                body: br#"{"command":"changelevel \"de_nuke\""}"#,
                ..status_req.clone()
            },
            &mut backend,
        );
        assert_eq!(rcon_resp.status_code, 200);
        assert_eq!(
            backend.last_cmd.as_deref(),
            Some("changelevel \"de_nuke\"")
        );

        let too_big = vec![b'a'; MAX_API_BODY_BYTES + 1];
        let big_resp = AdminRouter::handle(
            &AdminHttpRequest {
                method: "POST",
                path: "/api/rcon",
                body: &too_big,
                ..status_req.clone()
            },
            &mut backend,
        );
        assert_eq!(big_resp.status_code, 413);

        // GET /api/invite builds ?join=<host:port>&room=<code>.
        let invite_resp = AdminRouter::handle(
            &AdminHttpRequest {
                path: "/api/invite",
                query: Some("host=play.example.com&port=27016&room=dust-01"),
                ..status_req
            },
            &mut backend,
        );
        assert_eq!(invite_resp.status_code, 200);
        let invite_body = String::from_utf8(invite_resp.body).unwrap();
        assert!(invite_body.contains("?join=play.example.com%3A27016&room=dust-01"));
    }

    #[test]
    fn reads_http_post_with_content_length_and_caps_oversized() {
        let raw = b"POST /api/rcon HTTP/1.1\r\nHost: localhost\r\nContent-Length: 20\r\n\r\n{\"command\":\"status\"}";
        let mut cursor = Cursor::new(&raw[..]);
        let req = read_http_request_with_body(&mut cursor, MAX_API_BODY_BYTES).unwrap();
        assert_eq!(req.head.method, "POST");
        assert_eq!(req.head.path, "/api/rcon");
        assert_eq!(req.body, br#"{"command":"status"}"#);

        let big = b"POST /api/rcon HTTP/1.1\r\nContent-Length: 99999\r\n\r\n{}";
        let mut cursor2 = Cursor::new(&big[..]);
        assert!(matches!(
            read_http_request_with_body(&mut cursor2, MAX_API_BODY_BYTES),
            Err(HttpReadError::PayloadTooLarge)
        ));
    }
}
