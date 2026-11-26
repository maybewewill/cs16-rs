//! Native-only HTTP/3 WebTransport datagram server and client transports (`wtransport` + `ring`).
//!
//! Zero `unsafe` code; pure-Rust QUIC + TLS 1.3 via `ring` (no `aws-lc-rs` / `aws-lc-sys`).
//! Designed for simultaneous multiplexing with UDP and RFC 6455 WebSocket inside
//! [`crate::MultiTransport`] using distinct [`PeerAddr::WebTransport`] peer IDs.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc as tokio_mpsc, watch};
use wtransport::config::QuicTransportConfig;
use wtransport::error::SendDatagramError;
use wtransport::tls::client::{ServerHashVerification, build_default_tls_config};
use wtransport::tls::rustls::RootCertStore;
use wtransport::tls::{Sha256Digest, build_native_cert_store};
use wtransport::{ClientConfig, Connection, Endpoint, Identity, ServerConfig, VarInt};

use crate::ws_proto::base64_encode;
use crate::{MAX_UDP_PACKET, PacketTransport, PeerAddr, TransportError, check_len};

const MAX_INBOUND_QUEUE: usize = 1024;
const PEER_OUTBOUND_QUEUE: usize = 256;
const QUIC_DATAGRAM_BUFFER_BYTES: usize = 256 * 1024;

/// Ephemeral self-signed ECDSA P-256 certificate lifetime in days.
///
/// W3C WebTransport `serverCertificateHashes` requires a validity window `<= 14` days;
/// `13` days stays strictly inside the browser limit even across minor clock skew.
pub const SELF_SIGNED_VALIDITY_DAYS: u32 = 13;

/// Default SANs embedded in generated ephemeral WebTransport certificates.
pub const DEFAULT_SELF_SIGNED_SANS: &[&str] = &["localhost", "127.0.0.1", "::1"];

/// SHA-256 digest of a leaf TLS X.509 certificate's DER encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WtCertDigest([u8; 32]);

impl WtCertDigest {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Standard Base64 encoding of the 32-byte SHA-256 digest (for browser `serverCertificateHashes`).
    pub fn to_base64(&self) -> String {
        base64_encode(&self.0)
    }

    /// Colon-delimited lowercase hex representation (`aa:bb:cc:...`).
    pub fn to_colon_hex(&self) -> String {
        let mut out = String::with_capacity(32 * 3 - 1);
        for (idx, b) in self.0.iter().enumerate() {
            if idx > 0 {
                out.push(':');
            }
            let _ = fmt::Write::write_fmt(&mut out, format_args!("{b:02x}"));
        }
        out
    }

    /// Plain 64-character lowercase hex representation.
    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for b in self.0 {
            let _ = fmt::Write::write_fmt(&mut out, format_args!("{b:02x}"));
        }
        out
    }

    pub fn to_wtransport(&self) -> Sha256Digest {
        Sha256Digest::new(self.0)
    }
}

impl fmt::Display for WtCertDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_colon_hex())
    }
}

/// Configuration for [`WtServerTransport`].
#[derive(Debug, Clone)]
pub struct WtServerConfig {
    pub bind_addr: String,
    pub wt_path: String,
    pub max_clients: usize,
    pub cert_path: Option<PathBuf>,
    pub key_path: Option<PathBuf>,
    /// Extra SANs appended to [`DEFAULT_SELF_SIGNED_SANS`] when generating an ephemeral cert.
    pub extra_sans: Vec<String>,
    /// Sustained per-peer inbound datagram rate limit (datagrams/sec; `0` disables).
    pub max_datagrams_per_sec: u32,
    /// Burst capacity for the per-peer token bucket.
    pub max_datagram_burst: u32,
    pub max_idle_timeout: Duration,
    pub keep_alive_interval: Option<Duration>,
}

impl Default for WtServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:27017".to_string(),
            wt_path: "/wt".to_string(),
            max_clients: 64,
            cert_path: None,
            key_path: None,
            extra_sans: Vec::new(),
            max_datagrams_per_sec: 500,
            max_datagram_burst: 250,
            max_idle_timeout: Duration::from_secs(30),
            keep_alive_interval: Some(Duration::from_secs(5)),
        }
    }
}

