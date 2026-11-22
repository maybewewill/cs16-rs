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
//! [`spawn`]. The movement is `cs_sim::pm`, the port of `pm_shared`, run by [`crate::pmove`] in the
//! order of `SV_RunCmd` (`PlayerPreThink`, `PM_Move`, `PlayerPostThink`).
//!
//! A player that joins looks through the map's first `trigger_camera` at its target until it picks
//! a team ([`crate::intro`]: the next camera every six seconds); a map with no camera that has a
//! target puts it at the counter-terrorists' spawn point as in `ClientPutInServer` (without the
//! telefrag the vanilla join path would do).
//!
//! The dead leave a corpse: `PlayerDeathThink` sends `ClCorpse` once the player has been dead
//! for `DEATH_ANIMATION_TIME`, then calls `StartDeathCam` ([`observer::start_death_cam`]): the
//! dead player becomes an observer ([`crate::observer`]), which `PlayerPreThink` runs from then
//! on instead of `PlayerDeathThink`.
//!
//! ## Known gaps
//!
//! Not ported yet, each a later task: the flinch animations, the shield, the
//! bomb and grenade flags of `TakeDamage`, and the VIP. Bullets and the damage they do
//! (`TraceAttack`, `TakeDamage`, `Pain`) are in [`crate::bullets`] and here.
//!
//! The flashlight (`impulse 100`, the battery and its `Flashlight`/`FlashBat` messages), the
//! Geiger counter and the spray (`impulse 201`) are in [`crate::suit`] and [`crate::spray`], the
//! hints (`HintMessage`, `m_flDisplayHistory`) in [`crate::hint`]. `PreThink` hides the
//! flashlight from the HUD unless `mp_flashlight` allows it (`HIDEHUD_FLASHLIGHT`,
//! [`run_usercmd`]), and `FlashlightTurnOn` honours `FAllowFlashlight()` too.
//!
//! `WaterMove` (`PreThink`: drowning, `air_finished`, `DMG_DROWN` and its recovery) is in
//! [`water_move`] and [`check_time_based_damage`]; `waterlevel` and `watertype` come from the
//! movement.
//!
//! Fields of the client data ([`client_data`]) that stay zero for now:
//! - `weaponanim`, `viewmodel`, `m_flNextAttack`, the ammo fields and the `iuser3` flags
//!   (`PLAYER_CAN_SHOOT`, `PLAYER_IN_BOMB_ZONE`, `PLAYER_HOLDING_SHIELD`; only
//!   `PLAYER_FREEZE_TIME_OVER` is set): they belong to the weapons and the bomb zones (2D, 2E).
//! - `physinfo` (`ENGINE_GETPHYSINFO`): the movement does not read it.
//! - `weapons` carries only the suit bit (`1 << WEAPON_SUIT`) that `PlayerSpawn` sets; the
//!   weapon bits come with the weapons.

use cs_assets::bsp::SpawnTeam;
use cs_engine::consts::{
    FL_CLIENT, FL_DORMANT, FL_DUCKING, FL_INWATER, FL_ONGROUND, FL_PROXY, FL_SPECTATOR,
    MOVETYPE_NOCLIP, MOVETYPE_NONE, MOVETYPE_TOSS, MOVETYPE_WALK, SOLID_NOT,
    SOLID_SLIDEBOX,
};
use cs_engine::{EdictId, EngineApi, EntTrace, MsgDest};
use cs_proto48::delta::{GoldSrcClientData, GoldSrcUserCmd};
use cs_proto48::messages::svc;
use cs_proto48::usermsg::CsUserMsg;
use cs_sim::pm::consts::{IN_ATTACK, IN_JUMP, IN_USE, PM_VEC_VIEW};
use glam::Vec3;

use crate::GameState;
use crate::bullets;
use crate::entity::{FCAP_CONTINUOUS_USE, FCAP_IMPULSE_USE, FCAP_ONOFF_USE, UseType};
use crate::menus::{self, Menu};
use crate::intro;
use crate::weapons::core::{self as weapon_core, PlayerAnim};
use crate::weapons::{MAX_AMMO_SLOTS, MAX_ITEM_TYPES, WEAPON_SUIT, WeaponId, WeaponSlotItem};
use crate::messages;
use crate::money;
use crate::observer::{self, ObserverState};
use crate::player_anim;
use crate::pmove;
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
    pub const BURN: u32 = 1 << 3;
    /// `DMG_PARALYZE`, the first of the time-based kinds (`DMG_PARALYZE << ITBD_*`).
    pub const PARALYZE: u32 = 1 << 15;
    pub const DROWNRECOVER: u32 = 1 << 19;
    pub const ACID: u32 = 1 << 20;
    /// Counter-Strike's `DMG_EXPLOSION` (`dlls/util.h`).
    pub const EXPLOSION: u32 = 1 << 24;
    /// The kinds of damage the HUD has art for (`DMG_SHOWNHUD`: poison, acid, freeze, slow
    /// freeze, drown, burn, slow burn, nerve gas, radiation, shock). Values of the Half-Life
    /// SDK's `damage.h` (assumption: not fetched for this task).
    pub const SHOWNHUD: u32 =
        (1 << 17) | (1 << 20) | (1 << 4) | (1 << 22) | (1 << 14) | (1 << 3) | (1 << 21) | (1 << 16)
            | (1 << 18) | (1 << 8);
    /// `DMG_TIMEBASED`: the kinds that last (Half-Life SDK, assumption as above).
    pub const TIMEBASED: u32 = !0x3fff & 0x7fff_ffff;
}

