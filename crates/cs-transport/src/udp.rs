//! Non-blocking UDP transport (native targets only).

use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};

use crate::{PacketTransport, PeerAddr, TransportError, check_len};

/// Receive buffer size: the largest possible UDP payload, so no datagram is truncated.
const RECV_BUF_LEN: usize = 65536;

pub struct UdpTransport {
    socket: UdpSocket,
    local_addr: SocketAddr,
    buf: Box<[u8]>,
}

impl UdpTransport {
    /// Binds a non-blocking socket to `addr` (e.g. `"0.0.0.0:27015"`).
    pub fn bind(addr: &str) -> Result<Self, TransportError> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        let local_addr = socket.local_addr()?;
        Ok(Self {
            socket,
            local_addr,
            buf: vec![0; RECV_BUF_LEN].into_boxed_slice(),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

impl PacketTransport for UdpTransport {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        loop {
            match self.socket.recv_from(&mut self.buf) {
                Ok((len, from)) => return Some((PeerAddr::Udp(from), self.buf[..len].to_vec())),
                // Windows reports an ICMP port-unreachable from an earlier send this way;
                // skip it so one dead peer cannot stall the receive loop.
                Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
                Err(_) => return None,
            }
        }
    }

    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), TransportError> {
        check_len(data)?;
        let PeerAddr::Udp(addr) = to else {
            return Err(TransportError::Closed);
        };
        self.socket.send_to(data, addr)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::MAX_UDP_PACKET;

    /// Polls `t` until a datagram arrives or a second passes.
    fn recv_within_a_second(t: &mut UdpTransport) -> Option<(PeerAddr, Vec<u8>)> {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if let Some(got) = t.recv() {
                return Some(got);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn udp_roundtrip_on_localhost() {
        let mut a = UdpTransport::bind("127.0.0.1:0").unwrap();
        let mut b = UdpTransport::bind("127.0.0.1:0").unwrap();
        assert!(b.recv().is_none(), "nothing waiting yet");

        a.send(PeerAddr::Udp(b.local_addr()), b"hello").unwrap();
        let (from, data) = recv_within_a_second(&mut b).expect("datagram arrives");
        assert_eq!(from, PeerAddr::Udp(a.local_addr()));
        assert_eq!(data, b"hello");
    }

    #[test]
    fn udp_rejects_oversize_and_loopback_peers() {
        let mut a = UdpTransport::bind("127.0.0.1:0").unwrap();
        let to = PeerAddr::Udp(a.local_addr());
        assert!(matches!(
            a.send(to, &[0; MAX_UDP_PACKET + 1]),
            Err(TransportError::TooLarge(4011))
        ));
        assert!(a.send(to, &[0; MAX_UDP_PACKET]).is_ok());
        assert!(matches!(
            a.send(PeerAddr::Loopback(0), b"x"),
            Err(TransportError::Closed)
        ));
    }

    #[test]
    fn udp_recv_survives_unreachable_peer() {
        let mut a = UdpTransport::bind("127.0.0.1:0").unwrap();
        let dead = UdpTransport::bind("127.0.0.1:0").unwrap();
        let dead_addr = dead.local_addr();
        drop(dead);
        // On Windows the ICMP port-unreachable surfaces as ConnectionReset on recv.
        a.send(PeerAddr::Udp(dead_addr), b"void").unwrap();
        // Gives the reset a head start so it is queued ahead of the live datagram;
        // the outcome does not depend on this timing, only how much the test proves.
        std::thread::sleep(Duration::from_millis(20));
        let mut b = UdpTransport::bind("127.0.0.1:0").unwrap();
        b.send(PeerAddr::Udp(a.local_addr()), b"alive").unwrap();

        // Blocking reads with a timeout remove any arrival-time assumption: a single
        // recv must step over the reset and wait for the live datagram. Polling would
        // not discriminate, since a recv that wrongly returns None is simply retried.
        a.socket.set_nonblocking(false).unwrap();
        a.socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (_, data) = a.recv().expect("datagram behind the reset is returned");
        assert_eq!(data, b"alive");
    }
}
