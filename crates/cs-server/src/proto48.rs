//! `cs-server --proto48`: the Protocol 48 server engine running `WalkGame` on a UDP socket,
//! for the original Counter-Strike 1.6 client. Players spawn and walk; there are no weapons,
//! rounds or HUD.

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cs_assets::bsp::BspMap;
use cs_assets::materials::MaterialTable;
use cs_cvar::CVarRegistry;
use cs_engine::{Server, ServerConfig, ServerMap};
use cs_platform::{Clock, NativeClock};
use cs_proto48::delta::DeltaRegistry;
use cs_sim::pmove::MoveParams;
use cs_sim::trace::BspCollision;
use cs_sim::walk_game::WalkGame;
use cs_transport::UdpTransport;
use cs_vfs::{Vfs, VfsConfig};
use tracing::info;

/// Seconds between server frames: 100 Hz.
const FRAME_SECS: f64 = 0.01;

struct Options {
    gamedir: Option<PathBuf>,
    map: String,
    port: u16,
    max_players: u8,
    hostname: String,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut opts = Options {
        gamedir: None,
        map: "de_dust2".to_string(),
        port: cs_net::DEFAULT_PORT,
        max_players: 16,
        hostname: ServerConfig::default().hostname,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--proto48" => {}
            "--gamedir" | "-g" => opts.gamedir = Some(PathBuf::from(value()?)),
            "--map" | "-m" => opts.map = value()?.clone(),
            "--port" | "-p" => {
                let v = value()?;
                opts.port = v.parse().map_err(|_| format!("{arg}: not a port: {v}"))?;
            }
            "--maxplayers" => {
                let v = value()?;
                opts.max_players = v
                    .parse()
                    .ok()
                    .filter(|n| (1..=32).contains(n))
                    .ok_or(format!("--maxplayers: not a number from 1 to 32: {v}"))?;
            }
            "--hostname" => opts.hostname = value()?.clone(),
            "--bots" | "-b" => {
                value()?;
                info!("--bots is ignored with --proto48: this mode has no bots");
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(opts)
}

/// Runs the server until the process is stopped. A start-up error (bad arguments, map not
/// found, port taken) is printed and ends the process with exit code 1.
pub fn run(args: &[String]) -> ! {
    let Err(msg) = start(args);
    eprintln!("error: {msg}");
    std::process::exit(1);
}

fn start(args: &[String]) -> Result<Infallible, String> {
    let opts = parse(args)?;
    let vfs = Vfs::discover(opts.gamedir.as_deref(), VfsConfig::default())
        .map_err(|e| format!("cannot find the Half-Life / CS 1.6 directory: {e}"))?;
    let bsp =
        BspMap::load(&vfs, &opts.map).map_err(|e| format!("cannot load map {}: {e}", opts.map))?;
    let path = format!("maps/{}.bsp", opts.map);
    let raw = vfs
        .read_bytes(&path)
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    let collision = BspCollision::from_map(&bsp, MaterialTable::load(&vfs));
    let game = WalkGame::new(collision, bsp.spawns.clone(), MoveParams::default());
    let map = ServerMap::from_bsp(&opts.map, &raw, Arc::new(bsp))
        .ok_or(format!("{path} has no valid BSP header"))?;
    let (map_name, map_crc) = (map.name.clone(), map.crc);

    let bind = format!("0.0.0.0:{}", opts.port);
    let mut net = UdpTransport::bind(&bind).map_err(|e| format!("cannot bind UDP {bind}: {e}"))?;
    let cfg = ServerConfig {
        hostname: opts.hostname,
        max_clients: opts.max_players,
        ..ServerConfig::default()
    };
    let mut server = Server::new(
        cfg,
        map,
        game,
        DeltaRegistry::cs16_default(),
        CVarRegistry::new_with_defaults(),
    );
    info!(
        addr = %net.local_addr(),
        map = %map_name,
        crc = format_args!("{map_crc:#010x}"),
        maxplayers = server.config().max_clients,
        hostname = %server.config().hostname,
        "cs-server (Protocol 48, walking only) listening at 100 Hz (Ctrl+C to stop)"
    );

    let clock = NativeClock::new();
    let mut next_frame = clock.now();
    loop {
        server.frame(&mut net, clock.now());
        next_frame += FRAME_SECS;
        let now = clock.now();
        if next_frame > now {
            std::thread::sleep(Duration::from_secs_f64(next_frame - now));
        } else {
            // Behind schedule: start counting again from now instead of catching up.
            next_frame = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_and_flags() {
        let o = parse(&args(&["--proto48"])).unwrap();
        assert_eq!(
            (o.map.as_str(), o.port, o.max_players),
            ("de_dust2", 27015, 16)
        );
        assert_eq!(o.hostname, ServerConfig::default().hostname);

        let o = parse(&args(&[
            "--proto48",
            "--map",
            "cs_office",
            "--port",
            "27115",
            "--maxplayers",
            "8",
            "--hostname",
            "Test",
            "--gamedir",
            "C:/hl",
            "--bots",
            "3",
        ]))
        .unwrap();
        assert_eq!(
            (o.map.as_str(), o.port, o.max_players),
            ("cs_office", 27115, 8)
        );
        assert_eq!(o.hostname, "Test");
        assert_eq!(o.gamedir, Some(PathBuf::from("C:/hl")));
    }

    #[test]
    fn bad_values_are_errors() {
        for bad in [
            &["--maxplayers", "0"][..],
            &["--maxplayers", "33"],
            &["--port", "x"],
            &["--map"],
            &["--frobnicate"],
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
    }
}
