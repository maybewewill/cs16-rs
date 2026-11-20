//! In-process loopback transport with fault injection.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::{PacketTransport, PeerAddr, TransportError, check_len};

/// Fault injection for one endpoint, applied to both of its directions.
/// Each field is a period counted over datagrams offered in a direction; `0` disables it.
#[derive(Default, Clone, Copy)]
pub struct Faults {
    pub drop_every: u32,
    pub duplicate_every: u32,
    pub swap_every: u32,
}

/// Fault state of one direction (endpoint → hub or hub → endpoint).
#[derive(Default)]
struct Direction {
    /// Datagrams offered so far, including dropped ones.
    offered: u64,
    /// Datagram (with its duplicate, if any) held back to be swapped with the next one.
    held: Option<Vec<Vec<u8>>>,
}

impl Direction {
    /// Applies `faults` (drop, then duplicate, then swap) to one datagram and
    /// returns what to deliver now, in order.
    fn offer(&mut self, faults: Faults, data: &[u8]) -> Vec<Vec<u8>> {
        self.offered += 1;
        let count = self.offered;
        let hits = |every: u32| every != 0 && count.is_multiple_of(u64::from(every));

        let mut out = Vec::new();
        if hits(faults.drop_every) {
            return out;
        }
        let copies = if hits(faults.duplicate_every) { 2 } else { 1 };
        let unit = vec![data.to_vec(); copies];

        if let Some(held) = self.held.take() {
            if hits(faults.swap_every) {
                out.extend(unit);
                out.extend(held);
                return out;
            }
            out.extend(held);
        }
        let next_swaps =
            faults.swap_every != 0 && (count + 1).is_multiple_of(u64::from(faults.swap_every));
        if next_swaps {
            self.held = Some(unit);
        } else {
            out.extend(unit);
        }
        out
    }

    /// Gives up the held-back datagram, if any.
    fn release(&mut self) -> Vec<Vec<u8>> {
        self.held.take().unwrap_or_default()
    }
}

struct EndpointState {
    faults: Faults,
    to_hub: Direction,
    from_hub: Direction,
    inbox: VecDeque<Vec<u8>>,
}

struct Shared {
    next_id: u32,
    /// Datagrams waiting for the hub, tagged with the sending endpoint's id.
    hub_inbox: VecDeque<(u32, Vec<u8>)>,
    endpoints: BTreeMap<u32, EndpointState>,
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Server side of the loopback; peers appear as `PeerAddr::Loopback(id)`.
pub struct LoopbackHub {
    shared: Arc<Mutex<Shared>>,
}

impl LoopbackHub {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared {
                next_id: 1,
                hub_inbox: VecDeque::new(),
                endpoints: BTreeMap::new(),
            })),
        }
    }

    pub fn connect(&self) -> LoopbackEndpoint {
        let mut shared = lock(&self.shared);
        let id = shared.next_id;
        shared.next_id += 1;
        shared.endpoints.insert(
            id,
            EndpointState {
                faults: Faults::default(),
                to_hub: Direction::default(),
                from_hub: Direction::default(),
                inbox: VecDeque::new(),
            },
        );
        LoopbackEndpoint {
            shared: Arc::clone(&self.shared),
            id,
        }
    }
}

impl Default for LoopbackHub {
    fn default() -> Self {
        Self::new()
    }
}

impl PacketTransport for LoopbackHub {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        let mut guard = lock(&self.shared);
        let shared = &mut *guard;
        if shared.hub_inbox.is_empty() {
            let released = shared
                .endpoints
                .iter_mut()
                .map(|(&id, ep)| (id, ep.to_hub.release()))
                .find(|(_, held)| !held.is_empty());
            if let Some((id, held)) = released {
                shared.hub_inbox.extend(held.into_iter().map(|d| (id, d)));
            }
        }
        let (id, data) = shared.hub_inbox.pop_front()?;
        Some((PeerAddr::Loopback(id), data))
    }

    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), TransportError> {
        check_len(data)?;
        let PeerAddr::Loopback(id) = to else {
            return Err(TransportError::Closed);
        };
        let mut shared = lock(&self.shared);
        let ep = shared
            .endpoints
            .get_mut(&id)
            .ok_or(TransportError::Closed)?;
        let out = ep.from_hub.offer(ep.faults, data);
        ep.inbox.extend(out);
        Ok(())
    }
}