/// Hit groups (`common/const.h`).
pub(crate) const HITGROUP_GENERIC: i32 = 0;
pub(crate) const HITGROUP_HEAD: i32 = 1;
pub(crate) const HITGROUP_CHEST: i32 = 2;
pub(crate) const HITGROUP_STOMACH: i32 = 3;
pub(crate) const HITGROUP_LEFTARM: i32 = 4;
pub(crate) const HITGROUP_RIGHTARM: i32 = 5;
const HITGROUP_LEFTLEG: i32 = 6;
const HITGROUP_RIGHTLEG: i32 = 7;
pub(crate) const HITGROUP_SHIELD: i32 = 8;

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
/// `ITBD_*` (`player.h`): the time-based damage kinds, `DMG_PARALYZE << index` (0 paralyze, 1 nerve
/// gas, 2 poison, 3 radiation, 4 drown recover, 5 acid, 6 slow burn, 7 slow freeze).
const ITBD_POISON: usize = 2;
const ITBD_DROWN_RECOVER: usize = 4;
const ITBD_END: usize = 8;
/// `AIRTIME`: seconds a player can stay under water.
const AIRTIME: f64 = 12.0;
/// `HIDEHUD_*` (`cdll_dll.h`, read 2026-11-22).
pub(crate) const HIDEHUD_WEAPONS: i32 = 1 << 0;
pub(crate) const HIDEHUD_FLASHLIGHT: i32 = 1 << 1;
pub(crate) const HIDEHUD_HEALTH: i32 = 1 << 3;
const HIDEHUD_TIMER: i32 = 1 << 4;
const HIDEHUD_MONEY: i32 = 1 << 5;
pub(crate) const HIDEHUD_CROSSHAIR: i32 = 1 << 6;
/// `CONTENTS_WATER`.
const CONTENT_WATER: i32 = -3;
const CONTENT_SLIME: i32 = -4;
const CONTENT_LAVA: i32 = -5;
/// `EF_NODRAW` and `EF_NOINTERP` (the engine's `const.h` values, one home for the game).
pub(crate) use cs_engine::consts::{EF_NODRAW, EF_NOINTERP};
/// `CHAN_VOICE` and `CHAN_ITEM`, `ATTN_NORM`, `PITCH_NORM`.
pub(crate) const CHAN_VOICE: u8 = 2;
const CHAN_BODY: u8 = 4;
pub(crate) const CHAN_ITEM: u8 = 3;
pub(crate) const ATTN_NORM: f32 = 0.8;
pub(crate) const PITCH_NORM: u8 = 100;
/// `VEC_HULL_MIN`/`MAX` and `VEC_DUCK_HULL_MIN`/`MAX`.
const HULL_MIN: Vec3 = Vec3::new(-16.0, -16.0, -36.0);
const HULL_MAX: Vec3 = Vec3::new(16.0, 16.0, 36.0);
/// `VEC_VIEW`: the view offset of a standing player.
const VEC_VIEW: f32 = PM_VEC_VIEW;
/// `PLAYER_FREEZE_TIME_OVER` (`cdll_dll.h`, read 2026-11-22), a bit of `iuser3` in the client
/// data. The name is inverted in the source: the bit is set while the freeze period is on.
const PLAYER_FREEZE_TIME_OVER: i32 = 1 << 1;
/// `PLAYER_CAN_SHOOT` and `PLAYER_HOLDING_SHIELD` (`cdll_dll.h`), bits of `iuser3` in the client
/// data. (`PLAYER_IN_BOMB_ZONE` is phase 2E.)
const PLAYER_CAN_SHOOT: i32 = cs_sim::pm::consts::PLAYER_CAN_SHOOT;
/// `PLAYER_IN_BOMB_ZONE` (`cdll_dll.h`): the `iuser3` bit of a player whose `HandleSignals`
/// state has `SIGNAL_BOMB` ([`crate::zones`] raises it).
const PLAYER_IN_BOMB_ZONE: i32 = cs_sim::pm::consts::PLAYER_IN_BOMB_ZONE;
const PLAYER_HOLDING_SHIELD: i32 = cs_sim::pm::consts::PLAYER_HOLDING_SHIELD;
/// `DEATH_ANIMATION_TIME` (`gamerules.h`): how long a death lasts before the corpse is sent.
const DEATH_ANIMATION_TIME: f64 = 3.0;
/// `HITGROUP_SHIELD_ENABLED` and `HITGROUP_SHIELD_DISABLED`: the values of `pev->gamestate` of
/// a player (`player.h`, read 2026-11-22); the engine's hitbox traces skip the shield box at 1.
const HITGROUP_SHIELD_ENABLED: i32 = 0;
const HITGROUP_SHIELD_DISABLED: i32 = 1;
/// The speed of a player with no active item (`CBasePlayer::ResetMaxSpeed`, ReGameDLL
/// `player.cpp`); weapons set it themselves once they exist.
const NO_ITEM_MAX_SPEED: f32 = 240.0;
/// The speed of an observer (`ResetMaxSpeed`).
const OBSERVER_MAX_SPEED: f32 = 900.0;
/// The speed of a player during the freeze period (`ResetMaxSpeed`): it cannot move.
const FROZEN_MAX_SPEED: f32 = 1.0;
/// `IN_SCORE` (`in_buttons.h`): the scoreboard key, the one button `PlayerDeathThink` ignores.
const IN_SCORE: i32 = 1 << 15;
/// Seconds between two suicides (`ClientKill`).
const SUICIDE_DELAY: f64 = 1.0;

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
    /// `pev->armorvalue`. Kept here only: the edict has no such field; the client hears of it
    /// in `Battery` (client data update) and `ArmorType` ([`crate::buy`]).
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
    /// `m_flFallVelocity`.
    pub(crate) fall_velocity: f32,
    /// `m_flVelocityModifier`: the share of its speed the player keeps (1 = all); it climbs back
    /// to 1 by 0.01 a command on the ground (`PreThink`). 2D lowers it on a hit.
    pub(crate) velocity_modifier: f32,
    /// `pmove->oldangles`: the player's `v_angle` from before the command's angles were applied.
    pub(crate) oldangles: Vec3,
    /// `m_random_seed` (`CmdStart`): the seed of the command being run, for the weapons' spread.
    pub(crate) random_seed: u32,
    /// `m_fNextSuicideTime`.
    pub(crate) next_suicide_time: f64,
    /// `m_iClientHealth`: the health the client was last told.
    pub(crate) client_health: i32,
    /// `m_LastHitGroup`.
    pub(crate) last_hit_group: i32,
    /// `m_bHighDamage`: the last hit that made the player flinch was a hard one (over 60 on the
    /// head, over 20 elsewhere); the death animation reads it.
    pub(crate) high_damage: bool,
    /// `m_vBlastVector`: from the inflictor of the last explosion to the player.
    pub(crate) blast_vector: Vec3,
    /// `m_bitsDamageType`: the kinds of damage since the HUD was last told.
    pub(crate) damage_bits: u32,
    /// `m_idrowndmg` and `m_idrownrestored`: the drowning damage taken and given back.
    pub(crate) drown_dmg: i32,
    pub(crate) drown_restored: i32,
    /// `m_rgbTimeBasedDamage`: seconds-of-2 left of each time-based kind (`ITBD_*`).
    pub(crate) time_based_damage: [u8; ITBD_END],
    /// `m_tbdPrev`: server time of the last time-based damage check.
    pub(crate) tbd_prev: f32,
    /// `pev->weapons`: the suit bit, and the bit of every weapon the player carries.
    pub(crate) weapons: u32,
    /// `m_rgpPlayerItems`: the weapons the player carries, by inventory slot (`iItemSlot()`,
    /// 1 to 5; 0 is not used). Each list is the linked list `m_pNext` makes in C: the head
    /// (the weapon picked up last) first.
    pub(crate) items: [Vec<WeaponSlotItem>; MAX_ITEM_TYPES],
    /// `m_bHasPrimary`: the player carries (or carried since it last lost its items) a primary
    /// weapon; the armoury hands out no gun to such a player.
    pub(crate) has_primary: bool,
    /// `m_pActiveItem`, by weapon id.
    pub(crate) active_item: Option<WeaponId>,
    /// `m_pLastItem`.
    pub(crate) last_item: Option<WeaponId>,
    /// `m_pClientActiveItem`: the active item the client was last told.
    pub(crate) client_active_item: Option<WeaponId>,
    /// `m_fWeapon`: a `CurWeapon` has been sent since the spawn.
    pub(crate) f_weapon: bool,
    /// `m_rgAmmo`: the reserve of each ammo type (the registry's index, see
    /// [`crate::weapons::ammo_index`]).
    pub(crate) ammo: [i32; MAX_AMMO_SLOTS],
    /// `m_rgAmmoLast`: what the client was last told of each ammo.
    pub(crate) ammo_last: [i32; MAX_AMMO_SLOTS],
    /// The legacy counters `ammo_buckshot`, `ammo_9mm`, ... of `TabulateAmmo` (the order of
    /// [`weapon_core::TABULATED_AMMO`]); the client data carries them.
    pub(crate) tabulated_ammo: [i32; 10],
    /// `m_flNextAttack`: seconds until the player may act with a weapon again (a countdown).
    pub(crate) next_attack: f32,
    /// `m_bCanShoot`.
    pub(crate) can_shoot: bool,
    /// `m_bIsDefusing` (the bomb is phase 2E).
    pub(crate) is_defusing: bool,
    /// `m_bOwnsShield` (the shield is Task 12).
    pub(crate) owns_shield: bool,
    /// `m_bShieldDrawn`.
    pub(crate) shield_drawn: bool,
    /// `m_bResumeZoom`: a shot closed the scope; it opens again when the weapon may shoot.
    pub(crate) resume_zoom: bool,
    /// `m_iLastZoom`: the FOV to go back to.
    pub(crate) last_zoom: i32,
    /// `m_flEjectBrass`: server time the shell leaves the weapon, 0 for none.
    pub(crate) eject_brass: f32,
    /// `m_szAnimExtention`: the animation set of the weapon in hand.
    pub(crate) anim_extension: String,
    /// The animation of the third-person model: `m_Activity`, the gait and the rest
    /// ([`crate::player_anim`]).
    pub(crate) anim: crate::player_anim::AnimState,
    /// `PFLAG_OBSERVER`: the player is an observer ([`observer::start_observer`] ran), so the
    /// corpse was sent and `PlayerDeathThink` does not run again.
    pub(crate) pflag_observer: bool,
    /// The observer's state (`m_hObserverTarget`, `m_iObserverLastMode`, ...).
    pub(crate) obs: ObserverState,
    /// `m_afButtonPressed`: the buttons that went down with the last command.
    pub(crate) buttons_pressed: u16,
    /// `m_afButtonReleased`: the buttons that went up with the last command.
    pub(crate) buttons_released: u16,
    /// Server time of the death, `m_fDeadTime` once the player is `DEAD_DEAD`.
    pub(crate) dead_time: f64,
    /// `m_iJoiningState`: where the player is on its way into the game ([`crate::teams`]).
    pub joining: JoinState,
    /// `m_pIntroCamera`: the `trigger_camera` whose view the joining player has
    /// ([`crate::intro`]); `None` once it picked a team, or when the map has none to look through.
    pub intro_camera: Option<EdictId>,
    /// `m_fIntroCamTime`: server time of the next camera (0 until a camera sets it).
    pub intro_cam_time: f64,
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
    /// `m_bHasNightVision`: the player bought night vision goggles ([`crate::buy`]).
    pub(crate) has_night_vision: bool,
    /// `m_bNightVisionOn`: the goggles are on ([`crate::hud_misc`]).
    pub(crate) night_vision_on: bool,
    /// The radio, the radar, the status bar ([`crate::hud_misc`]).
    pub hud: crate::hud_misc::HudState,
    /// `m_signals`: the zones the player stands in ([`crate::buy`]).
    pub(crate) signals: crate::buy::Signals,
    /// `m_tmHandleSignals`: server time `HandleSignals` runs next.
    pub(crate) next_handle_signals: f64,
    /// `m_bHasC4`: the player carries the bomb (the C4 weapon and its handing out are 2E,
    /// tasks 4 and 5; the zones and the icons already read the flag).
    pub has_c4: bool,
    /// `m_pentCurBombTarget`: the `func_bomb_target` the player's last touch was in (set by
    /// [`crate::zones::BombTarget`]'s touch, cleared by `Spawn`).
    pub cur_bomb_target: Option<EdictId>,
    /// `m_bEscaped`: the player has reached an escape zone or a VIP safety zone this round.
    pub escaped: bool,
    /// `m_bIsVIP`: the player is the VIP of the round (the VIP is task 9; the buy check and the
    /// safety zone already read the flag).
    pub is_vip: bool,
    /// `m_iTimeCheckAllowed`: `timeleft` answers when the server time is past this (whole
    /// seconds).
    pub(crate) time_check_allowed: i32,
    /// `m_flLastCommandTime[CMD_LISTMAPS]`: `listmaps` answers when the server time reaches
    /// this.
    pub(crate) next_listmaps: f64,
    /// `m_blindUntilTime`: server time the flashbang's blindness counts as over, for the status
    /// bar (`UpdateStatusBar` names no target while it lasts). `Blind` sets it, `Spawn` clears
    /// it (`player.cpp` 5894, 8259, read 2026-11-22).
    pub(crate) blind_until_time: f32,
    /// `m_blindStartTime`: server time of the last flash that hit the player.
    pub(crate) blind_start_time: f32,
    /// `m_blindHoldTime`: seconds the screen stays white.
    pub(crate) blind_hold_time: f32,
    /// `m_blindFadeTime`: seconds the white takes to fade out.
    pub(crate) blind_fade_time: f32,
    /// `m_blindAlpha`: the opacity (0 to 255) of the white.
    pub(crate) blind_alpha: i32,
    /// `m_hintMessageQueue`: the hints waiting to be shown ([`crate::hint`]).
    pub(crate) hint_queue: crate::hint::HintQueue,
    /// `m_flDisplayHistory`: the hints the player has been shown (`DHF_*`).
    pub(crate) display_history: u32,
    /// `m_bShowHints`: the client's `_ah` (`cl_autohelp`).
    pub(crate) show_hints: bool,
    /// `m_iFlashBattery`: the flashlight's charge, 0 to 100 ([`crate::suit`]).
    pub(crate) flash_battery: i32,
    /// `m_flFlashLightTime`: server time the battery changes next, 0 for never.
    pub(crate) flash_light_time: f32,
    /// `m_flgeigerRange`: distance to the nearest radiation source (1000 for none).
    pub(crate) geiger_range: f32,
    /// `m_flgeigerDelay`: server time of the next Geiger update.
    pub(crate) geiger_delay: f32,
    /// `m_igeigerRangePrev`: the range last told to the client (before the division by 4).
    pub(crate) geiger_range_prev: i32,
    /// `m_flNextDecalTime`: server time the player may spray again ([`crate::spray`]).
    pub(crate) next_decal_time: f32,
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
            fall_velocity: 0.0,
            velocity_modifier: 1.0,
            oldangles: Vec3::ZERO,
            random_seed: 0,
            next_suicide_time: 0.0,
            client_health: -1,
            last_hit_group: HITGROUP_GENERIC,
            high_damage: false,
            blast_vector: Vec3::ZERO,
            damage_bits: 0,
            drown_dmg: 0,
            drown_restored: 0,
            time_based_damage: [0; ITBD_END],
            tbd_prev: 0.0,
            weapons: 0,
            items: Default::default(),
            has_primary: false,
            active_item: None,
            last_item: None,
            client_active_item: None,
            f_weapon: false,
            ammo: [0; MAX_AMMO_SLOTS],
            ammo_last: [0; MAX_AMMO_SLOTS],
            tabulated_ammo: [0; 10],
            next_attack: 0.0,
            can_shoot: false,
            is_defusing: false,
            owns_shield: false,
            shield_drawn: false,
            resume_zoom: false,
            last_zoom: 90,
            eject_brass: 0.0,
            anim_extension: String::new(),
            anim: crate::player_anim::AnimState::new(),
            pflag_observer: false,
            obs: ObserverState::default(),
            buttons_pressed: 0,
            buttons_released: 0,
            dead_time: 0.0,
            joining: JoinState::ShowLText,
            intro_camera: None,
            intro_cam_time: 0.0,
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
            signals: Default::default(),
            next_handle_signals: 0.0,
            has_c4: false,
            cur_bomb_target: None,
            escaped: false,
            is_vip: false,
            time_check_allowed: 0,
            next_listmaps: 0.0,
            blind_until_time: 0.0,
            blind_start_time: 0.0,
            blind_hold_time: 0.0,
            blind_fade_time: 0.0,
            blind_alpha: 0,
            hint_queue: Default::default(),
            display_history: 0,
            show_hints: true,
            flash_battery: 99,
            flash_light_time: 1.0,
            geiger_range: 1000.0,
            geiger_delay: 0.0,
            geiger_range_prev: 1000,
            next_decal_time: 0.0,
        }
    }

    /// What `CBasePlayer::Spawn` and `Precache` reset of the hints, the flashlight and the Geiger
    /// counter (`player.cpp` 5851, 5866, 5967, 5973-5978, 6165-6166, read 2026-11-22), `now` being
    /// `gpGlobals->time`: the hint queue starts over and the spawn forgets the round's hints
    /// (`DHM_ROUND_CLEAR`); the next spray may come at once; the Geiger counter waits two seconds
    /// and starts at range 1000; the flashlight's battery is 99 with its clock due (the first
    /// `UpdateClientData` charges it to 100 and tells the client: `FlashBat`).
    pub(crate) fn on_spawn_hud_extras(&mut self, now: f32) {
        self.hint_queue.reset();
        self.display_history &= !crate::hint::dhf::ROUND_CLEAR;
        self.next_decal_time = 0.0;
        self.geiger_delay = now + 2.0;
        self.flash_battery = 99;
        self.flash_light_time = 1.0;
        self.geiger_range = 1000.0;
        self.geiger_range_prev = 1000;
    }

    /// `CBasePlayer::Blind(duration, holdTime, fadeTime, alpha)` (`player.cpp` 8257, read
    /// 2026-11-22): remembers the flash that hit the player, `now` being `gpGlobals->time`. (The
    /// `m_RadiusFlash` hook chain of `REGAMEDLL_API` has no default but this.)
    pub(crate) fn blind(&mut self, now: f32, duration: f32, hold_time: f32, fade_time: f32, alpha: i32) {
        self.blind_until_time = now + duration;
        self.blind_start_time = now;
        self.blind_hold_time = hold_time;
        self.blind_fade_time = fade_time;
        self.blind_alpha = alpha;
    }

    /// The reset of `Spawn` (`player.cpp` 5894-5898): no flash is left on the player.
    fn clear_blindness(&mut self) {
        self.blind_until_time = 0.0;
        self.blind_start_time = 0.0;
        self.blind_hold_time = 0.0;
        self.blind_fade_time = 0.0;
        self.blind_alpha = 0;
    }

    /// `m_bHasNightVision`. For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn test_has_night_vision(&self) -> bool {
        self.has_night_vision
    }

    /// `m_bNightVisionOn`. For tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn test_night_vision_on(&self) -> bool {
        self.night_vision_on
    }

    /// `m_signals.GetState()`: the zones the player was in at the last `HandleSignals`. For
    /// tests only (cargo feature `test-hooks`).
    #[cfg(feature = "test-hooks")]
    #[doc(hidden)]
    pub fn test_zone_signals(&self) -> u8 {
        self.signals.state()
    }

    /// Whether the player has joined the game with a team that plays: the stand-in for
    /// `m_iJoiningState == JOINED` with a team that spawns.
    pub(crate) fn in_play(&self) -> bool {
        self.joining == JoinState::Joined && self.team.plays()
    }

    /// What `CBasePlayer::Spawn` and `Precache` reset about what the client knows, so the next
    /// client data update sends it all again (`m_fInitHUD`, `m_iClientBattery`,
    /// `m_bitsHUDDamage`, `m_iClientFOV`, `m_iFOV`, `m_iClientHideHUD`, `m_iTrain`).
    fn reset_hud_state(&mut self, init_hud_global: bool) {
        // `Precache` (player.cpp:6178, called by `Spawn` first): `if (gInitHUD) m_fInitHUD = TRUE`.
        // `Spawn` then sets it whatever `gInitHUD` is (6030), so it matters to nobody that
        // spawns; kept as the source has it.
        if init_hud_global {
            self.init_hud = true;
        }
        self.init_hud = true;
        // `m_iClientHealth` is not part of it: `Spawn` leaves it, so a respawn that finds the
        // client already knows the health (a joiner's 100) does not send `Health` again.
        self.client_battery = -1;
        self.hud_damage = -1;
        self.damage_bits = 0;
        self.dmg_take = 0.0;
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

/// The `pPlayer->Spawn()` that `ClientPutInServer` runs for a player that has only connected
/// (`m_bJustConnected`, `m_bNotKilled` true; ReGameDLL `player.cpp` `Spawn`, read 2026-11-22),
/// what survives of it once `ClientPutInServer` has overwritten the player's state:
/// - `GetPlayerSpawnSpot` for a player on no team (`EntSelectSpawnPoint`: the first
///   `info_player_deathmatch`, which becomes `g_pLastCTSpawn`), which puts the player there;
/// - `m_fInitHUD` (a new [`Player`] has it set) and the `m_iHideHUD` clears (nothing to clear);
/// - the messages, in the order of `Spawn`: `SVC_ROOMTYPE`, `NVGToggle 0`, then (after
///   `PlayerSpawn`, which returns at once for `m_bJustConnected`: no suit bit, no equipment, no
///   model) `SetProgressBarTime(0)` (`BarTime 0`), `SetScoreboardAttributes`, `TeamInfo` and
///   `ScoreInfo` to everyone (team `UNASSIGNED`), `UTIL_ScreenFade(0.001)` and `SyncRoundTimer`.
///   (No `Flashlight` message: `Spawn` only sets `m_iFlashBattery = 99`.) `UpdateLocation` sends nothing (no bots, the player is `DEAD_NO` only
///   for the length of `Spawn`).
fn spawn_on_connect(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    set_player_classname(eng, slot);
    crate::world::get_player_spawn_spot(g, eng, slot);
    // `Spawn` (player.cpp:5849, read 2026-11-22): `SET_PHYSICS_KEY_VALUE(edict(), "slj", "0")` and
    // `"hl", "1"`: the client's physics info, which goes to it with its client data.
    set_physics_keys(eng, slot);
    // `Spawn` (player.cpp:6036-6058, read 2026-11-22): `ClientPutInServer` sets `m_bNotKilled`
    // before it, so the player keeps what it has and `m_rgAmmoLast[i] = -1` makes the first
    // `SendAmmoUpdate` tell the client every ammo count, zeros included (32 `AmmoX`).
    let init_global = g.init_hud_global;
    let now = eng.time() as f32;
    if let Some(p) = g.players[usize::from(slot)].as_mut() {
        p.on_spawn_hud_extras(now);
        p.ammo_last = [-1; crate::weapons::MAX_AMMO_SLOTS];
        // `Precache` and `Spawn` (player.cpp:6178, 6030): `m_fInitHUD = TRUE`.
        if init_global {
            p.init_hud = true;
        }
        p.init_hud = true;
    }
    eng.set_view(eng.client_edict(slot), eng.client_edict(slot));
    send_room_type(eng, slot);
    crate::hud_misc::on_spawn(g, eng, slot);
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::BarTime(0));
    messages::set_scoreboard_attributes_alive(g, eng, usize::from(slot));
    let id = eng.client_edict(slot);
    if let Some(p) = g.players[usize::from(slot)].as_ref() {
        eng.send_usermsg(
            MsgDest::All,
            &CsUserMsg::TeamInfo {
                player_id: id.index as u8,
                team_name: teams::team_name(p.team).to_string(),
            },
        );
        messages::score_info(eng, id.index, p);
    }
    observer::screen_fade(eng, slot, 0.001, 0.0, 0, observer::FFADE_IN);
    messages::sync_round_timer(g, eng, slot);
}

/// `pev->classname = MAKE_STRING("player")`, the first statement of `CBasePlayer::Spawn`
/// (player.cpp, read 2026-11-22): `FindEntityByClassname`/`FindEntityInSphere` loops (grenades,
/// flashes, spawn-point occupants) find the player by it.
fn set_player_classname(eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    if let Some(v) = eng.ent_mut(id) {
        v.classname = "player".to_string();
    }
}

