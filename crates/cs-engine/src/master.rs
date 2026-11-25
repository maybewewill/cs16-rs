//! Server-side GoldSrc Master Server registration & heartbeats (`setmaster`, `q` -> `s\n<ch>`
//! -> `0\n\protocol\48\...`, shutdown `b\n`).

use cs_transport::{PacketTransport, PeerAddr};

const DEFAULT_HEARTBEAT_INTERVAL_SECS: f64 = 60.0;
const MAX_MASTERS: usize = 16;

#[derive(Debug, Clone, PartialEq)]
struct MasterTarget {
    addr: PeerAddr,
    next_heartbeat_at: f64,
}

/// Server metadata included in `0\n` heartbeat updates sent to master servers.
#[derive(Debug, Clone)]
pub struct HeartbeatSnapshot {
    pub players: u8,
    pub max_players: u8,
    pub bots: u8,
    pub gamedir: String,
    pub map: String,
    pub password: bool,
    pub build: u32,
}

/// Tracks configured master servers (`setmaster add|remove|clear|enable|disable|list`) and emits
/// GoldSrc `q` challenge requests, `0\n` updates, and `b\n` shutdown notices.
#[derive(Debug)]
pub struct MasterRegistration {
    targets: Vec<MasterTarget>,
    pub enabled: bool,
    interval_secs: f64,
}

impl Default for MasterRegistration {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            enabled: true,
            interval_secs: DEFAULT_HEARTBEAT_INTERVAL_SECS,
        }
    }
}

impl MasterRegistration {
    pub fn add(&mut self, addr: PeerAddr, now: f64) -> bool {
        if self.targets.iter().any(|t| t.addr == addr) || self.targets.len() >= MAX_MASTERS {
            return false;
        }
        self.targets.push(MasterTarget {
            addr,
            next_heartbeat_at: now,
        });
        true
    }

    pub fn remove(&mut self, addr: &PeerAddr) -> bool {
        let before = self.targets.len();
        self.targets.retain(|t| &t.addr != addr);
        self.targets.len() != before
    }

    pub fn clear(&mut self) {
        self.targets.clear();
    }

    pub fn is_registered(&self, addr: &PeerAddr) -> bool {
        self.targets.iter().any(|t| &t.addr == addr)
    }

    pub fn targets(&self) -> Vec<PeerAddr> {
        self.targets.iter().map(|t| t.addr).collect()
    }

    pub fn list(&self) -> String {
        if self.targets.is_empty() {
            return format!(
                "Master server list empty (enabled: {})\n",
                usize::from(self.enabled)
            );
        }
        let mut out = format!("Master servers (enabled: {}):\n", usize::from(self.enabled));
        for (i, t) in self.targets.iter().enumerate() {
            out.push_str(&format!("{}: {}\n", i + 1, t.addr));
        }
        out
    }

    /// Sends `"q"` heartbeat challenge requests to any registered master due at `now`.
    pub fn tick(&mut self, net: &mut dyn PacketTransport, now: f64) {
        if !self.enabled || self.targets.is_empty() {
            return;
        }
        for target in &mut self.targets {
            if now >= target.next_heartbeat_at {
                target.next_heartbeat_at = now + self.interval_secs;
                let _ = net.send(target.addr, b"q");
            }
        }
    }

    /// Sends `"b\n"` shutdown packets to every registered master server.
    pub fn shutdown_all(&self, net: &mut dyn PacketTransport) {
        for target in &self.targets {
            let _ = net.send(target.addr, b"b\n");
        }
    }

    /// Parses `\xFF\xFF\xFF\xFFs\n<u32 le>` sent by `cs-master` (or HLDS master).
    pub fn parse_challenge_reply(raw: &[u8]) -> Option<u32> {
        if raw.len() == 10 && raw.starts_with(b"\xFF\xFF\xFF\xFFs\n") {
            Some(u32::from_le_bytes([raw[6], raw[7], raw[8], raw[9]]))
        } else {
            None
        }
    }

    /// Encodes `0\n\protocol\48\challenge\<ch>\...` matching `cs_master::protocol::parse_heartbeat_packet`.
    pub fn encode_heartbeat_update(challenge: u32, snap: &HeartbeatSnapshot) -> Vec<u8> {
        let gamedir = snap.gamedir.replace('\\', "");
        let map = snap.map.replace('\\', "");
        let os = if cfg!(windows) { "w" } else { "l" };
        let pw = u8::from(snap.password);
        format!(
            "0\n\\protocol\\48\\challenge\\{challenge}\\players\\{}\\max\\{}\\bots\\{}\\gamedir\\{gamedir}\\map\\{map}\\type\\d\\password\\{pw}\\os\\{os}\\secure\\0\\lan\\0\\version\\1.1.2.{}\\region\\255\\product\\{gamedir}\n",
            snap.players,
            snap.max_players,
            snap.bots,
            snap.build,
        )
        .into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;

    #[test]
    fn master_registration_lifecycle_and_heartbeat_format() {
        let mut reg = MasterRegistration::default();
        let addr = PeerAddr::Udp(SocketAddr::from(([127, 0, 0, 1], 27010)));
        assert!(reg.add(addr, 0.0));
        assert!(!reg.add(addr, 0.0));
        assert!(reg.is_registered(&addr));
        assert_eq!(MasterRegistration::parse_challenge_reply(b"\xFF\xFF\xFF\xFFs\n\x01\x00\x00\x00"), Some(1));

        let pkt = MasterRegistration::encode_heartbeat_update(
            77,
            &HeartbeatSnapshot {
                players: 3,
                max_players: 16,
                bots: 1,
                gamedir: "cstrike".into(),
                map: "de_dust2".into(),
                password: false,
                build: 10211,
            },
        );
        let text = String::from_utf8(pkt).unwrap();
        assert!(text.starts_with("0\n\\protocol\\48\\challenge\\77"));
        assert!(text.contains("\\map\\de_dust2\\"));
        assert!(reg.remove(&addr));
        assert!(!reg.is_registered(&addr));
    }
}