/// Client side of the loopback; the hub is `PeerAddr::Loopback(0)`.
pub struct LoopbackEndpoint {
    shared: Arc<Mutex<Shared>>,
    id: u32,
}

impl LoopbackEndpoint {
    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn set_faults(&mut self, f: Faults) {
        if let Some(ep) = lock(&self.shared).endpoints.get_mut(&self.id) {
            ep.faults = f;
        }
    }
}

impl PacketTransport for LoopbackEndpoint {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        let mut shared = lock(&self.shared);
        let ep = shared.endpoints.get_mut(&self.id)?;
        if ep.inbox.is_empty() {
            let held = ep.from_hub.release();
            ep.inbox.extend(held);
        }
        ep.inbox
            .pop_front()
            .map(|data| (PeerAddr::Loopback(0), data))
    }

    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), TransportError> {
        check_len(data)?;
        if to != PeerAddr::Loopback(0) {
            return Err(TransportError::Closed);
        }
        let mut guard = lock(&self.shared);
        let shared = &mut *guard;
        let ep = shared
            .endpoints
            .get_mut(&self.id)
            .ok_or(TransportError::Closed)?;
        let out = ep.to_hub.offer(ep.faults, data);
        shared
            .hub_inbox
            .extend(out.into_iter().map(|d| (self.id, d)));
        Ok(())
    }
}