/// The physics keys of `Spawn` (`slj` off, `hl` on).
fn set_physics_keys(eng: &mut EngineApi<'_>, slot: u8) {
    eng.set_physics_key(slot, "slj", "0");
    eng.set_physics_key(slot, "hl", "1");
}

/// `SVC_ROOMTYPE` with the `room_type` cvar (`Spawn`).
fn send_room_type(eng: &mut EngineApi<'_>, slot: u8) {
    let room_type = eng.cvar_f32("room_type") as i32 as i16 as u16;
    let [lo, hi] = room_type.to_le_bytes();
    eng.send(MsgDest::One(slot), &[svc::ROOMTYPE, lo, hi]);
}

/// `ClientPutInServer` of a multiplayer game (ReGameDLL `client.cpp`, read 2026-11-22): the
/// player enters with no team, invisible, not solid and taking no damage, until it is spawned.
/// With a `trigger_camera` that has a target it stands at the camera, looking at the target, with
/// no view offset ([`intro::join_view`]); else it stands at the counter-terrorists' spawn point
/// (the `GetPlayerSpawnSpot` of the original's no-camera path) with `v_angle` zero and the
/// spot's angles, and keeps the view offset `VEC_VIEW`. It begins with the `Spawn()` of the
/// original ([`spawn_on_connect`]).
///
/// Known gaps: the original's no-camera path ends with `pev->angles = gpGlobals->v_forward`, a
/// stale global (the last `MakeVectors`, a direction vector used as angles), which is not
/// reproduced; the spot's angles stand. Like the original it keeps the player as a
/// `MOVETYPE_NOCLIP` spectator with `models/player.mdl` while it looks at the menus (the golden
/// capture of HLDS: `spectator`, `movetype 8` and the model index in its entity state).
pub(crate) fn put_in_server(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let index = usize::from(slot);
    if index >= MAX_PLAYERS {
        return;
    }
    // `CheckStartMoney`: `mp_startmoney` clamped to the account's limits.
    let mut player = Player::new(money::check_start_money(eng));
    player.vgui_menus = prefers_vgui_menus(eng, slot);
    player.show_hints = crate::hint::prefers_hints(eng, slot);
    player.chat = crate::chat::ChatState::entered(eng.client_key(slot, "name").unwrap_or(""));
    g.join_counter += 1;
    player.join_order = g.join_counter;
    // `m_pIntroCamera = UTIL_FindEntityByClassname(nullptr, "trigger_camera")`; with a target to
    // look at the player takes the camera's view for six seconds ([`intro`]); with no camera, or
    // none with a target (the vanilla `else`), it stands at the CT spawn point.
    let (camera, view) = intro::join_view(eng);
    player.intro_camera = camera;
    let time = eng.time();
    let intro_view = view.is_some();
    if intro_view {
        player.intro_cam_time = time + intro::INTRO_CAM_INTERVAL;
    }
    g.players[index] = Some(player);
    let id = eng.client_edict(slot);
    spawn_on_connect(g, eng, slot);
    let (origin, angles, view_ofs) = match view {
        Some((origin, angles)) => (origin, angles, Vec3::ZERO),
        None => {
            let (origin, angles) =
                match select_join_spawn_point(g, eng, id, SpawnTeam::CounterTerrorist) {
                    Some(spot) => {
                        let v = eng.ent(spot);
                        (
                            v.map_or(Vec3::ZERO, |v| v.origin) + Vec3::new(0.0, 0.0, 1.0),
                            v.map_or(Vec3::ZERO, |v| v.angles),
                        )
                    }
                    None => (Vec3::new(0.0, 0.0, 1.0), Vec3::ZERO),
                };
            (origin, angles, Vec3::new(0.0, 0.0, VEC_VIEW))
        }
    };
    let Some(v) = eng.ent_mut(id) else {
        return;
    };
    v.origin = origin;
    // KNOWN GAP (accepted): the golden capture of HLDS shows zero `angles` for the pre-join
    // player in its first entity states, though the source sets the camera's angles here (as
    // this does); what zeroes them there was not found. Ours show the camera's angles.
    v.angles = angles;
    // The camera's `v_angle` is its angles; the spawn point's path leaves it zero.
    v.v_angle = if intro_view { angles } else { Vec3::ZERO };
    v.flags = FL_CLIENT | FL_SPECTATOR;
    v.solid = SOLID_NOT;
    v.movetype = MOVETYPE_NOCLIP;
    v.effects = EF_NODRAW | EF_NOINTERP;
    // `ClientPutInServer` runs `Spawn()` first (client.cpp 637), which ends with `pev->friction`
    // and `pev->gravity` set to 1 (player.cpp 5846): the golden capture of HLDS has both in
    // the first entity state of a player that has not picked a team.
    v.friction = 1.0;
    v.gravity = 1.0;
    v.deadflag = DEAD_DEAD;
    v.health = 100.0;
    v.velocity = Vec3::ZERO;
    v.punchangle = Vec3::ZERO;
    v.fixangle = 1;
    // `SET_CLIENT_MAXSPEED(ENT(pev), 1)`. No `fov`: `Spawn` leaves `pev->fov` as the engine
    // cleared it, the first `UpdateClientData` sets it with `SetFOV`.
    v.max_speed = 1.0;
    v.view_ofs = view_ofs;
    eng.set_origin(id, origin);
    // `SET_MODEL(ENT(pev), "models/player.mdl")`: the player has a model, though it is not drawn.
    eng.set_model(id, "models/player.mdl");
    // The end of `ClientPutInServer` (client.cpp:735-749, read 2026-11-22): everyone is told the
    // player connected, `%` of the name made a space (the text is a format string).
    let name = eng
        .client_key(slot, "name")
        .unwrap_or("")
        .replace('%', " ");
    let name = if name.is_empty() { "<unconnected>" } else { name.as_str() };
    menus::print_all_notify(eng, "#Game_connected", name);
}

/// `SetPrefsFromUserinfo`: whether the client wants VGUI menus (`_vgui_menus`, on when the
/// key is missing or empty).
fn prefers_vgui_menus(eng: &EngineApi<'_>, slot: u8) -> bool {
    match eng.client_key(slot, "_vgui_menus") {
        Some(v) if !v.is_empty() => menus::atoi(v) != 0,
        _ => true,
    }
}

/// `ClientUserInfoChanged` (ReGameDLL `client.cpp:3746`, `CHalfLifeMultiplay::ClientUserInfoChanged`,
/// `multiplay_gamerules.cpp:5087`, read 2026-11-22): `SetPlayerModel` forces the `model` key back
/// to the one the player's team and appearance give (`SetClientUserInfoModel` writes it only when
/// it differs), then `SetPrefsFromUserinfo` reads the menu preference. A player on no team (or a
/// spectator) is `urban`. The name part is [`crate::chat::name_changed`].
pub(crate) fn userinfo_changed(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let vgui = prefers_vgui_menus(eng, slot);
    let hints = crate::hint::prefers_hints(eng, slot);
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    p.vgui_menus = vgui;
    p.show_hints = hints;
    let model = model_name(p.team, p.class);
    if eng.client_key(slot, "model") != Some(model) {
        eng.set_client_key(slot, "model", model);
    }
}

/// `ClientDisconnect` (ReGameDLL `client.cpp:316`, read 2026-11-22). Unless the match is over
/// (`IsGameOver()`: then nothing is announced and the rules are not told), everyone is told
/// `#Game_disconnected`, the player stops taking damage and being solid and is made `FL_DORMANT`
/// (so the player counts of the rules skip it), and `CHalfLifeMultiplay::ClientDisconnected`
/// ([`rules::on_disconnect`]) runs, ending with `CheckWinConditions`. The slot's player is gone
/// afterwards in either case, and its edict inert.
pub(crate) fn disconnect(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if g.players.get(usize::from(slot)).is_none_or(Option::is_none) {
        return;
    }
    let id = eng.client_edict(slot);
    let announce = !g.rules.game_over();
    if announce {
        let name = teams::player_name(eng, slot);
        menus::print_all_notify(eng, "#Game_disconnected", &name);
        if let Some(p) = g.players[usize::from(slot)].as_mut() {
            p.takes_damage = false;
        }
    }
    if let Some(v) = eng.ent_mut(id) {
        v.solid = SOLID_NOT;
        v.flags = FL_DORMANT;
        let origin = v.origin;
        eng.set_origin(id, origin);
    }
    if announce {
        rules::on_disconnect(g, eng, slot);
    } else {
        g.players[usize::from(slot)] = None;
    }
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
    set_player_classname(eng, slot);
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
    let freeze = g.rules.is_freeze_period();
    let Some(p) = g.players[index].as_mut() else {
        return false;
    };
    p.team = team;
    p.class = class;
    p.health = 100.0;
    set_physics_keys(eng, slot);
    // A player killed before starts with nothing (`Spawn`: armor, kevlar and the goggles).
    let lost_items = !p.not_killed;
    if lost_items {
        p.armor = 0.0;
        p.kevlar = ArmorType::None;
        p.has_night_vision = false;
        // `Spawn` (player.cpp:6030-6039) starts the client's cached `m_iClientHideHUD` over
        // only for a player killed before (or under `REGAMEDLL_FIXES`, not vanilla), so a
        // joiner's first update sends no `HideWeapon`.
        p.client_hide_hud = -1;
        // `Spawn` (player.cpp 6036-6048): nothing is left of the ammo or the primary weapon.
        p.ammo = [0; crate::weapons::MAX_AMMO_SLOTS];
        p.has_primary = false;
    } else {
        // A survivor keeps its ammo, but the client is told all of it again.
        p.ammo_last = [-1; crate::weapons::MAX_AMMO_SLOTS];
    }
    p.hide_hud &=
        !(HIDEHUD_WEAPONS | HIDEHUD_HEALTH | HIDEHUD_TIMER | HIDEHUD_MONEY | HIDEHUD_CROSSHAIR);
    p.alive = true;
    p.takes_damage = true;
    p.not_killed = true;
    p.just_killed_teammate = false;
    p.receives_no_money_next_round = false;
    p.fall_velocity = 0.0;
    // `Spawn`: no drowning damage to give back.
    p.damage_bits &= !(dmg::DROWN | dmg::DROWNRECOVER);
    p.time_based_damage[ITBD_DROWN_RECOVER] = 0;
    p.drown_dmg = 0;
    p.drown_restored = 0;
    // `Spawn`: `m_flVelocityModifier = 1`.
    p.velocity_modifier = 1.0;
    p.last_hit_group = HITGROUP_GENERIC;
    p.high_damage = false;
    p.clear_blindness();
    p.on_spawn_hud_extras(time as f32);
    observer::on_spawn(p);
    // `m_tmHandleSignals = 0`: the zones are looked at again with the first client data update.
    p.next_handle_signals = 0.0;
    // `Spawn` (player.cpp 5838, 5863, 5870, read 2026-11-23): no bomb, no bomb site, not escaped.
    p.cur_bomb_target = None;
    p.has_c4 = false;
    p.escaped = false;
    p.chat.on_spawn();
    // `PlayerSpawn`: `pev->weapons |= (1 << WEAPON_SUIT)`.
    p.weapons |= 1 << WEAPON_SUIT;
    p.reset_hud_state(g.init_hud_global);
    // `Spawn` (player.cpp 6026-6028, read 2026-11-22): the weapon HUD starts over.
    p.last_item = None;
    p.f_weapon = false;
    p.client_active_item = None;
    // `Spawn` (player.cpp 5937): no shooting during the freeze period.
    p.can_shoot = !freeze;
    // `Spawn` (player.cpp 5972): `m_flNextAttack = UTIL_WeaponTimeBase()`, 0.
    p.next_attack = 0.0;
    // `Spawn` (player.cpp 5841-5843): a player without a shield has `HITGROUP_SHIELD_DISABLED`
    // (1) in `gamestate`, so the engine's studio traces skip the shield's hit box.
    let shield_state = if p.owns_shield { HITGROUP_SHIELD_ENABLED } else { HITGROUP_SHIELD_DISABLED };

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
        // Vanilla `CBasePlayer::Spawn` (ReGameDLL `player.cpp` 5827-5831, read 2026-11-22) resets
        // `watertype`, `waterlevel` and `basevelocity` only under `REGAMEDLL_FIXES`, which the
        // target is not: they keep what the last movement left until the next `pm` run
        // recategorizes the position.
        v.friction = 1.0;
        v.gravity = 1.0;
        v.fov = f32::from(weapon_core::DEFAULT_FOV);
        v.gamestate = shield_state;
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
        // `ResetStamina`: the jump stamina and the other `pm_shared` user floats.
        v.fuser1 = 0.0;
        v.fuser2 = 0.0;
        v.fuser3 = 0.0;
    }
    eng.set_size(id, HULL_MIN, HULL_MAX);
    eng.set_origin(id, origin);
    // `Spawn`: the gait starts over and the sequence is an idle one.
    player_anim::on_spawn(g, eng, slot);
    reset_max_speed(g, eng, slot);
    // The order of the messages is that of `Spawn` (player.cpp, read 2026-11-22): the
    // `SVC_ROOMTYPE` (`room_type`, 5937), `SendItemStatus` of a player that lost its items
    // (6046-6052), `NVGToggle 0` to the player and its first-person watchers (6064-6081,
    // `hud_misc::on_spawn`), then `PlayerSpawn` (6085), whose `RemoveAllItems` sends the empty
    // `CurWeapon` (no weapons exist yet), `SetScoreboardAttributes` (6104), `TeamInfo` to all
    // (6106), the scoreboard line (`ScoreInfo`, 6112-6118), the name asked for while dead, the
    // `UTIL_ScreenFade(this, Vector(0, 0, 0), 0.001)` that clears a fade left from the death
    // (6132), and `SyncRoundTimer` (6133). (`UpdateLocation` is not ported.) The first client
    // data update (`m_fInitHUD`) sends the round timer again, as `UpdateClientData` does.
    // `SET_VIEW(ENT(pev), ENT(pev))` (5882): the view is the player's own again.
    eng.set_view(id, id);
    send_room_type(eng, slot);
    if lost_items {
        crate::buy::send_item_status(eng, slot, false);
    }
    crate::hud_misc::on_spawn(g, eng, slot);
    // `PlayerSpawn` -> `OnSpawnEquip`: a player that lost its items gets the default ones; the
    // `RemoveAllItems` in it sends the HUD update and the empty `CurWeapon`.
    crate::inventory::on_spawn_equip(g, eng, slot, lost_items);
    // `m_bNotKilled = true; m_bIsDefusing = false;` and "get rid of the progress bar":
    // `SetProgressBarTime(0)` (player.cpp:6085-6092, read 2026-11-22), before `ResetMaxSpeed`.
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::BarTime(0));
    reset_max_speed(g, eng, slot);
    if let Some(p) = g.players[index].as_mut() {
        p.num_spawns += 1;
    }
    messages::set_scoreboard_attributes(g, eng, index);
    if let Some(p) = g.players[index].as_ref() {
        eng.send_usermsg(
            MsgDest::All,
            &CsUserMsg::TeamInfo {
                player_id: id.index as u8,
                team_name: teams::team_name(p.team).to_string(),
            },
        );
        messages::score_info(eng, id.index, p);
    }
    // A name asked for while dead is taken at the spawn.
    crate::chat::apply_pending_name(g, eng, slot);
    // `UTIL_ScreenFade(this, Vector(0, 0, 0), 0.001)`: fade in, alpha 0, no hold.
    observer::screen_fade(eng, slot, 0.001, 0.0, 0, observer::FFADE_IN);
    messages::sync_round_timer(g, eng, slot);
    true
}

