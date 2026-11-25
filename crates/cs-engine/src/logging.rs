//! HL Standard Log formatter (`L MM/DD/YYYY - HH:MM:SS: ...`) and remote UDP `logaddress`
//! streaming (`0xFFFF_FFFF "log " <line>\n\0`).
//!
//! Ported from ReHLDS `rehlds/engine/sv_log.cpp` / `sv_main.cpp` (read 2026-11-25).

use std::collections::VecDeque;

use cs_transport::{PacketTransport, PeerAddr};

const MAX_LOG_HISTORY: usize = 512;
const MAX_LOG_ADDRESSES: usize = 16;

#[derive(Debug, Default)]
pub struct ServerLogger {
    pub enabled: bool,
    pub addresses: Vec<PeerAddr>,
    pending_udp: Vec<String>,
    pub history: VecDeque<String>,
}

impl ServerLogger {
    pub fn format_hl_line(secs: f64, body: &str) -> String {
        let total = secs.max(0.0).floor() as u64;
        let sec = total % 60;
        let min = (total / 60) % 60;
        let hour = (total / 3600) % 24;
        let day = 1 + (total / 86_400) % 28;
        format!("L 01/{day:02}/1970 - {hour:02}:{min:02}:{sec:02}: {body}")
    }

    pub fn log(&mut self, secs: f64, body: &str) {
        if !self.enabled {
            return;
        }
        let clean_body: String = body
            .chars()
            .map(|c| if c < ' ' || c == '\u{7F}' { '?' } else { c })
            .collect();
        let line = Self::format_hl_line(secs, &clean_body);
        if self.history.len() >= MAX_LOG_HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(line.clone());
        if !self.addresses.is_empty() {
            self.pending_udp.push(line);
        }
    }

    pub fn set_single_address(&mut self, addr: PeerAddr) {
        self.addresses.clear();
        self.addresses.push(addr);
    }

    pub fn add_address(&mut self, addr: PeerAddr) -> bool {
        if self.addresses.contains(&addr) || self.addresses.len() >= MAX_LOG_ADDRESSES {
            return false;
        }
        self.addresses.push(addr);
        true
    }

    pub fn remove_address(&mut self, addr: &PeerAddr) -> bool {
        let before = self.addresses.len();
        self.addresses.retain(|a| a != addr);
        self.addresses.len() != before
    }

    pub fn clear_addresses(&mut self) {
        self.addresses.clear();
    }

    /// Flushes queued log lines to every configured `logaddress` UDP target.
    pub fn flush(&mut self, net: &mut dyn PacketTransport) {
        if self.pending_udp.is_empty() || self.addresses.is_empty() {
            self.pending_udp.clear();
            return;
        }
        for line in std::mem::take(&mut self.pending_udp) {
            let pkt = encode_log_packet(&line);
            for &addr in &self.addresses {
                let _ = net.send(addr, &pkt);
            }
        }
    }
}

/// Encodes `\xFF\xFF\xFF\xFFlog <line>\n\0`.
pub fn encode_log_packet(line: &str) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(line.len() + 12);
    pkt.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    pkt.extend_from_slice(b"log ");
    pkt.extend_from_slice(line.as_bytes());
    if !line.ends_with('\n') {
        pkt.push(b'\n');
    }
    pkt.push(0);
    pkt
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;

    #[test]
    fn formats_hl_standard_log_line_and_udp_packet() {
        let line = ServerLogger::format_hl_line(3665.0, "Loading map \"de_dust2\"");
        assert!(line.starts_with("L 01/01/1970 - 01:01:05: Loading map \"de_dust2\""));
        let pkt = encode_log_packet(&line);
        assert!(pkt.starts_with(b"\xFF\xFF\xFF\xFFlog L "));
        assert!(pkt.ends_with(b"\n\0"));
    }

    #[test]
    fn add_remove_and_flush_logaddress() {
        let mut logger = ServerLogger {
            enabled: true,
            ..Default::default()
        };
        let a1 = PeerAddr::Udp(SocketAddr::from(([127, 0, 0, 1], 27500)));
        let a2 = PeerAddr::Udp(SocketAddr::from(([127, 0, 0, 1], 27501)));
        assert!(logger.add_address(a1));
        assert!(logger.add_address(a2));
        assert!(!logger.add_address(a1));
        assert!(logger.remove_address(&a2));
        assert_eq!(logger.addresses, vec![a1]);
    }
}
