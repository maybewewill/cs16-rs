//! The Protocol 48 server engine running the game on a UDP socket, for the original
//! Counter-Strike 1.6 client and for `cs-client`. The game is `cs_game::CsGame`; `--walk-game` runs the
//! older `cs_sim::walk_game::WalkGame` instead (players only walk: no weapons, rounds or HUD).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use cs_engine::{ConsoleOutcome, GameDll, Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_local::{load_server_map, new_server};
use cs_platform::{Clock, NativeClock};
use cs_relay::{RelayHandle, RelayPolicy, TargetAllowlist};
use cs_sim::pmove::MoveParams;
use cs_sim::walk_game::WalkGame;
use cs_transport::{
    MultiTransport, PacketTransport, PeerAddr, PeerKind, TransportError, UdpTransport,
    UpgradeHook, WsServerConfig, WsServerTransport,
};
use cs_vfs::{Vfs, VfsConfig};
use tracing::info;

use crate::console_io;

/// The port a Half-Life server listens on by default.
const DEFAULT_PORT: u16 = 27015;

/// Seconds between server frames: 100 Hz.
const FRAME_SECS: f64 = 0.01;

struct Options {
    gamedir: Option<PathBuf>,
    map: String,
    port: u16,
    max_players: u8,
    hostname: String,
    bots: Option<u8>,
    /// Run `WalkGame` instead of `CsGame`.
    walk_game: bool,
    web_port: Option<u16>,
    web_dir: Option<PathBuf>,
    relay_allow: Vec<SocketAddr>,
    mods_dir: Option<PathBuf>,
    amxx_dir: Option<PathBuf>,
    masters: Vec<SocketAddr>,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut opts = Options {
        gamedir: None,
        map: "de_dust2".to_string(),
        port: DEFAULT_PORT,
        max_players: 16,
        hostname: ServerConfig::default().hostname,
        bots: None,
        walk_game: false,
        web_port: None,
        web_dir: None,
        relay_allow: Vec::new(),
        mods_dir: None,
        amxx_dir: None,
        masters: Vec::new(),
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
                let v = value()?;
                let count = v
                    .parse()
                    .ok()
                    .filter(|n: &u8| *n <= 32)
                    .ok_or(format!("{arg}: not a number from 0 to 32: {v}"))?;
                opts.bots = Some(count);
            }
            "--web-port" => {
                let v = value()?;
                let p: u16 = v
                    .parse()
                    .map_err(|_| format!("{arg}: not a port: {v}"))?;
                opts.web_port = Some(p);
            }
            "--web-dir" => opts.web_dir = Some(PathBuf::from(value()?)),
            "--mods-dir" => opts.mods_dir = Some(PathBuf::from(value()?)),
            "--amxx-dir" => opts.amxx_dir = Some(PathBuf::from(value()?)),
            "--master" => {
                let v = value()?;
                let addr: SocketAddr = v
                    .parse()
                    .map_err(|e| format!("{arg}: invalid <ip:port> `{v}`: {e}"))?;
                opts.masters.push(addr);
            }
            "--relay-allow" => {
                let v = value()?;
                let addr: SocketAddr = v
                    .parse()
                    .map_err(|e| format!("{arg}: invalid <ip:port> `{v}`: {e}"))?;
                opts.relay_allow.push(addr);
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    if !opts.relay_allow.is_empty() && opts.web_port.is_none() {
        return Err("--relay-allow requires --web-port".to_string());
    }
    if opts.amxx_dir.is_some() && opts.walk_game {
        return Err("--amxx-dir cannot be combined with --walk-game".to_string());
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

fn load_mods_into_vfs(mods_dir: &Path, vfs: &mut Vfs) -> Result<cs_mod_host::ServerModHost, String> {
    let packages = cs_mod_host::discover_packages(mods_dir)
        .map_err(|e| format!("failed to scan mods dir `{}`: {e}", mods_dir.display()))?;
    let mut host = cs_mod_host::ServerModHost::new();
    host.set_mods_dir(mods_dir);
    let mut loaded = 0usize;
    for pkg in packages {
        if !pkg.assets.is_empty() {
            let entries: Vec<(String, Vec<u8>)> = pkg
                .assets
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            vfs.mount_mod_assets(&pkg.manifest.id, entries)
                .map_err(|e| format!("failed to mount assets for mod `{}`: {e}", pkg.manifest.id))?;
        }
        let id = pkg.manifest.id.clone();
        host.load_package(pkg)
            .map_err(|e| format!("failed to load mod `{id}`: {e}"))?;
        loaded += 1;
    }
    info!(dir = %mods_dir.display(), count = loaded, "loaded server mods");
    Ok(host)
}

fn load_amxx_plugins(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("failed to scan amxx dir `{}`: {e}", dir.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read amxx dir `{}`: {e}", dir.display()))?;
        let path = entry.path();
        let is_amxx = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("amxx") || e.eq_ignore_ascii_case("amx"));
        if path.is_file() && is_amxx {
            paths.push(path);
        }
    }
    paths.sort();
    let mut plugins = Vec::with_capacity(paths.len());
    for path in paths {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("plugin.amxx")
            .to_string();
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("failed to read AMXX plugin `{}`: {e}", path.display()))?;
        plugins.push((name, bytes));
    }
    info!(dir = %dir.display(), count = plugins.len(), "loaded AMXX plugins");
    Ok(plugins)
}

fn start(args: &[String]) -> Result<(), String> {
    let opts = parse(args)?;
    let mut vfs = Vfs::discover(opts.gamedir.as_deref(), VfsConfig::default())
        .map_err(|e| format!("cannot find the Half-Life / CS 1.6 directory: {e}"))?;
    let map = load_server_map(&vfs, &opts.map)?;
    let host = match &opts.mods_dir {
        Some(dir) => Some(load_mods_into_vfs(dir, &mut vfs)?),
        None => None,
    };
    if opts.walk_game {
        let game = WalkGame::new(MoveParams::default());
        match host {
            Some(h) => serve(&opts, &vfs, map, cs_mod_host::ModdedGame::new(game, h), "WalkGame + Mods"),
            None => serve(&opts, &vfs, map, game, "WalkGame (walking only)"),
        }
    } else {
        let mut game = CsGame::new();
        game.load_motd(&vfs);
        game.load_map_cycle(&vfs);
        game.enable_cvar_bot_quota();
        match (&opts.amxx_dir, host) {
            (Some(dir), Some(h)) => {
                let amxx = cs_amxx::AmxxGame::new(game, load_amxx_plugins(dir)?);
                serve(
                    &opts,
                    &vfs,
                    map,
                    cs_mod_host::ModdedGame::new(amxx, h),
                    "CsGame + AMXX + Mods",
                )
            }
            (Some(dir), None) => {
                let amxx = cs_amxx::AmxxGame::new(game, load_amxx_plugins(dir)?);
                serve(&opts, &vfs, map, amxx, "CsGame + AMXX")
            }
            (None, Some(h)) => {
                serve(&opts, &vfs, map, cs_mod_host::ModdedGame::new(game, h), "CsGame + Mods")
            }
            (None, None) => serve(&opts, &vfs, map, game, "CsGame"),
        }
    }
}

enum ServerTransport {
    Udp(UdpTransport),
    Multi(MultiTransport),
}

impl PacketTransport for ServerTransport {
    fn recv(&mut self) -> Option<(PeerAddr, Vec<u8>)> {
        match self {
            Self::Udp(t) => t.recv(),
            Self::Multi(m) => m.recv(),
        }
    }

