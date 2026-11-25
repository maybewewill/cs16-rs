//! The Protocol 48 server engine running the game on a UDP socket, for the original
//! Counter-Strike 1.6 client and for `cs-client`. The game is `cs_game::CsGame`; `--walk-game` runs the
//! older `cs_sim::walk_game::WalkGame` instead (players only walk: no weapons, rounds or HUD).

use std::collections::HashMap;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use cs_engine::collision::Hull;
use cs_engine::{
    ConsoleOutcome, EdictId, GameDll, GameStatEvent, Server, ServerConfig, ServerMap,
    WorldCollision,
};
use cs_game::CsGame;
use cs_local::{load_server_map, new_server};
use cs_platform::{Clock, NativeClock};
use cs_proto48::messages::{SvcMessage, SvcParseContext, parse_svc_stream_partial};
use cs_proto48::usermsg::CsUserMsg;
use cs_relay::{RelayHandle, RelayPolicy, TargetAllowlist};
use cs_sim::pmove::MoveParams;
use cs_sim::walk_game::WalkGame;
use cs_transport::{
    HttpHook, MultiTransport, PacketTransport, PeerAddr, PeerKind, TransportError, UdpTransport,
    UpgradeHook, WsServerConfig, WsServerTransport,
};
use cs_transport::ws_proto::HttpRequest;
use cs_vfs::{Vfs, VfsConfig};
use tracing::info;

use crate::admin_http::{
    AdminBackend, AdminHttpRequest, AdminHttpResponse, AdminRouter, InviteConfig,
    MAX_API_BODY_BYTES, PlayerSnapshot, ServerStatusSnapshot, handle_lobby_request, is_lobby_path,
    read_http_body,
};
use cs_master::lobbies::LobbyRegistry;
use crate::console_io;
use crate::replays::{
    DEFAULT_MAX_REPLAY_ROUNDS, MapOverviewBundle, MatchStatsTracker, RoundReplayStore,
    RoundWinner, TeamSide, TimelineBombState, TimelineGrenadeKind, TimelinePlayerSample,
    escape_json_str, load_map_overview_from_vfs, valid_map_overview_name,
};