/// `CBasePlayer::ResetMaxSpeed`: 900 for an observer, 1 while the round is in its freeze period
/// (nobody moves), else the active item's `GetMaxSpeed()`, or [`NO_ITEM_MAX_SPEED`] with none.
/// (The VIP's 227 is not ported.)
pub(crate) fn reset_max_speed(g: &GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let item = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .and_then(Player::active_max_speed);
    apply_max_speed(g, eng, slot, item);
}

/// [`reset_max_speed`] with the active item's speed given (`None` for no active item), for a
/// weapon that is checked out of the player's list while it runs.
pub(crate) fn apply_max_speed(g: &GameState, eng: &mut EngineApi<'_>, slot: u8, item: Option<f32>) {
    let id = eng.client_edict(slot);
    let freeze = g.rules.is_freeze_period();
    if let Some(v) = eng.ent_mut(id) {
        v.max_speed = if v.iuser1 != 0 {
            OBSERVER_MAX_SPEED
        } else if freeze {
            FROZEN_MAX_SPEED
        } else {
            item.unwrap_or(NO_ITEM_MAX_SPEED)
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

/// The command with every number the movement reads made finite (the engine refuses a
/// non-finite command before the game sees it; a game called directly must not trust that).
fn finite_cmd(cmd: &GoldSrcUserCmd) -> GoldSrcUserCmd {
    let f = |x: f32| if x.is_finite() { x } else { 0.0 };
    GoldSrcUserCmd {
        viewangles: Vec3::new(f(cmd.viewangles.x), f(cmd.viewangles.y), f(cmd.viewangles.z)),
        forwardmove: f(cmd.forwardmove),
        sidemove: f(cmd.sidemove),
        upmove: f(cmd.upmove),
        impact_position: Vec3::new(
            f(cmd.impact_position.x),
            f(cmd.impact_position.y),
            f(cmd.impact_position.z),
        ),
        ..*cmd
    }
}

/// Runs one command of the player of `slot` (the game's part of ReHLDS `SV_RunCmd`, read
/// 2026-11-22, around `PM_Move`): `CmdStart`, the view angles, `PlayerPreThink`, the movement
/// ([`pmove::run_pmove`]); the engine then relinks the player and touches what it met, and
/// [`post_usercmd`] runs `PlayerPostThink` and `CmdEnd`. A command of 0 ms runs all of it, with a
/// frame time of 0. A player that has not been spawned does not move; a dead one is run by
/// `PlayerDeathThink` (which `PreThink` calls before the movement), then `PM_Physics_Toss` moves
/// the body.
pub(crate) fn run_usercmd(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    cmd: &GoldSrcUserCmd,
    random_seed: u32,
) {
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    let cmd = finite_cmd(cmd);
    // `CmdStart`: `m_random_seed = random_seed`.
    p.random_seed = random_seed;
    // `SV_RunCmd`: the old angles are the view angles before the command's, which the game
    // module can take over with `fixangle`.
    let id = eng.client_edict(slot);
    if let Some(v) = eng.ent_mut(id) {
        p.oldangles = v.v_angle;
        if v.fixangle == 0 {
            v.v_angle = cmd.viewangles;
        }
    }
    run_cmd_body(g, eng, slot, &cmd);
}

/// The rest of `SV_RunCmd` after the engine's `SV_ProcessImpacts`: `PlayerPostThink` (the fall
/// damage; not for the dead, observers and players not in play, nor once the match is over), then
/// `CmdEnd`.
pub(crate) fn post_usercmd(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    let observing = p.pflag_observer && observer::mode_of(eng, slot) != observer::OBS_NONE;
    if !observing && p.in_play() && p.alive && !g.rules.game_over() {
        post_think(g, eng, slot);
    }
    // `pt_end`: `PostThink` jumps here when the match is over or the player is dead.
    weapon_core::decay_timers(g, eng, slot);
    // `m_iGaitsequence = pev->gaitsequence; StudioProcessGait();` close `pt_end`.
    player_anim::studio_process_gait(g, eng, slot);
    pmove::cmd_end(eng, slot);
}

/// What `SV_RunCmd` runs between `CmdStart` and `CmdEnd`.
fn run_cmd_body(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, cmd: &GoldSrcUserCmd) {
    // `PlayerPreThink`: the way into the game advances with the player's commands (not once the
    // match is over: `PreThink` returns after `PlayerThink` while `IsGameOver()`, so there is no
    // `JoiningThink`, and `PostThink` skips the fall damage).
    let game_over = g.rules.game_over();
    // `m_hintMessageQueue.Update(this)`: "Hint messages should be updated even if the game is
    // over".
    crate::hint::update_queue(g, eng, slot);
    teams::pre_think(g, eng, slot);
    rules::player_think(g, slot, cmd.buttons);
    weapon_player_think(g, eng, slot);
    let id = eng.client_edict(slot);
    if !game_over {
        // `ItemPreFrame; WaterMove` come before the velocity modifier.
        weapon_core::player_item_pre_frame(g, eng, slot);
        water_move(g, eng, slot);
        let on_ground = eng.ent(id).is_some_and(|v| v.flags & FL_ONGROUND != 0);
        let Some(p) = g
            .players
            .get_mut(usize::from(slot))
            .and_then(Option::as_mut)
        else {
            return;
        };
        // `PreThink` (player.cpp:4774-4786): on the ground the velocity modifier slows the
        // player down and climbs back to 1. `real_t modvel = m_flVelocityModifier + 0.01` adds
        // a double and stores a float.
        if on_ground {
            if p.velocity_modifier < 1.0 {
                let modvel = (f64::from(p.velocity_modifier) + 0.01) as f32;
                p.velocity_modifier = modvel;
                if let Some(v) = eng.ent_mut(id) {
                    v.velocity *= modvel;
                }
            }
            if p.velocity_modifier > 1.0 {
                p.velocity_modifier = 1.0;
            }
        }
        // `PreThink` (player.cpp:4825, read 2026-11-22): the flashlight is hidden from the HUD
        // unless `mp_flashlight` allows it (`FAllowFlashlight`), before `UpdateClientData`. (The
        // idle check before it, `DropIdlePlayer`, is not ported.)
        let allow = eng.cvar_f32("mp_flashlight") != 0.0;
        if allow {
            p.hide_hud &= !HIDEHUD_FLASHLIGHT;
        } else {
            p.hide_hud |= HIDEHUD_FLASHLIGHT;
        }
        // `UpdateClientData` (player.cpp:4831, read 2026-11-22): "JOHN: checks if new client data
        // (for HUD and view control) needs to be sent to the client", on every command, before
        // that command's movement and touches, and not once the match is over (`PreThink` has
        // returned). Not the engine's `UpdateClientData` export ([`client_data`]), which only
        // fills the structure of the update.
        messages::update_client_data(g, eng, slot);
        // The time-based damage check follows it.
        check_time_based_damage(g, eng, slot);
        // `CheckSuitUpdate`: the Geiger counter.
        crate::suit::check_suit_update(g, eng, slot);
    }
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    // An observer (the dead after their camera, the spectators) does not walk: it flies, or
    // follows a player (`PlayerPreThink` runs `Observer_Think` and returns).
    if p.pflag_observer && observer::mode_of(eng, slot) != observer::OBS_NONE {
        observer::run_cmd(g, eng, slot);
        // `PM_Move` takes the spectator branch (`PM_SpectatorMove`).
        pmove::run_pmove(g, eng, slot, cmd);
        return;
    }
    if !p.in_play() {
        return;
    }
    let alive = p.alive;
    // `if (pev->deadflag >= DEAD_DYING && pev->deadflag != DEAD_RESPAWNABLE)` (player.cpp:4853).
    let deadflag = eng.ent(id).map_or(DEAD_NO, |v| v.deadflag);
    if deadflag >= DEAD_DYING && deadflag != DEAD_RESPAWNABLE {
        // `PreThink` calls `PlayerDeathThink` before the movement and returns; the dead body
        // then goes through `PM_Move` (`MOVETYPE_TOSS`, or `MOVETYPE_NONE` once it has landed).
        death_think(g, eng, slot);
        // When the death camera has just started, the player is an observer now and the
        // movement below is `PM_SpectatorMove` (`PM_Move` follows `PreThink` in `SV_RunCmd`).
    }
    if alive {
        // `PreThink` (player.cpp:4984-4994): "if (pev->button & IN_JUMP) Jump();" and, trying to
        // duck, already ducked or ducking, `Duck()`. (`Duck` does something only for IN_DUCK.)
        if eng.ent(id).is_some_and(|v| v.button & IN_JUMP != 0) {
            player_anim::jump(g, eng, slot);
        }
        player_anim::duck(g, eng, slot);
    }
    if alive
        && let Some(v) = eng.ent(id)
        && v.flags & FL_ONGROUND == 0
    {
        // `CBasePlayer::PreThink`: the fall speed of the last command.
        let fall = -v.velocity.z;
        if let Some(p) = g
            .players
            .get_mut(usize::from(slot))
            .and_then(Option::as_mut)
        {
            p.fall_velocity = fall;
        }
    }
    // `PreThink` (player.cpp:5011, read 2026-11-22): the first time in a round the player may buy,
    // it is told how.
    if alive
        && g.players[usize::from(slot)]
            .as_ref()
            .is_some_and(|p| p.display_history & crate::hint::dhf::ROUND_STARTED == 0)
        && crate::buy::can_player_buy(g, eng, slot, false)
    {
        crate::hint::hint(g, eng, slot, "#Hint_press_buy_to_purchase");
        crate::hint::first_time(g, slot, crate::hint::dhf::ROUND_STARTED);
    }
    pmove::run_pmove(g, eng, slot, cmd);
}

/// `CBasePlayer::WaterMove` (ReGameDLL `player.cpp`, read 2026-11-22, vanilla): breath, drowning
/// damage, the drowning recovery flag, the swim sounds, and lava and slime damage; run from
/// `PreThink` with the `waterlevel` the previous command's movement left. QUIRKS kept: the dead
/// test is `health < 0` (a player at 0 health goes on); the "up for air" gasp plays whenever the
/// player is not fully under water, whether it was in water or not (vanilla has no `FL_INWATER`
/// test there); `pev->dmg` starts at 2 and grows by 1 per pain up to 5.
fn water_move(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    let time = eng.time() as f32;
    let Some(v) = eng.ent(id) else {
        return;
    };
    let (movetype, health, waterlevel, watertype, flags) =
        (v.movetype, v.health, v.waterlevel, v.watertype, v.flags);
    if movetype == MOVETYPE_NOCLIP || movetype == MOVETYPE_NONE || health < 0.0 {
        return;
    }
    let (mut air_finished, mut pain_finished, mut pev_dmg, dmgtime) =
        (v.air_finished, v.pain_finished, v.dmg, v.dmgtime);
    if waterlevel != 3 {
        // Not underwater: play 'up for air' sound.
        if air_finished < time {
            eng.emit_sound(id, CHAN_VOICE, "player/pl_wade1.wav", 1.0, ATTN_NORM, PITCH_NORM);
        } else if air_finished < time + 9.0 {
            eng.emit_sound(id, CHAN_VOICE, "player/pl_wade2.wav", 1.0, ATTN_NORM, PITCH_NORM);
        }
        air_finished = time + AIRTIME as f32;
        pev_dmg = 2.0;
        // If we took drowning damage, give it back slowly.
        if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut)
            && p.drown_dmg > p.drown_restored
        {
            p.time_based_damage[ITBD_DROWN_RECOVER] = 0;
            p.damage_bits |= dmg::DROWNRECOVER;
            p.damage_bits &= !dmg::DROWN;
        }
    } else {
        // Fully under water: stop restoring damage while underwater.
        if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
            p.time_based_damage[ITBD_DROWN_RECOVER] = 0;
            p.damage_bits &= !dmg::DROWNRECOVER;
        }
        // Drown!
        if time > air_finished {
            if time > pain_finished {
                // Take drowning damage.
                pev_dmg += 1.0;
                if pev_dmg > 5.0 {
                    pev_dmg = 5.0;
                }
                take_damage(g, eng, slot, None, None, pev_dmg, dmg::DROWN);
                pain_finished = time + 1.0;
                // Track drowning damage, give it back when the player finally takes a breath.
                if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
                    p.drown_dmg += pev_dmg as i32;
                }
            }
        } else if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
            p.damage_bits &= !(dmg::DROWNRECOVER | dmg::DROWN);
        }
    }
    if let Some(v) = eng.ent_mut(id) {
        v.air_finished = air_finished;
        v.pain_finished = pain_finished;
        v.dmg = pev_dmg;
    }

    if waterlevel == 0 {
        if let Some(v) = eng.ent_mut(id) {
            v.flags &= !FL_INWATER;
        }
        return;
    }

    // Make bubbles.
    let air = (air_finished - time) as i32;
    if g.random_range(0, 0x1f) == 0 && g.random_range(0, AIRTIME as i32 - 1) >= air {
        let sample = match g.random_range(0, 3) {
            0 => "player/pl_swim1.wav",
            1 => "player/pl_swim2.wav",
            2 => "player/pl_swim3.wav",
            _ => "player/pl_swim4.wav",
        };
        eng.emit_sound(id, CHAN_BODY, sample, 0.8, ATTN_NORM, PITCH_NORM);
    }

    if watertype == CONTENT_LAVA {
        // Do damage.
        if dmgtime < time {
            take_damage(g, eng, slot, None, None, f32::from(waterlevel as u8) * 10.0, dmg::BURN);
        }
    } else if watertype == CONTENT_SLIME {
        if let Some(v) = eng.ent_mut(id) {
            v.dmgtime = time + 1.0;
        }
        take_damage(g, eng, slot, None, None, f32::from(waterlevel as u8) * 4.0, dmg::ACID);
    }
    if flags & FL_INWATER == 0
        && let Some(v) = eng.ent_mut(id)
    {
        v.flags |= FL_INWATER;
        v.dmgtime = 0.0;
    }
}

