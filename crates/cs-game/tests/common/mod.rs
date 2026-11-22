//! A server on a map running a `Rig` (a `CsGame` that also runs closures the test queued, at the
//! start of the next frame) with one probe client in slot 0. Shared by the integration tests.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use cs_assets::bsp::{BspMap, Entity};
use cs_cvar::CVarRegistry;
use cs_engine::{EdictId, EngineApi, GameDll, Server, ServerConfig, ServerMap};
use cs_game::CsGame;
use cs_probe::{ProbeClient, ProbeConfig, ProbeState};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcUserCmd};
use cs_proto48::messages::SvcMessage;
use cs_proto48::usermsg::{CsUserMsg, UserMsgRegistry};
use cs_transport::{LoopbackEndpoint, LoopbackHub, PeerAddr};
use cs_vfs::{Vfs, VfsConfig};

pub type Action = Box<dyn FnOnce(&mut CsGame, &mut EngineApi<'_>)>;

/// `CsGame` plus the closures the test queued, which run at the start of the next frame.
pub struct Rig {
    pub game: CsGame,
    pub actions: Vec<Action>,
    /// Event scripts to precache before the game activates (the game does not precache the
    /// weapons' events yet).
    pub precache_events: Vec<String>,
}

impl GameDll for Rig {
    fn usermsgs(&self) -> UserMsgRegistry {
        self.game.usermsgs()
    }
    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        for name in &self.precache_events {
            eng.precache_event(name);
        }
        self.game.server_activate(eng);
    }
    fn server_deactivate(&mut self, eng: &mut EngineApi<'_>) {
        self.game.server_deactivate(eng);
    }
    fn spawn_entity(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        classname: &str,
        keys: &[(String, String)],
    ) -> bool {
        self.game.spawn_entity(eng, id, classname, keys)
    }
    fn client_connect(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Result<(), String> {
        self.game.client_connect(eng, slot)
    }
    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_put_in_server(eng, slot);
    }
    fn client_disconnect(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_disconnect(eng, slot);
    }
    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
        self.game.client_command(eng, slot, argv);
    }
    fn client_userinfo_changed(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.game.client_userinfo_changed(eng, slot);
    }
    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        random_seed: u32,
    ) {
        self.game.run_usercmd(eng, slot, cmd, random_seed);
    }
    fn post_usercmd(&mut self, eng: &mut EngineApi<'_>, slot: u8, cmd: &GoldSrcUserCmd) {
        self.game.post_usercmd(eng, slot, cmd);
    }
    fn start_frame(&mut self, eng: &mut EngineApi<'_>, frametime: f32) {
        for action in std::mem::take(&mut self.actions) {
            action(&mut self.game, eng);
        }
        self.game.start_frame(eng, frametime);
    }
    fn think(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        self.game.think(eng, id);
    }
    fn touch(&mut self, eng: &mut EngineApi<'_>, toucher: EdictId, other: EdictId) {
        self.game.touch(eng, toucher, other);
    }
    fn blocked(&mut self, eng: &mut EngineApi<'_>, pusher: EdictId, other: EdictId) {
        self.game.blocked(eng, pusher, other);
    }
    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData {
        self.game.update_client_data(eng, slot)
    }
    fn setup_visibility(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Option<glam::Vec3> {
        self.game.setup_visibility(eng, slot)
    }
}

/// A server on a map running a [`Rig`], with one probe client in slot 0, and its clock.
pub struct Bench {
    pub server: Server<Rig>,
    pub hub: LoopbackHub,
    pub probe: ProbeClient<LoopbackEndpoint>,
    pub now: f64,
    /// Whether each frame the probe sends a command with time in it (a client does; the game's
    /// per-player logic runs from the commands).
    pub idle_cmds: bool,
}

impl Bench {
    /// A two-slot server on `map`; the probe has entered the game when this returns.
    pub fn on(map: ServerMap) -> Self {
        Self::with_slots(map, 2)
    }