/// The port a Half-Life server listens on by default.
const DEFAULT_PORT: u16 = 27015;

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
    admin: bool,
    replay_rounds: usize,
    embed_web: bool,
    relay_allow: Vec<SocketAddr>,
    mods_dir: Option<PathBuf>,
    amxx_dir: Option<PathBuf>,
    masters: Vec<SocketAddr>,
    sys_ticrate: Option<f64>,
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
        admin: false,
        replay_rounds: DEFAULT_MAX_REPLAY_ROUNDS,
        embed_web: false,
        relay_allow: Vec::new(),
        mods_dir: None,
        amxx_dir: None,
        masters: Vec::new(),
        sys_ticrate: None,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--proto48" => {}
            "--walk-game" => opts.walk_game = true,
            "--admin" => opts.admin = true,
            "--embed-web" => opts.embed_web = true,
            "--replay-rounds" => {
                let v = value()?;
                opts.replay_rounds = v
                    .parse()
                    .map_err(|_| format!("{arg}: not a non-negative integer: {v}"))?;
            }
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
            "-sys_ticrate" | "--sys-ticrate" => {
                let v = value()?;
                let rate: f64 = v
                    .parse()
                    .ok()
                    .filter(|r: &f64| r.is_finite() && *r > 0.0)
                    .ok_or_else(|| format!("{arg}: not a positive number: {v}"))?;
                opts.sys_ticrate = Some(rate);
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

const EMBEDDED_INDEX_HTML: &[u8] = include_bytes!("../../../web/index.html");
const EMBEDDED_APP_JS: &[u8] = include_bytes!("../../../web/app.js");
const EMBEDDED_VFS_LOADER_JS: &[u8] = include_bytes!("../../../web/vfs_loader.js");
const EMBEDDED_TRANSPORT_JS: &[u8] = include_bytes!("../../../web/transport.js");
const EMBEDDED_INPUT_JS: &[u8] = include_bytes!("../../../web/input.js");
const EMBEDDED_AUDIO_JS: &[u8] = include_bytes!("../../../web/audio.js");
const EMBEDDED_SW_JS: &[u8] = include_bytes!("../../../web/sw.js");
const EMBEDDED_MANIFEST: &[u8] = include_bytes!("../../../web/manifest.webmanifest");
const EMBEDDED_ADMIN_HTML: &[u8] = include_bytes!("../../../web/admin.html");
const EMBEDDED_ADMIN_JS: &[u8] = include_bytes!("../../../web/admin.js");

fn embedded_web_asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    match path {
        "/" | "/index.html" => Some(("text/html; charset=utf-8", EMBEDDED_INDEX_HTML)),
        "/app.js" => Some(("text/javascript; charset=utf-8", EMBEDDED_APP_JS)),
        "/vfs_loader.js" => Some(("text/javascript; charset=utf-8", EMBEDDED_VFS_LOADER_JS)),
        "/transport.js" => Some(("text/javascript; charset=utf-8", EMBEDDED_TRANSPORT_JS)),
        "/input.js" => Some(("text/javascript; charset=utf-8", EMBEDDED_INPUT_JS)),
        "/audio.js" => Some(("text/javascript; charset=utf-8", EMBEDDED_AUDIO_JS)),
        "/sw.js" => Some(("text/javascript; charset=utf-8", EMBEDDED_SW_JS)),
        "/manifest.webmanifest" => Some(("application/manifest+json; charset=utf-8", EMBEDDED_MANIFEST)),
        "/admin" | "/admin.html" => Some(("text/html; charset=utf-8", EMBEDDED_ADMIN_HTML)),
        "/admin.js" => Some(("text/javascript; charset=utf-8", EMBEDDED_ADMIN_JS)),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct TrackedPlayerMeta {
    name: String,
    team: TeamSide,
    frags: i32,
    deaths: i32,
    is_bot: bool,
}

fn weapon_id_from_p_model(path: &str) -> u8 {
    let norm = path.replace('\\', "/").to_ascii_lowercase();
    let file = norm.rsplit('/').next().unwrap_or(&norm);
    match file {
        "p_p228.mdl" => 1,
        "p_scout.mdl" => 3,
        "p_hegrenade.mdl" => 4,
        "p_xm1014.mdl" => 5,
        "p_c4.mdl" => 6,
        "p_mac10.mdl" => 7,
        "p_aug.mdl" => 8,
        "p_smokegrenade.mdl" => 9,
        "p_elite.mdl" => 10,
        "p_fiveseven.mdl" => 11,
        "p_ump45.mdl" => 12,
        "p_sg550.mdl" => 13,
        "p_galil.mdl" => 14,
        "p_famas.mdl" => 15,
        "p_usp.mdl" => 16,
        "p_glock18.mdl" => 17,
        "p_awp.mdl" => 18,
        "p_mp5.mdl" => 19,
        "p_m249.mdl" => 20,
        "p_m3.mdl" => 21,
        "p_m4a1.mdl" => 22,
        "p_tmp.mdl" => 23,
        "p_g3sg1.mdl" => 24,
        "p_flashbang.mdl" => 25,
        "p_deagle.mdl" => 26,
        "p_sg552.mdl" => 27,
        "p_ak47.mdl" => 28,
        "p_knife.mdl" => 29,
        "p_p90.mdl" => 30,
        "p_shield.mdl" => 99,
        _ => 0,
    }
}

const fn is_firearm_weapon_id(id: u8) -> bool {
    !matches!(id, 0 | 4 | 6 | 9 | 25 | 29 | 99)
}

fn grenade_kind_from_model(model: &str) -> Option<TimelineGrenadeKind> {
    let lower = model.to_ascii_lowercase();
    if lower.contains("hegrenade") {
        Some(TimelineGrenadeKind::He)
    } else if lower.contains("flashbang") {
        Some(TimelineGrenadeKind::Flash)
    } else if lower.contains("smokegrenade") {
        Some(TimelineGrenadeKind::Smoke)
    } else {
        None
    }
}

/// Live server-side replay store + match stats + player scoreboard cache for `AdminBackend`.
struct Phase13Runtime {
    replays: RoundReplayStore,
    replays_enabled: bool,
    stats: MatchStatsTracker,
    players: HashMap<u8, TrackedPlayerMeta>,
    started_at: f64,
    round_started_at: f64,
    game_port: u16,
    web_port: Option<u16>,
    available_maps: Vec<String>,
    admin_html: Option<Vec<u8>>,
    vfs: Option<Vfs>,
    world_collision: Option<Arc<WorldCollision>>,
    current_map_name: String,
    current_map_overview: Option<MapOverviewBundle>,
    svc_ctx: SvcParseContext,
    p_model_weapons: HashMap<u16, u8>,
    active_grenades: HashMap<EdictId, (u32, TimelineGrenadeKind, [f32; 3], bool)>,
    next_grenade_key: u32,
    prev_animtime: [f32; 32],
    last_shot_at: [f64; 32],
    flashed_until: [f64; 32],
    flashed_alpha: [u8; 32],
    last_bomb_pos: [f32; 3],
    lobbies: LobbyRegistry,
}

impl Phase13Runtime {
    fn new(
        map_name: &str,
        replay_rounds: usize,
        started_at: f64,
        game_port: u16,
        web_port: Option<u16>,
        available_maps: Vec<String>,
        static_dir: Option<&Path>,
    ) -> Self {
        let admin_html = static_dir
            .and_then(|d| std::fs::read(d.join("admin.html")).ok())
            .or_else(|| Some(EMBEDDED_ADMIN_HTML.to_vec()));
        Self {
            replays: RoundReplayStore::new(replay_rounds.max(1)),
            replays_enabled: replay_rounds > 0,
            stats: MatchStatsTracker::new(map_name),
            players: HashMap::new(),
            started_at,
            round_started_at: started_at,
            game_port,
            web_port,
            available_maps,
            admin_html,
            vfs: None,
            world_collision: None,
            current_map_name: map_name.to_string(),
            current_map_overview: None,
            svc_ctx: SvcParseContext::new(),
            p_model_weapons: HashMap::new(),
            active_grenades: HashMap::new(),
            next_grenade_key: 1,
            prev_animtime: [0.0; 32],
            last_shot_at: [-10.0; 32],
            flashed_until: [0.0; 32],
            flashed_alpha: [0; 32],
            last_bomb_pos: [0.0; 3],
            lobbies: LobbyRegistry::default(),
        }
    }

    fn set_map_context(
        &mut self,
        vfs: Vfs,
        collision: Option<Arc<WorldCollision>>,
        map_name: &str,
    ) {
        self.vfs = Some(vfs);
        self.refresh_map_overview(collision, map_name);
    }

    fn refresh_map_overview(&mut self, collision: Option<Arc<WorldCollision>>, map_name: &str) {
        self.world_collision = collision;
        self.current_map_name = map_name.to_string();
        let fallback_bounds = self
            .world_collision
            .as_deref()
            .and_then(|c| c.model_bounds(0))
            .map(|(mn, mx)| ([mn.x, mn.y, mn.z], [mx.x, mx.y, mx.z]))
            .or(Some(([-2048.0, -2048.0, -512.0], [2048.0, 2048.0, 512.0])));
        self.current_map_overview = self.vfs.as_ref().and_then(|v| {
            load_map_overview_from_vfs(v, map_name, fallback_bounds)
        }).or_else(|| {
            let (mins, maxs) = fallback_bounds?;
            valid_map_overview_name(map_name).then(|| MapOverviewBundle {
                map: map_name.strip_suffix(".bsp").unwrap_or(map_name).to_string(),
                has_overview: false,
                zoom: 1.0,
                origin: [0.0; 3],
                rotated: false,
                mins,
                maxs,
                png_bytes: None,
            })
        });
    }

    fn resolve_map_overview(&self, map_name: &str) -> Option<MapOverviewBundle> {
        let clean = map_name.strip_suffix(".bsp").unwrap_or(map_name);
        if !valid_map_overview_name(clean) {
            return None;
        }
        if clean.eq_ignore_ascii_case(&self.current_map_name)
            && let Some(ref cached) = self.current_map_overview
        {
            return Some(cached.clone());
        }
        self.vfs
            .as_ref()
            .and_then(|v| load_map_overview_from_vfs(v, clean, None))
    }

    fn begin_round_recording<G: GameDll>(&mut self, server: &mut Server<G>, now: f64) {
        self.round_started_at = now;
        if !self.replays_enabled {
            return;
        }
        server.enable_replay_capture(true);
        self.replays
            .begin_round(server.map_name(), server.map_crc(), now);
        self.svc_ctx = SvcParseContext::new();
        self.p_model_weapons.clear();
        self.active_grenades.clear();
        self.prev_animtime = [0.0; 32];
        self.last_shot_at = [-10.0; 32];
        self.flashed_until = [0.0; 32];
        self.flashed_alpha = [0; 32];
        self.last_bomb_pos = [0.0; 3];

        let signon = server.encode_replay_signon();
        let (msgs, _) = parse_svc_stream_partial(&signon, &mut self.svc_ctx);
        for msg in msgs {
            if let SvcMessage::ResourceList { resources, .. } = msg {
                for r in resources {
                    if r.res_type == 2 {
                        let wid = weapon_id_from_p_model(&r.filename);
                        if wid != 0 {
                            self.p_model_weapons.insert(r.index, wid);
                        }
                    }
                }
            }
        }
        self.replays.record_svc_packet(now, true, &signon);
        self.replays.mark_signon_complete(now);
    }

    fn sync_players_from_server<G: GameDll>(&mut self, server: &Server<G>) {
        let infos = server.client_infos();
        for info in &infos {
            let entry = self
                .players
                .entry(info.slot)
                .or_insert_with(|| TrackedPlayerMeta {
                    name: info.name.clone(),
                    team: TeamSide::Unassigned,
                    frags: 0,
                    deaths: 0,
                    is_bot: info.is_bot,
                });
            entry.name = info.name.clone();
            entry.is_bot = info.is_bot;
            let key = format!("slot:{}", info.slot);
            self.stats
                .register_player(&key, &entry.name, entry.team, entry.is_bot);
            let team_id = match entry.team {
                TeamSide::Terrorist => 1,
                TeamSide::CounterTerrorist => 2,
                TeamSide::Spectator => 3,
                TeamSide::Unassigned => 0,
            };
            self.replays
                .set_timeline_player(info.slot + 1, &entry.name, team_id, entry.is_bot);
        }
    }

    fn trace_shot_end(&self, from: [f32; 3], angles: glam::Vec3) -> [f32; 3] {
        let pitch = angles.x.to_radians();
        let yaw = angles.y.to_radians();
        let dir = glam::Vec3::new(
            yaw.cos() * pitch.cos(),
            yaw.sin() * pitch.cos(),
            -pitch.sin(),
        );
        let start = glam::Vec3::from_array(from);
        let end = start + dir * 2048.0;
        if let Some(ref coll) = self.world_collision {
            let tr = coll.trace_world(start, end, Hull::Point);
            [tr.end_pos.x, tr.end_pos.y, tr.end_pos.z]
        } else {
            [end.x, end.y, end.z]
        }
    }

    fn apply_flash_detonation<G: GameDll>(&mut self, server: &Server<G>, pos: [f32; 3], now: f64) {
        let max_clients = server.config().max_clients;
        let flash_pos = glam::Vec3::from_array(pos);
        for slot in 0..max_clients {
            let Some((_, edict)) = server.edicts().by_index(u16::from(slot) + 1) else {
                continue;
            };
            let v = &edict.vars;
            if v.deadflag != 0 || v.health <= 0.0 {
                continue;
            }
            let eye = v.origin + glam::Vec3::new(0.0, 0.0, 17.0);
            let to_flash = flash_pos - eye;
            let dist = to_flash.length();
            if dist > 1500.0 {
                continue;
            }
            if let Some(ref coll) = self.world_collision {
                let tr = coll.trace_world(flash_pos, eye, Hull::Point);
                if tr.fraction < 0.99 {
                    continue;
                }
            }
            let falloff: f32 = (1.0 - dist / 1500.0).clamp(0.15, 1.0);
            let yaw = v.v_angle.y.to_radians();
            let fwd = glam::Vec3::new(yaw.cos(), yaw.sin(), 0.0);
            let dir_xy = to_flash.with_z(0.0).normalize_or_zero();
            let facing: f32 = if fwd.dot(dir_xy) > 0.25 { 1.0 } else { 0.45 };
            let alpha = (255.0_f32 * falloff * facing).round().clamp(30.0, 255.0) as u8;
            let dur = 1.8 + 2.4 * f64::from(falloff * facing);
            let idx = usize::from(slot);
            if alpha >= self.flashed_alpha[idx] {
                self.flashed_alpha[idx] = alpha;
                self.flashed_until[idx] = now + dur;
            }
        }
    }

    fn step_from_server<G: GameDll>(&mut self, server: &mut Server<G>, now: f64) {
        self.lobbies.expire_stale(now);
        self.sync_players_from_server(server);
        let max_clients = server.config().max_clients;
        let mut deathmsgs_seen: Vec<(u8, u8)> = Vec::new();
        let mut bomb_planted_seen = false;

        if self.replays_enabled {
            let pkts = server.take_replay_svc_packets();
            for pkt in pkts {
                self.replays.record_svc_packet(now, false, &pkt);
                let (msgs, _) = parse_svc_stream_partial(&pkt, &mut self.svc_ctx);
                for msg in msgs {
                    match msg {
                        SvcMessage::UserMsg { decoded, .. } => match decoded {
                            CsUserMsg::TeamInfo {
                                player_id,
                                team_name,
                            } if player_id > 0 => {
                                let slot0 = player_id - 1;
                                let team = match team_name.as_str() {
                                    "CT" => TeamSide::CounterTerrorist,
                                    "TERRORIST" => TeamSide::Terrorist,
                                    "SPECTATOR" => TeamSide::Spectator,
                                    _ => TeamSide::Unassigned,
                                };
                                if let Some(entry) = self.players.get_mut(&slot0) {
                                    entry.team = team;
                                    let tid = match team {
                                        TeamSide::Terrorist => 1,
                                        TeamSide::CounterTerrorist => 2,
                                        TeamSide::Spectator => 3,
                                        TeamSide::Unassigned => 0,
                                    };
                                    self.replays.set_timeline_player(
                                        player_id,
                                        &entry.name,
                                        tid,
                                        entry.is_bot,
                                    );
                                }
                            }
                            CsUserMsg::DeathMsg {
                                killer_id,
                                victim_id,
                                headshot,
                                weapon,
                            } => {
                                deathmsgs_seen.push((killer_id, victim_id));
                                let killer = (killer_id > 0).then_some(killer_id);
                                self.replays.record_timeline_kill(
                                    now,
                                    killer,
                                    victim_id,
                                    &weapon,
                                    headshot,
                                );
                            }
                            CsUserMsg::BombDrop { origin, planted } => {
                                let pos = [origin.x, origin.y, origin.z];
                                self.last_bomb_pos = pos;
                                let state = if planted {
                                    bomb_planted_seen = true;
                                    TimelineBombState::Planted
                                } else {
                                    TimelineBombState::Dropped
                                };
                                self.replays.record_timeline_bomb(now, state, pos);
                            }
                            CsUserMsg::BombPickup => {
                                self.replays.record_timeline_bomb(
                                    now,
                                    TimelineBombState::Pickup,
                                    self.last_bomb_pos,
                                );
                            }
                            _ => {}
                        },
                        SvcMessage::PacketEntities(pe) => {
                            let player_schema = self.svc_ctx.deltas.get("entity_state_player_t");
                            for (&ent_num, (_, rec)) in &pe.entities {
                                if ent_num >= 1 && ent_num <= u16::from(max_clients) {
                                    let effects = player_schema
                                        .map_or(0, |s| rec.get_i32(s, "effects"));
                                    if (effects & 2) != 0 {
                                        let slot = (ent_num - 1) as usize;
                                        if slot < 32
                                            && now - self.last_shot_at[slot] >= 0.04
                                            && let Some((_, edict)) =
                                                server.edicts().by_index(ent_num)
                                        {
                                            let v = &edict.vars;
                                            let wid = self
                                                .p_model_weapons
                                                .get(&v.weaponmodel)
                                                .copied()
                                                .unwrap_or(0);
                                            let eye = [v.origin.x, v.origin.y, v.origin.z + 17.0];
                                            let to = self.trace_shot_end(eye, v.v_angle);
                                            self.last_shot_at[slot] = now;
                                            self.replays.record_timeline_shot(
                                                now,
                                                ent_num as u8,
                                                eye,
                                                to,
                                                wid,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        for ev in server.take_stat_events() {
            match ev {
                GameStatEvent::RoundStart { round_number } => {
                    if self.replays_enabled && self.replays.is_recording() {
                        let (ct, t) = self.stats.scores();
                        let _ = self.replays.finish_round(
                            RoundWinner::Draw,
                            "Round Restarted",
                            now,
                            ct,
                            t,
                            None,
                        );
                    }
                    self.stats.begin_round(round_number);
                    self.begin_round_recording(server, now);
                }
                GameStatEvent::RoundEnd { winner, reason } => {
                    if self.replays_enabled
                        && (reason.contains("Bombed") || reason.contains("Target_Bombed"))
                    {
                        self.replays.record_timeline_bomb(
                            now,
                            TimelineBombState::Exploded,
                            self.last_bomb_pos,
                        );
                    }
                    let w = match winner {
                        1 => RoundWinner::Terrorist,
                        2 => RoundWinner::CounterTerrorist,
                        _ => RoundWinner::Draw,
                    };
                    let duration = (now - self.round_started_at).max(0.0);
                    let summary = self.stats.end_round(w, &reason, duration, None);
                    if self.replays_enabled {
                        let _ = self.replays.finish_round(
                            w,
                            &reason,
                            now,
                            summary.score_ct,
                            summary.score_t,
                            summary.mvp_name,
                        );
                    }
                }
                GameStatEvent::Damage {
                    attacker_slot,
                    victim_slot,
                    damage,
                } => {
                    let att = attacker_slot.map(|s| format!("slot:{s}"));
                    let vic = format!("slot:{victim_slot}");
                    self.stats.record_damage(att.as_deref(), &vic, damage);
                    if self.replays_enabled
                        && let Some(a_slot) = attacker_slot
                        && usize::from(a_slot) < 32
                        && now - self.last_shot_at[usize::from(a_slot)] >= 0.04
                        && let Some((_, att_ed)) = server.edicts().by_index(u16::from(a_slot) + 1)
                        && let Some((_, vic_ed)) =
                            server.edicts().by_index(u16::from(victim_slot) + 1)
                    {
                        let wid = self
                            .p_model_weapons
                            .get(&att_ed.vars.weaponmodel)
                            .copied()
                            .unwrap_or(0);
                        let from = [
                            att_ed.vars.origin.x,
                            att_ed.vars.origin.y,
                            att_ed.vars.origin.z + 17.0,
                        ];
                        let to = [
                            vic_ed.vars.origin.x,
                            vic_ed.vars.origin.y,
                            vic_ed.vars.origin.z + 12.0,
                        ];
                        self.last_shot_at[usize::from(a_slot)] = now;
                        self.replays
                            .record_timeline_shot(now, a_slot + 1, from, to, wid);
                    }
                }
                GameStatEvent::Kill {
                    killer_slot,
                    victim_slot,
                    headshot,
                    friendly_fire,
                } => {
                    if let Some(v) = self.players.get_mut(&victim_slot) {
                        v.deaths += 1;
                    }
                    if let Some(ks) = killer_slot
                        && let Some(k) = self.players.get_mut(&ks)
                    {
                        if ks == victim_slot || friendly_fire {
                            k.frags -= 1;
                        } else {
                            k.frags += 1;
                        }
                    }
                    if self.replays_enabled {
                        let k_id = killer_slot.map_or(0, |s| s + 1);
                        let v_id = victim_slot + 1;
                        if !deathmsgs_seen.contains(&(k_id, v_id)) {
                            self.replays.record_timeline_kill(
                                now,
                                killer_slot.map(|s| s + 1),
                                v_id,
                                "weapon",
                                headshot,
                            );
                        }
                    }
                    let killer = killer_slot.map(|s| format!("slot:{s}"));
                    let victim = format!("slot:{victim_slot}");
                    self.stats.record_kill(
                        killer.as_deref(),
                        &victim,
                        None,
                        headshot,
                        friendly_fire,
                    );
                }
                GameStatEvent::BombPlant { slot } => {
                    self.stats.record_bomb_plant(&format!("slot:{slot}"));
                    if self.replays_enabled && !bomb_planted_seen {
                        let pos = server
                            .edicts()
                            .by_index(u16::from(slot) + 1)
                            .map_or(self.last_bomb_pos, |(_, e)| {
                                [e.vars.origin.x, e.vars.origin.y, e.vars.origin.z]
                            });
                        self.last_bomb_pos = pos;
                        self.replays
                            .record_timeline_bomb(now, TimelineBombState::Planted, pos);
                    }
                }
                GameStatEvent::BombDefuse { slot } => {
                    self.stats.record_bomb_defuse(&format!("slot:{slot}"));
                    if self.replays_enabled {
                        let pos = server
                            .edicts()
                            .by_index(u16::from(slot) + 1)
                            .map_or(self.last_bomb_pos, |(_, e)| {
                                [e.vars.origin.x, e.vars.origin.y, e.vars.origin.z]
                            });
                        self.replays
                            .record_timeline_bomb(now, TimelineBombState::Defused, pos);
                    }
                }
                GameStatEvent::HostageRescue { slot } => {
                    self.stats.record_hostage_rescue(&format!("slot:{slot}"));
                }
            }
        }

        if self.replays_enabled && self.replays.is_recording() {
            // Track active grenades and planted C4 edicts.
            let mut seen_grenades: Vec<EdictId> = Vec::new();
            for (eid, edict) in server
                .edicts()
                .iter_live_from(usize::from(max_clients) + 1)
            {
                if edict.vars.classname != "grenade" {
                    continue;
                }
                let pos = [
                    edict.vars.origin.x,
                    edict.vars.origin.y,
                    edict.vars.origin.z,
                ];
                if edict.vars.model.to_ascii_lowercase().contains("w_c4") {
                    self.last_bomb_pos = pos;
                    continue;
                }
                let Some(kind) = grenade_kind_from_model(&edict.vars.model) else {
                    continue;
                };
                seen_grenades.push(eid);
                let nodraw = (edict.vars.effects & cs_engine::consts::EF_NODRAW) != 0;
                if let Some((gid, _, last_pos, detonated)) = self.active_grenades.get_mut(&eid) {
                    *last_pos = pos;
                    self.replays.record_timeline_grenade_pos(now, *gid, kind, pos);
                    if kind == TimelineGrenadeKind::Smoke && nodraw && !*detonated {
                        *detonated = true;
                        self.replays.detonate_timeline_grenade(now, *gid, Some(pos));
                    }
                } else {
                    let gid = self.next_grenade_key;
                    self.next_grenade_key = self.next_grenade_key.saturating_add(1);
                    self.replays.record_timeline_grenade_pos(now, gid, kind, pos);
                    self.active_grenades
                        .insert(eid, (gid, kind, pos, false));
                }
            }

            let removed_grenades: Vec<EdictId> = self
                .active_grenades
                .keys()
                .copied()
                .filter(|eid| !seen_grenades.contains(eid))
                .collect();
            let mut flash_detonations: Vec<[f32; 3]> = Vec::new();
            for eid in removed_grenades {
                if let Some((gid, kind, last_pos, detonated)) = self.active_grenades.remove(&eid)
                    && !detonated
                {
                    self.replays
                        .detonate_timeline_grenade(now, gid, Some(last_pos));
                    if kind == TimelineGrenadeKind::Flash {
                        flash_detonations.push(last_pos);
                    }
                }
            }
            for fpos in flash_detonations {
                self.apply_flash_detonation(server, fpos, now);
            }

            // Detect per-frame player firearm shots via animtime/button transition and sample at 10 Hz.
            let mut samples = Vec::new();
            for info in server.client_infos() {
                let slot_idx = usize::from(info.slot);
                let Some((_, edict)) = server.edicts().by_index(u16::from(info.slot) + 1) else {
                    continue;
                };
                let v = &edict.vars;
                let alive = v.deadflag == 0 && v.health > 0.0;
                let weapon_id = self
                    .p_model_weapons
                    .get(&v.weaponmodel)
                    .copied()
                    .unwrap_or(0);
                if alive
                    && slot_idx < 32
                    && is_firearm_weapon_id(weapon_id)
                    && (v.button & 1 != 0 || v.oldbuttons & 1 != 0)
                    && (v.animtime - self.prev_animtime[slot_idx]).abs() > 1e-4
                    && now - self.last_shot_at[slot_idx] >= 0.04
                {
                    let eye = [v.origin.x, v.origin.y, v.origin.z + 17.0];
                    let to = self.trace_shot_end(eye, v.v_angle);
                    self.last_shot_at[slot_idx] = now;
                    self.replays
                        .record_timeline_shot(now, info.slot + 1, eye, to, weapon_id);
                }
                if slot_idx < 32 {
                    self.prev_animtime[slot_idx] = v.animtime;
                    if now >= self.flashed_until[slot_idx] {
                        self.flashed_alpha[slot_idx] = 0;
                    }
                }
                let team = self.players.get(&info.slot).map_or(0, |m| match m.team {
                    TeamSide::Terrorist => 1,
                    TeamSide::CounterTerrorist => 2,
                    TeamSide::Spectator => 3,
                    TeamSide::Unassigned => 0,
                });
                let has_c4 = v.body == 1 || weapon_id == 6;
                let flashed_alpha = if slot_idx < 32 && alive {
                    self.flashed_alpha[slot_idx]
                } else {
                    0
                };
                samples.push(TimelinePlayerSample {
                    slot: info.slot + 1,
                    team,
                    alive,
                    hp: if alive { v.health.round() as i32 } else { 0 },
                    x: v.origin.x,
                    y: v.origin.y,
                    z: v.origin.z,
                    yaw_deg: if v.v_angle != glam::Vec3::ZERO {
                        v.v_angle.y
                    } else {
                        v.angles.y
                    },
                    active_weapon_id: weapon_id,
                    has_c4,
                    flashed_alpha,
                });
            }
            self.replays.sample_timeline_if_due(now, &samples);
        }
    }

    fn servers_list_json<G: GameDll>(&self, server: &Server<G>, host_header: Option<&str>) -> String {
        let host = host_header
            .and_then(|h| h.split(':').next())
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .unwrap_or("127.0.0.1");
        let addr = format!("{host}:{}", self.game_port);
        let infos = server.client_infos();
        let players = infos.len();
        let bots = infos.iter().filter(|c| c.is_bot).count();
        let web_port_json = match self.web_port {
            Some(p) => p.to_string(),
            None => "null".to_string(),
        };
        format!(
            "[{{\"addr\":{},\"hostname\":{},\"gamedir\":\"cstrike\",\"map\":{},\"players\":{},\"max\":{},\"bots\":{},\"web_port\":{web_port_json}}}]",
            escape_json_str(&addr),
            escape_json_str(&server.hostname()),
            escape_json_str(server.map_name()),
            players,
            server.config().max_clients,
            bots,
        )
    }
}

enum PendingHttpRequest {
    Admin {
        head: HttpRequest,
        body: Vec<u8>,
        reply: Sender<AdminHttpResponse>,
    },
    Lobby {
        head: HttpRequest,
        body: Vec<u8>,
        reply: Sender<AdminHttpResponse>,
    },
    ServersJson {
        host: Option<String>,
        reply: Sender<String>,
    },
}

struct ServerAdminBridge<'a, G: GameDll> {
    server: &'a mut Server<G>,
    runtime: &'a mut Phase13Runtime,
    now: f64,
    load_map: &'a mut dyn FnMut(&str) -> Result<ServerMap, String>,
    quit_requested: &'a mut bool,
}

impl<G: GameDll> AdminBackend for ServerAdminBridge<'_, G> {
    fn rcon_password(&self) -> &str {
        self.server.rcon_password()
    }

    fn status(&self) -> ServerStatusSnapshot {
        let infos = self.server.client_infos();
        let bots = infos.iter().filter(|c| c.is_bot).count() as u8;
        let (score_ct, score_t) = self.runtime.stats.scores();
        ServerStatusSnapshot {
            hostname: self.server.hostname(),
            map: self.server.map_name().to_string(),
            players: infos.len() as u8,
            bots,
            max_players: self.server.config().max_clients,
            uptime_secs: (self.now - self.runtime.started_at).max(0.0),
            tick_rate: self
                .server
                .cvar_str("sys_ticrate")
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v > 0.0)
                .map_or(100, |v| v.round().max(1.0) as u32),
            round_number: self.runtime.stats.current_round(),
            score_ct,
            score_t,
            game_port: self.runtime.game_port,
            web_port: self.runtime.web_port,
            bot_quota: self.server.cvar_i32("bot_quota").max(0) as u32,
            bot_difficulty: self.server.cvar_i32("bot_difficulty").clamp(0, 3) as u8,
            available_maps: self.runtime.available_maps.clone(),
        }
    }

    fn players(&self) -> Vec<PlayerSnapshot> {
        self.server
            .client_infos()
            .into_iter()
            .map(|c| {
                let meta = self.runtime.players.get(&c.slot);
                PlayerSnapshot {
                    slot: c.slot + 1,
                    userid: c.userid,
                    name: c.name,
                    team: meta.map_or("UNASSIGNED", |m| m.team.as_str()).to_string(),
                    frags: meta.map_or(0, |m| m.frags),
                    deaths: meta.map_or(0, |m| m.deaths),
                    ping_ms: c.ping_ms,
                    state: if c.active {
                        "active".to_string()
                    } else {
                        "connecting".to_string()
                    },
                    addr: c.addr,
                    is_bot: c.is_bot,
                }
            })
            .collect()
    }

    fn stats_json(&self) -> String {
        self.runtime.stats.to_json()
    }

    fn replays_index_json(&self) -> String {
        self.runtime.replays.to_json_index()
    }

    fn replay_dem(&self, id: &str) -> Option<Vec<u8>> {
        self.runtime.replays.get_demo(id).map(|b| b.to_vec())
    }

    fn replay_timeline_json(&self, id: &str) -> Option<String> {
        self.runtime
            .replays
            .get_timeline_json(id)
            .map(ToString::to_string)
    }

    fn map_overview_json(&self, map: &str) -> Option<String> {
        self.runtime
            .resolve_map_overview(map)
            .map(|b| b.to_json())
    }

    fn map_overview_png(&self, map: &str) -> Option<Vec<u8>> {
        self.runtime
            .resolve_map_overview(map)
            .and_then(|b| b.png_bytes)
    }

    fn execute_rcon(&mut self, command: &str) -> Result<String, String> {
        if self.runtime.replays_enabled {
            self.runtime.replays.record_console_command(self.now, command);
        }
        match self.server.console_command(command) {
            ConsoleOutcome::Output(text) => Ok(text),
            ConsoleOutcome::ChangeLevel(name) => {
                match (self.load_map)(&name) {
                    Ok(map) => {
                        let coll = map.collision_arc();
                        self.server.change_map(map);
                        self.runtime.stats.reset_match(&name);
                        self.runtime.refresh_map_overview(coll, &name);
                        self.runtime.begin_round_recording(self.server, self.now);
                        Ok(format!("changing level to {name}\n"))
                    }
                    Err(msg) => Err(msg),
                }
            }
            ConsoleOutcome::Quit => {
                *self.quit_requested = true;
                Ok("Server shutting down\n".to_string())
            }
        }
    }

    fn admin_html(&self) -> Option<&[u8]> {
        self.runtime.admin_html.as_deref()
    }

    fn invite_config(&self) -> InviteConfig {
        InviteConfig {
            default_host: "127.0.0.1".to_string(),
            web_port: self.runtime.web_port.unwrap_or(27016),
            game_port: self.runtime.game_port,
            default_room: "alpha".to_string(),
        }
    }
}

fn write_cors_json_response(
    stream: &mut dyn Write,
    status_line: &str,
    body: &str,
    head_only: bool,
) {
    let head = format!(
        "HTTP/1.1 {status_line}\r\n\
         Content-Type: application/json; charset=utf-8\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    if !head_only && !body.is_empty() {
        let _ = stream.write_all(body.as_bytes());
    }
    let _ = stream.flush();
}

fn make_http_hook(
    tx: Sender<PendingHttpRequest>,
    static_dir: Option<PathBuf>,
    embed_web: bool,
) -> HttpHook {
    Arc::new(move |req: &HttpRequest, leftover: &[u8], stream, _peer_ip| {
        if req.path == "/servers" {
            if req.method != "GET" && req.method != "HEAD" {
                write_cors_json_response(
                    stream,
                    "405 Method Not Allowed",
                    "{\"error\":\"method not allowed\"}",
                    false,
                );
                return true;
            }
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx
                .send(PendingHttpRequest::ServersJson {
                    host: req.header("host").map(str::to_string),
                    reply: reply_tx,
                })
                .is_err()
            {
                write_cors_json_response(
                    stream,
                    "503 Service Unavailable",
                    "{\"error\":\"server unavailable\"}",
                    false,
                );
                return true;
            }
            match reply_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(json) => {
                    write_cors_json_response(stream, "200 OK", &json, req.method == "HEAD");
                }
                Err(_) => {
                    write_cors_json_response(
                        stream,
                        "503 Service Unavailable",
                        "{\"error\":\"server timeout\"}",
                        false,
                    );
                }
            }
            return true;
        }

        if is_lobby_path(&req.path) {
            let body = match read_http_body(req, leftover, stream, MAX_API_BODY_BYTES) {
                Ok(b) => b,
                Err(crate::admin_http::HttpReadError::PayloadTooLarge) => {
                    let _ = AdminHttpResponse::cors_json_error(
                        413,
                        "Payload Too Large",
                        "request body too large",
                    )
                    .write_to(stream, false);
                    return true;
                }
                Err(_) => {
                    let _ = AdminHttpResponse::cors_json_error(
                        400,
                        "Bad Request",
                        "invalid HTTP request",
                    )
                    .write_to(stream, false);
                    return true;
                }
            };
            let (reply_tx, reply_rx) = mpsc::channel();
            let pending = PendingHttpRequest::Lobby {
                head: req.clone(),
                body,
                reply: reply_tx,
            };
            if tx.send(pending).is_err() {
                let _ = AdminHttpResponse::cors_json_error(
                    503,
                    "Service Unavailable",
                    "server unavailable",
                )
                .write_to(stream, false);
                return true;
            }
            match reply_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(resp) => {
                    let _ = resp.write_to(stream, req.method == "HEAD");
                }
                Err(_) => {
                    let _ = AdminHttpResponse::cors_json_error(
                        503,
                        "Service Unavailable",
                        "server timeout",
                    )
                    .write_to(stream, false);
                }
            }
            return true;
        }

        if AdminRouter::handles_path(&req.path) {
            let body = match read_http_body(req, leftover, stream, MAX_API_BODY_BYTES) {
                Ok(b) => b,
                Err(crate::admin_http::HttpReadError::PayloadTooLarge) => {
                    let _ = AdminHttpResponse::json_error(
                        413,
                        "Payload Too Large",
                        "request body too large",
                    )
                    .write_to(stream, false);
                    return true;
                }
                Err(_) => {
                    let _ =
                        AdminHttpResponse::json_error(400, "Bad Request", "invalid HTTP request")
                            .write_to(stream, false);
                    return true;
                }
            };
            let (reply_tx, reply_rx) = mpsc::channel();
            let pending = PendingHttpRequest::Admin {
                head: req.clone(),
                body,
                reply: reply_tx,
            };
            if tx.send(pending).is_err() {
                let _ = AdminHttpResponse::json_error(
                    503,
                    "Service Unavailable",
                    "server unavailable",
                )
                .write_to(stream, false);
                return true;
            }
            match reply_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(resp) => {
                    let _ = resp.write_to(stream, req.method == "HEAD");
                }
                Err(_) => {
                    let _ = AdminHttpResponse::json_error(
                        503,
                        "Service Unavailable",
                        "server timeout",
                    )
                    .write_to(stream, false);
                }
            }
            return true;
        }

        if (req.method == "GET" || req.method == "HEAD")
            && let Some((mime, bytes)) = embedded_web_asset(&req.path)
        {
            let disk_exists = !embed_web
                && static_dir
                    .as_deref()
                    .and_then(|root| cs_transport::ws_proto::resolve_static_path(root, &req.path))
                    .is_some_and(|p| p.is_file());
            if !disk_exists {
                let _ = cs_transport::ws_native::send_http_response(
                    stream,
                    "200 OK",
                    mime,
                    bytes,
                    req.method == "HEAD",
                );
                return true;
            }
        }

        false
    })
}

fn process_pending_http<G: GameDll>(
    rx: &Receiver<PendingHttpRequest>,
    server: &mut Server<G>,
    runtime: &mut Phase13Runtime,
    now: f64,
    load_map: &mut dyn FnMut(&str) -> Result<ServerMap, String>,
) -> bool {
    let mut quit_requested = false;
    while let Ok(req) = rx.try_recv() {
        match req {
            PendingHttpRequest::ServersJson { host, reply } => {
                let json = runtime.servers_list_json(server, host.as_deref());
                let _ = reply.send(json);
            }
            PendingHttpRequest::Admin {
                head,
                body,
                reply,
            } => {
                let view = AdminHttpRequest::from_http_request(&head, &body);
                let mut bridge = ServerAdminBridge {
                    server,
                    runtime,
                    now,
                    load_map,
                    quit_requested: &mut quit_requested,
                };
                let resp = AdminRouter::handle(&view, &mut bridge);
                let _ = reply.send(resp);
            }
            PendingHttpRequest::Lobby { head, body, reply } => {
                let view = AdminHttpRequest::from_http_request(&head, &body);
                let resp = handle_lobby_request(&view, &mut runtime.lobbies, now);
                let _ = reply.send(resp);
            }
        }
    }
    quit_requested
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
    let effective_web_port = opts.web_port.or(opts.admin.then_some(27016));
    let static_dir = opts.web_dir.clone().or_else(default_web_dir);
    let (http_tx, http_rx) = mpsc::channel::<PendingHttpRequest>();
    let mut net = if let Some(web_port) = effective_web_port {
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
        let http_hook = make_http_hook(http_tx, static_dir.clone(), opts.embed_web);
        let ws_srv = WsServerTransport::bind(WsServerConfig {
            bind_addr: format!("0.0.0.0:{web_port}"),
            static_dir: static_dir.clone(),
            ws_path: "/ws".to_string(),
            max_clients: usize::from(opts.max_players) + 8,
            upgrade_hook,
            http_hook: Some(http_hook),
        })
        .map_err(|e| format!("cannot bind WebSocket/HTTP on 0.0.0.0:{web_port}: {e}"))?;
        let ws_addr = ws_srv.local_addr();
        info!(
            http = %format!("http://127.0.0.1:{}", ws_addr.port()),
            admin = %format!("http://127.0.0.1:{}/admin", ws_addr.port()),
            ws = %format!("ws://127.0.0.1:{}/ws", ws_addr.port()),
            static_dir = ?static_dir,
            embed_web = opts.embed_web,
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

    let initial_collision = map.collision_arc();
    let cfg = ServerConfig {
        hostname: opts.hostname.clone(),
        max_clients: opts.max_players,
        ..ServerConfig::default()
    };
    let mut server = new_server(cfg, map, game, vfs);
    server.set_voice_bridge(Box::new(crate::voice_bridge::ServerVoiceBridge::new()));
    if let Some(ticrate) = opts.sys_ticrate {
        server.set_cvar("sys_ticrate", &ticrate.to_string());
    }
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
        replay_rounds = opts.replay_rounds,
        "cs-server (Protocol 48) listening at 100 Hz (type quit or press Ctrl+C to stop)"
    );

    let commands = console_io::spawn_line_reader(std::io::BufReader::new(std::io::stdin()));
    let interrupted = console_io::interrupt_flag();
    let clock = NativeClock::new();
    let start_now = clock.now();
    let mut runtime = Phase13Runtime::new(
        &map_name,
        opts.replay_rounds,
        start_now,
        udp_addr.port(),
        effective_web_port,
        vfs.list_maps(),
        static_dir.as_deref(),
    );
    runtime.set_map_context(vfs.clone(), initial_collision, &map_name);
    runtime.begin_round_recording(&mut server, start_now);

    let mut next_frame = start_now;
    let mut load_map = |name: &str| load_server_map(vfs, name);
    loop {
        let ticrate = server
            .cvar_str("sys_ticrate")
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(100.0);
        let frame_secs = 1.0 / ticrate;
        let now = clock.now();
        // Server time starts at 0 with the server, so `clock.now()` (seconds since the clock
        // was created, just before this loop) plus one frame makes the first `frametime` one
        // frame long instead of about 0, and later frames get their real elapsed time.
        server.frame(&mut net, now + frame_secs);
        runtime.step_from_server(&mut server, now);
        if process_pending_http(&http_rx, &mut server, &mut runtime, now, &mut load_map) {
            info!("shutting down via admin RCON");
            server.shutdown(&mut net, "Server shutting down");
            return Ok(());
        }
        // A map change the game asked for (`CHANGE_LEVEL`), done as the console's `changelevel`.
        // A change leaves the console lines to the next frame, as a console `changelevel` does.
        let changed = match server.take_pending_change_level() {
            Some(name) => {
                if let Ok(next_map) = load_map(&name) {
                    let next_coll = next_map.collision_arc();
                    println!("changing level to {name}");
                    server.change_map(next_map);
                    runtime.stats.reset_match(&name);
                    runtime.refresh_map_overview(next_coll, &name);
                    runtime.begin_round_recording(&mut server, now);
                } else {
                    change_level(&mut load_map, &mut server, &name);
                }
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
        next_frame += frame_secs;
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
            http_hook: None,
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

    #[test]
    fn parses_phase13_admin_replay_and_embed_flags() {
        let o = parse(&args(&[
            "--admin",
            "--replay-rounds",
            "12",
            "--embed-web",
            "-sys_ticrate",
            "66",
        ]))
        .unwrap();
        assert!(o.admin);
        assert_eq!(o.replay_rounds, 12);
        assert!(o.embed_web);
        assert_eq!(o.sys_ticrate, Some(66.0));

        assert!(parse(&args(&["-sys_ticrate", "0"])).is_err());
        assert!(parse(&args(&["-sys_ticrate", "abc"])).is_err());
        assert!(parse(&args(&["-sys_ticrate"])).is_err());

        let zero = parse(&args(&["--replay-rounds", "0"])).unwrap();
        assert_eq!(zero.replay_rounds, 0);

        assert!(parse(&args(&["--replay-rounds", "abc"])).is_err());
        assert!(parse(&args(&["--replay-rounds"])).is_err());
    }

    fn tcp_http_request(addr: SocketAddr, raw: &str) -> Vec<u8> {
        use std::io::{ErrorKind, Read};
        use std::net::TcpStream;

        let mut s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        s.flush().unwrap();
        let mut out = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            match s.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(e)
                    if !out.is_empty()
                        && matches!(
                            e.kind(),
                            ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
                        ) =>
                {
                    break;
                }
                Err(e) => panic!("tcp_http_request read error: {e}"),
            }
        }
        out
    }

    #[test]
    fn admin_http_endpoints_replays_stats_rcon_and_servers_work_over_ws_transport() {
        let (http_tx, http_rx) = mpsc::channel::<PendingHttpRequest>();
        let http_hook = make_http_hook(http_tx, None, true);
        let ws_srv = WsServerTransport::bind(WsServerConfig {
            bind_addr: "127.0.0.1:0".into(),
            static_dir: None,
            ws_path: "/ws".into(),
            max_clients: 8,
            upgrade_hook: None,
            http_hook: Some(http_hook),
        })
        .unwrap();
        let http_addr = ws_srv.local_addr();

        let mut server = Server::new(
            ServerConfig {
                hostname: "Phase13 Admin Test".into(),
                max_clients: 8,
                ..ServerConfig::default()
            },
            ServerMap::stub("de_dust2"),
            WalkGame::new(MoveParams::default()),
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        server.set_cvar("rcon_password", "op-secret");

        let mut runtime = Phase13Runtime::new(
            "de_dust2",
            4,
            0.0,
            27015,
            Some(http_addr.port()),
            vec!["de_dust2".into(), "de_inferno".into()],
            None,
        );
        runtime.set_map_context(
            Vfs::from_memory([]),
            None,
            "de_dust2",
        );
        runtime.begin_round_recording(&mut server, 0.0);
        runtime.replays.sample_timeline_if_due(
            0.1,
            &[TimelinePlayerSample {
                slot: 1,
                team: 2,
                alive: true,
                hp: 100,
                x: -223.0,
                y: 1097.0,
                z: -192.0,
                yaw_deg: 90.0,
                active_weapon_id: 22,
                has_c4: false,
                flashed_alpha: 0,
            }],
        );
        runtime.players.insert(
            0,
            TrackedPlayerMeta {
                name: "Alice".into(),
                team: TeamSide::CounterTerrorist,
                frags: 2,
                deaths: 1,
                is_bot: false,
            },
        );
        runtime
            .stats
            .register_player("slot:0", "Alice", TeamSide::CounterTerrorist, false);
        runtime.stats.begin_round(1);
        runtime.stats.record_damage(Some("slot:0"), "slot:1", 100);
        runtime
            .stats
            .record_kill(Some("slot:0"), "slot:1", None, true, false);
        let summary = runtime
            .stats
            .end_round(RoundWinner::CounterTerrorist, "#CTs_Win", 24.0, None);
        let meta = runtime
            .replays
            .finish_round(
                RoundWinner::CounterTerrorist,
                "#CTs_Win",
                24.0,
                summary.score_ct,
                summary.score_t,
                summary.mvp_name.clone(),
            )
            .unwrap()
            .expect("round 1 replay saved");
        assert!(meta.byte_len > 544);

        let running = Arc::new(AtomicBool::new(true));
        let run_flag = Arc::clone(&running);
        let mut net = ws_srv;
        let handle = std::thread::spawn(move || {
            let mut load = |name: &str| Ok(ServerMap::stub(name));
            let mut t = 0.0;
            while run_flag.load(Ordering::Relaxed) {
                t += 0.01;
                server.frame(&mut net, t);
                runtime.step_from_server(&mut server, t);
                if process_pending_http(&http_rx, &mut server, &mut runtime, t, &mut load) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        });

        let admin_html = String::from_utf8(tcp_http_request(
            http_addr,
            "GET /admin HTTP/1.1\r\nHost: localhost\r\n\r\n",
        ))
        .unwrap();
        assert!(admin_html.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(admin_html.contains("Web Admin Panel"));

        let admin_js = String::from_utf8(tcp_http_request(
            http_addr,
            "GET /admin.js HTTP/1.1\r\nHost: localhost\r\n\r\n",
        ))
        .unwrap();
        assert!(admin_js.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(admin_js.contains("cs16_admin_token"));

        let servers_resp = String::from_utf8(tcp_http_request(
            http_addr,
            "GET /servers HTTP/1.1\r\nHost: play.example.com:27016\r\n\r\n",
        ))
        .unwrap();
        assert!(servers_resp.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(servers_resp.contains("Access-Control-Allow-Origin: *\r\n"));
        assert!(servers_resp.contains("\"map\":\"de_dust2\""));

        let unauth = String::from_utf8(tcp_http_request(
            http_addr,
            "GET /api/status HTTP/1.1\r\nHost: localhost\r\n\r\n",
        ))
        .unwrap();
        assert!(unauth.starts_with("HTTP/1.1 401 Unauthorized\r\n"));

        let status_ok = String::from_utf8(tcp_http_request(
            http_addr,
            "GET /api/status HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer op-secret\r\n\r\n",
        ))
        .unwrap();
        assert!(status_ok.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(status_ok.contains("\"hostname\":\"Phase13 Admin Test\""));

        let stats_ok = String::from_utf8(tcp_http_request(
            http_addr,
            "GET /api/stats HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer op-secret\r\n\r\n",
        ))
        .unwrap();
        assert!(stats_ok.contains("\"name\":\"Alice\""));
        assert!(stats_ok.contains("\"adr\":100.0"));

        let rcon_body = r#"{"command":"status"}"#;
        let rcon_req = format!(
            "POST /api/rcon HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer op-secret\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            rcon_body.len(),
            rcon_body
        );
        let rcon_resp = String::from_utf8(tcp_http_request(http_addr, &rcon_req)).unwrap();
        assert!(rcon_resp.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(rcon_resp.contains("hostname: Phase13 Admin Test"));

        let replay_raw = tcp_http_request(
            http_addr,
            &format!(
                "GET /api/replays/{}.dem HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer op-secret\r\n\r\n",
                meta.id
            ),
        );
        let split = replay_raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap();
        let dem_bytes = &replay_raw[split + 4..];
        let parsed = cs_proto48::demo::parse_demo(dem_bytes).expect("valid downloaded .dem");
        assert_eq!(parsed.header.map_name, "de_dust2");

        let timeline_ok = String::from_utf8(tcp_http_request(
            http_addr,
            &format!(
                "GET /api/replays/{}/timeline HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer op-secret\r\n\r\n",
                meta.id
            ),
        ))
        .unwrap();
        assert!(timeline_ok.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(timeline_ok.contains("\"sample_rate_hz\":10"));

        let overview_ok = String::from_utf8(tcp_http_request(
            http_addr,
            "GET /api/map-overview/de_dust2 HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer op-secret\r\n\r\n",
        ))
        .unwrap();
        assert!(overview_ok.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(overview_ok.contains("\"map\":\"de_dust2\""));

        // Public CORS WebRTC lobby signaling routes (/lobbies, /lobbies/<room>, /lobbies/<room>/signal).
        let lobby_body = r#"{"room":"p2p-1","host":"browser-p2p","port":443,"map":"de_dust2","players":1,"max_players":10,"mode":"webrtc"}"#;
        let lobby_req = format!(
            "POST /lobbies HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            lobby_body.len(),
            lobby_body
        );
        let lobby_resp = String::from_utf8(tcp_http_request(http_addr, &lobby_req)).unwrap();
        assert!(lobby_resp.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(lobby_resp.contains("Access-Control-Allow-Origin: *\r\n"));
        assert!(lobby_resp.contains("\"room\":\"p2p-1\""));

        let sig_body = r#"{"from":"host","to":"guest1","kind":"offer","payload":"v=0"}"#;
        let sig_req = format!(
            "POST /lobbies/p2p-1/signal HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            sig_body.len(),
            sig_body
        );
        let sig_resp = String::from_utf8(tcp_http_request(http_addr, &sig_req)).unwrap();
        assert!(sig_resp.starts_with("HTTP/1.1 200 OK\r\n"));

        let poll_resp = String::from_utf8(tcp_http_request(
            http_addr,
            "GET /lobbies/p2p-1/signal?since=0&to=guest1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
        ))
        .unwrap();
        assert!(poll_resp.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(poll_resp.contains("\"kind\":\"offer\""));

        let del_resp = String::from_utf8(tcp_http_request(
            http_addr,
            "DELETE /lobbies/p2p-1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
        ))
        .unwrap();
        assert!(del_resp.starts_with("HTTP/1.1 200 OK\r\n"));

        running.store(false, Ordering::Relaxed);
        handle.join().unwrap();
    }
}
