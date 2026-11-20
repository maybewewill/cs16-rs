//! Test game and harness shared by the engine's integration tests.
#![allow(dead_code)]

use std::collections::BTreeSet;

use cs_cvar::CVarRegistry;
use cs_engine::{EngineApi, GameDll, PrecacheError, Server, ServerConfig, ServerMap};
use cs_probe::{ProbeClient, ProbeConfig};
use cs_proto48::crypto::md5_hex;
use cs_proto48::delta::{DeltaRegistry, GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd};
use cs_proto48::messages::{svc, write_svc_print, write_svc_updateuserinfo};
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
        Self::with_config(cfg, game)
    }

    pub fn with_config(cfg: ServerConfig, game: TestGame) -> Self {
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
    /// Reason `client_connect` kicks the connecting slot with (and then accepts), if any.
    pub kick_on_connect: Option<String>,
    /// Bytes `client_command("flood")` writes into that client's reliable buffer.
    pub flood_bytes: usize,
    /// While non-zero, every `run_usercmd` writes this many bytes into that client's reliable
    /// buffer.
    pub flood_per_usercmd: usize,
    /// When set, `client_connect` also writes `flood_bytes` into the connecting slot's
    /// reliable buffer.
    pub flood_on_connect: bool,
    /// When set, `update_client_data` (called for every update sent to an active client)
    /// writes an `svc_nop` to `reliable_all` and sets a userinfo key on that client.
    pub broadcast_every_frame: bool,
    /// When set, `client_put_in_server` writes an `svc_print` of [`SHOUT`] to `reliable_all`.
    pub shout_on_put_in_server: bool,
    /// Entities with `modelindex = 2` that `server_activate` creates.
    pub extra_entities: usize,
    /// When set, the extra entities get `EF_NODRAW` in `effects`.
    pub nodraw_extras: bool,
    /// When set, every `update_client_data` call gives each extra entity field values that
    /// differ from its spawn baseline, from the other extras and from the previous call.
    pub churn_extras: bool,
    /// When set, `client_put_in_server` gives each extra entity field values that differ from
    /// its spawn baseline and from the other extras, once.
    pub scatter_extras: bool,
    /// Entity numbers `add_to_full_pack` was called for.
    pub add_to_full_pack_ents: BTreeSet<u16>,
    /// `(viewer slot, entity number)` of every `add_to_full_pack` call.
    pub add_to_full_pack_pairs: BTreeSet<(u8, u16)>,
    /// Result of `precache_model("models/late.mdl")` in `client_put_in_server`.
    pub late_precache: Option<Result<u16, PrecacheError>>,
    /// Every `argv` received by `client_command`.
    pub commands: Vec<Vec<String>>,
    /// Every `random_seed` received by `run_usercmd`, in call order.
    pub random_seeds: Vec<u32>,

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
            if self.nodraw_extras {
                vars.effects |= EF_NODRAW;
            }
        }
    }

    fn server_deactivate(&mut self, _eng: &mut EngineApi<'_>) {
        self.server_deactivate_calls += 1;
    }

    fn client_connect(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Result<(), String> {
        self.client_connect_calls += 1;
        if let Some(reason) = &self.kick_on_connect {
            eng.kick(slot, reason);
        }
        if self.flood_on_connect {
            eng.reliable(slot).write_bytes(&vec![1; self.flood_bytes]);
        }
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
        if self.shout_on_put_in_server {
            write_svc_print(eng.reliable_all(), SHOUT);
        }
        if self.scatter_extras {
            churn_extras(eng, 0);
        }
    }

    fn client_disconnect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {
        self.client_disconnect_calls += 1;
    }

    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
        self.client_command_calls += 1;
        if argv.first().is_some_and(|c| c == "flood") {
            // `svc_nop` bytes: harmless if they ever reached the client.
            eng.reliable(slot).write_bytes(&vec![1; self.flood_bytes]);
        }
        if argv.first().is_some_and(|c| c == "kickme") {
            eng.kick(slot, GAME_KICK_REASON);
        }
        if argv.first().is_some_and(|c| c == "rekey") {
            // A new value on every call, so every call is a real change.
            let value = (self.client_command_calls).to_string();
            eng.set_client_key(slot, "rekey", &value);
        }
        if argv.first().is_some_and(|c| c == "biginfo") {
            // A userinfo update of slot 15 whose text is `BIG_INFO`.
            write_svc_updateuserinfo(eng.reliable(slot), 15, 99, &big_info(), &[0; 16]);
        }
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
        random_seed: u32,
    ) {
        self.run_usercmd_calls += 1;
        self.random_seeds.push(random_seed);
        if self.flood_per_usercmd > 0 {
            // `svc_nop` bytes.
            eng.reliable(slot).write_bytes(&vec![svc::NOP; self.flood_per_usercmd]);
        }
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
        if self.broadcast_every_frame {
            eng.reliable_all().write_u8(svc::NOP);
            let frame = self.update_client_data_calls.to_string();
            eng.set_client_key(slot, "frame", &frame);
        }
        if self.churn_extras {
            churn_extras(eng, self.update_client_data_calls);
        }
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
        viewer: u8,
        ent: u16,
        _state: &mut GoldSrcEntityState,
    ) -> bool {
        self.add_to_full_pack_calls += 1;
        self.add_to_full_pack_ents.insert(ent);
        self.add_to_full_pack_pairs.insert((viewer, ent));
        true
    }
}

/// Reason of the kick `client_command("kickme")` issues.
pub const GAME_KICK_REASON: &str = "Kicked by the game";

/// Text of the `svc_print` that `shout_on_put_in_server` broadcasts.
pub const SHOUT: &str = "shout\n";

/// Text of the userinfo update `client_command("biginfo")` sends: about 1900 bytes.
pub fn big_info() -> String {
    (0..95).map(|i| format!("\\key{i:03}\\{i:012}")).collect()
}

/// `EF_NODRAW` in `EntVars::effects`.
pub const EF_NODRAW: u32 = 128;

/// Gives every `test_extra` entity values derived from its index and `call`.
fn churn_extras(eng: &mut EngineApi<'_>, call: usize) {
    let ids: Vec<_> = eng
        .edicts
        .iter_live()
        .filter(|(_, e)| e.vars.classname == "test_extra")
        .map(|(id, _)| id)
        .collect();
    let k = call as f32;
    for id in ids {
        let n = f32::from(id.index);
        let vars = &mut eng.edicts.get_mut(id).expect("live").vars;
        vars.origin = Vec3::new(8.0 * n + k, -4.0 * n - k, 16.0 * n + 0.5 * k);
        vars.angles = Vec3::new((n + k) % 90.0, (3.0 * n + k) % 360.0, (n * 7.0) % 180.0);
        vars.frame = (n + k) % 200.0;
        vars.framerate = 1.0 + (n % 7.0) * 0.25;
        vars.sequence = id.index % 50;
        vars.skin = (id.index % 13) as i16;
        vars.body = (id.index % 11) as u8;
        vars.renderamt = (id.index % 251) as u8;
    }
}
