//! The player: its lifecycle (`ClientConnect`, `ClientPutInServer`, `CBasePlayer::Spawn`,
//! respawn, disconnect), damage and death (`TakeDamage`, `Killed`, `PlayerDeathThink`,
//! `ClientKill`), the per-command movement with `PlayerPreThink`/`PlayerPostThink` (fall damage)
//! and the client data. Ported from ReGameDLL_CS (`player.cpp`, `client.cpp`,
//! `multiplay_gamerules.cpp`, `player.h`, read 2026-11-22), the vanilla `mp.dll` behaviour (not
//! the `REGAMEDLL_FIXES` build).
//!
//! A player lives in [`GameState::players`], by slot, from `client_put_in_server` to
//! `client_disconnect`. A fresh player is [`Team::Unassigned`]: invisible, not solid, taking no
//! damage, until it has picked a team and an appearance ([`crate::teams`]) and spawns with
//! [`spawn`]. The movement is `cs_sim::pmove` over the engine's traces (`EngineWorld`); the rest
//! of `pm_shared` is not ported (a dying player moves like a walking one with no input, where
//! the original runs `PM_Physics_Toss`).
//!
//! A player that joins stands at the counter-terrorists' spawn point as in `ClientPutInServer`
//! (without the telefrag the vanilla join path would do); the `trigger_camera` intro path (the
//! player looks through the map's camera until it picks a team) is not ported.
//!
//! The dead leave a corpse: `PlayerDeathThink` sends `ClCorpse` once the player has been dead
//! for `DEATH_ANIMATION_TIME`, then calls `StartDeathCam` ([`observer::start_death_cam`]): the
//! dead player becomes an observer ([`crate::observer`]), which `PlayerPreThink` runs from then
//! on instead of `PlayerDeathThink`.
//!
//! ## Known gaps
//!
//! Not ported yet, each a later task: weapons and the active item, the economy rewards
//! (`AddAccount`),
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
use crate::menus::{self, Menu};
use crate::messages;
use crate::money;
use crate::observer::{self, ObserverState};
use crate::rules;
use crate::score;
use crate::teams::{self, JoinState};
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
    /// The kinds of damage the HUD has art for (`DMG_SHOWNHUD`: poison, acid, freeze, slow
    /// freeze, drown, burn, slow burn, nerve gas, radiation, shock). Values of the Half-Life
    /// SDK's `damage.h` (assumption: not fetched for this task).
    pub const SHOWNHUD: u32 =
        (1 << 17) | (1 << 20) | (1 << 4) | (1 << 22) | (1 << 14) | (1 << 3) | (1 << 21) | (1 << 16)
            | (1 << 18) | (1 << 8);
    /// `DMG_TIMEBASED`: the kinds that last (Half-Life SDK, assumption as above).
    pub const TIMEBASED: u32 = !0x3fff;
}