/// Runtime counters exposed by [`WtServerTransport::stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WtServerStats {
    pub active_peers: usize,
    pub dropped_oversized: u64,
    pub dropped_empty: u64,
    pub dropped_rate_limited: u64,
    pub dropped_send_mtu: u64,
}

#[derive(Default)]
struct SharedStats {
    active_peers: AtomicUsize,
    dropped_oversized: AtomicU64,
    dropped_empty: AtomicU64,
    dropped_rate_limited: AtomicU64,
    dropped_send_mtu: AtomicU64,
}

impl SharedStats {
    fn snapshot(&self) -> WtServerStats {
        WtServerStats {
            active_peers: self.active_peers.load(Ordering::Relaxed),
            dropped_oversized: self.dropped_oversized.load(Ordering::Relaxed),
            dropped_empty: self.dropped_empty.load(Ordering::Relaxed),
            dropped_rate_limited: self.dropped_rate_limited.load(Ordering::Relaxed),
            dropped_send_mtu: self.dropped_send_mtu.load(Ordering::Relaxed),
        }
    }
}

struct PeerRateLimiter {
    rate_per_sec: f64,
    capacity: f64,
    tokens: f64,
    last_refill: Instant,
}

impl PeerRateLimiter {
    fn new(rate_per_sec: u32, burst: u32) -> Self {
        let capacity = f64::from(burst.max(1));
        Self {
            rate_per_sec: f64::from(rate_per_sec),
            capacity,
            tokens: capacity,
            last_refill: Instant::now(),
        }
    }

    fn allow(&mut self) -> bool {
        if self.rate_per_sec <= 0.0 {
            return true;
        }
        let now = Instant::now();
        let elapsed = now
            .saturating_duration_since(self.last_refill)
            .as_secs_f64();
        if elapsed > 0.0 {
            self.tokens = (self.tokens + elapsed * self.rate_per_sec).min(self.capacity);
            self.last_refill = now;
        }
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

type PeerSenderMap = Arc<Mutex<HashMap<PeerAddr, tokio_mpsc::Sender<Vec<u8>>>>>;
type PacketQueue = Arc<Mutex<VecDeque<(PeerAddr, Vec<u8>)>>>;

struct BoundServerInfo {
    local_addr: SocketAddr,
    cert_sha256: WtCertDigest,
    cert_der: Vec<u8>,
    self_signed: bool,
}

/// Native HTTP/3 WebTransport datagram server emitting [`PeerAddr::WebTransport`] peers.
pub struct WtServerTransport {
    local_addr: SocketAddr,
    cert_sha256: WtCertDigest,
    cert_der: Vec<u8>,
    self_signed: bool,
    running: Arc<AtomicBool>,
    inbound: PacketQueue,
    peers: PeerSenderMap,
    stats: Arc<SharedStats>,
    shutdown_tx: watch::Sender<bool>,
    worker: Option<JoinHandle<()>>,
}

impl WtServerTransport {
    /// Binds a QUIC/HTTP/3 WebTransport server according to `cfg`.
    ///
    /// When `cfg.cert_path` and `cfg.key_path` are omitted, an ephemeral ECDSA P-256
    /// certificate valid for [`SELF_SIGNED_VALIDITY_DAYS`] (13 days, `<= 14` days per the
    /// W3C WebTransport `serverCertificateHashes` specification) is generated in memory.
    pub fn bind(cfg: WtServerConfig) -> io::Result<Self> {
        if cfg.cert_path.is_some() != cfg.key_path.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "both cert_path and key_path must be specified together",
            ));
        }
        let bind_addr = resolve_bind_addr(&cfg.bind_addr)?;

        let running = Arc::new(AtomicBool::new(true));
        let inbound: PacketQueue = Arc::new(Mutex::new(VecDeque::new()));
        let peers: PeerSenderMap = Arc::new(Mutex::new(HashMap::new()));
        let stats = Arc::new(SharedStats::default());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (ready_tx, ready_rx) = std_mpsc::sync_channel::<io::Result<BoundServerInfo>>(1);

        let r_flag = Arc::clone(&running);
        let q = Arc::clone(&inbound);
        let p_map = Arc::clone(&peers);
        let s_stats = Arc::clone(&stats);

