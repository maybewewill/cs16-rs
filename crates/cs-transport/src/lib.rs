//! Packet transport seam: every component exchanges raw datagrams through
//! [`PacketTransport`], so the same code runs over UDP, an in-process loopback,
//! a browser channel, or RFC 6455 WebSocket.

use std::fmt;

mod channel;
mod loopback;
mod multi;
#[cfg(not(target_arch = "wasm32"))]
mod udp;
#[cfg(not(target_arch = "wasm32"))]
pub mod ws_native;
pub mod ws_proto;

pub use channel::{ChannelHandle, ChannelTransport};
pub use loopback::{FaultStats, Faults, LoopbackEndpoint, LoopbackHub};
pub use multi::{MultiTransport, PeerKind};
#[cfg(not(target_arch = "wasm32"))]
pub use udp::UdpTransport;
#[cfg(not(target_arch = "wasm32"))]
pub use ws_native::{
    HttpHook, UpgradeHook, WsClientTransport, WsServerConfig, WsServerTransport,
};

/// Largest datagram a transport accepts for sending (`ReHLDS rehlds/engine/net.h:117`).
pub const MAX_UDP_PACKET: usize = 4010;

/// Address of a remote peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PeerAddr {
    Udp(std::net::SocketAddr),
    /// Loopback peer; `0` is the hub (server), endpoints are numbered from `1`.
    Loopback(u32),
    /// Browser / WebSocket / WebTransport peer (`0` for the remote server from a client).
    Web(u32),
}

impl fmt::Display for PeerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeerAddr::Udp(addr) => write!(f, "{addr}"),
            PeerAddr::Loopback(id) => write!(f, "loopback:{id}"),
            PeerAddr::Web(id) => write!(f, "web:{id}"),
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
    }
}
