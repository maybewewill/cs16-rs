//! Allowlist, per-IP session caps, and deterministic token-bucket rate limits for `cs-relay`.

use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

/// Explicit server target allowlist; empty by default (fail-closed).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TargetAllowlist {
    allowed: BTreeSet<SocketAddr>,
}

impl TargetAllowlist {
    pub fn add(&mut self, addr: SocketAddr) {
        self.allowed.insert(addr);
    }

    pub fn from_addrs(addrs: impl IntoIterator<Item = SocketAddr>) -> Self {
        Self {
            allowed: addrs.into_iter().collect(),
        }
    }

    pub fn is_allowed(&self, addr: &SocketAddr) -> bool {
        self.allowed.contains(addr)
    }

    pub fn is_empty(&self) -> bool {
        self.allowed.is_empty()
    }

    pub fn len(&self) -> usize {
        self.allowed.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &SocketAddr> {
        self.allowed.iter()
    }
}

#[derive(Debug, Clone)]
pub struct RelayPolicy {
    pub allowlist: TargetAllowlist,
    pub max_sessions_per_ip: usize,
    pub max_total_sessions: usize,
    pub max_packets_per_sec: f64,
    pub burst_packets: f64,
    pub max_bytes_per_sec: f64,
    pub idle_timeout_secs: f64,
}

impl Default for RelayPolicy {
    fn default() -> Self {
        Self {
            allowlist: TargetAllowlist::default(),
            max_sessions_per_ip: 4,
            max_total_sessions: 64,
            max_packets_per_sec: 250.0,
            burst_packets: 120.0,
            max_bytes_per_sec: 262_144.0,
            idle_timeout_secs: 60.0,
        }
    }
}

/// Deterministic token-bucket limiter for packets/sec and bytes/sec.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    pkt_tokens: f64,
    pkt_cap: f64,
    pkt_rate: f64,
    byte_tokens: f64,
    byte_cap: f64,
    byte_rate: f64,
    last_secs: f64,
}

impl TokenBucket {
    pub fn from_policy(policy: &RelayPolicy, now_secs: f64) -> Self {
        let byte_cap = policy.max_bytes_per_sec.max(4096.0);
        Self {
            pkt_tokens: policy.burst_packets,
            pkt_cap: policy.burst_packets,
            pkt_rate: policy.max_packets_per_sec,
            byte_tokens: byte_cap,
            byte_cap,
            byte_rate: policy.max_bytes_per_sec,
            last_secs: now_secs,
        }
    }

    /// Consumes 1 packet and `bytes` from the bucket at `now_secs`, returning `false` if limited.
    pub fn allow(&mut self, bytes: usize, now_secs: f64) -> bool {
        let dt = (now_secs - self.last_secs).max(0.0);
        self.last_secs = now_secs;
        self.pkt_tokens = (self.pkt_tokens + dt * self.pkt_rate).min(self.pkt_cap);
        self.byte_tokens = (self.byte_tokens + dt * self.byte_rate).min(self.byte_cap);
        let need_bytes = bytes as f64;
        if self.pkt_tokens < 1.0 || self.byte_tokens < need_bytes {
            return false;
        }
        self.pkt_tokens -= 1.0;
        self.byte_tokens -= need_bytes;
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayReject {
    TooManyForIp,
    TooManyTotal,
}

#[derive(Debug, Default)]
struct TrackerInner {
    per_ip: HashMap<IpAddr, usize>,
    total: usize,
}

/// Tracks active relay sessions per source IP and globally.
#[derive(Debug, Default)]
pub struct SessionTracker {
    inner: Mutex<TrackerInner>,
}

/// RAII session permit that releases capacity on drop.
#[derive(Debug)]
pub struct SessionGuard {
    tracker: Arc<SessionTracker>,
    ip: IpAddr,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = self.tracker.inner.lock() {
            g.total = g.total.saturating_sub(1);
            if let Some(count) = g.per_ip.get_mut(&self.ip) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    g.per_ip.remove(&self.ip);
                }
            }
        }
    }
}

impl SessionTracker {
    pub fn try_acquire(
        tracker: &Arc<Self>,
        ip: IpAddr,
        policy: &RelayPolicy,
    ) -> Result<SessionGuard, RelayReject> {
        let mut g = tracker.inner.lock().map_err(|_| RelayReject::TooManyTotal)?;
        if g.total >= policy.max_total_sessions {
            return Err(RelayReject::TooManyTotal);
        }
        let ip_count = g.per_ip.get(&ip).copied().unwrap_or(0);
        if ip_count >= policy.max_sessions_per_ip {
            return Err(RelayReject::TooManyForIp);
        }
        g.total += 1;
        g.per_ip.insert(ip, ip_count + 1);
        Ok(SessionGuard {
            tracker: Arc::clone(tracker),
            ip,
        })
    }

    pub fn active_total(&self) -> usize {
        self.inner.lock().map_or(0, |g| g.total)
    }
}

/// Extracts `target=<ip>:<port>` from a URL query string.
pub fn parse_relay_target(query: Option<&str>) -> Option<SocketAddr> {
    let q = query?;
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=')?;
        if k == "target" {
            let decoded = v.replace("%3A", ":").replace("%3a", ":");
            return decoded.parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_and_query_parser_fail_closed() {
        let empty = TargetAllowlist::default();
        let a: SocketAddr = "127.0.0.1:27016".parse().unwrap();
        assert!(!empty.is_allowed(&a));

        let allow = TargetAllowlist::from_addrs([a]);
        assert!(allow.is_allowed(&a));
        assert!(!allow.is_allowed(&"127.0.0.1:27017".parse().unwrap()));

        assert_eq!(parse_relay_target(Some("target=127.0.0.1:27016")), Some(a));
        assert_eq!(
            parse_relay_target(Some("foo=1&target=127.0.0.1%3A27016")),
            Some(a)
        );
        assert_eq!(parse_relay_target(Some("target=evil.example:27016")), None);
        assert_eq!(parse_relay_target(None), None);
    }

    #[test]
    fn session_tracker_and_token_bucket_enforce_limits() {
        let policy = RelayPolicy {
            max_sessions_per_ip: 2,
            max_total_sessions: 3,
            max_packets_per_sec: 10.0,
            burst_packets: 2.0,
            max_bytes_per_sec: 4096.0,
            ..RelayPolicy::default()
        };
        let tracker = Arc::new(SessionTracker::default());
        let ip1: IpAddr = "10.0.0.1".parse().unwrap();
        let ip2: IpAddr = "10.0.0.2".parse().unwrap();

        let g1 = SessionTracker::try_acquire(&tracker, ip1, &policy).unwrap();
        let _g2 = SessionTracker::try_acquire(&tracker, ip1, &policy).unwrap();
        assert_eq!(
            SessionTracker::try_acquire(&tracker, ip1, &policy).unwrap_err(),
            RelayReject::TooManyForIp
        );
        let _g3 = SessionTracker::try_acquire(&tracker, ip2, &policy).unwrap();
        assert_eq!(
            SessionTracker::try_acquire(&tracker, ip2, &policy).unwrap_err(),
            RelayReject::TooManyTotal
        );
        drop(g1);
        assert!(SessionTracker::try_acquire(&tracker, ip1, &policy).is_ok());

        let mut tb = TokenBucket::from_policy(&policy, 0.0);
        assert!(tb.allow(100, 0.0));
        assert!(tb.allow(100, 0.0));
        assert!(!tb.allow(100, 0.0));
        assert!(tb.allow(100, 0.15));
    }
}
