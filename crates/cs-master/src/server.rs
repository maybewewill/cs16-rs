//! Non-blocking UDP + optional HTTP/1.1 GoldSrc Master Server.

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::time::Duration;

use tracing::{debug, warn};

use crate::lobbies::{LobbyError, LobbyRegistry};
use crate::protocol::{
    MASTER_QUERY_OPCODE, MasterError, encode_query_reply, parse_query_request,
};
use crate::registry::{
    DEFAULT_MAX_CHALLENGES, DEFAULT_MAX_ENTRIES, DEFAULT_MAX_PER_PAGE, DEFAULT_TTL_SECS,
    MasterFilter, MasterRegistry,
};

const MAX_UDP_DATAGRAMS_PER_STEP: usize = 256;
const MAX_HTTP_ACCEPTS_PER_STEP: usize = 64;
const MAX_HTTP_REQUEST_BYTES: usize = 4096;
const MAX_HTTP_BODY_BYTES: usize = 8192;
const HTTP_STREAM_TIMEOUT: Duration = Duration::from_millis(200);

/// Configuration for [`MasterServer`].
#[derive(Debug, Clone)]
pub struct MasterConfig {
    pub udp_addr: SocketAddr,
    pub http_addr: Option<SocketAddr>,
    pub ttl_secs: f64,
    pub max_entries: usize,
    pub max_challenges: usize,
    pub max_per_page: usize,
}

impl Default for MasterConfig {
    fn default() -> Self {
        Self {
            udp_addr: "0.0.0.0:27010".parse().expect("valid default UDP addr"),
            http_addr: None,
            ttl_secs: DEFAULT_TTL_SECS,
            max_entries: DEFAULT_MAX_ENTRIES,
            max_challenges: DEFAULT_MAX_CHALLENGES,
            max_per_page: DEFAULT_MAX_PER_PAGE,
        }
    }
}

/// GoldSrc Master Server handling `0x31` queries, heartbeats, and an optional HTTP JSON endpoint.
pub struct MasterServer {
    udp: UdpSocket,
    udp_addr: SocketAddr,
    http: Option<TcpListener>,
    http_addr: Option<SocketAddr>,
    registry: MasterRegistry,
    lobbies: LobbyRegistry,
    max_per_page: usize,
}

impl MasterServer {
    /// Bind the UDP socket and optional HTTP/1.1 TCP listener in non-blocking mode.
    pub fn bind(config: MasterConfig) -> Result<Self, MasterError> {
        let udp = UdpSocket::bind(config.udp_addr)?;
        udp.set_nonblocking(true)?;
        let udp_addr = udp.local_addr()?;

        let (http, http_addr) = match config.http_addr {
            Some(addr) => {
                let listener = TcpListener::bind(addr)?;
                listener.set_nonblocking(true)?;
                let local = listener.local_addr()?;
                (Some(listener), Some(local))
            }
            None => (None, None),
        };

        Ok(Self {
            udp,
            udp_addr,
            http,
            http_addr,
            registry: MasterRegistry::new(
                config.ttl_secs,
                config.max_entries,
                config.max_challenges,
            ),
            lobbies: LobbyRegistry::default(),
            max_per_page: config.max_per_page.max(1),
        })
    }

    /// Local bound UDP address.
    pub fn udp_addr(&self) -> SocketAddr {
        self.udp_addr
    }

    /// Local bound HTTP address, if enabled.
    pub fn http_addr(&self) -> Option<SocketAddr> {
        self.http_addr
    }

    /// Read-only view of the underlying [`MasterRegistry`].
    pub fn registry(&self) -> &MasterRegistry {
        &self.registry
    }

    /// Mutable access to the underlying [`MasterRegistry`].
    pub fn registry_mut(&mut self) -> &mut MasterRegistry {
        &mut self.registry
    }

    /// Read-only view of the underlying [`LobbyRegistry`].
    pub fn lobbies(&self) -> &LobbyRegistry {
        &self.lobbies
    }

    /// Mutable access to the underlying [`LobbyRegistry`].
    pub fn lobbies_mut(&mut self) -> &mut LobbyRegistry {
        &mut self.lobbies
    }

