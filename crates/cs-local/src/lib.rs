//! Local play: the engine (`cs-engine`) and the game (`cs-game`) in the client's own process,
//! and a [`cs_clientcore::Connection`] that reaches them over a [`LoopbackHub`] with the real
//! Protocol 48 handshake. The client code is the one that plays over UDP; only the transport
//! differs.
//!
//! Time is the caller's: [`LocalGame::step`] runs one 10 ms frame, so a test (or a window's
//! loop) decides how many frames pass and nothing reads a clock.

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
        }
    }
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
    let bsp = Arc::new(BspMap::load(vfs, name).map_err(|e| format!("cannot load map {name}: {e}"))?);
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
    let wads: Vec<Vec<String>> = ["valve", "cstrike"].into_iter().filter_map(lumps).collect();
    decal_names_from_wads(&wads)
}

/// The server as every Protocol 48 front end starts it (`cs-server`, local play): the engine
/// on `map` with `game`, the Protocol 48 deltas, the vanilla cvars, the decal list of the game
/// directory and the game files as the studio model loader (the players' hitboxes).
pub fn new_server<G: GameDll>(cfg: ServerConfig, map: ServerMap, game: G, vfs: &Vfs) -> Server<G> {
    new_server_with(cfg, map, game, Some(vfs))
}

fn new_server_with<G: GameDll>(
    cfg: ServerConfig,
    map: ServerMap,
    game: G,
    vfs: Option<&Vfs>,
) -> Server<G> {
    let mut server = Server::new(cfg, map, game, DeltaRegistry::cs16_default(), vanilla_cvars());
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
/// and optionally hosting unified WASM + AMXX mods via [`ServerModHost`].
pub struct LocalGame {
    server: LocalServer,
    net: MultiTransport,
    p2p: MultiPeerHandle,
    conn: Box<Connection<LoopbackEndpoint>>,
    vfs: Option<Vfs>,
    now: f64,
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
        if opts.mods_dir.is_some() || opts.amxx_dir.is_some() || opts.plugins_toml.is_some() {
            let mut host = ServerModHost::new();
            if let Some(ref v) = vfs {
                let _ = host.load_configs_dir(v.root().join("cstrike"));
            }
            if let Some(ref toml_path) = opts.plugins_toml {
                let _ = host.load_plugins_toml(toml_path)?;
                if let Some(ref mut v) = vfs {
                    for (mod_id, entries) in host.loaded_mod_assets() {
                        v.mount_mod_assets(&mod_id, entries)
                            .map_err(|e| format!("failed to mount assets for mod `{mod_id}`: {e}"))?;
                    }
                }
            }
            if let Some(ref dir) = opts.mods_dir {
                let pkgs = cs_mod_host::discover_packages(dir)
                    .map_err(|e| format!("failed to scan mods dir `{}`: {e}", dir.display()))?;
                host.set_mods_dir(dir);
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
                }
            }
            if let Some(ref dir) = opts.amxx_dir {
                let _ = host.load_amxx_dir(dir)?;
            }
            return Self::start_with_host(map, vfs, opts, host);
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
        let server = LocalServer::Vanilla(Box::new(new_server_with(cfg, map, game, vfs.as_ref())));
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
        let server = LocalServer::Modded(Box::new(new_server_with(cfg, map, modded, vfs.as_ref())));
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
                panic!("LocalGame is running a modded server; use modded_server_mut() or cs_game_mut()")
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
        assert_eq!(CVarRegistry::new_with_defaults().get_f32("mp_roundtime"), 1.75);
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
        assert!(guest_in_game, "external PeerAddr::Web(1) should complete signon");
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
}
