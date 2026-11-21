//! `cs-game`: the game logic of Counter-Strike 1.6 behind the server engine's [`GameDll`]
//! boundary (the `mp.dll` of the original), ported from ReGameDLL_CS. It reaches the engine only
//! through [`EngineApi`].
//!
//! This is the foundation: the entity registry ([`entity`]), the world and the static entities
//! of a map ([`world`]) and [`CsGame`], which dispatches the engine's calls to the entities. The
//! players are a stand-in (`walker.rs`: spawn at a spawn point, walk) until the player task.

pub mod entity;
mod walker;
pub mod world;

use cs_engine::{EdictId, EngineApi, GameDll};
use cs_proto48::delta::{GoldSrcClientData, GoldSrcUserCmd};
use cs_proto48::usermsg::UserMsgRegistry;
use cs_sim::pmove::MoveParams;
use glam::Vec3;

pub use entity::{GameEntity, create_entity};

use entity::{attach, with_entity};
use walker::Walkers;

/// What the game keeps outside its entities: handed to every entity call.
pub struct GameState {
    /// World-space bounds of the map's ladders, found when the map is activated.
    pub ladders: Vec<(Vec3, Vec3)>,
    /// The spawn point the last counter-terrorist spawned at (`g_pLastCTSpawn`).
    pub last_ct_spawn: Option<EdictId>,
    /// The spawn point the last terrorist spawned at (`g_pLastTerroristSpawn`).
    pub last_t_spawn: Option<EdictId>,
    /// Parameters of the player movement.
    pub move_params: MoveParams,
    /// Every call the dispatch delivered to a `test_dispatch` entity. For tests only (feature
    /// `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub dispatch_log: Vec<entity::DispatchEvent>,
}

impl GameState {
    fn new() -> Self {
        Self {
            ladders: Vec::new(),
            last_ct_spawn: None,
            last_t_spawn: None,
            move_params: MoveParams::default(),
            #[cfg(feature = "test-hooks")]
            dispatch_log: Vec::new(),
        }
    }

    /// Forgets what belonged to the previous map.
    fn reset_map(&mut self) {
        self.ladders.clear();
        self.last_ct_spawn = None;
        self.last_t_spawn = None;
    }
}

/// The game: [`GameState`], the entities (in the private data of their edicts) and the players.
pub struct CsGame {
    state: GameState,
    walkers: Walkers,
}

impl Default for CsGame {
    fn default() -> Self {
        Self::new()
    }
}

impl CsGame {
    /// A game with no map: the entities arrive through `spawn_entity` as the engine activates a
    /// map.
    pub fn new() -> Self {
        Self {
            state: GameState::new(),
            walkers: Walkers::default(),
        }
    }

    pub fn state(&self) -> &GameState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut GameState {
        &mut self.state
    }
}

impl GameDll for CsGame {
    fn usermsgs(&self) -> UserMsgRegistry {
        UserMsgRegistry::cs16_default()
    }

    /// Every entity of the map has spawned: finds the ladders.
    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        self.state.ladders = world::ladder_bounds(eng);
    }

    fn server_deactivate(&mut self, _eng: &mut EngineApi<'_>) {
        self.walkers.clear();
        self.state.reset_map();
    }

    /// Makes the entity of class `classname` (see [`create_entity`]) and spawns it. A class the
    /// game does not know, and an entity that refuses to spawn, are refused: the engine frees
    /// the edict.
    fn spawn_entity(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        classname: &str,
        keys: &[(String, String)],
    ) -> bool {
        let Some(mut entity) = create_entity(classname) else {
            tracing::debug!(classname, "map entity of an unknown class removed");
            return false;
        };
        if !entity.spawn(eng, id, keys) {
            return false;
        }
        attach(eng, id, entity);
        true
    }

    fn client_connect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> Result<(), String> {
        Ok(())
    }

    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.walkers.put_in_server(&mut self.state, eng, slot);
    }

    fn client_disconnect(&mut self, _eng: &mut EngineApi<'_>, slot: u8) {
        self.walkers.remove(slot);
    }

    fn client_command(&mut self, _eng: &mut EngineApi<'_>, _slot: u8, _argv: &[String]) {}

    fn client_userinfo_changed(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) {}

    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        _random_seed: u32,
    ) {
        self.walkers.run_usercmd(&self.state, eng, slot, cmd);
    }

    fn start_frame(&mut self, _eng: &mut EngineApi<'_>, _frametime: f32) {}

    fn think(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        with_entity(&mut self.state, eng, id, |entity, g, eng| {
            entity.think(g, eng, id);
        });
    }

    /// `toucher`'s entity is touched by `other`; a toucher with no entity (a player, for now) is
    /// not told.
    fn touch(&mut self, eng: &mut EngineApi<'_>, toucher: EdictId, other: EdictId) {
        with_entity(&mut self.state, eng, toucher, |entity, g, eng| {
            entity.touch(g, eng, toucher, other);
        });
    }

    fn blocked(&mut self, eng: &mut EngineApi<'_>, pusher: EdictId, other: EdictId) {
        with_entity(&mut self.state, eng, pusher, |entity, g, eng| {
            entity.blocked(g, eng, pusher, other);
        });
    }

    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData {
        walker::client_data(eng, slot)
    }
}
