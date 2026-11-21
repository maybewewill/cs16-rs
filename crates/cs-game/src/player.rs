//! The player: its lifecycle (`ClientConnect`, `ClientPutInServer`, `CBasePlayer::Spawn`,
//! respawn, disconnect), damage and death (`TakeDamage`, `Killed`, `PlayerDeathThink`,
//! `ClientKill`), the per-command movement with `PlayerPreThink`/`PlayerPostThink` (fall damage)
//! and the client data. Ported from ReGameDLL_CS (`player.cpp`, `client.cpp`,
//! `multiplay_gamerules.cpp`, `player.h`, read 2026-11-22), the vanilla `mp.dll` behaviour (not
//! the `REGAMEDLL_FIXES` build).
//!
//! A player lives in [`GameState::players`], by slot, from `client_put_in_server` to
//! `client_disconnect`. A fresh player is [`Team::Unassigned`]: invisible, not solid, taking no
//! damage, until the team task spawns it with [`spawn`]. The movement is `cs_sim::pmove` over
//! the engine's traces (`EngineWorld`); the rest of `pm_shared` is not ported (a dying player
//! moves like a walking one with no input, where the original runs `PM_Physics_Toss`).
//!
//! A player that joins stands at the counter-terrorists' spawn point as in `ClientPutInServer`
//! (without the telefrag the vanilla join path would do); the `trigger_camera` intro path (the
//! player looks through the map's camera until it picks a team) is not ported.
//!
//! The dead leave a corpse: `PlayerDeathThink` sends `ClCorpse` once the player has been dead
//! for `DEATH_ANIMATION_TIME`, then calls `StartDeathCam`, which belongs to the observer task
//! (see [`start_death_cam`], its call point; until then it only records that the corpse was
//! sent, as `PFLAG_OBSERVER` does in the original).
//!
//! ## Known gaps
//!
//! Not ported yet, each a later task: weapons and the active item, the economy rewards
//! (`AddAccount`), team selection and `m_iJoiningState`, rounds (`CheckWinConditions`,
//! `FPlayerCanRespawn`, the freeze period), the observer camera of the dead (`StartDeathCam`),
//! `Pain` sounds, flinch, the momentum block of `TakeDamage` (see [`take_damage`]), the blast
//! branch of `TakeDamage`, the HUD damage message, armor (`Battery`, `ArmorType`) messages,
//! animations and the VIP.
//!
//! Fields of the client data ([`client_data`]) that stay zero for now:
//! - `bInDuck`, `flDuckTime`, `flTimeStepSound`, `waterjumptime`, `flSwimTime`, `pushmsec`: the
//!   state of `pm_shared`'s duck, step-sound and water-jump timers; `cs_sim::pmove` has no such
//!   timers, they come with the faithful `pm_shared` port.
//! - `weaponanim`, `viewmodel`, `m_flNextAttack`, the ammo fields and the `iuser3` flags
//!   (`PLAYER_CAN_SHOOT`, `PLAYER_FREEZE_TIME_OVER`, `PLAYER_IN_BOMB_ZONE`, ...): they belong to
//!   the weapons, the freeze period and the bomb zones.
//! - `fuser1`-`fuser3`: set by the spectator and HLTV code.
//! - `weapons` carries only the suit bit (`1 << WEAPON_SUIT`) that `PlayerSpawn` sets; the
//!   weapon bits come with the weapons.

use cs_assets::bsp::SpawnTeam;
use cs_engine::consts::{
    FL_CLIENT, FL_DORMANT, FL_DUCKING, FL_ONGROUND, FL_PROXY, FL_SPECTATOR, MOVETYPE_NONE,
    MOVETYPE_TOSS, MOVETYPE_WALK, SOLID_NOT, SOLID_SLIDEBOX,
};
use cs_engine::{EdictId, EngineApi};
use cs_proto48::delta::{GoldSrcClientData, GoldSrcUserCmd};
use cs_proto48::usermsg::CsUserMsg;
use cs_sim::pmove::{MoveInputCmd, PlayerMoveState, VEC_VIEW, step_player_move};
use cs_sim::walk_game::{EngineWorld, usercmd_to_move_input};
use glam::Vec3;

use crate::GameState;
use crate::messages;
use crate::world::{select_join_spawn_point, select_spawn_point};

/// Most players a server holds (the slots of [`GameState::players`]).
pub const MAX_PLAYERS: usize = 32;

/// `entvars_t::deadflag` values.
pub const DEAD_NO: i32 = 0;
pub const DEAD_DYING: i32 = 1;
pub const DEAD_DEAD: i32 = 2;
pub const DEAD_RESPAWNABLE: i32 = 3;

/// `DMG_*` bits (`common/const.h`) the player code uses.
pub mod dmg {
    pub const GENERIC: u32 = 0;
    pub const CRUSH: u32 = 1 << 0;
    pub const BULLET: u32 = 1 << 1;
    pub const FALL: u32 = 1 << 5;
    pub const BLAST: u32 = 1 << 6;
    pub const DROWN: u32 = 1 << 14;
    /// Counter-Strike's `DMG_EXPLOSION` (`dlls/util.h`).
    pub const EXPLOSION: u32 = 1 << 24;
}

