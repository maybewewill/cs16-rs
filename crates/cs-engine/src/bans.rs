//! IP/CIDR (`addip`/`removeip`/`listip`/`writeip`) and SteamID/AuthID (`banid`/`removeid`/
//! `listid`/`writeid`) ban lists enforced at connect time.
//!
//! Ported from ReHLDS `rehlds/engine/sv_user.cpp` / `host_cmd.cpp` (read 2026-11-25).

use std::net::{IpAddr, Ipv4Addr};

use cs_transport::PeerAddr;

#[derive(Debug, Clone, PartialEq)]
pub struct IpBan {
    pub pattern: String,
    pub ipv4_net: Option<(u32, u32)>,
    pub expires_at: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IdBan {
    pub auth_id: String,
    pub expires_at: Option<f64>,
}

#[derive(Debug, Default, Clone)]
pub struct BanList {
    pub ip_bans: Vec<IpBan>,
    pub id_bans: Vec<IdBan>,
}

fn parse_ip_pattern(pattern: &str) -> Option<(u32, u32)> {
    let trimmed = pattern.trim();
    if let Some((ip_part, prefix_str)) = trimmed.split_once('/') {
        let ip: Ipv4Addr = ip_part.parse().ok()?;
        let bits: u32 = prefix_str.parse().ok()?;
        if bits > 32 {
            return None;
        }
        let mask = if bits == 0 {
            0
        } else {
            u32::MAX << (32 - bits)
        };
        return Some((u32::from(ip) & mask, mask));
    }
    let ip: Ipv4Addr = trimmed.parse().ok()?;
    let octets = ip.octets();
    // GoldSrc `addip` treats trailing `.0` octets as subnet wildcards (`192.168.1.0` -> `/24`).
    let mask = if octets == [0, 0, 0, 0] {
        0
    } else if octets[1..] == [0, 0, 0] {
        0xFF00_0000
    } else if octets[2..] == [0, 0] {
        0xFFFF_0000
    } else if octets[3] == 0 {
        0xFFFF_FF00
    } else {
        u32::MAX
    };
    Some((u32::from(ip) & mask, mask))
}

impl BanList {
    pub fn prune(&mut self, now: f64) {
        self.ip_bans
            .retain(|b| b.expires_at.is_none_or(|until| now < until));
        self.id_bans
            .retain(|b| b.expires_at.is_none_or(|until| now < until));
    }

    pub fn add_ip(&mut self, minutes: f64, pattern: &str, now: f64) -> Result<(), String> {
        let pat = pattern.trim();
        if pat.is_empty() {
            return Err("Usage: addip <minutes> <ipaddress>".into());
        }
        let ipv4_net = parse_ip_pattern(pat);
        if ipv4_net.is_none()
            && !pat.starts_with("loopback:")
            && !pat.starts_with("web:")
        {
            return Err(format!("Invalid IP address: {pat}"));
        }
        let expires_at = if minutes > 0.0 && minutes.is_finite() {
            Some(now + minutes * 60.0)
        } else {
            None
        };
        if let Some(existing) = self.ip_bans.iter_mut().find(|b| b.pattern == pat) {
            existing.expires_at = expires_at;
        } else {
            self.ip_bans.push(IpBan {
                pattern: pat.to_string(),
                ipv4_net,
                expires_at,
            });
        }
        Ok(())
    }

    pub fn remove_ip(&mut self, pattern: &str) -> bool {
        let pat = pattern.trim();
        let before = self.ip_bans.len();
        self.ip_bans.retain(|b| b.pattern != pat);
        self.ip_bans.len() != before
    }

    pub fn is_ip_banned(&mut self, addr: &PeerAddr, now: f64) -> bool {
        self.prune(now);
        let peer_str = addr.to_string();
        for ban in &self.ip_bans {
            if let (Some((net, mask)), PeerAddr::Udp(sa)) = (ban.ipv4_net, addr)
                && let IpAddr::V4(v4) = sa.ip()
                && (u32::from(v4) & mask) == net
            {
                return true;
            }
            if ban.pattern == peer_str {
                return true;
            }
        }
        false
    }

    pub fn list_ip(&mut self, now: f64) -> String {
        self.prune(now);
        if self.ip_bans.is_empty() {
            return "IP filter list: empty\n".to_string();
        }
        let mut out = format!("IP filter list ({}):\n", self.ip_bans.len());
        for (i, b) in self.ip_bans.iter().enumerate() {
            let mins = match b.expires_at {
                Some(until) => format!("{:.2} min", ((until - now) / 60.0).max(0.0)),
                None => "permanent".to_string(),
            };
            out.push_str(&format!("{}: {} : {}\n", i + 1, b.pattern, mins));
        }
        out
    }

    pub fn write_ip_cfg(&mut self, now: f64) -> String {
        self.prune(now);
        let mut out = String::new();
        for b in &self.ip_bans {
            let mins = match b.expires_at {
                Some(until) => ((until - now) / 60.0).max(0.1),
                None => 0.0,
            };
            out.push_str(&format!("addip {:.2} {}\n", mins, b.pattern));
        }
        out
    }

