//! `cs-relay`: standalone hardened WebSocket-to-UDP proxy for Protocol 48 GoldSrc servers.

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use tracing::{error, info};
    use tracing_subscriber::EnvFilter;

    use cs_relay::{RelayPolicy, RelayServer, TargetAllowlist};

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let mut listen = "127.0.0.1:27017".to_string();
    let mut allowlist = TargetAllowlist::default();
    let mut policy = RelayPolicy::default();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| panic!("{arg} needs a value"));
        match arg.as_str() {
            "--listen" => listen = val(),
            "--allow" => {
                let raw = val();
                let addr: SocketAddr = raw
                    .parse()
                    .unwrap_or_else(|e| panic!("invalid --allow address `{raw}`: {e}"));
                allowlist.add(addr);
            }
            "--max-per-ip" => {
                policy.max_sessions_per_ip = val().parse().expect("valid --max-per-ip");
            }
            "--max-sessions" => {
                policy.max_total_sessions = val().parse().expect("valid --max-sessions");
            }
            other => {
                error!("unknown argument `{other}`");
                std::process::exit(2);
            }
        }
    }

    if allowlist.is_empty() {
        error!("refusing to start open relay: pass at least one --allow <ip:port>");
        std::process::exit(2);
    }
    policy.allowlist = allowlist;

    let srv = match RelayServer::bind(&listen, policy) {
        Ok(s) => s,
        Err(err) => {
            error!(%err, %listen, "failed to bind cs-relay");
            std::process::exit(1);
        }
    };
    info!(addr = %srv.local_addr(), "cs-relay listening");

    let stop = Arc::new(AtomicBool::new(false));
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {}