/// Hit groups (`common/const.h`).
const HITGROUP_GENERIC: i32 = 0;
const HITGROUP_HEAD: i32 = 1;
const HITGROUP_CHEST: i32 = 2;
const HITGROUP_STOMACH: i32 = 3;
const HITGROUP_LEFTARM: i32 = 4;
const HITGROUP_RIGHTARM: i32 = 5;

/// `MAX_PLAYER_FATAL_FALL_SPEED`, `MAX_PLAYER_SAFE_FALL_SPEED`, `DAMAGE_FOR_FALL_SPEED` and
/// `PLAYER_FALL_PUNCH_THRESHHOLD` (ReGameDLL `player.h`, read 2026-11-22).
pub const MAX_PLAYER_FATAL_FALL_SPEED: f32 = 1100.0;
pub const MAX_PLAYER_SAFE_FALL_SPEED: f32 = 500.0;
pub const DAMAGE_FOR_FALL_SPEED: f32 =
    100.0 / (MAX_PLAYER_FATAL_FALL_SPEED - MAX_PLAYER_SAFE_FALL_SPEED);
pub const PLAYER_FALL_PUNCH_THRESHHOLD: f32 = 250.0;

/// `ARMOR_RATIO` and `ARMOR_BONUS` (`player.h`).
const ARMOR_RATIO: f32 = 0.5;
const ARMOR_BONUS: f32 = 0.5;
/// `AIRTIME`: seconds a player can stay under water.
const AIRTIME: f64 = 12.0;
/// `CONTENTS_EMPTY` and `CONTENTS_WATER`.
const CONTENT_EMPTY: i32 = -1;
const CONTENT_WATER: i32 = -3;
/// `EF_NODRAW` and `EF_NOINTERP`.
const EF_NODRAW: u32 = 128;
const EF_NOINTERP: u32 = 32;
/// `CHAN_VOICE` and `CHAN_ITEM`, `ATTN_NORM`, `PITCH_NORM`.
const CHAN_VOICE: u8 = 2;
const CHAN_ITEM: u8 = 3;
const ATTN_NORM: f32 = 0.8;
const PITCH_NORM: u8 = 100;
/// `VEC_HULL_MIN`/`MAX` and `VEC_DUCK_HULL_MIN`/`MAX`.
const HULL_MIN: Vec3 = Vec3::new(-16.0, -16.0, -36.0);
const HULL_MAX: Vec3 = Vec3::new(16.0, 16.0, 36.0);
const DUCK_HULL_MIN: Vec3 = Vec3::new(-16.0, -16.0, -18.0);
const DUCK_HULL_MAX: Vec3 = Vec3::new(16.0, 16.0, 18.0);
/// `WEAPON_SUIT`: the bit of `pev->weapons` that stands for the HEV suit.
const WEAPON_SUIT: u32 = 31;
/// `DEATH_ANIMATION_TIME` (`gamerules.h`): how long a death lasts before the corpse is sent.
const DEATH_ANIMATION_TIME: f64 = 3.0;
/// `DEFAULT_FOV`.
const DEFAULT_FOV: f32 = 90.0;
/// The speed of a player with no active item (`CBasePlayer::ResetMaxSpeed`, ReGameDLL
/// `player.cpp`); weapons set it themselves once they exist.
const NO_ITEM_MAX_SPEED: f32 = 240.0;
/// `MAX_MONEY_THRESHOLD`: the largest account (`CheckStartMoney` clamps `mp_startmoney`).
const MAX_MONEY: i32 = 16000;
/// Seconds between two suicides (`ClientKill`).
const SUICIDE_DELAY: f64 = 1.0;
/// Seconds between two runs of `PlayerDeathThink`.
const DEATH_THINK_INTERVAL: f64 = 0.1;
/// Longest a player stays `DEAD_DYING` in the air. Assumption: the original waits for the end
/// of the death animation (a few seconds), which does not exist here.
const DYING_MAX_SECONDS: f64 = 3.0;

/// The team of a player; [`id`](Team::id) is the number the scoreboard messages carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Team {
    #[default]
    Unassigned,
    Terrorist,
    Ct,
    Spectator,
}

impl Team {
    /// The team number the clients know (`UNASSIGNED` 0, `TERRORIST` 1, `CT` 2, `SPECTATOR` 3).
    pub fn id(self) -> u8 {
        match self {
            Team::Unassigned => 0,
            Team::Terrorist => 1,
            Team::Ct => 2,
            Team::Spectator => 3,
        }
    }

    /// The team of the spawn points a player of this team uses; `None` for a team that does not
    /// spawn.
    fn spawn_team(self) -> Option<SpawnTeam> {
        match self {
            Team::Terrorist => Some(SpawnTeam::Terrorist),
            Team::Ct => Some(SpawnTeam::CounterTerrorist),
            Team::Unassigned | Team::Spectator => None,
        }
    }

    /// Whether this is a team that plays (the stand-in for `m_iJoiningState == JOINED`).
    pub fn plays(self) -> bool {
        self.spawn_team().is_some()
    }
}

/// `m_iKevlar`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArmorType {
    #[default]
    None,
    Vest,
    VestHelm,
}

