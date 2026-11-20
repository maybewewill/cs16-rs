//! Test game and harness shared by the engine's integration tests.
#![allow(dead_code)]

use cs_cvar::CVarRegistry;
use cs_engine::{EngineApi, GameDll, PrecacheError, Server, ServerConfig, ServerMap};
use cs_probe::{ProbeClient, ProbeConfig};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd};
use cs_proto48::usermsg::UserMsgRegistry;
use cs_transport::{LoopbackEndpoint, LoopbackHub, PeerAddr};
use glam::Vec3;

/// Seconds of one harness frame.
pub const FRAME: f64 = 0.01;

/// A `Server<TestGame>` on a stub map plus a loopback hub the probes connect through.
pub struct Harness {
    pub server: Server<TestGame>,
    pub hub: LoopbackHub,
    pub now: f64,
}

impl Harness {
    pub fn new(max_clients: u8) -> Self {
        Self::with_game(max_clients, TestGame::default())
    }

    pub fn with_game(max_clients: u8, game: TestGame) -> Self {
        let cfg = ServerConfig {
            max_clients,
            ..ServerConfig::default()
        };
        let server = Server::new(
            cfg,
            ServerMap::stub("test"),
            game,
            DeltaRegistry::cs16_default(),
            CVarRegistry::new_with_defaults(),
        );
        Self {
            server,
            hub: LoopbackHub::new(),
            now: 0.0,
        }
    }

    /// A probe on a new loopback endpoint; `connect()` was already called.
    pub fn add_probe(&mut self, name: &str) -> ProbeClient<LoopbackEndpoint> {
        let endpoint = self.hub.connect();
        let cfg = ProbeConfig {
            name: name.to_string(),
            steam_id: 76_561_197_960_265_728 + u64::from(endpoint.id()),
            cdkey_md5_hex: md5_hex(format!("probe {}", endpoint.id()).as_bytes()),
        };
        let mut probe = ProbeClient::new(endpoint, PeerAddr::Loopback(0), cfg);
        probe.connect();
        probe
    }

    /// Advances `frames` frames of [`FRAME`] seconds: each pumps every listed probe, then
    /// runs one server frame. Probes not listed are not pumped.
    pub fn step(&mut self, probes: &mut [&mut ProbeClient<LoopbackEndpoint>], frames: usize) {
        for _ in 0..frames {
            // Counted in whole frames so the clock does not drift.
            self.now = ((self.now / FRAME).round() + 1.0) * FRAME;
            for probe in probes.iter_mut() {
                probe.pump(self.now);
            }
            self.server.frame(&mut self.hub, self.now);
        }
    }
}

/// Minimal game: places players on a line, moves them forward by
/// `forwardmove`, and records every call.
#[derive(Debug, Default)]
pub struct TestGame {
    /// Reason `client_connect` refuses with, if any.
    pub refuse: Option<String>,
    /// Entities with `modelindex = 2` that `server_activate` creates.
    pub extra_entities: usize,
    /// Result of `precache_model("models/late.mdl")` in `client_put_in_server`.
    pub late_precache: Option<Result<u16, PrecacheError>>,
    /// Every `argv` received by `client_command`.
    pub commands: Vec<Vec<String>>,

    pub server_activate_calls: usize,
    pub server_deactivate_calls: usize,
    pub client_connect_calls: usize,
    pub client_put_in_server_calls: usize,
    pub client_disconnect_calls: usize,
    pub client_command_calls: usize,
    pub client_userinfo_changed_calls: usize,
    pub run_usercmd_calls: usize,
    pub start_frame_calls: usize,
    pub update_client_data_calls: usize,
    pub add_to_full_pack_calls: usize,
}

impl GameDll for TestGame {
    fn usermsgs(&self) -> UserMsgRegistry {
        UserMsgRegistry::cs16_default()
    }

    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        self.server_activate_calls += 1;
        // Lands at model index 2 on a stub map.
        eng.precache
            .precache_model("models/test.mdl")
            .expect("precache before lock");
        for n in 1..=self.extra_entities {
            let id = eng.edicts.alloc(eng.time).expect("free edict");
            let vars = &mut eng.edicts.get_mut(id).expect("just allocated").vars;
            vars.classname = "test_extra".to_string();
            vars.modelindex = 2;
            vars.origin = Vec3::new(0.0, 0.0, 16.0 * n as f32);
        }
    }

    fn server_deactivate(&mut self, _eng: &mut EngineApi<'_>) {
        self.server_deactivate_calls += 1;
    }

    fn client_connect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> Result<(), String> {
        self.client_connect_calls += 1;
        match &self.refuse {
            Some(text) => Err(text.clone()),
            None => Ok(()),
        }
    }

    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.client_put_in_server_calls += 1;
        let id = eng.edicts.client(slot);
        if let Some(edict) = eng.edicts.get_mut(id) {
            edict.vars.origin = Vec3::new(64.0 * f32::from(slot), 0.0, 0.0);
            edict.vars.modelindex = 2;
            edict.vars.health = 100.0;
        }
        self.late_precache = Some(eng.precache.precache_model("models/late.mdl"));
    }

    fn client_disconnect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {
        self.client_disconnect_calls += 1;
    }

    fn client_command(&mut self, _eng: &mut EngineApi<'_>, _slot: u8, argv: &[String]) {
        self.client_command_calls += 1;
        self.commands.push(argv.to_vec());
    }

    fn client_userinfo_changed(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {
        self.client_userinfo_changed_calls += 1;
    }

    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        _random_seed: u32,
    ) {
        self.run_usercmd_calls += 1;
        let id = eng.edicts.client(slot);
        if let Some(edict) = eng.edicts.get_mut(id) {
            edict.vars.origin.x += cmd.forwardmove * f32::from(cmd.msec) / 1000.0;
        }
    }

    fn start_frame(&mut self, _eng: &mut EngineApi<'_>, _frametime: f32) {
        self.start_frame_calls += 1;
    }

    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData {
        self.update_client_data_calls += 1;
        let id = eng.edicts.client(slot);
        let vars = eng
            .edicts
            .get(id)
            .map(|e| e.vars.clone())
            .unwrap_or_default();
        GoldSrcClientData {
            origin: vars.origin,
            health: vars.health,
            maxspeed: 250.0,
            ..Default::default()
        }
    }

    fn add_to_full_pack(
        &mut self,
        _eng: &mut EngineApi<'_>,
        _viewer: u8,
        _ent: u16,
        _state: &mut GoldSrcEntityState,
    ) -> bool {
        self.add_to_full_pack_calls += 1;
        true
    }
}
