//! Capability-gated (`permissions = ["http:<host>"]`) non-blocking HTTP POST JSON webhooks.
//!
//! - Enforces strict per-mod host allowlists (`is_host_allowed`), rejecting non-allowlisted
//!   domains, malformed URLs, or header-injection characters before any socket opens.
//! - On native targets, executes on a bounded background worker thread (`5s` timeout, `64 KiB`
//!   response cap) so the 100 Hz server game tick incurs `0.00 ms` network blocking.
//! - On `wasm32-unknown-unknown`, provides a clean zero-thread stub returning a structured error.

/// Hard per-request socket connect/read/write timeout (`5s`).
pub const WEBHOOK_TIMEOUT_SECS: u64 = 5;
/// Hard response body cap (`64 KiB`).
pub const WEBHOOK_MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// Hard request body cap (`64 KiB`).
pub const WEBHOOK_MAX_REQUEST_BYTES: usize = 64 * 1024;
/// Maximum queued outbound webhook requests before shedding load.
pub const WEBHOOK_QUEUE_CAPACITY: usize = 64;

/// Parsed and validated HTTP target URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHttpUrl {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path_and_query: String,
}

/// Validates and parses an outbound webhook URL (`http://host[:port]/path` or `https://...`).
pub fn parse_webhook_url(raw: &str) -> Result<ParsedHttpUrl, String> {
    let trimmed = raw.trim();
    if trimmed.contains('\r') || trimmed.contains('\n') || trimmed.contains('\0') {
        return Err("URL contains forbidden control characters".to_string());
    }
    let (scheme, default_port, rest) = if let Some(r) = trimmed.strip_prefix("http://") {
        ("http", 80u16, r)
    } else if let Some(r) = trimmed.strip_prefix("https://") {
        ("https", 443u16, r)
    } else {
        return Err("URL must begin with http:// or https://".to_string());
    };

    let (authority, path_and_query) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || authority.contains('@') {
        return Err("URL authority is empty or contains userinfo".to_string());
    }
    let (host_raw, port) = match authority.rsplit_once(':') {
        Some((h, p)) => {
            let parsed_port = p
                .parse::<u16>()
                .map_err(|_| format!("invalid port `{p}` in webhook URL"))?;
            (h, parsed_port)
        }
        None => (authority, default_port),
    };
    let host = host_raw.trim().to_ascii_lowercase();
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
    {
        return Err(format!("invalid hostname `{host_raw}`"));
    }
    Ok(ParsedHttpUrl {
        scheme: scheme.to_string(),
        host,
        port,
        path_and_query: path_and_query.to_string(),
    })
}