/// What the game knows of one player: the fields of `CBasePlayer` this task needs.
///
/// `health` mirrors `pev->health` and is written to the edict whenever it changes; the edict's
/// variables carry the rest of the entity state (origin, flags, movetype, ...). `armor` and
/// `weapons` live only here.
#[derive(Debug, Clone)]
pub struct Player {
    pub team: Team,
    /// The model the player chose in the appearance menu, 1 to 4 (`0` the team's default): for
    /// the counter-terrorists urban, gsg9, sas, gign; for the terrorists terror, leet, arctic,
    /// guerilla (`client.cpp`, `HandleMenu_ChooseAppearance`).
    pub class: u8,
    pub health: f32,
    /// `pev->armorvalue`. Kept here only: the edict has no such field and the client is not
    /// told yet (the `Battery` message belongs to the HUD and economy tasks).
    pub armor: f32,
    /// `m_iKevlar`.
    pub kevlar: ArmorType,
    /// `pev->frags`.
    pub frags: i32,
    /// `m_iDeaths`.
    pub deaths: i32,
    /// `m_iAccount`.
    pub money: i32,
    /// `deadflag == DEAD_NO` of a spawned player.
    pub alive: bool,
    /// `pev->takedamage != DAMAGE_NO`.
    pub takes_damage: bool,
    /// `m_bNotKilled`: the player was alive when it last left the game, so it keeps its armor.
    pub(crate) not_killed: bool,
    /// The movement state `cs_sim::pmove` carries between commands.
    pub(crate) mv: PlayerMoveState,
    /// `m_flFallVelocity`.
    pub(crate) fall_velocity: f32,
    /// `m_fNextSuicideTime`.
    pub(crate) next_suicide_time: f64,
    /// `m_iClientHealth`: the health the client was last told.
    pub(crate) client_health: i32,
    /// `m_LastHitGroup`.
    pub(crate) last_hit_group: i32,
    /// `pev->weapons`: only the suit bit for now.
    pub(crate) weapons: u32,
    /// `PFLAG_OBSERVER` as far as `PlayerDeathThink` is concerned: the corpse was sent and
    /// [`start_death_cam`] ran.
    pub(crate) death_cam: bool,
    /// Server time of the death, `m_fDeadTime` once the player is `DEAD_DEAD`.
    pub(crate) dead_time: f64,
    /// Server time of the next `PlayerDeathThink`.
    next_death_think: f64,
}

impl Player {
    /// A player as `ClientPutInServer` leaves it: no team, nothing earned, `money` in the
    /// account.
    fn new(money: i32) -> Self {
        Self {
            team: Team::Unassigned,
            class: 0,
            health: 100.0,
            armor: 0.0,
            kevlar: ArmorType::None,
            frags: 0,
            deaths: 0,
            money,
            alive: false,
            takes_damage: false,
            not_killed: true,
            mv: PlayerMoveState::default(),
            fall_velocity: 0.0,
            next_suicide_time: 0.0,
            client_health: -1,
            last_hit_group: HITGROUP_GENERIC,
            weapons: 0,
            death_cam: false,
            dead_time: 0.0,
            next_death_think: 0.0,
        }
    }

    /// `CBasePlayer::IsArmored`.
    fn is_armored(&self, hit_group: i32) -> bool {
        match self.kevlar {
            ArmorType::None => false,
            ArmorType::VestHelm if hit_group == HITGROUP_HEAD => true,
            _ => matches!(
                hit_group,
                HITGROUP_GENERIC
                    | HITGROUP_CHEST
                    | HITGROUP_STOMACH
                    | HITGROUP_LEFTARM
                    | HITGROUP_RIGHTARM
            ),
        }
    }
}

/// The slot of the player edict `id`, if it is one of the server's.
fn slot_of(eng: &EngineApi<'_>, id: EdictId) -> Option<usize> {
    let slot = usize::from(id.index.checked_sub(1)?);
    let in_range = slot < usize::from(eng.max_clients()).min(MAX_PLAYERS);
    (in_range && eng.client_edict(slot as u8) == id).then_some(slot)
}

/// The slot of the player `id` names, if it is one of the game's players.
pub(crate) fn player_slot(g: &GameState, eng: &EngineApi<'_>, id: EdictId) -> Option<usize> {
    slot_of(eng, id).filter(|&s| g.players[s].is_some())
}

