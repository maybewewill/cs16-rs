//! Pure-Rust HTTP/1.1 router for the `cs-server` Web Admin Panel and JSON/replay API.
//!
//! Endpoints:
//! - `GET  /admin` — Web Admin HTML shell
//! - `GET  /api/status` — live server status snapshot (token-protected)
//! - `GET  /api/players` — connected players & bots (token-protected)
//! - `GET  /api/stats` — match leaderboard & round history (token-protected)
//! - `GET  /api/replays` — recorded HLDEMO v5 round index (token-protected)
//! - `GET  /api/replays/<id>.dem` — download `.dem` bytes (token-protected)
//! - `GET  /api/replays/<id>/timeline` — 10 Hz 2D round replay JSON (token-protected)
//! - `GET  /api/map-overview/<map>` — overview spec / BSP wireframe bounds JSON (token-protected)
//! - `GET  /api/map-overview/<map>.png` — overview BMP converted to RGBA PNG (token-protected)
//! - `POST /api/rcon` — execute server console/RCON command (token-protected)
//! - `GET  /api/invite` — generate `?join=<host:port>&room=<code>` lobby link (token-protected)
//! - `GET  /api/mods` — loaded & available polyglot plugins with live ResMon metrics (token-protected)
//! - `POST /api/mods/<id>/toggle` — pause / unpause / reload plugin (token-protected)
//! - `GET  /api/mods/faults` — list retained black-box fault dumps (token-protected)
//! - `GET  /api/mods/faults/<id>.json` — download black-box fault dump JSON (token-protected)
//! - `GET  /api/mods/source/<file>` — read sandboxed `.py` / `configs/*.toml` source (token-protected)
//! - `PUT  /api/mods/source/<file>` — save & hot-reload sandboxed source/config (token-protected)
//!
//! Zero `unsafe` code; constant-time token comparison against `rcon_password`; strict JSON
//! escaping and bounded request sizes.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use cs_master::lobbies::{LobbyError, LobbyRegistry};
use cs_transport::ws_proto::{HttpRequest, MAX_HTTP_HEAD, parse_http_request};

use crate::replays::{escape_json_str, valid_map_overview_name, valid_replay_id};

/// Maximum HTTP request body accepted by standard `/api/*` endpoints (8 KiB).
pub const MAX_API_BODY_BYTES: usize = 8192;
/// Maximum HTTP request body accepted by `PUT /api/mods/source/<file>` (128 KiB).
pub const MAX_SOURCE_BODY_BYTES: usize = 128 * 1024;
/// Maximum RCON command length accepted via `POST /api/rcon`.
pub const MAX_RCON_CMD_BYTES: usize = 1024;
/// Maximum accepted admin bearer/header token length.
pub const MAX_AUTH_TOKEN_BYTES: usize = 256;

/// Returns the bounded request body limit applicable to `path`.
pub fn max_body_bytes_for_path(path: &str) -> usize {
    if path.starts_with("/api/mods/source/") {
        MAX_SOURCE_BODY_BYTES
    } else {
        MAX_API_BODY_BYTES
    }
}

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
    fn replay_timeline_json(&self, _id: &str) -> Option<String> {
        None
    }
    fn map_overview_json(&self, _map: &str) -> Option<String> {
        None
    }
    fn map_overview_png(&self, _map: &str) -> Option<Vec<u8>> {
        None
    }
    fn execute_rcon(&mut self, command: &str) -> Result<String, String>;
    fn admin_html(&self) -> Option<&[u8]> {
        None
    }
    fn invite_config(&self) -> InviteConfig {
        InviteConfig::default()
    }
    fn mods_json(&self) -> String {
        "{\"count\":0,\"mods\":[],\"editable_files\":[]}".to_string()
    }
    fn toggle_mod(&mut self, _id: &str, _action: &str) -> Result<String, String> {
        Err("mod host not enabled on server".to_string())
    }
    fn mod_faults_json(&self) -> String {
        "{\"count\":0,\"faults\":[]}".to_string()
    }
    fn mod_fault_detail_json(&self, _id: &str) -> Option<String> {
        None
    }
    fn read_mod_source(&self, _rel_path: &str) -> Result<String, String> {
        Err("plugin studio not enabled on server".to_string())
    }
    fn write_mod_source(&mut self, _rel_path: &str, _content: &str) -> Result<String, String> {
        Err("plugin studio not enabled on server".to_string())
    }
}

/// Strict path-traversal sandbox guarding `GET/PUT /api/mods/source/<file>` and disk fault reads.
#[derive(Debug, Clone)]
pub struct ModStudioSandbox {
    root: PathBuf,
}