        let worker = std::thread::Builder::new()
            .name("cs-wt-server".to_string())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                rt.block_on(run_server_loop(
                    bind_addr,
                    cfg,
                    r_flag,
                    q,
                    p_map,
                    s_stats,
                    shutdown_rx,
                    ready_tx,
                ));
            })?;

        match ready_rx.recv() {
            Ok(Ok(info)) => Ok(Self {
                local_addr: info.local_addr,
                cert_sha256: info.cert_sha256,
                cert_der: info.cert_der,
                self_signed: info.self_signed,
                running,
                inbound,
                peers,
                stats,
                shutdown_tx,
                worker: Some(worker),
            }),
            Ok(Err(e)) => {
                let _ = worker.join();
                Err(e)
            }
            Err(_) => {
                let _ = worker.join();
                Err(io::Error::other(
                    "WebTransport server worker exited during startup",
                ))
            }
        }
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// SHA-256 digest of the active leaf TLS certificate's DER bytes.
    pub fn cert_sha256(&self) -> WtCertDigest {
        self.cert_sha256
    }

    /// Raw DER encoding of the active leaf X.509 certificate.
    pub fn cert_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// Returns `true` when using an ephemeral self-signed certificate (rather than PEM files).
    pub fn is_self_signed(&self) -> bool {
        self.self_signed
    }

    pub fn stats(&self) -> WtServerStats {
        self.stats.snapshot()
    }
}

impl Drop for WtServerTransport {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        let _ = self.shutdown_tx.send(true);
        if let Ok(mut map) = self.peers.lock() {
            map.clear();
        }
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

impl PacketTransport for WtServerTransport {
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
        match tx.try_send(data.to_vec()) {
            Ok(()) => Ok(()),
            // Unreliable datagram channel saturated: drop packet without stalling the tick loop.
            Err(tokio_mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
                if let Ok(mut map) = self.peers.lock() {
                    map.remove(&to);
                }
                Err(TransportError::Closed)
            }
        }
    }
}

fn resolve_bind_addr(addr: &str) -> io::Result<SocketAddr> {
    addr.to_socket_addrs()?.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("cannot resolve {addr}"),
        )
    })
}

async fn load_or_generate_identity(
    cfg: &WtServerConfig,
) -> io::Result<(Identity, WtCertDigest, Vec<u8>, bool)> {
    let (identity, self_signed) = match (&cfg.cert_path, &cfg.key_path) {
        (Some(cert), Some(key)) => {
            let id = Identity::load_pemfiles(cert, key).await.map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("failed to load WebTransport TLS PEM files: {e}"),
                )
            })?;
            (id, false)
        }
        (None, None) => {
            let mut sans: Vec<String> = DEFAULT_SELF_SIGNED_SANS
                .iter()
                .map(|s| (*s).to_string())
                .collect();
            for san in &cfg.extra_sans {
                let trimmed = san.trim();
                if !trimmed.is_empty() && !sans.iter().any(|s| s == trimmed) {
                    sans.push(trimmed.to_string());
                }
            }
            let not_before = wtransport::tls::self_signed::time::OffsetDateTime::now_utc()
                - wtransport::tls::self_signed::time::Duration::minutes(1);
            let id = Identity::self_signed_builder()
                .subject_alt_names(sans)
                .not_before(not_before)
                .validity_days(SELF_SIGNED_VALIDITY_DAYS)
                .build()
                .map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("failed to generate self-signed WebTransport certificate: {e}"),
                    )
                })?;
            (id, true)
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "both cert_path and key_path must be specified together",
            ));
        }
    };

    let leaf = identity
        .certificate_chain()
        .as_slice()
        .first()
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "WebTransport certificate chain is empty",
            )
        })?;
    let digest = WtCertDigest::from_bytes(*leaf.hash().as_ref());
    let cert_der = leaf.der().to_vec();
    Ok((identity, digest, cert_der, self_signed))
}