/// `ClientPutInServer` of a multiplayer game (ReGameDLL `client.cpp`, read 2026-11-22): the
/// player enters with no team, invisible, not solid and taking no damage, until it is spawned.
/// It stands at the counter-terrorists' spawn point (the `GetPlayerSpawnSpot` of the original's
/// no-camera path; the `trigger_camera` intro path is not ported) with `v_angle` zero and the
/// spot's angles, and keeps the view offset `VEC_VIEW`. The original keeps the player as a
/// `MOVETYPE_NOCLIP` spectator with `models/player.mdl` while it looks at the menus; with no
/// menus yet, this is `MOVETYPE_NONE` and no model.
pub(crate) fn put_in_server(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let index = usize::from(slot);
    if index >= MAX_PLAYERS {
        return;
    }
    // `CheckStartMoney`: `mp_startmoney` clamped to the account's limits.
    let money = (eng.cvar_f32("mp_startmoney") as i32).clamp(0, MAX_MONEY);
    g.players[index] = Some(Player::new(money));
    let id = eng.client_edict(slot);
    let (origin, angles) = match select_join_spawn_point(g, eng, id, SpawnTeam::CounterTerrorist) {
        Some(spot) => {
            let v = eng.ent(spot);
            (
                v.map_or(Vec3::ZERO, |v| v.origin) + Vec3::new(0.0, 0.0, 1.0),
                v.map_or(Vec3::ZERO, |v| v.angles),
            )
        }
        None => (Vec3::new(0.0, 0.0, 1.0), Vec3::ZERO),
    };
    let Some(v) = eng.ent_mut(id) else {
        return;
    };
    v.origin = origin;
    v.angles = angles;
    v.v_angle = Vec3::ZERO;
    v.flags = FL_CLIENT | FL_SPECTATOR;
    v.solid = SOLID_NOT;
    v.movetype = MOVETYPE_NONE;
    v.effects = EF_NODRAW | EF_NOINTERP;
    v.modelindex = 0;
    v.deadflag = DEAD_DEAD;
    v.health = 100.0;
    v.velocity = Vec3::ZERO;
    v.punchangle = Vec3::ZERO;
    v.fixangle = 1;
    v.max_speed = 1.0;
    v.fov = DEFAULT_FOV;
    v.view_ofs = Vec3::new(0.0, 0.0, VEC_VIEW);
    eng.set_origin(id, origin);
}

/// `ClientDisconnect`: the slot's player is gone and its edict inert.
pub(crate) fn disconnect(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(entry) = g.players.get_mut(usize::from(slot)) else {
        return;
    };
    *entry = None;
    let id = eng.client_edict(slot);
    let Some(v) = eng.ent_mut(id) else {
        return;
    };
    v.solid = SOLID_NOT;
    v.flags = FL_DORMANT;
    let origin = v.origin;
    eng.set_origin(id, origin);
}

/// The model name of a team and appearance choice (`CBasePlayer::SetPlayerModel`).
fn model_name(team: Team, class: u8) -> &'static str {
    match (team, class) {
        (Team::Ct, 2) => "gsg9",
        (Team::Ct, 3) => "sas",
        (Team::Ct, 4) => "gign",
        (Team::Terrorist, 2) => "leet",
        (Team::Terrorist, 3) => "arctic",
        (Team::Terrorist, 4) => "guerilla",
        (Team::Terrorist, _) => "terror",
        _ => "urban",
    }
}

/// Spawns the player of `slot` on `team` with appearance `class`: `CBasePlayer::Spawn` of a
/// joined player, including `GetPlayerSpawnSpot` (the spawn point may kill the players standing
/// at it) and the team's model. `false` for a team that does not spawn or a slot with no
/// player.
#[allow(dead_code)] // Called by the team selection, which comes after this task (tests use it).
pub(crate) fn spawn(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    team: Team,
    class: u8,
) -> bool {
    let index = usize::from(slot);
    let Some(spawn_team) = team.spawn_team() else {
        return false;
    };
    if g.players.get(index).is_none_or(Option::is_none) {
        return false;
    }
    let id = eng.client_edict(slot);
    // `GetPlayerSpawnSpot`.
    let (origin, angles) = match select_spawn_point(g, eng, id, spawn_team) {
        Some(spot) => {
            let v = eng.ent(spot);
            (
                v.map_or(Vec3::ZERO, |v| v.origin) + Vec3::new(0.0, 0.0, 1.0),
                v.map_or(Vec3::ZERO, |v| v.angles),
            )
        }
        None => (Vec3::new(0.0, 0.0, 1.0), Vec3::ZERO),
    };
    let time = eng.time();
    let Some(p) = g.players[index].as_mut() else {
        return false;
    };
    p.team = team;
    p.class = class;
    p.health = 100.0;
    if !p.not_killed {
        p.armor = 0.0;
        p.kevlar = ArmorType::None;
    }
    p.alive = true;
    p.takes_damage = true;
    p.not_killed = true;
    p.fall_velocity = 0.0;
    p.client_health = -1;
    p.last_hit_group = HITGROUP_GENERIC;
    p.death_cam = false;
    // `PlayerSpawn`: `pev->weapons |= (1 << WEAPON_SUIT)`.
    p.weapons |= 1 << WEAPON_SUIT;
    p.mv = PlayerMoveState::at_spawn(origin);

    if eng.ent(id).is_some_and(|v| v.modelindex == 0) {
        eng.set_model(id, "models/player.mdl");
    }
    eng.set_client_key(slot, "model", model_name(team, class));
    if let Some(v) = eng.ent_mut(id) {
        v.max_speed = NO_ITEM_MAX_SPEED;
        v.solid = SOLID_SLIDEBOX;
        v.movetype = MOVETYPE_WALK;
        v.health = 100.0;
        v.flags &= FL_PROXY;
        v.flags |= FL_CLIENT;
        v.air_finished = (time + AIRTIME) as f32;
        v.dmg = 2.0;
        v.effects = 0;
        v.deadflag = DEAD_NO;
        v.watertype = CONTENT_EMPTY;
        v.waterlevel = 0;
        v.basevelocity = Vec3::ZERO;
        v.friction = 1.0;
        v.gravity = 1.0;
        v.fov = DEFAULT_FOV;
        v.origin = origin;
        v.v_angle = Vec3::ZERO;
        v.velocity = Vec3::ZERO;
        v.angles = angles;
        v.punchangle = Vec3::ZERO;
        v.fixangle = 1;
        v.iuser1 = 0;
        v.iuser2 = 0;
        v.iuser3 = 0;
        v.nextthink = 0.0;
        // Overrides what the model set up: `pev->view_ofs = VEC_VIEW`.
        v.view_ofs = Vec3::new(0.0, 0.0, VEC_VIEW);
    }
    eng.set_size(id, HULL_MIN, HULL_MAX);
    eng.set_origin(id, origin);
    if let Some(p) = g.players[index].as_ref() {
        messages::score_info(eng, id.index, p);
    }
    true
}

