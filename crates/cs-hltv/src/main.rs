//! Standalone HLTV (Half-Life TV) proxy CLI (`cs-hltv`).
//!
//! Example:
//! `cs-hltv --connect 127.0.0.1:27015 --port 27020 --delay 30 --max-clients 64 --hostname "My HLTV" --record match.dem`

use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;

use cs_hltv::delay::DEFAULT_TV_DELAY_SECS;
use cs_hltv::proxy::HltvConfig;

#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};
#[cfg(not(target_arch = "wasm32"))]
use cs_hltv::proxy::{HltvProxy, UpstreamState};
#[cfg(not(target_arch = "wasm32"))]
use cs_transport::PeerAddr;
#[cfg(not(target_arch = "wasm32"))]
use tracing::{error, info};

const DEFAULT_HLTV_PORT: u16 = 27020;
#[cfg(not(target_arch = "wasm32"))]
const TICK_SECS: f64 = 0.01;

#[derive(Debug, Clone, PartialEq)]
struct CliOptions {
    connect: Option<SocketAddr>,
    port: u16,
    delay: f64,
    max_clients: u8,
    record: Option<PathBuf>,
    hostname: String,
    name: String,
    password: Option<String>,
}

impl Default for CliOptions {
    fn default() -> Self {
        let defaults = HltvConfig::default();
        Self {
            connect: None,
            port: DEFAULT_HLTV_PORT,
            delay: DEFAULT_TV_DELAY_SECS,
            max_clients: defaults.max_spectators,
            record: None,
            hostname: defaults.hostname,
            name: defaults.name,
            password: None,
        }
    }
}

fn usage() -> &'static str {
    "Usage: cs-hltv [OPTIONS]\n\
     Options:\n  \
       --connect <addr>       Upstream GoldSrc server ip:port\n  \
       --port <u16>           UDP port for spectators (default 27020)\n  \
       --delay <f64>          Broadcast delay in seconds (default 30.0)\n  \
       --max-clients <u8>     Maximum spectator slots (default 64)\n  \
       --record <path.dem>    Record HLDEMO v5 demo file\n  \
       --hostname <str>       Proxy hostname shown in A2S_INFO\n  \
       --name <str>           Proxy name sent to upstream server\n  \
       --password <str>       Optional spectator password\n  \
       -h, --help             Show this help message"
}

fn parse_args(args: &[String]) -> Result<Option<CliOptions>, String> {
    let mut opts = CliOptions::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut next_val = || iter.next().ok_or_else(|| format!("{arg} requires a value"));
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--connect" | "-c" => {
                let raw = next_val()?;
                let addr = resolve_addr(raw)?;
                opts.connect = Some(addr);
            }
            "--port" | "-p" => {
                let raw = next_val()?;
                opts.port = raw
                    .parse::<u16>()
                    .map_err(|_| format!("{arg}: invalid port `{raw}`"))?;
            }
            "--delay" | "-d" => {
                let raw = next_val()?;
                let delay = raw
                    .parse::<f64>()
                    .map_err(|_| format!("{arg}: invalid delay `{raw}`"))?;
                if !delay.is_finite() || delay < 0.0 {
                    return Err(format!("{arg}: delay must be >= 0.0, got `{raw}`"));
                }
                opts.delay = delay;
            }
            "--max-clients" | "--max-spectators" => {
                let raw = next_val()?;
                opts.max_clients = raw
                    .parse::<u8>()
                    .map_err(|_| format!("{arg}: invalid count `{raw}` (0..=255)"))?;
            }
            "--record" | "-r" => {
                opts.record = Some(PathBuf::from(next_val()?));
            }
            "--hostname" => {
                opts.hostname = next_val()?.clone();
            }
            "--name" => {
                opts.name = next_val()?.clone();
            }
            "--password" => {
                let pw = next_val()?;
                opts.password = (!pw.is_empty()).then(|| pw.clone());
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(Some(opts))
}