/// Hit groups (`common/const.h`).
const HITGROUP_GENERIC: i32 = 0;
pub(crate) const HITGROUP_HEAD: i32 = 1;
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
/// `HIDEHUD_*` (`cdll_dll.h`; values of the Half-Life SDK, assumption).
pub(crate) const HIDEHUD_WEAPONS: i32 = 1 << 0;
pub(crate) const HIDEHUD_HEALTH: i32 = 1 << 3;
const HIDEHUD_TIMER: i32 = 1 << 4;
const HIDEHUD_MONEY: i32 = 1 << 5;
const HIDEHUD_CROSSHAIR: i32 = 1 << 6;
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
pub(crate) const PITCH_NORM: u8 = 100;
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
/// The speed of an observer (`ResetMaxSpeed`).
const OBSERVER_MAX_SPEED: f32 = 900.0;
/// The speed of a player during the freeze period (`ResetMaxSpeed`): it cannot move.
const FROZEN_MAX_SPEED: f32 = 1.0;
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
    pub(crate) fn spawn_team(self) -> Option<SpawnTeam> {
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
    /// `m_iTeamKills`: teammates killed since the player entered the game.
    pub team_kills: i32,
    /// `m_bJustKilledTeammate`: the player killed a teammate since it last spawned.
    pub just_killed_teammate: bool,
    /// `m_bPunishedForTK`: `mp_tkpunish` made the player kill itself for a team kill, until it
    /// dies.
    pub punished_for_tk: bool,
    /// `m_bReceivesNoMoneyNextRound`: the round's reward skips the player (until it spawns).
    pub receives_no_money_next_round: bool,
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
    /// `m_bitsDamageType`: the kinds of damage since the HUD was last told.
    pub(crate) damage_bits: u32,
    /// `pev->weapons`: only the suit bit for now.
    pub(crate) weapons: u32,
    /// `PFLAG_OBSERVER`: the player is an observer ([`observer::start_observer`] ran), so the
    /// corpse was sent and `PlayerDeathThink` does not run again.
    pub(crate) pflag_observer: bool,
    /// The observer's state (`m_hObserverTarget`, `m_iObserverLastMode`, ...).
    pub(crate) obs: ObserverState,
    /// `m_afButtonPressed`: the buttons that went down with the last command.
    pub(crate) buttons_pressed: u16,
    /// Server time of the death, `m_fDeadTime` once the player is `DEAD_DEAD`.
    pub(crate) dead_time: f64,
    /// Server time of the next `PlayerDeathThink`.
    next_death_think: f64,
    /// `m_iJoiningState`: where the player is on its way into the game ([`crate::teams`]).
    pub joining: JoinState,
    /// `m_iMenu`: the menu the player is looking at.
    pub menu: Menu,
    /// `m_bTeamChanged`: the player has picked a team since the round began (the round restart
    /// clears it).
    pub team_changed: bool,
    /// `m_iNumSpawns`: how often the player has spawned in this round.
    pub num_spawns: i32,
    /// The order the players entered the server in (the user id of the original), for the team
    /// balance, which moves the last to join.
    pub(crate) join_order: u32,
    /// The buttons of the player's previous command (`m_afButtonLast`).
    pub(crate) last_buttons: u16,
    /// `m_bVGUIMenus`: the client's `_vgui_menus` (VGUI menus, else text ones).
    pub vgui_menus: bool,
    /// `m_fInitHUD`: the next client data update starts the HUD over (`ResetHUD`, the money, the
    /// team scores, and, once per entry, `InitHUD` and the rules' welcome).
    pub(crate) init_hud: bool,
    /// `m_fGameHUDInitialized`: `CHalfLifeMultiplay::InitHUD` has run for this entry.
    pub(crate) game_hud_initialized: bool,
    /// `m_iClientBattery`: the armor the client was last told.
    pub(crate) client_battery: i32,
    /// `m_bitsHUDDamage`: the damage kinds the client was last told.
    pub(crate) hud_damage: i32,
    /// `pev->dmg_take`: health lost since the HUD was last told.
    pub(crate) dmg_take: f32,
    /// `pev->dmg_save`: never set by the game (ReGameDLL `basemonster.cpp`), so always 0.
    pub(crate) dmg_save: f32,
    /// `pev->dmg_inflictor`: what last hurt the player.
    pub(crate) dmg_inflictor: Option<EdictId>,
    /// `m_iHideHUD`: the HUD elements hidden (`HIDEHUD_*`).
    pub(crate) hide_hud: i32,
    /// `m_iClientHideHUD`: what the client was last told.
    pub(crate) client_hide_hud: i32,
    /// `m_iFOV`.
    pub(crate) fov: u8,
    /// `m_iClientFOV`.
    pub(crate) client_fov: u8,
    /// `m_iTrain & TRAIN_NEW`: the train HUD has not been sent yet.
    pub(crate) train_new: bool,
    /// Chat, the name and `ignoremsg` ([`crate::chat`]).
    pub chat: crate::chat::ChatState,
    /// `m_bHasNightVision`: set by the buy menu (Task 10).
    pub(crate) has_night_vision: bool,
    /// `m_bNightVisionOn`.
    pub(crate) night_vision_on: bool,
    /// The radio, the radar, the status bar ([`crate::hud_misc`]).
    pub hud: crate::hud_misc::HudState,
}