/// `respawn()` (ReGameDLL `client.cpp`): spawns the player again on its team. The round rules
/// call this when a round starts and, with `forcerespawn`, after a death; nothing respawns a
/// dead player by itself. `false` when the player has no team that spawns.
#[allow(dead_code)] // Called by the round rules, which come after this task.
pub(crate) fn respawn_player(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) -> bool {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return false;
    };
    let (team, class) = (p.team, p.class);
    spawn(g, eng, slot, team, class)
}

/// `CHalfLifeMultiplay::FlPlayerFallDamage`: the damage of landing at `fall_velocity` (above
/// `MAX_PLAYER_SAFE_FALL_SPEED`).
pub fn fall_damage(fall_velocity: f32) -> f32 {
    (fall_velocity - MAX_PLAYER_SAFE_FALL_SPEED) * DAMAGE_FOR_FALL_SPEED * 1.25
}

/// Moves the player of `slot` by one command (`SV_RunCmd` around `PM_Move`): `PlayerPreThink`,
/// the movement, `PlayerPostThink`; a dead player moves with no input and runs
/// `PlayerDeathThink`. Players that have not been spawned do not move.
pub(crate) fn run_usercmd(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    cmd: &GoldSrcUserCmd,
) {
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if cmd.msec == 0 || !p.team.plays() {
        return;
    }
    let alive = p.alive;
    let id = eng.client_edict(slot);
    let Some(v) = eng.ent(id) else {
        return;
    };
    if v.movetype == MOVETYPE_NONE {
        // A body at rest does not move.
        if !alive {
            death_think(g, eng, slot);
        }
        return;
    }
    if alive && v.flags & FL_ONGROUND == 0 {
        // `CBasePlayer::PreThink`: the fall speed of the last command.
        p.fall_velocity = -v.velocity.z;
    }
    move_player(g, eng, slot, cmd, alive);
    if alive {
        post_think(g, eng, slot);
    } else {
        death_think(g, eng, slot);
    }
}

/// Runs the movement of one command and writes the result to the edict.
fn move_player(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    cmd: &GoldSrcUserCmd,
    alive: bool,
) {
    let id = eng.client_edict(slot);
    let max_speed = match eng.ent(id).map(|v| v.max_speed) {
        Some(s) if s.is_finite() && s > 0.0 => s,
        _ => NO_ITEM_MAX_SPEED,
    };
    let input = if alive {
        usercmd_to_move_input(cmd, max_speed)
    } else {
        MoveInputCmd::default()
    };
    let dt = f32::from(cmd.msec) / 1000.0;
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    // Start from the edict, so a move the engine (or the game) made since the last command is
    // not overwritten.
    if let Some(v) = eng.ent(id) {
        p.mv.origin = v.origin;
        p.mv.velocity = v.velocity;
        p.mv.on_ground = v.flags & FL_ONGROUND != 0;
    }
    let world = EngineWorld::new(eng, id, &g.ladders);
    step_player_move(&mut p.mv, &input, max_speed, &g.move_params, &world, dt);
    // What the movement ran into reaches the engine as impacts, delivered after the command.
    let touched = world.into_touched();
    for other in touched {
        eng.player_touched(id, other);
    }
    let Some(v) = eng.ent_mut(id) else {
        return;
    };
    let state = &p.mv;
    v.v_angle = cmd.viewangles;
    if alive {
        v.angles.y = v.v_angle.y;
    }
    v.origin = state.origin;
    v.velocity = state.velocity;
    v.view_ofs.z = state.view_ofs_z;
    v.flags &= !(FL_ONGROUND | FL_DUCKING);
    if state.on_ground {
        v.flags |= FL_ONGROUND;
    }
    if state.ducking {
        v.flags |= FL_DUCKING;
        v.mins = DUCK_HULL_MIN;
        v.maxs = DUCK_HULL_MAX;
    } else {
        v.mins = HULL_MIN;
        v.maxs = HULL_MAX;
    }
    v.size = v.maxs - v.mins;
    // The engine links the player after the command.
}

