//! Bounded in-memory bidirectional packet channel for bridging browser JS sockets (`cs-web`)
//! into `cs_clientcore::Connection<ChannelTransport>`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::{PacketTransport, PeerAddr, TransportError, check_len};

const MAX_CHANNEL_QUEUE: usize = 512;
/// Maximum simultaneous external P2P peer slots (`PeerAddr::Web(1..=16)`).
pub const MAX_MULTI_PEER_SLOTS: u32 = 16;

#[derive(Debug)]
struct ChannelShared {
    remote: PeerAddr,
    inbound: VecDeque<Vec<u8>>,
    outbound: VecDeque<Vec<u8>>,
    closed: bool,
}

/// Transport end owned by `cs_clientcore::Connection` or a server peer.
#[derive(Debug, Clone)]
pub struct ChannelTransport {
    shared: Arc<Mutex<ChannelShared>>,
}

/// External pump handle owned by JS/WebAssembly glue or test harness.
#[derive(Debug, Clone)]
pub struct ChannelHandle {
    shared: Arc<Mutex<ChannelShared>>,
}

impl ChannelTransport {
    /// Creates a linked `(transport, handle)` pair reporting inbound packets as sent by `remote`.
    pub fn new_pair(remote: PeerAddr) -> (Self, ChannelHandle) {
        let shared = Arc::new(Mutex::new(ChannelShared {
            remote,
            inbound: VecDeque::new(),
            outbound: VecDeque::new(),
            closed: false,
        }));
        (
            Self {
                shared: Arc::clone(&shared),
            },
            ChannelHandle { shared },
        )
    }

    pub fn close(&self) {
        if let Ok(mut g) = self.shared.lock() {
            g.closed = true;
        }
    }
}

impl PacketTransport for ChannelTransport {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        let mut g = self.shared.lock().ok()?;
        let pkt = g.inbound.pop_front()?;
        Some((g.remote, pkt))
    }

    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), TransportError> {
        check_len(data)?;
        let mut g = self.shared.lock().map_err(|_| TransportError::Closed)?;
        if g.closed || to != g.remote {
            return Err(TransportError::Closed);
        }
        if g.outbound.len() >= MAX_CHANNEL_QUEUE {
            g.outbound.pop_front();
        }
        g.outbound.push_back(data.to_vec());
        Ok(())
    }
}

impl ChannelHandle {
    /// Queues a datagram received from the network for the transport's `recv()`.
    pub fn push_inbound(&self, data: Vec<u8>) -> Result<(), TransportError> {
        check_len(&data)?;
        let mut g = self.shared.lock().map_err(|_| TransportError::Closed)?;
        if g.closed {
            return Err(TransportError::Closed);
        }
        if g.inbound.len() >= MAX_CHANNEL_QUEUE {
            g.inbound.pop_front();
        }
        g.inbound.push_back(data);
        Ok(())
    }

    /// Pops one datagram queued by the transport's `send()`.
    pub fn pop_outbound(&self) -> Option<Vec<u8>> {
        self.shared.lock().ok()?.outbound.pop_front()
    }

    /// Drains all datagrams queued by the transport's `send()`.
    pub fn drain_outbound(&self) -> Vec<Vec<u8>> {
        let Ok(mut g) = self.shared.lock() else {
            return Vec::new();
        };
        g.outbound.drain(..).collect()
    }

    pub fn close(&self) {
        if let Ok(mut g) = self.shared.lock() {
            g.closed = true;
        }
    }

    pub fn is_closed(&self) -> bool {
        self.shared.lock().map_or(true, |g| g.closed)
    }
}

#[derive(Debug)]
struct MultiPeerShared {
    inbound: VecDeque<(u32, Vec<u8>)>,
    outbound: VecDeque<(u32, Vec<u8>)>,
    closed: bool,
}

/// Multi-peer datagram transport accepting external `PeerAddr::Web(1..=16)` clients.
#[derive(Debug, Clone)]
pub struct MultiPeerChannel {
    shared: Arc<Mutex<MultiPeerShared>>,
}

/// External pump handle for pushing/draining `(peer_id, bytes)` into [`MultiPeerChannel`].
#[derive(Debug, Clone)]
pub struct MultiPeerHandle {
    shared: Arc<Mutex<MultiPeerShared>>,
}

impl MultiPeerChannel {
    /// Creates a linked `(transport, handle)` pair multiplexing `PeerAddr::Web(1..=16)`.
    pub fn new_pair() -> (Self, MultiPeerHandle) {
        let shared = Arc::new(Mutex::new(MultiPeerShared {
            inbound: VecDeque::new(),
            outbound: VecDeque::new(),
            closed: false,
        }));
        (
            Self {
                shared: Arc::clone(&shared),
            },
            MultiPeerHandle { shared },
        )
    }