impl ModStudioSandbox {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Validates a POSIX relative path inside `mods/` or `configs/`.
    pub fn validate_editable_rel_path(rel_path: &str) -> Result<Vec<String>, &'static str> {
        if rel_path.is_empty() || rel_path.len() > 256 {
            return Err("invalid source path length");
        }
        if rel_path.contains('\0')
            || rel_path.contains('\\')
            || rel_path.contains(':')
            || rel_path.starts_with('/')
            || rel_path.chars().any(|c| c.is_control())
        {
            return Err("disallowed character in source path");
        }
        let segments: Vec<String> = rel_path.split('/').map(ToString::to_string).collect();
        if segments.len() < 2 {
            return Err("source path must be inside mods/ or configs/");
        }
        for seg in &segments {
            if seg.is_empty() || seg == "." || seg == ".." {
                return Err("path traversal is forbidden");
            }
            if !seg
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
            {
                return Err("disallowed path segment character");
            }
        }
        let top = segments[0].as_str();
        let file = segments.last().map(String::as_str).unwrap_or("");
        let lower_file = file.to_ascii_lowercase();
        match top {
            "mods" => {
                if !lower_file.ends_with(".py") {
                    return Err("only .py scripts under mods/ are editable in Plugin Studio");
                }
            }
            "configs" => {
                if !(lower_file.ends_with(".toml")
                    || lower_file.ends_with(".cfg")
                    || lower_file.ends_with(".txt"))
                {
                    return Err("only .toml/.cfg/.txt files under configs/ are editable");
                }
            }
            _ => return Err("source path must be inside mods/ or configs/"),
        }
        Ok(segments)
    }

    fn resolve_editable_path(&self, rel_path: &str, must_exist: bool) -> Result<PathBuf, String> {
        let segments = Self::validate_editable_rel_path(rel_path).map_err(str::to_string)?;
        let allowed_dir = self.root.join(&segments[0]);
        let canon_allowed = std::fs::canonicalize(&allowed_dir)
            .map_err(|_| format!("sandbox directory `{}` not found", segments[0]))?;

        let mut candidate = allowed_dir;
        for seg in &segments[1..] {
            candidate.push(seg);
        }
        // Reject any symlink along the path to prevent symlink escape.
        let mut cursor = self.root.join(&segments[0]);
        for seg in &segments[1..] {
            cursor.push(seg);
            if let Ok(meta) = std::fs::symlink_metadata(&cursor)
                && meta.file_type().is_symlink()
            {
                return Err("symlinks are forbidden inside Plugin Studio sandbox".to_string());
            }
        }

        if must_exist {
            let canon = std::fs::canonicalize(&candidate)
                .map_err(|_| format!("source file `{rel_path}` not found"))?;
            if !canon.starts_with(&canon_allowed) || !canon.is_file() {
                return Err("source path escapes Plugin Studio sandbox".to_string());
            }
            Ok(canon)
        } else {
            let parent = candidate
                .parent()
                .ok_or_else(|| "invalid parent directory".to_string())?;
            let canon_parent = std::fs::canonicalize(parent)
                .map_err(|_| format!("parent directory for `{rel_path}` not found"))?;
            if !canon_parent.starts_with(&canon_allowed) {
                return Err("source path escapes Plugin Studio sandbox".to_string());
            }
            let file_name = candidate
                .file_name()
                .ok_or_else(|| "invalid filename".to_string())?;
            Ok(canon_parent.join(file_name))
        }
    }

    /// Lists all editable `.py` and `configs/*.{toml,cfg,txt}` files relative to sandbox root.
    pub fn list_editable_files(&self) -> Vec<String> {
        let mut out = Vec::new();
        for top in ["mods", "configs"] {
            let dir = self.root.join(top);
            if dir.is_dir() {
                collect_editable_files(&dir, top, &mut out, 0);
            }
        }
        out.sort();
        out
    }

    /// Reads one sandboxed source/config file (bounded to `MAX_SOURCE_BODY_BYTES`).
    pub fn read_source(&self, rel_path: &str) -> Result<String, String> {
        let abs = self.resolve_editable_path(rel_path, true)?;
        let meta = std::fs::metadata(&abs).map_err(|e| format!("failed to stat file: {e}"))?;
        if meta.len() as usize > MAX_SOURCE_BODY_BYTES {
            return Err("source file exceeds 128 KiB Plugin Studio limit".to_string());
        }
        std::fs::read_to_string(&abs).map_err(|e| format!("failed to read `{rel_path}`: {e}"))
    }

    /// Validates and writes one sandboxed source/config file, returning its canonical disk path.
    pub fn write_source(&self, rel_path: &str, content: &str) -> Result<PathBuf, String> {
        if content.len() > MAX_SOURCE_BODY_BYTES {
            return Err("source content exceeds 128 KiB Plugin Studio limit".to_string());
        }
        if rel_path.to_ascii_lowercase().ends_with(".py") {
            cs_mod_py::PySourceSyntaxCheck::check(rel_path, content)
                .map_err(|e| format!("Python syntax error: {e}"))?;
        }
        let abs = self.resolve_editable_path(rel_path, false)?;
        std::fs::write(&abs, content.as_bytes())
            .map_err(|e| format!("failed to write `{rel_path}`: {e}"))?;
        Ok(abs)
    }

    /// Reads a sandboxed black-box fault dump from `logs/mod_faults/<id>.json`.
    pub fn read_fault_file(&self, id: &str) -> Option<String> {
        if !valid_fault_dump_id(id) {
            return None;
        }
        let dir = self.root.join("logs").join("mod_faults");
        let canon_dir = std::fs::canonicalize(&dir).ok()?;
        let filename = if id.ends_with(".json") {
            id.to_string()
        } else {
            format!("{id}.json")
        };
        let target = canon_dir.join(&filename);
        let canon = std::fs::canonicalize(&target).ok()?;
        if !canon.starts_with(&canon_dir) || !canon.is_file() {
            return None;
        }
        let meta = std::fs::metadata(&canon).ok()?;
        if meta.len() > 300 * 1024 {
            return None;
        }
        std::fs::read_to_string(&canon).ok()
    }
}