    /// A server of `max_clients` slots on `map`; the probe (slot 0) has entered the game when
    /// this returns, the other slots are free until a test puts a player in them.
    pub fn with_slots(map: ServerMap, max_clients: u8) -> Self {
        Self::with_userinfo(map, max_clients, &[])
    }

    /// Like [`with_slots`](Self::with_slots), the probe's `connect` userinfo carrying `userinfo`
    /// in place of its defaults.
    pub fn with_userinfo(map: ServerMap, max_clients: u8, userinfo: &[(&str, &str)]) -> Self {
        Self::with_probe_moves(map, max_clients, userinfo, true, &[])
    }

    /// A stub-map server whose probe sends no `clc_move` at all (so the game runs no command
    /// of it) until `probe.suppress_moves(false)`.
    pub fn stub_without_moves(entities: Vec<Entity>) -> Self {
        let map = ServerMap::stub_with_entities("test", entities);
        Self::with_probe_moves(map, 2, &[], false, &[])
    }

    /// Like [`stub_without_moves`](Self::stub_without_moves), the probe not predicting
    /// (`cl_lw 0`, so it is sent the events of its own shots) and the event scripts `events`
    /// precached.
    pub fn stub_with_events(entities: Vec<Entity>, events: &[&str]) -> Self {
        let map = ServerMap::stub_with_entities("test", entities);
        Self::with_probe_moves(map, 2, &[("cl_lw", "0")], false, events)
    }

    fn with_probe_moves(
        map: ServerMap,
        max_clients: u8,
        userinfo: &[(&str, &str)],
        send_moves: bool,
        events: &[&str],
    ) -> Self {
        let cfg = ServerConfig {
            max_clients,
            max_edicts: 256,
            ..ServerConfig::default()
        };
        Self::with_config_and_events(map, cfg, userinfo, send_moves, events)
    }

    /// A server of `cfg` on `map` with the probe in the game (`with_probe_moves` with the whole
    /// configuration given).
    pub fn with_config(
        map: ServerMap,
        cfg: ServerConfig,
        userinfo: &[(&str, &str)],
        send_moves: bool,
    ) -> Self {
        Self::with_config_and_events(map, cfg, userinfo, send_moves, &[])
    }