/// The fall part of `CBasePlayer::PostThink`: a player that lands faster than
/// `MAX_PLAYER_SAFE_FALL_SPEED` out of water takes [`fall_damage`] from the world.
fn post_think(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    let Some(v) = eng.ent(id) else {
        return;
    };
    let (flags, watertype) = (v.flags, v.watertype);
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    let (health, fall_velocity) = (p.health, p.fall_velocity);
    if flags & FL_ONGROUND == 0 {
        return;
    }
    p.fall_velocity = 0.0;
    if health > 0.0
        && fall_velocity >= PLAYER_FALL_PUNCH_THRESHHOLD
        && watertype != CONTENT_WATER
        && fall_velocity > MAX_PLAYER_SAFE_FALL_SPEED
    {
        let damage = fall_damage(fall_velocity);
        if damage > health {
            // Played on the item channel: the footstep landing uses the body channel.
            eng.emit_sound(
                id,
                CHAN_ITEM,
                "common/bodysplat.wav",
                1.0,
                ATTN_NORM,
                PITCH_NORM,
            );
        }
        if damage > 0.0 {
            // Vanilla: the damage comes from the world (no attacker, no inflictor).
            p.last_hit_group = HITGROUP_GENERIC;
            take_damage(g, eng, slot, None, None, damage, dmg::FALL);
            if let Some(v) = eng.ent_mut(id) {
                v.punchangle.x = 0.0;
            }
        }
    }
}

/// `CBasePlayer::TakeDamage` (ReGameDLL `player.cpp`, the branch for everything but explosions,
/// which come with the grenades), then `CBaseMonster::TakeDamage`, which for a player resolves to
/// `CBaseEntity::TakeDamage` (`cbase.cpp:660-712`, read 2026-11-22): refuses damage the rules
/// forbid
/// (`FPlayerCanTakeDamage`: a teammate's needs `mp_friendlyfire`), scales a teammate's (and
/// one's own) by 0.35, lets armor absorb its share, takes the health off and kills the player at
/// zero. `attacker` and `inflictor` are `None` for the world. Returns whether the player took
/// damage and survived (the original's result).
///
/// Left out for the tasks that need them: the active-item multipliers of the armor ratio, the
/// VIP's, flinch and `TakeDamageImpulse` knockback, the `Pain` sound, the teammate-attack hints,
/// the HUD damage message, and the momentum block of `CBaseEntity::TakeDamage` (a `MOVETYPE_WALK`
/// or `MOVETYPE_STEP` victim whose inflictor is not the world gets `velocity += dir * min(1000,
/// damage * (32 * 32 * 72 / size volume) * 5)`, `dir` from the inflictor's centre to the
/// victim): it needs the inflictor entities, so it is ported with the weapons (2D).
pub(crate) fn take_damage(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    inflictor: Option<EdictId>,
    attacker: Option<EdictId>,
    damage: f32,
    bits: u32,
) -> bool {
    let index = usize::from(slot);
    if !damage.is_finite() {
        return false;
    }
    let attacker_slot = attacker.and_then(|a| player_slot(g, eng, a));
    let friendly_fire = eng.cvar_f32("mp_friendlyfire") != 0.0;
    let Some(victim_team) = g
        .players
        .get(index)
        .and_then(Option::as_ref)
        .map(|p| p.team)
    else {
        return false;
    };
    let attacker_team = attacker_slot
        .and_then(|a| g.players[a].as_ref())
        .map(|p| p.team);
    // `PlayerRelationship` is `GR_TEAMMATE` for oneself and for the same team.
    let teammate = attacker_slot == Some(index) || attacker_team == Some(victim_team);

    let Some(p) = g.players[index].as_mut() else {
        return false;
    };
    if bits & (dmg::EXPLOSION | dmg::BLAST | dmg::FALL) != 0 {
        p.last_hit_group = HITGROUP_GENERIC;
    }
    // `FPlayerCanTakeDamage`.
    if attacker_slot.is_some() && teammate && attacker_slot != Some(index) && !friendly_fire {
        return false;
    }
    // `pev->takedamage == DAMAGE_NO`, and already dead.
    if !p.takes_damage || !p.alive {
        return false;
    }
    let mut damage = damage;
    if attacker_slot.is_some()
        && !(attacker_slot == Some(index) && bits & dmg::FALL != 0)
        && teammate
    {
        damage *= 0.35;
    }
    // Armor does not protect against falls and drowning.
    if p.armor != 0.0 && bits & (dmg::DROWN | dmg::FALL) == 0 && p.is_armored(p.last_hit_group) {
        let mut new = ARMOR_RATIO * damage;
        let mut armor = (damage - new) * ARMOR_BONUS;
        if armor > p.armor {
            // Uses more armor than there is.
            armor = p.armor * (1.0 / ARMOR_BONUS);
            new = damage - armor;
            p.armor = 0.0;
        } else {
            if armor < 0.0 {
                armor = 1.0;
            }
            p.armor -= armor;
        }
        damage = new;
        if p.armor <= 0.0 {
            p.kevlar = ArmorType::None;
        }
    }
    // `CBaseMonster::TakeDamage` gets the damage as an integer: a player with half a point of
    // health would look dead to the client.
    p.health -= damage.trunc();
    let health = p.health;
    let id = eng.client_edict(slot);
    if let Some(v) = eng.ent_mut(id) {
        v.health = health;
    }
    if health <= 0.0 {
        killed(g, eng, slot, attacker, inflictor);
        return false;
    }
    true
}

/// `CBasePlayer::Kill`: the player kills itself. `false` when it cannot (it has not joined).
fn kill(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) -> bool {
    let id = eng.client_edict(slot);
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return false;
    };
    if !p.team.plays() {
        return false;
    }
    p.last_hit_group = HITGROUP_GENERIC;
    // Have the player kill himself.
    p.health = 0.0;
    if let Some(v) = eng.ent_mut(id) {
        v.health = 0.0;
    }
    killed(g, eng, slot, Some(id), Some(id));
    true
}

