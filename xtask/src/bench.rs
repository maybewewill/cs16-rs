//! Phase 13 performance benchmark harness (`cargo xtask bench`).
//!
//! Measures startup readiness, 32-player 100 Hz server tick timing, and delta snapshot packet
//! sizes using an in-process `Server<CsGame>` + `LoopbackHub` client + ZBot players.
//! Runs under whatever Cargo profile `xtask` was compiled with (dev by default; never invokes
//! `--release`).

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use cs_clientcore::{ConnState, Connection, ConnectionConfig};
use cs_cvar::CVarRegistry;
use cs_engine::{Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_local::{load_server_map, new_server};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::DeltaRegistry;
use cs_transport::{LoopbackHub, PacketTransport, PeerAddr};
use cs_vfs::{Vfs, VfsConfig};

/// CLI configuration for `cargo xtask bench`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchConfig {
    pub ticks: usize,
    pub players: u8,
    pub map: String,
    pub gamedir: Option<PathBuf>,
    pub stub: bool,
    pub json: bool,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            ticks: 100,
            players: 32,
            map: "de_dust2".to_string(),
            gamedir: None,
            stub: false,
            json: false,
        }
    }
}

/// Results emitted by `cargo xtask bench`.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchReport {
    pub profile: &'static str,
    pub map: String,
    pub map_source: &'static str,
    pub players: u8,
    pub ticks: usize,
    pub startup_ms: f64,
    pub tick_avg_ms: f64,
    pub tick_p50_ms: f64,
    pub tick_p95_ms: f64,
    pub tick_p99_ms: f64,
    pub tick_max_ms: f64,
    pub delta_pkt_count: u64,
    pub delta_pkt_avg_bytes: f64,
    pub delta_pkt_max_bytes: usize,
    pub bandwidth_kbps_per_client: f64,
}

struct MeasuringTransport {
    hub: LoopbackHub,
    recording: bool,
    tx_packets: u64,
    tx_bytes: u64,
    max_packet_bytes: usize,
}

impl MeasuringTransport {
    fn new(hub: LoopbackHub) -> Self {
        Self {
            hub,
            recording: false,
            tx_packets: 0,
            tx_bytes: 0,
            max_packet_bytes: 0,
        }
    }

    fn reset_metrics(&mut self) {
        self.recording = true;
        self.tx_packets = 0;
        self.tx_bytes = 0;
        self.max_packet_bytes = 0;
    }
}

impl PacketTransport for MeasuringTransport {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        self.hub.recv()
    }

    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), cs_transport::TransportError> {
        if self.recording {
            self.tx_packets += 1;
            self.tx_bytes += data.len() as u64;
            self.max_packet_bytes = self.max_packet_bytes.max(data.len());
        }
        self.hub.send(to, data)
    }
}

/// Parses CLI arguments for `cargo xtask bench`.
pub fn parse_bench_args(mut args: impl Iterator<Item = String>) -> Result<BenchConfig, String> {
    let mut cfg = BenchConfig::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ticks" => {
                let raw = args.next().ok_or("--ticks needs a value")?;
                let ticks = raw
                    .parse::<usize>()
                    .ok()
                    .filter(|&n| n >= 1)
                    .ok_or_else(|| format!("--ticks: expected positive integer, got `{raw}`"))?;
                cfg.ticks = ticks;
            }
            "--players" => {
                let raw = args.next().ok_or("--players needs a value")?;
                let players = raw
                    .parse::<u8>()
                    .ok()
                    .filter(|&n| (1..=32).contains(&n))
                    .ok_or_else(|| format!("--players: expected 1..=32, got `{raw}`"))?;
                cfg.players = players;
            }
            "--map" => cfg.map = args.next().ok_or("--map needs a value")?,
            "--gamedir" => cfg.gamedir = Some(PathBuf::from(args.next().ok_or("--gamedir needs a value")?)),
            "--stub" => cfg.stub = true,
            "--json" => cfg.json = true,
            other => return Err(format!("unknown bench argument `{other}`")),
        }
    }
    Ok(cfg)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Executes the benchmark in-process (on an 8 MiB stack thread for Windows dev builds)
/// and returns measured metrics.
pub fn run_bench_with_config(cfg: &BenchConfig) -> Result<BenchReport, String> {
    let cfg_owned = cfg.clone();
    let handle = std::thread::Builder::new()
        .name("xtask-bench".to_string())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || run_bench_inner(&cfg_owned))
        .map_err(|e| format!("failed to spawn bench thread: {e}"))?;
    handle
        .join()
        .map_err(|_| "benchmark thread panicked".to_string())?
}

