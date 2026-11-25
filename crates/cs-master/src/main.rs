//! `cs-master`: standalone GoldSrc Master Server (`0x31` query + heartbeat + optional HTTP JSON).

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    use std::net::SocketAddr;
    use std::time::{Duration, Instant};

    use tracing::{error, info};
    use tracing_subscriber::EnvFilter;

    use cs_master::registry::DEFAULT_TTL_SECS;
    use cs_master::server::{MasterConfig, MasterServer};

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let mut config = MasterConfig::default();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "Usage: cs-master [--udp <addr>] [--http <addr>] [--ttl <secs>]\n\
                     \n\
                     Options:\n\
                     \x20 --udp <addr>   UDP listen address (default: 0.0.0.0:27010)\n\
                     \x20 --http <addr>  Optional HTTP/1.1 JSON listen address (e.g. 0.0.0.0:27080)\n\
                     \x20 --ttl <secs>   Server heartbeat TTL in seconds (default: {DEFAULT_TTL_SECS})"
                );
                return;
            }
            "--udp" => {
                let raw = args
                    .next()
                    .unwrap_or_else(|| exit_with_arg_error("--udp requires an address"));
                config.udp_addr = raw.parse::<SocketAddr>().unwrap_or_else(|e| {
                    exit_with_arg_error(&format!("invalid --udp address '{raw}': {e}"))
                });
            }
            "--http" => {
                let raw = args
                    .next()
                    .unwrap_or_else(|| exit_with_arg_error("--http requires an address"));
                let addr = raw.parse::<SocketAddr>().unwrap_or_else(|e| {
                    exit_with_arg_error(&format!("invalid --http address '{raw}': {e}"))
                });
                config.http_addr = Some(addr);
            }
            "--ttl" => {
                let raw = args
                    .next()
                    .unwrap_or_else(|| exit_with_arg_error("--ttl requires a value"));
                let ttl = raw.parse::<f64>().unwrap_or_else(|e| {
                    exit_with_arg_error(&format!("invalid --ttl value '{raw}': {e}"))
                });
                if !ttl.is_finite() || ttl <= 0.0 {
                    exit_with_arg_error("--ttl must be a positive finite number");
                }
                config.ttl_secs = ttl;
            }
            other => {
                exit_with_arg_error(&format!("unknown argument '{other}' (try --help)"));
            }
        }
    }

    let mut server = match MasterServer::bind(config) {
        Ok(s) => s,
        Err(err) => {
            error!(%err, "failed to bind cs-master");
            std::process::exit(1);
        }
    };

    info!(
        udp = %server.udp_addr(),
        http = ?server.http_addr(),
        ttl_secs = server.registry().ttl_secs(),
        "cs-master listening"
    );

    let start = Instant::now();
    if let Err(err) = server.run_until(
        || false,
        || start.elapsed().as_secs_f64(),
        Duration::from_millis(5),
    ) {
        error!(%err, "cs-master stopped with error");
        std::process::exit(1);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn exit_with_arg_error(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(2);
}

#[cfg(target_arch = "wasm32")]
fn main() {}