/// `CBasePlayer::TakeHealth` (`CBaseMonster::TakeHealth` and `CBaseEntity::TakeHealth`,
/// basemonster.cpp and cbase.cpp, read 2026-11-22) up to `max_health`, which is 100 for a player
/// (assumption: the edict keeps none). Returns whether anything was given.
fn take_health(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, health: f32, bits: u32) -> bool {
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    if !p.takes_damage {
        return false;
    }
    // Clear out any damage types we healed.
    p.damage_bits &= !(bits & !dmg::TIMEBASED);
    if p.health >= 100.0 {
        return false;
    }
    p.health = (p.health + health).min(100.0);
    let value = p.health;
    let id = eng.client_edict(slot);
    if let Some(v) = eng.ent_mut(id) {
        v.health = value;
    }
    true
}

/// `CBasePlayer::CheckTimeBasedDamage` (player.cpp, read 2026-11-22): every two seconds the
/// counters of the time-based kinds run down. Of the kinds only the drowning recovery has an
/// effect so far (up to 10 health back per check, from `m_idrowndmg - m_idrownrestored`); poison
/// and the antidote wait for the items. QUIRK: the two-second test truncates the time
/// difference to whole seconds first.
fn check_time_based_damage(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let time = eng.time() as f32;
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    if p.damage_bits & dmg::TIMEBASED == 0 {
        return;
    }
    if ((time - p.tbd_prev) as i64).abs() < 2 {
        return;
    }
    p.tbd_prev = time;
    for i in 0..ITBD_END {
        let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
            return;
        };
        if p.damage_bits & (dmg::PARALYZE << i) == 0 {
            continue;
        }
        let duration: u8 = match i {
            ITBD_POISON => 5,
            // Get up to 5*10 = 50 points back.
            ITBD_DROWN_RECOVER => 4,
            _ => 2,
        };
        let mut restore = 0;
        if i == ITBD_DROWN_RECOVER && p.drown_dmg > p.drown_restored {
            // This hack is used to RESTORE health after the player has been drowning.
            restore = (p.drown_dmg - p.drown_restored).min(10);
            p.drown_restored += restore;
        }
        if p.time_based_damage[i] != 0 {
            // Decrement damage duration, detect when done.
            p.time_based_damage[i] -= 1;
            if p.time_based_damage[i] == 0 {
                p.damage_bits &= !(dmg::PARALYZE << i);
            }
        } else {
            // First time taking this damage type: init damage duration.
            p.time_based_damage[i] = duration;
        }
        if restore > 0 {
            take_health(g, eng, slot, restore as f32, dmg::GENERIC);
        }
    }
}

/// What `CHalfLifeMultiplay::PlayerThink` does to the weapons' state, right after the round rules
/// have noted the buttons (multiplay_gamerules.cpp 3722-3755, read 2026-11-22): once the match
/// is over the player's buttons are cleared (the pressed ones were noted for the intermission
/// first), `m_bCanShoot` comes back when the freeze period is over, and a weapon with its
/// shield drawn cannot shoot. (`NOFIREUNDERWATER` is `REGAMEDLL_ADD`: not here.)
fn weapon_player_think(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let game_over = g.rules.game_over();
    let freeze = g.rules.is_freeze_period();
    let id = eng.client_edict(slot);
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    if game_over {
        // Clear attack/use commands from the player.
        p.buttons_pressed = 0;
        p.buttons_released = 0;
        if let Some(v) = eng.ent_mut(id) {
            v.button = 0;
        }
    }
    if !p.can_shoot && !freeze {
        p.can_shoot = true;
    }
    let shield_drawn = p.active_item.and_then(|a| p.find_item(a)).is_some_and(|(s, i)| {
        p.items[s][i].weapon.weapon_state & crate::weapons::WPNSTATE_SHIELD_DRAWN != 0
    });
    if shield_drawn {
        p.can_shoot = false;
    }
}

/// `VIEW_FIELD_NARROW` (`util.h`): "+-45 degrees, more narrow check used to set up ranged
/// attacks", the cosine the use key's target must beat. A `double` literal stored in a float.
const VIEW_FIELD_NARROW: f32 = 0.7;

/// `CBasePlayer::PlayerUse` (`player.cpp:4263`, read 2026-11-23, vanilla), what is ported: the
/// use key finds the usable entity (`FCAP_IMPULSE_USE`, `FCAP_CONTINUOUS_USE` or
/// `FCAP_ONOFF_USE`) among those within `MAX_PLAYER_USE_RADIUS` of the player's origin whose
/// direction from the eyes is the closest to where the view points (the cosine must beat
/// [`VIEW_FIELD_NARROW`]; the entity's position is its brush model origin `absmin + size * 0.5`,
/// `VecBModelOrigin`), through walls ("TODO: traceline here to prevent +USEing buttons through
/// walls"). If one is found, a press plays `common/wpn_select.wav` (volume 0.4); a held key on a
/// continuous entity or a press on an impulse or on-off one then uses it (`Use(this, this,
/// USE_SET, 1)`), and a release of the key on an on-off entity does so with value 0 ("This is an
/// 'off' use"). If none is found, a press plays `common/wpn_denyselect.wav` (`UseEmpty`).
///
/// Not ported (gaps): `DetachTank` and the train control (`func_tank` and `func_train` are not
/// ported, so `m_pTank` is always empty and `PFLAG_ONTRAIN` never set), `PFLAG_USING` (nothing
/// reads it), and the hostage branch of `useNewHostages` (`!TheNavAreaList.empty() &&
/// cv_hostage_ai_enable`: it needs the Condition Zero navigation areas, which the classic hostage
/// of this port does not have). QUIRK kept: `ImpulseCommands`, which calls this, runs from
/// `ItemPostFrame`, and that returns before it while `m_flNextAttack` is above 0, so the use key
/// is dead during a weapon's deploy or attack delay.
pub(crate) fn player_use(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    let (pressed, released) = (i32::from(p.buttons_pressed), i32::from(p.buttons_released));
    let Some(v) = eng.ent(id) else {
        return;
    };
    let button = v.button;
    // Was use pressed or released?
    if (button | pressed | released) & IN_USE == 0 {
        return;
    }
    let (origin, view_ofs, v_angle) = (v.origin, v.view_ofs, v.v_angle);
    // So we know which way we are facing.
    let (forward, _, _) = cs_sim::pm::math::angle_vectors(v_angle);
    let mut max_dot = VIEW_FIELD_NARROW;
    let mut closest = None;
    for object in eng.entities_in_sphere(origin, crate::world::MAX_PLAYER_USE_RADIUS) {
        let caps = crate::entity::object_caps_of(eng, object);
        if caps & (FCAP_IMPULSE_USE | FCAP_CONTINUOUS_USE | FCAP_ONOFF_USE) == 0 {
            continue;
        }
        let Some(o) = eng.ent(object) else {
            continue;
        };
        // `VecBModelOrigin(pObject->pev) - (pev->origin + pev->view_ofs)`, `NormalizeInPlace`.
        let los = bullets::normalize_reciprocal((o.absmin + o.size * 0.5) - (origin + view_ofs));
        let dot = los.dot(forward);
        // Only if the item is in front of the user.
        if dot > max_dot {
            max_dot = dot;
            closest = Some(object);
        }
    }
    let Some(object) = closest else {
        if pressed & IN_USE != 0 {
            // `UseEmpty`.
            eng.emit_sound(id, CHAN_ITEM, "common/wpn_denyselect.wav", 0.4, ATTN_NORM, PITCH_NORM);
        }
        return;
    };
    let caps = crate::entity::object_caps_of(eng, object);
    if pressed & IN_USE != 0 {
        eng.emit_sound(id, CHAN_ITEM, "common/wpn_select.wav", 0.4, ATTN_NORM, PITCH_NORM);
    }
    if (button & IN_USE != 0 && caps & FCAP_CONTINUOUS_USE != 0)
        || (pressed & IN_USE != 0 && caps & (FCAP_IMPULSE_USE | FCAP_ONOFF_USE) != 0)
    {
        crate::entity::use_entity(g, eng, object, Some(id), Some(id), UseType::Set, 1.0);
    } else if released & IN_USE != 0 && caps & FCAP_ONOFF_USE != 0 {
        // UNDONE (the source): send different USE codes for ON/OFF. "BUGBUG This is an 'off' use".
        crate::entity::use_entity(g, eng, object, Some(id), Some(id), UseType::Set, 0.0);
    }
}

/// `CBasePlayer::ImpulseCommands` (`player.cpp` 6816, read 2026-11-22), what is ported:
/// `PlayerUse` ([`player_use`], "Handle use events", first), then impulse 100 (the flashlight,
/// [`crate::suit::flashlight_toggle`]) and 201 (the spray, [`crate::spray::paint_decal`]);
/// `pev->impulse` is consumed (set to 0) afterwards, whatever it was. Not ported: impulse 99 (the
/// logo message, a development switch) and the cheat impulses (`CheatImpulseCommands` needs
/// `g_flWeaponCheat`, off on a server), so any other impulse is just consumed.
pub(crate) fn impulse_commands(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    // Handle use events.
    player_use(g, eng, slot);
    let id = eng.client_edict(slot);
    let impulse = eng.ent(id).map_or(0, |v| v.impulse);
    match impulse {
        // "temporary flashlight for level designers"
        100 => crate::suit::flashlight_toggle(g, eng, slot),
        // "paint decal"
        201 => crate::spray::paint_decal(g, eng, slot),
        _ => {}
    }
    if let Some(v) = eng.ent_mut(id) {
        v.impulse = 0;
    }
}

/// `CBasePlayer::PostThink` (`player.cpp` 5352-5480, read 2026-11-22), what is ported, for a
/// player that is alive in a match that is on: `ItemPostFrame`; a landing faster than
/// `MAX_PLAYER_SAFE_FALL_SPEED` out of water takes [`fall_damage`] from the world, and a hard
/// enough landing is followed by `SetAnimation(PLAYER_WALK)` for the living; the fall speed is
/// forgotten on the ground; the animation that fits the movement (`PLAYER_WALK` moving on the
/// ground or in deep water, else `PLAYER_IDLE` unless flying); `StudioFrameAdvance`. Not
/// ported: the tank (`m_pTank`), `CheckPowerups` (the model index is never changed) and
/// `UpdatePlayerSound` (useless in CS 1.6). The end of `PostThink` (`pt_end`) is
/// [`post_usercmd`].
fn post_think(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    // `PostThink` does the weapon stuff before the fall check (`ItemPostFrame`).
    weapon_core::player_item_post_frame(g, eng, slot);
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
    if flags & FL_ONGROUND != 0 && health > 0.0 && fall_velocity >= PLAYER_FALL_PUNCH_THRESHHOLD {
        if watertype != CONTENT_WATER && fall_velocity > MAX_PLAYER_SAFE_FALL_SPEED {
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
        if g.players[usize::from(slot)].as_ref().is_some_and(|p| p.alive) {
            player_anim::set_animation(g, eng, slot, PlayerAnim::Walk);
        }
    }
    if eng.ent(id).is_some_and(|v| v.flags & FL_ONGROUND != 0)
        && let Some(p) = g.players[usize::from(slot)].as_mut()
    {
        p.fall_velocity = 0.0;
    }
    // "Select the proper animation for the player character."
    if g.players[usize::from(slot)].as_ref().is_some_and(|p| p.alive)
        && let Some(v) = eng.ent(id)
    {
        let (moving, on_ground, waterlevel, gait) = (
            v.velocity.x != 0.0 || v.velocity.y != 0.0,
            v.flags & FL_ONGROUND != 0,
            v.waterlevel,
            i32::from(v.gaitsequence as i16),
        );
        if moving {
            if on_ground || waterlevel > 1 {
                player_anim::set_animation(g, eng, slot, PlayerAnim::Walk);
            }
        } else if gait != player_anim::ACT_FLY {
            player_anim::set_animation(g, eng, slot, PlayerAnim::Idle);
        }
    }
    player_anim::studio_frame_advance(g, eng, slot);
}

/// The id of the weapon the player of `slot` holds out (`m_pActiveItem->m_iId`), `None` when it
/// holds none (or the slot is empty).
pub(crate) fn active_weapon_id(g: &GameState, slot: usize) -> Option<WeaponId> {
    g.players.get(slot)?.as_ref()?.active_item
}

/// The armor ratio of an attacker holding `weapon` (the `switch (iGunType)` of
/// `CBasePlayer::TakeDamage`, ReGameDLL `player.cpp`, read 2026-11-22), from the base `ratio`
/// (which has the shield's 0.2 added already). The product is a `float` times a `double`
/// constant, stored back into a `float`.
pub fn weapon_armor_ratio(weapon: WeaponId, ratio: f32) -> f32 {
    use WeaponId as W;
    let factor = match weapon {
        W::Aug | W::M4a1 => 1.4,
        W::Awp => 1.95,
        W::G3sg1 => 1.65,
        W::Sg550 => 1.45,
        W::M249 => 1.5,
        W::Elite => 1.05,
        W::Deagle => 1.5,
        W::Glock18 => 1.05,
        W::FiveSeven | W::P90 => 1.5,
        W::Mac10 => 0.95,
        W::P228 => 1.25,
        W::Scout | W::Knife => 1.7,
        W::Famas | W::Sg552 => 1.4,
        W::Galil | W::Ak47 => 1.55,
        _ => return ratio,
    };
    (f64::from(ratio) * factor) as f32
}

/// `CBasePlayer::ShouldDoLargeFlinch` (`player.cpp:9372`, read 2026-11-22): a ducking player
/// never; a hit anywhere but the legs by one of the heavy rifles does.
fn should_do_large_flinch(ducking: bool, hit_group: i32, weapon: Option<WeaponId>) -> bool {
    use WeaponId as W;
    if ducking {
        return false;
    }
    if hit_group != HITGROUP_LEFTLEG && hit_group != HITGROUP_RIGHTLEG {
        return matches!(
            weapon,
            Some(
                W::Scout
                    | W::Aug
                    | W::Sg550
                    | W::Galil
                    | W::Famas
                    | W::Awp
                    | W::M3
                    | W::M4a1
                    | W::G3sg1
                    | W::Deagle
                    | W::Sg552
                    | W::Ak47
            )
        );
    }
    false
}

/// `CBasePlayer::Pain` (`player.cpp:543`, read 2026-11-22): the sound of a hit. `RANDOM_LONG(0, 2)`
/// is drawn first, whatever the sound; a helmet and the kevlar have their own sounds (the
/// kevlar one not for a hit on the legs).
fn pain(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, hit_group: i32, has_armor: bool) {
    let temp = g.random_range(0, 2);
    let id = eng.client_edict(slot);
    let kevlar = g.players[usize::from(slot)].as_ref().map(|p| p.kevlar);
    let sample = if hit_group == HITGROUP_HEAD {
        if kevlar == Some(ArmorType::VestHelm) {
            "player/bhit_helmet-1.wav"
        } else {
            match temp {
                0 => "player/headshot1.wav",
                1 => "player/headshot2.wav",
                _ => "player/headshot3.wav",
            }
        }
    } else if hit_group != HITGROUP_LEFTLEG && hit_group != HITGROUP_RIGHTLEG && has_armor {
        "player/bhit_kevlar-1.wav"
    } else {
        match temp {
            0 => "player/bhit_flesh-1.wav",
            1 => "player/bhit_flesh-2.wav",
            _ => "player/bhit_flesh-3.wav",
        }
    };
    eng.emit_sound(id, CHAN_VOICE, sample, 1.0, ATTN_NORM, PITCH_NORM);
}

/// `CBasePlayer::TraceAttack` (ReGameDLL `player.cpp:670-822`, read 2026-11-22) for the player
/// of `slot`: the hit group of the trace says how much of the damage counts (head x4, stomach
/// x1.25, legs x0.75, the rest x1) and what the player's view does; the damage is then added to
/// `mm` for `TakeDamage`. A teammate's shot makes no blood while friendly fire is off; the
/// kevlar stops the blood of a body hit and the helmet that of a head hit (sparks instead).
///
/// QUIRKS kept: the head multiplier applies before the blood test, so the punch uses the
/// multiplied damage; `m_LastHitGroup` is stored before the shield test; `BloodSplat` has no
/// body in the fetched sources (assumption: it does nothing). The shield branch is ported but
/// `IsHittingShield` is false until the shield task (`HasShield`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn trace_attack(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: usize,
    attacker: Option<EdictId>,
    mm: &mut bullets::MultiDamage,
    damage: f32,
    dir: Vec3,
    tr: &EntTrace,
    bits: u32,
) {
    let victim = eng.client_edict(slot as u8);
    let hit_group = tr.trace.hitgroup;
    let attacker_slot = attacker.and_then(|a| player_slot(g, eng, a));
    let friendly_fire = eng.cvar_f32("mp_friendlyfire") != 0.0;
    let attacker_team = attacker_slot.and_then(|a| g.players[a].as_ref()).map(|p| p.team);
    let Some(p) = g.players[slot].as_mut() else { return };
    let mut should_bleed = true;
    let mut should_spark = false;
    // `IsHittingShield`: no shield yet.
    let hit_shield = false;
    if attacker_team == Some(p.team) && !friendly_fire {
        should_bleed = false;
    }
    if !p.takes_damage {
        return;
    }
    p.last_hit_group = hit_group;
    let kevlar = p.kevlar;
    let mut damage = damage;
    let mut punch_x = None;
    let mut punch_z = None;
    let mut punch_roll = false;
    if hit_shield {
        // BUGBUG kept: the damage is zeroed before the shield alters the punch.
        damage = 0.0;
        should_bleed = false;
    } else {
        match hit_group {
            HITGROUP_HEAD => {
                if kevlar == ArmorType::VestHelm {
                    should_bleed = false;
                    should_spark = true;
                }
                damage *= 4.0;
                if should_bleed {
                    punch_x = Some(((f64::from(damage) * -0.5) as f32).max(-12.0));
                    punch_roll = true;
                }
            }
            HITGROUP_CHEST => {
                if kevlar != ArmorType::None {
                    should_bleed = false;
                } else if should_bleed {
                    punch_x = Some(((f64::from(damage) * -0.1) as f32).max(-4.0));
                }
            }
            HITGROUP_STOMACH => {
                damage = (f64::from(damage) * 1.25) as f32;
                if kevlar != ArmorType::None {
                    should_bleed = false;
                } else if should_bleed {
                    punch_x = Some(((f64::from(damage) * -0.1) as f32).max(-4.0));
                }
            }
            HITGROUP_LEFTARM | HITGROUP_RIGHTARM => {
                if kevlar != ArmorType::None {
                    should_bleed = false;
                }
            }
            HITGROUP_LEFTLEG | HITGROUP_RIGHTLEG => {
                damage = (f64::from(damage) * 0.75) as f32;
            }
            _ => {}
        }
    }
    if punch_roll {
        let r = bullets::random_float(g, -1.0, 1.0);
        punch_z = Some((damage * r).clamp(-9.0, 9.0));
    }
    if let Some(v) = eng.ent_mut(victim) {
        if let Some(x) = punch_x {
            v.punchangle.x = x;
        }
        if let Some(z) = punch_z {
            v.punchangle.z = z;
        }
    }
    let end_pos = tr.trace.end_pos;
    if should_bleed {
        // `BloodSplat(...)`: no body in the source we have.
        bullets::spawn_blood(eng, end_pos, bullets::BLOOD_COLOR_RED, damage);
        bullets::trace_bleed(g, eng, victim, damage, dir, end_pos, bits);
    } else if hit_group == HITGROUP_HEAD && should_spark {
        // They hit a helmet.
        let te = cs_proto48::tempent::streak_splash(end_pos, tr.trace.plane_normal, 5, 22, 25, 65);
        eng.send_temp_entity(MsgDest::Pvs(end_pos), &te);
    }
    bullets::add_multi_damage(g, eng, mm, attacker, victim, damage, bits);
}

