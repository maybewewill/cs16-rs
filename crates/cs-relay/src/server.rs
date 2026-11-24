//! Native HTTP/WebSocket-to-UDP relay server and reusable upgrade handler.

use std::io::{self, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use cs_transport::MAX_UDP_PACKET;
use cs_transport::ws_native::{complete_ws_handshake, send_http_response};
use cs_transport::ws_proto::{
    HttpRequest, WsFrame, WsOpcode, decode_frame, encode_frame, read_http_request,
};

use crate::policy::{
    RelayPolicy, RelayReject, SessionTracker, TokenBucket, parse_relay_target,
};
use crate::validate::{is_valid_c2s_protocol48, is_valid_s2c_protocol48};

const POLL_TIMEOUT: Duration = Duration::from_millis(25);

/// Reusable relay handler that can be mounted inside `cs-server --web-port` or [`RelayServer`].
#[derive(Clone)]
pub struct RelayHandle {
    policy: Arc<RelayPolicy>,
    tracker: Arc<SessionTracker>,
    running: Arc<AtomicBool>,
}

impl RelayHandle {
    pub fn new(policy: RelayPolicy) -> Self {
        Self {
            policy: Arc::new(policy),
            tracker: Arc::new(SessionTracker::default()),
            running: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn shutdown(&self) {
        self.running.store(false, Ordering::Relaxed);
    }

    pub fn active_sessions(&self) -> usize {
        self.tracker.active_total()
    }

    /// Validates target allowlist & caps, upgrades the WebSocket, and bridges Protocol 48 packets.
    pub fn handle_http_upgrade(
        &self,
        req: &HttpRequest,
        mut stream: TcpStream,
        peer_addr: SocketAddr,
    ) {
        if req.path != "/relay" {
            let _ = send_http_response(
                &mut stream,
                "404 Not Found",
                "text/plain; charset=utf-8",
                b"unknown relay endpoint",
                false,
            );
            return;
        }
        let Some(target) = parse_relay_target(req.query.as_deref()) else {
            let _ = send_http_response(
                &mut stream,
                "400 Bad Request",
                "text/plain; charset=utf-8",
                b"missing or invalid target=<ip:port>",
                false,
            );
            return;
        };
        if !self.policy.allowlist.is_allowed(&target) {
            warn!(%peer_addr, %target, "Relay target denied by allowlist");
            let _ = send_http_response(
                &mut stream,
                "403 Forbidden",
                "text/plain; charset=utf-8",
                b"target not in relay allowlist",
                false,
            );
            return;
        }
        let guard = match SessionTracker::try_acquire(&self.tracker, peer_addr.ip(), &self.policy) {
            Ok(g) => g,
            Err(RelayReject::TooManyForIp) => {
                let _ = send_http_response(
                    &mut stream,
                    "429 Too Many Requests",
                    "text/plain; charset=utf-8",
                    b"too many relay sessions from this IP",
                    false,
                );
                return;
            }
            Err(RelayReject::TooManyTotal) => {
                let _ = send_http_response(
                    &mut stream,
                    "503 Service Unavailable",
                    "text/plain; charset=utf-8",
                    b"relay session limit reached",
                    false,
                );
                return;
            }
        };
        let Some(key) = req.header("sec-websocket-key") else {
            return;
        };
        let bind_addr = if target.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let Ok(udp) = UdpSocket::bind(bind_addr) else {
            let _ = send_http_response(
                &mut stream,
                "500 Internal Server Error",
                "text/plain; charset=utf-8",
                b"failed to bind UDP socket",
                false,
            );
            return;
        };
        if udp.connect(target).is_err() {
            let _ = send_http_response(
                &mut stream,
                "502 Bad Gateway",
                "text/plain; charset=utf-8",
                b"failed to connect UDP target",
                false,
            );
            return;
        }
        if complete_ws_handshake(&mut stream, key).is_err() {
            return;
        }
        info!(%peer_addr, %target, "Started Protocol 48 WS->UDP relay session");
        bridge_ws_udp(stream, udp, &self.policy, &self.running);
        drop(guard);
    }
}

/// Standalone WebSocket-to-UDP relay server.
pub struct RelayServer {
    local_addr: SocketAddr,
    handle: RelayHandle,
    acceptor: Option<JoinHandle<()>>,
}

impl RelayServer {
    pub fn bind(listen_addr: &str, policy: RelayPolicy) -> io::Result<Self> {
        let listener = TcpListener::bind(listen_addr)?;
        listener.set_nonblocking(true)?;
        let local_addr = listener.local_addr()?;
        let handle = RelayHandle::new(policy);

        let h = handle.clone();
        let acceptor = std::thread::spawn(move || {
            while h.running.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, peer_addr)) => {
                        let h2 = h.clone();
                        std::thread::spawn(move || {
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_nodelay(true);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                            let Ok(req) = read_http_request(&mut stream) else {
                                return;
                            };
                            if !req.is_websocket_upgrade() {
                                let _ = send_http_response(
                                    &mut stream,
                                    "426 Upgrade Required",
                                    "text/plain; charset=utf-8",
                                    b"websocket upgrade required on /relay?target=ip:port",
                                    false,
                                );
                                return;
                            }
                            h2.handle_http_upgrade(&req, stream, peer_addr);
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
            handle,
            acceptor: Some(acceptor),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn active_sessions(&self) -> usize {
        self.handle.active_sessions()
    }
}

impl Drop for RelayServer {
    fn drop(&mut self) {
        self.handle.shutdown();
        if let Some(h) = self.acceptor.take() {
            let _ = h.join();
        }
    }
}

fn bridge_ws_udp(
    ws_stream: TcpStream,
    udp: UdpSocket,
    policy: &RelayPolicy,
    running: &Arc<AtomicBool>,
) {
    let Ok(mut ws_write) = ws_stream.try_clone() else {
        return;
    };
    let Ok(udp_rx) = udp.try_clone() else {
        return;
    };
    let _ = ws_stream.set_read_timeout(Some(POLL_TIMEOUT));
    let _ = udp_rx.set_read_timeout(Some(POLL_TIMEOUT));

    let session_alive = Arc::new(AtomicBool::new(true));
    let start = Instant::now();
    let last_activity = Arc::new(std::sync::Mutex::new(start));
    let (tx_ws, rx_ws) = mpsc::channel::<Vec<u8>>();

    // WS writer thread (takes pre-encoded RFC 6455 frames).
    let w_alive = Arc::clone(&session_alive);
    let w_run = Arc::clone(running);
    let ws_writer = std::thread::spawn(move || {
        while w_run.load(Ordering::Relaxed) && w_alive.load(Ordering::Relaxed) {
            match rx_ws.recv_timeout(POLL_TIMEOUT) {
                Ok(frame) => {
                    if ws_write.write_all(&frame).is_err() || ws_write.flush().is_err() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        w_alive.store(false, Ordering::Relaxed);
    });

    // UDP -> WS reader thread.
    let u_alive = Arc::clone(&session_alive);
    let u_run = Arc::clone(running);
    let u_tx = tx_ws.clone();
    let u_act = Arc::clone(&last_activity);
    let mut s2c_bucket = TokenBucket::from_policy(policy, 0.0);
    let udp_reader = std::thread::spawn(move || {
        let mut buf = [0u8; MAX_UDP_PACKET + 1];
        while u_run.load(Ordering::Relaxed) && u_alive.load(Ordering::Relaxed) {
            match udp_rx.recv(&mut buf) {
                Ok(n) => {
                    let pkt = &buf[..n];
                    let now = start.elapsed().as_secs_f64();
                    if is_valid_s2c_protocol48(pkt) && s2c_bucket.allow(n, now) {
                        if let Ok(mut t) = u_act.lock() {
                            *t = Instant::now();
                        }
                        let frame = encode_frame(
                            &WsFrame {
                                fin: true,
                                opcode: WsOpcode::Binary,
                                payload: pkt.to_vec(),
                            },
                            None,
                        );
                        if u_tx.send(frame).is_err() {
                            break;
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {
                    continue;
                }
                Err(_) => break,
            }
        }
        u_alive.store(false, Ordering::Relaxed);
    });

    // WS -> UDP loop on current thread.
    let mut reader = BufReader::new(ws_stream);
    let mut c2s_bucket = TokenBucket::from_policy(policy, 0.0);
    while running.load(Ordering::Relaxed) && session_alive.load(Ordering::Relaxed) {
        if let Ok(t) = last_activity.lock()
            && t.elapsed().as_secs_f64() > policy.idle_timeout_secs
        {
            break;
        }
        match decode_frame(&mut reader, MAX_UDP_PACKET, true) {
            Ok(frame) => match frame.opcode {
                WsOpcode::Binary => {
                    let now = start.elapsed().as_secs_f64();
                    if is_valid_c2s_protocol48(&frame.payload)
                        && c2s_bucket.allow(frame.payload.len(), now)
                    {
                        if let Ok(mut t) = last_activity.lock() {
                            *t = Instant::now();
                        }
                        if udp_send_bridge(&udp, &frame.payload).is_err() {
                            break;
                        }
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
                    if tx_ws.send(pong).is_err() {
                        break;
                    }
                }
                WsOpcode::Close => break,
                WsOpcode::Text | WsOpcode::Continuation | WsOpcode::Pong => {}
            },
            Err(cs_transport::ws_proto::WsProtoError::Io(e))
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => break,
        }
    }

    session_alive.store(false, Ordering::Relaxed);
    drop(tx_ws);
    let _ = udp_reader.join();
    let _ = ws_writer.join();
}

fn udp_send_bridge(_udp: &UdpSocket, _payload: &[u8]) -> io::Result<usize> {
    _udp.send(_payload)
}
