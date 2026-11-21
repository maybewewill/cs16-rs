//! `cs-game`: the game logic of Counter-Strike 1.6 behind the server engine's [`GameDll`]
//! boundary (the `mp.dll` of the original), ported from ReGameDLL_CS. It reaches the engine only
//! through [`EngineApi`].
//!
//! This is the foundation: the entity registry ([`entity`]), the world and the static entities
//! of a map ([`world`]) and [`CsGame`], which dispatches the engine's calls to the entities. The
//! players are in [`player`]: their lifecycle, damage, death and movement; [`teams`] and [`menus`]
//! take them from the team menu into the game, and [`rules`] runs the rounds, the match and the
//! map cycle (`CHalfLifeMultiplay`).

pub mod entity;
pub mod mapcycle;
pub mod menus;
mod messages;
pub mod player;
pub mod rules;
pub mod teams;
pub mod world;

use cs_engine::{EdictId, EngineApi, GameDll};
use cs_proto48::delta::{GoldSrcClientData, GoldSrcUserCmd};
use cs_proto48::usermsg::UserMsgRegistry;
use cs_sim::pmove::MoveParams;
use glam::Vec3;

pub use entity::{GameEntity, create_entity};

use entity::{attach, with_entity};
use player::{MAX_PLAYERS, Player};

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
    /// The players, by slot, from `client_put_in_server` to `client_disconnect`.
    pub players: [Option<Player>; MAX_PLAYERS],
    /// The message of the day, the bytes of `motd.txt` (`motdfile`): empty for none. See
    /// [`CsGame::load_motd`].
    pub motd: Vec<u8>,
    /// `m_iNumTerroristWins`: the rounds the terrorists have won, for `TeamScore`.
    pub num_terrorist_wins: i32,
    /// `m_iNumCTWins`: the rounds the counter-terrorists have won, for `TeamScore`.
    pub num_ct_wins: i32,
    /// The state of the round rules (`CHalfLifeMultiplay`); new for every map.
    pub rules: rules::Rules,
    /// `mapcycle.txt`: the maps the match goes through. Outlives the map ([`CsGame::load_map_cycle`]).
    pub map_cycle: mapcycle::MapCycle,
    /// How many players have entered the server since the game started (the user id the team
    /// balance orders by).
    pub(crate) join_counter: u32,
    /// State of the game's own random numbers (`RANDOM_LONG`).
    rng: u32,
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
            players: std::array::from_fn(|_| None),
            motd: Vec::new(),
            num_terrorist_wins: 0,
            num_ct_wins: 0,
            rules: rules::Rules::default(),
            map_cycle: mapcycle::MapCycle::default(),
            join_counter: 0,
            rng: 0x2545_f491,
            #[cfg(feature = "test-hooks")]
            dispatch_log: Vec::new(),
        }
    }

    /// A random number in `lo..=hi` (xorshift32: the engine offers the game no generator).
    pub(crate) fn random_range(&mut self, lo: i32, hi: i32) -> i32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        let span = (hi - lo + 1).max(1) as u32;
        lo + (self.rng % span) as i32
    }

    /// Forgets what belonged to the previous map.
    fn reset_map(&mut self) {
        self.ladders.clear();
        self.last_ct_spawn = None;
        self.last_t_spawn = None;
        self.num_terrorist_wins = 0;
        self.num_ct_wins = 0;
        self.rules = rules::Rules::default();
    }
}