/// The part of `CBaseMonster::TakeDamage` a player runs (the health comes off, a player at zero
/// dies). `damage` arrives truncated (`int(flDamage)`). The player's own bookkeeping of the
/// hit (what the `Damage` message reports) follows `CBaseMonster::TakeDamage` as reviewed
/// before (`basemonster.cpp:430-441`); `player_bits` is false for the explosion branch, which
/// returns before `m_bitsHUDDamage` is set (the damage bits are ORed in either way).
///
/// Assumption (`CBaseMonster::TakeDamage` is not among the fetched sources): it is the
/// Half-Life monster's, which has no momentum push. The block of `CBaseEntity::TakeDamage`
/// (`cbase.cpp:660-712`, `velocity += dir * min(1000, damage * ...)`) is not run for a player;
/// the only knockback is `TakeDamageImpulse` ([`take_damage_with`]). A bullet that pushed a
/// corpse 500 units a second is not what the game does.
#[allow(clippy::too_many_arguments)]
fn monster_take_damage(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    inflictor: Option<EdictId>,
    attacker: Option<EdictId>,
    attacker_weapon: Option<WeaponId>,
    damage: f32,
    bits: u32,
    player_bits: bool,
) -> bool {
    let index = usize::from(slot);
    let id = eng.client_edict(slot);
    let Some(p) = g.players[index].as_ref() else { return false };
    if !p.takes_damage {
        return false;
    }
    // Do the damage.
    let Some(p) = g.players[index].as_mut() else { return false };
    p.health -= damage;
    // `m_bitsDamageType |= bitsDamageType` is the first thing `CBaseMonster::TakeDamage` does.
    p.damage_bits |= bits;
    // What the `Damage` message reports (`CBaseMonster::TakeDamage`, ReGameDLL
    // `basemonster.cpp:430-441`, as reviewed): for a player, the inflictor when there is one
    // (the old one stays otherwise) and the health lost. Nothing sets `pev->dmg_save`, so the
    // message's armor byte is always 0 (the field is not kept).
    if inflictor.is_some() {
        p.dmg_inflictor = inflictor;
    }
    p.dmg_take += damage;
    if player_bits {
        // `m_bitsHUDDamage = -1` at the end of `CBasePlayer::TakeDamage` (player.cpp:1374): the
        // next client data update sends the `Damage` message whatever the bits.
        p.hud_damage = -1;
    }
    let health = p.health;
    if let Some(v) = eng.ent_mut(id) {
        v.health = health;
    }
    if health <= 0.0 {
        // `g_pevLastInflictor = pevInflictor; Killed(pevAttacker, GIB_NORMAL);` the inflictor
        // goes to the death message as an argument.
        killed(g, eng, slot, attacker, inflictor, attacker_weapon);
    }
    // The observers that follow the player are told its health.
    observer::notify_spec_health(g, eng, slot);
    health > 0.0
}

/// `CBasePlayer::TakeDamage` (ReGameDLL `player.cpp:909-1380`, read 2026-11-22) for an attacker
/// whose active weapon is found by [`active_weapon_id`]; see [`take_damage_with`].
pub(crate) fn take_damage(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    inflictor: Option<EdictId>,
    attacker: Option<EdictId>,
    damage: f32,
    bits: u32,
) -> bool {
    let weapon = attacker
        .and_then(|a| player_slot(g, eng, a))
        .and_then(|a| active_weapon_id(g, a));
    take_damage_with(g, eng, slot, inflictor, None, attacker, weapon, damage, bits)
}

/// `TakeDamage` for the damage of a grenade of team `inflictor_team` (`m_iTeam`), whose attacker
/// may have left or changed team since the throw.
#[allow(clippy::too_many_arguments)]
pub(crate) fn take_damage_from_grenade(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    inflictor: Option<EdictId>,
    inflictor_team: Option<i32>,
    attacker: Option<EdictId>,
    damage: f32,
    bits: u32,
) -> bool {
    let weapon = attacker
        .and_then(|a| player_slot(g, eng, a))
        .and_then(|a| active_weapon_id(g, a));
    take_damage_with(g, eng, slot, inflictor, inflictor_team, attacker, weapon, damage, bits)
}