    /// Advance expiration and process pending UDP datagrams and HTTP connections without blocking.
    ///
    /// Returns the number of UDP packets and HTTP connections handled during this step.
    pub fn step(&mut self, now_secs: f64) -> Result<usize, MasterError> {
        self.registry.expire_stale(now_secs);
        self.lobbies.expire_stale(now_secs);

        let mut handled = 0usize;
        handled += self.poll_udp(now_secs)?;
        handled += self.poll_http(now_secs)?;
        Ok(handled)
    }

    /// Run [`Self::step`] in a loop until `should_stop()` returns `true`.
    pub fn run_until(
        &mut self,
        mut should_stop: impl FnMut() -> bool,
        mut now_fn: impl FnMut() -> f64,
        tick_sleep: Duration,
    ) -> Result<(), MasterError> {
        while !should_stop() {
            let handled = self.step(now_fn())?;
            if handled == 0 && !tick_sleep.is_zero() {
                std::thread::sleep(tick_sleep);
            }
        }
        Ok(())
    }

    fn poll_udp(&mut self, now_secs: f64) -> Result<usize, MasterError> {
        let mut buf = [0u8; 4096];
        let mut count = 0usize;

        for _ in 0..MAX_UDP_DATAGRAMS_PER_STEP {
            match self.udp.recv_from(&mut buf) {
                Ok((len, SocketAddr::V4(peer))) => {
                    count += 1;
                    self.handle_udp_datagram(peer, &buf[..len], now_secs);
                }
                Ok((_len, SocketAddr::V6(peer))) => {
                    count += 1;
                    debug!(%peer, "ignoring IPv6 master UDP packet");
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::ConnectionReset
                            | ErrorKind::ConnectionAborted
                            | ErrorKind::Interrupted
                    ) =>
                {
                    continue;
                }
                Err(e) => return Err(MasterError::Io(e)),
            }
        }

        Ok(count)
    }

    fn handle_udp_datagram(
        &mut self,
        peer: std::net::SocketAddrV4,
        data: &[u8],
        now_secs: f64,
    ) {
        if data.first() == Some(&MASTER_QUERY_OPCODE) {
            match parse_query_request(data)
                .and_then(|req| Ok((req.cursor, MasterFilter::parse(req.region, &req.filter)?)))
            {
                Ok((cursor, filter)) => {
                    let (addrs, has_more) =
                        self.registry
                            .query(&filter, Some(cursor), self.max_per_page, now_secs);
                    let reply = encode_query_reply(&addrs, has_more);
                    if let Err(err) = self.udp.send_to(&reply, SocketAddr::V4(peer)) {
                        warn!(%peer, %err, "failed to send master query reply");
                    }
                }
                Err(err) => {
                    debug!(%peer, %err, "ignoring invalid master query packet");
                }
            }
            return;
        }

        match self.registry.handle_heartbeat(peer, data, now_secs) {
            Ok(Some(reply)) => {
                if let Err(err) = self.udp.send_to(&reply, SocketAddr::V4(peer)) {
                    warn!(%peer, %err, "failed to send heartbeat challenge");
                }
            }
            Ok(None) => {}
            Err(err) => {
                debug!(%peer, %err, "ignoring invalid heartbeat packet");
            }
        }
    }

