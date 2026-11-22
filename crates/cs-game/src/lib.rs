//! `cs-game`: the game logic of Counter-Strike 1.6 behind the server engine's [`GameDll`]
//! boundary (the `mp.dll` of the original), ported from ReGameDLL_CS. It reaches the engine only
//! through [`EngineApi`].
//!
//! This is the foundation: the entity registry ([`entity`]), the world and the static entities
//! of a map ([`world`]) and [`CsGame`], which dispatches the engine's calls to the entities. The
//! players are in [`player`]: their lifecycle, damage, death and movement; [`teams`] and [`menus`]
//! take them from the team menu into the game, and [`rules`] runs the rounds, the match and the
//! map cycle (`CHalfLifeMultiplay`); [`chat`] carries what the players say to each other, their
//! names and the text messages of the HUD; [`hud_misc`] has the radio, the radar, the status bar
//! and the night vision; [`buy`] has the buy zones and the purchase of the equipment. The dead
//! and the spectators watch through [`observer`].

pub mod bullets;
pub mod buy;
pub mod chat;
pub mod entity;
pub mod hud_misc;
pub mod intro;
pub mod inventory;
pub mod mapcycle;
pub mod menus;
mod messages;
pub mod money;
pub mod observer;
pub mod player;
mod pmove;
pub mod rules;
mod score;
pub mod teams;
pub mod weapons;
pub mod world;

use cs_engine::{EdictId, EngineApi, GameDll};
use cs_proto48::delta::{GoldSrcClientData, GoldSrcUserCmd};
use cs_proto48::usermsg::UserMsgRegistry;
use cs_sim::pm::{PmShared, pm_init};
use glam::Vec3;

pub use entity::{GameEntity, create_entity};

use entity::{attach, with_entity};
use player::{MAX_PLAYERS, Player};

/// What the game keeps outside its entities: handed to every entity call.
pub struct GameState {
    /// The spawn point the last counter-terrorist spawned at (`g_pLastCTSpawn`).
    pub last_ct_spawn: Option<EdictId>,
    /// The spawn point the last terrorist spawned at (`g_pLastTerroristSpawn`).
    pub last_t_spawn: Option<EdictId>,
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
    /// `gInitHUD` (player.cpp:31): true until the first `UpdateClientData` that starts a HUD over
    /// has run, in the life of the game module (it is not reset with the map). `Precache` makes a
    /// player start its HUD over while it is set; `Spawn` does so whatever it is.
    pub(crate) init_hud_global: bool,
    /// State of the game's own random numbers (`RANDOM_LONG`).
    rng: u32,
    /// The statics of `pm_shared` (the stuck table, the texture materials, the wade-step skip
    /// counter), built by `PM_Init` ([`CsGame::load_motd`] reads `sound/materials.txt` for it).
    pm_shared: PmShared,
    /// Every call the dispatch delivered to a `test_dispatch` entity. For tests only (feature
    /// `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub dispatch_log: Vec<entity::DispatchEvent>,
}

impl GameState {
    fn new() -> Self {
        Self {
            last_ct_spawn: None,
            last_t_spawn: None,
            players: std::array::from_fn(|_| None),
            motd: Vec::new(),
            num_terrorist_wins: 0,
            num_ct_wins: 0,
            rules: rules::Rules::default(),
            map_cycle: mapcycle::MapCycle::default(),
            join_counter: 0,
            init_hud_global: true,
            rng: 0x2545_f491,
            pm_shared: pm_init(None),
            #[cfg(feature = "test-hooks")]
            dispatch_log: Vec::new(),
        }
    }

    /// A random number in `lo..=hi` (xorshift32: the engine offers the game no generator).
    pub(crate) fn random_range(&mut self, lo: i32, hi: i32) -> i32 {
        random_in(&mut self.rng, lo, hi)
    }

    /// Forgets what belonged to the previous map.
    fn reset_map(&mut self) {
        self.last_ct_spawn = None;
        self.last_t_spawn = None;
        self.num_terrorist_wins = 0;
        self.num_ct_wins = 0;
        self.rules = rules::Rules::default();
    }
}