fn run_bench_inner(cfg: &BenchConfig) -> Result<BenchReport, String> {
    let profile = if cfg!(debug_assertions) { "dev" } else { "release" };
    let t_start = Instant::now();

    let (mut server, map_source) = if cfg.stub {
        let mut game = CsGame::new();
        game.enable_cvar_bot_quota();
        let srv = Server::new(
            ServerConfig {
                hostname: "Bench Stub".to_string(),
                max_clients: cfg.players,
                ..ServerConfig::default()
            },
            ServerMap::stub(&cfg.map),
            game,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        (srv, "stub")
    } else if let Ok(vfs) = Vfs::discover(cfg.gamedir.as_deref(), VfsConfig::default()) {
        let map = load_server_map(&vfs, &cfg.map)?;
        let mut game = CsGame::new();
        game.load_motd(&vfs);
        game.load_map_cycle(&vfs);
        game.enable_cvar_bot_quota();
        let srv = new_server(
            ServerConfig {
                hostname: "Bench Real".to_string(),
                max_clients: cfg.players,
                ..ServerConfig::default()
            },
            map,
            game,
            &vfs,
        );
        (srv, "gamedir")
    } else if cfg.gamedir.is_some() {
        return Err("cannot discover Half-Life assets at specified --gamedir".to_string());
    } else {
        let mut game = CsGame::new();
        game.enable_cvar_bot_quota();
        let srv = Server::new(
            ServerConfig {
                hostname: "Bench Stub".to_string(),
                max_clients: cfg.players,
                ..ServerConfig::default()
            },
            ServerMap::stub(&cfg.map),
            game,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        (srv, "stub")
    };

    let startup_ms = t_start.elapsed().as_secs_f64() * 1000.0;

    let hub = LoopbackHub::new();
    let endpoint = hub.connect();
    let mut net = MeasuringTransport::new(hub);
    let mut conn = Connection::new(
        endpoint,
        PeerAddr::Loopback(0),
        ConnectionConfig::new("bench-probe", 76_561_197_960_265_799, &md5_hex(b"bench")),
    );
    conn.set_userinfo("cl_updaterate", "20");
    conn.set_userinfo("rate", "30000");
    conn.connect();

    let bot_count = cfg.players.saturating_sub(1);
    let _ = server.console_command(&format!("bot_quota {bot_count}"));

    let mut now = 0.0;
    for _ in 0..400 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut net, now);
        if conn.state() == &ConnState::InGame {
            break;
        }
    }
    if conn.state() != &ConnState::InGame {
        return Err("benchmark loopback client failed to reach InGame".to_string());
    }

    // Warm up until bots/navmesh are initialized and active in the match.
    for _ in 0..100 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut net, now);
        if bot_count == 0 || (server.game().bot_nav_built() && server.fake_client_count() >= usize::from(bot_count)) {
            break;
        }
    }
    for _ in 0..10 {
        now += 0.01;
        conn.pump(now);
        server.frame(&mut net, now);
    }

    net.reset_metrics();
    let mut tick_samples = Vec::with_capacity(cfg.ticks);
    for _ in 0..cfg.ticks {
        now += 0.01;
        conn.pump(now);
        let t0 = Instant::now();
        server.frame(&mut net, now);
        tick_samples.push(t0.elapsed().as_secs_f64() * 1000.0);
    }

    let sum_ms: f64 = tick_samples.iter().sum();
    let tick_avg_ms = sum_ms / (tick_samples.len().max(1) as f64);
    tick_samples.sort_by(f64::total_cmp);
    let tick_p50_ms = percentile(&tick_samples, 0.50);
    let tick_p95_ms = percentile(&tick_samples, 0.95);
    let tick_p99_ms = percentile(&tick_samples, 0.99);
    let tick_max_ms = tick_samples.last().copied().unwrap_or(0.0);

    let delta_pkt_avg_bytes = if net.tx_packets == 0 {
        0.0
    } else {
        net.tx_bytes as f64 / net.tx_packets as f64
    };
    let sim_secs = (cfg.ticks as f64) * 0.01;
    let bandwidth_kbps_per_client = if sim_secs > 0.0 {
        (net.tx_bytes as f64 * 8.0) / 1000.0 / sim_secs
    } else {
        0.0
    };

    Ok(BenchReport {
        profile,
        map: cfg.map.clone(),
        map_source,
        players: cfg.players,
        ticks: cfg.ticks,
        startup_ms,
        tick_avg_ms,
        tick_p50_ms,
        tick_p95_ms,
        tick_p99_ms,
        tick_max_ms,
        delta_pkt_count: net.tx_packets,
        delta_pkt_avg_bytes,
        delta_pkt_max_bytes: net.max_packet_bytes,
        bandwidth_kbps_per_client,
    })
}