    pub fn add_id(&mut self, minutes: f64, auth_id: &str, now: f64) -> Result<(), String> {
        let id = auth_id.trim();
        if id.is_empty() {
            return Err("Usage: banid <minutes> <#userid | uniqueid> [kick]".into());
        }
        let expires_at = if minutes > 0.0 && minutes.is_finite() {
            Some(now + minutes * 60.0)
        } else {
            None
        };
        if let Some(existing) = self
            .id_bans
            .iter_mut()
            .find(|b| b.auth_id.eq_ignore_ascii_case(id))
        {
            existing.expires_at = expires_at;
        } else {
            self.id_bans.push(IdBan {
                auth_id: id.to_string(),
                expires_at,
            });
        }
        Ok(())
    }

    pub fn remove_id(&mut self, auth_id: &str) -> bool {
        let id = auth_id.trim();
        let before = self.id_bans.len();
        self.id_bans.retain(|b| !b.auth_id.eq_ignore_ascii_case(id));
        self.id_bans.len() != before
    }

    pub fn is_id_banned(&mut self, auth_id: &str, now: f64) -> bool {
        self.prune(now);
        if auth_id.is_empty() {
            return false;
        }
        self.id_bans
            .iter()
            .any(|b| b.auth_id.eq_ignore_ascii_case(auth_id))
    }

    pub fn list_id(&mut self, now: f64) -> String {
        self.prune(now);
        if self.id_bans.is_empty() {
            return "ID filter list: empty\n".to_string();
        }
        let mut out = format!("ID filter list ({}):\n", self.id_bans.len());
        for (i, b) in self.id_bans.iter().enumerate() {
            let mins = match b.expires_at {
                Some(until) => format!("{:.2} min", ((until - now) / 60.0).max(0.0)),
                None => "permanent".to_string(),
            };
            out.push_str(&format!("{}: {} : {}\n", i + 1, b.auth_id, mins));
        }
        out
    }

    pub fn write_id_cfg(&mut self, now: f64) -> String {
        self.prune(now);
        let mut out = String::new();
        for b in &self.id_bans {
            let mins = match b.expires_at {
                Some(until) => ((until - now) / 60.0).max(0.1),
                None => 0.0,
            };
            out.push_str(&format!("banid {:.2} {}\n", mins, b.auth_id));
        }
        out
    }
}

/// Derives a stable GoldSrc-style SteamID/uniqueid string from the client's connect info.
pub fn derive_auth_id(protinfo_raw: &str, userinfo_sid: Option<&str>, from: &PeerAddr) -> String {
    if let Some(sid) = userinfo_sid.filter(|s| !s.is_empty()) {
        return sid.to_string();
    }
    if let Some(idx) = protinfo_raw.find("STEAM_") {
        let rest = &protinfo_raw[idx..];
        let end = rest
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != ':')
            .unwrap_or(rest.len());
        return rest[..end].to_string();
    }
    format!("VALVE_{}", cs_proto48::crypto::md5_hex(from.to_string().as_bytes())[..12].to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;

    #[test]
    fn ip_and_cidr_and_expiry_work() {
        let mut bans = BanList::default();
        bans.add_ip(5.0, "10.20.30.0", 100.0).unwrap();
        bans.add_ip(0.0, "192.168.5.0/24", 100.0).unwrap();

        let match1 = PeerAddr::Udp(SocketAddr::from(([10, 20, 30, 44], 27005)));
        let match2 = PeerAddr::Udp(SocketAddr::from(([192, 168, 5, 99], 27005)));
        let other = PeerAddr::Udp(SocketAddr::from(([10, 20, 31, 44], 27005)));

        assert!(bans.is_ip_banned(&match1, 110.0));
        assert!(bans.is_ip_banned(&match2, 110.0));
        assert!(!bans.is_ip_banned(&other, 110.0));

        // 5-minute ban expires at t=400; permanent stays.
        assert!(!bans.is_ip_banned(&match1, 405.0));
        assert!(bans.is_ip_banned(&match2, 405.0));

        assert!(bans.remove_ip("192.168.5.0/24"));
        assert!(!bans.is_ip_banned(&match2, 405.0));
    }

    #[test]
    fn id_bans_and_cfg_format_roundtrip() {
        let mut bans = BanList::default();
        bans.add_id(0.0, "STEAM_0:1:4242", 0.0).unwrap();
        assert!(bans.is_id_banned("steam_0:1:4242", 10.0));
        let cfg = bans.write_id_cfg(10.0);
        assert!(cfg.contains("banid 0.00 STEAM_0:1:4242"));
        assert!(bans.remove_id("STEAM_0:1:4242"));
        assert!(!bans.is_id_banned("STEAM_0:1:4242", 10.0));
    }
}