fn collect_editable_files(dir: &Path, prefix: &str, out: &mut Vec<String>, depth: usize) {
    if depth > 4 || out.len() >= 256 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if let Ok(meta) = std::fs::symlink_metadata(&p)
            && meta.file_type().is_symlink()
        {
            continue;
        }
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') || name == "target" {
            continue;
        }
        let rel = format!("{prefix}/{name}");
        if p.is_dir() {
            collect_editable_files(&p, &rel, out, depth + 1);
        } else if p.is_file() && ModStudioSandbox::validate_editable_rel_path(&rel).is_ok() {
            out.push(rel);
        }
    }
}

pub fn valid_mod_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.contains("..")
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

pub fn valid_fault_dump_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.contains("..")
        && !id.contains('/')
        && !id.contains('\\')
        && Path::new(id)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
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
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct OwnedHttpRequest {
    pub head: HttpRequest,
    pub body: Vec<u8>,
}

/// Errors returned by [`read_http_request_with_body`].
#[allow(dead_code)]
#[derive(Debug)]
pub enum HttpReadError {
    Io(io::Error),
    InvalidHead(&'static str),
    PayloadTooLarge,
    UnsupportedTransferEncoding,
}

/// Reads an HTTP/1.1 request head (max `MAX_HTTP_HEAD`) and optional `Content-Length` body
/// (max `max_body` bytes) from `stream`.
#[allow(dead_code)]
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

    let body = read_http_body(&head, &buf[head_len..], stream, max_body)?;
    Ok(OwnedHttpRequest { head, body })
}

/// Reads the `Content-Length` body (max `max_body` bytes) given an already-parsed head and any
/// trailing bytes read alongside the head.
pub fn read_http_body(
    head: &HttpRequest,
    leftover: &[u8],
    stream: &mut impl Read,
    max_body: usize,
) -> Result<Vec<u8>, HttpReadError> {
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
    if leftover.len() >= content_len {
        body.extend_from_slice(&leftover[..content_len]);
    } else {
        body.extend_from_slice(leftover);
        let remaining = content_len - body.len();
        let mut tail = vec![0u8; remaining];
        stream.read_exact(&mut tail).map_err(HttpReadError::Io)?;
        body.extend_from_slice(&tail);
    }

    Ok(body)
}

/// Serialized HTTP/1.1 response produced by [`AdminRouter`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminHttpResponse {
    pub status_code: u16,
    pub status_text: &'static str,
    pub content_type: &'static str,
    pub content_disposition: Option<String>,
    pub cors_allow_origin: bool,
    pub allow_header: Option<&'static str>,
    pub body: Vec<u8>,
}

impl AdminHttpResponse {
    pub fn json(status_code: u16, status_text: &'static str, body: String) -> Self {
        Self {
            status_code,
            status_text,
            content_type: "application/json; charset=utf-8",
            content_disposition: None,
            cors_allow_origin: false,
            allow_header: None,
            body: body.into_bytes(),
        }
    }

    pub fn cors_json(status_code: u16, status_text: &'static str, body: String) -> Self {
        let mut resp = Self::json(status_code, status_text, body);
        resp.cors_allow_origin = true;
        resp
    }

    pub fn cors_no_content() -> Self {
        Self {
            status_code: 204,
            status_text: "No Content",
            content_type: "text/plain; charset=utf-8",
            content_disposition: None,
            cors_allow_origin: true,
            allow_header: None,
            body: Vec::new(),
        }
    }

    pub fn json_error(status_code: u16, status_text: &'static str, message: &str) -> Self {
        Self::json(
            status_code,
            status_text,
            format!("{{\"error\":{}}}", escape_json_str(message)),
        )
    }

    pub fn cors_json_error(status_code: u16, status_text: &'static str, message: &str) -> Self {
        Self::cors_json(
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
            cors_allow_origin: false,
            allow_header: None,
            body,
        }
    }

    pub fn binary_download(filename: &str, body: Vec<u8>) -> Self {
        Self {
            status_code: 200,
            status_text: "OK",
            content_type: "application/octet-stream",
            content_disposition: Some(format!("attachment; filename=\"{filename}\"")),
            cors_allow_origin: false,
            allow_header: None,
            body,
        }
    }