#[allow(clippy::too_many_arguments)]
async fn run_server_loop(
    bind_addr: SocketAddr,
    cfg: WtServerConfig,
    running: Arc<AtomicBool>,
    inbound: PacketQueue,
    peers: PeerSenderMap,
    stats: Arc<SharedStats>,
    mut shutdown_rx: watch::Receiver<bool>,
    ready_tx: std_mpsc::SyncSender<io::Result<BoundServerInfo>>,
) {
    let (identity, cert_sha256, cert_der, self_signed) = match load_or_generate_identity(&cfg).await
    {
        Ok(v) => v,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    let mut quic_transport = QuicTransportConfig::default();
    quic_transport.datagram_receive_buffer_size(Some(QUIC_DATAGRAM_BUFFER_BYTES));
    quic_transport.datagram_send_buffer_size(QUIC_DATAGRAM_BUFFER_BYTES);

    let builder = ServerConfig::builder()
        .with_bind_address(bind_addr)
        .with_custom_transport(identity, quic_transport)
        .keep_alive_interval(cfg.keep_alive_interval);
    let mut server_config = match builder.max_idle_timeout(Some(cfg.max_idle_timeout)) {
        Ok(b) => b.build(),
        Err(_) => {
            let _ = ready_tx.send(Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid WebTransport max_idle_timeout",
            )));
            return;
        }
    };
    // Allow receiving QUIC UDP payloads above MAX_UDP_PACKET on jumbo/loopback paths so the
    // application-level MAX_UDP_PACKET guard explicitly drops oversized datagrams without
    // stalling the session (outbound MTU still starts at RFC 9000's 1200 bytes).
    let _ = server_config
        .quic_endpoint_config_mut()
        .max_udp_payload_size(8192);

    let endpoint = match Endpoint::server(server_config) {
        Ok(ep) => ep,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };
    let local_addr = match endpoint.local_addr() {
        Ok(addr) => addr,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    if ready_tx
        .send(Ok(BoundServerInfo {
            local_addr,
            cert_sha256,
            cert_der,
            self_signed,
        }))
        .is_err()
    {
        return;
    }

    let next_id = Arc::new(AtomicU32::new(1));
    let wt_path = Arc::new(cfg.wt_path);
    let max_clients = cfg.max_clients;
    let max_rate = cfg.max_datagrams_per_sec;
    let max_burst = cfg.max_datagram_burst;

    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            incoming = endpoint.accept() => {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
                let r_flag = Arc::clone(&running);
                let q = Arc::clone(&inbound);
                let p_map = Arc::clone(&peers);
                let s_stats = Arc::clone(&stats);
                let id_gen = Arc::clone(&next_id);
                let path_cfg = Arc::clone(&wt_path);
                let peer_shutdown = shutdown_rx.clone();

                tokio::spawn(async move {
                    let Ok(session_request) = incoming.await else {
                        return;
                    };
                    let req_path = session_request
                        .path()
                        .split('?')
                        .next()
                        .unwrap_or("/");
                    let path_allowed = req_path == path_cfg.as_str()
                        || (path_cfg.as_str() == "/wt" && req_path == "/");
                    if !path_allowed {
                        session_request.not_found().await;
                        return;
                    }
                    if s_stats.active_peers.load(Ordering::Relaxed) >= max_clients {
                        session_request.too_many_requests().await;
                        return;
                    }
                    let Ok(connection) = session_request.accept().await else {
                        return;
                    };
                    let id = id_gen.fetch_add(1, Ordering::Relaxed);
                    let peer = PeerAddr::WebTransport(id);
                    s_stats.active_peers.fetch_add(1, Ordering::Relaxed);
                    pump_wt_server_peer(
                        connection,
                        peer,
                        r_flag,
                        q,
                        p_map,
                        Arc::clone(&s_stats),
                        peer_shutdown,
                        max_rate,
                        max_burst,
                    )
                    .await;
                    s_stats.active_peers.fetch_sub(1, Ordering::Relaxed);
                });
            }
        }
    }

    endpoint.close(VarInt::from_u32(0), b"shutdown");
}

