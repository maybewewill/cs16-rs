//! Native-only RFC 6455 WebSocket client/server transport and static HTTP/1.1 asset host.
//! Zero `unsafe` code; designed for multiplexing with UDP in `cs-server --web-port` and
//! integration testing `cs-relay`.

use std::collections::{HashMap, VecDeque};
use std::io::{self, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::ws_proto::{
    HttpRequest, WsFrame, WsOpcode, decode_frame, encode_frame, mime_type_for_path,
    read_http_request_with_leftover, resolve_static_path, ws_accept_key,
};
use crate::{MAX_UDP_PACKET, PacketTransport, PeerAddr, TransportError, check_len};

const MAX_INBOUND_QUEUE: usize = 1024;
const WS_READ_TIMEOUT: Duration = Duration::from_millis(150);

/// Optional upgrade handler invoked for non-default WebSocket paths (such as `/relay`).
pub type UpgradeHook = Arc<dyn Fn(&HttpRequest, TcpStream, SocketAddr) + Send + Sync>;

/// Optional HTTP/1.1 handler invoked for non-upgrade requests before static asset fallback.
/// Returning `true` indicates the hook handled the request and wrote the response.
pub type HttpHook =
    Arc<dyn Fn(&HttpRequest, &[u8], &mut TcpStream, SocketAddr) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct WsServerConfig {
    pub bind_addr: String,
    pub static_dir: Option<PathBuf>,
    pub ws_path: String,
    pub max_clients: usize,
    pub upgrade_hook: Option<UpgradeHook>,
    pub http_hook: Option<HttpHook>,
}

impl Default for WsServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:27016".to_string(),
            static_dir: None,
            ws_path: "/ws".to_string(),
            max_clients: 64,
            upgrade_hook: None,
            http_hook: None,
        }
    }
}

/// Sends an RFC 6455 `101 Switching Protocols` response using `sec_websocket_key`.
pub fn complete_ws_handshake(stream: &mut TcpStream, sec_websocket_key: &str) -> io::Result<()> {
    let accept = ws_accept_key(sec_websocket_key);
    let reply = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream.write_all(reply.as_bytes())?;
    stream.flush()
}

/// Writes an HTTP/1.1 response with explicit status line and closes the connection.
pub fn send_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    head_only: bool,
) -> io::Result<()> {
    let header = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-cache\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    if !head_only && !body.is_empty() {
        stream.write_all(body)?;
    }
    stream.flush()
}

type PeerSenderMap = Arc<Mutex<HashMap<PeerAddr, mpsc::Sender<Vec<u8>>>>>;
type PacketQueue = Arc<Mutex<VecDeque<(PeerAddr, Vec<u8>)>>>;

/// Native HTTP + WebSocket transport emitting `PeerAddr::Web(id)` peers.
pub struct WsServerTransport {
    local_addr: SocketAddr,
    running: Arc<AtomicBool>,
    inbound: PacketQueue,
    peers: PeerSenderMap,
    acceptor: Option<JoinHandle<()>>,
}