    pub fn png(body: Vec<u8>) -> Self {
        Self {
            status_code: 200,
            status_text: "OK",
            content_type: "image/png",
            content_disposition: None,
            cors_allow_origin: false,
            allow_header: None,
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
        if self.cors_allow_origin {
            head.push_str(
                "Access-Control-Allow-Origin: *\r\n\
                 Access-Control-Allow-Methods: GET, POST, DELETE, OPTIONS\r\n\
                 Access-Control-Allow-Headers: Content-Type\r\n",
            );
        }
        if let Some(allow) = self.allow_header {
            let _ = write!(head, "Allow: {allow}\r\n");
        }
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

/// Returns `true` if `path` belongs to the public WebRTC/lobby signaling API.
pub fn is_lobby_path(path: &str) -> bool {
    path == "/lobbies" || path.starts_with("/lobbies/")
}

/// Handles public CORS lobby signaling routes (`/lobbies`, `/lobbies/<room>`, `/lobbies/<room>/signal`)
/// backed by `LobbyRegistry` when `--web-port` is enabled on `cs-server`.
pub fn handle_lobby_request(
    req: &AdminHttpRequest<'_>,
    lobbies: &mut LobbyRegistry,
    now_secs: f64,
) -> AdminHttpResponse {
    if req.method == "OPTIONS" {
        return AdminHttpResponse::cors_no_content();
    }
    if req.body.len() > MAX_API_BODY_BYTES {
        return AdminHttpResponse::cors_json_error(
            413,
            "Payload Too Large",
            "request body too large",
        );
    }

    if req.path == "/lobbies" {
        return match req.method {
            "GET" | "HEAD" => {
                AdminHttpResponse::cors_json(200, "OK", lobbies.list_json(now_secs))
            }
            "POST" => match lobbies.upsert_from_json(req.body, now_secs) {
                Ok(info) => AdminHttpResponse::cors_json(200, "OK", info.to_json()),
                Err(err) => {
                    AdminHttpResponse::cors_json_error(400, "Bad Request", &err.to_string())
                }
            },
            _ => {
                let mut resp = AdminHttpResponse::cors_json_error(
                    405,
                    "Method Not Allowed",
                    "method not allowed",
                );
                resp.allow_header = Some("GET, POST, OPTIONS");
                resp
            }
        };
    }

    let rest = req.path.strip_prefix("/lobbies/").unwrap_or("");
    if let Some(room) = rest.strip_suffix("/signal") {
        return match req.method {
            "POST" => match lobbies.push_signal_from_json(room, req.body, now_secs) {
                Ok(sig) => AdminHttpResponse::cors_json(200, "OK", sig.to_json()),
                Err(LobbyError::NotFound) => {
                    AdminHttpResponse::cors_json_error(404, "Not Found", "lobby room not found")
                }
                Err(err) => {
                    AdminHttpResponse::cors_json_error(400, "Bad Request", &err.to_string())
                }
            },
            "GET" | "HEAD" => {
                let params = parse_query_params(req.query.unwrap_or(""));
                let since_id = param_value(&params, "since")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                let target = param_value(&params, "to").filter(|v| !v.is_empty());
                match lobbies.poll_signals(room, since_id, target, now_secs) {
                    Ok(sigs) => AdminHttpResponse::cors_json(
                        200,
                        "OK",
                        LobbyRegistry::signals_to_json(&sigs),
                    ),
                    Err(LobbyError::NotFound) => {
                        AdminHttpResponse::cors_json_error(404, "Not Found", "lobby room not found")
                    }
                    Err(err) => {
                        AdminHttpResponse::cors_json_error(400, "Bad Request", &err.to_string())
                    }
                }
            }
            _ => {
                let mut resp = AdminHttpResponse::cors_json_error(
                    405,
                    "Method Not Allowed",
                    "method not allowed",
                );
                resp.allow_header = Some("GET, POST, OPTIONS");
                resp
            }
        };
    }

    match req.method {
        "GET" | "HEAD" => match lobbies.get(rest, now_secs) {
            Some(info) => AdminHttpResponse::cors_json(200, "OK", info.to_json()),
            None => AdminHttpResponse::cors_json_error(404, "Not Found", "lobby room not found"),
        },
        "DELETE" => {
            if lobbies.remove(rest) {
                AdminHttpResponse::cors_json(200, "OK", "{\"deleted\":true}".to_string())
            } else {
                AdminHttpResponse::cors_json_error(404, "Not Found", "lobby room not found")
            }
        }
        _ => {
            let mut resp =
                AdminHttpResponse::cors_json_error(405, "Method Not Allowed", "method not allowed");
            resp.allow_header = Some("GET, DELETE, OPTIONS");
            resp
        }
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
                if let Some(id) = rest.strip_suffix("/timeline") {
                    if !valid_replay_id(id) {
                        return AdminHttpResponse::json_error(
                            400,
                            "Bad Request",
                            "invalid replay id",
                        );
                    }
                    return match backend.replay_timeline_json(id) {
                        Some(json) => AdminHttpResponse::json(200, "OK", json),
                        None => AdminHttpResponse::json_error(
                            404,
                            "Not Found",
                            "replay timeline not found",
                        ),
                    };
                }
                let Some(id) = rest.strip_suffix(".dem") else {
                    return AdminHttpResponse::json_error(404, "Not Found", "expected .dem or /timeline");
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
            ("GET" | "HEAD", path) if path.starts_with("/api/map-overview/") => {
                let Some(rest) = path.strip_prefix("/api/map-overview/") else {
                    return AdminHttpResponse::json_error(404, "Not Found", "not found");
                };
                if let Some(map) = rest.strip_suffix(".png") {
                    if !valid_map_overview_name(map) {
                        return AdminHttpResponse::json_error(
                            400,
                            "Bad Request",
                            "invalid map name",
                        );
                    }
                    return match backend.map_overview_png(map) {
                        Some(bytes) => AdminHttpResponse::png(bytes),
                        None => AdminHttpResponse::json_error(
                            404,
                            "Not Found",
                            "overview image not found",
                        ),
                    };
                }
                if !valid_map_overview_name(rest) {
                    return AdminHttpResponse::json_error(400, "Bad Request", "invalid map name");
                }
                match backend.map_overview_json(rest) {
                    Some(json) => AdminHttpResponse::json(200, "OK", json),
                    None => AdminHttpResponse::json_error(
                        404,
                        "Not Found",
                        "map overview not found",
                    ),
                }
            }
            ("POST", "/api/rcon") => handle_rcon_post(req.body, backend),
            (_, "/api/rcon") => {
                AdminHttpResponse::json_error(405, "Method Not Allowed", "expected POST")
            }
            ("GET" | "HEAD", "/api/mods") => {
                AdminHttpResponse::json(200, "OK", backend.mods_json())
            }
            ("GET" | "HEAD", "/api/mods/faults") => {
                AdminHttpResponse::json(200, "OK", backend.mod_faults_json())
            }
            ("GET" | "HEAD", path) if path.starts_with("/api/mods/faults/") => {
                let Some(rest) = path.strip_prefix("/api/mods/faults/") else {
                    return AdminHttpResponse::json_error(404, "Not Found", "not found");
                };
                let (id, as_download) = match rest.strip_suffix(".json") {
                    Some(stem) => (stem, true),
                    None => (rest, false),
                };
                if !valid_fault_dump_id(id) {
                    return AdminHttpResponse::json_error(
                        400,
                        "Bad Request",
                        "invalid fault dump id",
                    );
                }
                match backend.mod_fault_detail_json(id) {
                    Some(json) => {
                        let mut resp = AdminHttpResponse::json(200, "OK", json);
                        if as_download {
                            resp.content_disposition =
                                Some(format!("attachment; filename=\"{id}.json\""));
                        }
                        resp
                    }
                    None => {
                        AdminHttpResponse::json_error(404, "Not Found", "fault dump not found")
                    }
                }
            }
            ("GET" | "HEAD", path) if path.starts_with("/api/mods/source/") => {
                let Some(raw_rel) = path.strip_prefix("/api/mods/source/") else {
                    return AdminHttpResponse::json_error(404, "Not Found", "not found");
                };
                let Some(rel) = url_decode(raw_rel) else {
                    return AdminHttpResponse::json_error(
                        400,
                        "Bad Request",
                        "invalid percent-encoding in source path",
                    );
                };
                if let Err(msg) = ModStudioSandbox::validate_editable_rel_path(&rel) {
                    return AdminHttpResponse::json_error(400, "Bad Request", msg);
                }
                match backend.read_mod_source(&rel) {
                    Ok(content) => AdminHttpResponse::json(
                        200,
                        "OK",
                        format!(
                            "{{\"path\":{},\"content\":{}}}",
                            escape_json_str(&rel),
                            escape_json_str(&content),
                        ),
                    ),
                    Err(err) => {
                        let code = if err.contains("not found") { 404 } else { 400 };
                        let text = if code == 404 {
                            "Not Found"
                        } else {
                            "Bad Request"
                        };
                        AdminHttpResponse::json_error(code, text, &err)
                    }
                }
            }
            ("PUT", path) if path.starts_with("/api/mods/source/") => {
                handle_mod_source_put(path, req.body, backend)
            }
            ("POST", path)
                if path.starts_with("/api/mods/") && path.ends_with("/toggle") =>
            {
                handle_mod_toggle_post(path, req.body, backend)
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

fn handle_mod_toggle_post(
    path: &str,
    body: &[u8],
    backend: &mut dyn AdminBackend,
) -> AdminHttpResponse {
    if body.len() > MAX_API_BODY_BYTES {
        return AdminHttpResponse::json_error(413, "Payload Too Large", "request body too large");
    }
    let Some(id_raw) = path
        .strip_prefix("/api/mods/")
        .and_then(|s| s.strip_suffix("/toggle"))
    else {
        return AdminHttpResponse::json_error(404, "Not Found", "unknown route");
    };
    let Some(id) = url_decode(id_raw) else {
        return AdminHttpResponse::json_error(400, "Bad Request", "invalid mod id encoding");
    };
    if !valid_mod_id(&id) {
        return AdminHttpResponse::json_error(400, "Bad Request", "invalid mod id");
    }
    let action = match extract_json_string_field(body, "action") {
        Ok(a) => a.trim().to_ascii_lowercase(),
        Err(msg) => return AdminHttpResponse::json_error(400, "Bad Request", msg),
    };
    if !matches!(action.as_str(), "pause" | "unpause" | "reload") {
        return AdminHttpResponse::json_error(
            400,
            "Bad Request",
            "action must be pause, unpause, or reload",
        );
    }
    match backend.toggle_mod(&id, &action) {
        Ok(message) => AdminHttpResponse::json(
            200,
            "OK",
            format!(
                "{{\"ok\":true,\"id\":{},\"action\":{},\"message\":{}}}",
                escape_json_str(&id),
                escape_json_str(&action),
                escape_json_str(&message),
            ),
        ),
        Err(err) => AdminHttpResponse::json_error(400, "Bad Request", &err),
    }
}

fn handle_mod_source_put(
    path: &str,
    body: &[u8],
    backend: &mut dyn AdminBackend,
) -> AdminHttpResponse {
    if body.len() > MAX_SOURCE_BODY_BYTES {
        return AdminHttpResponse::json_error(
            413,
            "Payload Too Large",
            "source payload exceeds 128 KiB",
        );
    }
    let Some(raw_rel) = path.strip_prefix("/api/mods/source/") else {
        return AdminHttpResponse::json_error(404, "Not Found", "not found");
    };
    let Some(rel) = url_decode(raw_rel) else {
        return AdminHttpResponse::json_error(
            400,
            "Bad Request",
            "invalid percent-encoding in source path",
        );
    };
    if let Err(msg) = ModStudioSandbox::validate_editable_rel_path(&rel) {
        return AdminHttpResponse::json_error(400, "Bad Request", msg);
    }
    let content = match extract_json_string_field(body, "content") {
        Ok(s) => s,
        Err(_) => match std::str::from_utf8(body) {
            Ok(raw) if !raw.trim_start().starts_with('{') => raw.to_string(),
            Ok(_) => {
                return AdminHttpResponse::json_error(
                    400,
                    "Bad Request",
                    "expected JSON {\"content\":\"...\"} or raw UTF-8 source",
                );
            }
            Err(_) => {
                return AdminHttpResponse::json_error(
                    400,
                    "Bad Request",
                    "source body must be valid UTF-8",
                );
            }
        },
    };
    match backend.write_mod_source(&rel, &content) {
        Ok(msg) => AdminHttpResponse::json(
            200,
            "OK",
            format!(
                "{{\"ok\":true,\"path\":{},\"message\":{}}}",
                escape_json_str(&rel),
                escape_json_str(&msg),
            ),
        ),
        Err(err) => AdminHttpResponse::json_error(400, "Bad Request", &err),
    }
}

/// Encodes unified ResMon plugin rows and editable Plugin Studio files into `/api/mods` JSON.
pub fn encode_mods_json(
    mods: &[cs_mod_host::PluginResmon],
    editable_files: &[String],
) -> String {
    let mut out = String::with_capacity(128 + mods.len() * 220 + editable_files.len() * 32);
    let _ = write!(out, "{{\"count\":{},\"mods\":[", mods.len());
    for (idx, m) in mods.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        let avg = if m.avg_us_per_tick.is_finite() && m.avg_us_per_tick >= 0.0 {
            m.avg_us_per_tick
        } else {
            0.0
        };
        let src_json = match m.source_path.as_deref() {
            Some(s) => escape_json_str(s),
            None => "null".to_string(),
        };
        let _ = write!(
            out,
            "{{\"id\":{},\"version\":{},\"language\":{},\"status\":{},\"enabled\":{},\"avg_us_per_tick\":{avg:.2},\"p99_us_per_tick\":{},\"max_us\":{},\"fuel_used\":{},\"memory_bytes\":{},\"hook_calls\":{},\"fault_count\":{},\"source_path\":{src_json}}}",
            escape_json_str(&m.id),
            escape_json_str(&m.version),
            escape_json_str(&m.language),
            escape_json_str(&m.status),
            m.enabled,
            m.p99_us_per_tick,
            m.max_us,
            m.fuel_used,
            m.memory_bytes,
            m.hook_calls,
            m.fault_count,
        );
    }
    out.push_str("],\"editable_files\":[");
    for (idx, f) in editable_files.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(&escape_json_str(f));
    }
    out.push_str("]}");
    out
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

        fn replay_timeline_json(&self, id: &str) -> Option<String> {
            (id == "r0001-de_dust2")
                .then(|| "{\"id\":\"r0001-de_dust2\",\"sample_rate_hz\":10,\"frames\":[]}".to_string())
        }

        fn map_overview_json(&self, map: &str) -> Option<String> {
            match map {
                "de_dust2" => Some(
                    "{\"map\":\"de_dust2\",\"has_overview\":true,\"zoom\":1.5,\"origin\":[-223,1097,-192],\"rotated\":false,\"mins\":[-2432,-1280,-512],\"maxs\":[2048,3200,1024],\"image_url\":\"/api/map-overview/de_dust2.png\"}".to_string(),
                ),
                "fy_custom" => Some(
                    "{\"map\":\"fy_custom\",\"has_overview\":false,\"zoom\":1.0,\"origin\":[0,0,0],\"rotated\":false,\"mins\":[-512,-512,-128],\"maxs\":[512,512,256],\"image_url\":null}".to_string(),
                ),
                _ => None,
            }
        }

        fn map_overview_png(&self, map: &str) -> Option<Vec<u8>> {
            (map == "de_dust2").then(|| b"\x89PNG\r\n\x1a\nfake-png".to_vec())
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
                ..status_req.clone()
            },
            &mut backend,
        );
        assert_eq!(invite_resp.status_code, 200);
        let invite_body = String::from_utf8(invite_resp.body).unwrap();
        assert!(invite_body.contains("?join=play.example.com%3A27016&room=dust-01"));

        // GET /api/replays/<id>/timeline returns 2D timeline JSON and blocks traversal.
        let tl_resp = AdminRouter::handle(
            &AdminHttpRequest {
                path: "/api/replays/r0001-de_dust2/timeline",
                ..status_req.clone()
            },
            &mut backend,
        );
        assert_eq!(tl_resp.status_code, 200);
        assert!(String::from_utf8(tl_resp.body).unwrap().contains("\"sample_rate_hz\":10"));

        let bad_tl = AdminRouter::handle(
            &AdminHttpRequest {
                path: "/api/replays/..%2Fbad/timeline",
                ..status_req.clone()
            },
            &mut backend,
        );
        assert_eq!(bad_tl.status_code, 400);

        // GET /api/map-overview/<map> and .png + missing-overview BSP bounds fallback.
        let ov_resp = AdminRouter::handle(
            &AdminHttpRequest {
                path: "/api/map-overview/de_dust2",
                ..status_req.clone()
            },
            &mut backend,
        );
        assert_eq!(ov_resp.status_code, 200);
        assert!(String::from_utf8(ov_resp.body).unwrap().contains("\"/api/map-overview/de_dust2.png\""));

        let png_resp = AdminRouter::handle(
            &AdminHttpRequest {
                path: "/api/map-overview/de_dust2.png",
                ..status_req.clone()
            },
            &mut backend,
        );
        assert_eq!(png_resp.status_code, 200);
        assert_eq!(png_resp.content_type, "image/png");
        assert!(png_resp.body.starts_with(b"\x89PNG\r\n\x1a\n"));

        let fb_resp = AdminRouter::handle(
            &AdminHttpRequest {
                path: "/api/map-overview/fy_custom",
                ..status_req.clone()
            },
            &mut backend,
        );
        assert_eq!(fb_resp.status_code, 200);
        let fb_body = String::from_utf8(fb_resp.body).unwrap();
        assert!(fb_body.contains("\"has_overview\":false"));
        assert!(fb_body.contains("\"mins\":[-512,-512,-128]"));

        let bad_ov = AdminRouter::handle(
            &AdminHttpRequest {
                path: "/api/map-overview/..%2Fetc",
                ..status_req
            },
            &mut backend,
        );
        assert_eq!(bad_ov.status_code, 400);
    }

    #[test]
    fn mod_studio_sandbox_blocks_traversal_and_symlinks_and_validates_python() {
        let tmp = std::env::temp_dir().join(format!(
            "cs16_studio_sandbox_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("mods/community")).unwrap();
        std::fs::create_dir_all(tmp.join("configs/lang")).unwrap();
        std::fs::write(
            tmp.join("mods/community/demo.py"),
            "import cs16\n@cs16.mod.info(id=\"demo\", version=\"1.0.0\")\ndef init(): pass\n",
        )
        .unwrap();
        std::fs::write(tmp.join("configs/admins.toml"), "[[admin]]\n").unwrap();
        std::fs::write(tmp.join("secret.txt"), "top-secret").unwrap();

        let sb = ModStudioSandbox::new(&tmp);
        let files = sb.list_editable_files();
        assert!(files.contains(&"mods/community/demo.py".to_string()));
        assert!(files.contains(&"configs/admins.toml".to_string()));
        assert!(!files.iter().any(|f| f.contains("secret.txt")));

        // Traversal attempts rejected.
        for bad in [
            "../secret.txt",
            "mods/../../secret.txt",
            "mods/../secret.txt",
            "mods/community/demo.wasm",
            "configs/evil.exe",
            "mods\\community\\demo.py",
            "/etc/passwd",
            "C:/Windows/win.ini",
        ] {
            assert!(sb.read_source(bad).is_err(), "should reject read `{bad}`");
            assert!(
                sb.write_source(bad, "x = 1\n").is_err(),
                "should reject write `{bad}`"
            );
        }

        // Valid Python syntax passes; broken syntax rejected before touching disk.
        let orig = sb.read_source("mods/community/demo.py").unwrap();
        assert!(orig.contains("demo"));
        assert!(
            sb.write_source("mods/community/demo.py", "def broken(:\n")
                .is_err()
        );
        assert_eq!(sb.read_source("mods/community/demo.py").unwrap(), orig);

        let updated = "import cs16\n@cs16.mod.info(id=\"demo\", version=\"1.1.0\")\ndef init(): pass\n";
        sb.write_source("mods/community/demo.py", updated).unwrap();
        assert_eq!(sb.read_source("mods/community/demo.py").unwrap(), updated);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn routes_mods_resmon_toggle_faults_and_studio_source_endpoints() {
        struct ModStubBackend {
            base: StubBackend,
            sandbox: ModStudioSandbox,
            last_toggle: Option<(String, String)>,
            reloads: usize,
        }

        impl AdminBackend for ModStubBackend {
            fn rcon_password(&self) -> &str {
                self.base.rcon_password()
            }
            fn status(&self) -> ServerStatusSnapshot {
                self.base.status()
            }
            fn players(&self) -> Vec<PlayerSnapshot> {
                self.base.players()
            }
            fn stats_json(&self) -> String {
                self.base.stats_json()
            }
            fn replays_index_json(&self) -> String {
                self.base.replays_index_json()
            }
            fn replay_dem(&self, id: &str) -> Option<Vec<u8>> {
                self.base.replay_dem(id)
            }
            fn execute_rcon(&mut self, command: &str) -> Result<String, String> {
                self.base.execute_rcon(command)
            }
            fn mods_json(&self) -> String {
                encode_mods_json(
                    &[cs_mod_host::PluginResmon {
                        id: "resetscore".to_string(),
                        version: "1.0.0".to_string(),
                        language: "Python".to_string(),
                        status: "Active".to_string(),
                        enabled: true,
                        avg_us_per_tick: 12.5,
                        p99_us_per_tick: 24,
                        max_us: 30,
                        fuel_used: 420,
                        memory_bytes: 8192,
                        hook_calls: 9,
                        fault_count: 0,
                        source_path: Some("mods/community/resetscore.py".to_string()),
                    }],
                    &self.sandbox.list_editable_files(),
                )
            }
            fn toggle_mod(&mut self, id: &str, action: &str) -> Result<String, String> {
                self.last_toggle = Some((id.to_string(), action.to_string()));
                Ok(format!("{action}d {id}"))
            }
            fn mod_faults_json(&self) -> String {
                "{\"count\":1,\"faults\":[{\"dump_id\":\"1000_bad_hook\",\"mod_id\":\"bad\"}]}".to_string()
            }
            fn mod_fault_detail_json(&self, id: &str) -> Option<String> {
                (id == "1000_bad_hook").then(|| "{\"dump_id\":\"1000_bad_hook\"}".to_string())
            }
            fn read_mod_source(&self, rel_path: &str) -> Result<String, String> {
                self.sandbox.read_source(rel_path)
            }
            fn write_mod_source(&mut self, rel_path: &str, content: &str) -> Result<String, String> {
                let _ = self.sandbox.write_source(rel_path, content)?;
                self.reloads += 1;
                Ok("hot-reloaded".to_string())
            }
        }

        let tmp = std::env::temp_dir().join(format!(
            "cs16_studio_routes_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("mods/community")).unwrap();
        std::fs::write(
            tmp.join("mods/community/resetscore.py"),
            "def on_init():\n    return 0\n",
        )
        .unwrap();

        let mut backend = ModStubBackend {
            base: StubBackend {
                rcon_password: "pw".to_string(),
                last_cmd: None,
                replay: Vec::new(),
            },
            sandbox: ModStudioSandbox::new(&tmp),
            last_toggle: None,
            reloads: 0,
        };

        let auth_get = |path: &'static str| AdminHttpRequest {
            method: "GET",
            path,
            query: None,
            authorization: Some("Bearer pw"),
            x_admin_token: None,
            host: None,
            body: b"",
        };

        // GET /api/mods
        let mods_resp = AdminRouter::handle(&auth_get("/api/mods"), &mut backend);
        assert_eq!(mods_resp.status_code, 200);
        let mods_body = String::from_utf8(mods_resp.body).unwrap();
        assert!(mods_body.contains("\"id\":\"resetscore\""));
        assert!(mods_body.contains("\"avg_us_per_tick\":12.50"));
        assert!(mods_body.contains("mods/community/resetscore.py"));

        // POST /api/mods/resetscore/toggle
        let tog_resp = AdminRouter::handle(
            &AdminHttpRequest {
                method: "POST",
                path: "/api/mods/resetscore/toggle",
                body: br#"{"action":"pause"}"#,
                ..auth_get("/api/mods/resetscore/toggle")
            },
            &mut backend,
        );
        assert_eq!(tog_resp.status_code, 200);
        assert_eq!(
            backend.last_toggle,
            Some(("resetscore".to_string(), "pause".to_string()))
        );

        // GET /api/mods/faults & download .json
        let fl_resp = AdminRouter::handle(&auth_get("/api/mods/faults"), &mut backend);
        assert_eq!(fl_resp.status_code, 200);
        let fd_resp =
            AdminRouter::handle(&auth_get("/api/mods/faults/1000_bad_hook.json"), &mut backend);
        assert_eq!(fd_resp.status_code, 200);
        assert_eq!(
            fd_resp.content_disposition.as_deref(),
            Some("attachment; filename=\"1000_bad_hook.json\"")
        );

        // GET & PUT /api/mods/source/<file>
        let src_get = AdminRouter::handle(
            &auth_get("/api/mods/source/mods/community/resetscore.py"),
            &mut backend,
        );
        assert_eq!(src_get.status_code, 200);
        assert!(String::from_utf8(src_get.body).unwrap().contains("on_init"));

        let src_put = AdminRouter::handle(
            &AdminHttpRequest {
                method: "PUT",
                path: "/api/mods/source/mods/community/resetscore.py",
                body: br#"{"content":"def on_init():\n    return 1\n"}"#,
                ..auth_get("/api/mods/source/mods/community/resetscore.py")
            },
            &mut backend,
        );
        assert_eq!(src_put.status_code, 200);
        assert_eq!(backend.reloads, 1);

        // Path traversal rejected at router level.
        let trav = AdminRouter::handle(
            &auth_get("/api/mods/source/..%2F..%2FCargo.toml"),
            &mut backend,
        );
        assert_eq!(trav.status_code, 400);

        let _ = std::fs::remove_dir_all(&tmp);
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

    #[test]
    fn lobby_signaling_routes_handle_cors_crud_and_signals() {
        let mut lobbies = LobbyRegistry::default();
        let opt_resp = handle_lobby_request(
            &AdminHttpRequest {
                method: "OPTIONS",
                path: "/lobbies/p2p-alpha/signal",
                query: None,
                authorization: None,
                x_admin_token: None,
                host: None,
                body: b"",
            },
            &mut lobbies,
            10.0,
        );
        assert_eq!(opt_resp.status_code, 204);
        assert!(opt_resp.cors_allow_origin);

        let post_room = handle_lobby_request(
            &AdminHttpRequest {
                method: "POST",
                path: "/lobbies",
                query: None,
                authorization: None,
                x_admin_token: None,
                host: None,
                body: br#"{"room":"p2p-alpha","host":"browser-p2p","port":443,"map":"de_dust2","players":1,"max_players":10,"mode":"webrtc"}"#,
            },
            &mut lobbies,
            10.0,
        );
        assert_eq!(post_room.status_code, 200);
        assert!(String::from_utf8(post_room.body).unwrap().contains("\"mode\":\"webrtc\""));

        let push_sig = handle_lobby_request(
            &AdminHttpRequest {
                method: "POST",
                path: "/lobbies/p2p-alpha/signal",
                query: None,
                authorization: None,
                x_admin_token: None,
                host: None,
                body: br#"{"from":"guest1","to":"host","kind":"join","payload":""}"#,
            },
            &mut lobbies,
            11.0,
        );
        assert_eq!(push_sig.status_code, 200);

        let poll_sig = handle_lobby_request(
            &AdminHttpRequest {
                method: "GET",
                path: "/lobbies/p2p-alpha/signal",
                query: Some("since=0&to=host"),
                authorization: None,
                x_admin_token: None,
                host: None,
                body: b"",
            },
            &mut lobbies,
            11.5,
        );
        assert_eq!(poll_sig.status_code, 200);
        assert!(String::from_utf8(poll_sig.body).unwrap().contains("\"kind\":\"join\""));

        let del_room = handle_lobby_request(
            &AdminHttpRequest {
                method: "DELETE",
                path: "/lobbies/p2p-alpha",
                query: None,
                authorization: None,
                x_admin_token: None,
                host: None,
                body: b"",
            },
            &mut lobbies,
            12.0,
        );
        assert_eq!(del_room.status_code, 200);
    }
}