impl Player {
    /// A player as `ClientPutInServer` leaves it: no team, nothing earned, `money` in the
    /// account.
    pub(crate) fn new(money: i32) -> Self {
        Self {
            team: Team::Unassigned,
            class: 0,
            health: 100.0,
            armor: 0.0,
            kevlar: ArmorType::None,
            frags: 0,
            deaths: 0,
            money,
            team_kills: 0,
            just_killed_teammate: false,
            punished_for_tk: false,
            receives_no_money_next_round: false,
            alive: false,
            takes_damage: false,
            not_killed: true,
            mv: PlayerMoveState::default(),
            fall_velocity: 0.0,
            next_suicide_time: 0.0,
            client_health: -1,
            last_hit_group: HITGROUP_GENERIC,
            damage_bits: 0,
            weapons: 0,
            pflag_observer: false,
            obs: ObserverState::default(),
            buttons_pressed: 0,
            dead_time: 0.0,
            next_death_think: 0.0,
            joining: JoinState::ShowLText,
            menu: Menu::Off,
            team_changed: false,
            num_spawns: 0,
            join_order: 0,
            last_buttons: 0,
            vgui_menus: true,
            init_hud: true,
            game_hud_initialized: false,
            client_battery: -1,
            hud_damage: -1,
            dmg_take: 0.0,
            dmg_save: 0.0,
            dmg_inflictor: None,
            hide_hud: 0,
            client_hide_hud: 0,
            fov: 90,
            client_fov: 0,
            train_new: true,
            chat: Default::default(),
            has_night_vision: false,
            night_vision_on: false,
            hud: Default::default(),
        }
    }

    /// Whether the player has joined the game with a team that plays: the stand-in for
    /// `m_iJoiningState == JOINED` with a team that spawns.
    pub(crate) fn in_play(&self) -> bool {
        self.joining == JoinState::Joined && self.team.plays()
    }