impl WsServerTransport {
    pub fn bind(cfg: WsServerConfig) -> io::Result<Self> {
        let listener = TcpListener::bind(&cfg.bind_addr)?;
        listener.set_nonblocking(true)?;
        let local_addr = listener.local_addr()?;

        let running = Arc::new(AtomicBool::new(true));
        let inbound: PacketQueue = Arc::new(Mutex::new(VecDeque::new()));
        let peers: PeerSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let next_id = Arc::new(AtomicU32::new(1));
        let active_clients = Arc::new(AtomicUsize::new(0));

        let r_flag = Arc::clone(&running);
        let q = Arc::clone(&inbound);
        let p_map = Arc::clone(&peers);
        let acceptor = std::thread::spawn(move || {
            while r_flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, addr)) => {
                        let cfg = cfg.clone();
                        let r_flag = Arc::clone(&r_flag);
                        let q = Arc::clone(&q);
                        let p_map = Arc::clone(&p_map);
                        let next_id = Arc::clone(&next_id);
                        let active = Arc::clone(&active_clients);
                        std::thread::spawn(move || {
                            handle_incoming_tcp(stream, addr, cfg, r_flag, q, p_map, next_id, active);
                        });
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(15));
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            local_addr,
            running,
            inbound,
            peers,
            acceptor: Some(acceptor),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl Drop for WsServerTransport {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Ok(mut map) = self.peers.lock() {
            map.clear();
        }
        if let Some(h) = self.acceptor.take() {
            let _ = h.join();
        }
    }
}

impl PacketTransport for WsServerTransport {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        self.inbound.lock().ok()?.pop_front()
    }

    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), TransportError> {
        check_len(data)?;
        let tx = {
            let map = self.peers.lock().map_err(|_| TransportError::Closed)?;
            map.get(&to).cloned()
        };
        let Some(tx) = tx else {
            return Err(TransportError::Closed);
        };
        let frame = encode_frame(
            &WsFrame {
                fin: true,
                opcode: WsOpcode::Binary,
                payload: data.to_vec(),
            },
            None,
        );
        if tx.send(frame).is_err() {
            if let Ok(mut map) = self.peers.lock() {
                map.remove(&to);
            }
            return Err(TransportError::Closed);
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_incoming_tcp(
    mut stream: TcpStream,
    peer_ip: SocketAddr,
    cfg: WsServerConfig,
    running: Arc<AtomicBool>,
    inbound: PacketQueue,
    peers: PeerSenderMap,
    next_id: Arc<AtomicU32>,
    active_clients: Arc<AtomicUsize>,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let Ok((req, leftover)) = read_http_request_with_leftover(&mut stream) else {
        return;
    };

    if req.is_websocket_upgrade() {
        if req.path == cfg.ws_path {
            if active_clients.load(Ordering::Relaxed) >= cfg.max_clients {
                let _ = send_http_response(
                    &mut stream,
                    "503 Service Unavailable",
                    "text/plain; charset=utf-8",
                    b"server full",
                    false,
                );
                return;
            }
            let Some(key) = req.header("sec-websocket-key") else {
                return;
            };
            if complete_ws_handshake(&mut stream, key).is_err() {
                return;
            }
            let id = next_id.fetch_add(1, Ordering::Relaxed);
            let peer = PeerAddr::Web(id);
            active_clients.fetch_add(1, Ordering::Relaxed);
            pump_ws_peer(stream, peer, running, inbound, peers);
            active_clients.fetch_sub(1, Ordering::Relaxed);
        } else if let Some(hook) = &cfg.upgrade_hook {
            (hook)(&req, stream, peer_ip);
        } else {
            let _ = send_http_response(
                &mut stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                b"unknown websocket endpoint",
                false,
            );
        }
        return;
    }

    if let Some(hook) = &cfg.http_hook
        && (hook)(&req, &leftover, &mut stream, peer_ip)
    {
        return;
    }

    if req.method != "GET" && req.method != "HEAD" {
        let _ = send_http_response(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            b"method not allowed",
            false,
        );
        return;
    }
    let head_only = req.method == "HEAD";
    serve_static(&mut stream, cfg.static_dir.as_deref(), &req.path, head_only);
}

fn serve_static(
    stream: &mut TcpStream,
    static_dir: Option<&std::path::Path>,
    url_path: &str,
    head_only: bool,
) {
    let Some(root) = static_dir else {
        if url_path == "/" || url_path == "/index.html" {
            let _ = send_http_response(
                stream,
                "200 OK",
                "text/html; charset=utf-8",
                b"<!doctype html><html><body>cs16-rs WebSocket host</body></html>",
                head_only,
            );
        } else {
            let _ = send_http_response(
                stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                b"not found",
                head_only,
            );
        }
        return;
    };

    let Some(file_path) = resolve_static_path(root, url_path) else {
        let _ = send_http_response(
            stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"not found",
            head_only,
        );
        return;
    };
    match std::fs::read(&file_path) {
        Ok(bytes) => {
            let mime = mime_type_for_path(&file_path);
            let _ = send_http_response(stream, "200 OK", mime, &bytes, head_only);
        }
        Err(_) => {
            let _ = send_http_response(
                stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                b"not found",
                head_only,
            );
        }
    }
}

fn pump_ws_peer(
    stream: TcpStream,
    peer: PeerAddr,
    running: Arc<AtomicBool>,
    inbound: PacketQueue,
    peers: PeerSenderMap,
) {
    let Ok(mut write_stream) = stream.try_clone() else {
        return;
    };
    let _ = stream.set_read_timeout(Some(WS_READ_TIMEOUT));
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    if let Ok(mut map) = peers.lock() {
        map.insert(peer, tx.clone());
    }

    let writer_running = Arc::clone(&running);
    let writer = std::thread::spawn(move || {
        while writer_running.load(Ordering::Relaxed) {
            match rx.recv_timeout(WS_READ_TIMEOUT) {
                Ok(bytes) => {
                    if write_stream.write_all(&bytes).is_err() || write_stream.flush().is_err() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    let mut reader = BufReader::new(stream);
    while running.load(Ordering::Relaxed) {
        match decode_frame(&mut reader, MAX_UDP_PACKET, true) {
            Ok(frame) => match frame.opcode {
                WsOpcode::Binary => {
                    if !frame.payload.is_empty()
                        && let Ok(mut q) = inbound.lock()
                    {
                        if q.len() >= MAX_INBOUND_QUEUE {
                            q.pop_front();
                        }
                        q.push_back((peer, frame.payload));
                    }
                }
                WsOpcode::Ping => {
                    let pong = encode_frame(
                        &WsFrame {
                            fin: true,
                            opcode: WsOpcode::Pong,
                            payload: frame.payload,
                        },
                        None,
                    );
                    if tx.send(pong).is_err() {
                        break;
                    }
                }
                WsOpcode::Close => {
                    let close = encode_frame(
                        &WsFrame {
                            fin: true,
                            opcode: WsOpcode::Close,
                            payload: Vec::new(),
                        },
                        None,
                    );
                    let _ = tx.send(close);
                    break;
                }
                WsOpcode::Text | WsOpcode::Continuation | WsOpcode::Pong => {}
            },
            Err(crate::ws_proto::WsProtoError::Io(e))
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => break,
        }
    }

    if let Ok(mut map) = peers.lock() {
        map.remove(&peer);
    }
    drop(tx);
    let _ = writer.join();
}

/// Parses `ws://host:port/path?query` (or `host:port/path`) into `(host_port, request_uri)`.
pub fn parse_ws_url(url: &str) -> Result<(String, String), String> {
    let trimmed = url
        .trim()
        .strip_prefix("ws://")
        .or_else(|| url.trim().strip_prefix("http://"))
        .unwrap_or(url.trim());
    let (host_port, rest) = match trimmed.split_once('/') {
        Some((hp, path)) => (hp.to_string(), format!("/{path}")),
        None => (trimmed.to_string(), "/".to_string()),
    };
    if host_port.is_empty() {
        return Err(format!("missing host in WebSocket URL `{url}`"));
    }
    Ok((host_port, rest))
}

/// Native RFC 6455 WebSocket client implementing [`PacketTransport`] (peer `PeerAddr::Web(0)`).
pub struct WsClientTransport {
    remote: PeerAddr,
    running: Arc<AtomicBool>,
    inbound: Arc<Mutex<VecDeque<Vec<u8>>>>,
    tx: mpsc::Sender<Vec<u8>>,
    closed: Arc<AtomicBool>,
    mask_seq: AtomicU32,
    reader_thread: Option<JoinHandle<()>>,
    writer_thread: Option<JoinHandle<()>>,
}

impl WsClientTransport {
    /// Connects to `ws://host:port/path` and completes the RFC 6455 upgrade handshake.
    pub fn connect(url: &str) -> Result<Self, String> {
        let (host_port, req_uri) = parse_ws_url(url)?;
        let addr = host_port
            .to_socket_addrs()
            .map_err(|e| format!("resolve {host_port}: {e}"))?
            .next()
            .ok_or_else(|| format!("no address for {host_port}"))?;
        let mut stream =
            TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| e.to_string())?;
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));

        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let expected_accept = ws_accept_key(key);
        let req = format!(
            "GET {req_uri} HTTP/1.1\r\n\
             Host: {host_port}\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: {key}\r\n\
             Sec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
        stream.flush().map_err(|e| e.to_string())?;

        // Read HTTP response header before any trailing WebSocket frames.
        let mut head_buf = Vec::with_capacity(512);
        let mut byte = [0u8; 1];
        loop {
            stream.read_exact(&mut byte).map_err(|e| e.to_string())?;
            head_buf.push(byte[0]);
            if head_buf.ends_with(b"\r\n\r\n") {
                break;
            }
            if head_buf.len() > 8192 {
                return Err("HTTP upgrade response exceeded 8 KB".into());
            }
        }
        let head_str = String::from_utf8_lossy(&head_buf);
        let first_line = head_str.lines().next().unwrap_or("");
        if !first_line.contains(" 101 ") {
            return Err(format!("WebSocket upgrade rejected: {first_line}"));
        }
        let has_accept = head_str.lines().any(|line| {
            line.split_once(':')
                .is_some_and(|(k, v)| k.trim().eq_ignore_ascii_case("sec-websocket-accept") && v.trim() == expected_accept)
        });
        if !has_accept {
            return Err("missing or mismatched Sec-WebSocket-Accept".into());
        }

        let running = Arc::new(AtomicBool::new(true));
        let closed = Arc::new(AtomicBool::new(false));
        let inbound = Arc::new(Mutex::new(VecDeque::new()));
        let (tx, rx) = mpsc::channel::<Vec<u8>>();

        let mut write_stream = stream.try_clone().map_err(|e| e.to_string())?;
        let w_run = Arc::clone(&running);
        let w_closed = Arc::clone(&closed);
        let writer_thread = std::thread::spawn(move || {
            while w_run.load(Ordering::Relaxed) {
                match rx.recv_timeout(WS_READ_TIMEOUT) {
                    Ok(bytes) => {
                        if write_stream.write_all(&bytes).is_err() || write_stream.flush().is_err() {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            w_closed.store(true, Ordering::Relaxed);
        });

        let _ = stream.set_read_timeout(Some(WS_READ_TIMEOUT));
        let r_run = Arc::clone(&running);
        let r_closed = Arc::clone(&closed);
        let r_in = Arc::clone(&inbound);
        let r_tx = tx.clone();
        let reader_thread = std::thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            while r_run.load(Ordering::Relaxed) {
                match decode_frame(&mut reader, MAX_UDP_PACKET, false) {
                    Ok(frame) => match frame.opcode {
                        WsOpcode::Binary => {
                            if let Ok(mut q) = r_in.lock() {
                                if q.len() >= MAX_INBOUND_QUEUE {
                                    q.pop_front();
                                }
                                q.push_back(frame.payload);
                            }
                        }
                        WsOpcode::Ping => {
                            let pong = encode_frame(
                                &WsFrame {
                                    fin: true,
                                    opcode: WsOpcode::Pong,
                                    payload: frame.payload,
                                },
                                Some([0xA5, 0x5A, 0x3C, 0xC3]),
                            );
                            if r_tx.send(pong).is_err() {
                                break;
                            }
                        }
                        WsOpcode::Close => break,
                        WsOpcode::Text | WsOpcode::Continuation | WsOpcode::Pong => {}
                    },
                    Err(crate::ws_proto::WsProtoError::Io(e))
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::TimedOut =>
                    {
                        continue;
                    }
                    Err(_) => break,
                }
            }
            r_closed.store(true, Ordering::Relaxed);
        });

        Ok(Self {
            remote: PeerAddr::Web(0),
            running,
            inbound,
            tx,
            closed,
            mask_seq: AtomicU32::new(0x1357_2468),
            reader_thread: Some(reader_thread),
            writer_thread: Some(writer_thread),
        })
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}

impl Drop for WsClientTransport {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        let close = encode_frame(
            &WsFrame {
                fin: true,
                opcode: WsOpcode::Close,
                payload: Vec::new(),
            },
            Some([0x11, 0x22, 0x33, 0x44]),
        );
        let _ = self.tx.send(close);
        if let Some(h) = self.reader_thread.take() {
            let _ = h.join();
        }
        if let Some(h) = self.writer_thread.take() {
            let _ = h.join();
        }
    }
}

impl PacketTransport for WsClientTransport {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        let pkt = self.inbound.lock().ok()?.pop_front()?;
        Some((self.remote, pkt))
    }

    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), TransportError> {
        check_len(data)?;
        if self.closed.load(Ordering::Relaxed) || to != self.remote {
            return Err(TransportError::Closed);
        }
        let seq = self.mask_seq.fetch_add(0x9E37_79B9, Ordering::Relaxed);
        let frame = encode_frame(
            &WsFrame {
                fin: true,
                opcode: WsOpcode::Binary,
                payload: data.to_vec(),
            },
            Some(seq.to_le_bytes()),
        );
        self.tx.send(frame).map_err(|_| TransportError::Closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LoopbackHub, MultiTransport, PeerKind};

    fn http_get(addr: SocketAddr, path: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n");
        s.write_all(req.as_bytes()).unwrap();
        s.flush().unwrap();
        let mut buf = String::new();
        s.read_to_string(&mut buf).unwrap();
        buf
    }

    #[test]
    fn ws_server_serves_static_http_and_exchanges_binary_frames() {
        let dir = std::env::temp_dir().join(format!("cs16-ws-static-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), b"<html><canvas id=\"cs16-canvas\"></canvas></html>")
            .unwrap();

        let mut srv = WsServerTransport::bind(WsServerConfig {
            bind_addr: "127.0.0.1:0".into(),
            static_dir: Some(dir.clone()),
            ws_path: "/ws".into(),
            max_clients: 8,
            upgrade_hook: None,
            http_hook: None,
        })
        .unwrap();
        let addr = srv.local_addr();

        let resp_ok = http_get(addr, "/");
        assert!(resp_ok.starts_with("HTTP/1.1 200 OK"), "{resp_ok}");
        assert!(resp_ok.contains("cs16-canvas"), "{resp_ok}");

        let resp_trav = http_get(addr, "/../secret.txt");
        assert!(resp_trav.starts_with("HTTP/1.1 404"), "{resp_trav}");

        let mut client = WsClientTransport::connect(&format!("ws://{addr}/ws")).unwrap();
        client.send(PeerAddr::Web(0), b"ping-proto48").unwrap();

        let mut from_client = None;
        for _ in 0..100 {
            if let Some(pkt) = srv.recv() {
                from_client = Some(pkt);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (peer, payload) = from_client.expect("server receives client binary frame");
        assert_eq!(payload, b"ping-proto48");

        srv.send(peer, b"pong-proto48").unwrap();
        let mut from_srv = None;
        for _ in 0..100 {
            if let Some(pkt) = client.recv() {
                from_srv = Some(pkt);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            from_srv,
            Some((PeerAddr::Web(0), b"pong-proto48".to_vec()))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn multi_transport_routes_udp_and_ws_peers() {
        let hub = LoopbackHub::new();
        let mut ep = hub.connect();
        let ep_peer = PeerAddr::Loopback(ep.id());

        let ws_srv = WsServerTransport::bind(WsServerConfig {
            bind_addr: "127.0.0.1:0".into(),
            static_dir: None,
            ws_path: "/ws".into(),
            max_clients: 4,
            upgrade_hook: None,
            http_hook: None,
        })
        .unwrap();
        let addr = ws_srv.local_addr();

        let mut multi = MultiTransport::new();
        multi.add(PeerKind::Loopback, hub);
        multi.add(PeerKind::Web, ws_srv);

        let mut ws_client = WsClientTransport::connect(&format!("ws://{addr}/ws")).unwrap();
        ep.send(PeerAddr::Loopback(0), b"lb-hello").unwrap();
        ws_client.send(PeerAddr::Web(0), b"ws-hello").unwrap();

        let mut got = Vec::new();
        for _ in 0..100 {
            while let Some(pkt) = multi.recv() {
                got.push(pkt);
            }
            if got.len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got.len(), 2, "{got:?}");

        let ws_peer = got
            .iter()
            .find(|(_, d)| d == b"ws-hello")
            .map(|(p, _)| *p)
            .unwrap();
        multi.send(ep_peer, b"lb-reply").unwrap();
        multi.send(ws_peer, b"ws-reply").unwrap();

        assert_eq!(ep.recv(), Some((PeerAddr::Loopback(0), b"lb-reply".to_vec())));
        let mut ws_got = None;
        for _ in 0..100 {
            if let Some(pkt) = ws_client.recv() {
                ws_got = Some(pkt);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(ws_got, Some((PeerAddr::Web(0), b"ws-reply".to_vec())));
    }
}