/// The game: [`GameState`] (with the players) and the entities (in the private data of their
/// edicts).
pub struct CsGame {
    state: GameState,
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
        }
    }

    /// Reads the message of the day, `motd.txt` of the game directories (`LOAD_FILE_FOR_ME` of
    /// the `motdfile` cvar, which is not read: the name is fixed at its default), through
    /// `vfs`. A missing file means no
    /// message of the day, as in `SendMOTDToClient`, which then sends the server name alone.
    pub fn load_motd(&mut self, vfs: &cs_vfs::Vfs) {
        self.state.motd = match vfs.read_bytes("motd.txt") {
            Ok(bytes) => bytes,
            Err(cs_vfs::VfsError::NotFound(_)) => Vec::new(),
            Err(err) => {
                tracing::warn!(%err, "motd.txt could not be read");
                Vec::new()
            }
        };
    }

    /// Reads the map cycle, `mapcycle.txt`, through `vfs` (see [`mapcycle::MapCycle::load`]); the
    /// match asks for the next map of it when it ends.
    pub fn load_map_cycle(&mut self, vfs: &cs_vfs::Vfs) {
        self.state.map_cycle = mapcycle::MapCycle::load(vfs);
    }

    pub fn state(&self) -> &GameState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut GameState {
        &mut self.state
    }

    /// Spawns the player of `slot` on `team` with appearance `class` (1 to 4, 0 the team's
    /// default), as the team selection will once it exists. For tests only (cargo feature
    /// `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn spawn_player(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        team: player::Team,
        class: u8,
    ) -> bool {
        if let Some(p) = self
            .state
            .players
            .get_mut(usize::from(slot))
            .and_then(Option::as_mut)
        {
            teams::mark_joined(p);
        }
        player::spawn(&mut self.state, eng, slot, team, class)
    }

    /// Respawns the player of `slot` on its team (`respawn()`). For tests only (cargo feature
    /// `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn respawn_player(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> bool {
        player::respawn_player(&mut self.state, eng, slot)
    }

    /// Hurts the player of `slot` as `attacker` (a player's slot; `None` the world) would, with
    /// `TakeDamage`. For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn damage_player(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        attacker: Option<u8>,
        damage: f32,
        bits: u32,
    ) -> bool {
        let attacker = attacker.map(|a| eng.client_edict(a));
        player::take_damage(&mut self.state, eng, slot, attacker, attacker, damage, bits)
    }

    /// Puts the movement of the player of `slot` at `origin`, stopped, to place a player
    /// mid-air or elsewhere in tests. `false` when nobody is in that slot. For tests only
    /// (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn place_player(&mut self, eng: &mut EngineApi<'_>, slot: u8, origin: Vec3) -> bool {
        let Some(p) = self.state.players.get_mut(usize::from(slot)).and_then(Option::as_mut)
        else {
            return false;
        };
        p.mv.origin = origin;
        p.mv.velocity = Vec3::ZERO;
        p.mv.on_ground = false;
        let id = eng.client_edict(slot);
        if let Some(v) = eng.ent_mut(id) {
            v.velocity = Vec3::ZERO;
            v.flags &= !cs_engine::consts::FL_ONGROUND;
        }
        eng.set_origin(id, origin);
        true
    }
}

impl GameDll for CsGame {
    fn usermsgs(&self) -> UserMsgRegistry {
        messages::link_user_messages()
    }

    /// Every entity of the map has spawned: finds the ladders and starts the rules of the map
    /// (the freeze period before the first round).
    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        self.state.ladders = world::ladder_bounds(eng);
        self.state.rules = rules::Rules::new(eng);
    }

    fn server_deactivate(&mut self, _eng: &mut EngineApi<'_>) {
        self.state.players = std::array::from_fn(|_| None);
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

    /// `CHalfLifeMultiplay::ClientConnected`: every client may connect.
    fn client_connect(&mut self, _eng: &mut EngineApi<'_>, _slot: u8) -> Result<(), String> {
        Ok(())
    }

    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        player::put_in_server(&mut self.state, eng, slot);
    }

    fn client_disconnect(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        player::disconnect(&mut self.state, eng, slot);
    }

    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
        player::client_command(&mut self.state, eng, slot, argv);
    }

    fn client_userinfo_changed(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        player::userinfo_changed(&mut self.state, eng, slot);
    }

    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        _random_seed: u32,
    ) {
        player::run_usercmd(&mut self.state, eng, slot, cmd);
    }

    /// `CHalfLifeMultiplay::Think`: the clock of the round and the match.
    fn start_frame(&mut self, eng: &mut EngineApi<'_>, _frametime: f32) {
        rules::think(&mut self.state, eng);
    }

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
        player::client_data(&mut self.state, eng, slot)
    }
}
