//! `cs-server --proto48`: the Protocol 48 server engine running `WalkGame` on a UDP socket,
//! for the original Counter-Strike 1.6 client. Players spawn and walk; there are no weapons,
//! rounds or HUD.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use cs_assets::bsp::BspMap;
use cs_cvar::CVarRegistry;
use cs_engine::{ConsoleOutcome, Server, ServerConfig, ServerMap};
use cs_platform::{Clock, NativeClock};
use cs_proto48::delta::DeltaRegistry;
use cs_sim::pmove::MoveParams;
use cs_sim::walk_game::WalkGame;
use cs_transport::UdpTransport;
use cs_vfs::{Vfs, VfsConfig};
use tracing::info;

use crate::console_io;

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

/// Runs the server until the operator types `quit` or `exit` or presses Ctrl+C, then exits
/// with code 0 after telling every client why. A start-up error (bad arguments, map not found,
/// port taken) is printed and ends the process with exit code 1.
pub fn run(args: &[String]) -> ! {
    match start(args) {
        Ok(()) => std::process::exit(0),
        Err(msg) => {
            eprintln!("error: {msg}");
            std::process::exit(1);
        }
    }
}

type WalkServer = Server<WalkGame>;

/// Loads `maps/<name>.bsp` through `vfs`, for start-up and for `changelevel`.
fn load_server_map(vfs: &Vfs, name: &str) -> Result<ServerMap, String> {
    let bsp = Arc::new(BspMap::load(vfs, name).map_err(|e| format!("cannot load map {name}: {e}"))?);
    let path = format!("maps/{name}.bsp");
    let raw = vfs
        .read_bytes(&path)
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    ServerMap::from_bsp(name, &raw, bsp).ok_or(format!("{path} has no valid BSP header"))
}

/// Carries out `changelevel <name>`: loads the map and switches the server to it. A failure is
/// printed and the server keeps running on its current map.
fn change_level(vfs: &Vfs, server: &mut WalkServer, name: &str) {
    match load_server_map(vfs, name) {
        Ok(map) => {
            println!("changing level to {name}");
            server.change_map(map);
        }
        Err(msg) => println!("changelevel failed: {msg}"),
    }
}

/// Runs the console lines that arrived since the last frame and checks Ctrl+C. Returns `true`
/// once the server was shut down (the caller runs one more frame and exits). Closed or silent
/// input is not a request to stop.
fn run_console(
    server: &mut WalkServer,
    net: &mut UdpTransport,
    vfs: &Vfs,
    commands: &Receiver<String>,
    interrupted: &AtomicBool,
) -> bool {
    let mut quit = interrupted.load(Ordering::SeqCst);
    while !quit {
        let Ok(line) = commands.try_recv() else {
            break;
        };
        match server.console_command(&line) {
            ConsoleOutcome::Output(text) => {
                if !text.is_empty() {
                    print!("{text}");
                }
            }
            ConsoleOutcome::ChangeLevel(name) => change_level(vfs, server, &name),
            ConsoleOutcome::Quit => quit = true,
        }
    }
    if quit {
        info!("shutting down");
        server.shutdown(net, "Server shutting down");
    }
    quit
}

fn start(args: &[String]) -> Result<(), String> {
    let opts = parse(args)?;
    let vfs = Vfs::discover(opts.gamedir.as_deref(), VfsConfig::default())
        .map_err(|e| format!("cannot find the Half-Life / CS 1.6 directory: {e}"))?;
    let map = load_server_map(&vfs, &opts.map)?;
    let game = WalkGame::new(MoveParams::default());
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
        "cs-server (Protocol 48, walking only) listening at 100 Hz (type quit or press Ctrl+C to stop)"
    );

    let commands = console_io::spawn_line_reader(std::io::BufReader::new(std::io::stdin()));
    let interrupted = console_io::interrupt_flag();
    let clock = NativeClock::new();
    let mut next_frame = clock.now();
    let mut quitting = false;
    loop {
        server.frame(&mut net, clock.now());
        if quitting {
            // One frame after `shutdown`, so the farewells are out; then done.
            return Ok(());
        }
        quitting = run_console(&mut server, &mut net, &vfs, &commands, &interrupted);
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