    /// What `CBasePlayer::Spawn` and `Precache` reset about what the client knows, so the next
    /// client data update sends it all again (`m_fInitHUD`, `m_iClientBattery`,
    /// `m_bitsHUDDamage`, `m_iClientFOV`, `m_iFOV`, `m_iClientHideHUD`, `m_iTrain`).
    fn reset_hud_state(&mut self) {
        self.init_hud = true;
        self.client_health = -1;
        self.client_battery = -1;
        self.hud_damage = -1;
        self.damage_bits = 0;
        self.dmg_take = 0.0;
        self.dmg_save = 0.0;
        self.dmg_inflictor = None;
        self.fov = 90;
        self.client_fov = 0;
        self.train_new = true;
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
    let mut player = Player::new(money::check_start_money(eng));
    player.vgui_menus = prefers_vgui_menus(eng, slot);
    player.chat = crate::chat::ChatState::entered(eng.client_key(slot, "name").unwrap_or(""));
    g.join_counter += 1;
    player.join_order = g.join_counter;
    g.players[index] = Some(player);
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

/// `SetPrefsFromUserinfo`: whether the client wants VGUI menus (`_vgui_menus`, on when the
/// key is missing or empty).
fn prefers_vgui_menus(eng: &EngineApi<'_>, slot: u8) -> bool {
    match eng.client_key(slot, "_vgui_menus") {
        Some(v) if !v.is_empty() => menus::atoi(v) != 0,
        _ => true,
    }
}

/// `ClientUserInfoChanged`: the part that concerns the menus.
pub(crate) fn userinfo_changed(g: &mut GameState, eng: &EngineApi<'_>, slot: u8) {
    let vgui = prefers_vgui_menus(eng, slot);
    if let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    {
        p.vgui_menus = vgui;
    }
}

/// `ClientDisconnect`: the slot's player is gone and its edict inert.
pub(crate) fn disconnect(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if g.players.get(usize::from(slot)).is_none_or(Option::is_none) {
        return;
    }
    // `CHalfLifeMultiplay::ClientDisconnected`: the round rules see the player go.
    rules::on_disconnect(g, eng, slot);
    let Some(entry) = g.players.get_mut(usize::from(slot)) else {
        return;
    };
    if entry.take().is_some() {
        teams::announce_departure(eng, slot);
    }
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

/// `SetClientUserInfoModel` and `SetNewPlayerModel` of `HandleMenu_ChooseAppearance`: the player
/// of `slot` wears appearance `class` of `team` (the userinfo `model` key the clients render it
/// by) on the team's default model file; a player on no team gets the plain `player.mdl`.
pub(crate) fn apply_model(eng: &mut EngineApi<'_>, slot: u8, team: Team, class: u8) {
    let file = match team {
        Team::Terrorist => {
            eng.set_client_key(slot, "model", model_name(team, class));
            "models/player/terror/terror.mdl"
        }
        Team::Ct => {
            eng.set_client_key(slot, "model", model_name(team, class));
            "models/player/urban/urban.mdl"
        }
        Team::Unassigned | Team::Spectator => "models/player.mdl",
    };
    let id = eng.client_edict(slot);
    eng.set_model(id, file);
}

/// Spawns the player of `slot` on `team` with appearance `class`: `CBasePlayer::Spawn` of a
/// joined player, including `GetPlayerSpawnSpot` (the spawn point may kill the players standing
/// at it) and the team's model. `false` for a team that does not spawn or a slot with no
/// player.
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
        // `Spawn` (player.cpp:6030-6039) starts the client's cached `m_iClientHideHUD` over
        // only for a player killed before (or under `REGAMEDLL_FIXES`, not vanilla), so a
        // joiner's first update sends no `HideWeapon`.
        p.client_hide_hud = -1;
        p.has_night_vision = false;
    }
    p.hide_hud &=
        !(HIDEHUD_WEAPONS | HIDEHUD_HEALTH | HIDEHUD_TIMER | HIDEHUD_MONEY | HIDEHUD_CROSSHAIR);
    p.alive = true;
    p.takes_damage = true;
    p.not_killed = true;
    p.just_killed_teammate = false;
    p.receives_no_money_next_round = false;
    p.fall_velocity = 0.0;
    p.last_hit_group = HITGROUP_GENERIC;
    observer::on_spawn(p);
    p.chat.on_spawn();
    // `PlayerSpawn`: `pev->weapons |= (1 << WEAPON_SUIT)`.
    p.weapons |= 1 << WEAPON_SUIT;
    p.reset_hud_state();
    p.mv = PlayerMoveState::at_spawn(origin);

    if eng.ent(id).is_some_and(|v| v.modelindex == 0) {
        eng.set_model(id, "models/player.mdl");
    }
    eng.set_client_key(slot, "model", model_name(team, class));
    if let Some(v) = eng.ent_mut(id) {
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
    reset_max_speed(g, eng, slot);
    if let Some(p) = g.players[index].as_mut() {
        p.num_spawns += 1;
    }
    if let Some(p) = g.players[index].as_ref() {
        messages::score_info(eng, id.index, p);
    }
    // A name asked for while dead is taken at the spawn.
    crate::chat::apply_pending_name(g, eng, slot);
    // `PlayerSpawn` takes the items away and starts the weapons over; no weapons exist yet, so
    // the ammo HUD is told there is none (`RemoveAllItems`).
    messages::cur_weapon_none(eng, slot);
    crate::hud_misc::on_spawn(g, eng, slot);
    true
}

/// `CBasePlayer::ResetMaxSpeed`, for a player with no weapon: 900 for an observer, 1 while the
/// round is in its freeze period (nobody moves), else [`NO_ITEM_MAX_SPEED`]. The active item
/// sets the speed of an armed player once weapons exist.
pub(crate) fn reset_max_speed(g: &GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    let freeze = g.rules.is_freeze_period();
    if let Some(v) = eng.ent_mut(id) {
        v.max_speed = if v.iuser1 != 0 {
            OBSERVER_MAX_SPEED
        } else if freeze {
            FROZEN_MAX_SPEED
        } else {
            NO_ITEM_MAX_SPEED
        };
    }
}

/// `respawn()` (ReGameDLL `client.cpp`): spawns the player again on its team. The round rules
/// call this when a round starts and, with `forcerespawn`, after a death; nothing respawns a
/// dead player by itself. `false` when the player has no team that spawns.
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
    if g.players
        .get(usize::from(slot))
        .is_none_or(Option::is_none)
    {
        return;
    }
    // `PlayerPreThink`: the way into the game advances with the player's commands.
    teams::pre_think(g, eng, slot);
    rules::player_think(g, slot, cmd.buttons);
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    // An observer (the dead after their camera, the spectators) does not walk: it flies, or
    // follows a player (`PlayerPreThink` runs `Observer_Think` and returns).
    if p.pflag_observer && observer::mode_of(eng, slot) != observer::OBS_NONE {
        observer::run_cmd(g, eng, slot, cmd);
        return;
    }
    if cmd.msec == 0 || !p.in_play() {
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
    p.damage_bits |= bits;
    // What the `Damage` message reports (`CBaseMonster::TakeDamage`, ReGameDLL
    // `basemonster.cpp:430-441`, as reviewed): for a player, the inflictor when there is one
    // (the old one stays otherwise) and the health lost. Nothing sets `dmg_save`, so the
    // message's armor byte is always 0.
    if inflictor.is_some() {
        p.dmg_inflictor = inflictor;
    }
    p.dmg_take += damage.trunc();
    let health = p.health;
    let id = eng.client_edict(slot);
    if let Some(v) = eng.ent_mut(id) {
        v.health = health;
    }
    if health <= 0.0 {
        killed(g, eng, slot, attacker, inflictor);
    }
    // The observers that follow the player are told its health.
    observer::notify_spec_health(g, eng, slot);
    health > 0.0
}

/// `CBasePlayer::Kill`: the player kills itself. `false` when it cannot (it has not joined).
pub(crate) fn kill(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) -> bool {
    let id = eng.client_edict(slot);
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return false;
    };
    // Only a player in the game, and alive (an observer is not killed).
    if !p.in_play() || !p.alive {
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

/// `CHalfLifeMultiplay::PlayerKilled` (the part this task has the data for) and
/// `CBasePlayer::Killed`: the death notice, the frag rules, the scoreboard, then the player
/// dies (`DEAD_DYING`, not solid, `MOVETYPE_TOSS`) and runs `PlayerDeathThink`.
///
/// Frags: a suicide costs one, killing a teammate costs one, killing an enemy gives one, the
/// world (an attacker that is no player) changes none. Money: 300 for an enemy, -3300 for a
/// teammate ([`money::killed_by`]). A death ends with `CheckWinConditions` ([`rules`]).
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
    if g.players[index].is_none() {
        return;
    }
    // `PlayerKilled` with `DeathNotice`: the death notice, deaths, frags, money, scoreboard.
    score::player_killed(g, eng, slot, attacker, inflictor);

    // CBasePlayer::Killed.
    if let Some(p) = g.players[index].as_mut() {
        // A player punished for a team kill is told why it cannot play
        // (`#Hint_cannot_play_because_tk` needs the hint queue, not ported yet).
        p.punished_for_tk = false;
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
    // The camera of the dying: it chases the killer, or the screen fades to black.
    observer::killed(g, eng, slot, attacker);
    // `SetScoreboardAttributes()`: the scoreboard marks the player dead.
    messages::set_scoreboard_attributes(g, eng, usize::from(slot));
    // `DeathSound`: one of four, `RANDOM_LONG(1, 4)`.
    let sample = match g.random_range(1, 4) {
        1 => "player/die1.wav",
        2 => "player/die2.wav",
        3 => "player/die3.wav",
        _ => "player/death6.wav",
    };
    eng.emit_sound(id, CHAN_VOICE, sample, 1.0, ATTN_NORM, PITCH_NORM);
    // A team may be out of players now (`CheckWinConditions` of `Killed`).
    rules::check_win_conditions(g, eng);
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
    if v.deadflag != DEAD_RESPAWNABLE && !p.pflag_observer && now > p.dead_time + DEATH_ANIMATION_TIME {
        let origin = [
            (v.origin.x * 128.0) as i32,
            (v.origin.y * 128.0) as i32,
            (v.origin.z * 128.0) as i32,
        ];
        let (angles, body) = (v.angles, v.body);
        let time_diff = ((f64::from(v.animtime) - now) * 100.0) as i32;
        let sequence = v.sequence as u8;
        let (team, player_id) = (p.team.id(), id.index as u8);
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
        // Go to the dead camera.
        observer::start_death_cam(g, eng, slot);
    }
}

/// `UpdateClientData` (ReGameDLL `player.cpp`, `CBasePlayer::UpdateClientData`): the player's
/// state for its own client, and the HUD messages for what changed ([`messages::update_client_data`]).
pub(crate) fn client_data(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
) -> GoldSrcClientData {
    let id = eng.client_edict(slot);
    let Some(own) = eng.ent(id) else {
        return GoldSrcClientData::default();
    };
    // An observer in first person is sent the state of the player it follows, with its own
    // `iuser*` and death state.
    let source = observer::first_person_source(g, eng, slot).map(|t| (t, eng.client_edict(t)));
    let v = source.and_then(|(_, sid)| eng.ent(sid)).unwrap_or(own);
    let data = GoldSrcClientData {
        origin: v.origin,
        velocity: v.velocity,
        view_ofs: v.view_ofs,
        flags: v.flags,
        waterlevel: v.waterlevel,
        health: v.health,
        maxspeed: v.max_speed,
        fov: v.fov,
        deadflag: own.deadflag,
        punchangle: v.punchangle,
        iuser1: own.iuser1,
        iuser2: own.iuser2,
        iuser3: own.iuser3,
        ..Default::default()
    };
    let mut data = data;
    let weapons_of = source.map_or(slot, |(t, _)| t);
    if let Some(p) = g
        .players
        .get(usize::from(weapons_of))
        .and_then(Option::as_ref)
    {
        data.weapons = p.weapons;
    }
    messages::update_client_data(g, eng, slot);
    data
}

/// What a player's `client_command` does: `kill`, and the team and menu commands (`ClientCommand_`
/// of ReGameDLL `client.cpp`). Nothing happens for a slot with no player or an unknown command.
pub(crate) fn client_command(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    argv: &[String],
) {
    let Some(command) = argv.first() else {
        return;
    };
    if g.players
        .get(usize::from(slot))
        .is_none_or(Option::is_none)
    {
        return;
    }
    // `kill` is the engine's command, which the engine matches without regard to case.
    if command.eq_ignore_ascii_case("kill") {
        client_kill(g, eng, slot);
        return;
    }
    // The game compares the rest exactly (`FStrEq`).
    match command.as_str() {
        "menuselect" => menus::menuselect(g, eng, slot, argv),
        "chooseteam" => teams::choose_team(g, eng, slot),
        "jointeam" => teams::join_team(g, eng, slot, argv),
        "joinclass" => teams::join_class(g, eng, slot, argv),
        "say" => crate::chat::say_command(g, eng, slot, argv, false),
        "say_team" => crate::chat::say_command(g, eng, slot, argv, true),
        "ignoremsg" => crate::chat::ignore_msg_command(g, eng, slot),
        _ => {
            // Distinct commands, so the order of `ClientCommand_` (`ignorerad`, the observer
            // commands, then the living player's) does not matter.
            if !observer::command(g, eng, slot, argv) {
                crate::hud_misc::client_command(g, eng, slot, argv);
            }
        }
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
