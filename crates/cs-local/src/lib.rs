//! Local play: the engine (`cs-engine`) and the game (`cs-game`) in the client's own process,
//! and a [`cs_clientcore::Connection`] that reaches them over a [`LoopbackHub`] with the real
//! Protocol 48 handshake. The client code is the one that plays over UDP; only the transport
//! differs.
//!
//! Time is the caller's: [`LocalGame::step`] runs one 10 ms frame, so a test (or a window's
//! loop) decides how many frames pass and nothing reads a clock.

use std::sync::Arc;

use cs_assets::bsp::BspMap;
use cs_clientcore::{ConnState, Connection, ConnectionConfig};
use cs_cvar::CVarRegistry;
use cs_engine::{
    ConsoleOutcome, GameDll, Server, ServerConfig, ServerMap, decal_names_from_wads, wad_lump_names,
};
use cs_game::CsGame;
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::DeltaRegistry;
use cs_transport::{
    LoopbackEndpoint, LoopbackHub, MultiPeerChannel, MultiPeerHandle, MultiTransport, PeerAddr,
    PeerKind,
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
}

impl Default for LocalOptions {
    fn default() -> Self {
        Self {
            max_clients: 16,
            bots: 0,
            hostname: "cs16-rs local game".to_string(),
            player_name: "Player".to_string(),
            text_menus: false,
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

/// A server and one local client in one process, optionally accepting external P2P WebRTC peers.
pub struct LocalGame {
    server: Box<Server<CsGame>>,
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
        vfs: Option<Vfs>,
        opts: &LocalOptions,
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
        let mut server = Box::new(new_server_with(cfg, map, game, vfs.as_ref()));
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

    /// The server the client plays on.
    pub fn server(&self) -> &Server<CsGame> {
        &self.server
    }

    pub fn server_mut(&mut self) -> &mut Server<CsGame> {
        &mut self.server
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
    }
}
