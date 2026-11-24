//! Pure-Rust RFC 6455 WebSocket framing, SHA-1 + Base64 handshake key derivation, and
//! HTTP/1.1 request parsing + safe static asset resolution (zero `unsafe`, zero extra deps).

use std::collections::HashMap;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

/// RFC 6455 §1.3 globally unique WebSocket handshake GUID.
pub const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// Maximum HTTP request head size accepted by the built-in server.
pub const MAX_HTTP_HEAD: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WsOpcode {
    Continuation = 0x0,
    Text = 0x1,
    Binary = 0x2,
    Close = 0x8,
    Ping = 0x9,
    Pong = 0xA,
}

impl WsOpcode {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x0 => Some(Self::Continuation),
            0x1 => Some(Self::Text),
            0x2 => Some(Self::Binary),
            0x8 => Some(Self::Close),
            0x9 => Some(Self::Ping),
            0xA => Some(Self::Pong),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsFrame {
    pub fin: bool,
    pub opcode: WsOpcode,
    pub payload: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum WsProtoError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid or unsupported WebSocket frame: {0}")]
    InvalidFrame(&'static str),
    #[error("WebSocket payload {len} exceeds maximum {max}")]
    PayloadTooLarge { len: usize, max: usize },
    #[error("invalid HTTP request: {0}")]
    InvalidHttp(&'static str),
}

/// Computes `base64(sha1(key + WS_GUID))` per RFC 6455 §1.3.
pub fn ws_accept_key(sec_websocket_key: &str) -> String {
    let mut input = Vec::with_capacity(sec_websocket_key.len() + WS_GUID.len());
    input.extend_from_slice(sec_websocket_key.trim().as_bytes());
    input.extend_from_slice(WS_GUID.as_bytes());
    let digest = sha1_digest(&input);
    base64_encode(&digest)
}

/// Encodes a single RFC 6455 frame; pass `Some(mask_key)` for client-to-server frames.
pub fn encode_frame(frame: &WsFrame, mask: Option<[u8; 4]>) -> Vec<u8> {
    let len = frame.payload.len();
    let mut out = Vec::with_capacity(len + 14);
    let b0 = (if frame.fin { 0x80 } else { 0 }) | (frame.opcode as u8);
    out.push(b0);

    let mask_bit = if mask.is_some() { 0x80 } else { 0 };
    if len <= 125 {
        out.push(mask_bit | (len as u8));
    } else if len <= 65535 {
        out.push(mask_bit | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(mask_bit | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }

    if let Some(key) = mask {
        out.extend_from_slice(&key);
        for (i, &byte) in frame.payload.iter().enumerate() {
            out.push(byte ^ key[i % 4]);
        }
    } else {
        out.extend_from_slice(&frame.payload);
    }
    out
}

/// Decodes one RFC 6455 frame, validating payload length against `max_payload` before allocating.
pub fn decode_frame(
    reader: &mut impl Read,
    max_payload: usize,
    require_masked: bool,
) -> Result<WsFrame, WsProtoError> {
    let mut head = [0u8; 2];
    reader.read_exact(&mut head)?;
    let fin = head[0] & 0x80 != 0;
    let rsv = head[0] & 0x70;
    if rsv != 0 {
        return Err(WsProtoError::InvalidFrame("non-zero reserved bits"));
    }
    let opcode = WsOpcode::from_u8(head[0] & 0x0F)
        .ok_or(WsProtoError::InvalidFrame("unsupported opcode"))?;
    let masked = head[1] & 0x80 != 0;
    if require_masked && !masked {
        return Err(WsProtoError::InvalidFrame("unmasked client frame"));
    }
    let len_code = head[1] & 0x7F;
    let payload_len: u64 = match len_code {
        0..=125 => u64::from(len_code),
        126 => {
            let mut ext = [0u8; 2];
            reader.read_exact(&mut ext)?;
            u64::from(u16::from_be_bytes(ext))
        }
        _ => {
            let mut ext = [0u8; 8];
            reader.read_exact(&mut ext)?;
            let raw = u64::from_be_bytes(ext);
            if raw & (1u64 << 63) != 0 {
                return Err(WsProtoError::InvalidFrame("high bit set on 64-bit length"));
            }
            raw
        }
    };

    if matches!(opcode, WsOpcode::Close | WsOpcode::Ping | WsOpcode::Pong)
        && (!fin || payload_len > 125)
    {
        return Err(WsProtoError::InvalidFrame("invalid control frame"));
    }

    let len = usize::try_from(payload_len).map_err(|_| WsProtoError::PayloadTooLarge {
        len: usize::MAX,
        max: max_payload,
    })?;
    if len > max_payload {
        return Err(WsProtoError::PayloadTooLarge {
            len,
            max: max_payload,
        });
    }

    let mask_key = if masked {
        let mut key = [0u8; 4];
        reader.read_exact(&mut key)?;
        Some(key)
    } else {
        None
    };

    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    if let Some(key) = mask_key {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= key[i % 4];
        }
    }

    Ok(WsFrame {
        fin,
        opcode,
        payload,
    })
}

/// Parsed HTTP/1.1 request head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: HashMap<String, String>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn is_websocket_upgrade(&self) -> bool {
        let has_upgrade_conn = self
            .header("connection")
            .is_some_and(|v| v.to_ascii_lowercase().contains("upgrade"));
        let has_ws_upg = self
            .header("upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
        has_upgrade_conn && has_ws_upg && self.header("sec-websocket-key").is_some()
    }
}

/// Attempts to parse an HTTP/1.1 head from `buf`; returns `Ok(None)` until `\r\n\r\n` arrives.
pub fn parse_http_request(buf: &[u8]) -> Result<Option<(HttpRequest, usize)>, WsProtoError> {
    let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        if buf.len() >= MAX_HTTP_HEAD {
            return Err(WsProtoError::InvalidHttp("header exceeds limit"));
        }
        return Ok(None);
    };
    let head_len = pos + 4;
    if head_len > MAX_HTTP_HEAD {
        return Err(WsProtoError::InvalidHttp("header exceeds limit"));
    }
    let text = std::str::from_utf8(&buf[..pos])
        .map_err(|_| WsProtoError::InvalidHttp("non-utf8 header"))?;
    let mut lines = text.split("\r\n");
    let req_line = lines
        .next()
        .ok_or(WsProtoError::InvalidHttp("empty request"))?;
    let mut parts = req_line.split_whitespace();
    let method = parts
        .next()
        .ok_or(WsProtoError::InvalidHttp("missing method"))?;
    let target = parts
        .next()
        .ok_or(WsProtoError::InvalidHttp("missing target"))?;
    let version = parts
        .next()
        .ok_or(WsProtoError::InvalidHttp("missing version"))?;
    if !version.starts_with("HTTP/1.") {
        return Err(WsProtoError::InvalidHttp("unsupported HTTP version"));
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (target.to_string(), None),
    };
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, val) = line
            .split_once(':')
            .ok_or(WsProtoError::InvalidHttp("malformed header line"))?;
        headers.insert(name.trim().to_ascii_lowercase(), val.trim().to_string());
    }
    Ok(Some((
        HttpRequest {
            method: method.to_ascii_uppercase(),
            path,
            query,
            headers,
        },
        head_len,
    )))
}