#[allow(clippy::too_many_arguments)]
async fn pump_wt_server_peer(
    connection: Connection,
    peer: PeerAddr,
    running: Arc<AtomicBool>,
    inbound: PacketQueue,
    peers: PeerSenderMap,
    stats: Arc<SharedStats>,
    mut shutdown_rx: watch::Receiver<bool>,
    max_datagrams_per_sec: u32,
    max_datagram_burst: u32,
) {
    let (tx, mut rx) = tokio_mpsc::channel::<Vec<u8>>(PEER_OUTBOUND_QUEUE);
    if let Ok(mut map) = peers.lock() {
        map.insert(peer, tx);
    }

    let mut limiter = PeerRateLimiter::new(max_datagrams_per_sec, max_datagram_burst);

    loop {
        if !running.load(Ordering::Relaxed) {
            break;
        }
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            maybe_out = rx.recv() => {
                let Some(payload) = maybe_out else {
                    break;
                };
                match connection.send_datagram(&payload) {
                    Ok(()) => {}
                    Err(SendDatagramError::TooLarge) => {
                        // Datagram exceeded current QUIC path MTU; drop this unreliable
                        // frame without terminating the peer session.
                        stats.dropped_send_mtu.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
            recv_res = connection.receive_datagram() => {
                match recv_res {
                    Ok(dgram) => {
                        let bytes = dgram.payload();
                        if bytes.is_empty() {
                            stats.dropped_empty.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        if bytes.len() > MAX_UDP_PACKET {
                            stats.dropped_oversized.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        if !limiter.allow() {
                            stats.dropped_rate_limited.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        if let Ok(mut q) = inbound.lock() {
                            if q.len() >= MAX_INBOUND_QUEUE {
                                q.pop_front();
                            }
                            q.push_back((peer, bytes.to_vec()));
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }

    if let Ok(mut map) = peers.lock() {
        map.remove(&peer);
    }
    connection.close(VarInt::from_u32(0), b"bye");
}

/// Native HTTP/3 WebTransport datagram client implementing [`PacketTransport`]
/// (remote peer [`PeerAddr::WebTransport(0)`]).
pub struct WtClientTransport {
    remote: PeerAddr,
    running: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    inbound: PacketQueue,
    outbound: tokio_mpsc::Sender<Vec<u8>>,
    shutdown_tx: watch::Sender<bool>,
    worker: Option<JoinHandle<()>>,
}

impl WtClientTransport {
    /// Connects to a WebTransport endpoint verifying the server certificate by its exact
    /// SHA-256 DER digest (matching W3C WebTransport `serverCertificateHashes` semantics:
    /// ECDSA P-256 and validity `<= 14` days).
    pub fn connect_with_cert_hash(
        url: &str,
        cert_sha256: WtCertDigest,
    ) -> Result<Self, TransportError> {
        Self::connect_with_options(url, Some(cert_sha256), None)
    }

    /// Connects to a WebTransport endpoint using the platform's native root CA certificates.
    pub fn connect_with_native_certs(url: &str) -> Result<Self, TransportError> {
        Self::connect_with_options(url, None, None)
    }

    /// Connects with optional certificate hash pinning and optional initial QUIC MTU override
    /// (useful on loopback tests to probe oversized `> MAX_UDP_PACKET` rejection).
    pub fn connect_with_options(
        url: &str,
        cert_sha256: Option<WtCertDigest>,
        initial_mtu: Option<u16>,
    ) -> Result<Self, TransportError> {
        let url_owned = url.to_string();
        let running = Arc::new(AtomicBool::new(true));
        let closed = Arc::new(AtomicBool::new(false));
        let inbound: PacketQueue = Arc::new(Mutex::new(VecDeque::new()));
        let (outbound_tx, outbound_rx) = tokio_mpsc::channel::<Vec<u8>>(PEER_OUTBOUND_QUEUE);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (ready_tx, ready_rx) = std_mpsc::sync_channel::<io::Result<()>>(1);

        let r_flag = Arc::clone(&running);
        let c_flag = Arc::clone(&closed);
        let q = Arc::clone(&inbound);

        let worker = std::thread::Builder::new()
            .name("cs-wt-client".to_string())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                rt.block_on(run_client_loop(
                    url_owned,
                    cert_sha256,
                    initial_mtu,
                    r_flag,
                    c_flag,
                    q,
                    outbound_rx,
                    shutdown_rx,
                    ready_tx,
                ));
            })?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                remote: PeerAddr::WebTransport(0),
                running,
                closed,
                inbound,
                outbound: outbound_tx,
                shutdown_tx,
                worker: Some(worker),
            }),
            Ok(Err(e)) => {
                let _ = worker.join();
                Err(TransportError::Io(e))
            }
            Err(_) => {
                let _ = worker.join();
                Err(TransportError::Closed)
            }
        }
    }

    /// Sends raw datagram bytes without enforcing [`MAX_UDP_PACKET`], allowing tests to verify
    /// that servers reject oversized HTTP/3 datagrams cleanly over the wire.
    pub fn send_raw_unchecked(&mut self, data: &[u8]) -> Result<(), TransportError> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(TransportError::Closed);
        }
        match self.outbound.try_send(data.to_vec()) {
            Ok(()) => Ok(()),
            Err(tokio_mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(tokio_mpsc::error::TrySendError::Closed(_)) => Err(TransportError::Closed),
        }
    }
}

impl Drop for WtClientTransport {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        let _ = self.shutdown_tx.send(true);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

impl PacketTransport for WtClientTransport {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        self.inbound.lock().ok()?.pop_front()
    }

    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), TransportError> {
        check_len(data)?;
        if self.closed.load(Ordering::Relaxed) || to != self.remote {
            return Err(TransportError::Closed);
        }
        match self.outbound.try_send(data.to_vec()) {
            Ok(()) => Ok(()),
            Err(tokio_mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(tokio_mpsc::error::TrySendError::Closed(_)) => Err(TransportError::Closed),
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_client_loop(
    url: String,
    cert_sha256: Option<WtCertDigest>,
    initial_mtu: Option<u16>,
    running: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    inbound: PacketQueue,
    mut outbound_rx: tokio_mpsc::Receiver<Vec<u8>>,
    mut shutdown_rx: watch::Receiver<bool>,
    ready_tx: std_mpsc::SyncSender<io::Result<()>>,
) {
    let mut quic_transport = QuicTransportConfig::default();
    quic_transport.datagram_receive_buffer_size(Some(QUIC_DATAGRAM_BUFFER_BYTES));
    quic_transport.datagram_send_buffer_size(QUIC_DATAGRAM_BUFFER_BYTES);
    if let Some(mtu) = initial_mtu {
        quic_transport.initial_mtu(mtu.max(1200));
        quic_transport.mtu_discovery_config(None);
    }

    let tls_config = match cert_sha256 {
        Some(digest) => build_default_tls_config(
            Arc::new(RootCertStore::empty()),
            Some(Arc::new(ServerHashVerification::new([
                digest.to_wtransport()
            ]))),
        ),
        None => build_default_tls_config(Arc::new(build_native_cert_store()), None),
    };

    let mut client_config = ClientConfig::builder()
        .with_bind_default()
        .with_custom_tls_and_transport(tls_config, quic_transport)
        .keep_alive_interval(Some(Duration::from_secs(5)))
        .build();
    if let Some(mtu) = initial_mtu {
        let _ = client_config
            .quic_endpoint_config_mut()
            .max_udp_payload_size(mtu.max(1200));
    }

    let endpoint = match Endpoint::client(client_config) {
        Ok(ep) => ep,
        Err(e) => {
            closed.store(true, Ordering::Relaxed);
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    let connection = match endpoint.connect(&url).await {
        Ok(conn) => conn,
        Err(e) => {
            closed.store(true, Ordering::Relaxed);
            let _ = ready_tx.send(Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("WebTransport connect failed: {e}"),
            )));
            return;
        }
    };

    if ready_tx.send(Ok(())).is_err() {
        connection.close(VarInt::from_u32(0), b"cancelled");
        return;
    }

    loop {
        if !running.load(Ordering::Relaxed) {
            break;
        }
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    break;
                }
            }
            maybe_out = outbound_rx.recv() => {
                let Some(payload) = maybe_out else {
                    break;
                };
                match connection.send_datagram(&payload) {
                    Ok(()) | Err(SendDatagramError::TooLarge) => {}
                    Err(_) => {
                        break;
                    }
                }
            }
            recv_res = connection.receive_datagram() => {
                match recv_res {
                    Ok(dgram) => {
                        let bytes = dgram.payload();
                        if bytes.is_empty() || bytes.len() > MAX_UDP_PACKET {
                            continue;
                        }
                        if let Ok(mut q) = inbound.lock() {
                            if q.len() >= MAX_INBOUND_QUEUE {
                                q.pop_front();
                            }
                            q.push_back((PeerAddr::WebTransport(0), bytes.to_vec()));
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }

    closed.store(true, Ordering::Relaxed);
    connection.close(VarInt::from_u32(0), b"bye");
    endpoint.close(VarInt::from_u32(0), b"shutdown");
}