/// Returns `true` iff `(host, port)` is explicitly permitted by `allowed_hosts`
/// (exact lowercase host match, or exact `host:port` match).
pub fn is_host_allowed(allowed_hosts: &[String], host: &str, port: u16) -> bool {
    let normalized = host.trim().to_ascii_lowercase();
    let host_port = format!("{normalized}:{port}");
    allowed_hosts.iter().any(|allowed| {
        let a = allowed.trim();
        a.eq_ignore_ascii_case(&normalized) || a.eq_ignore_ascii_case(&host_port)
    })
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
    use std::thread;
    use std::time::Duration;

    use cs_mod_api::{HttpPostJsonSpec, HttpWebhookResult};

    use super::{
        ParsedHttpUrl, WEBHOOK_MAX_REQUEST_BYTES, WEBHOOK_MAX_RESPONSE_BYTES,
        WEBHOOK_QUEUE_CAPACITY, WEBHOOK_TIMEOUT_SECS, is_host_allowed, parse_webhook_url,
    };

    struct WorkItem {
        mod_id: String,
        callback_id: i32,
        parsed: ParsedHttpUrl,
        body: String,
    }

    /// Non-blocking outbound HTTP POST JSON client backed by a single daemon worker thread.
    pub struct WebhookClient {
        tx: Option<SyncSender<WorkItem>>,
        rx: Receiver<HttpWebhookResult>,
        next_request_id: u64,
    }

    impl Default for WebhookClient {
        fn default() -> Self {
            Self::new()
        }
    }

    impl WebhookClient {
        pub fn new() -> Self {
            let (work_tx, work_rx) = mpsc::sync_channel::<WorkItem>(WEBHOOK_QUEUE_CAPACITY);
            let (res_tx, res_rx) = mpsc::channel::<HttpWebhookResult>();
            let _ = thread::Builder::new()
                .name("cs16-mod-webhook".to_string())
                .spawn(move || {
                    while let Ok(item) = work_rx.recv() {
                        let result = execute_http_post(&item);
                        if res_tx.send(result).is_err() {
                            break;
                        }
                    }
                });
            Self {
                tx: Some(work_tx),
                rx: res_rx,
                next_request_id: 1,
            }
        }

        /// Validates allowlist + URL and enqueues an asynchronous HTTP POST JSON call.
        /// Never blocks the game tick. Returns the assigned request ID (`Err` if rejected).
        pub fn enqueue(
            &mut self,
            mod_id: &str,
            allowed_hosts: &[String],
            spec: HttpPostJsonSpec,
        ) -> Result<u64, String> {
            if spec.body.len() > WEBHOOK_MAX_REQUEST_BYTES {
                return Err(format!(
                    "webhook JSON body exceeds {WEBHOOK_MAX_REQUEST_BYTES} bytes"
                ));
            }
            let parsed = parse_webhook_url(&spec.url)?;
            if !is_host_allowed(allowed_hosts, &parsed.host, parsed.port) {
                return Err(format!(
                    "host `{}` is not in mod `{mod_id}` http:<host> allowlist",
                    parsed.host
                ));
            }
            let req_id = self.next_request_id;
            self.next_request_id = self.next_request_id.wrapping_add(1).max(1);

            let item = WorkItem {
                mod_id: mod_id.to_string(),
                callback_id: spec.callback_id,
                parsed,
                body: spec.body,
            };
            let Some(ref tx) = self.tx else {
                return Err("webhook worker shut down".to_string());
            };
            match tx.try_send(item) {
                Ok(()) => Ok(req_id),
                Err(TrySendError::Full(_)) => Err("webhook outbound queue is full".to_string()),
                Err(TrySendError::Disconnected(_)) => {
                    Err("webhook worker disconnected".to_string())
                }
            }
        }

        /// Drains all completed webhook results without blocking.
        pub fn poll_results(&mut self) -> Vec<HttpWebhookResult> {
            let mut out = Vec::new();
            while let Ok(item) = self.rx.try_recv() {
                out.push(item);
            }
            out
        }
    }

    impl Drop for WebhookClient {
        fn drop(&mut self) {
            self.tx.take();
        }
    }

    fn execute_http_post(item: &WorkItem) -> HttpWebhookResult {
        if item.parsed.scheme == "https" {
            return HttpWebhookResult {
                mod_id: item.mod_id.clone(),
                callback_id: item.callback_id,
                status_code: 0,
                body: String::new(),
                error: Some(
                    "https TLS transport requires an external TLS gateway; use local http relay or allowlisted http endpoint"
                        .to_string(),
                ),
            };
        }
        let timeout = Duration::from_secs(WEBHOOK_TIMEOUT_SECS);
        let addr_str = format!("{}:{}", item.parsed.host, item.parsed.port);
        let Ok(mut addrs) = addr_str.to_socket_addrs() else {
            return HttpWebhookResult {
                mod_id: item.mod_id.clone(),
                callback_id: item.callback_id,
                status_code: 0,
                body: String::new(),
                error: Some(format!("DNS resolution failed for `{addr_str}`")),
            };
        };
        let Some(sock_addr) = addrs.next() else {
            return HttpWebhookResult {
                mod_id: item.mod_id.clone(),
                callback_id: item.callback_id,
                status_code: 0,
                body: String::new(),
                error: Some(format!("no socket address resolved for `{addr_str}`")),
            };
        };
        let mut stream = match TcpStream::connect_timeout(&sock_addr, timeout) {
            Ok(s) => s,
            Err(err) => {
                return HttpWebhookResult {
                    mod_id: item.mod_id.clone(),
                    callback_id: item.callback_id,
                    status_code: 0,
                    body: String::new(),
                    error: Some(format!("connect failed: {err}")),
                };
            }
        };
        let _ = stream.set_read_timeout(Some(timeout));
        let _ = stream.set_write_timeout(Some(timeout));

        let req = format!(
            "POST {} HTTP/1.1\r\nHost: {}:{}\r\nUser-Agent: cs16-rs-mod-host/2.0\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            item.parsed.path_and_query,
            item.parsed.host,
            item.parsed.port,
            item.body.len(),
            item.body,
        );

        if let Err(err) = stream.write_all(req.as_bytes()) {
            return HttpWebhookResult {
                mod_id: item.mod_id.clone(),
                callback_id: item.callback_id,
                status_code: 0,
                body: String::new(),
                error: Some(format!("write failed: {err}")),
            };
        }

        let mut raw = Vec::new();
        let mut limited = stream.take((WEBHOOK_MAX_RESPONSE_BYTES + 4096) as u64);
        if let Err(err) = limited.read_to_end(&mut raw)
            && raw.is_empty()
        {
            return HttpWebhookResult {
                mod_id: item.mod_id.clone(),
                callback_id: item.callback_id,
                status_code: 0,
                body: String::new(),
                error: Some(format!("read failed: {err}")),
            };
        }

        let text = String::from_utf8_lossy(&raw);
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status_code = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse::<u16>().ok())
            .unwrap_or(0);
        let mut body_out = body.to_string();
        if body_out.len() > WEBHOOK_MAX_RESPONSE_BYTES {
            let mut end = WEBHOOK_MAX_RESPONSE_BYTES;
            while end > 0 && !body_out.is_char_boundary(end) {
                end -= 1;
            }
            body_out.truncate(end);
        }
        HttpWebhookResult {
            mod_id: item.mod_id.clone(),
            callback_id: item.callback_id,
            status_code,
            body: body_out,
            error: None,
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use cs_mod_api::{HttpPostJsonSpec, HttpWebhookResult};

    use super::{is_host_allowed, parse_webhook_url};

    /// `wasm32`-clean stub for outbound webhooks (validates allowlist and returns an immediate error).
    #[derive(Default)]
    pub struct WebhookClient {
        results: Vec<HttpWebhookResult>,
        next_request_id: u64,
    }

    impl WebhookClient {
        pub fn new() -> Self {
            Self {
                results: Vec::new(),
                next_request_id: 1,
            }
        }

        pub fn enqueue(
            &mut self,
            mod_id: &str,
            allowed_hosts: &[String],
            spec: HttpPostJsonSpec,
        ) -> Result<u64, String> {
            let parsed = parse_webhook_url(&spec.url)?;
            if !is_host_allowed(allowed_hosts, &parsed.host, parsed.port) {
                return Err(format!(
                    "host `{}` is not in mod `{mod_id}` http:<host> allowlist",
                    parsed.host
                ));
            }
            let req_id = self.next_request_id;
            self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
            self.results.push(HttpWebhookResult {
                mod_id: mod_id.to_string(),
                callback_id: spec.callback_id,
                status_code: 0,
                body: String::new(),
                error: Some("outbound HTTP webhooks are unavailable on wasm32".to_string()),
            });
            Ok(req_id)
        }

        pub fn poll_results(&mut self) -> Vec<HttpWebhookResult> {
            std::mem::take(&mut self.results)
        }
    }
}

pub use imp::WebhookClient;
