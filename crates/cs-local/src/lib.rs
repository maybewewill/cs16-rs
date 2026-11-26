//! Local play: the engine (`cs-engine`) and the game (`cs-game`) in the client's own process,
//! and a [`cs_clientcore::Connection`] that reaches them over a [`LoopbackHub`] with the real
//! Protocol 48 handshake. The client code is the one that plays over UDP; only the transport
//! differs.
//!
//! Time is the caller's: [`LocalGame::step`] runs one 10 ms frame, so a test (or a window's
//! loop) decides how many frames pass and nothing reads a clock.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use cs_assets::bsp::BspMap;
use cs_clientcore::{ConnState, Connection, ConnectionConfig};
use cs_cvar::CVarRegistry;
use cs_engine::{
    ConsoleOutcome, GameDll, Server, ServerConfig, ServerMap, decal_names_from_wads, wad_lump_names,
};
use cs_game::CsGame;
use cs_mod_host::{ModdedGame, ServerModHost};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::DeltaRegistry;
use cs_transport::{
    LoopbackEndpoint, LoopbackHub, MultiPeerChannel, MultiPeerHandle, MultiTransport,
    PacketTransport, PeerAddr, PeerKind,
};
use cs_vfs::Vfs;

/// One server frame, in seconds (the engine's 100 Hz).
pub const FRAME_SECS: f64 = 0.01;
/// Frames [`LocalGame::start`] runs at most while the client signs on.
const SIGNON_FRAMES: usize = 2000;

/// What a local game starts with.
#[derive(Debug, Clone)]
pub struct LocalOptions {
    /// Player slots of the listen server, the local player included.
    pub max_clients: u8,
    /// `bot_quota`: the bots the game keeps in play (0 for none).
    pub bots: u32,
    pub hostname: String,
    pub player_name: String,
    /// Ask the game for text menus (`ShowMenu`, answered with `menuselect`) instead of VGUI
    /// ones: `_vgui_menus 0` in the userinfo, set before the sign-on.
    pub text_menus: bool,
    /// Optional directory containing WASM / `.csmod` mods for the listen server.
    pub mods_dir: Option<PathBuf>,
    /// Optional directory containing `.amxx` plugins for the listen server.
    pub amxx_dir: Option<PathBuf>,
    /// Optional `configs/plugins.toml` manifest for the unified mod + AMXX host.
    pub plugins_toml: Option<PathBuf>,
    /// Server CVars applied onto [`vanilla_cvars`] BEFORE `server_activate` and host sign-on
    /// (e.g. `mp_startmoney`, `mp_freezetime`, `mp_roundtime`, `mp_buytime`, `bot_difficulty`).
    pub initial_cvars: Vec<(String, String)>,
    /// Start the listen server with a unified [`ServerModHost`] + pure-Rust Python 3 runtime
    /// attached even when no disk directories are configured (enables live `< 5 ms` hot-reload
    /// in offline sessions and the browser `/playground/`).
    pub enable_mods: bool,
    /// In-memory Python 3 plugins `(name, source)` loaded before `server_activate` so
    /// `ServerActivate` / `RoundStart` / `PlayerSpawn` hooks run from frame 0.
    pub initial_python_plugins: Vec<(String, String)>,
}

impl Default for LocalOptions {
    fn default() -> Self {
        Self {
            max_clients: 16,
            bots: 0,
            hostname: "cs16-rs local game".to_string(),
            player_name: "Player".to_string(),
            text_menus: false,
            mods_dir: None,
            amxx_dir: None,
            plugins_toml: None,
            initial_cvars: Vec::new(),
            enable_mods: false,
            initial_python_plugins: Vec::new(),
        }
    }
}

/// Structured report returned after validating or hot-reloading a Python 3 plugin.
#[derive(Debug, Clone, PartialEq)]
pub struct PythonReloadReport {
    pub id: String,
    pub name: String,
    pub version: String,
    pub load_ms: f64,
    pub hook_count: usize,
    pub chat_command_count: usize,
    pub console_command_count: usize,
    pub timer_count: usize,
    pub hooks: Vec<String>,
    pub chat_commands: Vec<String>,
    pub console_commands: Vec<String>,
}

impl PythonReloadReport {
    /// Serializes this report to compact JSON for JS/browser consumption.
    pub fn to_json(&self, queued: bool) -> String {
        let mut out = String::with_capacity(256);
        let _ = write!(
            out,
            "{{\"ok\":true,\"id\":\"{}\",\"name\":\"{}\",\"version\":\"{}\",\"load_ms\":{:.3},\"queued\":{},\"hook_count\":{},\"chat_command_count\":{},\"console_command_count\":{},\"timer_count\":{},\"hooks\":",
            json_escape(&self.id),
            json_escape(&self.name),
            json_escape(&self.version),
            self.load_ms,
            queued,
            self.hook_count,
            self.chat_command_count,
            self.console_command_count,
            self.timer_count,
        );
        write_json_str_array(&mut out, &self.hooks);
        out.push_str(",\"chat_commands\":");
        write_json_str_array(&mut out, &self.chat_commands);
        out.push_str(",\"console_commands\":");
        write_json_str_array(&mut out, &self.console_commands);
        out.push('}');
        out
    }
}

/// One log, reload notification, or hook fault entry emitted by the mod host.
#[derive(Debug, Clone, PartialEq)]
pub struct ModLogEntry {
    pub level: String,
    pub mod_id: String,
    pub time_secs: f64,
    pub message: String,
}

/// Escapes a string for JSON embedding.
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn write_json_str_array(out: &mut String, items: &[String]) {
    out.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(&json_escape(item));
        out.push('"');
    }
    out.push(']');
}