/// `CBasePlayer::TakeDamage` (ReGameDLL `player.cpp:909-1380`, read 2026-11-22) then
/// `CBaseMonster::TakeDamage` ([`monster_take_damage`]). `attacker_weapon` is the active weapon
/// of the attacker (`pAttack->m_pActiveItem->m_iId`; `None` for the world or an attacker with
/// no weapon in hand). `attacker` and `inflictor` are `None` for the world. Returns whether the
/// player took damage and survived (the original's result).
///
/// In the source's order: a blast, an explosion or a fall sets the hit group to generic, a bullet
/// at the shield's hit group is refused. An explosion (`DMG_EXPLOSION` or `DMG_BLAST`) has its
/// own branch: a grenade of the victim's team hurts it only with friendly fire or if the victim
/// threw it (`m_iTeam`, the team of the thrower when it threw: given by [`crate::grenade`]), armor takes its share with the explosion formula, there are two pain sounds, and it
/// returns before `m_bitsHUDDamage`. QUIRK: the `flBonus *= 2` for `DMG_BLAST` further down is
/// unreachable in the source, because `DMG_BLAST` is caught by the explosion branch first.
///
/// Everything else: `FPlayerCanTakeDamage` (a teammate's needs `mp_friendlyfire`, except a
/// grenade), then for a player attacker (not the victim's own fall): a teammate's (and one's
/// own) damage is scaled by 0.35 (`float * double`), the armor ratio takes the multiplier of
/// the attacker's weapon, then the flinch: a standing victim hit by a heavy rifle outside the
/// legs gets knocked back 170 units if it moves slower than 300 and its speed modifier set to
/// 0.65, any other hit sets the modifier to 0.5 and `m_bHighDamage`. Armor absorbs
/// `(1 - ratio) * damage * bonus` points where the hit group is armored; if it runs out the
/// rest of the damage goes through at `armor / bonus`. Then `Pain`, `int(flDamage)` goes to
/// [`monster_take_damage`], and the time-based damage counters of the hit's kinds reset.
///
/// Left out: the VIP's halved ratio and the shield's added 0.2 (VIP and shield tasks), the
/// teammate-attack message (`#Game_teammate_attack`) and `m_flLastAttackedTeammate`, `m_lastDamageAmount`,
/// `m_bKilledByBomb`/`m_bKilledByGrenade` (bomb task), `LogAttack`, bots, the career tasks and
/// the director/`HLTV`
/// messages.
#[allow(clippy::too_many_arguments)]
pub(crate) fn take_damage_with(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    inflictor: Option<EdictId>,
    inflictor_team: Option<i32>,
    attacker: Option<EdictId>,
    attacker_weapon: Option<WeaponId>,
    damage: f32,
    bits: u32,
) -> bool {
    let index = usize::from(slot);
    if !damage.is_finite() {
        return false;
    }
    let id = eng.client_edict(slot);
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
    let inflictor_is_grenade = inflictor
        .and_then(|i| eng.ent(i))
        .is_some_and(|v| v.classname == "grenade");

    let mut ratio = ARMOR_RATIO;
    let bonus = ARMOR_BONUS;
    let mut damage = damage;

    let Some(p) = g.players[index].as_mut() else {
        return false;
    };
    if bits & (dmg::EXPLOSION | dmg::BLAST | dmg::FALL) != 0 {
        p.last_hit_group = HITGROUP_GENERIC;
    } else if p.last_hit_group == HITGROUP_SHIELD && bits & dmg::BULLET != 0 {
        return false;
    }
    // `HasShield()` (flShieldRatio 0.2) and `m_bIsVIP` (ratio * 0.5): neither exists yet.

    if bits & (dmg::EXPLOSION | dmg::BLAST) != 0 {
        if !p.alive {
            return false;
        }
        // `pGrenade->m_iTeam == m_iTeam`: the team the grenade was thrown by, whoever the attacker
        // is now (a thrower that left or changed team does not make its grenade an enemy's).
        if bits & dmg::EXPLOSION != 0
            && inflictor_is_grenade
            && inflictor_team == Some(i32::from(victim_team.id()))
            && !friendly_fire
            && attacker_slot != Some(index)
        {
            // If cvar friendlyfire is disabled and the victim is a teammate, ignore this damage.
            return false;
        }
        if let Some(i) = inflictor {
            let origin = eng.ent(id).map(|v| v.origin);
            let inflictor_origin = eng.ent(i).map(|v| v.origin);
            if let (Some(o), Some(io), Some(p)) = (origin, inflictor_origin, g.players[index].as_mut()) {
                p.blast_vector = o - io;
            }
        }
        let Some(p) = g.players[index].as_mut() else {
            return false;
        };
        let hit_group = p.last_hit_group;
        let armored = p.armor != 0.0 && p.is_armored(hit_group);
        if armored {
            let mut new = ratio * damage;
            let mut armor = (damage - new) * bonus;
            // Does this use more armor than we have?
            if armor > p.armor {
                new = damage - p.armor;
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
        pain(g, eng, slot, hit_group, armored);
        if g.players[index].as_ref().is_some_and(|p| p.health > damage) {
            player_anim::set_animation(g, eng, slot, PlayerAnim::Flinch);
            pain(g, eng, slot, hit_group, false);
        }
        let took = monster_take_damage(
            g,
            eng,
            slot,
            inflictor,
            attacker,
            attacker_weapon,
            damage.trunc(),
            bits,
            false,
        );
        reset_time_based_damage(g, index, bits);
        return took;
    }

    // `FPlayerCanTakeDamage`; a grenade's damage is never refused here.
    if attacker_slot.is_some() && teammate && attacker_slot != Some(index) && !friendly_fire
        && !inflictor_is_grenade
    {
        return false;
    }
    // Already dead, and `pev->takedamage == DAMAGE_NO`.
    if !p.takes_damage || !p.alive {
        return false;
    }
    // `if ((bitsDamageType & DMG_BLAST) && IsMultiplayer()) flBonus *= 2;` is unreachable (see above).
    if let Some(a) = attacker_slot
        && !(a == index && bits & dmg::FALL != 0)
    {
        if teammate {
            // "warn about team attacks" (player.cpp:1139, read 2026-11-22). QUIRK (vanilla, not
            // `REGAMEDLL_FIXES`): the history that is looked at and set is the victim's, the
            // hint goes to the attacker.
            if a != index && crate::hint::first_time(g, slot, crate::hint::dhf::FRIEND_INJURED) {
                crate::hint::hint(g, eng, a as u8, "#Hint_try_not_to_injure_teammates");
            }
            // Vanilla: `flDamage *= 0.35` (a `double` product).
            damage = (f64::from(damage) * 0.35) as f32;
        }
        if let Some(w) = attacker_weapon {
            // `flRatio += flShieldRatio` (0 without a shield), then the weapon's multiplier.
            ratio = weapon_armor_ratio(w, ratio);
        }
        let hit_group = g.players[index].as_ref().map_or(HITGROUP_GENERIC, |p| p.last_hit_group);
        let ducking = eng.ent(id).is_some_and(|v| v.flags & FL_DUCKING != 0);
        let attacker_origin = eng.ent(eng.client_edict(a as u8)).map(|v| v.origin);
        let victim_vars = eng.ent(id).map(|v| (v.origin, v.velocity));
        let Some(p) = g.players[index].as_mut() else {
            return false;
        };
        let flinch = if !should_do_large_flinch(ducking, hit_group, attacker_weapon) {
            // `TakeDamageImpulse(pAttack, 0.0f, 0.5f)`.
            p.velocity_modifier = 0.5;
            p.high_damage = if hit_group == HITGROUP_HEAD { damage > 60.0 } else { damage > 20.0 };
            PlayerAnim::Flinch
        } else {
            if let (Some((origin, velocity)), Some(ao)) = (victim_vars, attacker_origin)
                && velocity.length() < 300.0
            {
                // `TakeDamageImpulse(pAttack, 170, 0.65f)`; 170 is the vanilla constant (the
                // `knockback` cvar is `REGAMEDLL_ADD`).
                let push = bullets::normalize_hl(origin - ao) * 170.0;
                p.velocity_modifier = 0.65;
                if let Some(v) = eng.ent_mut(id) {
                    v.velocity += push;
                }
            }
            PlayerAnim::LargeFlinch
        };
        // `SetAnimation(PLAYER_FLINCH)` or `SetAnimation(PLAYER_LARGE_FLINCH)`.
        player_anim::set_animation(g, eng, slot, flinch);
    }

    // Armor. It doesn't protect against fall or drown damage!
    let Some(p) = g.players[index].as_mut() else {
        return false;
    };
    let hit_group = p.last_hit_group;
    let armored = p.armor != 0.0 && bits & (dmg::DROWN | dmg::FALL) == 0 && p.is_armored(hit_group);
    if armored {
        let mut new = ratio * damage;
        let mut armor = (damage - new) * bonus;
        // Does this use more armor than we have?
        if armor > p.armor {
            armor = p.armor;
            armor *= 1.0 / bonus;
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
    pain(g, eng, slot, hit_group, armored);

    // This cast to INT is critical: a player with 0.5 health would look dead to the engine.
    let took = monster_take_damage(
        g,
        eng,
        slot,
        inflictor,
        attacker,
        attacker_weapon,
        damage.trunc(),
        bits,
        true,
    );
    reset_time_based_damage(g, index, bits);
    took
}

/// Reset the damage time countdown for each type of time-based damage the player just sustained.
fn reset_time_based_damage(g: &mut GameState, index: usize, bits: u32) {
    if let Some(p) = g.players[index].as_mut() {
        for (i, counter) in p.time_based_damage.iter_mut().enumerate() {
            if bits & (dmg::PARALYZE << i) != 0 {
                *counter = 0;
            }
        }
    }
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
    // `Killed(pev, GIB_NEVER)`: no inflictor was set, so the death notice names the world.
    killed(g, eng, slot, Some(id), None, None);
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

/// `CBasePlayer::Disappear` (ReGameDLL `player.cpp` 3902-3972, read 2026-11-23, vanilla): the
/// player leaves the round without dying (the VIP that reached a safety zone): it stops
/// animating on the plain player model, lies down (`view_ofs` -8, `DEAD_DYING`, not solid, off
/// the ground), its HUD health and weapon go, the field of view resets, the rules look at the
/// teams again, it counts as not killed, the buy zone icon goes and `PlayerDeathThink` takes it
/// over. A player that carries the C4 drops it and one with a defuser loses it
/// (`DropPlayerItem("weapon_c4")`, `RemoveDefuser`: tasks 4 and 5, so nothing happens yet).
///
/// Not ported: `DetachTank` (there is no `func_tank`), the `CSound` reset of the player's sound
/// slot (the game has no sound entity), `SetSuitUpdate(nullptr, ...)` (no suit sentence queue).
pub(crate) fn disappear(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let index = usize::from(slot);
    let id = eng.client_edict(slot);
    if g.players.get(index).is_none_or(Option::is_none) {
        return;
    }
    let player_model = eng.model_index("models/player.mdl");
    if let Some(p) = g.players[index].as_mut() {
        // `m_fSequenceFinished = TRUE`.
        p.anim.sequence_finished = true;
        // `m_iClientHealth = 0`, and `SendFOV(0)`: `m_iClientFOV = m_iFOV = 0`.
        p.client_health = 0;
        p.client_fov = 0;
        p.fov = 0;
    }
    if let Some(v) = eng.ent_mut(id) {
        // `SendFOV(0)`: `pev->fov = 0`.
        v.fov = 0.0;
        if let Some(model) = player_model {
            v.modelindex = model;
        }
        v.view_ofs = Vec3::new(0.0, 0.0, -8.0);
        v.deadflag = DEAD_DYING;
        v.solid = SOLID_NOT;
        v.flags &= !FL_ONGROUND;
    }
    messages::health(eng, slot, 0);
    messages::cur_weapon_dead(eng, slot);
    // `SendFOV(0)`.
    messages::set_fov(eng, slot, 0);
    rules::check_win_conditions(g, eng);
    if let Some(p) = g.players[index].as_mut() {
        p.not_killed = false;
    }
    // (`if (m_bHasC4) DropPlayerItem("weapon_c4")` and `else if (m_bHasDefuser)
    // RemoveDefuser(); GiveNamedItem("item_thighpack")` go here: the bomb and the defuser are
    // tasks 4 and 5.)
    crate::buy::buy_zone_icon_clear(g, eng, slot);
    // `SetThink(&CBasePlayer::PlayerDeathThink); pev->nextthink = time + 0.1`, and the angles
    // lose their pitch and roll.
    let now = eng.time();
    if let Some(v) = eng.ent_mut(id) {
        v.nextthink = f64::from(now as f32 + 0.1);
        v.angles.x = 0.0;
        v.angles.z = 0.0;
    }
}

/// `CHalfLifeMultiplay::PlayerKilled` (the part this task has the data for) and
/// `CBasePlayer::Killed` (ReGameDLL `player.cpp`, read 2026-11-22): the death notice, the frag
/// rules, the scoreboard, then the player dies (`DEAD_DYING`, not solid, `MOVETYPE_TOSS`) and
/// runs `PlayerDeathThink`. The messages go out in the order of `Killed`: `PlayerKilled`'s
/// (`DeathMsg`, `ScoreInfo`), `NVGToggle` of the player and of its first-person watchers, the
/// camera (`ADStop` or the black `ScreenFade`), `ScoreAttrib`, `Health 0`, `CurWeapon 0 255 255`,
/// `SetFOV 0`, the `CheckWinConditions` ones, `BuyZoneIcon_Clear`, and the death sound.
///
/// Frags: a suicide costs one, killing a teammate costs one, killing an enemy gives one, the
/// world (an attacker that is no player) changes none. Money: 300 for an enemy, -3300 for a
/// teammate ([`money::killed_by`]).
fn killed(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    attacker: Option<EdictId>,
    inflictor: Option<EdictId>,
    attacker_weapon: Option<WeaponId>,
) {
    let index = usize::from(slot);
    let id = eng.client_edict(slot);
    let now = eng.time();
    if g.players[index].is_none() {
        return;
    }
    // `PlayerKilled` with `DeathNotice`: the death notice, deaths, frags, money, scoreboard.
    score::player_killed(g, eng, slot, attacker, inflictor, attacker_weapon);
    // `NVGToggle 0` to the player and to the observers that watch it in first person.
    observer::killed_night_vision(g, eng, slot);
    // `SetAnimation(PLAYER_DIE)` (player.cpp:2360): the death sequence, and (vanilla) a first
    // `DeathSound`; `Killed` plays another one at its end.
    player_anim::set_animation(g, eng, slot, PlayerAnim::Die);
    // `Killed` (player.cpp 2365-2411, vanilla): a player that dies holding a grenade and the
    // fire button throws it.
    throw_grenade_on_death(g, eng, slot);

    // CBasePlayer::Killed.
    let mut punished_for_tk = false;
    if let Some(p) = g.players[index].as_mut() {
        punished_for_tk = std::mem::take(&mut p.punished_for_tk);
        p.alive = false;
        p.takes_damage = false;
        p.dead_time = now;
        p.client_health = 0;
    }
    if let Some(v) = eng.ent_mut(id) {
        v.fov = 0.0;
        // `SetThink(&CBasePlayer::PlayerDeathThink); pev->nextthink = gpGlobals->time + 0.1f`
        // (player.cpp:2604): the entity think runs it too, besides the one in `PreThink`.
        v.nextthink = f64::from(now as f32 + 0.1);
        // `Killed` (player.cpp 2418): the dead have no shield hit box.
        v.gamestate = HITGROUP_SHIELD_DISABLED;
        v.deadflag = DEAD_DYING;
        v.movetype = MOVETYPE_TOSS;
        v.flags &= !FL_ONGROUND;
        v.solid = SOLID_NOT;
        // `Killed` (player.cpp:3919, read 2026-11-22): `pev->view_ofs = Vector(0, 0, -8)`.
        v.view_ofs = Vec3::new(0.0, 0.0, -8.0);
    }
    // `Killed` (player.cpp:2606, read 2026-11-22): a player punished for a team kill is told
    // why it cannot play (shown to the dead, whatever `_ah` says).
    if punished_for_tk {
        crate::hint::hint_message(g, eng, slot, "#Hint_cannot_play_because_tk", true, true);
    }
    // The camera of the dying: it chases the killer, or the screen fades to black.
    observer::killed(g, eng, slot, attacker);
    // `SetScoreboardAttributes()`: the scoreboard marks the player dead.
    messages::set_scoreboard_attributes(g, eng, usize::from(slot));
    // The corpse's throw (`m_iThrowDirection`, which `SetAnimation(PLAYER_DIE)` chose).
    throw_corpse(g, eng, slot, attacker);
    messages::health(eng, slot, 0);
    messages::cur_weapon_dead(eng, slot);
    // `SendFOV(0)`.
    messages::set_fov(eng, slot, 0);
    // A team may be out of players now (`CheckWinConditions` of `Killed`).
    rules::check_win_conditions(g, eng);
    // The buy zone icon goes (and a buy menu that is open closes).
    crate::buy::buy_zone_icon_clear(g, eng, slot);
    // `DeathSound`: one of four, `RANDOM_LONG(1, 4)`.
    let sample = match g.random_range(1, 4) {
        1 => "player/die1.wav",
        2 => "player/die2.wav",
        3 => "player/die3.wav",
        _ => "player/death6.wav",
    };
    eng.emit_sound(id, CHAN_VOICE, sample, 1.0, ATTN_NORM, PITCH_NORM);
    // `pev->angles.x = 0; pev->angles.z = 0;` (player.cpp 2631): after the throw above, which
    // takes its direction from the angles with the pitch the player died with.
    if let Some(v) = eng.ent_mut(id) {
        v.angles.x = 0.0;
        v.angles.z = 0.0;
    }
    // `Killed` ends (player.cpp:2634, read 2026-11-22), for a player that was not gibbed, by
    // telling the dead how to change the view, once.
    if crate::hint::first_time(g, slot, crate::hint::dhf::SPEC_DUCK) {
        crate::hint::hint_message(g, eng, slot, "#Spec_Duck", true, true);
    }
}

/// The corpse throw of `CBasePlayer::Killed` (`player.cpp` 2493-2547, read 2026-11-22, vanilla):
/// `m_iThrowDirection`, which `SetAnimation(PLAYER_DIE)` chose, gives the dead body a velocity.
/// `THROW_FORWARD` and `THROW_BACKWARD` use the player's own `angles`; `THROW_HITVEL` and
/// `THROW_HITVEL_MINUS_AIRVEL` the attacker's, when the attacker is a player (the velocity stays
/// as it was otherwise); `THROW_BOMB` and `THROW_GRENADE` take the direction from
/// `m_vBlastVector`. Then (QUIRK, the `REGAMEDLL_FIXES` build drops it) the yaw of `angles` and
/// `v_angle` is turned to face against the new velocity: `UTIL_VecToAngles(-velocity).y`. The
/// throw direction is cleared.
///
/// The random numbers are taken in the source's order. Assumption: `m_vBlastVector` of length 0
/// (the source divides by it) throws nothing, instead of writing a non-finite velocity.
fn throw_corpse(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, attacker: Option<EdictId>) {
    const THROW_FORWARD: i32 = 1;
    const THROW_BACKWARD: i32 = 2;
    const THROW_HITVEL: i32 = 3;
    const THROW_BOMB: i32 = 4;
    const THROW_GRENADE: i32 = 5;
    const THROW_HITVEL_MINUS_AIRVEL: i32 = 6;
    let id = eng.client_edict(slot);
    let Some(p) = g.players[usize::from(slot)].as_ref() else {
        return;
    };
    let (direction, blast) = (p.anim.throw_direction, p.blast_vector);
    if direction == 0 {
        return;
    }
    let Some(v) = eng.ent(id) else {
        return;
    };
    let (angles, mut velocity) = (v.angles, v.velocity);
    // `FClassnameIs(pevAttacker, "player")`: the attacker is a client.
    let attacker_angles = attacker
        .filter(|&a| player_slot(g, eng, a).is_some())
        .and_then(|a| eng.ent(a))
        .map(|a| a.angles);
    let forward = |angles: Vec3| cs_sim::pm::math::angle_vectors(angles).0;
    match direction {
        THROW_FORWARD => {
            let f = forward(angles);
            velocity = f * bullets::random_float(g, 100.0, 200.0);
            velocity.z = bullets::random_float(g, 50.0, 100.0);
        }
        THROW_BACKWARD => {
            let f = forward(angles);
            velocity = f * bullets::random_float(g, -100.0, -200.0);
            velocity.z = bullets::random_float(g, 50.0, 100.0);
        }
        THROW_HITVEL => {
            if let Some(a) = attacker_angles {
                let f = forward(a);
                velocity = f * bullets::random_float(g, 200.0, 300.0);
                velocity.z = bullets::random_float(g, 200.0, 300.0);
            }
        }
        THROW_BOMB => {
            let length = blast.length();
            if length != 0.0 {
                velocity = blast * (1.0 / length) * (2300.0f32 - length) * 0.25f32;
                velocity.z = (2300.0f32 - length) / 2.75f32;
            }
        }
        THROW_GRENADE => {
            let length = blast.length();
            if length != 0.0 {
                velocity = blast * (1.0 / length) * (500.0 - length);
                // `(350 - length) * 1.5`: the double literal.
                velocity.z = (f64::from(350.0 - length) * 1.5) as f32;
            }
        }
        THROW_HITVEL_MINUS_AIRVEL => {
            if let Some(a) = attacker_angles {
                let f = forward(a);
                velocity = f * bullets::random_float(g, 200.0, 300.0);
            }
        }
        _ => {}
    }
    if let Some(v) = eng.ent_mut(id) {
        v.velocity = velocity;
        // `pev->angles.y = UTIL_VecToAngles(-pev->velocity).y; pev->v_angle.y = pev->angles.y`.
        let yaw = intro::vec_to_angles(-velocity).y;
        v.angles.y = yaw;
        v.v_angle.y = yaw;
    }
    if let Some(p) = g.players[usize::from(slot)].as_mut() {
        p.anim.throw_direction = 0;
    }
}

/// The grenade switch of `CBasePlayer::Killed` (`player.cpp` 2365-2411, read 2026-11-22): a
/// player whose active item is a grenade (HE, flashbang, smoke) and who holds the fire button
/// while it has one throws it, from its eyes, with its `pev->angles` as the velocity (QUIRK: a
/// vector of a few degrees, so the grenade drops at its feet), a fuse of 1.5 seconds. The ammo
/// is not taken (`REGAMEDLL_FIXES` does).
fn throw_grenade_on_death(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    let Some(p) = g.players[usize::from(slot)].as_ref() else {
        return;
    };
    let Some(weapon) = p.active_item.filter(|w| {
        matches!(w, WeaponId::HeGrenade | WeaponId::Flashbang | WeaponId::SmokeGrenade)
    }) else {
        return;
    };
    let ammo_type = p
        .active_item
        .and_then(|w| p.find_item(w))
        .map_or(-1, |(s, i)| p.items[s][i].weapon.primary_ammo_type);
    let has_ammo = usize::try_from(ammo_type).ok().and_then(|i| p.ammo.get(i)).is_some_and(|&a| a != 0);
    let Some(v) = eng.ent(id) else {
        return;
    };
    if v.button & IN_ATTACK == 0 || !has_ammo {
        return;
    }
    let (src, angles) = (v.origin + v.view_ofs, v.angles);
    crate::grenade::throw_grenade(g, eng, slot, weapon, src, angles, 1.5);
}

/// `CBasePlayer::PlayerDeathThink` (`player.cpp` 3977-4080, read 2026-11-22) for a dead player,
/// on every command (`PreThink` calls it each frame) and from the entity think (`SetThink` from
/// `Killed`, [`death_think_entity`], every tenth of a second while it reschedules itself): a body on
/// the ground slows down by 20 units a second per call; the
/// guns are dropped; while the death animation plays (`DEAD_DYING`, the sequence not finished)
/// the frame advances (`StudioFrameAdvance`) and nothing else happens; then a body on the
/// ground stops colliding, `DEAD_DYING` becomes `DEAD_DEAD` (the time is taken), the animation
/// stops (`StopAnimation`: `framerate = 0`, the vanilla inline of the animating base class;
/// assumption, its header is not among the fetched sources) and the model stops interpolating
/// (`EF_NOINTERP`); a player dead for `DEATH_ANIMATION_TIME` leaves a corpse and goes to the
/// death camera. Becoming respawnable is the round rules' business.
///
/// Assumption: a player whose model the engine has not loaded (a test server without a model
/// loader) has no animation to wait for, so it is not held in `DEAD_DYING`; with the model the
/// wait is the source's.
/// The entity think of a dead player (`SetThink(&CBasePlayer::PlayerDeathThink)`): the same
/// function, called by the engine when `nextthink` is due, besides `PreThink`'s call. It
/// reschedules itself (`nextthink = time + 0.1`) only in the "wait for all buttons released"
/// branch, so the think ends when the body is animating, or a button is held, or the player has
/// no team; a live player (respawned meanwhile) is left alone.
pub(crate) fn death_think_entity(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if eng.ent(eng.client_edict(slot)).is_some_and(|v| v.deadflag != DEAD_NO) {
        death_think(g, eng, slot);
    }
}

fn death_think(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time();
    let id = eng.client_edict(slot);
    // `if (m_iJoiningState != JOINED) return;`
    if g.players[usize::from(slot)]
        .as_ref()
        .is_none_or(|p| p.joining != JoinState::Joined)
    {
        return;
    }
    let Some(v) = eng.ent_mut(id) else {
        return;
    };
    // "If the anim is done playing, go to the next state": a body on the ground slows down.
    if v.flags & FL_ONGROUND != 0 {
        let forward = v.velocity.length() - 20.0;
        v.velocity = if forward <= 0.0 {
            Vec3::ZERO
        } else {
            v.velocity.normalize() * forward
        };
    }
    // `PlayerDeathThink`: "we drop the guns here" (`if (HasWeapons()) PackDeadPlayerItems()`).
    crate::inventory::death_think_pack(g, eng, slot);
    let Some(v) = eng.ent(id) else {
        return;
    };
    // "If the anim is not done playing, go on animating."
    let (modelindex, dying) = (v.modelindex, v.deadflag == DEAD_DYING);
    let animating = modelindex != 0
        && dying
        && eng.studio_model(modelindex).is_some()
        && g.players[usize::from(slot)].as_ref().is_some_and(|p| !p.anim.sequence_finished);
    if animating {
        player_anim::studio_frame_advance(g, eng, slot);
        return;
    }
    let Some(p) = g.players[usize::from(slot)].as_mut() else {
        return;
    };
    let Some(v) = eng.ent_mut(id) else {
        return;
    };
    // Done "animating": a body on the ground stops colliding and moving.
    if v.movetype != MOVETYPE_NONE && v.flags & FL_ONGROUND != 0 {
        v.movetype = MOVETYPE_NONE;
    }
    if dying {
        v.deadflag = DEAD_DEAD;
        p.dead_time = now;
    }
    // `StopAnimation()`, and `pev->effects |= EF_NOINTERP`.
    v.framerate = 0.0;
    v.effects |= EF_NOINTERP;
    // A player dead for `DEATH_ANIMATION_TIME` leaves a corpse and goes to the death camera.
    if v.deadflag != DEAD_RESPAWNABLE && !p.pflag_observer && now > p.dead_time + DEATH_ANIMATION_TIME {
        let origin = [
            (v.origin.x * 128.0) as i32,
            (v.origin.y * 128.0) as i32,
            (v.origin.z * 128.0) as i32,
        ];
        let (angles, body) = (v.angles, v.body);
        // `float timeDiff = pev->animtime - gpGlobals->time; WRITE_LONG(timeDiff * 100)`.
        let time_diff = ((v.animtime - now as f32) * 100.0) as i32;
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
    // "wait for all buttons released": a dead player on a team that holds no button thinks again
    // in a tenth of a second (`pev->nextthink = gpGlobals->time + 0.1f`).
    let team = g.players[usize::from(slot)].as_ref().map(|p| p.team);
    if let (Some(team), Some(v)) = (team, eng.ent_mut(id))
        && v.deadflag == DEAD_DEAD
        && !matches!(team, Team::Unassigned | Team::Spectator)
        && v.button & !IN_SCORE == 0
    {
        v.nextthink = f64::from(now as f32 + 0.1);
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
    // `sendweapons` is the engine's `host_client->lw`: only a client that predicts its weapons
    // (`cl_lw`) is sent the weapon part of the client data (client.cpp:5073-5153).
    let sendweapons = eng.client_lw(slot);
    // `iuser3` (`sendweapons && pPlayer`, only `iuser1 == OBS_NONE && !pevOrg`): the bits of
    // the player's own state, not the entity's value.
    let iuser3 = if sendweapons && own.iuser1 == observer::OBS_NONE && source.is_none() {
        let mut bits = 0;
        if let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) {
            // `m_bCanShoot && !m_bIsDefusing` (`m_bCanShootOverride` is a mod API).
            if p.can_shoot && !p.is_defusing {
                bits |= PLAYER_CAN_SHOOT;
            }
            if p.signals.state() & crate::buy::SIGNAL_BOMB != 0 {
                bits |= PLAYER_IN_BOMB_ZONE;
            }
            if p.owns_shield {
                bits |= PLAYER_HOLDING_SHIELD;
            }
        }
        if g.rules.is_freeze_period() {
            bits |= PLAYER_FREEZE_TIME_OVER;
        }
        bits
    } else {
        own.iuser3
    };
    let mut data = GoldSrcClientData {
        origin: v.origin,
        velocity: v.velocity,
        view_ofs: v.view_ofs,
        flags: v.flags,
        waterlevel: v.waterlevel,
        health: v.health,
        maxspeed: v.max_speed,
        fov: v.fov,
        // `MODEL_INDEX(STRING(pev->viewmodel))`, `pev->weaponanim` (the followed player's for an
        // observer in first person).
        viewmodel: i32::from(v.viewmodel),
        weaponanim: v.weaponanim,
        // `deadflag` is not filled: the `UpdateClientData` export (client.cpp:5020, read
        // 2026-11-22) leaves it as the engine cleared it, and the golden capture of HLDS sends 0
        // for a player that is dead or not yet in the game.
        punchangle: v.punchangle,
        iuser1: own.iuser1,
        iuser2: own.iuser2,
        iuser3,
        // The movement's state (`UpdateClientData`, client.cpp 5019-5081, read 2026-11-22):
        // the followed player's for an observer in first person. `waterjumptime` is
        // `int(teleport_time)`; `watertype` has no field in the client data. `fuser4` and
        // `iuser4` are not sent.
        physinfo: eng.physics_info(slot),
        b_in_duck: i32::from(v.b_in_duck),
        fl_duck_time: v.fl_duck_time,
        fl_time_step_sound: v.fl_time_step_sound,
        fl_swim_time: v.fl_swim_time,
        waterjumptime: v.teleport_time as i32,
        pushmsec: v.pushmsec,
        fuser1: v.fuser1,
        fuser2: v.fuser2,
        fuser3: v.fuser3,
        ..Default::default()
    };
    let weapons_of = source.map_or(slot, |(t, _)| t);
    let Some(p) = g
        .players
        .get(usize::from(weapons_of))
        .and_then(Option::as_ref)
    else {
        return data;
    };
    data.weapons = p.weapons;
    if !sendweapons {
        return data;
    }
    // The legacy ammo counters (`TabulateAmmo`'s, in [`weapon_core::TABULATED_AMMO`] order) go
    // into the fields the Half-Life client reads as its ammo.
    let t = &p.tabulated_ammo;
    data.ammo_shells = t[0];
    data.ammo_nails = t[1];
    data.ammo_cells = t[2];
    data.ammo_rockets = t[3];
    data.vuser2 = Vec3::new(t[4] as f32, t[5] as f32, t[6] as f32);
    data.vuser3 = Vec3::new(t[7] as f32, t[8] as f32, t[9] as f32);
    data.m_fl_next_attack = p.next_attack;
    // The held weapon: its id, and the ammo it uses (the type and what is left of it); a
    // weapon with no ammo (the knife) says -1 and 0. QUIRK: the type is tested as unsigned.
    if let Some((s, i)) = p.active_item.and_then(|a| p.find_item(a)) {
        let item = &p.items[s][i];
        if item.behavior.use_decrement()
            && let Some(info) = crate::weapons::item_info(item.weapon.id)
        {
            data.m_i_id = info.id as i32;
            let ammo_type = item.weapon.primary_ammo_type;
            data.vuser4 = match usize::try_from(ammo_type) {
                Ok(index) if index < MAX_AMMO_SLOTS => {
                    Vec3::new(ammo_type as f32, p.ammo[index] as f32, 0.0)
                }
                _ => Vec3::new(-1.0, 0.0, 0.0),
            };
        }
    }
    data
}

/// `GetWeaponData` (ReGameDLL `client.cpp` 4947-5018, read 2026-11-22, `CLIENT_WEAPONS`): the
/// `weapon_data_t` of every weapon the player carries that the client predicts
/// (`UseDecrement()`), by weapon id, for the `svc_clientdata` of a client with `cl_lw`. Always
/// answers true (C returns 1, also for a slot with no player, whose list is then all zero).
///
/// QUIRKS kept: the timers are not sent below -0.001 (`Q_max(.., -0.001f)`); fields are reused
/// for what the client's weapon code reads from them: `m_fInZoom` carries `m_iShotsFired`,
/// `m_fAimedDamage` carries `m_flLastFire`, `fuser2` and `fuser3` the grenade's `m_flStartThrow`
/// and `m_flReleaseThrow`, `iuser1` the knife's `m_iSwing`. `m_flPumpTime`, `m_fReloadTime` and
/// `m_fNextAimBonus` are not sent (0). The `REGAMEDLL_FIXES` clip-size override is not ported.
pub(crate) fn weapon_data(
    g: &GameState,
    slot: u8,
    out: &mut [cs_proto48::delta::GoldSrcWeaponData; 64],
) -> bool {
    use cs_proto48::delta::GoldSrcWeaponData;
    for entry in out.iter_mut() {
        *entry = GoldSrcWeaponData::default();
    }
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return true;
    };
    for item in p.items.iter().flatten() {
        if !item.behavior.use_decrement() {
            continue;
        }
        let Some(info) = crate::weapons::item_info(item.weapon.id) else {
            continue;
        };
        let id = info.id as i32;
        let Some(entry) = usize::try_from(id).ok().and_then(|i| out.get_mut(i)) else {
            continue;
        };
        let w = &item.weapon;
        // `Q_max(x, -0.001f)`.
        let floor = -0.001_f32;
        entry.m_i_id = id;
        entry.m_i_clip = w.clip;
        entry.m_fl_time_weapon_idle = w.time_weapon_idle.max(floor);
        entry.m_fl_next_primary_attack = w.next_primary_attack.max(floor);
        entry.m_fl_next_secondary_attack = w.next_secondary_attack.max(floor);
        entry.m_fl_next_reload = w.next_reload.max(floor);
        entry.m_f_in_reload = i32::from(w.in_reload);
        entry.m_f_in_special_reload = w.in_special_reload;
        entry.m_f_in_zoom = w.shots_fired;
        entry.m_f_aimed_damage = w.last_fire;
        entry.m_i_weapon_state = w.weapon_state;
        entry.fuser2 = w.start_throw;
        entry.fuser3 = w.release_throw;
        entry.iuser1 = w.swing;
    }
    true
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
        "timeleft" => rules::timeleft_command(g, eng, slot),
        "listmaps" => rules::listmaps_command(g, eng, slot),
        _ => {
            // Distinct commands, so the order of `ClientCommand_` (`ignorerad`, the observer
            // commands, then the living player's) does not matter.
            if !observer::command(g, eng, slot, argv)
                && !crate::hud_misc::client_command(g, eng, slot, argv)
                && !crate::inventory::client_command(g, eng, slot, argv)
                && crate::buy::is_buy_command(command)
            {
                crate::buy::client_command(g, eng, slot, argv);
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