    fn send(&mut self, to: PeerAddr, data: &[u8]) -> Result<(), TransportError> {
        match self {
            Self::Udp(t) => t.send(to, data),
            Self::Multi(m) => m.send(to, data),
        }
    }
}

fn default_web_dir() -> Option<PathBuf> {
    let dist = Path::new("web/dist");
    if dist.is_dir() {
        return Some(dist.to_path_buf());
    }
    let plain = Path::new("web");
    plain.is_dir().then(|| plain.to_path_buf())
}

/// Binds the socket (and optional HTTP/WebSocket host) and runs `game` on `map`.
fn serve<G: GameDll>(
    opts: &Options,
    vfs: &Vfs,
    map: ServerMap,
    game: G,
    game_name: &str,
) -> Result<(), String> {
    let (map_name, map_crc) = (map.name.clone(), map.crc);

    let bind = format!("0.0.0.0:{}", opts.port);
    let udp = UdpTransport::bind(&bind).map_err(|e| format!("cannot bind UDP {bind}: {e}"))?;
    let udp_addr = udp.local_addr();
    let mut net = if let Some(web_port) = opts.web_port {
        let upgrade_hook: Option<UpgradeHook> = if !opts.relay_allow.is_empty() {
            let policy = RelayPolicy {
                allowlist: TargetAllowlist::from_addrs(opts.relay_allow.iter().copied()),
                ..RelayPolicy::default()
            };
            let relay = RelayHandle::new(policy);
            Some(Arc::new(move |req, stream, peer| {
                relay.handle_http_upgrade(req, stream, peer);
            }))
        } else {
            None
        };
        let static_dir = opts.web_dir.clone().or_else(default_web_dir);
        let ws_srv = WsServerTransport::bind(WsServerConfig {
            bind_addr: format!("0.0.0.0:{web_port}"),
            static_dir: static_dir.clone(),
            ws_path: "/ws".to_string(),
            max_clients: usize::from(opts.max_players) + 8,
            upgrade_hook,
        })
        .map_err(|e| format!("cannot bind WebSocket/HTTP on 0.0.0.0:{web_port}: {e}"))?;
        let ws_addr = ws_srv.local_addr();
        info!(
            http = %format!("http://127.0.0.1:{}", ws_addr.port()),
            ws = %format!("ws://127.0.0.1:{}/ws", ws_addr.port()),
            static_dir = ?static_dir,
            relay_targets = opts.relay_allow.len(),
            "Web client hosting and WebSocket transport enabled"
        );
        let mut multi = MultiTransport::new();
        multi.add(PeerKind::Udp, udp);
        multi.add(PeerKind::Web, ws_srv);
        ServerTransport::Multi(multi)
    } else {
        ServerTransport::Udp(udp)
    };

    let cfg = ServerConfig {
        hostname: opts.hostname.clone(),
        max_clients: opts.max_players,
        ..ServerConfig::default()
    };
    let mut server = new_server(cfg, map, game, vfs);
    if let Some(n) = opts.bots {
        let _ = server.console_command(&format!("bot_quota {n}"));
    }
    for addr in &opts.masters {
        let _ = server.console_command(&format!("setmaster add {addr}"));
    }
    info!(
        addr = %udp_addr,
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
    use cs_cvar::CVarRegistry;
    use cs_proto48::delta::DeltaRegistry;

    type WalkServer = Server<WalkGame>;

    /// A server built as `serve` builds it knows the decals of the installed game.
    #[test]
    fn server_knows_the_decals_of_the_game() {
        let Ok(vfs) = Vfs::discover(None, VfsConfig::default()) else {
            eprintln!("SKIPPED: no game directory (set CS16_GAMEDIR)");
            return;
        };
        if !vfs.root().join("valve").join("decals.wad").is_file()
            && !vfs.root().join("cstrike").join("decals.wad").is_file()
        {
            eprintln!("SKIPPED: the game directory has no decals.wad");
            return;
        }
        let server = new_server(
            ServerConfig::default(),
            ServerMap::stub("start"),
            WalkGame::new(MoveParams::default()),
            &vfs,
        );
        assert!(server.decal_index("{shot1") >= 0);
        assert!(server.decal_index("{blood1") >= 0);
        assert_eq!(server.decal_index("{no such decal"), -1);
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_and_flags() {
        let o = parse(&args(&["--proto48"])).unwrap();
        assert_eq!(
            (o.map.as_str(), o.port, o.max_players, o.bots),
            ("de_dust2", 27015, 16, None)
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
            (o.map.as_str(), o.port, o.max_players, o.bots),
            ("cs_office", 27115, 8, Some(3))
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
            &["--bots", "33"],
            &["--bots", "x"],
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

    #[test]
    fn parses_web_and_relay_flags() {
        let o = parse(&args(&[
            "--web-port",
            "27120",
            "--web-dir",
            "web",
            "--relay-allow",
            "127.0.0.1:27016",
        ]))
        .unwrap();
        assert_eq!(o.web_port, Some(27120));
        assert_eq!(o.web_dir, Some(PathBuf::from("web")));
        assert_eq!(
            o.relay_allow,
            vec!["127.0.0.1:27016".parse::<SocketAddr>().unwrap()]
        );

        assert!(parse(&args(&["--relay-allow", "127.0.0.1:27016"])).is_err());
    }

    #[test]
    fn udp_and_websocket_clients_play_in_the_same_server() {
        use cs_clientcore::{ConnState, Connection, ConnectionConfig};
        use cs_proto48::crypto::md5_hex;
        use cs_transport::WsClientTransport;

        let udp = UdpTransport::bind("127.0.0.1:0").unwrap();
        let udp_addr = udp.local_addr();
        let ws_srv = WsServerTransport::bind(WsServerConfig {
            bind_addr: "127.0.0.1:0".into(),
            static_dir: None,
            ws_path: "/ws".into(),
            max_clients: 8,
            upgrade_hook: None,
        })
        .unwrap();
        let ws_addr = ws_srv.local_addr();

        let mut multi = MultiTransport::new();
        multi.add(PeerKind::Udp, udp);
        multi.add(PeerKind::Web, ws_srv);

        let mut server = Server::new(
            ServerConfig {
                max_clients: 8,
                ..ServerConfig::default()
            },
            ServerMap::stub("de_dust2"),
            WalkGame::new(MoveParams::default()),
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );

        let mut udp_conn = Connection::new(
            UdpTransport::bind("127.0.0.1:0").unwrap(),
            PeerAddr::Udp(udp_addr),
            ConnectionConfig::new("udp-player", 76_561_197_960_265_740, &md5_hex(b"udp")),
        );
        let mut ws_conn = Connection::new(
            WsClientTransport::connect(&format!("ws://{ws_addr}/ws")).unwrap(),
            PeerAddr::Web(0),
            ConnectionConfig::new("ws-player", 76_561_197_960_265_741, &md5_hex(b"ws")),
        );
        udp_conn.connect();
        ws_conn.connect();

        let mut now = 0.0;
        for _ in 0..800 {
            now += 0.01;
            udp_conn.pump(now);
            ws_conn.pump(now);
            server.frame(&mut multi, now);
            if udp_conn.state() == &ConnState::InGame && ws_conn.state() == &ConnState::InGame {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        assert_eq!(udp_conn.state(), &ConnState::InGame);
        assert_eq!(ws_conn.state(), &ConnState::InGame);
        assert_eq!(udp_conn.stats().parse_errors, 0);
        assert_eq!(ws_conn.stats().parse_errors, 0);
    }

    #[test]
    fn parses_mods_dir_flag() {
        let o = parse(&args(&["--mods-dir", "mods"])).unwrap();
        assert_eq!(o.mods_dir, Some(PathBuf::from("mods")));
    }

    #[test]
    fn parses_repeatable_master_flag() {
        let o = parse(&args(&[
            "--master",
            "127.0.0.1:27010",
            "--master",
            "127.0.0.1:27011",
        ]))
        .unwrap();
        assert_eq!(
            o.masters,
            vec![
                "127.0.0.1:27010".parse::<SocketAddr>().unwrap(),
                "127.0.0.1:27011".parse::<SocketAddr>().unwrap(),
            ]
        );
        assert!(parse(&args(&["--master", "not-an-addr"])).is_err());
    }

    #[test]
    fn parses_amxx_dir_flag_and_rejects_walk_game() {
        let o = parse(&args(&["--amxx-dir", "addons/amxmodx/plugins"])).unwrap();
        assert_eq!(o.amxx_dir, Some(PathBuf::from("addons/amxmodx/plugins")));
        assert!(parse(&args(&["--walk-game", "--amxx-dir", "plugins"])).is_err());
    }

    #[test]
    fn modded_server_runs_reference_mod_for_clients() {
        use cs_clientcore::{ConnState, Connection, ConnectionConfig};
        use cs_mod_api::{MOD_CAPABILITY_KEY, USERMSG_MOD_MANIFEST};
        use cs_proto48::crypto::md5_hex;

        let mods_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../mods");
        let mut vfs = Vfs::from_layered_memory(Vec::new(), VfsConfig::default());
        let host = load_mods_into_vfs(&mods_dir, &mut vfs).unwrap();
        assert!(vfs.read_bytes("sprites/mod_badge.txt").is_ok());

        let modded_game = cs_mod_host::ModdedGame::new(WalkGame::new(MoveParams::default()), host);
        let mut server = Server::new(
            ServerConfig {
                max_clients: 4,
                ..ServerConfig::default()
            },
            ServerMap::stub("de_dust2"),
            modded_game,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );

        let mut srv_udp = UdpTransport::bind("127.0.0.1:0").unwrap();
        let srv_addr = srv_udp.local_addr();
        let cli_udp = UdpTransport::bind("127.0.0.1:0").unwrap();
        let mut conn = Connection::new(
            cli_udp,
            PeerAddr::Udp(srv_addr),
            ConnectionConfig::new("mod-player", 76_561_197_960_265_745, &md5_hex(b"mod")),
        );
        conn.set_userinfo(MOD_CAPABILITY_KEY, "1");
        conn.connect();

        let mut saw_manifest = false;
        let mut now = 0.0;
        for _ in 0..400 {
            now += 0.01;
            conn.pump(now);
            server.frame(&mut srv_udp, now);
            while let Some(um) = conn.fx_mut().hud.unknown_messages.pop_front() {
                if um.name == USERMSG_MOD_MANIFEST {
                    saw_manifest = true;
                }
            }
            if conn.state() == &ConnState::InGame && saw_manifest {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(conn.state(), &ConnState::InGame);
        assert!(saw_manifest, "mod-capable client should receive CsModManifest");
    }
}
