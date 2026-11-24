//! Bounded in-memory bidirectional packet channel for bridging browser JS sockets (`cs-web`)
//! into `cs_clientcore::Connection<ChannelTransport>`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::{PacketTransport, PeerAddr, TransportError, check_len};

const MAX_CHANNEL_QUEUE: usize = 512;

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
}