impl Drop for LoopbackEndpoint {
    /// Unregisters the endpoint; a datagram it had in flight to the hub still arrives.
    fn drop(&mut self) {
        let mut shared = lock(&self.shared);
        if let Some(mut ep) = shared.endpoints.remove(&self.id) {
            let held = ep.to_hub.release();
            shared
                .hub_inbox
                .extend(held.into_iter().map(|d| (self.id, d)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_UDP_PACKET;

    const HUB: PeerAddr = PeerAddr::Loopback(0);

    fn drain(t: &mut impl PacketTransport) -> Vec<(PeerAddr, Vec<u8>)> {
        std::iter::from_fn(|| t.recv()).collect()
    }

    fn payloads(got: &[(PeerAddr, Vec<u8>)]) -> Vec<u8> {
        got.iter().map(|(_, d)| d[0]).collect()
    }

    #[test]
    fn loopback_endpoints_get_distinct_ids() {
        let mut hub = LoopbackHub::new();
        let mut a = hub.connect();
        let mut b = hub.connect();
        assert_eq!((a.id(), b.id()), (1, 2));

        a.send(HUB, b"a").unwrap();
        b.send(HUB, b"b").unwrap();
        assert_eq!(
            drain(&mut hub),
            vec![
                (PeerAddr::Loopback(1), b"a".to_vec()),
                (PeerAddr::Loopback(2), b"b".to_vec()),
            ]
        );

        hub.send(PeerAddr::Loopback(2), b"to-b").unwrap();
        assert!(a.recv().is_none());
        assert_eq!(b.recv(), Some((HUB, b"to-b".to_vec())));
    }

    #[test]
    fn loopback_drop_every_third() {
        let mut hub = LoopbackHub::new();
        let mut ep = hub.connect();
        ep.set_faults(Faults {
            drop_every: 3,
            ..Faults::default()
        });
        for i in 1..=9u8 {
            ep.send(HUB, &[i]).unwrap();
        }
        assert_eq!(payloads(&drain(&mut hub)), vec![1, 2, 4, 5, 7, 8]);

        for i in 1..=9u8 {
            hub.send(PeerAddr::Loopback(ep.id()), &[i]).unwrap();
        }
        assert_eq!(payloads(&drain(&mut ep)), vec![1, 2, 4, 5, 7, 8]);
    }

    #[test]
    fn loopback_duplicate_and_swap() {
        let mut hub = LoopbackHub::new();
        let mut ep = hub.connect();
        let id = PeerAddr::Loopback(ep.id());

        ep.set_faults(Faults {
            duplicate_every: 2,
            ..Faults::default()
        });
        for i in 1..=4u8 {
            ep.send(HUB, &[i]).unwrap();
        }
        assert_eq!(payloads(&drain(&mut hub)), vec![1, 2, 2, 3, 4, 4]);
        for i in 1..=4u8 {
            hub.send(id, &[i]).unwrap();
        }
        assert_eq!(payloads(&drain(&mut ep)), vec![1, 2, 2, 3, 4, 4]);

        let mut hub = LoopbackHub::new();
        let mut ep = hub.connect();
        let id = PeerAddr::Loopback(ep.id());
        ep.set_faults(Faults {
            swap_every: 2,
            ..Faults::default()
        });
        for i in 1..=4u8 {
            ep.send(HUB, &[i]).unwrap();
        }
        assert_eq!(payloads(&drain(&mut hub)), vec![2, 1, 4, 3]);
        for i in 1..=4u8 {
            hub.send(id, &[i]).unwrap();
        }
        assert_eq!(payloads(&drain(&mut ep)), vec![2, 1, 4, 3]);
    }

    #[test]
    fn loopback_held_back_datagram_is_not_lost() {
        let mut hub = LoopbackHub::new();
        let mut ep = hub.connect();
        let id = PeerAddr::Loopback(ep.id());
        ep.set_faults(Faults {
            swap_every: 2,
            ..Faults::default()
        });
        for i in 1..=3u8 {
            ep.send(HUB, &[i]).unwrap();
            hub.send(id, &[i]).unwrap();
        }
        assert_eq!(payloads(&drain(&mut hub)), vec![2, 1, 3]);
        assert_eq!(payloads(&drain(&mut ep)), vec![2, 1, 3]);
    }

    #[test]
    fn loopback_faults_apply_drop_then_duplicate_then_swap() {
        let mut hub = LoopbackHub::new();
        let mut ep = hub.connect();
        ep.set_faults(Faults {
            drop_every: 3,
            duplicate_every: 2,
            swap_every: 2,
        });
        for i in 1..=6u8 {
            ep.send(HUB, &[i]).unwrap();
        }
        // 3 and 6 dropped; 2 and 4 duplicated; the pair of 2s swaps ahead of 1;
        // 4 has no partner (3 was dropped); 5 waits for 6, then is released on drain.
        assert_eq!(payloads(&drain(&mut hub)), vec![2, 2, 1, 4, 4, 5]);
    }

    #[test]
    fn loopback_closed_peers() {
        let mut hub = LoopbackHub::new();
        let mut ep = hub.connect();
        assert!(matches!(
            hub.send(PeerAddr::Loopback(7), b"x"),
            Err(TransportError::Closed)
        ));
        let udp = PeerAddr::Udp("127.0.0.1:1".parse().unwrap());
        assert!(matches!(hub.send(udp, b"x"), Err(TransportError::Closed)));
        assert!(matches!(
            ep.send(PeerAddr::Loopback(1), b"x"),
            Err(TransportError::Closed)
        ));
        assert!(matches!(ep.send(udp, b"x"), Err(TransportError::Closed)));

        let id = PeerAddr::Loopback(ep.id());
        drop(ep);
        assert!(matches!(hub.send(id, b"x"), Err(TransportError::Closed)));
    }

    #[test]
    fn oversize_send_is_rejected() {
        let mut hub = LoopbackHub::new();
        let mut ep = hub.connect();
        ep.set_faults(Faults {
            drop_every: 1,
            ..Faults::default()
        });
        let big = vec![0u8; MAX_UDP_PACKET + 1];
        assert!(matches!(
            ep.send(HUB, &big),
            Err(TransportError::TooLarge(4011))
        ));
        assert!(matches!(
            hub.send(PeerAddr::Loopback(ep.id()), &big),
            Err(TransportError::TooLarge(4011))
        ));
        ep.set_faults(Faults::default());
        ep.send(HUB, &big[..MAX_UDP_PACKET]).unwrap();
        assert_eq!(hub.recv().map(|(_, d)| d.len()), Some(MAX_UDP_PACKET));
    }
}
