//! Test game shared by the engine's integration tests.
#![allow(dead_code)]

use cs_engine::{EngineApi, GameDll, PrecacheError};
use cs_proto48::delta::{GoldSrcClientData, GoldSrcUserCmd};
use cs_proto48::usermsg::UserMsgRegistry;
use glam::Vec3;

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
}
