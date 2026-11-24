//! Multiplexes multiple [`PacketTransport`] backends (e.g. UDP + WebSocket) behind a single
//! `PacketTransport` instance by routing sends according to [`PeerAddr`] kind.

use crate::{PacketTransport, PeerAddr, TransportError};

/// Address family discriminator for routing [`MultiTransport::send`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerKind {
    Udp,
    Loopback,
    Web,
}

impl PeerKind {
    pub fn of(addr: &PeerAddr) -> Self {
        match addr {
            PeerAddr::Udp(_) => Self::Udp,
            PeerAddr::Loopback(_) => Self::Loopback,
            PeerAddr::Web(_) => Self::Web,
        }
    }
}

/// Polls registered transports round-robin in `recv` and routes `send` to matching peers.
pub struct MultiTransport {
    entries: Vec<(PeerKind, Box<dyn PacketTransport>)>,
    cursor: usize,
}

impl MultiTransport {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            cursor: 0,
        }
    }

    pub fn add<T: PacketTransport + 'static>(&mut self, kind: PeerKind, transport: T) {
        self.entries.push((kind, Box::new(transport)));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for MultiTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl PacketTransport for MultiTransport {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        let count = self.entries.len();
        if count == 0 {
            return None;
        }
        for offset in 0..count {
            let idx = (self.cursor + offset) % count;
            if let Some(pkt) = self.entries[idx].1.recv() {
                self.cursor = (idx + 1) % count;
                return Some(pkt);
            }
        }
        None
    }

    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), TransportError> {
        let want = PeerKind::of(&to);
        for (kind, transport) in &mut self.entries {
            if *kind == want {
                return transport.send(to, data);
            }
        }
        Err(TransportError::Closed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChannelTransport, LoopbackHub};

    #[test]
    fn multi_transport_multiplexes_loopback_and_web_channels() {
        let hub = LoopbackHub::new();
        let mut ep = hub.connect();
        let ep_addr = PeerAddr::Loopback(ep.id());

        let (chan, handle) = ChannelTransport::new_pair(PeerAddr::Web(7));

        let mut multi = MultiTransport::new();
        multi.add(PeerKind::Loopback, hub);
        multi.add(PeerKind::Web, chan);

        ep.send(PeerAddr::Loopback(0), b"from-loopback").unwrap();
        handle.push_inbound(b"from-web".to_vec()).unwrap();

        let mut seen = Vec::new();
        while let Some((from, data)) = multi.recv() {
            seen.push((from, data));
        }
        assert_eq!(seen.len(), 2);
        assert!(seen.contains(&(ep_addr, b"from-loopback".to_vec())));
        assert!(seen.contains(&(PeerAddr::Web(7), b"from-web".to_vec())));

        multi.send(ep_addr, b"to-loopback").unwrap();
        multi.send(PeerAddr::Web(7), b"to-web").unwrap();
        assert_eq!(
            ep.recv(),
            Some((PeerAddr::Loopback(0), b"to-loopback".to_vec()))
        );
        assert_eq!(handle.pop_outbound(), Some(b"to-web".to_vec()));
    }
}