/// Serializes a slice of [`ModLogEntry`] items into a JSON array.
pub fn mod_logs_to_json(entries: &[ModLogEntry]) -> String {
    let mut out = String::with_capacity(entries.len() * 96 + 2);
    out.push('[');
    for (i, e) in entries.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"level\":\"{}\",\"mod_id\":\"{}\",\"time\":{:.2},\"message\":\"{}\"}}",
            json_escape(&e.level),
            json_escape(&e.mod_id),
            e.time_secs,
            json_escape(&e.message),
        );
    }
    out.push(']');
    out
}

/// Derives `(fallback_id, file_name)` from a user-supplied plugin name (`"vip_menu.py"` or `"vip_menu"`).
pub fn normalize_python_plugin_name(name: &str) -> (String, String) {
    let trimmed = name.trim();
    let stem = trimmed
        .strip_suffix(".py")
        .or_else(|| trimmed.strip_suffix(".PY"))
        .unwrap_or(trimmed);
    let sanitized: String = stem
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    let fallback_id = if sanitized.is_empty() {
        "playground_mod".to_string()
    } else {
        sanitized
    };
    let file_name = format!("{fallback_id}.py");
    (fallback_id, file_name)
}

/// Validates Python plugin source in a bounded dry-run VM and extracts metadata, decorator names,
/// and hook/command/timer counts without touching any live server.
pub fn inspect_python_plugin(name: &str, code: &str) -> Result<PythonReloadReport, String> {
    #[cfg(not(target_arch = "wasm32"))]
    let started = std::time::Instant::now();

    let (fallback_id, file_name) = normalize_python_plugin_name(name);
    let check =
        cs_mod_py::PySourceSyntaxCheck::check(&file_name, code).map_err(|err| err.to_string())?;
    let id = check
        .meta
        .id
        .as_deref()
        .map(|raw| normalize_python_plugin_name(raw).0)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_id);
    let display_name = check.meta.name.unwrap_or_else(|| id.clone());
    let version = check.meta.version.unwrap_or_else(|| "1.0.0".to_string());
    let (hooks, chat_commands, console_commands) = extract_python_decorators(code);

    #[cfg(not(target_arch = "wasm32"))]
    let load_ms = started.elapsed().as_secs_f64() * 1000.0;
    #[cfg(target_arch = "wasm32")]
    let load_ms = 0.0;

    Ok(PythonReloadReport {
        id,
        name: display_name,
        version,
        load_ms,
        hook_count: check.hook_count,
        chat_command_count: check.chat_command_count,
        console_command_count: check.console_command_count,
        timer_count: check.timer_count,
        hooks,
        chat_commands,
        console_commands,
    })
}

fn extract_python_decorators(code: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut hooks = Vec::new();
    let mut chat_cmds = Vec::new();
    let mut console_cmds = Vec::new();
    for raw_line in code.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("@mod.hook(") {
            if let Some(label) = extract_first_quoted(rest)
                && !hooks.contains(&label)
            {
                hooks.push(label);
            }
        } else if let Some(rest) = line.strip_prefix("@mod.chat_command(") {
            for label in extract_all_quoted(rest) {
                if !chat_cmds.contains(&label) {
                    chat_cmds.push(label);
                }
            }
        } else if let Some(rest) = line.strip_prefix("@mod.console_command(") {
            for label in extract_all_quoted(rest) {
                if !console_cmds.contains(&label) {
                    console_cmds.push(label);
                }
            }
        }
    }
    (hooks, chat_cmds, console_cmds)
}

fn extract_first_quoted(s: &str) -> Option<String> {
    extract_all_quoted(s).into_iter().next()
}

