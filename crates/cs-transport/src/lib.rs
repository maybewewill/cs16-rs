//! Packet transport seam: every component exchanges raw datagrams through
//! [`PacketTransport`], so the same code runs over UDP, an in-process loopback,
//! a browser channel, RFC 6455 WebSocket, or HTTP/3 WebTransport datagrams.

use std::fmt;

mod channel;
mod loopback;
mod multi;
#[cfg(not(target_arch = "wasm32"))]
mod udp;
#[cfg(not(target_arch = "wasm32"))]
pub mod ws_native;
pub mod ws_proto;
#[cfg(not(target_arch = "wasm32"))]
pub mod wt_native;

pub use channel::{
    ChannelHandle, ChannelTransport, MAX_MULTI_PEER_SLOTS, MultiPeerChannel, MultiPeerHandle,
};
pub use loopback::{FaultStats, Faults, LoopbackEndpoint, LoopbackHub};
pub use multi::{MultiTransport, PeerKind};
#[cfg(not(target_arch = "wasm32"))]
pub use udp::UdpTransport;
#[cfg(not(target_arch = "wasm32"))]
pub use ws_native::{
    HttpHook, UpgradeHook, WsClientTransport, WsServerConfig, WsServerTransport,
};
#[cfg(not(target_arch = "wasm32"))]
pub use wt_native::{
    WtCertDigest, WtClientTransport, WtServerConfig, WtServerTransport,
};

/// Largest datagram a transport accepts for sending (`ReHLDS rehlds/engine/net.h:117`).
pub const MAX_UDP_PACKET: usize = 4010;

/// Address of a remote peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PeerAddr {
    Udp(std::net::SocketAddr),
    /// Loopback peer; `0` is the hub (server), endpoints are numbered from `1`.
    Loopback(u32),
    /// Browser / WebSocket / WebRTC peer (`0` for the remote server from a client).
    Web(u32),
    /// HTTP/3 WebTransport datagram peer (`0` for the remote server from a client).
    WebTransport(u32),
}

impl fmt::Display for PeerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeerAddr::Udp(addr) => write!(f, "{addr}"),
            PeerAddr::Loopback(id) => write!(f, "loopback:{id}"),
            PeerAddr::Web(id) => write!(f, "web:{id}"),
            PeerAddr::WebTransport(id) => write!(f, "wt:{id}"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("transport I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("datagram of {0} bytes exceeds MAX_UDP_PACKET")]
    TooLarge(usize),
    #[error("peer unreachable or transport closed")]
    Closed,
}

/// A datagram transport. Both methods are non-blocking.
pub trait PacketTransport {
    /// Next waiting datagram and its sender, or `None` if nothing is waiting.
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)>;
    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), TransportError>;
}

/// Rejects datagrams longer than [`MAX_UDP_PACKET`].
fn check_len(data: &[u8]) -> Result<(), TransportError> {
    if data.len() > MAX_UDP_PACKET {
        Err(TransportError::TooLarge(data.len()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_addr_display() {
        let udp: std::net::SocketAddr = "127.0.0.1:27015".parse().unwrap();
        assert_eq!(PeerAddr::Udp(udp).to_string(), "127.0.0.1:27015");
        assert_eq!(PeerAddr::Loopback(3).to_string(), "loopback:3");
        assert_eq!(PeerAddr::Web(4).to_string(), "web:4");
        assert_eq!(PeerAddr::WebTransport(5).to_string(), "wt:5");
    }
}