    fn poll_http(&mut self, now_secs: f64) -> Result<usize, MasterError> {
        let Some(listener) = self.http.as_ref() else {
            return Ok(0);
        };

        let mut accepted = Vec::new();
        for _ in 0..MAX_HTTP_ACCEPTS_PER_STEP {
            match listener.accept() {
                Ok((stream, _peer)) => accepted.push(stream),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::Interrupted
                            | ErrorKind::ConnectionAborted
                            | ErrorKind::ConnectionReset
                    ) =>
                {
                    continue;
                }
                Err(e) => return Err(MasterError::Io(e)),
            }
        }
        let count = accepted.len();
        for stream in accepted {
            self.handle_http_connection(stream, now_secs);
        }
        Ok(count)
    }

    fn handle_http_connection(&mut self, mut stream: TcpStream, now_secs: f64) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(HTTP_STREAM_TIMEOUT));
        let _ = stream.set_write_timeout(Some(HTTP_STREAM_TIMEOUT));

        let mut buf = [0u8; MAX_HTTP_REQUEST_BYTES];
        let mut total = 0usize;
        let mut header_end = None;

        while total < MAX_HTTP_REQUEST_BYTES {
            match stream.read(&mut buf[total..]) {
                Ok(0) => break,
                Ok(n) => {
                    total += n;
                    if let Some(pos) = find_http_header_end(&buf[..total]) {
                        header_end = Some(pos);
                        break;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                    ) =>
                {
                    break;
                }
                Err(_) => return,
            }
        }

        let Some(head_end) = header_end else {
            if total == MAX_HTTP_REQUEST_BYTES {
                write_http_response(
                    &mut stream,
                    "413 Payload Too Large",
                    "text/plain; charset=utf-8",
                    None,
                    "Request headers too large\n",
                );
            } else {
                write_http_response(
                    &mut stream,
                    "400 Bad Request",
                    "text/plain; charset=utf-8",
                    None,
                    "Incomplete HTTP request\n",
                );
            }
            return;
        };

        let Ok(raw_req) = std::str::from_utf8(&buf[..head_end]) else {
            write_http_response(
                &mut stream,
                "400 Bad Request",
                "text/plain; charset=utf-8",
                None,
                "Invalid UTF-8 request\n",
            );
            return;
        };

        let mut lines = raw_req.lines();
        let Some(request_line) = lines.next() else {
            write_http_response(
                &mut stream,
                "400 Bad Request",
                "text/plain; charset=utf-8",
                None,
                "Empty request\n",
            );
            return;
        };

        let mut parts = request_line.split_whitespace();
        let (Some(method), Some(target), Some(version)) =
            (parts.next(), parts.next(), parts.next())
        else {
            write_http_response(
                &mut stream,
                "400 Bad Request",
                "text/plain; charset=utf-8",
                None,
                "Malformed request line\n",
            );
            return;
        };

        if !version.starts_with("HTTP/1.") {
            write_http_response(
                &mut stream,
                "400 Bad Request",
                "text/plain; charset=utf-8",
                None,
                "Unsupported HTTP version\n",
            );
            return;
        }

        let mut content_len = 0usize;
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                if k.trim().eq_ignore_ascii_case("transfer-encoding") {
                    write_http_response(
                        &mut stream,
                        "400 Bad Request",
                        "text/plain; charset=utf-8",
                        None,
                        "Transfer-Encoding not supported\n",
                    );
                    return;
                }
                if k.trim().eq_ignore_ascii_case("content-length") {
                    let Ok(parsed) = v.trim().parse::<usize>() else {
                        write_http_response(
                            &mut stream,
                            "400 Bad Request",
                            "text/plain; charset=utf-8",
                            None,
                            "Invalid Content-Length\n",
                        );
                        return;
                    };
                    if parsed > MAX_HTTP_BODY_BYTES {
                        write_http_response(
                            &mut stream,
                            "413 Payload Too Large",
                            "text/plain; charset=utf-8",
                            None,
                            "Request body too large\n",
                        );
                        return;
                    }
                    content_len = parsed;
                }
            }
        }

        let mut body = Vec::with_capacity(content_len);
        let leftover = &buf[head_end..total];
        if leftover.len() >= content_len {
            body.extend_from_slice(&leftover[..content_len]);
        } else {
            body.extend_from_slice(leftover);
            let remaining = content_len - body.len();
            let mut tail = vec![0u8; remaining];
            if stream.read_exact(&mut tail).is_err() {
                write_http_response(
                    &mut stream,
                    "400 Bad Request",
                    "text/plain; charset=utf-8",
                    None,
                    "Truncated request body\n",
                );
                return;
            }
            body.extend_from_slice(&tail);
        }

        if method == "OPTIONS" {
            write_http_response(
                &mut stream,
                "204 No Content",
                "text/plain; charset=utf-8",
                Some("Access-Control-Allow-Methods: GET, POST, DELETE, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\n"),
                "",
            );
            return;
        }

        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (target, None),
        };
        if path == "/servers" {
            if method == "GET" {
                let json = self.registry.to_json(now_secs);
                write_http_response(
                    &mut stream,
                    "200 OK",
                    "application/json; charset=utf-8",
                    None,
                    &json,
                );
            } else {
                write_http_response(
                    &mut stream,
                    "405 Method Not Allowed",
                    "text/plain; charset=utf-8",
                    Some("Allow: GET\r\n"),
                    "Method Not Allowed\n",
                );
            }
        } else if path == "/lobbies" || path.starts_with("/lobbies/") {
            self.handle_lobby_http(&mut stream, method, path, query, &body, now_secs);
        } else {
            write_http_response(
                &mut stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                None,
                "Not Found\n",
            );
        }
    }

    fn handle_lobby_http(
        &mut self,
        stream: &mut TcpStream,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
        now_secs: f64,
    ) {
        if path == "/lobbies" {
            match method {
                "GET" => {
                    let json = self.lobbies.list_json(now_secs);
                    write_http_response(
                        stream,
                        "200 OK",
                        "application/json; charset=utf-8",
                        None,
                        &json,
                    );
                }
                "POST" => match self.lobbies.upsert_from_json(body, now_secs) {
                    Ok(info) => write_http_response(
                        stream,
                        "200 OK",
                        "application/json; charset=utf-8",
                        None,
                        &info.to_json(),
                    ),
                    Err(err) => write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json; charset=utf-8",
                        None,
                        &format!("{{\"error\":\"{err}\"}}"),
                    ),
                },
                _ => write_http_response(
                    stream,
                    "405 Method Not Allowed",
                    "text/plain; charset=utf-8",
                    Some("Allow: GET, POST\r\n"),
                    "Method Not Allowed\n",
                ),
            }
            return;
        }

        let rest = path.strip_prefix("/lobbies/").unwrap_or("");
        if let Some(room) = rest.strip_suffix("/signal") {
            match method {
                "POST" => match self.lobbies.push_signal_from_json(room, body, now_secs) {
                    Ok(sig) => write_http_response(
                        stream,
                        "200 OK",
                        "application/json; charset=utf-8",
                        None,
                        &sig.to_json(),
                    ),
                    Err(LobbyError::NotFound) => write_http_response(
                        stream,
                        "404 Not Found",
                        "application/json; charset=utf-8",
                        None,
                        "{\"error\":\"lobby room not found\"}",
                    ),
                    Err(err) => write_http_response(
                        stream,
                        "400 Bad Request",
                        "application/json; charset=utf-8",
                        None,
                        &format!("{{\"error\":\"{err}\"}}"),
                    ),
                },
                "GET" => {
                    let (since_id, target) = parse_signal_query(query.unwrap_or(""));
                    match self
                        .lobbies
                        .poll_signals(room, since_id, target.as_deref(), now_secs)
                    {
                        Ok(sigs) => write_http_response(
                            stream,
                            "200 OK",
                            "application/json; charset=utf-8",
                            None,
                            &LobbyRegistry::signals_to_json(&sigs),
                        ),
                        Err(LobbyError::NotFound) => write_http_response(
                            stream,
                            "404 Not Found",
                            "application/json; charset=utf-8",
                            None,
                            "{\"error\":\"lobby room not found\"}",
                        ),
                        Err(err) => write_http_response(
                            stream,
                            "400 Bad Request",
                            "application/json; charset=utf-8",
                            None,
                            &format!("{{\"error\":\"{err}\"}}"),
                        ),
                    }
                }
                _ => write_http_response(
                    stream,
                    "405 Method Not Allowed",
                    "text/plain; charset=utf-8",
                    Some("Allow: GET, POST\r\n"),
                    "Method Not Allowed\n",
                ),
            }
            return;
        }

        match method {
            "GET" => match self.lobbies.get(rest, now_secs) {
                Some(info) => write_http_response(
                    stream,
                    "200 OK",
                    "application/json; charset=utf-8",
                    None,
                    &info.to_json(),
                ),
                None => write_http_response(
                    stream,
                    "404 Not Found",
                    "application/json; charset=utf-8",
                    None,
                    "{\"error\":\"lobby room not found\"}",
                ),
            },
            "DELETE" => {
                if self.lobbies.remove(rest) {
                    write_http_response(
                        stream,
                        "200 OK",
                        "application/json; charset=utf-8",
                        None,
                        "{\"deleted\":true}",
                    );
                } else {
                    write_http_response(
                        stream,
                        "404 Not Found",
                        "application/json; charset=utf-8",
                        None,
                        "{\"error\":\"lobby room not found\"}",
                    );
                }
            }
            _ => write_http_response(
                stream,
                "405 Method Not Allowed",
                "text/plain; charset=utf-8",
                Some("Allow: GET, DELETE\r\n"),
                "Method Not Allowed\n",
            ),
        }
    }
}