fn extract_all_quoted(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b')' {
            break;
        }
        if b == b'"' || b == b'\'' {
            let quote = b;
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i <= bytes.len()
                && i > start
                && let Ok(text) = std::str::from_utf8(&bytes[start..i])
            {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
            if i < bytes.len() {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// The cvars a Protocol 48 server starts with: the shared defaults with the vanilla
/// `mp_roundtime` (5 minutes in ReGameDLL `game.cpp`; the shared registry keeps 1.75) and no
/// bots until asked. Applied before the first `server_activate`, so a value the operator sets
/// later in the console wins.
pub fn vanilla_cvars() -> CVarRegistry {
    let mut cvars = CVarRegistry::new_with_defaults();
    cvars.set("mp_roundtime", "5");
    cvars.set("bot_quota", "0");
    cvars
}

/// Loads `maps/<name>.bsp` through `vfs` as a server map.
pub fn load_server_map(vfs: &Vfs, name: &str) -> Result<ServerMap, String> {
    let bsp =
        Arc::new(BspMap::load(vfs, name).map_err(|e| format!("cannot load map {name}: {e}"))?);
    let path = format!("maps/{name}.bsp");
    let raw = vfs
        .read_bytes(&path)
        .map_err(|e| format!("cannot read {path}: {e}"))?;
    ServerMap::from_bsp(name, &raw, bsp).ok_or(format!("{path} has no valid BSP header"))
}

/// The decal names the engine holds (`Decal_Init`, ReHLDS `rehlds/engine/decals.cpp`, read
/// 2026-11-22): the lump names of `valve/decals.wad` (`DEFAULTGAME`) and then of
/// `cstrike/decals.wad` (`GAME`), merged as the engine does. A wad that is missing or is not a
/// WAD3 is left out; with neither, the list is empty and `decal_index` answers -1.
pub fn load_decal_names(vfs: &Vfs) -> Vec<String> {
    let lumps = |dir: &str| {
        std::fs::read(vfs.root().join(dir).join("decals.wad"))
            .ok()
            .and_then(|bytes| wad_lump_names(&bytes))
    };
    let mut wads: Vec<Vec<String>> = ["valve", "cstrike"].into_iter().filter_map(lumps).collect();
    if wads.is_empty()
        && let Ok(bytes) = vfs.read_bytes("decals.wad")
        && let Some(names) = wad_lump_names(&bytes)
    {
        wads.push(names);
    }
    decal_names_from_wads(&wads)
}

/// The server as every Protocol 48 front end starts it (`cs-server`, local play): the engine
/// on `map` with `game`, the Protocol 48 deltas, the vanilla cvars, the decal list of the game
/// directory and the game files as the studio model loader (the players' hitboxes).
pub fn new_server<G: GameDll>(cfg: ServerConfig, map: ServerMap, game: G, vfs: &Vfs) -> Server<G> {
    new_server_with(cfg, map, game, Some(vfs), &[])
}

fn new_server_with<G: GameDll>(
    cfg: ServerConfig,
    map: ServerMap,
    game: G,
    vfs: Option<&Vfs>,
    initial_cvars: &[(String, String)],
) -> Server<G> {
    let mut cvars = vanilla_cvars();
    for (k, v) in initial_cvars {
        if !v.is_empty() {
            cvars.set(k, v);
        }
    }
    let mut server = Server::new(cfg, map, game, DeltaRegistry::cs16_default(), cvars);
    if let Some(vfs) = vfs {
        server.set_decals(load_decal_names(vfs));
        let model_vfs = vfs.clone();
        server.set_model_loader(Arc::new(move |name| model_vfs.read_bytes(name).ok()));
    }
    server
}

enum LocalServer {
    Vanilla(Box<Server<CsGame>>),
    Modded(Box<Server<ModdedGame<CsGame>>>),
}

impl LocalServer {
    fn set_available_maps(&mut self, maps: Vec<String>) {
        match self {
            Self::Vanilla(s) => s.set_available_maps(maps),
            Self::Modded(s) => s.set_available_maps(maps),
        }
    }

    fn console_command(&mut self, line: &str) -> ConsoleOutcome {
        match self {
            Self::Vanilla(s) => s.console_command(line),
            Self::Modded(s) => s.console_command(line),
        }
    }

    fn frame(&mut self, net: &mut dyn PacketTransport, now: f64) {
        match self {
            Self::Vanilla(s) => s.frame(net, now),
            Self::Modded(s) => s.frame(net, now),
        }
    }

    fn take_pending_change_level(&mut self) -> Option<String> {
        match self {
            Self::Vanilla(s) => s.take_pending_change_level(),
            Self::Modded(s) => s.take_pending_change_level(),
        }
    }

    fn change_map(&mut self, map: ServerMap) {
        match self {
            Self::Vanilla(s) => s.change_map(map),
            Self::Modded(s) => s.change_map(map),
        }
    }

    fn shutdown(&mut self, net: &mut dyn PacketTransport, reason: &str) {
        match self {
            Self::Vanilla(s) => s.shutdown(net, reason),
            Self::Modded(s) => s.shutdown(net, reason),
        }
    }
}

/// A server and one local client in one process, optionally accepting external P2P WebRTC peers
/// and optionally hosting unified WASM + Python 3 + AMXX mods via [`ServerModHost`].
pub struct LocalGame {
    server: LocalServer,
    net: MultiTransport,
    p2p: MultiPeerHandle,
    conn: Box<Connection<LoopbackEndpoint>>,
    vfs: Option<Vfs>,
    now: f64,
    python_meta: BTreeMap<String, PythonReloadReport>,
    pending_mod_logs: Vec<ModLogEntry>,
    seen_fault_ids: BTreeSet<String>,
}

impl LocalGame {
    /// Starts the stock map `map` of `vfs` and signs the client on; returns once it is in the
    /// game (or an error if the sign-on does not finish).
    pub fn start(vfs: &Vfs, map: &str, opts: &LocalOptions) -> Result<Self, String> {
        let server_map = load_server_map(vfs, map)?;
        Self::start_with_map(server_map, Some(vfs.clone()), opts)
    }

    /// Like [`start`](Self::start) on a map the caller built (a test map). With a `vfs` the
    /// game loads its message of the day and map cycle and the engine its decals and the
    /// players' hitbox models.
    pub fn start_with_map(
        map: ServerMap,
        mut vfs: Option<Vfs>,
        opts: &LocalOptions,
    ) -> Result<Self, String> {
        if opts.enable_mods
            || !opts.initial_python_plugins.is_empty()
            || opts.mods_dir.is_some()
            || opts.amxx_dir.is_some()
            || opts.plugins_toml.is_some()
        {
            let mut host = ServerModHost::new();
            cs_mod_py::attach_python_runtime(&mut host);
            if let Some(ref v) = vfs {
                let _ = host.load_configs_dir(v.root().join("cstrike"));
            }
            if let Some(ref toml_path) = opts.plugins_toml {
                let _ = host.load_plugins_toml(toml_path)?;
                if let Some(ref mut v) = vfs {
                    for (mod_id, entries) in host.loaded_mod_assets() {
                        v.mount_mod_assets(&mod_id, entries).map_err(|e| {
                            format!("failed to mount assets for mod `{mod_id}`: {e}")
                        })?;
                    }
                }
            }
            if let Some(ref dir) = opts.mods_dir {
                let pkgs = cs_mod_host::discover_packages(dir)
                    .map_err(|e| format!("failed to scan mods dir `{}`: {e}", dir.display()))?;
                host.set_mods_dir(dir);
                let mut loaded_ids = std::collections::BTreeSet::new();
                for pkg in pkgs {
                    if let Some(ref mut v) = vfs
                        && !pkg.assets.is_empty()
                    {
                        let entries: Vec<(String, Vec<u8>)> = pkg
                            .assets
                            .iter()
                            .map(|(k, b)| (k.clone(), b.clone()))
                            .collect();
                        v.mount_mod_assets(&pkg.manifest.id, entries).map_err(|e| {
                            format!("failed to mount assets for mod `{}`: {e}", pkg.manifest.id)
                        })?;
                    }
                    let id = pkg.manifest.id.clone();
                    host.load_package(pkg)
                        .map_err(|e| format!("failed to load mod `{id}`: {e}"))?;
                    loaded_ids.insert(id);
                }
                for py_mod in cs_mod_py::discover_python_mods(dir)? {
                    if !loaded_ids.contains(&py_mod.mod_id) {
                        host.load_python_file(&py_mod.script_path)?;
                    }
                }
            }
            if let Some(ref dir) = opts.amxx_dir {
                let _ = host.load_amxx_dir(dir)?;
            }
            let mut initial_meta = BTreeMap::new();
            let mut initial_logs = Vec::new();
            for (name, code) in &opts.initial_python_plugins {
                let (fallback_id, file_name) = normalize_python_plugin_name(name);
                match inspect_python_plugin(name, code) {
                    Ok(mut report) => {
                        match host.load_python_source(&fallback_id, &file_name, code, None) {
                            Ok(loaded_id) => {
                                report.id = loaded_id.clone();
                                initial_logs.push(ModLogEntry {
                                    level: "info".to_string(),
                                    mod_id: loaded_id.clone(),
                                    time_secs: 0.0,
                                    message: format!(
                                        "Loaded `{}` v{} ({} hooks, {} chat cmds)",
                                        report.name,
                                        report.version,
                                        report.hook_count,
                                        report.chat_command_count
                                    ),
                                });
                                initial_meta.insert(loaded_id, report);
                            }
                            Err(err) => {
                                initial_logs.push(ModLogEntry {
                                    level: "error".to_string(),
                                    mod_id: fallback_id,
                                    time_secs: 0.0,
                                    message: err,
                                });
                            }
                        }
                    }
                    Err(err) => {
                        initial_logs.push(ModLogEntry {
                            level: "error".to_string(),
                            mod_id: fallback_id,
                            time_secs: 0.0,
                            message: err,
                        });
                    }
                }
            }
            let mut local = Self::start_with_host(map, vfs, opts, host)?;
            local.python_meta = initial_meta;
            local.pending_mod_logs = initial_logs;
            return Ok(local);
        }

        let mut game = CsGame::new();
        if let Some(vfs) = &vfs {
            game.load_motd(vfs);
            game.load_map_cycle(vfs);
        }
        game.enable_cvar_bot_quota();
        let cfg = ServerConfig {
            hostname: opts.hostname.clone(),
            max_clients: opts.max_clients,
            ..ServerConfig::default()
        };
        let server = LocalServer::Vanilla(Box::new(new_server_with(
            cfg,
            map,
            game,
            vfs.as_ref(),
            &opts.initial_cvars,
        )));
        Self::finish_start(server, vfs, opts)
    }

    /// Starts a modded local listen server backed by a caller-populated unified [`ServerModHost`]
    /// (hosting `.wasm` mods and/or `.amxx` plugins sharing menus, i18n, admins, storage, and RPC).
    pub fn start_with_host(
        map: ServerMap,
        vfs: Option<Vfs>,
        opts: &LocalOptions,
        host: ServerModHost,
    ) -> Result<Self, String> {
        let mut game = CsGame::new();
        if let Some(vfs) = &vfs {
            game.load_motd(vfs);
            game.load_map_cycle(vfs);
        }
        game.enable_cvar_bot_quota();
        let cfg = ServerConfig {
            hostname: opts.hostname.clone(),
            max_clients: opts.max_clients,
            ..ServerConfig::default()
        };
        let modded = ModdedGame::new(game, host);
        let server = LocalServer::Modded(Box::new(new_server_with(
            cfg,
            map,
            modded,
            vfs.as_ref(),
            &opts.initial_cvars,
        )));
        Self::finish_start(server, vfs, opts)
    }

    fn finish_start(
        mut server: LocalServer,
        vfs: Option<Vfs>,
        opts: &LocalOptions,
    ) -> Result<Self, String> {
        if let Some(vfs) = &vfs {
            server.set_available_maps(vfs.list_maps());
        }
        if opts.bots > 0 {
            let _ = server.console_command(&format!("bot_quota {}", opts.bots));
        }
        let hub = LoopbackHub::new();
        let endpoint = hub.connect();
        let (p2p_chan, p2p) = MultiPeerChannel::new_pair();
        let mut net = MultiTransport::new();
        net.add(PeerKind::Loopback, hub);
        net.add(PeerKind::Web, p2p_chan);
        let cfg = ConnectionConfig::new(
            &opts.player_name,
            76_561_197_960_265_728 + u64::from(endpoint.id()),
            &md5_hex(b"cs16-rs local game"),
        );
        let mut conn = Box::new(Connection::new(endpoint, PeerAddr::Loopback(0), cfg));
        if opts.text_menus {
            conn.set_userinfo("_vgui_menus", "0");
        }
        conn.connect();
        let mut local = Self {
            server,
            net,
            p2p,
            conn,
            vfs,
            now: 0.0,
            python_meta: BTreeMap::new(),
            pending_mod_logs: Vec::new(),
            seen_fault_ids: BTreeSet::new(),
        };
        for _ in 0..SIGNON_FRAMES {
            local.step();
            if local.conn.state() == &ConnState::InGame {
                return Ok(local);
            }
            if local.conn.state().is_terminal() {
                break;
            }
        }
        Err(format!(
            "the local client did not get into the game: {:?}",
            local.conn.state()
        ))
    }

    /// Runs one frame: the client sends what it has, then the server runs 10 ms and answers.
    /// The clock is whole frames, so it never drifts.
    pub fn step(&mut self) {
        self.conn.pump(self.now);
        self.now = ((self.now / FRAME_SECS).round() + 1.0) * FRAME_SECS;
        self.server.frame(&mut self.net, self.now);
        // A map change the game asked for (`CHANGE_LEVEL`), done as the console's `changelevel`.
        if let Some(name) = self.server.take_pending_change_level() {
            self.change_level(&name);
        }
    }

    /// Queues an inbound Protocol 48 packet from an external P2P peer (`PeerAddr::Web(peer_id)`).
    pub fn push_p2p_packet(&self, peer_id: u32, bytes: Vec<u8>) -> bool {
        self.p2p.push_inbound(peer_id, bytes).is_ok()
    }

    /// Drains outbound `(peer_id, bytes)` packets addressed to external P2P peers.
    pub fn drain_p2p_packets(&self) -> Vec<(u32, Vec<u8>)> {
        self.p2p.drain_outbound()
    }

    /// External P2P multi-peer handle (`PeerAddr::Web(1..=16)`).
    pub fn p2p_handle(&self) -> &MultiPeerHandle {
        &self.p2p
    }

    /// Runs `frames` frames.
    pub fn run_frames(&mut self, frames: usize) {
        for _ in 0..frames {
            self.step();
        }
    }

    /// Runs a server console line (`bot_quota 4`, `mp_roundtime 1`, `changelevel de_dust2`).
    /// Returns what it printed.
    pub fn console(&mut self, line: &str) -> String {
        match self.server.console_command(line) {
            ConsoleOutcome::Output(text) => text,
            ConsoleOutcome::ChangeLevel(name) => {
                self.change_level(&name);
                String::new()
            }
            ConsoleOutcome::Quit => {
                self.server.shutdown(&mut self.net, "Server shutting down");
                self.p2p.close();
                String::new()
            }
        }
    }

    fn change_level(&mut self, name: &str) {
        let Some(vfs) = &self.vfs else {
            return;
        };
        // A map that does not load leaves the server on its current one.
        if let Ok(map) = load_server_map(vfs, name) {
            self.server.change_map(map);
        }
    }

    /// The client's connection: state, entities, the tables, the command queue.
    pub fn connection(&self) -> &Connection<LoopbackEndpoint> {
        &self.conn
    }

    pub fn connection_mut(&mut self) -> &mut Connection<LoopbackEndpoint> {
        &mut self.conn
    }

    /// The vanilla server the client plays on.
    ///
    /// Panics if this [`LocalGame`] was started with mods/AMXX enabled; use [`cs_game`](Self::cs_game)
    /// or [`modded_server`](Self::modded_server) for modded listen servers.
    pub fn server(&self) -> &Server<CsGame> {
        match &self.server {
            LocalServer::Vanilla(s) => s,
            LocalServer::Modded(_) => {
                panic!("LocalGame is running a modded server; use modded_server() or cs_game()")
            }
        }
    }

    pub fn server_mut(&mut self) -> &mut Server<CsGame> {
        match &mut self.server {
            LocalServer::Vanilla(s) => s,
            LocalServer::Modded(_) => {
                panic!(
                    "LocalGame is running a modded server; use modded_server_mut() or cs_game_mut()"
                )
            }
        }
    }

    /// Returns `true` when this local listen server hosts WASM and/or AMXX plugins.
    pub fn is_modded(&self) -> bool {
        matches!(self.server, LocalServer::Modded(_))
    }

    /// Returns the modded server when mods/AMXX are loaded.
    pub fn modded_server(&self) -> Option<&Server<ModdedGame<CsGame>>> {
        match &self.server {
            LocalServer::Modded(s) => Some(s),
            LocalServer::Vanilla(_) => None,
        }
    }

    pub fn modded_server_mut(&mut self) -> Option<&mut Server<ModdedGame<CsGame>>> {
        match &mut self.server {
            LocalServer::Modded(s) => Some(s),
            LocalServer::Vanilla(_) => None,
        }
    }

    /// Returns the unified [`ServerModHost`] when this local session runs in modded mode.
    pub fn mod_host(&self) -> Option<&ServerModHost> {
        self.modded_server().map(|s| &s.game().host)
    }

    /// Mutable access to the unified [`ServerModHost`] when this local session runs in modded mode.
    pub fn mod_host_mut(&mut self) -> Option<&mut ServerModHost> {
        self.modded_server_mut().map(|s| &mut s.game_mut().host)
    }

    /// Compiles, validates, and hot-swaps a Python 3 plugin directly on the active listen server
    /// while preserving `mod.storage`. Syntax or load errors leave any previously running plugin
    /// and the active match untouched.
    pub fn reload_python_plugin(
        &mut self,
        name: &str,
        code: &str,
    ) -> Result<PythonReloadReport, String> {
        let now = self.now;
        let (fallback_id, file_name) = normalize_python_plugin_name(name);

        #[cfg(not(target_arch = "wasm32"))]
        let started = std::time::Instant::now();

        let mut report = match inspect_python_plugin(name, code) {
            Ok(r) => r,
            Err(err) => {
                self.pending_mod_logs.push(ModLogEntry {
                    level: "error".to_string(),
                    mod_id: fallback_id,
                    time_secs: now,
                    message: err.clone(),
                });
                return Err(err);
            }
        };

        let host = self
            .mod_host_mut()
            .ok_or_else(|| "local session is running without mod host enabled".to_string())?;
        if !host.has_python_runtime() {
            cs_mod_py::attach_python_runtime(host);
        }

        let loaded_id = match host.load_python_source(&fallback_id, &file_name, code, None) {
            Ok(id) => id,
            Err(err) => {
                self.pending_mod_logs.push(ModLogEntry {
                    level: "error".to_string(),
                    mod_id: report.id,
                    time_secs: now,
                    message: err.clone(),
                });
                return Err(err);
            }
        };

        #[cfg(not(target_arch = "wasm32"))]
        {
            report.load_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
        #[cfg(target_arch = "wasm32")]
        {
            report.load_ms = 0.0;
        }

        report.id = loaded_id.clone();
        self.pending_mod_logs.push(ModLogEntry {
            level: "info".to_string(),
            mod_id: loaded_id.clone(),
            time_secs: now,
            message: format!(
                "Hot-reloaded `{}` v{} in {:.2} ms ({} hooks, {} chat cmds)",
                report.name,
                report.version,
                report.load_ms,
                report.hook_count,
                report.chat_command_count
            ),
        });
        self.python_meta.insert(loaded_id, report.clone());
        Ok(report)
    }

    /// Unloads a loaded Python plugin by its manifest id or script name (`"vip_menu.py"`).
    pub fn unload_python_plugin(&mut self, name: &str) -> bool {
        let now = self.now;
        let (norm_id, _) = normalize_python_plugin_name(name);
        let Some(host) = self.mod_host_mut() else {
            return false;
        };
        let target = if host.unload_mod(name) {
            Some(name.to_string())
        } else if norm_id != name && host.unload_mod(&norm_id) {
            Some(norm_id)
        } else {
            None
        };
        if let Some(id) = target {
            self.python_meta.remove(&id);
            self.pending_mod_logs.push(ModLogEntry {
                level: "info".to_string(),
                mod_id: id.clone(),
                time_secs: now,
                message: format!("Unloaded plugin `{id}`"),
            });
            true
        } else {
            false
        }
    }

    /// Snapshot of live per-plugin ResMon metrics across all loaded mods.
    pub fn mod_resmon_snapshot(&self) -> Vec<cs_mod_host::PluginResmon> {
        self.mod_host()
            .map(ServerModHost::resmon_snapshot)
            .unwrap_or_default()
    }

    /// Live per-plugin ResMon metrics serialized as JSON (`active`, `count`, `plugins`).
    pub fn mod_resmon_json(&self) -> String {
        let rows = self.mod_resmon_snapshot();
        let mut out = String::with_capacity(rows.len() * 220 + 48);
        let _ = write!(
            out,
            "{{\"active\":{},\"count\":{},\"plugins\":[",
            self.is_modded(),
            rows.len()
        );
        for (idx, row) in rows.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            let meta = self.python_meta.get(&row.id);
            let display_name = meta.map_or(row.id.as_str(), |m| m.name.as_str());
            let _ = write!(
                out,
                "{{\"id\":\"{}\",\"name\":\"{}\",\"version\":\"{}\",\"language\":\"{}\",\"status\":\"{}\",\"enabled\":{},\"hook_calls\":{},\"avg_us\":{:.2},\"p99_us\":{},\"max_us\":{},\"fuel_used\":{},\"memory_bytes\":{},\"fault_count\":{},\"hooks\":",
                json_escape(&row.id),
                json_escape(display_name),
                json_escape(&row.version),
                json_escape(&row.language),
                json_escape(&row.status),
                row.enabled,
                row.hook_calls,
                row.avg_us_per_tick,
                row.p99_us_per_tick,
                row.max_us,
                row.fuel_used,
                row.memory_bytes,
                row.fault_count,
            );
            if let Some(m) = meta {
                write_json_str_array(&mut out, &m.hooks);
                out.push_str(",\"chat_commands\":");
                write_json_str_array(&mut out, &m.chat_commands);
                out.push_str(",\"console_commands\":");
                write_json_str_array(&mut out, &m.console_commands);
            } else {
                out.push_str("[],\"chat_commands\":[],\"console_commands\":[]");
            }
            out.push('}');
        }
        out.push_str("]}");
        out
    }

    /// Drains plugin `print`/`mod.log` lines, newly recorded hook faults, and reload notifications.
    pub fn drain_mod_logs(&mut self) -> Vec<ModLogEntry> {
        let mut out = std::mem::take(&mut self.pending_mod_logs);
        let now = self.now;
        let (wasi_logs, faults) = match self.mod_host_mut() {
            Some(host) => (host.take_wasi_logs(), host.list_faults()),
            None => return out,
        };
        for (mod_id, line) in wasi_logs {
            out.push(ModLogEntry {
                level: "log".to_string(),
                mod_id,
                time_secs: now,
                message: line,
            });
        }
        for summary in faults {
            if self.seen_fault_ids.insert(summary.id.clone()) {
                let detail = self
                    .mod_host()
                    .and_then(|h| h.get_fault(&summary.id))
                    .map(|d| {
                        if d.stack_trace.is_empty() {
                            d.error.clone()
                        } else {
                            format!("{}\n{}", d.error, d.stack_trace.join("\n"))
                        }
                    })
                    .unwrap_or_else(|| summary.error.clone());
                out.push(ModLogEntry {
                    level: "error".to_string(),
                    mod_id: summary.mod_id,
                    time_secs: now,
                    message: detail,
                });
            }
        }
        out
    }

    /// Drains mod logs and returns them encoded as a JSON array.
    pub fn drain_mod_logs_json(&mut self) -> String {
        let logs = self.drain_mod_logs();
        mod_logs_to_json(&logs)
    }

    /// Direct reference to the underlying [`CsGame`] regardless of whether mods are active.
    pub fn cs_game(&self) -> &CsGame {
        match &self.server {
            LocalServer::Vanilla(s) => s.game(),
            LocalServer::Modded(s) => &s.game().inner,
        }
    }

    pub fn cs_game_mut(&mut self) -> &mut CsGame {
        match &mut self.server {
            LocalServer::Vanilla(s) => s.game_mut(),
            LocalServer::Modded(s) => &mut s.game_mut().inner,
        }
    }

    /// Server edict pool regardless of whether mods are active.
    pub fn edicts(&self) -> &cs_engine::EdictPool {
        match &self.server {
            LocalServer::Vanilla(s) => s.edicts(),
            LocalServer::Modded(s) => s.edicts(),
        }
    }

    /// Configured maximum client slots regardless of whether mods are active.
    pub fn max_clients(&self) -> u8 {
        match &self.server {
            LocalServer::Vanilla(s) => s.config().max_clients,
            LocalServer::Modded(s) => s.config().max_clients,
        }
    }

    /// Teleports a connected client/bot slot to `origin` regardless of whether mods are active.
    pub fn set_client_origin(&mut self, slot: u8, origin: glam::Vec3) -> bool {
        match &mut self.server {
            LocalServer::Vanilla(s) => s.set_client_origin(slot, origin),
            LocalServer::Modded(s) => s.set_client_origin(slot, origin),
        }
    }

    /// Server time in seconds.
    pub fn time(&self) -> f64 {
        self.now
    }

    /// Stops the server, telling the client why.
    pub fn shutdown(&mut self) {
        self.server.shutdown(&mut self.net, "Server shutting down");
        self.p2p.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vanilla_round_time() {
        // A Protocol 48 server starts at the vanilla 5 minutes; the shared registry keeps 1.75.
        assert_eq!(vanilla_cvars().get_f32("mp_roundtime"), 5.0);
        assert_eq!(vanilla_cvars().get_f32("bot_quota"), 0.0);
        assert_eq!(
            CVarRegistry::new_with_defaults().get_f32("mp_roundtime"),
            1.75
        );
    }

    #[test]
    fn local_game_accepts_external_web_peer_alongside_loopback_host() {
        let opts = LocalOptions {
            max_clients: 4,
            bots: 0,
            text_menus: true,
            ..LocalOptions::default()
        };
        let mut local = LocalGame::start_with_map(ServerMap::stub("de_dust2"), None, &opts)
            .expect("host signs on");
        assert_eq!(local.connection().state(), &ConnState::InGame);

        let (guest_tr, guest_handle) = cs_transport::ChannelTransport::new_pair(PeerAddr::Web(0));
        let mut guest = Connection::new(
            guest_tr,
            PeerAddr::Web(0),
            ConnectionConfig::new("P2PGuest", 76_561_197_960_265_900, &md5_hex(b"p2p-guest")),
        );
        guest.connect();

        let mut guest_in_game = false;
        for _ in 0..400 {
            guest.pump(local.time());
            for pkt in guest_handle.drain_outbound() {
                assert!(local.push_p2p_packet(1, pkt));
            }
            local.step();
            for (peer_id, pkt) in local.drain_p2p_packets() {
                assert_eq!(peer_id, 1);
                let _ = guest_handle.push_inbound(pkt);
            }
            if guest.state() == &ConnState::InGame {
                guest_in_game = true;
                break;
            }
        }
        assert!(
            guest_in_game,
            "external PeerAddr::Web(1) should complete signon"
        );
        assert_eq!(local.server().client_infos().len(), 2);
        assert!(!local.is_modded());
    }

    #[test]
    fn local_game_supports_unified_modded_host_while_default_stays_vanilla() {
        let host = ServerModHost::new();
        let opts = LocalOptions {
            max_clients: 4,
            bots: 0,
            text_menus: true,
            ..LocalOptions::default()
        };
        let mut local = LocalGame::start_with_host(ServerMap::stub("de_dust2"), None, &opts, host)
            .expect("modded local game signs on");
        assert!(local.is_modded());
        assert!(local.modded_server().is_some());
        assert_eq!(local.connection().state(), &ConnState::InGame);
        let out = local.console("mod_list");
        assert!(out.contains("no mods loaded"), "out={out}");
        assert!(local.cs_game().state().players[0].is_some());
    }

    #[test]
    fn dm_rusthouse_hulls_spawns_and_5v5_zbot_combat() {
        use cs_assets::demo_pack::{DEMO_MAP_NAME, build_instant_demo_pack};
        use cs_engine::WorldCollision;
        use cs_engine::collision::{CONTENTS_EMPTY, Hull};
        use glam::Vec3;

        let pack = build_instant_demo_pack();
        let vfs = pack.to_vfs();
        let bsp = BspMap::load(&vfs, DEMO_MAP_NAME).expect("load dm_rusthouse.bsp");
        let coll = WorldCollision::from_bsp(&bsp);

        // 1. Every CT & T spawn point must be in empty space (never embedded in solid) across all 4 hulls.
        assert_eq!(bsp.spawns.len(), 16);
        for (idx, sp) in bsp.spawns.iter().enumerate() {
            for hull in [Hull::Point, Hull::Standing, Hull::Large, Hull::Crouched] {
                let contents = coll.point_contents(sp.position, hull);
                assert_eq!(
                    contents, CONTENTS_EMPTY,
                    "spawn #{idx} {:?} embedded in solid for hull {hull:?} (contents={contents})",
                    sp.position
                );
                let tr_down =
                    coll.trace_world(sp.position, sp.position - Vec3::new(0.0, 0.0, 64.0), hull);
                assert!(
                    !tr_down.start_solid && !tr_down.all_solid && tr_down.fraction < 1.0,
                    "spawn #{idx} hull {hull:?} floor trace failed: {tr_down:?}"
                );
                assert!(
                    tr_down.plane_normal.z > 0.99,
                    "spawn #{idx} hull {hull:?} should land on flat floor, normal={:?}",
                    tr_down.plane_normal
                );
            }
        }

        // 2. Verify walkable slope trace on the West ramp (`normal.z >= 0.7`) and central bridge top (`z = 64`).
        let ramp_tr = coll.trace_world(
            Vec3::new(-192.0, 0.0, 128.0),
            Vec3::new(-192.0, 0.0, 0.0),
            Hull::Standing,
        );
        assert!(
            !ramp_tr.start_solid && ramp_tr.fraction < 1.0 && ramp_tr.plane_normal.z >= 0.7,
            "west ramp standing hull trace should hit walkable slope: {ramp_tr:?}"
        );
        let bridge_tr = coll.trace_world(
            Vec3::new(0.0, 0.0, 160.0),
            Vec3::new(0.0, 0.0, 0.0),
            Hull::Standing,
        );
        assert!(
            !bridge_tr.start_solid
                && bridge_tr.fraction < 1.0
                && (bridge_tr.end_pos.z - 100.0).abs() < 1.0,
            "bridge standing floor should be at z = 64 + 36 = 100, got {:?}",
            bridge_tr.end_pos
        );
        // Verify bullet ray (Hull 0) hits cover crate cleanly and `trace_texture` finds surface name.
        let crate_ray = coll.trace_world(
            Vec3::new(-450.0, -288.0, 48.0),
            Vec3::new(-200.0, -288.0, 48.0),
            Hull::Point,
        );
        assert!(
            !crate_ray.start_solid && crate_ray.fraction < 1.0 && crate_ray.hit_solid,
            "Hull 0 bullet ray should hit NW crate: {crate_ray:?}"
        );
        let floor_tex =
            coll.trace_texture(0, Vec3::new(0.0, 200.0, 64.0), Vec3::new(0.0, 200.0, -16.0));
        assert_eq!(floor_tex, Some("RH_FLOOR"));

        // 3. Boot a 5v5 ZBot match (9 bots + local player) on `dm_rusthouse` and run 600+ ticks.
        let opts = LocalOptions {
            max_clients: 12,
            bots: 9,
            text_menus: true,
            ..LocalOptions::default()
        };
        let mut local = LocalGame::start(&vfs, DEMO_MAP_NAME, &opts).expect("start dm_rusthouse");
        assert_eq!(local.connection().state(), &ConnState::InGame);
        let _ = local.console("mp_freezetime 0");
        local.connection_mut().execute_local("jointeam 2");
        local.run_frames(5);
        local.connection_mut().execute_local("joinclass 1");
        // `bot_quota` adds bots staggered over time (~0.35s each); wait until all 9 join.
        for _ in 0..800 {
            local.step();
            if local.server().fake_client_count() >= 9 {
                break;
            }
        }
        assert_eq!(
            local.server().fake_client_count(),
            9,
            "all 9 ZBots should join for 5v5 match"
        );
        let _ = local.console("sv_restart 1");
        local.run_frames(130); // complete 1s restart + spawn all 10 players

        assert!(
            local.cs_game().bot_nav_built(),
            "dm_rusthouse.nav should be loaded"
        );

        let snapshot_bot_positions = |g: &LocalGame| -> Vec<(u8, Vec3)> {
            let sv = g.server();
            (0..12u8)
                .filter_map(|slot| {
                    if !sv.is_fake_client(slot) {
                        return None;
                    }
                    let eid = sv.edicts().client(slot);
                    let pos = sv.edicts().get(eid)?.vars.origin;
                    Some((slot, pos))
                })
                .collect()
        };
        let initial_pos = snapshot_bot_positions(&local);
        assert!(
            initial_pos.len() >= 8,
            "expected at least 8 active ZBots, got {}",
            initial_pos.len()
        );

        let mut any_bot_fired = false;
        let mut any_damage_or_kill = false;
        let mut max_moved_bots = 0usize;

        for _ in 0..8 {
            for _ in 0..100 {
                local.step();
                let sv = local.server();
                for &(slot, _) in &initial_pos {
                    let eid = sv.edicts().client(slot);
                    if let Some(e) = sv.edicts().get(eid)
                        && (e.vars.button & 1) != 0
                    {
                        any_bot_fired = true;
                    }
                }
            }
            let now_pos = snapshot_bot_positions(&local);
            let moved = now_pos
                .iter()
                .filter(|(slot, p)| {
                    initial_pos
                        .iter()
                        .find(|(s, _)| s == slot)
                        .is_some_and(|(_, p0)| (*p - *p0).truncate().length() > 64.0)
                })
                .count();
            max_moved_bots = max_moved_bots.max(moved);

            let st = local.cs_game().state();
            let sv = local.server();
            for &(slot, _) in &initial_pos {
                if let Some(p) = st.players[usize::from(slot)].as_ref()
                    && (p.deaths > 0 || p.frags != 0 || p.health < 100.0)
                {
                    any_bot_fired = true;
                    any_damage_or_kill = true;
                }
                let eid = sv.edicts().client(slot);
                if let Some(e) = sv.edicts().get(eid)
                    && e.vars.health < 100.0
                {
                    any_damage_or_kill = true;
                }
            }
            if max_moved_bots >= 6 && any_bot_fired && any_damage_or_kill {
                break;
            }
        }

        assert!(
            max_moved_bots >= 6,
            "expected >= 6 ZBots actively navigating dm_rusthouse (>64u moved), got {max_moved_bots}"
        );
        assert!(
            any_bot_fired,
            "expected ZBots to fire weapons on dm_rusthouse"
        );
        assert!(
            any_damage_or_kill,
            "expected ZBots to engage in combat (damage/frags) on dm_rusthouse"
        );
    }
}
