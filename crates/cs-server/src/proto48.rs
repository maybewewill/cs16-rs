//! `cs-server --proto48`: the Protocol 48 server engine running the game on a UDP socket, for
//! the original Counter-Strike 1.6 client. The game is `cs_game::CsGame`; `--walk-game` runs the
//! older `cs_sim::walk_game::WalkGame` instead (players only walk: no weapons, rounds or HUD).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use cs_assets::bsp::BspMap;
use cs_cvar::CVarRegistry;
use cs_engine::{ConsoleOutcome, GameDll, Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_platform::{Clock, NativeClock};
use cs_proto48::delta::DeltaRegistry;
use cs_sim::pmove::MoveParams;
use cs_sim::walk_game::WalkGame;
use cs_transport::{PacketTransport, UdpTransport};
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
    /// Run `WalkGame` instead of `CsGame`.
    walk_game: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut opts = Options {
        gamedir: None,
        map: "de_dust2".to_string(),
        port: cs_net::DEFAULT_PORT,
        max_players: 16,
        hostname: ServerConfig::default().hostname,
        walk_game: false,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--proto48" => {}
            "--walk-game" => opts.walk_game = true,
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

/// Loads `maps/<name>.bsp` through `vfs`, for start-up and for `changelevel`.
fn load_server_map(vfs: &Vfs, name: &str) -> Result<ServerMap, String> {
    let bsp = Arc::new(BspMap::load(vfs, name).map_err(|e| format!("cannot load map {name}: {e}"))?);
    let path = format!("maps/{name}.bsp");
    let raw = vfs
        .read_bytes(&path)
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    ServerMap::from_bsp(name, &raw, bsp).ok_or(format!("{path} has no valid BSP header"))
}

/// Carries out `changelevel <name>`: loads the map with `load_map` and switches the server to
/// it. A failure is printed and the server keeps running on its current map.
fn change_level<G: GameDll>(
    load_map: &mut dyn FnMut(&str) -> Result<ServerMap, String>,
    server: &mut Server<G>,
    name: &str,
) {
    match load_map(name) {
        Ok(map) => {
            println!("changing level to {name}");
            server.change_map(map);
        }
        Err(msg) => println!("changelevel failed: {msg}"),
    }
}

/// The shared cvar defaults with the vanilla game's own where they differ from what the legacy
/// offline client uses: `mp_roundtime` is 5 minutes in ReGameDLL `game.cpp` (the shared registry
/// keeps 1.75 for the offline game). Applied before the first `server_activate`, so a value the
/// operator sets later in the console wins, and the rules read it at the first restart.
fn vanilla_cvars() -> CVarRegistry {
    let mut cvars = CVarRegistry::new_with_defaults();
    cvars.set("mp_roundtime", "5");
    cvars
}

/// Runs the console lines that arrived since the last frame and checks Ctrl+C. Returns `true`
/// once the server was shut down (the caller then exits: `shutdown` already sent the
/// farewells). Closed or silent input is not a request to stop.
///
/// At most one map change happens per call: after a `changelevel` the remaining lines wait for
/// the next frame, so a map change is never followed by more commands in the same frame and
/// Ctrl+C pressed during a slow map load is honoured right after it. Lines after `quit` are
/// not run.
fn run_console<G: GameDll>(
    server: &mut Server<G>,
    net: &mut dyn PacketTransport,
    load_map: &mut dyn FnMut(&str) -> Result<ServerMap, String>,
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
            ConsoleOutcome::ChangeLevel(name) => {
                change_level(load_map, server, &name);
                quit = interrupted.load(Ordering::SeqCst);
                break;
            }
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
    if opts.walk_game {
        let game = WalkGame::new(MoveParams::default());
        serve(&opts, &vfs, map, game, "WalkGame (walking only)")
    } else {
        let mut game = CsGame::new();
        game.load_motd(&vfs);
        game.load_map_cycle(&vfs);
        serve(&opts, &vfs, map, game, "CsGame")
    }
}

/// Binds the socket and runs `game` on `map` until the operator stops the server.
fn serve<G: GameDll>(
    opts: &Options,
    vfs: &Vfs,
    map: ServerMap,
    game: G,
    game_name: &str,
) -> Result<(), String> {
    let (map_name, map_crc) = (map.name.clone(), map.crc);

    let bind = format!("0.0.0.0:{}", opts.port);
    let mut net = UdpTransport::bind(&bind).map_err(|e| format!("cannot bind UDP {bind}: {e}"))?;
    let cfg = ServerConfig {
        hostname: opts.hostname.clone(),
        max_clients: opts.max_players,
        ..ServerConfig::default()
    };
    let mut server = Server::new(
        cfg,
        map,
        game,
        DeltaRegistry::cs16_default(),
        vanilla_cvars(),
    );
    info!(
        addr = %net.local_addr(),
        map = %map_name,
        crc = format_args!("{map_crc:#010x}"),
        maxplayers = server.config().max_clients,
        hostname = %server.config().hostname,
        game = game_name,
        "cs-server (Protocol 48) listening at 100 Hz (type quit or press Ctrl+C to stop)"
    );

    let commands = console_io::spawn_line_reader(std::io::BufReader::new(std::io::stdin()));
    let interrupted = console_io::interrupt_flag();
    let clock = NativeClock::new();
    let mut next_frame = clock.now();
    let mut load_map = |name: &str| load_server_map(vfs, name);
    loop {
        // Server time starts at 0 with the server, so `clock.now()` (seconds since the clock
        // was created, just before this loop) plus one frame makes the first `frametime` one
        // frame long instead of about 0, and later frames get their real elapsed time.
        server.frame(&mut net, clock.now() + FRAME_SECS);
        // A map change the game asked for (`CHANGE_LEVEL`), done as the console's `changelevel`.
        // A change leaves the console lines to the next frame, as a console `changelevel` does.
        let changed = match server.take_pending_change_level() {
            Some(name) => {
                change_level(&mut load_map, &mut server, &name);
                true
            }
            None => false,
        };
        // `shutdown` has already sent every farewell, so there is nothing left to run a frame
        // for.
        if !changed
            && run_console(&mut server, &mut net, &mut load_map, &commands, &interrupted)
        {
            return Ok(());
        }
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

    type WalkServer = Server<WalkGame>;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn vanilla_round_time() {
        // The proto48 server starts at the vanilla 5 minutes, the legacy registry at 1.75.
        assert_eq!(vanilla_cvars().get_f32("mp_roundtime"), 5.0);
        assert_eq!(CVarRegistry::new_with_defaults().get_f32("mp_roundtime"), 1.75);
    }

    #[test]
    fn defaults_and_flags() {
        let o = parse(&args(&["--proto48"])).unwrap();
        assert_eq!(
            (o.map.as_str(), o.port, o.max_players),
            ("de_dust2", 27015, 16)
        );
        assert_eq!(o.hostname, ServerConfig::default().hostname);
        assert!(!o.walk_game);

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
    fn walk_game_flag_selects_the_old_game() {
        assert!(parse(&args(&["--proto48", "--walk-game"])).unwrap().walk_game);
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

    fn stub_server() -> WalkServer {
        Server::new(
            ServerConfig::default(),
            ServerMap::stub("start"),
            WalkGame::new(MoveParams::default()),
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        )
    }

    fn map_of(server: &mut WalkServer) -> String {
        match server.console_command("status") {
            ConsoleOutcome::Output(text) => text
                .lines()
                .find_map(|l| l.strip_prefix("map: "))
                .unwrap_or_default()
                .to_string(),
            _ => String::new(),
        }
    }

    /// What one `run_console` call over a batch of lines did.
    struct Batch {
        quit: bool,
        /// Map names the loader was asked for; names starting with `bad` fail to load.
        loaded: Vec<String>,
        /// The channel, with the lines the call left unread.
        rest: Receiver<String>,
    }

    fn console_once(server: &mut WalkServer, lines: &[&str], interrupted: &AtomicBool) -> Batch {
        let (tx, rest) = std::sync::mpsc::channel();
        for l in lines {
            tx.send(l.to_string()).unwrap();
        }
        let mut loaded = Vec::new();
        let mut net = cs_transport::LoopbackHub::new();
        let mut load = |name: &str| {
            loaded.push(name.to_string());
            if name.starts_with("bad") {
                Err(format!("no map {name}"))
            } else {
                Ok(ServerMap::stub(name))
            }
        };
        let quit = run_console(server, &mut net, &mut load, &rest, interrupted);
        Batch { quit, loaded, rest }
    }

    #[test]
    fn changelevel_then_quit_in_one_batch_changes_first_and_quits_next_frame() {
        let mut server = stub_server();
        let flag = AtomicBool::new(false);
        let b = console_once(&mut server, &["changelevel a", "quit"], &flag);
        assert!(!b.quit, "the quit line waits for the next frame");
        assert_eq!(b.loaded, ["a"]);
        assert_eq!(map_of(&mut server), "a");
        assert_eq!(b.rest.try_recv().as_deref(), Ok("quit"));
    }

    #[test]
    fn two_changelevels_in_one_batch_run_one_per_frame() {
        let mut server = stub_server();
        let flag = AtomicBool::new(false);
        let b = console_once(&mut server, &["changelevel a", "map b"], &flag);
        assert!(!b.quit);
        assert_eq!(b.loaded, ["a"]);
        assert_eq!(map_of(&mut server), "a");
        // The next frame picks up the second line.
        let mut net = cs_transport::LoopbackHub::new();
        let mut load = |name: &str| Ok(ServerMap::stub(name));
        assert!(!run_console(&mut server, &mut net, &mut load, &b.rest, &flag));
        assert_eq!(map_of(&mut server), "b");
    }

    #[test]
    fn a_failed_load_keeps_the_old_map() {
        let mut server = stub_server();
        let flag = AtomicBool::new(false);
        let b = console_once(&mut server, &["changelevel bad1"], &flag);
        assert!(!b.quit);
        assert_eq!(b.loaded, ["bad1"]);
        assert_eq!(map_of(&mut server), "start");
    }

    #[test]
    fn lines_after_quit_are_not_run() {
        let mut server = stub_server();
        let flag = AtomicBool::new(false);
        let b = console_once(&mut server, &["quit", "changelevel a"], &flag);
        assert!(b.quit);
        assert!(b.loaded.is_empty());
        assert_eq!(b.rest.try_recv().as_deref(), Ok("changelevel a"));
    }

    #[test]
    fn ctrl_c_during_a_load_quits_right_after_it() {
        let mut server = stub_server();
        let flag = AtomicBool::new(false);
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send("changelevel a".to_string()).unwrap();
        let mut net = cs_transport::LoopbackHub::new();
        let mut load = |name: &str| {
            flag.store(true, Ordering::SeqCst); // Ctrl+C arrives while the map loads.
            Ok(ServerMap::stub(name))
        };
        assert!(run_console(&mut server, &mut net, &mut load, &rx, &flag));
        assert_eq!(map_of(&mut server), "a");
    }

    #[test]
    fn ctrl_c_before_the_batch_runs_none_of_it() {
        let mut server = stub_server();
        let flag = AtomicBool::new(true);
        let b = console_once(&mut server, &["changelevel a"], &flag);
        assert!(b.quit);
        assert!(b.loaded.is_empty());
    }
}