fn parse_signal_query(query: &str) -> (u64, Option<String>) {
    let mut since = 0u64;
    let mut target = None;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == "since"
                && let Ok(parsed) = v.parse::<u64>()
            {
                since = parsed;
            } else if k == "to" && !v.is_empty() {
                target = Some(v.to_string());
            }
        }
    }
    (since, target)
}

fn find_http_header_end(buf: &[u8]) -> Option<usize> {
    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
        return Some(pos + 4);
    }
    buf.windows(2).position(|w| w == b"\n\n").map(|pos| pos + 2)
}

fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    extra_headers: Option<&str>,
    body: &str,
) {
    let extra = extra_headers.unwrap_or("");
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nAccess-Control-Allow-Origin: *\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddrV4;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use crate::client::query_master_servers;
    use crate::protocol::{
        REGION_ALL, encode_heartbeat_challenge_request, encode_heartbeat_shutdown,
        encode_heartbeat_update, parse_heartbeat_challenge,
    };

    fn http_get(addr: SocketAddr, request: &str) -> String {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().unwrap();
        let mut out = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&chunk[..n]),
                Err(e)
                    if !out.is_empty()
                        && matches!(
                            e.kind(),
                            ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
                        ) =>
                {
                    break;
                }
                Err(e) => panic!("http_get read failed: {e}"),
            }
        }
        String::from_utf8(out).unwrap()
    }

    fn register_udp_server(
        master: SocketAddr,
        gamedir: &str,
        map: &str,
        players: &str,
        extra: &[(&str, &str)],
    ) -> (UdpSocket, SocketAddrV4) {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let local_v4 = match sock.local_addr().unwrap() {
            SocketAddr::V4(v4) => v4,
            SocketAddr::V6(_) => unreachable!(),
        };

        sock.send_to(&encode_heartbeat_challenge_request(), master)
            .unwrap();
        let mut buf = [0u8; 128];
        let (n, _) = sock.recv_from(&mut buf).unwrap();
        let ch = parse_heartbeat_challenge(&buf[..n]).unwrap();
        let ch_str = ch.to_string();

        let mut fields = vec![
            ("protocol", "48"),
            ("challenge", ch_str.as_str()),
            ("gamedir", gamedir),
            ("map", map),
            ("players", players),
            ("max", "32"),
            ("bots", "0"),
            ("type", "d"),
            ("password", "0"),
            ("os", "l"),
            ("secure", "1"),
            ("region", "3"),
        ];
        fields.extend_from_slice(extra);
        let pkt = encode_heartbeat_update(&fields);
        sock.send_to(&pkt, master).unwrap();

        (sock, local_v4)
    }

    fn wait_until(mut pred: impl FnMut() -> bool) {
        for _ in 0..200 {
            if pred() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("condition not satisfied within timeout");
    }

    #[test]
    fn end_to_end_heartbeat_paginated_query_http_shutdown_and_ttl() {
        let config = MasterConfig {
            udp_addr: "127.0.0.1:0".parse().unwrap(),
            http_addr: Some("127.0.0.1:0".parse().unwrap()),
            ttl_secs: 300.0,
            max_entries: 128,
            max_challenges: 128,
            max_per_page: 2,
        };
        let mut server = MasterServer::bind(config).unwrap();
        let udp_addr = server.udp_addr();
        let http_addr = server.http_addr().unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let sim_now = Arc::new(AtomicU64::new(100.0_f64.to_bits()));
        let stop_bg = Arc::clone(&stop);
        let now_bg = Arc::clone(&sim_now);

        let handle = std::thread::spawn(move || {
            server
                .run_until(
                    || stop_bg.load(Ordering::Acquire),
                    || f64::from_bits(now_bg.load(Ordering::Acquire)),
                    Duration::from_millis(2),
                )
                .unwrap();
        });

        // Register 3 cstrike/de_dust2 servers (will span 2 pages with max_per_page = 2)
        // plus 1 czero server.
        let (sock1, addr1) = register_udp_server(udp_addr, "cstrike", "de_dust2", "10", &[]);
        let (_sock2, addr2) = register_udp_server(udp_addr, "cstrike", "de_dust2", "5", &[]);
        let (_sock3, addr3) = register_udp_server(udp_addr, "cstrike", "de_dust2", "1", &[]);
        let (_sock4, addr4) = register_udp_server(udp_addr, "czero", "de_inferno", "2", &[]);

        wait_until(|| {
            query_master_servers(udp_addr, REGION_ALL, "", Duration::from_secs(1))
                .map(|v| v.len() == 4)
                .unwrap_or(false)
        });

        // Filtered + paginated 0x31 query through client::query_master_servers.
        let mut expected_cstrike = vec![addr1, addr2, addr3];
        expected_cstrike.sort();
        let got_cstrike = query_master_servers(
            udp_addr,
            3,
            "\\gamedir\\cstrike\\map\\de_dust2\\empty\\1\\full\\1",
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(got_cstrike, expected_cstrike);

        // GET /servers returns HTTP 200 JSON with CORS header.
        let resp = http_get(
            http_addr,
            "GET /servers HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(resp.starts_with("HTTP/1.1 200 OK\r\n"), "{resp}");
        assert!(resp.contains("Access-Control-Allow-Origin: *\r\n"));
        assert!(resp.contains("Content-Type: application/json; charset=utf-8\r\n"));
        let (_, body) = resp.split_once("\r\n\r\n").unwrap();
        assert!(body.contains(&addr1.to_string()));
        assert!(body.contains(&addr4.to_string()));
        assert!(body.contains("\"gamedir\":\"cstrike\""));
        assert!(body.contains("\"gamedir\":\"czero\""));

        // 405 Method Not Allowed on POST /servers, 404 on unknown route, 413 on oversized header.
        let resp_405 = http_get(
            http_addr,
            "POST /servers HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(resp_405.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));

        let resp_404 = http_get(
            http_addr,
            "GET /unknown HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(resp_404.starts_with("HTTP/1.1 404 Not Found\r\n"));

        let huge_req = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(5000));
        let resp_413 = http_get(http_addr, &huge_req);
        assert!(resp_413.starts_with("HTTP/1.1 413 Payload Too Large\r\n"));

        // Shutdown server 1 via `b\n`.
        sock1
            .send_to(&encode_heartbeat_shutdown(), udp_addr)
            .unwrap();
        wait_until(|| {
            query_master_servers(udp_addr, REGION_ALL, "", Duration::from_secs(1))
                .map(|v| v.len() == 3 && !v.contains(&addr1))
                .unwrap_or(false)
        });

        // Advance simulated clock past 300.0s TTL -> all remaining entries expire.
        sim_now.store(405.0_f64.to_bits(), Ordering::Release);
        wait_until(|| {
            query_master_servers(udp_addr, REGION_ALL, "", Duration::from_secs(1))
                .map(|v| v.is_empty())
                .unwrap_or(false)
        });

        let empty_http = http_get(http_addr, "GET /servers HTTP/1.1\r\nHost: localhost\r\n\r\n");
        let (_, empty_body) = empty_http.split_once("\r\n\r\n").unwrap();
        assert_eq!(empty_body, "[]");

        // /lobbies registration, listing, signaling, and deletion.
        let lobby_body = r#"{"room":"alpha","host":"127.0.0.1","port":27016,"map":"de_dust2","players":2,"max_players":16,"mode":"ws"}"#;
        let create_lobby = http_get(
            http_addr,
            &format!(
                "POST /lobbies HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
                lobby_body.len(),
                lobby_body
            ),
        );
        assert!(create_lobby.starts_with("HTTP/1.1 200 OK\r\n"), "{create_lobby}");
        assert!(create_lobby.contains("?join=127.0.0.1:27016&room=alpha"));

        let list_lobbies = http_get(http_addr, "GET /lobbies HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert!(list_lobbies.contains("\"room\":\"alpha\""));

        let sig_body = r#"{"from":"host","to":"guest","kind":"offer","payload":"sdp-offer"}"#;
        let push_sig = http_get(
            http_addr,
            &format!(
                "POST /lobbies/alpha/signal HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
                sig_body.len(),
                sig_body
            ),
        );
        assert!(push_sig.starts_with("HTTP/1.1 200 OK\r\n"), "{push_sig}");

        let poll_sig = http_get(
            http_addr,
            "GET /lobbies/alpha/signal?since=0&to=guest HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(poll_sig.contains("sdp-offer"));

        let del_lobby = http_get(
            http_addr,
            "DELETE /lobbies/alpha HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(del_lobby.contains("\"deleted\":true"));

        stop.store(true, Ordering::Release);
        handle.join().unwrap();
    }
}