/// Reads an HTTP/1.1 request head from `stream`, capping read length at [`MAX_HTTP_HEAD`].
pub fn read_http_request(stream: &mut impl Read) -> Result<HttpRequest, WsProtoError> {
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 512];
    loop {
        if let Some((req, _)) = parse_http_request(&buf)? {
            return Ok(req);
        }
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Err(WsProtoError::InvalidHttp("connection closed before header end"));
        }
        let room = MAX_HTTP_HEAD.saturating_sub(buf.len());
        if n > room {
            return Err(WsProtoError::InvalidHttp("header exceeds limit"));
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

/// Safely resolves a URL path against `root`, rejecting traversal, backslashes, NUL, and drive prefixes.
pub fn resolve_static_path(root: &Path, url_path: &str) -> Option<PathBuf> {
    if !url_path.starts_with('/') || url_path.contains(['\\', '\0', ':']) || url_path.contains("..") {
        return None;
    }
    let trimmed = url_path.trim_start_matches('/');
    let rel_str = if trimmed.is_empty() {
        "index.html"
    } else {
        trimmed
    };
    let mut clean = PathBuf::new();
    for comp in Path::new(rel_str).components() {
        match comp {
            Component::Normal(seg) => {
                let s = seg.to_str()?;
                if s.is_empty() || s.starts_with('.') {
                    return None;
                }
                clean.push(s);
            }
            _ => return None,
        }
    }
    if clean.as_os_str().is_empty() {
        return None;
    }
    Some(root.join(clean))
}

pub fn mime_type_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("webmanifest") => "application/manifest+json; charset=utf-8",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// Pure-Rust SHA-1 digest (RFC 3174), used strictly for RFC 6455 Sec-WebSocket-Accept derivation.
fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((data.len() + 9).div_ceil(64) * 64);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, slot) in w[..16].iter_mut().enumerate() {
            let off = i * 4;
            *slot = u32::from_be_bytes([chunk[off], chunk[off + 1], chunk[off + 2], chunk[off + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (t, &wt) in w.iter().enumerate() {
            let (f, k) = match t {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDCu32),
                _ => (b ^ c ^ d, 0xCA62_C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wt);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0x0F) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rfc6455_accept_key_matches_standard_test_vector() {
        assert_eq!(
            ws_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn frame_roundtrip_handles_masked_unmasked_extended_and_caps() {
        for payload_len in [0usize, 5, 126, 300] {
            let payload: Vec<u8> = (0..payload_len).map(|i| (i & 0xFF) as u8).collect();
            let frame = WsFrame {
                fin: true,
                opcode: WsOpcode::Binary,
                payload: payload.clone(),
            };
            let masked_bytes = encode_frame(&frame, Some([0x12, 0x34, 0x56, 0x78]));
            let decoded = decode_frame(&mut Cursor::new(&masked_bytes), 4096, true).unwrap();
            assert_eq!(decoded, frame);

            let unmasked_bytes = encode_frame(&frame, None);
            assert!(matches!(
                decode_frame(&mut Cursor::new(&unmasked_bytes), 4096, true),
                Err(WsProtoError::InvalidFrame(_))
            ));
            let decoded_unmasked =
                decode_frame(&mut Cursor::new(&unmasked_bytes), 4096, false).unwrap();
            assert_eq!(decoded_unmasked, frame);
        }

        let big = WsFrame {
            fin: true,
            opcode: WsOpcode::Binary,
            payload: vec![7u8; 256],
        };
        let raw = encode_frame(&big, None);
        assert!(matches!(
            decode_frame(&mut Cursor::new(&raw), 100, false),
            Err(WsProtoError::PayloadTooLarge { len: 256, max: 100 })
        ));
    }

    #[test]
    fn parses_http_requests_and_blocks_static_path_traversal() {
        let raw = b"GET /relay?target=127.0.0.1:27016 HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
        let (req, end) = parse_http_request(raw).unwrap().unwrap();
        assert_eq!(end, raw.len());
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/relay");
        assert_eq!(req.query.as_deref(), Some("target=127.0.0.1:27016"));
        assert!(req.is_websocket_upgrade());

        let root = Path::new("web");
        assert_eq!(resolve_static_path(root, "/"), Some(root.join("index.html")));
        assert_eq!(
            resolve_static_path(root, "/pkg/cs_web.wasm"),
            Some(root.join("pkg").join("cs_web.wasm"))
        );
        assert_eq!(
            mime_type_for_path(Path::new("pkg/cs_web.wasm")),
            "application/wasm"
        );
        for bad in ["../secret", "/../etc/passwd", "/a\\b", "/.hidden", "/C:/x"] {
            assert!(resolve_static_path(root, bad).is_none(), "{bad}");
        }
    }
}