    pub fn close(&self) {
        if let Ok(mut g) = self.shared.lock() {
            g.closed = true;
        }
    }
}

impl PacketTransport for MultiPeerChannel {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        let mut g = self.shared.lock().ok()?;
        let (peer_id, pkt) = g.inbound.pop_front()?;
        Some((PeerAddr::Web(peer_id), pkt))
    }

    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), TransportError> {
        check_len(data)?;
        let PeerAddr::Web(peer_id) = to else {
            return Err(TransportError::Closed);
        };
        if !(1..=MAX_MULTI_PEER_SLOTS).contains(&peer_id) {
            return Err(TransportError::Closed);
        }
        let mut g = self.shared.lock().map_err(|_| TransportError::Closed)?;
        if g.closed {
            return Err(TransportError::Closed);
        }
        if g.outbound.len() >= MAX_CHANNEL_QUEUE {
            g.outbound.pop_front();
        }
        g.outbound.push_back((peer_id, data.to_vec()));
        Ok(())
    }
}

impl MultiPeerHandle {
    /// Queues an inbound packet from `PeerAddr::Web(peer_id)` (`1..=16`) for the server.
    pub fn push_inbound(&self, peer_id: u32, data: Vec<u8>) -> Result<(), TransportError> {
        if !(1..=MAX_MULTI_PEER_SLOTS).contains(&peer_id) {
            return Err(TransportError::Closed);
        }
        check_len(&data)?;
        let mut g = self.shared.lock().map_err(|_| TransportError::Closed)?;
        if g.closed {
            return Err(TransportError::Closed);
        }
        if g.inbound.len() >= MAX_CHANNEL_QUEUE {
            g.inbound.pop_front();
        }
        g.inbound.push_back((peer_id, data));
        Ok(())
    }

    /// Drains all outbound `(peer_id, bytes)` packets queued by the server.
    pub fn drain_outbound(&self) -> Vec<(u32, Vec<u8>)> {
        let Ok(mut g) = self.shared.lock() else {
            return Vec::new();
        };
        g.outbound.drain(..).collect()
    }

    pub fn close(&self) {
        if let Ok(mut g) = self.shared.lock() {
            g.closed = true;
        }
    }

    pub fn is_closed(&self) -> bool {
        self.shared.lock().map_or(true, |g| g.closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_UDP_PACKET;

    #[test]
    fn channel_transfers_packets_both_ways_and_enforces_limits() {
        let peer = PeerAddr::Web(0);
        let (mut tx, handle) = ChannelTransport::new_pair(peer);

        handle.push_inbound(b"hello-client".to_vec()).unwrap();
        assert_eq!(tx.recv(), Some((peer, b"hello-client".to_vec())));
        assert!(tx.recv().is_none());

        tx.send(peer, b"hello-server").unwrap();
        assert_eq!(handle.drain_outbound(), vec![b"hello-server".to_vec()]);

        let oversized = vec![0u8; MAX_UDP_PACKET + 1];
        assert!(matches!(
            tx.send(peer, &oversized),
            Err(TransportError::TooLarge(_))
        ));
        assert!(matches!(
            tx.send(PeerAddr::Loopback(0), b"x"),
            Err(TransportError::Closed)
        ));

        handle.close();
        assert!(handle.is_closed());
        assert!(matches!(tx.send(peer, b"x"), Err(TransportError::Closed)));
    }

    #[test]
    fn multi_peer_channel_routes_web_peers_and_enforces_bounds() {
        let (mut tx, handle) = MultiPeerChannel::new_pair();
        handle.push_inbound(1, b"p1".to_vec()).unwrap();
        handle.push_inbound(2, b"p2".to_vec()).unwrap();
        assert!(matches!(
            handle.push_inbound(0, b"bad".to_vec()),
            Err(TransportError::Closed)
        ));
        assert!(matches!(
            handle.push_inbound(MAX_MULTI_PEER_SLOTS + 1, b"bad".to_vec()),
            Err(TransportError::Closed)
        ));

        assert_eq!(tx.recv(), Some((PeerAddr::Web(1), b"p1".to_vec())));
        assert_eq!(tx.recv(), Some((PeerAddr::Web(2), b"p2".to_vec())));
        assert_eq!(tx.recv(), None);

        tx.send(PeerAddr::Web(2), b"reply2").unwrap();
        tx.send(PeerAddr::Web(1), b"reply1").unwrap();
        assert!(matches!(
            tx.send(PeerAddr::Loopback(1), b"x"),
            Err(TransportError::Closed)
        ));
        assert_eq!(
            handle.drain_outbound(),
            vec![(2, b"reply2".to_vec()), (1, b"reply1".to_vec())]
        );
    }
}