fn resolve_addr(raw: &str) -> Result<SocketAddr, String> {
    if let Ok(addr) = raw.parse::<SocketAddr>() {
        return Ok(addr);
    }
    raw.to_socket_addrs()
        .map_err(|e| format!("cannot resolve `{raw}`: {e}"))?
        .next()
        .ok_or_else(|| format!("no socket address found for `{raw}`"))
}

#[cfg(not(target_arch = "wasm32"))]
fn run(opts: CliOptions) -> Result<(), String> {
    let bind_addr = format!("0.0.0.0:{}", opts.port);
    let cfg = HltvConfig {
        name: opts.name,
        hostname: opts.hostname,
        delay_secs: opts.delay,
        max_spectators: opts.max_clients,
        password: opts.password,
        master_addr: opts.connect.map(PeerAddr::Udp),
        record_demo_path: opts.record.clone(),
    };

    let mut proxy = HltvProxy::bind_udp(cfg, &bind_addr)
        .map_err(|e| format!("failed to bind HLTV proxy on {bind_addr}: {e}"))?;
    let listen_addr = proxy.downstream_transport().local_addr();
    info!(
        listen = %listen_addr,
        master = ?opts.connect,
        delay_secs = opts.delay,
        max_clients = opts.max_clients,
        record = ?opts.record,
        "cs-hltv proxy started (100 Hz)"
    );

    let start = Instant::now();
    let mut next_tick = TICK_SECS;
    loop {
        let now = start.elapsed().as_secs_f64();
        proxy.step(now);

        if matches!(
            proxy.upstream_state(),
            UpstreamState::Rejected(_) | UpstreamState::Disconnected(_)
        ) {
            info!(state = ?proxy.upstream_state(), "upstream session ended");
            break;
        }

        next_tick += TICK_SECS;
        let elapsed = start.elapsed().as_secs_f64();
        if next_tick > elapsed {
            std::thread::sleep(Duration::from_secs_f64(next_tick - elapsed));
        } else {
            next_tick = elapsed;
        }
    }

    if let Err(err) = proxy.save_recording() {
        error!(error = %err, "failed to write recorded demo file");
        return Err(err.to_string());
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    use tracing_subscriber::EnvFilter;

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(None) => println!("{}", usage()),
        Ok(Some(opts)) => {
            if let Err(err) = run(opts) {
                eprintln!("error: {err}");
                std::process::exit(1);
            }
        }
        Err(err) => {
            eprintln!("error: {err}\n\n{}", usage());
            std::process::exit(1);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|v| (*v).to_string()).collect()
    }

    #[test]
    fn parses_cli_flags_and_defaults() {
        let def = parse_args(&[]).unwrap().unwrap();
        assert_eq!(def.port, 27020);
        assert_eq!(def.delay, 30.0);
        assert_eq!(def.max_clients, 64);
        assert_eq!(def.connect, None);
        assert_eq!(def.record, None);

        let parsed = parse_args(&s(&[
            "--connect",
            "127.0.0.1:27015",
            "--port",
            "27025",
            "--delay",
            "15.5",
            "--max-clients",
            "128",
            "--record",
            "match.dem",
            "--hostname",
            "Tournament HLTV",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(parsed.connect, Some("127.0.0.1:27015".parse().unwrap()));
        assert_eq!(parsed.port, 27025);
        assert_eq!(parsed.delay, 15.5);
        assert_eq!(parsed.max_clients, 128);
        assert_eq!(parsed.record, Some(PathBuf::from("match.dem")));
        assert_eq!(parsed.hostname, "Tournament HLTV");
    }

    #[test]
    fn rejects_invalid_cli_flags() {
        for bad in [
            &["--delay", "-1"][..],
            &["--port", "notaport"],
            &["--max-clients", "999"],
            &["--connect"],
            &["--unknown"],
        ] {
            assert!(parse_args(&s(bad)).is_err(), "{bad:?}");
        }
        assert!(parse_args(&s(&["--help"])).unwrap().is_none());
    }
}