    /// [`with_config`](Self::with_config) with the event scripts `events` precached before the
    /// game activates.
    fn with_config_and_events(
        map: ServerMap,
        cfg: ServerConfig,
        userinfo: &[(&str, &str)],
        send_moves: bool,
        events: &[&str],
    ) -> Self {
        let rig = Rig {
            game: CsGame::new(),
            actions: Vec::new(),
            precache_events: events.iter().map(|e| e.to_string()).collect(),
        };
        let mut server = Server::new(
            cfg,
            map,
            rig,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        let mut hub = LoopbackHub::new();
        let endpoint = hub.connect();
        let cfg = ProbeConfig {
            name: "p".to_string(),
            steam_id: 76_561_197_960_265_728 + u64::from(endpoint.id()),
            cdkey_md5_hex: md5_hex(b"probe"),
        };
        let mut probe = ProbeClient::new(endpoint, PeerAddr::Loopback(0), cfg);
        for (key, value) in userinfo {
            probe.set_userinfo(key, value);
        }
        probe.suppress_moves(!send_moves);
        probe.connect();
        let mut now = 0.0;
        for _ in 0..300 {
            probe.pump(now);
            now = ((now / 0.01).round() + 1.0) * 0.01;
            server.frame(&mut hub, now);
            if probe.state() == &ProbeState::InGame {
                break;
            }
        }
        assert_eq!(probe.state(), &ProbeState::InGame);
        Self {
            server,
            hub,
            probe,
            now,
            idle_cmds: false,
        }
    }

    /// Connects a second probe client (the next free slot) and pumps both until it is in the
    /// game. The bench's own `step` pumps only the first probe: use
    /// [`step_both`](Self::step_both) from then on.
    pub fn add_probe(&mut self, name: &str) -> ProbeClient<LoopbackEndpoint> {
        let endpoint = self.hub.connect();
        let cfg = ProbeConfig {
            name: name.to_string(),
            steam_id: 76_561_197_960_265_728 + u64::from(endpoint.id()),
            cdkey_md5_hex: md5_hex(name.as_bytes()),
        };
        let mut probe = ProbeClient::new(endpoint, PeerAddr::Loopback(0), cfg);
        probe.connect();
        for _ in 0..400 {
            self.step_both(&mut probe, 1);
            if probe.state() == &ProbeState::InGame {
                return probe;
            }
        }
        panic!("the second probe did not get into the game");
    }

    /// Like [`step`](Self::step) with `other` pumped as well.
    pub fn step_both(&mut self, other: &mut ProbeClient<LoopbackEndpoint>, frames: usize) {
        for _ in 0..frames {
            self.probe.pump(self.now);
            other.pump(self.now);
            self.now = ((self.now / 0.01).round() + 1.0) * 0.01;
            self.server.frame(&mut self.hub, self.now);
        }
    }

    pub fn stub(entities: Vec<Entity>) -> Self {
        Self::on(ServerMap::stub_with_entities("test", entities))
    }

    /// A server of `max_clients` slots on a stub map with `entities`.
    pub fn stub_slots(entities: Vec<Entity>, max_clients: u8) -> Self {
        Self::with_slots(ServerMap::stub_with_entities("test", entities), max_clients)
    }

    /// Runs `frames` server frames (10 ms each) with the probe pumped.
    pub fn step(&mut self, frames: usize) {
        for _ in 0..frames {
            if self.idle_cmds && self.probe.state() == &ProbeState::InGame {
                self.probe.queue_cmd(GoldSrcUserCmd {
                    msec: 10,
                    ..Default::default()
                });
            }
            self.probe.pump(self.now);
            self.now = ((self.now / 0.01).round() + 1.0) * 0.01;
            self.server.frame(&mut self.hub, self.now);
        }
    }

    /// Runs `f` as the game at the start of the next frame and returns what it returned.
    pub fn run<T: 'static>(
        &mut self,
        f: impl FnOnce(&mut CsGame, &mut EngineApi<'_>) -> T + 'static,
    ) -> T {
        let out = Rc::new(RefCell::new(None));
        let sink = Rc::clone(&out);
        self.server
            .game_mut()
            .actions
            .push(Box::new(move |game, eng| {
                *sink.borrow_mut() = Some(f(game, eng))
            }));
        self.step(1);
        let result = out.borrow_mut().take();
        result.expect("the game ran the action")
    }

    /// Puts a second client in the server (slot 1), as the engine does when one signs on.
    pub fn enter_second_client(&mut self) {
        self.run(|game, eng| game.client_put_in_server(eng, 1));
    }

    /// The game's user messages the probe has received, in order.
    pub fn messages(&self) -> Vec<CsUserMsg> {
        self.probe
            .messages()
            .iter()
            .filter_map(|m| match m {
                SvcMessage::UserMsg { decoded, .. } => Some(decoded.clone()),
                _ => None,
            })
            .collect()
    }
}

pub fn entity(pairs: &[(&str, &str)]) -> Entity {
    Entity {
        properties: pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<HashMap<_, _>>(),
    }
}

pub fn spawn_at(class: &str, origin: &str) -> Entity {
    entity(&[
        ("classname", class),
        ("origin", origin),
        ("angles", "0 90 0"),
    ])
}

/// A stub map with the world and a few spawn points per team.
pub fn stub_spawns() -> Vec<Entity> {
    vec![
        entity(&[("classname", "worldspawn")]),
        spawn_at("info_player_start", "100 0 0"),
        spawn_at("info_player_start", "300 0 0"),
        spawn_at("info_player_deathmatch", "-100 0 0"),
        spawn_at("info_player_deathmatch", "-300 0 0"),
    ]
}

/// A stub map with the world and `ct` counter-terrorist and `t` terrorist spawn points.
pub fn spawns(ct: usize, t: usize) -> Vec<Entity> {
    let mut v = vec![entity(&[("classname", "worldspawn")])];
    for i in 0..ct {
        v.push(spawn_at(
            "info_player_start",
            &format!("{} 0 0", 100 + 200 * i as i32),
        ));
    }
    for i in 0..t {
        v.push(spawn_at(
            "info_player_deathmatch",
            &format!("{} 0 0", -100 - 200 * i as i32),
        ));
    }
    v
}

pub fn discover() -> Option<Vfs> {
    Vfs::discover(None, VfsConfig::default()).ok()
}

pub fn dust2(vfs: &Vfs) -> ServerMap {
    map_named(vfs, "de_dust2")
}

/// The stock map `name` as a server map.
pub fn map_named(vfs: &Vfs, name: &str) -> ServerMap {
    let bsp = BspMap::load(vfs, name).unwrap_or_else(|e| panic!("load {name}: {e:?}"));
    let raw = vfs
        .read_bytes(&format!("maps/{name}.bsp"))
        .unwrap_or_else(|e| panic!("read {name}: {e:?}"));
    ServerMap::from_bsp(name, &raw, Arc::new(bsp)).expect("BSP header")
}

/// The first installed map (in name order) whose entity lump has an entity of class `class`:
/// scanned with `cs-assets`, not guessed.
pub fn find_map_with_entity(vfs: &Vfs, class: &str) -> Option<String> {
    vfs.list_maps().into_iter().find(|name| {
        BspMap::load(vfs, name)
            .is_ok_and(|bsp| bsp.entities.iter().any(|e| e.classname() == class))
    })
}

/// Runs the console command `line` as the player of `slot`, at the start of the next frame, with
/// two commands with time in them (the game's per-player logic runs from the commands), then
/// lets the probe read what the game sent.
pub fn command(b: &mut Bench, slot: u8, line: &str) {
    let argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
    b.run(move |game, eng| {
        game.client_command(eng, slot, &argv);
        commands_of(game, eng, slot, 2);
    });
    b.step(3);
}

/// `n` commands with time in them from the player of `slot` (the probe of slot 0 sends its own
/// every frame with `idle_cmds`; the other slots have no client).
pub fn commands_of(game: &mut CsGame, eng: &mut EngineApi<'_>, slot: u8, n: usize) {
    for _ in 0..n {
        let cmd = GoldSrcUserCmd {
            msec: 10,
            ..Default::default()
        };
        run_cmd(game, eng, slot, &cmd, 0);
    }
}

/// Puts a player in `slot` and lets it reach the team menu.
pub fn enter(b: &mut Bench, slot: u8) {
    b.run(move |game, eng| {
        game.client_put_in_server(eng, slot);
        commands_of(game, eng, slot, 3);
    });
    b.step(2);
}

/// The player of `slot` picks `team_choice` (1 terrorists, 2 counter-terrorists) and then
/// appearance `class`.
pub fn join(b: &mut Bench, slot: u8, team_choice: u8, class: u8) {
    command(b, slot, &format!("jointeam {team_choice}"));
    command(b, slot, &format!("joinclass {class}"));
    b.step(2);
}

/// One command as the engine runs it: `run_usercmd`, then (the touches left out) `post_usercmd`.
pub fn run_cmd(
    game: &mut CsGame,
    eng: &mut EngineApi<'_>,
    slot: u8,
    cmd: &GoldSrcUserCmd,
    random_seed: u32,
) {
    game.run_usercmd(eng, slot, cmd, random_seed);
    game.post_usercmd(eng, slot, cmd);
}