/// `ClientKill` (ReGameDLL `client.cpp`) behind the engine's `kill` command: refused while the
/// player is dead (the engine's `SV_Kill_f` says "Can't suicide -- allready dead!") and for a
/// second after the last suicide.
pub(crate) fn client_kill(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time();
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if p.health <= 0.0 {
        eng.client_print(slot, "Can't suicide -- allready dead!\n");
        return;
    }
    // Prevent suiciding too often.
    if p.next_suicide_time > now {
        return;
    }
    p.next_suicide_time = now + SUICIDE_DELAY;
    kill(g, eng, slot);
}

/// The weapon name of a death message (`CBasePlayer::GetKillerWeaponName`, `player.cpp:824-872`):
/// the inflictor's classname without its `weapon_`, `monster_` or `func_` prefix. A killer that is
/// no client with no inflictor is the world (`VARS(eoNullEntity)`): `worldspawn`. A client killer
/// that is its own inflictor names its active weapon, `world` while there are no weapons.
fn killer_weapon_name(
    eng: &EngineApi<'_>,
    inflictor: Option<EdictId>,
    attacker: Option<EdictId>,
) -> String {
    let killer_is_client = attacker
        .and_then(|a| eng.ent(a))
        .is_some_and(|v| v.flags & FL_CLIENT != 0);
    let world_class = eng
        .ent(eng.world())
        .map_or("worldspawn", |v| v.classname.as_str());
    let name = match inflictor {
        Some(i) if !(killer_is_client && Some(i) == attacker) => {
            eng.ent(i).map_or("world", |v| v.classname.as_str())
        }
        None if !killer_is_client => world_class,
        _ => "world",
    };
    ["weapon_", "monster_", "func_"]
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .unwrap_or(name)
        .to_string()
}

/// `CHalfLifeMultiplay::PlayerKilled` (the part this task has the data for) and
/// `CBasePlayer::Killed`: the death notice, the frag rules, the scoreboard, then the player
/// dies (`DEAD_DYING`, not solid, `MOVETYPE_TOSS`) and runs `PlayerDeathThink`.
///
/// Frags: a suicide costs one, killing a teammate costs one, killing an enemy gives one, the
/// world (an attacker that is no player) changes none. The money rewards come with the
/// economy, `CheckWinConditions` with the rounds.
fn killed(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    attacker: Option<EdictId>,
    inflictor: Option<EdictId>,
) {
    let index = usize::from(slot);
    let id = eng.client_edict(slot);
    let now = eng.time();
    let killer = attacker.and_then(|a| player_slot(g, eng, a));
    let weapon = killer_weapon_name(eng, inflictor, attacker);
    let Some(victim) = g.players[index].as_mut() else {
        return;
    };
    let headshot = victim.last_hit_group == HITGROUP_HEAD;
    let victim_team = victim.team;

    // DeathNotice.
    let killer_index = killer.map_or(0, |k| k as u16 + 1);
    messages::death_msg(eng, killer_index, id.index, headshot, &weapon);

    // PlayerKilled.
    if let Some(victim) = g.players[index].as_mut() {
        victim.deaths += 1;
        victim.not_killed = false;
    }
    if let Some(k) = killer {
        let teammate = g.players[k].as_ref().map(|p| p.team) == Some(victim_team);
        if let Some(kp) = g.players[k].as_mut() {
            kp.frags += if k == index || teammate { -1 } else { 1 };
        }
    }
    if let Some(p) = g.players[index].as_ref() {
        messages::score_info(eng, id.index, p);
    }
    if let Some(k) = killer
        && let Some(kp) = g.players[k].as_ref()
    {
        messages::score_info(eng, k as u16 + 1, kp);
    }

    // CBasePlayer::Killed.
    if let Some(p) = g.players[index].as_mut() {
        p.alive = false;
        p.takes_damage = false;
        p.dead_time = now;
        p.next_death_think = now + DEATH_THINK_INTERVAL;
        p.client_health = 0;
    }
    messages::health(eng, slot, 0);
    // `SendFOV(0)`.
    messages::set_fov(eng, slot, 0);
    if let Some(v) = eng.ent_mut(id) {
        v.fov = 0.0;
        v.deadflag = DEAD_DYING;
        v.movetype = MOVETYPE_TOSS;
        v.flags &= !FL_ONGROUND;
        v.solid = SOLID_NOT;
        v.angles.x = 0.0;
        v.angles.z = 0.0;
    }
    // `DeathSound`: one of four, `RANDOM_LONG(1, 4)`.
    let sample = match g.random_range(1, 4) {
        1 => "player/die1.wav",
        2 => "player/die2.wav",
        3 => "player/die3.wav",
        _ => "player/death6.wav",
    };
    eng.emit_sound(id, CHAN_VOICE, sample, 1.0, ATTN_NORM, PITCH_NORM);
}

