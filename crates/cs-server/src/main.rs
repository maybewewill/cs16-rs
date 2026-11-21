//! Dedicated headless 100 Hz Counter-Strike 1.6 Rust server (`cs-server`).
//!
//! Run with: `cargo run --release -p cs-server -- --map de_dust2 --port 27015 --bots 9`
//!
//! With `--proto48` it runs the Protocol 48 server for the original client instead (see
//! `proto48.rs`): `cargo run -p cs-server -- --proto48 --map de_dust2` runs the game of
//! `cs-game`; add `--walk-game` for the older walking-only `WalkGame` of `cs-sim`.

mod console_io;
mod proto48;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cs_assets::bsp::BspMap;
use cs_assets::materials::MaterialTable;
use cs_net::{DEFAULT_PORT, NetServer};
use cs_sim::rules::{MatchConfig, MatchWorld, TICK_DT};
use cs_vfs::{Vfs, VfsConfig};
use tracing::info;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let all_args: Vec<String> = std::env::args().skip(1).collect();
    if all_args.iter().any(|a| a == "--proto48") {
        proto48::run(&all_args);
    }

    let mut args = std::env::args().skip(1);
    let mut gamedir: Option<PathBuf> = None;
    let mut map_name = "de_dust2".to_string();
    let mut port: u16 = DEFAULT_PORT;
    let mut bots: usize = 9;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--gamedir" | "-g" => gamedir = args.next().map(PathBuf::from),
            "--map" | "-m" => {
                if let Some(m) = args.next() {
                    map_name = m;
                }
            }
            "--port" | "-p" => {
                if let Some(p) = args.next().and_then(|s| s.parse().ok()) {
                    port = p;
                }
            }
            "--bots" | "-b" => {
                if let Some(b) = args.next().and_then(|s| s.parse().ok()) {
                    bots = b;
                }
            }
            _ => {}
        }
    }

    let vfs = Vfs::discover(gamedir.as_deref(), VfsConfig::default())
        .expect("locate Half-Life / CS 1.6 directory");
    let map = BspMap::load(&vfs, &map_name).expect("load BSP map");
    let mats = MaterialTable::load(&vfs);

    let mut world = MatchWorld::new(&map, mats, MatchConfig::default());
    world.set_bot_quota(bots);
    world.start_new_round();

    let bind = format!("0.0.0.0:{port}");
    let mut server = NetServer::bind(&bind).expect("bind UDP server socket");

    info!(
        map = %map_name,
        port,
        bots,
        nav_nodes = world.nav.nodes.len(),
        "cs-server running at 100 Hz (Ctrl+C to stop)"
    );

    let tick_dur = Duration::from_secs_f32(TICK_DT);
    let mut next_tick = Instant::now();
    loop {
        server.tick(&mut world, TICK_DT);
        next_tick += tick_dur;
        if let Some(wait) = next_tick.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        } else {
            next_tick = Instant::now();
        }
    }
}