/// One step of the game's generator (xorshift32) over `state`, a number in `lo..=hi`.
pub(crate) fn random_in(state: &mut u32, lo: i32, hi: i32) -> i32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    let span = (hi - lo + 1).max(1) as u32;
    lo + (*state % span) as i32
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
    ///
    /// Also `PM_Init`: builds the statics of `pm_shared` from `sound/materials.txt` (the footstep
    /// materials of the textures). A missing file gives an empty table, every texture concrete.
    pub fn load_motd(&mut self, vfs: &cs_vfs::Vfs) {
        self.state.pm_shared = pm_init(vfs.read_bytes("sound/materials.txt").ok().as_deref());
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
    /// default), as `jointeam`/`joinclass` do (see [`teams`]), without the menus. For tests only (cargo feature
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

    /// Sets the hit group the player of `slot` was last hit in (`m_LastHitGroup`; 1 is the
    /// head). For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn set_last_hit_group(&mut self, slot: u8, group: i32) {
        if let Some(p) = self.state.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
            p.last_hit_group = group;
        }
    }

    /// Hurts the player of `slot` as `TakeDamage` with the given attacker and inflictor edicts
    /// (either may be `None`, the world). For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn damage_player_by(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        attacker: Option<EdictId>,
        inflictor: Option<EdictId>,
        damage: f32,
        bits: u32,
    ) -> bool {
        player::take_damage(&mut self.state, eng, slot, inflictor, attacker, damage, bits)
    }

    /// Gives the player of `slot` night vision (`m_bHasNightVision`, which the buy menu sets).
    /// `false` when nobody is in that slot. For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn grant_night_vision(&mut self, slot: u8) -> bool {
        let Some(p) = self.state.players.get_mut(usize::from(slot)).and_then(Option::as_mut)
        else {
            return false;
        };
        p.has_night_vision = true;
        true
    }

    /// Sets `m_flVelocityModifier` of the player of `slot`. `false` when nobody is in that slot.
    /// For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn set_velocity_modifier(&mut self, slot: u8, value: f32) -> bool {
        let Some(p) = self.state.players.get_mut(usize::from(slot)).and_then(Option::as_mut)
        else {
            return false;
        };
        p.velocity_modifier = value;
        true
    }

    /// `m_flVelocityModifier` of the player of `slot`. For tests only (cargo feature
    /// `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn velocity_modifier(&self, slot: u8) -> Option<f32> {
        self.state
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map(|p| p.velocity_modifier)
    }

    /// `(edict number, is a player, has a model)` of each physent the movement of the player of
    /// `slot` sees, as `EnginePmWorld` answers `PmWorld::physent_is_player` and
    /// `physent_has_model`. For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn pm_world_flags(&self, eng: &mut EngineApi<'_>, slot: u8) -> Vec<(i32, bool, bool)> {
        pmove::world_flags(eng, slot)
    }

    /// Gives the player of `slot` the weapon `behavior` with `clip` rounds loaded and `reserve`
    /// rounds of its ammo, without the pickup rules (the inventory operations are Task 12).
    /// `false` when nobody is in that slot or the weapon has no inventory slot. For tests only
    /// (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn give_weapon(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        behavior: Box<dyn weapons::WeaponBehavior>,
        clip: i32,
        reserve: i32,
    ) -> bool {
        weapons::core::give_weapon(&mut self.state, eng, slot, behavior, clip, reserve)
    }

    /// `SelectItem` for weapon `id` of the player of `slot`. For tests only (cargo feature
    /// `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn select_weapon(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        id: weapons::WeaponId,
    ) -> bool {
        weapons::core::select_item(&mut self.state, eng, slot, id)
    }

    /// The fields of weapon `id` the player of `slot` carries, to read or change. For tests only
    /// (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn weapon_mut(&mut self, slot: u8, id: weapons::WeaponId) -> Option<&mut weapons::Weapon> {
        let p = self.state.players.get_mut(usize::from(slot))?.as_mut()?;
        let (s, i) = p.find_item(id)?;
        Some(&mut p.items[s][i].weapon)
    }

    /// The weapons the player of `slot` carries, by inventory slot (primary first), the newest
    /// of a slot first. For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn carried_weapons(&self, slot: u8) -> Vec<weapons::WeaponId> {
        self.state
            .players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map(|p| p.items.iter().flatten().map(|i| i.weapon.id).collect())
            .unwrap_or_default()
    }

    /// Sets `m_pLastItem` of the player of `slot`. For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn set_last_item(&mut self, slot: u8, id: Option<weapons::WeaponId>) {
        if let Some(p) = self.state.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
            p.last_item = id;
        }
    }

    /// Drops weapon `id` of the player of `slot` in a box that lives `life` seconds (`DropPlayerItem`
    /// makes it `ITEM_KILL_DELAY`). The box's edict. For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn drop_weapon_with_life(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        id: weapons::WeaponId,
        life: f32,
    ) -> Option<EdictId> {
        let model = inventory::cs_model_name(id)?;
        let ent = eng.client_edict(slot);
        let v = eng.ent(ent)?;
        let (origin, angles) = (v.origin, v.angles);
        inventory::create_weapon_box(
            &mut self.state, eng, Some(slot), id, model, origin, angles, Vec3::ZERO, life, false,
        )
    }

    /// `pev->weapons` and `m_bHasPrimary` of the player of `slot`. For tests only (cargo feature
    /// `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn weapons_mask(&self, slot: u8) -> Option<(u32, bool)> {
        let p = self.state.players.get(usize::from(slot))?.as_ref()?;
        Some((p.weapons, p.has_primary))
    }

    /// `(m_flNextAttack, m_bCanShoot, the active weapon, m_iFOV, m_iClientFOV)` of the player of
    /// `slot`. For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn player_weapon_state(
        &self,
        slot: u8,
    ) -> Option<(f32, bool, Option<weapons::WeaponId>, u8, u8)> {
        let p = self.state.players.get(usize::from(slot))?.as_ref()?;
        Some((p.next_attack, p.can_shoot, p.active_item, p.fov, p.client_fov))
    }

    /// Sets `m_flNextAttack` of the player of `slot`. For tests only (cargo feature
    /// `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn set_next_attack(&mut self, slot: u8, value: f32) {
        if let Some(p) = self.state.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
            p.next_attack = value;
        }
    }

    /// Sets `m_iFOV` of the player of `slot` (a zoom, which a weapon would set). For tests only
    /// (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn set_fov(&mut self, slot: u8, fov: u8) {
        if let Some(p) = self.state.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
            p.fov = fov;
        }
    }

    /// `m_rgAmmo[index]` of the player of `slot`. For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn player_ammo(&self, slot: u8, index: usize) -> Option<i32> {
        self.state.players.get(usize::from(slot))?.as_ref()?.ammo.get(index).copied()
    }

    /// `m_rgAmmo[index] = value` of the player of `slot`. For tests only (cargo feature
    /// `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn set_player_ammo(&mut self, slot: u8, index: usize, value: i32) {
        if let Some(a) = self
            .state
            .players
            .get_mut(usize::from(slot))
            .and_then(Option::as_mut)
            .and_then(|p| p.ammo.get_mut(index))
        {
            *a = value;
        }
    }

    /// The legacy `ammo_*` counters of `TabulateAmmo` of the player of `slot`. For tests only
    /// (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn tabulated_ammo(&self, slot: u8) -> Option<[i32; 10]> {
        Some(self.state.players.get(usize::from(slot))?.as_ref()?.tabulated_ammo)
    }

    /// Ends the freeze period now (`OnRoundFreezeEnd`). For tests only (cargo feature
    /// `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn end_freeze_period(&mut self, eng: &mut EngineApi<'_>) {
        rules::on_round_freeze_end(&mut self.state, eng);
    }

    /// Sets `m_bGameOver` (no intermission). For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn force_game_over(&mut self, over: bool) {
        self.state.rules.force_game_over(over);
    }

    /// Closes the scope of the player of `slot` as a shot does (`m_bResumeZoom`, with
    /// `m_iLastZoom`). For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn set_resume_zoom(&mut self, slot: u8, last_zoom: i32) {
        if let Some(p) = self.state.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
            p.resume_zoom = true;
            p.last_zoom = last_zoom;
        }
    }

    /// Sets `m_bIsDefusing` of the player of `slot`. For tests only (cargo feature
    /// `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn set_defusing(&mut self, slot: u8, defusing: bool) {
        if let Some(p) = self.state.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
            p.is_defusing = defusing;
        }
    }

    /// Runs `ResetMaxSpeed` of the player of `slot`. For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn reset_max_speed(&self, eng: &mut EngineApi<'_>, slot: u8) {
        player::reset_max_speed(&self.state, eng, slot);
    }

    /// Runs `RestartRound` now (not the delayed `sv_restart`). For tests only (cargo feature
    /// `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn restart_round_now(&mut self, eng: &mut EngineApi<'_>) {
        rules::restart_round(&mut self.state, eng);
    }

    /// Puts the player of `slot` at `origin`, stopped, to place a player
    /// mid-air or elsewhere in tests. `false` when nobody is in that slot. For tests only
    /// (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn place_player(&mut self, eng: &mut EngineApi<'_>, slot: u8, origin: Vec3) -> bool {
        if self.state.players.get(usize::from(slot)).is_none_or(Option::is_none) {
            return false;
        }
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

    /// Every entity of the map has spawned: starts the rules of the map
    /// (the freeze period before the first round).
    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        self.state.rules = rules::Rules::new(eng);
        // `WriteSigonMessages`, after `LinkUserMessages`: the HUD's weapon table.
        inventory::write_sigon_messages(eng);
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
        chat::name_changed(&mut self.state, eng, slot);
        player::userinfo_changed(&mut self.state, eng, slot);
    }

    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        random_seed: u32,
    ) {
        player::run_usercmd(&mut self.state, eng, slot, cmd, random_seed);
    }

    /// `PlayerPostThink` and `CmdEnd`, after the engine's `SV_ProcessImpacts`.
    fn post_usercmd(&mut self, eng: &mut EngineApi<'_>, slot: u8, _cmd: &GoldSrcUserCmd) {
        player::post_usercmd(&mut self.state, eng, slot);
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

    /// `SetupVisibility`: an observer sees what the player it follows sees.
    fn setup_visibility(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Option<Vec3> {
        observer::setup_visibility(&self.state, eng, slot)
    }
}