/// `CBasePlayer::PlayerDeathThink` for a dead player, every `DEATH_THINK_INTERVAL`: a body on
/// the ground slows down; once it is on the ground (or after `DYING_MAX_SECONDS` in the air)
/// it stops moving and is `DEAD_DEAD`. Becoming respawnable is the round rules' business.
fn death_think(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time();
    let id = eng.client_edict(slot);
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if now < p.next_death_think {
        return;
    }
    p.next_death_think = now + DEATH_THINK_INTERVAL;
    let died = p.dead_time;
    let Some(v) = eng.ent_mut(id) else {
        return;
    };
    let on_ground = v.flags & FL_ONGROUND != 0;
    if on_ground {
        let forward = v.velocity.length() - 20.0;
        v.velocity = if forward <= 0.0 {
            Vec3::ZERO
        } else {
            v.velocity.normalize() * forward
        };
    }
    let dying = v.deadflag == DEAD_DYING;
    if dying && !on_ground && now - died < DYING_MAX_SECONDS {
        return;
    }
    // Done "animating": a body on the ground stops colliding and moving.
    if v.movetype != MOVETYPE_NONE && on_ground {
        v.movetype = MOVETYPE_NONE;
        p.mv.velocity = Vec3::ZERO;
    }
    if dying {
        v.deadflag = DEAD_DEAD;
        p.dead_time = now;
    }
    // A player dead for `DEATH_ANIMATION_TIME` leaves a corpse and goes to the death camera.
    if v.deadflag != DEAD_RESPAWNABLE && !p.death_cam && now > p.dead_time + DEATH_ANIMATION_TIME {
        let origin = [
            (v.origin.x * 128.0) as i32,
            (v.origin.y * 128.0) as i32,
            (v.origin.z * 128.0) as i32,
        ];
        let (angles, body) = (v.angles, v.body);
        let time_diff = ((f64::from(v.animtime) - now) * 100.0) as i32;
        let sequence = v.sequence as u8;
        let (team, player_id) = (p.team.id(), id.index as u8);
        start_death_cam(p);
        let model = eng.client_key(slot, "model").unwrap_or("urban").to_string();
        let corpse = CsUserMsg::ClCorpse {
            model,
            origin,
            angles,
            time_diff,
            sequence,
            body,
            team,
            player_id,
        };
        messages::corpse(eng, corpse);
    }
}

/// The call point of `CBasePlayer::StartDeathCam`, which `PlayerDeathThink` calls right after
/// the corpse was sent: the observer camera of the dead is the observer task's (Task 9). Until
/// then this only records what the camera's `PFLAG_OBSERVER` records, that the corpse is out.
fn start_death_cam(p: &mut Player) {
    p.death_cam = true;
}

/// `UpdateClientData` (ReGameDLL `client.cpp`, `CBasePlayer::UpdateClientData`): the player's
/// state for its own client, and the health message when the health changed.
pub(crate) fn client_data(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
) -> GoldSrcClientData {
    let id = eng.client_edict(slot);
    let Some(v) = eng.ent(id) else {
        return GoldSrcClientData::default();
    };
    let data = GoldSrcClientData {
        origin: v.origin,
        velocity: v.velocity,
        view_ofs: v.view_ofs,
        flags: v.flags,
        waterlevel: v.waterlevel,
        health: v.health,
        maxspeed: v.max_speed,
        fov: v.fov,
        deadflag: v.deadflag,
        punchangle: v.punchangle,
        iuser1: v.iuser1,
        iuser2: v.iuser2,
        iuser3: v.iuser3,
        ..Default::default()
    };
    let health = v.health;
    let mut data = data;
    if let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) {
        data.weapons = p.weapons;
    }
    if let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
        && health as i32 != p.client_health
    {
        let mut shown = (health as i32).clamp(0, 255) as u8;
        if health > 0.0 && health <= 1.0 {
            shown = 1;
        }
        p.client_health = health as i32;
        messages::health(eng, slot, shown);
    }
    data
}

/// What a player's `client_command` does: `kill`.
pub(crate) fn client_command(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    argv: &[String],
) {
    // Console commands are not case sensitive.
    if argv.first().is_some_and(|c| c.eq_ignore_ascii_case("kill")) {
        client_kill(g, eng, slot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fall_damage_follows_the_vanilla_formula() {
        // Nothing at the safe speed, 100 points per 600 units of speed above it, times 1.25.
        assert_eq!(fall_damage(MAX_PLAYER_SAFE_FALL_SPEED), 0.0);
        assert!((fall_damage(800.0) - 62.5).abs() < 1e-4);
        assert!((fall_damage(MAX_PLAYER_FATAL_FALL_SPEED) - 125.0).abs() < 1e-3);
    }

    #[test]
    fn models_follow_team_and_class() {
        assert_eq!(model_name(Team::Ct, 0), "urban");
        assert_eq!(model_name(Team::Ct, 4), "gign");
        assert_eq!(model_name(Team::Terrorist, 0), "terror");
        assert_eq!(model_name(Team::Terrorist, 2), "leet");
        assert_eq!(model_name(Team::Terrorist, 9), "terror");
    }

    #[test]
    fn armor_covers_the_body_and_the_head_only_with_a_helmet() {
        let mut p = Player::new(800);
        assert!(!p.is_armored(HITGROUP_CHEST));
        p.kevlar = ArmorType::Vest;
        assert!(p.is_armored(HITGROUP_CHEST) && p.is_armored(HITGROUP_GENERIC));
        assert!(!p.is_armored(HITGROUP_HEAD));
        p.kevlar = ArmorType::VestHelm;
        assert!(p.is_armored(HITGROUP_HEAD));
        assert!(!p.is_armored(7));
    }
}