/// CLI entrypoint for `cargo xtask bench`.
pub fn run_bench(args: impl Iterator<Item = String>) -> Result<ExitCode, String> {
    let cfg = parse_bench_args(args)?;
    let r = run_bench_with_config(&cfg)?;
    if cfg.json {
        println!(
            "{{\"profile\":\"{}\",\"map\":\"{}\",\"map_source\":\"{}\",\"players\":{},\"ticks\":{},\"startup_ms\":{:.3},\"tick_avg_ms\":{:.4},\"tick_p50_ms\":{:.4},\"tick_p95_ms\":{:.4},\"tick_p99_ms\":{:.4},\"tick_max_ms\":{:.4},\"delta_pkt_count\":{},\"delta_pkt_avg_bytes\":{:.1},\"delta_pkt_max_bytes\":{},\"bandwidth_kbps_per_client\":{:.2}}}",
            r.profile,
            r.map,
            r.map_source,
            r.players,
            r.ticks,
            r.startup_ms,
            r.tick_avg_ms,
            r.tick_p50_ms,
            r.tick_p95_ms,
            r.tick_p99_ms,
            r.tick_max_ms,
            r.delta_pkt_count,
            r.delta_pkt_avg_bytes,
            r.delta_pkt_max_bytes,
            r.bandwidth_kbps_per_client,
        );
    } else {
        println!("xtask bench (profile: {}, map: {} [{}], players: {}, ticks: {})",
            r.profile, r.map, r.map_source, r.players, r.ticks);
        println!("  startup_readiness_ms   : {:.3} ms", r.startup_ms);
        println!("  server_tick_avg_ms     : {:.4} ms", r.tick_avg_ms);
        println!("  server_tick_p50/95/99  : {:.4} / {:.4} / {:.4} ms (max {:.4} ms)",
            r.tick_p50_ms, r.tick_p95_ms, r.tick_p99_ms, r.tick_max_ms);
        println!("  delta_snapshot_packets : {} pkts, avg {:.1} B, max {} B ({:.2} kbps @ cl_updaterate 20)",
            r.delta_pkt_count, r.delta_pkt_avg_bytes, r.delta_pkt_max_bytes, r.bandwidth_kbps_per_client);
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bench_flags_and_rejects_invalid() {
        let args = vec![
            "--ticks".into(),
            "50".into(),
            "--players".into(),
            "32".into(),
            "--map".into(),
            "cs_office".into(),
            "--stub".into(),
            "--json".into(),
        ];
        let cfg = parse_bench_args(args.into_iter()).unwrap();
        assert_eq!(cfg.ticks, 50);
        assert_eq!(cfg.players, 32);
        assert_eq!(cfg.map, "cs_office");
        assert!(cfg.stub);
        assert!(cfg.json);

        for bad in [
            vec!["--players", "0"],
            vec!["--players", "33"],
            vec!["--ticks", "0"],
            vec!["--unknown"],
        ] {
            let owned: Vec<String> = bad.into_iter().map(String::from).collect();
            assert!(parse_bench_args(owned.into_iter()).is_err());
        }
    }

    #[test]
    fn bench_smoke_measures_32_player_ticks_and_delta_packets() {
        let cfg = BenchConfig {
            ticks: 25,
            players: 32,
            map: "de_dust2".to_string(),
            gamedir: None,
            stub: true,
            json: false,
        };
        let report = run_bench_with_config(&cfg).unwrap();
        assert_eq!(report.players, 32);
        assert_eq!(report.ticks, 25);
        assert!(report.startup_ms >= 0.0);
        assert!(report.tick_avg_ms > 0.0);
        assert!(report.delta_pkt_count > 0);
        assert!(report.delta_pkt_avg_bytes > 0.0);
    }
}
