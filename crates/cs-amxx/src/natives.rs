//! AMX Mod X natives, Counter-Strike hook trait, and shared host state.
//!
//! Clean-room GPL-3.0-or-later implementation citing AMX Mod X (`amxmodx.cpp`, `string.cpp`,
//! `float.cpp`, `newmenus.cpp`, `CLang.cpp`, `modules/nvault/amxxapi.cpp`, `modules/csx/`,
//! `cstrike`, `fun`, `engine`, `fakemeta`, `hamsandwich` `ham_const.inc`, read 2026-11-26)
//! and ReAPI (`reapi_gamedll.inc`, `reapi_gamedll_const.inc`, read 2026-11-26).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use cs_engine::consts::{FL_FAKECLIENT, FL_GODMODE, FL_PROXY};
use cs_engine::{EdictId, EngineApi, MsgDest};
use cs_mod_api::services::{
    LANG_PLAYER, LANG_SERVER, MenuManager, PluginRuntimeKind, RenderedMenu, RpcExportEntry,
    SharedPlatformServices, mprop,
};
use cs_mod_api::{AdminFlags, MenuCreateSpec, MenuItemSpec, RpcValue};
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;
use tracing::{info, warn};

use crate::runtime::{AmxxPlugin, invoke_plugin_in_slots, invoke_plugin_pure_in_slots};
use crate::vm::{AmxError, ExecEntry, NativeDispatch, PawnVm};

/// AMX Mod X forward return codes (`amxconst.inc`).
pub const PLUGIN_CONTINUE: i32 = 0;
pub const PLUGIN_HANDLED: i32 = 1;
pub const PLUGIN_HANDLED_MAIN: i32 = 2;

/// Hamsandwich forward return codes (`ham_const.inc`).
pub const HAM_IGNORED: i32 = 1;
pub const HAM_HANDLED: i32 = 2;
pub const HAM_OVERRIDE: i32 = 3;
pub const HAM_SUPERCEDE: i32 = 4;

/// `CreateMultiForward` execution modes (`amxconst.inc`).
pub const ET_IGNORE: i32 = 0;
pub const ET_STOP: i32 = 1;
pub const ET_STOP2: i32 = 2;
pub const ET_CONTINUE: i32 = 3;

/// `CreateMultiForward` parameter types (`amxconst.inc`).
pub const FP_CELL: i32 = 0;
pub const FP_FLOAT: i32 = 1;
pub const FP_STRING: i32 = 2;
pub const FP_ARRAY: i32 = 3;

/// `newmenus.inc` special item callback codes.
pub const MENU_MORE: i32 = -1;
pub const MENU_BACK: i32 = -2;
pub const MENU_EXIT: i32 = -3;

/// `client_print` destination constants (`amxconst.inc`).
pub const PRINT_NOTIFY: i32 = 1;
pub const PRINT_CONSOLE: i32 = 2;
pub const PRINT_CHAT: i32 = 3;
pub const PRINT_CENTER: i32 = 4;

/// `cs_set_user_armor` / `cs_get_user_armor` armor types (`cstrike.inc`).
pub const CS_ARMOR_NONE: i32 = 0;
pub const CS_ARMOR_KEVLAR: i32 = 1;
pub const CS_ARMOR_VESTHELM: i32 = 2;

/// ReAPI `rg_give_item` give types (`reapi_gamedll_const.inc`).
pub const GT_APPEND: i32 = 0;
pub const GT_REPLACE: i32 = 1;
pub const GT_DROP_AND_REPLACE: i32 = 2;

/// ReAPI `rg_add_account` account modes (`reapi_gamedll_const.inc`).
pub const AS_ADD: i32 = 0;
pub const AS_SET: i32 = 1;

/// ReAPI `get_member` / `set_member` field identifiers (`reapi_gamedll_const.inc`, read 2026-11-26).
pub mod reapi_member {
    pub const M_I_ACCOUNT: i32 = 1;
    pub const M_I_KEVLAR: i32 = 2;
    pub const M_B_HAS_DEFUSER: i32 = 3;
    pub const M_B_HAS_NIGHTVISION: i32 = 4;
    pub const M_I_TEAM: i32 = 5;
    pub const M_I_DEATHS: i32 = 6;
    pub const M_I_CLIP: i32 = 7;
    pub const M_I_FRAGS: i32 = 8;
    pub const M_FL_MAXSPEED: i32 = 9;
    pub const M_B_HAS_C4: i32 = 10;
}

/// Hamsandwich virtual hook identifiers (`plugins/include/ham_const.inc`, read 2026-11-26).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum HamFunction {
    Spawn = 0,
    TakeDamage = 9,
    Killed = 11,
    Touch = 44,
    ItemDeploy = 65,
    WeaponPrimaryAttack = 87,
    PlayerJump = 91,
}

impl TryFrom<i32> for HamFunction {
    type Error = ();

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Spawn),
            9 => Ok(Self::TakeDamage),
            11 => Ok(Self::Killed),
            44 => Ok(Self::Touch),
            65 => Ok(Self::ItemDeploy),
            87 => Ok(Self::WeaponPrimaryAttack),
            91 => Ok(Self::PlayerJump),
            _ => Err(()),
        }
    }
}

/// Fakemeta `pev_*` field identifiers (`fakemeta_const.inc`).
pub mod pev_const {
    pub const PEV_CLASSNAME: i32 = 1;
    pub const PEV_MODEL: i32 = 3;
    pub const PEV_TARGET: i32 = 4;
    pub const PEV_TARGETNAME: i32 = 5;
    pub const PEV_ENEMY: i32 = 16;
    pub const PEV_AIMENT: i32 = 17;
    pub const PEV_OWNER: i32 = 18;
    pub const PEV_GROUNDENTITY: i32 = 19;
    pub const PEV_IMPACTTIME: i32 = 26;
    pub const PEV_STARTTIME: i32 = 27;
    pub const PEV_IDEAL_YAW: i32 = 29;
    pub const PEV_LTIME: i32 = 32;
    pub const PEV_NEXTTHINK: i32 = 33;
    pub const PEV_GRAVITY: i32 = 34;
    pub const PEV_FRICTION: i32 = 35;
    pub const PEV_FRAME: i32 = 36;
    pub const PEV_ANIMTIME: i32 = 37;
    pub const PEV_FRAMERATE: i32 = 38;
    pub const PEV_HEALTH: i32 = 39;
    pub const PEV_TELEPORT_TIME: i32 = 43;
    pub const PEV_DMG: i32 = 48;
    pub const PEV_DMGTIME: i32 = 49;
    pub const PEV_SPEED: i32 = 50;
    pub const PEV_AIR_FINISHED: i32 = 51;
    pub const PEV_PAIN_FINISHED: i32 = 52;
    pub const PEV_RADSUIT_FINISHED: i32 = 53;
    pub const PEV_SCALE: i32 = 54;
    pub const PEV_RENDERAMT: i32 = 55;
    pub const PEV_MAXSPEED: i32 = 56;
    pub const PEV_FOV: i32 = 57;
    pub const PEV_FL_FALL_VELOCITY: i32 = 58;
    pub const PEV_FUSER1: i32 = 59;
    pub const PEV_FUSER2: i32 = 60;
    pub const PEV_FUSER3: i32 = 61;
    pub const PEV_FUSER4: i32 = 62;
    pub const PEV_FIXANGLE: i32 = 65;
    pub const PEV_MODELINDEX: i32 = 66;
    pub const PEV_VIEWMODEL: i32 = 67;
    pub const PEV_WEAPONMODEL: i32 = 68;
    pub const PEV_MOVETYPE: i32 = 69;
    pub const PEV_SOLID: i32 = 70;
    pub const PEV_SKIN: i32 = 71;
    pub const PEV_BODY: i32 = 72;
    pub const PEV_EFFECTS: i32 = 73;
    pub const PEV_SEQUENCE: i32 = 75;
    pub const PEV_GAITSEQUENCE: i32 = 76;
    pub const PEV_RENDERMODE: i32 = 77;
    pub const PEV_RENDERFX: i32 = 78;
    pub const PEV_DEADFLAG: i32 = 80;
    pub const PEV_BUTTON: i32 = 81;
    pub const PEV_IMPULSE: i32 = 82;
    pub const PEV_SPAWNFLAGS: i32 = 83;
    pub const PEV_FLAGS: i32 = 84;
    pub const PEV_COLORMAP: i32 = 85;
    pub const PEV_WATERLEVEL: i32 = 87;
    pub const PEV_WATERTYPE: i32 = 88;
    pub const PEV_WEAPONANIM: i32 = 90;
    pub const PEV_PUSHMSEC: i32 = 91;
    pub const PEV_B_IN_DUCK: i32 = 92;
    pub const PEV_FL_TIME_STEP_SOUND: i32 = 93;
    pub const PEV_FL_SWIM_TIME: i32 = 94;
    pub const PEV_FL_DUCK_TIME: i32 = 95;
    pub const PEV_I_STEP_LEFT: i32 = 96;
    pub const PEV_GAMESTATE: i32 = 97;
    pub const PEV_OLDBUTTONS: i32 = 98;
    pub const PEV_IUSER1: i32 = 100;
    pub const PEV_IUSER2: i32 = 101;
    pub const PEV_IUSER3: i32 = 102;
    pub const PEV_IUSER4: i32 = 103;
    pub const PEV_ORIGIN: i32 = 114;
    pub const PEV_VELOCITY: i32 = 116;
    pub const PEV_BASEVELOCITY: i32 = 117;
    pub const PEV_MOVEDIR: i32 = 119;
    pub const PEV_ANGLES: i32 = 120;
    pub const PEV_AVELOCITY: i32 = 121;
    pub const PEV_PUNCHANGLE: i32 = 122;
    pub const PEV_V_ANGLE: i32 = 123;
    pub const PEV_ENDPOS: i32 = 124;
    pub const PEV_STARTPOS: i32 = 125;
    pub const PEV_ABSMIN: i32 = 126;
    pub const PEV_ABSMAX: i32 = 127;
    pub const PEV_MINS: i32 = 128;
    pub const PEV_MAXS: i32 = 129;
    pub const PEV_SIZE: i32 = 130;
    pub const PEV_RENDERCOLOR: i32 = 131;
    pub const PEV_VIEW_OFS: i32 = 132;
    pub const PEV_VUSER1: i32 = 133;
    pub const PEV_VUSER2: i32 = 134;
    pub const PEV_VUSER3: i32 = 135;
    pub const PEV_VUSER4: i32 = 136;
}

/// Engine module `EV_*` field identifiers (`engine_const.inc`).
pub mod ev_const {
    pub const EV_INT_GAMESTATE: i32 = 0;
    pub const EV_INT_OLDBUTTONS: i32 = 1;
    pub const EV_INT_IUSER1: i32 = 3;
    pub const EV_INT_IUSER2: i32 = 4;
    pub const EV_INT_IUSER3: i32 = 5;
    pub const EV_INT_IUSER4: i32 = 6;
    pub const EV_INT_WEAPONANIM: i32 = 7;
    pub const EV_INT_PUSHMSEC: i32 = 8;
    pub const EV_INT_B_IN_DUCK: i32 = 9;
    pub const EV_INT_FL_TIME_STEP_SOUND: i32 = 10;
    pub const EV_INT_FL_SWIM_TIME: i32 = 11;
    pub const EV_INT_FL_DUCK_TIME: i32 = 12;
    pub const EV_INT_I_STEP_LEFT: i32 = 13;
    pub const EV_INT_MOVETYPE: i32 = 14;
    pub const EV_INT_SOLID: i32 = 15;
    pub const EV_INT_SKIN: i32 = 16;
    pub const EV_INT_BODY: i32 = 17;
    pub const EV_INT_EFFECTS: i32 = 18;
    pub const EV_INT_SEQUENCE: i32 = 20;
    pub const EV_INT_GAITSEQUENCE: i32 = 21;
    pub const EV_INT_RENDERMODE: i32 = 22;
    pub const EV_INT_RENDERFX: i32 = 23;
    pub const EV_INT_DEADFLAG: i32 = 25;
    pub const EV_INT_BUTTON: i32 = 26;
    pub const EV_INT_IMPULSE: i32 = 27;
    pub const EV_INT_SPAWNFLAGS: i32 = 28;
    pub const EV_INT_FLAGS: i32 = 29;
    pub const EV_INT_COLORMAP: i32 = 30;
    pub const EV_INT_WATERLEVEL: i32 = 32;
    pub const EV_INT_WATERTYPE: i32 = 33;
    pub const EV_INT_FIXANGLE: i32 = 35;
    pub const EV_INT_MODELINDEX: i32 = 36;
    pub const EV_INT_VIEWMODEL: i32 = 37;
    pub const EV_INT_WEAPONMODEL: i32 = 38;

    pub const EV_FL_IMPACTTIME: i32 = 0;
    pub const EV_FL_STARTTIME: i32 = 1;
    pub const EV_FL_IDEAL_YAW: i32 = 3;
    pub const EV_FL_LTIME: i32 = 6;
    pub const EV_FL_NEXTTHINK: i32 = 7;
    pub const EV_FL_GRAVITY: i32 = 8;
    pub const EV_FL_FRICTION: i32 = 9;
    pub const EV_FL_FRAME: i32 = 10;
    pub const EV_FL_ANIMTIME: i32 = 11;
    pub const EV_FL_FRAMERATE: i32 = 12;
    pub const EV_FL_HEALTH: i32 = 13;
    pub const EV_FL_TELEPORT_TIME: i32 = 17;
    pub const EV_FL_DMG: i32 = 22;
    pub const EV_FL_DMGTIME: i32 = 23;
    pub const EV_FL_SPEED: i32 = 24;
    pub const EV_FL_AIR_FINISHED: i32 = 25;
    pub const EV_FL_PAIN_FINISHED: i32 = 26;
    pub const EV_FL_RADSUIT_FINISHED: i32 = 27;
    pub const EV_FL_SCALE: i32 = 28;
    pub const EV_FL_RENDERAMT: i32 = 29;
    pub const EV_FL_MAXSPEED: i32 = 30;
    pub const EV_FL_FOV: i32 = 31;
    pub const EV_FL_FL_FALL_VELOCITY: i32 = 32;
    pub const EV_FL_FUSER1: i32 = 33;
    pub const EV_FL_FUSER2: i32 = 34;
    pub const EV_FL_FUSER3: i32 = 35;
    pub const EV_FL_FUSER4: i32 = 36;

    pub const EV_VEC_ORIGIN: i32 = 0;
    pub const EV_VEC_VELOCITY: i32 = 2;
    pub const EV_VEC_BASEVELOCITY: i32 = 3;
    pub const EV_VEC_MOVEDIR: i32 = 5;
    pub const EV_VEC_ANGLES: i32 = 6;
    pub const EV_VEC_AVELOCITY: i32 = 7;
    pub const EV_VEC_PUNCHANGLE: i32 = 8;
    pub const EV_VEC_V_ANGLE: i32 = 9;
    pub const EV_VEC_ENDPOS: i32 = 10;
    pub const EV_VEC_STARTPOS: i32 = 11;
    pub const EV_VEC_ABSMIN: i32 = 12;
    pub const EV_VEC_ABSMAX: i32 = 13;
    pub const EV_VEC_MINS: i32 = 14;
    pub const EV_VEC_MAXS: i32 = 15;
    pub const EV_VEC_SIZE: i32 = 16;
    pub const EV_VEC_RENDERCOLOR: i32 = 17;
    pub const EV_VEC_VIEW_OFS: i32 = 18;
    pub const EV_VEC_VUSER1: i32 = 19;
    pub const EV_VEC_VUSER2: i32 = 20;
    pub const EV_VEC_VUSER3: i32 = 21;
    pub const EV_VEC_VUSER4: i32 = 22;

    pub const EV_SZ_CLASSNAME: i32 = 0;
    pub const EV_SZ_MODEL: i32 = 2;
    pub const EV_SZ_TARGET: i32 = 3;
    pub const EV_SZ_TARGETNAME: i32 = 4;
}

/// Counter-Strike gameplay hooks implemented by `cs-game` (or [`NoCsHooks`] in standalone hosts).
pub trait AmxxCsHooks {
    fn get_user_money(&self, slot: u8) -> i32;
    fn set_user_money(&mut self, slot: u8, amount: i32);
    fn get_user_team(&self, slot: u8) -> i32;
    fn set_user_team(&mut self, slot: u8, team: i32);
    fn get_user_deaths(&self, slot: u8) -> i32;
    fn set_user_deaths(&mut self, slot: u8, deaths: i32);
    fn give_item(&mut self, slot: u8, eng: &mut EngineApi<'_>, item: &str) -> bool;
    fn strip_user_weapons(&mut self, slot: u8, eng: &mut EngineApi<'_>);
    fn get_user_armor(&self, slot: u8, eng: &EngineApi<'_>) -> i32;
    fn set_user_armor(&mut self, slot: u8, eng: &mut EngineApi<'_>, armor: i32, armor_type: i32);

    // --- Phase 15 ReAPI / Hamsandwich / Weapon Extensions ---
    fn round_respawn(&mut self, _slot: u8, _eng: &mut EngineApi<'_>) -> bool {
        false
    }
    fn add_account(&mut self, slot: u8, eng: &mut EngineApi<'_>, delta: i32, flash: bool) {
        let next = self.get_user_money(slot).saturating_add(delta).clamp(0, 16_000);
        self.set_user_money(slot, next);
        let _ = eng.send_usermsg(
            MsgDest::One(slot),
            &CsUserMsg::Money {
                amount: next,
                flash,
            },
        );
    }
    fn remove_all_items(&mut self, slot: u8, eng: &mut EngineApi<'_>, _remove_suit: bool) {
        self.strip_user_weapons(slot, eng);
    }
    fn get_active_weapon(&self, _slot: u8) -> (i32, i32, i32) {
        (0, 0, 0)
    }
    fn get_weapon_clip(&self, _slot: u8, _weapon_id: u8) -> i32 {
        0
    }
    fn set_weapon_clip(&mut self, _slot: u8, _weapon_id: u8, _clip: i32) {}
    fn get_user_bpammo(&self, _slot: u8, _weapon_id: u8) -> i32 {
        0
    }
    fn set_user_bpammo(
        &mut self,
        _slot: u8,
        _eng: &mut EngineApi<'_>,
        _weapon_id: u8,
        _amount: i32,
    ) {
    }
    fn get_member(&self, _ent_index: u16, _eng: &EngineApi<'_>, _member: i32) -> i32 {
        0
    }
    fn set_member(
        &mut self,
        _ent_index: u16,
        _eng: &mut EngineApi<'_>,
        _member: i32,
        _val: i32,
    ) -> bool {
        false
    }
    fn execute_ham(
        &mut self,
        _func: HamFunction,
        _ent_index: u16,
        _eng: &mut EngineApi<'_>,
        _args: &[i32],
    ) -> i32 {
        0
    }
}

/// Default no-op implementation of [`AmxxCsHooks`] for non-CS games or standalone VM hosts.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoCsHooks;

impl AmxxCsHooks for NoCsHooks {
    fn get_user_money(&self, _slot: u8) -> i32 {
        0
    }
    fn set_user_money(&mut self, _slot: u8, _amount: i32) {}
    fn get_user_team(&self, _slot: u8) -> i32 {
        0
    }
    fn set_user_team(&mut self, _slot: u8, _team: i32) {}
    fn get_user_deaths(&self, _slot: u8) -> i32 {
        0
    }
    fn set_user_deaths(&mut self, _slot: u8, _deaths: i32) {}
    fn give_item(&mut self, _slot: u8, _eng: &mut EngineApi<'_>, _item: &str) -> bool {
        false
    }
    fn strip_user_weapons(&mut self, _slot: u8, _eng: &mut EngineApi<'_>) {}
    fn get_user_armor(&self, _slot: u8, _eng: &EngineApi<'_>) -> i32 {
        0
    }
    fn set_user_armor(
        &mut self,
        _slot: u8,
        _eng: &mut EngineApi<'_>,
        _armor: i32,
        _armor_type: i32,
    ) {
    }
}

/// Bridge allowing `.amxx` dynamic natives and `ExecuteForward` to invoke non-AMXX plugins
/// (e.g. `.wasm` / `.py` mods hosted by `cs-mod-host`) synchronously over the shared bus.
pub trait AmxxExternalRpc: Send + Sync {
    fn call_external_rpc(&self, fn_name: &str, args: &[RpcValue]) -> Option<RpcValue>;
    fn emit_external_event(&self, event_name: &str, args: &[RpcValue]);
}

/// Per-player statistics stored for `csx.inc` (`get_stats` / `get_user_stats` / `get_statsnum`)
/// and synchronized into the shared [`LeaderboardStore`](cs_mod_api::services::LeaderboardStore)
/// under table `"csx"`.
#[derive(Debug, Clone, Default)]
pub struct CsxPlayerStats {
    pub kills: i32,
    pub deaths: i32,
    pub headshots: i32,
    pub teamkills: i32,
    pub shots: i32,
    pub hits: i32,
    pub damage: i32,
    pub bodyhits: [i32; 8],
    pub name: String,
    pub authid: String,
}

/// Metadata registered via `register_plugin`.
#[derive(Debug, Clone, Default)]
pub struct PluginMeta {
    pub file_name: String,
    pub title: String,
    pub version: String,
    pub author: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct AmxxCvar {
    pub name: String,
    pub flags: i32,
    /// `None` when backed by `EngineApi`'s `CVarRegistry`, `Some` for plugin-defined cvars.
    pub local_value: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct RegisteredCommand {
    pub plugin_idx: usize,
    pub command: String,
    pub callback: String,
    pub flags: i32,
    pub info: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ScheduledTask {
    pub handle: i32,
    pub plugin_idx: usize,
    pub task_id: i32,
    pub callback: String,
    pub interval: f64,
    pub next_run: f64,
    pub remaining_repeats: Option<i32>,
    pub param_data: Vec<i32>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct RegisteredEvent {
    pub plugin_idx: usize,
    pub event_name: String,
    pub callback: String,
    pub flags: String,
}

#[derive(Debug, Clone)]
pub(crate) struct RegisteredMessage {
    pub plugin_idx: usize,
    pub msg_id: i32,
    pub callback: String,
}

#[derive(Debug, Clone)]
pub(crate) struct RegisteredHam {
    pub plugin_idx: usize,
    pub function: HamFunction,
    pub classname: String,
    pub callback: String,
    pub post: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct AmxxNativeCallFrame {
    pub raw_cells: Vec<i32>,
    pub str_snapshots: Vec<Option<String>>,
    pub str_writes: Vec<Option<(String, usize)>>,
}

/// Shared AMX Mod X host state kept separately from the plugin VMs to allow disjoint mutable
/// borrowing during native execution.
pub struct AmxxHostState {
    pub plugin_metas: Vec<PluginMeta>,
    pub services: SharedPlatformServices,
    pub lang_search_dirs: Vec<PathBuf>,
    pub external_rpc: Option<Arc<dyn AmxxExternalRpc>>,
    pub(crate) amxx_native_frames: Vec<AmxxNativeCallFrame>,
    pub(crate) cvars: Vec<AmxxCvar>,
    pub(crate) cvar_by_name: HashMap<String, usize>,
    pub(crate) clcmds: Vec<RegisteredCommand>,
    pub(crate) concmds: Vec<RegisteredCommand>,
    pub(crate) srvcmds: Vec<RegisteredCommand>,
    pub(crate) tasks: Vec<ScheduledTask>,
    pub(crate) next_task_handle: i32,
    pub(crate) events: Vec<RegisteredEvent>,
    pub(crate) messages: Vec<RegisteredMessage>,
    pub(crate) ham_hooks: Vec<RegisteredHam>,
    pub(crate) ham_param_overrides: HashMap<i32, i32>,
    pub(crate) csx_stats: BTreeMap<String, CsxPlayerStats>,
    pub(crate) slot_csx_keys: [Option<String>; 32],
    pub(crate) pending_server_cmds: Vec<String>,
    pub(crate) server_prints: Vec<String>,
    pub(crate) amx_logs: Vec<String>,
    pub(crate) current_argv: Vec<String>,
    pub(crate) connected_slots: [bool; 32],
    pub(crate) admin_flags: [i32; 32],
    pub(crate) armor_types: [i32; 32],
}

impl std::fmt::Debug for AmxxHostState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AmxxHostState")
            .field("plugin_metas", &self.plugin_metas)
            .field("cvars", &self.cvars)
            .field("tasks_len", &self.tasks.len())
            .field("ham_hooks_len", &self.ham_hooks.len())
            .finish()
    }
}

impl Default for AmxxHostState {
    fn default() -> Self {
        Self::new()
    }
}

impl AmxxHostState {
    pub fn new() -> Self {
        Self::with_services(SharedPlatformServices::new())
    }

    pub fn with_services(services: SharedPlatformServices) -> Self {
        Self {
            plugin_metas: Vec::new(),
            services,
            lang_search_dirs: Vec::new(),
            external_rpc: None,
            amxx_native_frames: Vec::new(),
            cvars: Vec::new(),
            cvar_by_name: HashMap::new(),
            clcmds: Vec::new(),
            concmds: Vec::new(),
            srvcmds: Vec::new(),
            tasks: Vec::new(),
            next_task_handle: 1,
            events: Vec::new(),
            messages: Vec::new(),
            ham_hooks: Vec::new(),
            ham_param_overrides: HashMap::new(),
            csx_stats: BTreeMap::new(),
            slot_csx_keys: std::array::from_fn(|_| None),
            pending_server_cmds: Vec::new(),
            server_prints: Vec::new(),
            amx_logs: Vec::new(),
            current_argv: Vec::new(),
            connected_slots: [false; 32],
            admin_flags: [0; 32],
            armor_types: [0; 32],
        }
    }

    /// Returns recorded `server_print` lines.
    pub fn server_prints(&self) -> &[String] {
        &self.server_prints
    }

    /// Returns recorded `log_amx` lines.
    pub fn amx_logs(&self) -> &[String] {
        &self.amx_logs
    }

    /// Sets admin flags bitmask (`get_user_flags`) for 0-based `slot`, synchronized with shared
    /// [`AdminRegistry`](cs_mod_api::services::AdminRegistry).
    pub fn set_user_flags(&mut self, slot: u8, flags: i32) {
        if let Some(f) = self.admin_flags.get_mut(usize::from(slot)) {
            *f = flags;
        }
        let mut svc = self.services.lock();
        svc.admins
            .set_slot_flags(slot, AdminFlags::from_bits(flags as u32));
    }

    /// Reads effective admin flags for `slot` from shared `AdminRegistry` (falling back to local).
    pub fn effective_user_flags(&self, slot: u8) -> i32 {
        let local = self.admin_flags.get(usize::from(slot)).copied().unwrap_or(0);
        let shared = self.services.lock().admins.player_flags(slot).bits() as i32;
        if shared != 0 && shared != AdminFlags::USER.bits() as i32 {
            shared
        } else if local != 0 {
            local
        } else {
            shared
        }
    }

    pub(crate) fn read_rpc_param(&self, idx: i32) -> i32 {
        let Ok(i) = usize::try_from(idx - 1) else {
            return 0;
        };
        if let Some(frame) = self.amxx_native_frames.last() {
            return frame.raw_cells.get(i).copied().unwrap_or(0);
        }
        let svc = self.services.lock();
        svc.rpc
            .current_call_frame()
            .and_then(|f| f.get(i))
            .map_or(0, |v| match v {
                RpcValue::Int(n) => *n,
                RpcValue::Float(f) => f.to_bits() as i32,
                _ => 0,
            })
    }

    pub(crate) fn read_rpc_param_f(&self, idx: i32) -> i32 {
        let Ok(i) = usize::try_from(idx - 1) else {
            return 0;
        };
        if let Some(frame) = self.amxx_native_frames.last() {
            return frame.raw_cells.get(i).copied().unwrap_or(0);
        }
        let svc = self.services.lock();
        svc.rpc
            .current_call_frame()
            .and_then(|f| f.get(i))
            .map_or(0, |v| match v {
                RpcValue::Float(f) => f.to_bits() as i32,
                RpcValue::Int(n) => *n,
                _ => 0,
            })
    }

    pub(crate) fn read_rpc_string(&self, idx: i32) -> String {
        let Ok(i) = usize::try_from(idx - 1) else {
            return String::new();
        };
        if let Some(frame) = self.amxx_native_frames.last() {
            if let Some(Some((written, _))) = frame.str_writes.get(i) {
                return written.clone();
            }
            if let Some(Some(snap)) = frame.str_snapshots.get(i) {
                return snap.clone();
            }
            return String::new();
        }
        let svc = self.services.lock();
        svc.rpc
            .current_call_frame()
            .and_then(|f| f.get(i))
            .and_then(|v| match v {
                RpcValue::Str(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    pub(crate) fn write_rpc_string(&mut self, idx: i32, src: &str, max_len: usize) -> i32 {
        let clipped: String = src.chars().take(max_len).collect();
        let len = clipped.len() as i32;
        let Ok(i) = usize::try_from(idx - 1) else {
            return 0;
        };
        if let Some(frame) = self.amxx_native_frames.last_mut() {
            if i < frame.str_writes.len() {
                frame.str_writes[i] = Some((clipped, max_len));
            }
            return len;
        }
        let mut svc = self.services.lock();
        if let Some(frame) = svc.rpc.current_call_frame_mut() {
            if i >= frame.len() {
                frame.resize(i + 1, RpcValue::Int(0));
            }
            frame[i] = RpcValue::Str(clipped);
        }
        len
    }

    /// Derives a stable key for `csx.inc` leaderboard storage (`*sid` -> `name` -> `slot:N`).
    pub fn csx_key_for_slot(&mut self, eng: &EngineApi<'_>, slot: u8) -> String {
        let name = eng.client_key(slot, "name").unwrap_or("").to_string();
        let authid = if eng.is_fake_client(slot) {
            format!("BOT_{slot}")
        } else if let Some(sid) = eng.client_key(slot, "*sid").filter(|s| !s.is_empty()) {
            sid.to_string()
        } else if !name.is_empty() {
            name.clone()
        } else {
            format!("slot:{slot}")
        };
        if let Some(entry) = self.slot_csx_keys.get_mut(usize::from(slot)) {
            *entry = Some(authid.clone());
        }
        let stats = self.csx_stats.entry(authid.clone()).or_default();
        if !name.is_empty() {
            stats.name = name;
        }
        stats.authid = authid.clone();
        authid
    }

    /// Updates `csx` damage counters and upserts into shared `LeaderboardStore` (`"csx"`).
    pub fn record_csx_damage(
        &mut self,
        eng: &EngineApi<'_>,
        attacker_slot: Option<u8>,
        damage: i32,
        hitplace: i32,
    ) {
        let Some(att) = attacker_slot else {
            return;
        };
        let key = self.csx_key_for_slot(eng, att);
        let (score, payload) = {
            let st = self.csx_stats.entry(key.clone()).or_default();
            st.hits = st.hits.saturating_add(1);
            st.damage = st.damage.saturating_add(damage.max(0));
            if let Ok(hb) = usize::try_from(hitplace)
                && hb < st.bodyhits.len()
            {
                st.bodyhits[hb] = st.bodyhits[hb].saturating_add(1);
            }
            let score = f64::from(st.kills - st.deaths) + f64::from(st.damage) * 0.0001;
            let payload = format!(
                "{{\"name\":\"{}\",\"k\":{},\"d\":{},\"hs\":{},\"dmg\":{}}}",
                st.name.replace('"', "'"),
                st.kills,
                st.deaths,
                st.headshots,
                st.damage
            );
            (score, payload)
        };
        let mut svc = self.services.lock();
        svc.leaderboards.upsert("csx", key, score, payload);
    }

    /// Updates `csx` kill/death counters and upserts both players into shared `LeaderboardStore` (`"csx"`).
    pub fn record_csx_death(
        &mut self,
        eng: &EngineApi<'_>,
        killer_slot: Option<u8>,
        victim_slot: u8,
        hitplace: i32,
        team_kill: bool,
    ) {
        if let Some(k) = killer_slot {
            let key = self.csx_key_for_slot(eng, k);
            let (score, payload) = {
                let st = self.csx_stats.entry(key.clone()).or_default();
                if team_kill {
                    st.teamkills = st.teamkills.saturating_add(1);
                } else {
                    st.kills = st.kills.saturating_add(1);
                    if hitplace == 1 {
                        st.headshots = st.headshots.saturating_add(1);
                    }
                }
                let score = f64::from(st.kills - st.deaths) + f64::from(st.damage) * 0.0001;
                let payload = format!(
                    "{{\"name\":\"{}\",\"k\":{},\"d\":{},\"hs\":{},\"dmg\":{}}}",
                    st.name.replace('"', "'"),
                    st.kills,
                    st.deaths,
                    st.headshots,
                    st.damage
                );
                (score, payload)
            };
            let mut svc = self.services.lock();
            svc.leaderboards.upsert("csx", key, score, payload);
        }

        let vkey = self.csx_key_for_slot(eng, victim_slot);
        let (vscore, vpayload) = {
            let st = self.csx_stats.entry(vkey.clone()).or_default();
            st.deaths = st.deaths.saturating_add(1);
            let score = f64::from(st.kills - st.deaths) + f64::from(st.damage) * 0.0001;
            let payload = format!(
                "{{\"name\":\"{}\",\"k\":{},\"d\":{},\"hs\":{},\"dmg\":{}}}",
                st.name.replace('"', "'"),
                st.kills,
                st.deaths,
                st.headshots,
                st.damage
            );
            (score, payload)
        };
        let mut svc = self.services.lock();
        svc.leaderboards.upsert("csx", vkey, vscore, vpayload);
    }

    /// Gets a cvar string from either `EngineApi` or plugin-registered AMXX cvars.
    pub fn cvar_str(&self, eng: &EngineApi<'_>, name: &str) -> Option<String> {
        if let Some(v) = eng.cvar_str(name) {
            return Some(v);
        }
        let key = name.to_ascii_lowercase();
        let idx = *self.cvar_by_name.get(&key)?;
        self.cvars.get(idx)?.local_value.clone()
    }

    /// Sets a cvar in `EngineApi` (if registered there) and/or plugin-registered AMXX cvars.
    pub fn set_cvar_str(&mut self, eng: &mut EngineApi<'_>, name: &str, value: &str) -> bool {
        let mut updated = false;
        if eng.cvar_str(name).is_some() {
            eng.set_cvar(name, value);
            updated = true;
        }
        let key = name.to_ascii_lowercase();
        if let Some(&idx) = self.cvar_by_name.get(&key)
            && let Some(cvar) = self.cvars.get_mut(idx)
            && cvar.local_value.is_some()
        {
            cvar.local_value = Some(value.to_string());
            updated = true;
        }
        updated
    }

    fn register_cvar(
        &mut self,
        eng: &EngineApi<'_>,
        name: &str,
        default_val: &str,
        flags: i32,
    ) -> i32 {
        let key = name.to_ascii_lowercase();
        if let Some(&idx) = self.cvar_by_name.get(&key) {
            return (idx + 1) as i32;
        }
        let local_value = if eng.cvar_str(&key).is_some() {
            None
        } else {
            Some(default_val.to_string())
        };
        let idx = self.cvars.len();
        self.cvars.push(AmxxCvar {
            name: key.clone(),
            flags,
            local_value,
        });
        self.cvar_by_name.insert(key, idx);
        (idx + 1) as i32
    }

    fn get_cvar_pointer(&mut self, eng: &EngineApi<'_>, name: &str) -> i32 {
        let key = name.to_ascii_lowercase();
        if let Some(&idx) = self.cvar_by_name.get(&key) {
            return (idx + 1) as i32;
        }
        if eng.cvar_str(&key).is_some() {
            let idx = self.cvars.len();
            self.cvars.push(AmxxCvar {
                name: key.clone(),
                flags: 0,
                local_value: None,
            });
            self.cvar_by_name.insert(key, idx);
            return (idx + 1) as i32;
        }
        0
    }

    fn pcvar_str(&self, eng: &EngineApi<'_>, handle: i32) -> Option<String> {
        let idx = usize::try_from(handle - 1).ok()?;
        let cvar = self.cvars.get(idx)?;
        match &cvar.local_value {
            Some(v) => Some(v.clone()),
            None => eng.cvar_str(&cvar.name),
        }
    }

    fn set_pcvar_str(&mut self, eng: &mut EngineApi<'_>, handle: i32, value: &str) {
        let Ok(idx) = usize::try_from(handle - 1) else {
            return;
        };
        let Some(cvar) = self.cvars.get_mut(idx) else {
            return;
        };
        if cvar.local_value.is_some() {
            cvar.local_value = Some(value.to_string());
        } else {
            eng.set_cvar(&cvar.name, value);
        }
    }
}

/// Native call context connecting one running [`PawnVm`] to [`AmxxHostState`], [`EngineApi`],
/// [`AmxxCsHooks`], and sibling plugins for synchronous cross-plugin natives/forwards.
pub struct AmxxNativeDispatch<'a, 'e> {
    pub plugin_idx: usize,
    pub state: &'a mut AmxxHostState,
    pub eng: &'a mut EngineApi<'e>,
    pub hooks: &'a mut dyn AmxxCsHooks,
    pub peers: &'a mut [Option<AmxxPlugin>],
}

impl NativeDispatch for AmxxNativeDispatch<'_, '_> {
    fn call_native(
        &mut self,
        vm: &mut PawnVm,
        _native_idx: usize,
        native_name: &str,
        params_addr: i32,
    ) -> Result<i32, AmxError> {
        match native_name {
            // --- Core / Registration ---
            "register_plugin" => self.native_register_plugin(vm, params_addr),
            "register_clcmd" => self.native_register_cmd(vm, params_addr, CmdKind::Client),
            "register_concmd" => self.native_register_cmd(vm, params_addr, CmdKind::Console),
            "register_srvcmd" => self.native_register_cmd(vm, params_addr, CmdKind::Server),
            "register_event" => self.native_register_event(vm, params_addr),
            "register_message" => self.native_register_message(vm, params_addr),
            "get_user_msgid" => self.native_get_user_msgid(vm, params_addr),
            "precache_model" => {
                let name = vm.param_string(params_addr, 1)?;
                Ok(self.eng.precache_model(&name).map_or(0, i32::from))
            }
            "precache_sound" => {
                let name = vm.param_string(params_addr, 1)?;
                Ok(self.eng.precache_sound(&name).map_or(0, i32::from))
            }
            "precache_generic" => {
                let name = vm.param_string(params_addr, 1)?;
                Ok(self.eng.precache_generic(&name).map_or(0, i32::from))
            }

            // --- CVars & PCVars ---
            "register_cvar" => self.native_register_cvar(vm, params_addr),
            "get_cvar_pointer" => {
                let name = vm.param_string(params_addr, 1)?;
                Ok(self.state.get_cvar_pointer(self.eng, &name))
            }
            "get_cvar_num" => {
                let name = vm.param_string(params_addr, 1)?;
                Ok(self
                    .state
                    .cvar_str(self.eng, &name)
                    .map_or(0, |s| parse_cvar_i32(&s)))
            }
            "get_cvar_float" => {
                let name = vm.param_string(params_addr, 1)?;
                let f = self
                    .state
                    .cvar_str(self.eng, &name)
                    .map_or(0.0, |s| parse_cvar_f32(&s));
                Ok(f.to_bits() as i32)
            }
            "get_cvar_string" => {
                let name = vm.param_string(params_addr, 1)?;
                let out_addr = vm.param(params_addr, 2)?;
                let max_len = vm.param(params_addr, 3)?.max(0) as usize;
                let val = self.state.cvar_str(self.eng, &name).unwrap_or_default();
                vm.set_amx_string(out_addr, &val, max_len)?;
                Ok(val.len().min(max_len) as i32)
            }
            "set_cvar_num" => {
                let name = vm.param_string(params_addr, 1)?;
                let val = vm.param(params_addr, 2)?;
                self.state.set_cvar_str(self.eng, &name, &val.to_string());
                Ok(1)
            }
            "set_cvar_float" => {
                let name = vm.param_string(params_addr, 1)?;
                let val = vm.param_f32(params_addr, 2)?;
                self.state
                    .set_cvar_str(self.eng, &name, &format_amxx_float(val));
                Ok(1)
            }
            "set_cvar_string" => {
                let name = vm.param_string(params_addr, 1)?;
                let val = vm.param_string(params_addr, 2)?;
                self.state.set_cvar_str(self.eng, &name, &val);
                Ok(1)
            }
            "get_pcvar_num" => {
                let handle = vm.param(params_addr, 1)?;
                Ok(self
                    .state
                    .pcvar_str(self.eng, handle)
                    .map_or(0, |s| parse_cvar_i32(&s)))
            }
            "get_pcvar_float" => {
                let handle = vm.param(params_addr, 1)?;
                let f = self
                    .state
                    .pcvar_str(self.eng, handle)
                    .map_or(0.0, |s| parse_cvar_f32(&s));
                Ok(f.to_bits() as i32)
            }
            "get_pcvar_string" => {
                let handle = vm.param(params_addr, 1)?;
                let out_addr = vm.param(params_addr, 2)?;
                let max_len = vm.param(params_addr, 3)?.max(0) as usize;
                let val = self.state.pcvar_str(self.eng, handle).unwrap_or_default();
                vm.set_amx_string(out_addr, &val, max_len)?;
                Ok(val.len().min(max_len) as i32)
            }
            "set_pcvar_num" => {
                let handle = vm.param(params_addr, 1)?;
                let val = vm.param(params_addr, 2)?;
                self.state
                    .set_pcvar_str(self.eng, handle, &val.to_string());
                Ok(1)
            }
            "set_pcvar_float" => {
                let handle = vm.param(params_addr, 1)?;
                let val = vm.param_f32(params_addr, 2)?;
                self.state
                    .set_pcvar_str(self.eng, handle, &format_amxx_float(val));
                Ok(1)
            }
            "set_pcvar_string" => {
                let handle = vm.param(params_addr, 1)?;
                let val = vm.param_string(params_addr, 2)?;
                self.state.set_pcvar_str(self.eng, handle, &val);
                Ok(1)
            }

            // --- Tasks ---
            "set_task" => self.native_set_task(vm, params_addr),
            "remove_task" => self.native_remove_task(vm, params_addr),
            "task_exists" => self.native_task_exists(vm, params_addr),

            // --- Printing / Server Commands / Logging (`%L` LangCatalog aware!) ---
            "client_print" => self.native_client_print(vm, params_addr),
            "console_print" => self.native_console_print(vm, params_addr),
            "server_print" => {
                let text = {
                    let svc = self.state.services.lock();
                    vm.format_amx_string_with_lang(params_addr, 1, 2, None, Some(&svc.lang))?
                };
                info!(target: "amxx", "{text}");
                self.state.server_prints.push(text);
                Ok(1)
            }
            "server_cmd" => {
                let cmd = {
                    let svc = self.state.services.lock();
                    vm.format_amx_string_with_lang(params_addr, 1, 2, None, Some(&svc.lang))?
                };
                self.state.pending_server_cmds.push(cmd);
                Ok(1)
            }
            "log_amx" => {
                let text = {
                    let svc = self.state.services.lock();
                    vm.format_amx_string_with_lang(params_addr, 1, 2, None, Some(&svc.lang))?
                };
                info!(target: "amxx_log", "{text}");
                self.state.amx_logs.push(text);
                Ok(1)
            }

            // --- Player Info & Queries ---
            "get_user_name" => self.native_get_user_name(vm, params_addr),
            "get_user_authid" => self.native_get_user_authid(vm, params_addr),
            "get_user_ip" => self.native_get_user_ip(vm, params_addr),
            "get_user_health" => {
                let id = vm.param(params_addr, 1)?;
                Ok(self
                    .slot_edict(id)
                    .and_then(|e| self.eng.ent(e))
                    .map_or(0, |v| v.health as i32))
            }
            "get_user_armor" => {
                let id = vm.param(params_addr, 1)?;
                Ok(self
                    .connected_slot(id)
                    .map_or(0, |slot| self.hooks.get_user_armor(slot, self.eng)))
            }
            "get_user_team" => self.native_get_user_team(vm, params_addr),
            "get_user_origin" => self.native_get_user_origin(vm, params_addr),
            "get_user_weapon" => self.native_get_user_weapon(vm, params_addr),
            "get_user_flags" => {
                let id = vm.param(params_addr, 1)?;
                Ok(self
                    .valid_slot(id)
                    .map_or(0, |slot| self.state.effective_user_flags(slot)))
            }
            "is_user_alive" => {
                let id = vm.param(params_addr, 1)?;
                let alive = self.connected_slot(id).is_some_and(|slot| {
                    self.eng
                        .ent(self.eng.client_edict(slot))
                        .is_some_and(|v| v.deadflag == 0 && v.health > 0.0)
                });
                Ok(i32::from(alive))
            }
            "is_user_connected" => {
                let id = vm.param(params_addr, 1)?;
                Ok(i32::from(self.connected_slot(id).is_some()))
            }
            "is_user_bot" => {
                let id = vm.param(params_addr, 1)?;
                let is_bot = self.valid_slot(id).is_some_and(|slot| {
                    self.eng.is_fake_client(slot)
                        || self
                            .eng
                            .ent(self.eng.client_edict(slot))
                            .is_some_and(|v| (v.flags & FL_FAKECLIENT) != 0)
                });
                Ok(i32::from(is_bot))
            }
            "is_user_hltv" => {
                let id = vm.param(params_addr, 1)?;
                let is_hltv = self.valid_slot(id).is_some_and(|slot| {
                    self.eng
                        .ent(self.eng.client_edict(slot))
                        .is_some_and(|v| (v.flags & FL_PROXY) != 0)
                });
                Ok(i32::from(is_hltv))
            }
            "get_players" => self.native_get_players(vm, params_addr),
            "get_maxplayers" => Ok(i32::from(self.eng.max_clients())),
            "get_gametime" => Ok((self.eng.time() as f32).to_bits() as i32),

            // --- Command Args ---
            "read_argc" => Ok(self.state.current_argv.len() as i32),
            "read_argv" => {
                let idx = vm.param(params_addr, 1)?;
                let out_addr = vm.param(params_addr, 2)?;
                let max_len = vm.param(params_addr, 3)?.max(0) as usize;
                let arg = usize::try_from(idx)
                    .ok()
                    .and_then(|i| self.state.current_argv.get(i))
                    .map_or("", String::as_str);
                vm.set_amx_string(out_addr, arg, max_len)?;
                Ok(arg.len().min(max_len) as i32)
            }
            "read_args" => {
                let out_addr = vm.param(params_addr, 1)?;
                let max_len = vm.param(params_addr, 2)?.max(0) as usize;
                let joined = if self.state.current_argv.len() > 1 {
                    self.state.current_argv[1..].join(" ")
                } else {
                    String::new()
                };
                vm.set_amx_string(out_addr, &joined, max_len)?;
                Ok(joined.len().min(max_len) as i32)
            }

            // --- String / Conversion (`%L` LangCatalog aware!) ---
            "format" | "formatex" => {
                let out_addr = vm.param(params_addr, 1)?;
                let max_len = vm.param(params_addr, 2)?.max(0) as usize;
                let text = {
                    let svc = self.state.services.lock();
                    vm.format_amx_string_with_lang(params_addr, 3, 4, None, Some(&svc.lang))?
                };
                let written = vm.set_amx_string(out_addr, &text, max_len)?;
                Ok(written as i32)
            }
            "copy" => {
                let dest_addr = vm.param(params_addr, 1)?;
                let max_len = vm.param(params_addr, 2)?.max(0) as usize;
                let src = vm.param_string(params_addr, 3)?;
                let written = vm.set_amx_string(dest_addr, &src, max_len)?;
                Ok(written as i32)
            }
            "equal" => self.native_equal(vm, params_addr, false),
            "equali" => self.native_equal(vm, params_addr, true),
            "contain" => self.native_contain(vm, params_addr, false),
            "containi" => self.native_contain(vm, params_addr, true),
            "strlen" => {
                let s = vm.param_string(params_addr, 1)?;
                Ok(s.len() as i32)
            }
            "str_to_num" => {
                let s = vm.param_string(params_addr, 1)?;
                Ok(parse_cvar_i32(&s))
            }
            "num_to_str" => {
                let num = vm.param(params_addr, 1)?;
                let out_addr = vm.param(params_addr, 2)?;
                let max_len = vm.param(params_addr, 3)?.max(0) as usize;
                let s = num.to_string();
                let written = vm.set_amx_string(out_addr, &s, max_len)?;
                Ok(written as i32)
            }

            // --- Float Natives ---
            "float" => {
                let val = vm.param(params_addr, 1)?;
                Ok((val as f32).to_bits() as i32)
            }
            "floatround" => {
                let f = vm.param_f32(params_addr, 1)?;
                let argc = vm.param_count(params_addr)?;
                let method = if argc >= 2 {
                    vm.param(params_addr, 2)?
                } else {
                    0
                };
                let rounded = match method {
                    1 => f.floor(),
                    2 => f.ceil(),
                    3 => f.trunc(),
                    _ => f.round(),
                };
                Ok(rounded as i32)
            }
            "floatadd" => {
                let a = vm.param_f32(params_addr, 1)?;
                let b = vm.param_f32(params_addr, 2)?;
                Ok((a + b).to_bits() as i32)
            }
            "floatsub" => {
                let a = vm.param_f32(params_addr, 1)?;
                let b = vm.param_f32(params_addr, 2)?;
                Ok((a - b).to_bits() as i32)
            }
            "floatmul" => {
                let a = vm.param_f32(params_addr, 1)?;
                let b = vm.param_f32(params_addr, 2)?;
                Ok((a * b).to_bits() as i32)
            }
            "floatdiv" => {
                let a = vm.param_f32(params_addr, 1)?;
                let b = vm.param_f32(params_addr, 2)?;
                Ok((a / b).to_bits() as i32)
            }
            "floatcmp" => {
                let a = vm.param_f32(params_addr, 1)?;
                let b = vm.param_f32(params_addr, 2)?;
                Ok(if a == b {
                    0
                } else if a > b {
                    1
                } else {
                    -1
                })
            }

            // --- Module: newmenus.inc (backed by shared MenuManager) ---
            "menu_create" => self.native_menu_create(vm, params_addr),
            "menu_additem" => self.native_menu_additem(vm, params_addr),
            "menu_addblank" | "menu_addblank2" => self.native_menu_addblank(vm, params_addr),
            "menu_addtext" | "menu_addtext2" => self.native_menu_addtext(vm, params_addr),
            "menu_setprop" => self.native_menu_setprop(vm, params_addr),
            "menu_display" => self.native_menu_display(vm, params_addr),
            "menu_item_getinfo" => self.native_menu_item_getinfo(vm, params_addr),
            "menu_destroy" => self.native_menu_destroy(vm, params_addr),
            "player_menu_info" => self.native_player_menu_info(vm, params_addr),

            // --- Module: lang.inc (backed by shared LangCatalog) ---
            "register_dictionary" => self.native_register_dictionary(vm, params_addr),
            "LookupLangKey" => self.native_lookup_lang_key(vm, params_addr),

            // --- Module: csx.inc (backed by shared LeaderboardStore) ---
            "get_stats" => self.native_get_stats(vm, params_addr),
            "get_user_stats" => self.native_get_user_stats(vm, params_addr),
            "get_statsnum" => {
                let svc = self.state.services.lock();
                Ok(svc.leaderboards.count("csx") as i32)
            }

            // --- Module: nvault.inc (backed by shared NVaultStore) ---
            "nvault_open" => {
                let name = vm.param_string(params_addr, 1)?;
                let mut svc = self.state.services.lock();
                Ok(svc.nvault.open(&name))
            }
            "nvault_get" => self.native_nvault_get(vm, params_addr),
            "nvault_lookup" => self.native_nvault_lookup(vm, params_addr),
            "nvault_set" => {
                let handle = vm.param(params_addr, 1)?;
                let key = vm.param_string(params_addr, 2)?;
                let val = vm.param_string(params_addr, 3)?;
                let now_ts = (self.eng.time().round() as i64).max(1);
                let mut svc = self.state.services.lock();
                Ok(i32::from(svc.nvault.set(handle, key, val, now_ts)))
            }
            "nvault_pset" => {
                let handle = vm.param(params_addr, 1)?;
                let key = vm.param_string(params_addr, 2)?;
                let val = vm.param_string(params_addr, 3)?;
                let mut svc = self.state.services.lock();
                Ok(i32::from(svc.nvault.pset(handle, key, val)))
            }
            "nvault_remove" => {
                let handle = vm.param(params_addr, 1)?;
                let key = vm.param_string(params_addr, 2)?;
                let mut svc = self.state.services.lock();
                Ok(i32::from(svc.nvault.remove(handle, &key)))
            }
            "nvault_prune" => {
                let handle = vm.param(params_addr, 1)?;
                let start = i64::from(vm.param(params_addr, 2)?);
                let end = i64::from(vm.param(params_addr, 3)?);
                let mut svc = self.state.services.lock();
                Ok(svc.nvault.prune(handle, start, end) as i32)
            }
            "nvault_close" => {
                let handle = vm.param(params_addr, 1)?;
                let mut svc = self.state.services.lock();
                Ok(i32::from(svc.nvault.close(handle)))
            }

            // --- Module: cstrike ---
            "cs_get_user_money" => {
                let id = vm.param(params_addr, 1)?;
                Ok(self
                    .valid_slot(id)
                    .map_or(0, |slot| self.hooks.get_user_money(slot)))
            }
            "cs_set_user_money" => {
                let id = vm.param(params_addr, 1)?;
                let amount = vm.param(params_addr, 2)?;
                let argc = vm.param_count(params_addr)?;
                let flash = if argc >= 3 {
                    vm.param(params_addr, 3)? != 0
                } else {
                    true
                };
                if let Some(slot) = self.valid_slot(id) {
                    self.hooks.set_user_money(slot, amount);
                    let _ = self.eng.send_usermsg(
                        MsgDest::One(slot),
                        &CsUserMsg::Money { amount, flash },
                    );
                    Ok(1)
                } else {
                    Ok(0)
                }
            }
            "cs_get_user_team" => {
                let id = vm.param(params_addr, 1)?;
                let argc = vm.param_count(params_addr)?;
                let team = self
                    .valid_slot(id)
                    .map_or(0, |slot| self.hooks.get_user_team(slot));
                if argc >= 2 {
                    let model_addr = vm.param(params_addr, 2)?;
                    let _ = vm.write_cell(model_addr, 0);
                }
                Ok(team)
            }
            "cs_set_user_team" => {
                let id = vm.param(params_addr, 1)?;
                let team = vm.param(params_addr, 2)?;
                if let Some(slot) = self.valid_slot(id) {
                    self.hooks.set_user_team(slot, team);
                    Ok(1)
                } else {
                    Ok(0)
                }
            }
            "cs_get_user_armor" => {
                let id = vm.param(params_addr, 1)?;
                let argc = vm.param_count(params_addr)?;
                if let Some(slot) = self.valid_slot(id) {
                    let armor = self.hooks.get_user_armor(slot, self.eng);
                    if argc >= 2 {
                        let type_addr = vm.param(params_addr, 2)?;
                        let armor_type = if armor <= 0 {
                            CS_ARMOR_NONE
                        } else {
                            self.state.armor_types[usize::from(slot)].max(CS_ARMOR_KEVLAR)
                        };
                        vm.write_cell(type_addr, armor_type)?;
                    }
                    Ok(armor)
                } else {
                    Ok(0)
                }
            }
            "cs_set_user_armor" => {
                let id = vm.param(params_addr, 1)?;
                let armor = vm.param(params_addr, 2)?;
                let armor_type = vm.param(params_addr, 3)?;
                if let Some(slot) = self.valid_slot(id) {
                    self.state.armor_types[usize::from(slot)] = armor_type;
                    self.hooks
                        .set_user_armor(slot, self.eng, armor, armor_type);
                    Ok(1)
                } else {
                    Ok(0)
                }
            }
            "cs_get_user_deaths" => {
                let id = vm.param(params_addr, 1)?;
                Ok(self
                    .valid_slot(id)
                    .map_or(0, |slot| self.hooks.get_user_deaths(slot)))
            }
            "cs_set_user_deaths" => {
                let id = vm.param(params_addr, 1)?;
                let deaths = vm.param(params_addr, 2)?;
                if let Some(slot) = self.valid_slot(id) {
                    self.hooks.set_user_deaths(slot, deaths);
                    Ok(1)
                } else {
                    Ok(0)
                }
            }

            // --- Module: fun ---
            "set_user_health" => {
                let id = vm.param(params_addr, 1)?;
                let hp = vm.param(params_addr, 2)?;
                if let Some(edict) = self.slot_edict(id)
                    && let Some(vars) = self.eng.ent_mut(edict)
                {
                    vars.health = hp as f32;
                    Ok(1)
                } else {
                    Ok(0)
                }
            }
            "set_user_gravity" => {
                let id = vm.param(params_addr, 1)?;
                let argc = vm.param_count(params_addr)?;
                let grav = if argc >= 2 {
                    vm.param_f32(params_addr, 2)?
                } else {
                    1.0
                };
                if let Some(edict) = self.slot_edict(id)
                    && let Some(vars) = self.eng.ent_mut(edict)
                {
                    vars.gravity = grav;
                    Ok(1)
                } else {
                    Ok(0)
                }
            }
            "set_user_maxspeed" => {
                let id = vm.param(params_addr, 1)?;
                let argc = vm.param_count(params_addr)?;
                let speed = if argc >= 2 {
                    vm.param_f32(params_addr, 2)?
                } else {
                    -1.0
                };
                if let Some(edict) = self.slot_edict(id)
                    && let Some(vars) = self.eng.ent_mut(edict)
                {
                    vars.max_speed = speed;
                    Ok(1)
                } else {
                    Ok(0)
                }
            }
            "give_item" => {
                let id = vm.param(params_addr, 1)?;
                let item = vm.param_string(params_addr, 2)?;
                if let Some(slot) = self.valid_slot(id) {
                    Ok(i32::from(self.hooks.give_item(slot, self.eng, &item)))
                } else {
                    Ok(0)
                }
            }
            "strip_user_weapons" => {
                let id = vm.param(params_addr, 1)?;
                if let Some(slot) = self.valid_slot(id) {
                    self.hooks.strip_user_weapons(slot, self.eng);
                    Ok(1)
                } else {
                    Ok(0)
                }
            }
            "set_user_godmode" => {
                let id = vm.param(params_addr, 1)?;
                let argc = vm.param_count(params_addr)?;
                let god = if argc >= 2 {
                    vm.param(params_addr, 2)? != 0
                } else {
                    false
                };
                if let Some(edict) = self.slot_edict(id)
                    && let Some(vars) = self.eng.ent_mut(edict)
                {
                    if god {
                        vars.flags |= FL_GODMODE;
                    } else {
                        vars.flags &= !FL_GODMODE;
                    }
                    Ok(1)
                } else {
                    Ok(0)
                }
            }

            // --- Module: fakemeta ---
            "pev" => self.native_pev(vm, params_addr),
            "set_pev" => self.native_set_pev(vm, params_addr),

            // --- Module: engine ---
            "entity_get_float" => {
                let ent_idx = vm.param(params_addr, 1)?;
                let key = vm.param(params_addr, 2)?;
                let Some(eid) = self.resolve_edict(ent_idx) else {
                    return Ok(0);
                };
                let Some(vars) = self.eng.ent(eid) else {
                    return Ok(0);
                };
                let val = read_ev_float(vars, key).unwrap_or(0.0);
                Ok(val.to_bits() as i32)
            }
            "entity_get_int" => {
                let ent_idx = vm.param(params_addr, 1)?;
                let key = vm.param(params_addr, 2)?;
                let Some(eid) = self.resolve_edict(ent_idx) else {
                    return Ok(0);
                };
                let Some(vars) = self.eng.ent(eid) else {
                    return Ok(0);
                };
                Ok(read_ev_int(vars, key).unwrap_or(0))
            }
            "entity_get_vector" => {
                let ent_idx = vm.param(params_addr, 1)?;
                let key = vm.param(params_addr, 2)?;
                let out_addr = vm.param(params_addr, 3)?;
                let Some(eid) = self.resolve_edict(ent_idx) else {
                    return Ok(0);
                };
                let Some(vars) = self.eng.ent(eid) else {
                    return Ok(0);
                };
                let v = read_ev_vec(vars, key).unwrap_or(Vec3::ZERO);
                write_amx_vec3(vm, out_addr, v)?;
                Ok(1)
            }
            "entity_get_string" => {
                let ent_idx = vm.param(params_addr, 1)?;
                let key = vm.param(params_addr, 2)?;
                let out_addr = vm.param(params_addr, 3)?;
                let max_len = vm.param(params_addr, 4)?.max(0) as usize;
                let Some(eid) = self.resolve_edict(ent_idx) else {
                    return Ok(0);
                };
                let Some(vars) = self.eng.ent(eid) else {
                    return Ok(0);
                };
                let s = read_ev_string(vars, key).unwrap_or("");
                let written = vm.set_amx_string(out_addr, s, max_len)?;
                Ok(written as i32)
            }
            "entity_set_float" => {
                let ent_idx = vm.param(params_addr, 1)?;
                let key = vm.param(params_addr, 2)?;
                let val = vm.param_f32(params_addr, 3)?;
                let Some(eid) = self.resolve_edict(ent_idx) else {
                    return Ok(0);
                };
                let Some(vars) = self.eng.ent_mut(eid) else {
                    return Ok(0);
                };
                Ok(i32::from(write_ev_float(vars, key, val)))
            }
            "entity_set_int" => {
                let ent_idx = vm.param(params_addr, 1)?;
                let key = vm.param(params_addr, 2)?;
                let val = vm.param(params_addr, 3)?;
                let Some(eid) = self.resolve_edict(ent_idx) else {
                    return Ok(0);
                };
                let Some(vars) = self.eng.ent_mut(eid) else {
                    return Ok(0);
                };
                Ok(i32::from(write_ev_int(vars, key, val)))
            }
            "entity_set_vector" => {
                let ent_idx = vm.param(params_addr, 1)?;
                let key = vm.param(params_addr, 2)?;
                let vec_addr = vm.param(params_addr, 3)?;
                let v = read_amx_vec3(vm, vec_addr)?;
                let Some(eid) = self.resolve_edict(ent_idx) else {
                    return Ok(0);
                };
                let Some(vars) = self.eng.ent_mut(eid) else {
                    return Ok(0);
                };
                Ok(i32::from(write_ev_vec(vars, key, v)))
            }
            "entity_set_string" => {
                let ent_idx = vm.param(params_addr, 1)?;
                let key = vm.param(params_addr, 2)?;
                let val = vm.param_string(params_addr, 3)?;
                let Some(eid) = self.resolve_edict(ent_idx) else {
                    return Ok(0);
                };
                let Some(vars) = self.eng.ent_mut(eid) else {
                    return Ok(0);
                };
                Ok(i32::from(write_ev_string(vars, key, &val)))
            }

            // --- Module: hamsandwich ---
            "RegisterHam" => self.native_register_ham(vm, params_addr),
            "SetHamParamFloat" => {
                let which = vm.param(params_addr, 1)?;
                let val_bits = vm.param(params_addr, 2)?;
                self.state.ham_param_overrides.insert(which, val_bits);
                Ok(1)
            }
            "SetHamParamInteger" | "SetHamParamEntity" => {
                let which = vm.param(params_addr, 1)?;
                let val = vm.param(params_addr, 2)?;
                self.state.ham_param_overrides.insert(which, val);
                Ok(1)
            }
            "ExecuteHam" => self.native_execute_ham(vm, params_addr, false),
            "ExecuteHamB" => self.native_execute_ham(vm, params_addr, true),

            // --- Module: reapi subset ---
            "rg_give_item" => self.native_rg_give_item(vm, params_addr),
            "rg_remove_all_items" => {
                let id = vm.param(params_addr, 1)?;
                let argc = vm.param_count(params_addr)?;
                let remove_suit = argc >= 2 && vm.param(params_addr, 2)? != 0;
                if let Some(slot) = self.valid_slot(id) {
                    self.hooks.remove_all_items(slot, self.eng, remove_suit);
                    Ok(1)
                } else {
                    Ok(0)
                }
            }
            "rg_set_user_armor" => {
                let id = vm.param(params_addr, 1)?;
                let armor = vm.param(params_addr, 2)?;
                let armor_type = vm.param(params_addr, 3)?;
                if let Some(slot) = self.valid_slot(id) {
                    self.state.armor_types[usize::from(slot)] = armor_type;
                    self.hooks.set_user_armor(slot, self.eng, armor, armor_type);
                    Ok(1)
                } else {
                    Ok(0)
                }
            }
            "rg_add_account" => self.native_rg_add_account(vm, params_addr),
            "rg_round_respawn" => {
                let id = vm.param(params_addr, 1)?;
                if let Some(slot) = self.valid_slot(id) {
                    Ok(i32::from(self.hooks.round_respawn(slot, self.eng)))
                } else {
                    Ok(0)
                }
            }
            "get_member" => {
                let ent_idx = vm.param(params_addr, 1)?;
                let member = vm.param(params_addr, 2)?;
                let Ok(eid) = u16::try_from(ent_idx) else {
                    return Ok(0);
                };
                Ok(self.hooks.get_member(eid, self.eng, member))
            }
            "set_member" => {
                let ent_idx = vm.param(params_addr, 1)?;
                let member = vm.param(params_addr, 2)?;
                let val = vm.param(params_addr, 3)?;
                let Ok(eid) = u16::try_from(ent_idx) else {
                    return Ok(0);
                };
                Ok(i32::from(self.hooks.set_member(eid, self.eng, member, val)))
            }

            // --- Dynamic Inter-Plugin Natives & Multi-Forwards (bridged into shared RpcBus!) ---
            "register_native" => self.native_register_native(vm, params_addr),
            "get_param" => {
                let idx = vm.param(params_addr, 1)?;
                Ok(self.state.read_rpc_param(idx))
            }
            "get_param_f" => {
                let idx = vm.param(params_addr, 1)?;
                Ok(self.state.read_rpc_param_f(idx))
            }
            "get_string" => {
                let idx = vm.param(params_addr, 1)?;
                let out_addr = vm.param(params_addr, 2)?;
                let max_len = vm.param(params_addr, 3)?.max(0) as usize;
                let s = self.state.read_rpc_string(idx);
                let written = vm.set_amx_string(out_addr, &s, max_len)?;
                Ok(written as i32)
            }
            "set_string" => {
                let idx = vm.param(params_addr, 1)?;
                let src = vm.param_string(params_addr, 2)?;
                let argc = vm.param_count(params_addr)?;
                let max_len = if argc >= 3 {
                    vm.param(params_addr, 3)?.max(0) as usize
                } else {
                    src.len()
                };
                Ok(self.state.write_rpc_string(idx, &src, max_len))
            }
            "CreateMultiForward" => self.native_create_multi_forward(vm, params_addr),
            "ExecuteForward" => self.native_execute_forward(vm, params_addr),
            "DestroyForward" => {
                let handle = vm.param(params_addr, 1)?;
                let mut svc = self.state.services.lock();
                Ok(i32::from(svc.rpc.destroy_forward(handle)))
            }

            // --- Dynamic Cross-Plugin Native Fallback (AMXX <-> AMXX and AMXX <-> WASM!) ---
            other => {
                let resolved = {
                    let svc = self.state.services.lock();
                    svc.rpc.resolve_fn(other).cloned()
                };
                if let Some(entry) = resolved {
                    return self.invoke_dynamic_native(vm, params_addr, entry);
                }
                warn!(native = other, "unimplemented AMXX native called");
                Err(AmxError::InvalidNative(other.to_string()))
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CmdKind {
    Client,
    Console,
    Server,
}

impl AmxxNativeDispatch<'_, '_> {
    fn valid_slot(&self, player_id: i32) -> Option<u8> {
        if player_id >= 1 && player_id <= i32::from(self.eng.max_clients()) {
            Some((player_id - 1) as u8)
        } else {
            None
        }
    }

    fn connected_slot(&self, player_id: i32) -> Option<u8> {
        let slot = self.valid_slot(player_id)?;
        if self.eng.is_client_active(slot) || self.state.connected_slots[usize::from(slot)] {
            Some(slot)
        } else {
            None
        }
    }

    fn slot_edict(&self, player_id: i32) -> Option<EdictId> {
        self.valid_slot(player_id).map(|s| self.eng.client_edict(s))
    }

    fn resolve_edict(&self, index: i32) -> Option<EdictId> {
        if index < 0 || index > i32::from(u16::MAX) {
            return None;
        }
        let idx = index as u16;
        if idx == 0 {
            return Some(self.eng.world());
        }
        if idx <= u16::from(self.eng.max_clients()) {
            return Some(self.eng.client_edict((idx - 1) as u8));
        }
        self.eng.entity_ids().into_iter().find(|id| id.index == idx)
    }

    fn plugin_owner_id(&self) -> String {
        self.state
            .plugin_metas
            .get(self.plugin_idx)
            .map(|m| m.file_name.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("amxx:{}", self.plugin_idx))
    }

    fn invoke_target_plugin(
        &mut self,
        current_vm: &mut PawnVm,
        target_idx: usize,
        func_name: &str,
        args: &[i32],
    ) -> Result<Option<i32>, AmxError> {
        if target_idx == self.plugin_idx {
            let Some(pub_idx) = current_vm.find_public(func_name) else {
                return Ok(None);
            };
            let mut nested = AmxxNativeDispatch {
                plugin_idx: self.plugin_idx,
                state: self.state,
                eng: self.eng,
                hooks: self.hooks,
                peers: self.peers,
            };
            current_vm
                .exec(ExecEntry::Public(pub_idx), args, &mut nested)
                .map(Some)
        } else {
            invoke_plugin_in_slots(
                self.peers,
                self.state,
                target_idx,
                self.eng,
                self.hooks,
                func_name,
                args,
            )
        }
    }

    fn native_register_plugin(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let title = vm.param_string(params_addr, 1)?;
        let version = vm.param_string(params_addr, 2)?;
        let author = vm.param_string(params_addr, 3)?;
        if let Some(meta) = self.state.plugin_metas.get_mut(self.plugin_idx) {
            meta.title = title;
            meta.version = version;
            meta.author = author;
        }
        Ok(self.plugin_idx as i32)
    }

    fn native_register_cmd(
        &mut self,
        vm: &mut PawnVm,
        params_addr: i32,
        kind: CmdKind,
    ) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let command = vm.param_string(params_addr, 1)?.to_ascii_lowercase();
        let callback = vm.param_string(params_addr, 2)?;
        let flags = if argc >= 3 {
            vm.param(params_addr, 3)?
        } else {
            -1
        };
        let info = if argc >= 4 {
            vm.param_string(params_addr, 4)?
        } else {
            String::new()
        };
        let entry = RegisteredCommand {
            plugin_idx: self.plugin_idx,
            command,
            callback,
            flags,
            info,
        };
        let list = match kind {
            CmdKind::Client => &mut self.state.clcmds,
            CmdKind::Console => &mut self.state.concmds,
            CmdKind::Server => &mut self.state.srvcmds,
        };
        list.push(entry);
        Ok(list.len() as i32)
    }

    fn native_register_cvar(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let name = vm.param_string(params_addr, 1)?;
        let default_val = vm.param_string(params_addr, 2)?;
        let flags = if argc >= 3 {
            vm.param(params_addr, 3)?
        } else {
            0
        };
        Ok(self.state.register_cvar(self.eng, &name, &default_val, flags))
    }

    fn native_register_event(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let event_name = vm.param_string(params_addr, 1)?;
        let callback = vm.param_string(params_addr, 2)?;
        let flags = vm.param_string(params_addr, 3)?;
        self.state.events.push(RegisteredEvent {
            plugin_idx: self.plugin_idx,
            event_name,
            callback,
            flags,
        });
        Ok(self.state.events.len() as i32)
    }

    fn native_register_message(
        &mut self,
        vm: &mut PawnVm,
        params_addr: i32,
    ) -> Result<i32, AmxError> {
        let msg_id = vm.param(params_addr, 1)?;
        let callback = vm.param_string(params_addr, 2)?;
        self.state.messages.push(RegisteredMessage {
            plugin_idx: self.plugin_idx,
            msg_id,
            callback,
        });
        Ok(self.state.messages.len() as i32)
    }

    fn native_get_user_msgid(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let name = vm.param_string(params_addr, 1)?;
        let reg = cs_proto48::usermsg::UserMsgRegistry::cs16_default();
        Ok(reg.id_for_name(&name).map_or(0, i32::from))
    }

    // --- newmenus.inc ---
    fn native_menu_create(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let title = vm.param_string(params_addr, 1)?;
        let handler = vm.param_string(params_addr, 2)?;
        let owner_id = self.plugin_owner_id();
        let spec = MenuCreateSpec {
            title,
            callback_name: handler,
            ..Default::default()
        };
        let mut svc = self.state.services.lock();
        Ok(svc.menus.create_menu(
            owner_id,
            PluginRuntimeKind::Amxx,
            self.plugin_idx,
            spec,
        ) as i32)
    }

    fn native_menu_additem(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let menu_id = u32::try_from(vm.param(params_addr, 1)?).unwrap_or(0);
        let name = vm.param_string(params_addr, 2)?;
        let info = if argc >= 3 {
            vm.param_string(params_addr, 3)?
        } else {
            String::new()
        };
        let mut svc = self.state.services.lock();
        Ok(i32::from(
            svc.menus
                .add_item(menu_id, MenuItemSpec::selectable(name, info, true)),
        ))
    }

    fn native_menu_addblank(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let menu_id = u32::try_from(vm.param(params_addr, 1)?).unwrap_or(0);
        let mut svc = self.state.services.lock();
        Ok(i32::from(svc.menus.add_item(menu_id, MenuItemSpec::blank())))
    }

    fn native_menu_addtext(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let menu_id = u32::try_from(vm.param(params_addr, 1)?).unwrap_or(0);
        let text = vm.param_string(params_addr, 2)?;
        let mut svc = self.state.services.lock();
        Ok(i32::from(
            svc.menus.add_item(menu_id, MenuItemSpec::text(text)),
        ))
    }

    fn native_menu_setprop(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let menu_id = u32::try_from(vm.param(params_addr, 1)?).unwrap_or(0);
        let prop = vm.param(params_addr, 2)?;
        let is_str = matches!(
            prop,
            mprop::BACKNAME | mprop::NEXTNAME | mprop::EXITNAME | mprop::TITLE | mprop::NUMBER_COLOR
        );
        let ok = if is_str {
            let val = vm.param_string(params_addr, 3)?;
            let mut svc = self.state.services.lock();
            svc.menus.set_prop_str(menu_id, prop, val)
        } else {
            let val = vm.param(params_addr, 3)?;
            let mut svc = self.state.services.lock();
            svc.menus.set_prop_int(menu_id, prop, val)
        };
        Ok(i32::from(ok))
    }

    fn native_menu_display(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let id = vm.param(params_addr, 1)?;
        let menu_id = u32::try_from(vm.param(params_addr, 2)?).unwrap_or(0);
        let page = if argc >= 3 {
            vm.param(params_addr, 3)?.max(0).min(i32::from(u16::MAX)) as u16
        } else {
            0
        };
        let time_sec = if argc >= 4 {
            vm.param(params_addr, 4)?.clamp(-1, 127) as i8
        } else {
            -1
        };
        let Some(slot) = self.valid_slot(id) else {
            return Ok(0);
        };
        let rendered = {
            let mut svc = self.state.services.lock();
            svc.menus.display(slot, menu_id, page, time_sec)
        };
        if let Some(r) = rendered {
            send_amxx_rendered_menu(self.eng, &r);
            Ok(1)
        } else {
            Ok(0)
        }
    }

    fn native_menu_item_getinfo(
        &mut self,
        vm: &mut PawnVm,
        params_addr: i32,
    ) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let menu_id = u32::try_from(vm.param(params_addr, 1)?).unwrap_or(0);
        let item_idx = vm.param(params_addr, 2)?;
        let Ok(idx) = usize::try_from(item_idx) else {
            return Ok(0);
        };
        let item = {
            let svc = self.state.services.lock();
            svc.menus.selectable_item(menu_id, idx).cloned()
        };
        let Some(item) = item else {
            return Ok(0);
        };
        if argc >= 3 {
            let access_addr = vm.param(params_addr, 3)?;
            let _ = vm.write_cell(access_addr, 0);
        }
        if argc >= 5 {
            let info_addr = vm.param(params_addr, 4)?;
            let info_len = vm.param(params_addr, 5)?.max(0) as usize;
            if info_len > 0 {
                vm.set_amx_string(info_addr, &item.info, info_len)?;
            }
        }
        if argc >= 7 {
            let name_addr = vm.param(params_addr, 6)?;
            let name_len = vm.param(params_addr, 7)?.max(0) as usize;
            if name_len > 0 {
                vm.set_amx_string(name_addr, &item.label, name_len)?;
            }
        }
        if argc >= 8 {
            let cb_addr = vm.param(params_addr, 8)?;
            let _ = vm.write_cell(cb_addr, -1);
        }
        Ok(1)
    }

    fn native_menu_destroy(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let menu_id = u32::try_from(vm.param(params_addr, 1)?).unwrap_or(0);
        let mut svc = self.state.services.lock();
        Ok(i32::from(MenuManager::destroy_menu(&mut svc.menus, menu_id)))
    }

    fn native_player_menu_info(
        &mut self,
        vm: &mut PawnVm,
        params_addr: i32,
    ) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let id = vm.param(params_addr, 1)?;
        let Some(slot) = self.valid_slot(id) else {
            return Ok(0);
        };
        let active = {
            let svc = self.state.services.lock();
            svc.menus.player_menu_info(slot)
        };
        let Some((menu_id, page)) = active else {
            if argc >= 2 {
                let _ = vm.write_cell(vm.param(params_addr, 2)?, 0);
            }
            if argc >= 3 {
                let _ = vm.write_cell(vm.param(params_addr, 3)?, -1);
            }
            return Ok(0);
        };
        if argc >= 2 {
            let _ = vm.write_cell(vm.param(params_addr, 2)?, 0);
        }
        if argc >= 3 {
            let _ = vm.write_cell(vm.param(params_addr, 3)?, menu_id as i32);
        }
        if argc >= 4 {
            let _ = vm.write_cell(vm.param(params_addr, 4)?, i32::from(page));
        }
        Ok(1)
    }

    // --- lang.inc ---
    fn native_register_dictionary(
        &mut self,
        vm: &mut PawnVm,
        params_addr: i32,
    ) -> Result<i32, AmxError> {
        let filename = vm.param_string(params_addr, 1)?;
        let mut loaded_content = None;
        for dir in &self.state.lang_search_dirs {
            let candidate = dir.join(&filename);
            if let Ok(text) = std::fs::read_to_string(&candidate) {
                loaded_content = Some((filename.ends_with(".toml"), text));
                break;
            }
        }
        let mut svc = self.state.services.lock();
        if let Some((is_toml, text)) = loaded_content {
            if is_toml {
                svc.lang.load_toml_str(&text);
            } else {
                svc.lang.load_amxx_txt_str(&text);
            }
        }
        svc.lang.mark_dictionary_loaded(&filename);
        Ok(1)
    }

    fn native_lookup_lang_key(
        &mut self,
        vm: &mut PawnVm,
        params_addr: i32,
    ) -> Result<i32, AmxError> {
        let out_addr = vm.param(params_addr, 1)?;
        let out_size = vm.param(params_addr, 2)?.max(0) as usize;
        let key = vm.param_string(params_addr, 3)?;
        let id = vm.param(params_addr, 4)?;
        let slot = if id == LANG_SERVER || id == LANG_PLAYER {
            None
        } else {
            self.valid_slot(id)
        };
        let found = {
            let svc = self.state.services.lock();
            let lang = svc.lang.player_lang(slot);
            svc.lang.lookup_key(lang, &key).map(str::to_string)
        };
        if let Some(val) = found {
            vm.set_amx_string(out_addr, &val, out_size)?;
            Ok(1)
        } else {
            Ok(0)
        }
    }

    // --- csx.inc ---
    fn native_get_stats(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let index = vm.param(params_addr, 1)?;
        let stats_addr = vm.param(params_addr, 2)?;
        let bodyhits_addr = vm.param(params_addr, 3)?;
        let Ok(idx0) = usize::try_from(index) else {
            return Ok(0);
        };
        let (entry_opt, total) = {
            let svc = self.state.services.lock();
            (
                svc.leaderboards.get_by_rank("csx", idx0 + 1),
                svc.leaderboards.count("csx"),
            )
        };
        let Some(entry) = entry_opt else {
            return Ok(0);
        };
        let csx = self.state.csx_stats.get(&entry.key).cloned().unwrap_or_else(|| {
            CsxPlayerStats {
                kills: entry.score.round() as i32,
                name: entry.key.clone(),
                authid: entry.key.clone(),
                ..Default::default()
            }
        });
        write_csx_arrays(vm, stats_addr, bodyhits_addr, &csx)?;
        if argc >= 5 {
            let name_addr = vm.param(params_addr, 4)?;
            let name_len = vm.param(params_addr, 5)?.max(0) as usize;
            if name_len > 0 {
                vm.set_amx_string(name_addr, &csx.name, name_len)?;
            }
        }
        if argc >= 7 {
            let auth_addr = vm.param(params_addr, 6)?;
            let auth_len = vm.param(params_addr, 7)?.max(0) as usize;
            if auth_len > 0 {
                vm.set_amx_string(auth_addr, &csx.authid, auth_len)?;
            }
        }
        let next = idx0 + 1;
        Ok(if next < total { next as i32 } else { 0 })
    }

    fn native_get_user_stats(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let id = vm.param(params_addr, 1)?;
        let stats_addr = vm.param(params_addr, 2)?;
        let bodyhits_addr = vm.param(params_addr, 3)?;
        let Some(slot) = self.valid_slot(id) else {
            return Ok(0);
        };
        let key = self.state.csx_key_for_slot(self.eng, slot);
        let rank_opt = {
            let svc = self.state.services.lock();
            svc.leaderboards.rank("csx", &key)
        };
        let Some((entry, _)) = rank_opt else {
            return Ok(0);
        };
        let csx = self.state.csx_stats.get(&key).cloned().unwrap_or_default();
        write_csx_arrays(vm, stats_addr, bodyhits_addr, &csx)?;
        Ok(entry.rank as i32)
    }

    // --- nvault.inc ---
    fn native_nvault_get(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let handle = vm.param(params_addr, 1)?;
        let key = vm.param_string(params_addr, 2)?;
        let val = {
            let svc = self.state.services.lock();
            svc.nvault.get(handle, &key).map(str::to_string)
        };
        let val_str = val.as_deref().unwrap_or("");
        if argc >= 4 {
            let out_addr = vm.param(params_addr, 3)?;
            let max_len = vm.param(params_addr, 4)?.max(0) as usize;
            let written = vm.set_amx_string(out_addr, val_str, max_len)?;
            Ok(written as i32)
        } else if argc == 3 {
            let out_addr = vm.param(params_addr, 3)?;
            let f = parse_cvar_f32(val_str);
            vm.write_cell(out_addr, f.to_bits() as i32)?;
            Ok(1)
        } else {
            Ok(parse_cvar_i32(val_str))
        }
    }

    fn native_nvault_lookup(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let handle = vm.param(params_addr, 1)?;
        let key = vm.param_string(params_addr, 2)?;
        let out_addr = vm.param(params_addr, 3)?;
        let max_len = vm.param(params_addr, 4)?.max(0) as usize;
        let ts_addr = vm.param(params_addr, 5)?;
        let found = {
            let svc = self.state.services.lock();
            svc.nvault.lookup(handle, &key).map(|(v, ts)| (v.to_string(), ts))
        };
        if let Some((val, ts)) = found {
            vm.set_amx_string(out_addr, &val, max_len)?;
            vm.write_cell(ts_addr, ts as i32)?;
            Ok(1)
        } else {
            Ok(0)
        }
    }

    // --- hamsandwich ExecuteHam / ExecuteHamB ---
    fn native_execute_ham(
        &mut self,
        vm: &mut PawnVm,
        params_addr: i32,
        broadcast_hooks: bool,
    ) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let func_id = vm.param(params_addr, 1)?;
        let ent_idx = vm.param(params_addr, 2)?;
        let Ok(func) = HamFunction::try_from(func_id) else {
            return Ok(0);
        };
        let Ok(eid) = u16::try_from(ent_idx) else {
            return Ok(0);
        };
        let mut extra = Vec::new();
        for i in 3..=argc {
            extra.push(vm.param(params_addr, i)?);
        }

        if broadcast_hooks {
            let classname = self
                .resolve_edict(ent_idx)
                .and_then(|e| self.eng.ent(e))
                .map(|v| v.classname.clone())
                .unwrap_or_else(|| "player".to_string());
            let mut hook_args = Vec::with_capacity(1 + extra.len());
            hook_args.push(ent_idx);
            hook_args.extend_from_slice(&extra);

            let matching_pre: Vec<(usize, String)> = self
                .state
                .ham_hooks
                .iter()
                .filter(|h| h.function == func && !h.post && h.classname == classname)
                .map(|h| (h.plugin_idx, h.callback.clone()))
                .collect();
            self.state.ham_param_overrides.clear();
            let mut superceded = false;
            for (pidx, cb) in matching_pre {
                if let Ok(Some(ret)) = self.invoke_target_plugin(vm, pidx, &cb, &hook_args)
                    && ret >= HAM_SUPERCEDE
                {
                    superceded = true;
                    break;
                }
            }
            if superceded {
                return Ok(0);
            }
            for (param_idx, val) in std::mem::take(&mut self.state.ham_param_overrides) {
                if param_idx >= 2 {
                    let eidx = (param_idx - 2) as usize;
                    if let Some(slot) = extra.get_mut(eidx) {
                        *slot = val;
                    }
                }
            }
            let res = self.hooks.execute_ham(func, eid, self.eng, &extra);
            let matching_post: Vec<(usize, String)> = self
                .state
                .ham_hooks
                .iter()
                .filter(|h| h.function == func && h.post && h.classname == classname)
                .map(|h| (h.plugin_idx, h.callback.clone()))
                .collect();
            for (pidx, cb) in matching_post {
                let _ = self.invoke_target_plugin(vm, pidx, &cb, &hook_args);
            }
            return Ok(res);
        }

        Ok(self.hooks.execute_ham(func, eid, self.eng, &extra))
    }

    // --- reapi subset ---
    fn native_rg_give_item(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let id = vm.param(params_addr, 1)?;
        let item = vm.param_string(params_addr, 2)?;
        let give_type = if argc >= 3 {
            vm.param(params_addr, 3)?
        } else {
            GT_APPEND
        };
        let Some(slot) = self.valid_slot(id) else {
            return Ok(0);
        };
        if give_type == GT_REPLACE || give_type == GT_DROP_AND_REPLACE {
            self.hooks.remove_all_items(slot, self.eng, false);
        }
        Ok(i32::from(self.hooks.give_item(slot, self.eng, &item)))
    }

    fn native_rg_add_account(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let id = vm.param(params_addr, 1)?;
        let amount = vm.param(params_addr, 2)?;
        let mode = if argc >= 3 {
            vm.param(params_addr, 3)?
        } else {
            AS_ADD
        };
        let track = if argc >= 4 {
            vm.param(params_addr, 4)? != 0
        } else {
            true
        };
        let Some(slot) = self.valid_slot(id) else {
            return Ok(0);
        };
        if mode == AS_SET {
            self.hooks.set_user_money(slot, amount);
            let _ = self.eng.send_usermsg(
                MsgDest::One(slot),
                &CsUserMsg::Money {
                    amount: amount.clamp(0, 16_000),
                    flash: track,
                },
            );
        } else {
            self.hooks.add_account(slot, self.eng, amount, track);
        }
        Ok(1)
    }

    // --- Dynamic Inter-Plugin Natives & Multi-Forwards ---
    fn native_register_native(
        &mut self,
        vm: &mut PawnVm,
        params_addr: i32,
    ) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let fn_name = vm.param_string(params_addr, 1)?;
        let handler = vm.param_string(params_addr, 2)?;
        let style = if argc >= 3 {
            vm.param(params_addr, 3)?
        } else {
            0
        };
        let owner_id = self.plugin_owner_id();
        let mut svc = self.state.services.lock();
        svc.rpc.export_fn(RpcExportEntry {
            fn_name,
            owner_id,
            runtime: PluginRuntimeKind::Amxx,
            owner_index: self.plugin_idx,
            callback_id: style,
            callback_name: handler,
        });
        Ok(1)
    }

    fn native_create_multi_forward(
        &mut self,
        vm: &mut PawnVm,
        params_addr: i32,
    ) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let name = vm.param_string(params_addr, 1)?;
        let exec_type = vm.param(params_addr, 2)?;
        let mut param_types = Vec::new();
        for i in 3..=argc {
            param_types.push(vm.param(params_addr, i)?);
        }
        let mut svc = self.state.services.lock();
        Ok(svc.rpc.create_multi_forward(name, exec_type, param_types))
    }

    fn native_execute_forward(
        &mut self,
        vm: &mut PawnVm,
        params_addr: i32,
    ) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let handle = vm.param(params_addr, 1)?;
        let ret_addr = if argc >= 2 {
            Some(vm.param(params_addr, 2)?)
        } else {
            None
        };
        let desc = {
            let svc = self.state.services.lock();
            svc.rpc.multi_forward(handle).cloned()
        };
        let Some(desc) = desc else {
            return Ok(0);
        };

        let mut rpc_args = Vec::new();
        let mut scalar_args = Vec::new();
        let mut string_args: Vec<(usize, String)> = Vec::new();
        for (pidx, &ptype) in desc.param_types.iter().enumerate() {
            let arg_pos = pidx + 3;
            if arg_pos > argc {
                rpc_args.push(RpcValue::Int(0));
                scalar_args.push(0);
                continue;
            }
            let raw = vm.param(params_addr, arg_pos)?;
            match ptype {
                FP_STRING => {
                    let s = vm.get_amx_string(raw).unwrap_or_default();
                    rpc_args.push(RpcValue::Str(s.clone()));
                    string_args.push((pidx, s));
                    scalar_args.push(0);
                }
                FP_FLOAT => {
                    let val = if raw >= 0 {
                        vm.read_cell(raw).unwrap_or(raw)
                    } else {
                        raw
                    };
                    rpc_args.push(RpcValue::Float(f32::from_bits(val as u32)));
                    scalar_args.push(val);
                }
                _ => {
                    let val = if raw >= 0 && raw % 4 == 0 {
                        vm.read_cell(raw).unwrap_or(raw)
                    } else {
                        raw
                    };
                    rpc_args.push(RpcValue::Int(val));
                    scalar_args.push(val);
                }
            }
        }

        // Dispatch to all AMXX plugins implementing `desc.name`.
        let mut final_ret = PLUGIN_CONTINUE;
        let peer_count = self.peers.len();
        for idx in 0..peer_count {
            let mut call_args = scalar_args.clone();
            if !string_args.is_empty() {
                if idx == self.plugin_idx {
                    for (pos, s) in &string_args {
                        if let Ok(addr) = vm.allot_string(s) {
                            call_args[*pos] = addr;
                        }
                    }
                } else if let Some(Some(p)) = self.peers.get_mut(idx) {
                    for (pos, s) in &string_args {
                        if let Ok(addr) = p.vm.allot_string(s) {
                            call_args[*pos] = addr;
                        }
                    }
                }
            }
            if let Ok(Some(ret)) = self.invoke_target_plugin(vm, idx, &desc.name, &call_args) {
                match desc.exec_type {
                    ET_STOP => {
                        if ret > PLUGIN_CONTINUE {
                            final_ret = ret;
                            break;
                        }
                    }
                    ET_STOP2 => {
                        final_ret = final_ret.max(ret);
                        if ret == PLUGIN_HANDLED {
                            break;
                        }
                    }
                    ET_CONTINUE => {
                        final_ret = final_ret.max(ret);
                    }
                    _ => {} // ET_IGNORE
                }
            }
        }

        // Record & fan out to WASM / polyglot event subscribers over the shared RpcBus!
        {
            let mut svc = self.state.services.lock();
            svc.rpc
                .record_event(desc.name.clone(), String::new(), rpc_args.clone());
        }
        if let Some(ext) = self.state.external_rpc.clone() {
            ext.emit_external_event(&desc.name, &rpc_args);
        }

        if let Some(addr) = ret_addr {
            let _ = vm.write_cell(addr, final_ret);
        }
        Ok(1)
    }

    fn invoke_dynamic_native(
        &mut self,
        vm: &mut PawnVm,
        params_addr: i32,
        entry: RpcExportEntry,
    ) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let mut raw_params = Vec::with_capacity(argc);
        for i in 1..=argc {
            raw_params.push(vm.param(params_addr, i)?);
        }

        match entry.runtime {
            PluginRuntimeKind::Amxx => {
                let str_snapshots: Vec<Option<String>> = raw_params
                    .iter()
                    .map(|&raw| {
                        if raw >= 0 && raw % 4 == 0 {
                            vm.get_amx_string(raw).ok()
                        } else {
                            None
                        }
                    })
                    .collect();
                self.state.amxx_native_frames.push(AmxxNativeCallFrame {
                    raw_cells: raw_params.clone(),
                    str_snapshots,
                    str_writes: vec![None; argc],
                });
                let rc = invoke_plugin_in_slots(
                    self.peers,
                    self.state,
                    entry.owner_index,
                    self.eng,
                    self.hooks,
                    &entry.callback_name,
                    &[self.plugin_idx as i32, argc as i32],
                )
                .unwrap_or(None)
                .unwrap_or(0);
                if let Some(frame) = self.state.amxx_native_frames.pop() {
                    for (idx, write) in frame.str_writes.into_iter().enumerate() {
                        if let Some((s, max_len)) = write
                            && let Some(&dest_addr) = raw_params.get(idx)
                            && dest_addr >= 0
                        {
                            let _ = vm.set_amx_string(dest_addr, &s, max_len);
                        }
                    }
                }
                Ok(rc)
            }
            PluginRuntimeKind::Host | PluginRuntimeKind::Wasm | PluginRuntimeKind::Python => {
                let mut rpc_args = Vec::with_capacity(argc * 2);
                for &raw in &raw_params {
                    rpc_args.push(RpcValue::Int(raw));
                }
                for &raw in &raw_params {
                    if raw >= 0
                        && raw % 4 == 0
                        && let Ok(s) = vm.get_amx_string(raw)
                        && !s.is_empty()
                        && is_plausible_amx_string(&s)
                    {
                        rpc_args.push(RpcValue::Str(s));
                    }
                }
                let host_res = {
                    let svc = self.state.services.lock();
                    svc.rpc.call_host_fn(&entry.fn_name, &rpc_args)
                };
                let res = host_res.or_else(|| {
                    self.state
                        .external_rpc
                        .clone()
                        .and_then(|ext| ext.call_external_rpc(&entry.fn_name, &rpc_args))
                });
                match res {
                    Some(RpcValue::Int(n)) => Ok(n),
                    Some(RpcValue::Float(f)) => Ok(f.to_bits() as i32),
                    Some(RpcValue::Str(s)) => {
                        if argc >= 2
                            && let Some(&out_addr) = raw_params.get(argc - 2)
                            && let Some(&cap) = raw_params.last()
                            && out_addr >= 0
                            && cap > 0
                        {
                            let written = vm.set_amx_string(out_addr, &s, cap as usize)?;
                            Ok(written as i32)
                        } else {
                            Ok(s.len() as i32)
                        }
                    }
                    Some(RpcValue::Bytes(b)) => Ok(b.len() as i32),
                    None => Ok(0),
                }
            }
        }
    }

    fn native_set_task(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let delay = f64::from(vm.param_f32(params_addr, 1)?).max(0.0);
        let callback = vm.param_string(params_addr, 2)?;
        let task_id = if argc >= 3 {
            vm.param(params_addr, 3)?
        } else {
            0
        };
        let param_addr = if argc >= 4 {
            vm.param(params_addr, 4)?
        } else {
            0
        };
        let param_len = if argc >= 5 {
            vm.param(params_addr, 5)?.max(0) as usize
        } else {
            0
        };
        let flags = if argc >= 6 {
            vm.param_string(params_addr, 6)?
        } else {
            String::new()
        };
        let repeat = if argc >= 7 {
            vm.param(params_addr, 7)?
        } else {
            0
        };

        let mut param_data = Vec::new();
        if param_len > 0 {
            let cap = param_len.min(1024);
            param_data.reserve(cap);
            for i in 0..cap {
                param_data.push(vm.read_cell(param_addr + (i as i32) * 4)?);
            }
        }

        let remaining_repeats = if flags.contains('b') {
            None
        } else if flags.contains('a') {
            Some(repeat.max(1))
        } else {
            Some(1)
        };

        let handle = self.state.next_task_handle;
        self.state.next_task_handle = self.state.next_task_handle.wrapping_add(1).max(1);
        let interval = delay.max(0.001);
        self.state.tasks.push(ScheduledTask {
            handle,
            plugin_idx: self.plugin_idx,
            task_id,
            callback,
            interval,
            next_run: self.eng.time() + delay,
            remaining_repeats,
            param_data,
        });
        Ok(handle)
    }

    fn native_remove_task(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let id = if argc >= 1 {
            vm.param(params_addr, 1)?
        } else {
            0
        };
        let outside = if argc >= 2 {
            vm.param(params_addr, 2)? != 0
        } else {
            false
        };
        let before = self.state.tasks.len();
        self.state
            .tasks
            .retain(|t| !(t.task_id == id && (outside || t.plugin_idx == self.plugin_idx)));
        Ok((before - self.state.tasks.len()) as i32)
    }

    fn native_task_exists(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let id = if argc >= 1 {
            vm.param(params_addr, 1)?
        } else {
            0
        };
        let outside = if argc >= 2 {
            vm.param(params_addr, 2)? != 0
        } else {
            false
        };
        let found = self
            .state
            .tasks
            .iter()
            .any(|t| t.task_id == id && (outside || t.plugin_idx == self.plugin_idx));
        Ok(i32::from(found))
    }

    fn native_client_print(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let target = vm.param(params_addr, 1)?;
        let print_type = vm.param(params_addr, 2)?;
        if target == 0 {
            let max_clients = self.eng.max_clients();
            let mut per_slot: Vec<(u8, String)> = Vec::new();
            {
                let svc = self.state.services.lock();
                for slot in 0..max_clients {
                    if self.eng.is_client_active(slot)
                        || self.state.connected_slots[usize::from(slot)]
                    {
                        let text = vm.format_amx_string_with_lang(
                            params_addr,
                            3,
                            4,
                            Some(slot),
                            Some(&svc.lang),
                        )?;
                        per_slot.push((slot, text));
                    }
                }
            }
            for (slot, text) in per_slot {
                send_client_print(self.eng, slot, print_type, &text);
            }
            return Ok(1);
        }
        if let Some(slot) = self.connected_slot(target) {
            let text = {
                let svc = self.state.services.lock();
                vm.format_amx_string_with_lang(params_addr, 3, 4, Some(slot), Some(&svc.lang))?
            };
            send_client_print(self.eng, slot, print_type, &text);
            Ok(1)
        } else {
            Ok(0)
        }
    }

    fn native_console_print(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let target = vm.param(params_addr, 1)?;
        let fallback_slot = if target == 0 {
            None
        } else {
            self.valid_slot(target)
        };
        let text = {
            let svc = self.state.services.lock();
            vm.format_amx_string_with_lang(params_addr, 2, 3, fallback_slot, Some(&svc.lang))?
        };
        if target == 0 {
            info!(target: "amxx_console", "{text}");
            self.state.server_prints.push(text);
            return Ok(1);
        }
        if let Some(slot) = self.connected_slot(target) {
            self.eng.client_print(slot, &format!("{text}\n"));
            Ok(1)
        } else {
            Ok(0)
        }
    }

    fn native_get_user_name(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let id = vm.param(params_addr, 1)?;
        let out_addr = vm.param(params_addr, 2)?;
        let max_len = vm.param(params_addr, 3)?.max(0) as usize;
        let name = if id == 0 {
            self.eng
                .cvar_str("hostname")
                .unwrap_or_else(|| "Counter-Strike 1.6".to_string())
        } else if let Some(slot) = self.valid_slot(id) {
            self.eng
                .client_key(slot, "name")
                .unwrap_or("")
                .to_string()
        } else {
            String::new()
        };
        let written = vm.set_amx_string(out_addr, &name, max_len)?;
        Ok(written as i32)
    }

    fn native_get_user_authid(
        &mut self,
        vm: &mut PawnVm,
        params_addr: i32,
    ) -> Result<i32, AmxError> {
        let id = vm.param(params_addr, 1)?;
        let out_addr = vm.param(params_addr, 2)?;
        let max_len = vm.param(params_addr, 3)?.max(0) as usize;
        let authid = if let Some(slot) = self.valid_slot(id) {
            if self.eng.is_fake_client(slot) {
                "BOT".to_string()
            } else if let Some(sid) = self.eng.client_key(slot, "*sid") {
                sid.to_string()
            } else {
                "STEAM_ID_LAN".to_string()
            }
        } else {
            String::new()
        };
        let written = vm.set_amx_string(out_addr, &authid, max_len)?;
        Ok(written as i32)
    }

    fn native_get_user_ip(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let id = vm.param(params_addr, 1)?;
        let out_addr = vm.param(params_addr, 2)?;
        let max_len = vm.param(params_addr, 3)?.max(0) as usize;
        let argc = vm.param_count(params_addr)?;
        let without_port = if argc >= 4 {
            vm.param(params_addr, 4)? != 0
        } else {
            false
        };
        let mut ip = if let Some(slot) = self.valid_slot(id) {
            self.eng
                .client_key(slot, "ip")
                .unwrap_or("127.0.0.1:27005")
                .to_string()
        } else {
            String::new()
        };
        if without_port && let Some((host, _)) = ip.split_once(':') {
            ip = host.to_string();
        }
        let written = vm.set_amx_string(out_addr, &ip, max_len)?;
        Ok(written as i32)
    }

    fn native_get_user_team(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let id = vm.param(params_addr, 1)?;
        let argc = vm.param_count(params_addr)?;
        let team = self
            .valid_slot(id)
            .map_or(0, |slot| self.hooks.get_user_team(slot));
        if argc >= 3 {
            let out_addr = vm.param(params_addr, 2)?;
            let max_len = vm.param(params_addr, 3)?.max(0) as usize;
            let name = match team {
                1 => "TERRORIST",
                2 => "CT",
                3 => "SPECTATOR",
                _ => "UNASSIGNED",
            };
            vm.set_amx_string(out_addr, name, max_len)?;
        }
        Ok(team)
    }

    fn native_get_user_origin(
        &mut self,
        vm: &mut PawnVm,
        params_addr: i32,
    ) -> Result<i32, AmxError> {
        let id = vm.param(params_addr, 1)?;
        let out_addr = vm.param(params_addr, 2)?;
        let argc = vm.param_count(params_addr)?;
        let mode = if argc >= 3 {
            vm.param(params_addr, 3)?
        } else {
            0
        };
        let Some(eid) = self.slot_edict(id) else {
            return Ok(0);
        };
        let Some(vars) = self.eng.ent(eid) else {
            return Ok(0);
        };
        let pos = if mode == 1 {
            vars.origin + vars.view_ofs
        } else {
            vars.origin
        };
        vm.write_cell(out_addr, pos.x as i32)?;
        vm.write_cell(out_addr + 4, pos.y as i32)?;
        vm.write_cell(out_addr + 8, pos.z as i32)?;
        Ok(1)
    }

    fn native_get_user_weapon(
        &mut self,
        vm: &mut PawnVm,
        params_addr: i32,
    ) -> Result<i32, AmxError> {
        let id = vm.param(params_addr, 1)?;
        let argc = vm.param_count(params_addr)?;
        let (wid, clip, ammo) = self
            .valid_slot(id)
            .map_or((0, 0, 0), |slot| self.hooks.get_active_weapon(slot));
        if argc >= 2 {
            let clip_addr = vm.param(params_addr, 2)?;
            let _ = vm.write_cell(clip_addr, clip);
        }
        if argc >= 3 {
            let ammo_addr = vm.param(params_addr, 3)?;
            let _ = vm.write_cell(ammo_addr, ammo);
        }
        Ok(wid)
    }

    fn native_get_players(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let players_addr = vm.param(params_addr, 1)?;
        let num_addr = vm.param(params_addr, 2)?;
        let flags = if argc >= 3 {
            vm.param_string(params_addr, 3)?
        } else {
            String::new()
        };
        let team_or_name = if argc >= 4 {
            vm.param_string(params_addr, 4)?
        } else {
            String::new()
        };

        let skip_dead = flags.contains('a');
        let skip_alive = flags.contains('b');
        let skip_bots = flags.contains('c');
        let skip_real = flags.contains('d');
        let match_team = flags.contains('e');
        let match_name = flags.contains('f');
        let case_insensitive = flags.contains('g');
        let skip_hltv = flags.contains('h');

        let mut count = 0i32;
        for slot in 0..self.eng.max_clients() {
            if !(self.eng.is_client_active(slot) || self.state.connected_slots[usize::from(slot)])
            {
                continue;
            }
            let Some(vars) = self.eng.ent(self.eng.client_edict(slot)) else {
                continue;
            };
            let alive = vars.deadflag == 0 && vars.health > 0.0;
            if skip_dead && !alive {
                continue;
            }
            if skip_alive && alive {
                continue;
            }
            let is_bot = self.eng.is_fake_client(slot) || (vars.flags & FL_FAKECLIENT) != 0;
            if skip_bots && is_bot {
                continue;
            }
            if skip_real && !is_bot {
                continue;
            }
            if skip_hltv && (vars.flags & FL_PROXY) != 0 {
                continue;
            }
            if match_team {
                let team_str = match self.hooks.get_user_team(slot) {
                    1 => "TERRORIST",
                    2 => "CT",
                    3 => "SPECTATOR",
                    _ => "UNASSIGNED",
                };
                let ok = if case_insensitive {
                    team_str.eq_ignore_ascii_case(&team_or_name)
                } else {
                    team_str == team_or_name
                };
                if !ok {
                    continue;
                }
            }
            if match_name {
                let name = self.eng.client_key(slot, "name").unwrap_or("");
                let ok = if case_insensitive {
                    name.to_ascii_lowercase()
                        .contains(&team_or_name.to_ascii_lowercase())
                } else {
                    name.contains(&team_or_name)
                };
                if !ok {
                    continue;
                }
            }

            vm.write_cell(players_addr + count * 4, i32::from(slot) + 1)?;
            count += 1;
        }
        vm.write_cell(num_addr, count)?;
        Ok(1)
    }

    fn native_equal(
        &self,
        vm: &PawnVm,
        params_addr: i32,
        case_insensitive: bool,
    ) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let a = vm.param_string(params_addr, 1)?;
        let b = vm.param_string(params_addr, 2)?;
        let limit = if argc >= 3 {
            vm.param(params_addr, 3)?.max(0) as usize
        } else {
            0
        };
        let (sa, sb) = if limit > 0 {
            (
                a.chars().take(limit).collect::<String>(),
                b.chars().take(limit).collect::<String>(),
            )
        } else {
            (a, b)
        };
        let eq = if case_insensitive {
            sa.eq_ignore_ascii_case(&sb)
        } else {
            sa == sb
        };
        Ok(i32::from(eq))
    }

    fn native_contain(
        &self,
        vm: &PawnVm,
        params_addr: i32,
        case_insensitive: bool,
    ) -> Result<i32, AmxError> {
        let hay = vm.param_string(params_addr, 1)?;
        let needle = vm.param_string(params_addr, 2)?;
        let pos = if case_insensitive {
            hay.to_ascii_lowercase()
                .find(&needle.to_ascii_lowercase())
        } else {
            hay.find(&needle)
        };
        Ok(pos.map_or(-1, |i| i as i32))
    }

    fn native_pev(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let ent_idx = vm.param(params_addr, 1)?;
        let field = vm.param(params_addr, 2)?;
        let Some(eid) = self.resolve_edict(ent_idx) else {
            return Ok(0);
        };
        let Some(vars) = self.eng.ent(eid) else {
            return Ok(0);
        };

        if let Some(val) = read_pev_int(vars, field) {
            if argc >= 3 {
                let out_addr = vm.param(params_addr, 3)?;
                vm.write_cell(out_addr, val)?;
                return Ok(1);
            }
            return Ok(val);
        }
        if let Some(val) = read_pev_float(vars, field) {
            let bits = val.to_bits() as i32;
            if argc >= 3 {
                let out_addr = vm.param(params_addr, 3)?;
                vm.write_cell(out_addr, bits)?;
                return Ok(1);
            }
            return Ok(bits);
        }
        if let Some(v) = read_pev_vec(vars, field) {
            if argc >= 3 {
                let out_addr = vm.param(params_addr, 3)?;
                write_amx_vec3(vm, out_addr, v)?;
                return Ok(1);
            }
            return Ok(0);
        }
        if let Some(s) = read_pev_string(vars, field)
            && argc >= 4
        {
            let out_addr = vm.param(params_addr, 3)?;
            let max_len = vm.param(params_addr, 4)?.max(0) as usize;
            let written = vm.set_amx_string(out_addr, s, max_len)?;
            return Ok(written as i32);
        }
        Ok(0)
    }

    fn native_set_pev(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let ent_idx = vm.param(params_addr, 1)?;
        let field = vm.param(params_addr, 2)?;
        let Some(eid) = self.resolve_edict(ent_idx) else {
            return Ok(0);
        };
        let arg3 = vm.param(params_addr, 3)?;

        if is_pev_vec(field) {
            let v = read_amx_vec3(vm, arg3)?;
            let Some(vars) = self.eng.ent_mut(eid) else {
                return Ok(0);
            };
            return Ok(i32::from(write_pev_vec(vars, field, v)));
        }
        if is_pev_string(field) {
            let s = vm.get_amx_string(arg3)?;
            let Some(vars) = self.eng.ent_mut(eid) else {
                return Ok(0);
            };
            return Ok(i32::from(write_pev_string(vars, field, &s)));
        }

        let Some(vars) = self.eng.ent_mut(eid) else {
            return Ok(0);
        };
        if write_pev_int(vars, field, arg3) {
            return Ok(1);
        }
        if write_pev_float(vars, field, f32::from_bits(arg3 as u32)) {
            return Ok(1);
        }
        Ok(0)
    }

    fn native_register_ham(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let argc = vm.param_count(params_addr)?;
        let func_id = vm.param(params_addr, 1)?;
        let classname = vm.param_string(params_addr, 2)?;
        let callback = vm.param_string(params_addr, 3)?;
        let post = if argc >= 4 {
            vm.param(params_addr, 4)? != 0
        } else {
            false
        };
        let Ok(function) = HamFunction::try_from(func_id) else {
            return Err(AmxError::Params(format!(
                "unsupported Ham function id {func_id}"
            )));
        };
        self.state.ham_hooks.push(RegisteredHam {
            plugin_idx: self.plugin_idx,
            function,
            classname,
            callback,
            post,
        });
        Ok(self.state.ham_hooks.len() as i32)
    }
}

pub(crate) fn send_amxx_rendered_menu(eng: &mut EngineApi<'_>, rendered: &RenderedMenu) {
    const MAX_CHUNK: usize = 175;
    let text = rendered.text.as_str();
    if text.len() <= MAX_CHUNK {
        let _ = eng.send_usermsg(
            MsgDest::One(rendered.slot),
            &CsUserMsg::ShowMenu {
                keys_bitmask: rendered.keys_bitmask,
                display_time: rendered.time_sec,
                multipart: false,
                text: text.to_string(),
            },
        );
        return;
    }
    let mut start = 0usize;
    while start < text.len() {
        let mut end = (start + MAX_CHUNK).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            break;
        }
        let multipart = end < text.len();
        let _ = eng.send_usermsg(
            MsgDest::One(rendered.slot),
            &CsUserMsg::ShowMenu {
                keys_bitmask: rendered.keys_bitmask,
                display_time: rendered.time_sec,
                multipart,
                text: text[start..end].to_string(),
            },
        );
        start = end;
    }
}

fn write_csx_arrays(
    vm: &mut PawnVm,
    stats_addr: i32,
    bodyhits_addr: i32,
    csx: &CsxPlayerStats,
) -> Result<(), AmxError> {
    let vals = [
        csx.kills,
        csx.deaths,
        csx.headshots,
        csx.teamkills,
        csx.shots,
        csx.hits,
        csx.damage,
        csx.kills - csx.deaths,
    ];
    for (i, &v) in vals.iter().enumerate() {
        vm.write_cell(stats_addr + (i as i32) * 4, v)?;
    }
    for (i, &v) in csx.bodyhits.iter().enumerate() {
        vm.write_cell(bodyhits_addr + (i as i32) * 4, v)?;
    }
    Ok(())
}

fn is_plausible_amx_string(s: &str) -> bool {
    s.bytes()
        .all(|b| b >= 0x20 || matches!(b, b'\n' | b'\r' | b'\t' | 0x01 | 0x03 | 0x04))
}

fn send_client_print(eng: &mut EngineApi<'_>, slot: u8, print_type: i32, text: &str) {
    match print_type {
        PRINT_CENTER => eng.center_print(slot, text),
        PRINT_CHAT => {
            let _ = eng.send_usermsg(
                MsgDest::One(slot),
                &CsUserMsg::TextMsg {
                    dest: PRINT_CHAT as u8,
                    message: text.to_string(),
                    params: Vec::new(),
                },
            );
        }
        _ => eng.client_print(slot, &format!("{text}\n")),
    }
}

fn parse_cvar_i32(s: &str) -> i32 {
    let t = s.trim();
    t.parse::<i32>()
        .or_else(|_| {
            t.parse::<f32>()
                .ok()
                .filter(|f| f.is_finite())
                .map(|f| f as i32)
                .ok_or(())
        })
        .unwrap_or(0)
}

fn parse_cvar_f32(s: &str) -> f32 {
    s.trim()
        .parse::<f32>()
        .ok()
        .filter(|f| f.is_finite())
        .unwrap_or(0.0)
}

fn format_amxx_float(v: f32) -> String {
    if v.fract() == 0.0 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

fn read_amx_vec3(vm: &PawnVm, addr: i32) -> Result<Vec3, AmxError> {
    let x = f32::from_bits(vm.read_cell(addr)? as u32);
    let y = f32::from_bits(vm.read_cell(addr + 4)? as u32);
    let z = f32::from_bits(vm.read_cell(addr + 8)? as u32);
    Ok(Vec3::new(x, y, z))
}

fn write_amx_vec3(vm: &mut PawnVm, addr: i32, v: Vec3) -> Result<(), AmxError> {
    vm.write_cell(addr, v.x.to_bits() as i32)?;
    vm.write_cell(addr + 4, v.y.to_bits() as i32)?;
    vm.write_cell(addr + 8, v.z.to_bits() as i32)?;
    Ok(())
}

// --- EntVars <-> pev / engine field mappers ---

fn read_pev_int(v: &cs_engine::EntVars, field: i32) -> Option<i32> {
    use pev_const::*;
    Some(match field {
        PEV_ENEMY => v.enemy.map_or(0, |id| i32::from(id.index)),
        PEV_AIMENT => v.aiment.map_or(0, |id| i32::from(id.index)),
        PEV_OWNER => v.owner.map_or(0, |id| i32::from(id.index)),
        PEV_GROUNDENTITY => v.groundentity.map_or(0, |id| i32::from(id.index)),
        PEV_FIXANGLE => v.fixangle,
        PEV_MODELINDEX => i32::from(v.modelindex),
        PEV_VIEWMODEL => i32::from(v.viewmodel),
        PEV_WEAPONMODEL => i32::from(v.weaponmodel),
        PEV_MOVETYPE => i32::from(v.movetype),
        PEV_SOLID => i32::from(v.solid),
        PEV_SKIN => i32::from(v.skin),
        PEV_BODY => i32::from(v.body),
        PEV_EFFECTS => v.effects as i32,
        PEV_SEQUENCE => i32::from(v.sequence),
        PEV_GAITSEQUENCE => i32::from(v.gaitsequence),
        PEV_RENDERMODE => i32::from(v.rendermode),
        PEV_RENDERFX => i32::from(v.renderfx),
        PEV_DEADFLAG => v.deadflag,
        PEV_BUTTON => v.button,
        PEV_IMPULSE => v.impulse,
        PEV_SPAWNFLAGS => v.spawnflags as i32,
        PEV_FLAGS => v.flags as i32,
        PEV_COLORMAP => i32::from(v.colormap),
        PEV_WATERLEVEL => v.waterlevel,
        PEV_WATERTYPE => v.watertype,
        PEV_WEAPONANIM => v.weaponanim,
        PEV_PUSHMSEC => v.pushmsec,
        PEV_B_IN_DUCK => i32::from(v.b_in_duck),
        PEV_FL_TIME_STEP_SOUND => v.fl_time_step_sound,
        PEV_FL_SWIM_TIME => v.fl_swim_time,
        PEV_FL_DUCK_TIME => v.fl_duck_time,
        PEV_I_STEP_LEFT => v.i_step_left,
        PEV_GAMESTATE => v.gamestate,
        PEV_OLDBUTTONS => v.oldbuttons,
        PEV_IUSER1 => v.iuser1,
        PEV_IUSER2 => v.iuser2,
        PEV_IUSER3 => v.iuser3,
        PEV_IUSER4 => v.iuser4,
        _ => return None,
    })
}

fn write_pev_int(v: &mut cs_engine::EntVars, field: i32, val: i32) -> bool {
    use pev_const::*;
    match field {
        PEV_FIXANGLE => v.fixangle = val,
        PEV_MODELINDEX => v.modelindex = val as u16,
        PEV_VIEWMODEL => v.viewmodel = val as u16,
        PEV_WEAPONMODEL => v.weaponmodel = val as u16,
        PEV_MOVETYPE => v.movetype = val as u8,
        PEV_SOLID => v.solid = val as u8,
        PEV_SKIN => v.skin = val as i16,
        PEV_BODY => v.body = val as u8,
        PEV_EFFECTS => v.effects = val as u32,
        PEV_SEQUENCE => v.sequence = val as u16,
        PEV_GAITSEQUENCE => v.gaitsequence = val as u16,
        PEV_RENDERMODE => v.rendermode = val as u8,
        PEV_RENDERFX => v.renderfx = val as u8,
        PEV_DEADFLAG => v.deadflag = val,
        PEV_BUTTON => v.button = val,
        PEV_IMPULSE => v.impulse = val,
        PEV_SPAWNFLAGS => v.spawnflags = val as u32,
        PEV_FLAGS => v.flags = val as u32,
        PEV_COLORMAP => v.colormap = val as u16,
        PEV_WATERLEVEL => v.waterlevel = val,
        PEV_WATERTYPE => v.watertype = val,
        PEV_WEAPONANIM => v.weaponanim = val,
        PEV_PUSHMSEC => v.pushmsec = val,
        PEV_B_IN_DUCK => v.b_in_duck = val != 0,
        PEV_FL_TIME_STEP_SOUND => v.fl_time_step_sound = val,
        PEV_FL_SWIM_TIME => v.fl_swim_time = val,
        PEV_FL_DUCK_TIME => v.fl_duck_time = val,
        PEV_I_STEP_LEFT => v.i_step_left = val,
        PEV_GAMESTATE => v.gamestate = val,
        PEV_OLDBUTTONS => v.oldbuttons = val,
        PEV_IUSER1 => v.iuser1 = val,
        PEV_IUSER2 => v.iuser2 = val,
        PEV_IUSER3 => v.iuser3 = val,
        PEV_IUSER4 => v.iuser4 = val,
        _ => return false,
    }
    true
}

fn read_pev_float(v: &cs_engine::EntVars, field: i32) -> Option<f32> {
    use pev_const::*;
    Some(match field {
        PEV_IMPACTTIME => v.impacttime,
        PEV_STARTTIME => v.starttime,
        PEV_IDEAL_YAW => v.ideal_yaw,
        PEV_LTIME => v.ltime as f32,
        PEV_NEXTTHINK => v.nextthink as f32,
        PEV_GRAVITY => v.gravity,
        PEV_FRICTION => v.friction,
        PEV_FRAME => v.frame,
        PEV_ANIMTIME => v.animtime,
        PEV_FRAMERATE => v.framerate,
        PEV_HEALTH => v.health,
        PEV_TELEPORT_TIME => v.teleport_time,
        PEV_DMG => v.dmg,
        PEV_DMGTIME => v.dmgtime,
        PEV_SPEED => v.speed,
        PEV_AIR_FINISHED => v.air_finished,
        PEV_PAIN_FINISHED => v.pain_finished,
        PEV_RADSUIT_FINISHED => v.radsuit_finished,
        PEV_SCALE => v.scale,
        PEV_RENDERAMT => f32::from(v.renderamt),
        PEV_MAXSPEED => v.max_speed,
        PEV_FOV => v.fov,
        PEV_FL_FALL_VELOCITY => v.fl_fall_velocity,
        PEV_FUSER1 => v.fuser1,
        PEV_FUSER2 => v.fuser2,
        PEV_FUSER3 => v.fuser3,
        PEV_FUSER4 => v.fuser4,
        _ => return None,
    })
}

fn write_pev_float(v: &mut cs_engine::EntVars, field: i32, val: f32) -> bool {
    use pev_const::*;
    match field {
        PEV_IMPACTTIME => v.impacttime = val,
        PEV_STARTTIME => v.starttime = val,
        PEV_IDEAL_YAW => v.ideal_yaw = val,
        PEV_LTIME => v.ltime = f64::from(val),
        PEV_NEXTTHINK => v.nextthink = f64::from(val),
        PEV_GRAVITY => v.gravity = val,
        PEV_FRICTION => v.friction = val,
        PEV_FRAME => v.frame = val,
        PEV_ANIMTIME => v.animtime = val,
        PEV_FRAMERATE => v.framerate = val,
        PEV_HEALTH => v.health = val,
        PEV_TELEPORT_TIME => v.teleport_time = val,
        PEV_DMG => v.dmg = val,
        PEV_DMGTIME => v.dmgtime = val,
        PEV_SPEED => v.speed = val,
        PEV_AIR_FINISHED => v.air_finished = val,
        PEV_PAIN_FINISHED => v.pain_finished = val,
        PEV_RADSUIT_FINISHED => v.radsuit_finished = val,
        PEV_SCALE => v.scale = val,
        PEV_RENDERAMT => v.renderamt = val.clamp(0.0, 255.0) as u8,
        PEV_MAXSPEED => v.max_speed = val,
        PEV_FOV => v.fov = val,
        PEV_FL_FALL_VELOCITY => v.fl_fall_velocity = val,
        PEV_FUSER1 => v.fuser1 = val,
        PEV_FUSER2 => v.fuser2 = val,
        PEV_FUSER3 => v.fuser3 = val,
        PEV_FUSER4 => v.fuser4 = val,
        _ => return false,
    }
    true
}

fn is_pev_vec(field: i32) -> bool {
    use pev_const::*;
    matches!(
        field,
        PEV_ORIGIN
            | PEV_VELOCITY
            | PEV_BASEVELOCITY
            | PEV_MOVEDIR
            | PEV_ANGLES
            | PEV_AVELOCITY
            | PEV_PUNCHANGLE
            | PEV_V_ANGLE
            | PEV_ENDPOS
            | PEV_STARTPOS
            | PEV_ABSMIN
            | PEV_ABSMAX
            | PEV_MINS
            | PEV_MAXS
            | PEV_SIZE
            | PEV_RENDERCOLOR
            | PEV_VIEW_OFS
            | PEV_VUSER1
            | PEV_VUSER2
            | PEV_VUSER3
            | PEV_VUSER4
    )
}

fn read_pev_vec(v: &cs_engine::EntVars, field: i32) -> Option<Vec3> {
    use pev_const::*;
    Some(match field {
        PEV_ORIGIN => v.origin,
        PEV_VELOCITY => v.velocity,
        PEV_BASEVELOCITY => v.basevelocity,
        PEV_MOVEDIR => v.movedir,
        PEV_ANGLES => v.angles,
        PEV_AVELOCITY => v.avelocity,
        PEV_PUNCHANGLE => v.punchangle,
        PEV_V_ANGLE => v.v_angle,
        PEV_ENDPOS => v.endpos,
        PEV_STARTPOS => v.startpos,
        PEV_ABSMIN => v.absmin,
        PEV_ABSMAX => v.absmax,
        PEV_MINS => v.mins,
        PEV_MAXS => v.maxs,
        PEV_SIZE => v.size,
        PEV_RENDERCOLOR => Vec3::new(
            f32::from(v.rendercolor[0]),
            f32::from(v.rendercolor[1]),
            f32::from(v.rendercolor[2]),
        ),
        PEV_VIEW_OFS => v.view_ofs,
        PEV_VUSER1 => v.vuser1,
        PEV_VUSER2 => v.vuser2,
        PEV_VUSER3 => v.vuser3,
        PEV_VUSER4 => v.vuser4,
        _ => return None,
    })
}

fn write_pev_vec(v: &mut cs_engine::EntVars, field: i32, val: Vec3) -> bool {
    use pev_const::*;
    match field {
        PEV_ORIGIN => v.origin = val,
        PEV_VELOCITY => v.velocity = val,
        PEV_BASEVELOCITY => v.basevelocity = val,
        PEV_MOVEDIR => v.movedir = val,
        PEV_ANGLES => v.angles = val,
        PEV_AVELOCITY => v.avelocity = val,
        PEV_PUNCHANGLE => v.punchangle = val,
        PEV_V_ANGLE => v.v_angle = val,
        PEV_ENDPOS => v.endpos = val,
        PEV_STARTPOS => v.startpos = val,
        PEV_ABSMIN => v.absmin = val,
        PEV_ABSMAX => v.absmax = val,
        PEV_MINS => v.mins = val,
        PEV_MAXS => v.maxs = val,
        PEV_SIZE => v.size = val,
        PEV_RENDERCOLOR => {
            v.rendercolor = [
                val.x.clamp(0.0, 255.0) as u8,
                val.y.clamp(0.0, 255.0) as u8,
                val.z.clamp(0.0, 255.0) as u8,
            ]
        }
        PEV_VIEW_OFS => v.view_ofs = val,
        PEV_VUSER1 => v.vuser1 = val,
        PEV_VUSER2 => v.vuser2 = val,
        PEV_VUSER3 => v.vuser3 = val,
        PEV_VUSER4 => v.vuser4 = val,
        _ => return false,
    }
    true
}

fn is_pev_string(field: i32) -> bool {
    use pev_const::*;
    matches!(
        field,
        PEV_CLASSNAME | PEV_MODEL | PEV_TARGET | PEV_TARGETNAME
    )
}

fn read_pev_string(v: &cs_engine::EntVars, field: i32) -> Option<&str> {
    use pev_const::*;
    Some(match field {
        PEV_CLASSNAME => &v.classname,
        PEV_MODEL => &v.model,
        PEV_TARGET => &v.target,
        PEV_TARGETNAME => &v.targetname,
        _ => return None,
    })
}

fn write_pev_string(v: &mut cs_engine::EntVars, field: i32, val: &str) -> bool {
    use pev_const::*;
    match field {
        PEV_CLASSNAME => v.classname = val.to_string(),
        PEV_MODEL => v.model = val.to_string(),
        PEV_TARGET => v.target = val.to_string(),
        PEV_TARGETNAME => v.targetname = val.to_string(),
        _ => return false,
    }
    true
}

fn ev_int_to_pev(key: i32) -> Option<i32> {
    use ev_const::*;
    use pev_const::*;
    Some(match key {
        EV_INT_GAMESTATE => PEV_GAMESTATE,
        EV_INT_OLDBUTTONS => PEV_OLDBUTTONS,
        EV_INT_IUSER1 => PEV_IUSER1,
        EV_INT_IUSER2 => PEV_IUSER2,
        EV_INT_IUSER3 => PEV_IUSER3,
        EV_INT_IUSER4 => PEV_IUSER4,
        EV_INT_WEAPONANIM => PEV_WEAPONANIM,
        EV_INT_PUSHMSEC => PEV_PUSHMSEC,
        EV_INT_B_IN_DUCK => PEV_B_IN_DUCK,
        EV_INT_FL_TIME_STEP_SOUND => PEV_FL_TIME_STEP_SOUND,
        EV_INT_FL_SWIM_TIME => PEV_FL_SWIM_TIME,
        EV_INT_FL_DUCK_TIME => PEV_FL_DUCK_TIME,
        EV_INT_I_STEP_LEFT => PEV_I_STEP_LEFT,
        EV_INT_MOVETYPE => PEV_MOVETYPE,
        EV_INT_SOLID => PEV_SOLID,
        EV_INT_SKIN => PEV_SKIN,
        EV_INT_BODY => PEV_BODY,
        EV_INT_EFFECTS => PEV_EFFECTS,
        EV_INT_SEQUENCE => PEV_SEQUENCE,
        EV_INT_GAITSEQUENCE => PEV_GAITSEQUENCE,
        EV_INT_RENDERMODE => PEV_RENDERMODE,
        EV_INT_RENDERFX => PEV_RENDERFX,
        EV_INT_DEADFLAG => PEV_DEADFLAG,
        EV_INT_BUTTON => PEV_BUTTON,
        EV_INT_IMPULSE => PEV_IMPULSE,
        EV_INT_SPAWNFLAGS => PEV_SPAWNFLAGS,
        EV_INT_FLAGS => PEV_FLAGS,
        EV_INT_COLORMAP => PEV_COLORMAP,
        EV_INT_WATERLEVEL => PEV_WATERLEVEL,
        EV_INT_WATERTYPE => PEV_WATERTYPE,
        EV_INT_FIXANGLE => PEV_FIXANGLE,
        EV_INT_MODELINDEX => PEV_MODELINDEX,
        EV_INT_VIEWMODEL => PEV_VIEWMODEL,
        EV_INT_WEAPONMODEL => PEV_WEAPONMODEL,
        _ => return None,
    })
}

fn ev_fl_to_pev(key: i32) -> Option<i32> {
    use ev_const::*;
    use pev_const::*;
    Some(match key {
        EV_FL_IMPACTTIME => PEV_IMPACTTIME,
        EV_FL_STARTTIME => PEV_STARTTIME,
        EV_FL_IDEAL_YAW => PEV_IDEAL_YAW,
        EV_FL_LTIME => PEV_LTIME,
        EV_FL_NEXTTHINK => PEV_NEXTTHINK,
        EV_FL_GRAVITY => PEV_GRAVITY,
        EV_FL_FRICTION => PEV_FRICTION,
        EV_FL_FRAME => PEV_FRAME,
        EV_FL_ANIMTIME => PEV_ANIMTIME,
        EV_FL_FRAMERATE => PEV_FRAMERATE,
        EV_FL_HEALTH => PEV_HEALTH,
        EV_FL_TELEPORT_TIME => PEV_TELEPORT_TIME,
        EV_FL_DMG => PEV_DMG,
        EV_FL_DMGTIME => PEV_DMGTIME,
        EV_FL_SPEED => PEV_SPEED,
        EV_FL_AIR_FINISHED => PEV_AIR_FINISHED,
        EV_FL_PAIN_FINISHED => PEV_PAIN_FINISHED,
        EV_FL_RADSUIT_FINISHED => PEV_RADSUIT_FINISHED,
        EV_FL_SCALE => PEV_SCALE,
        EV_FL_RENDERAMT => PEV_RENDERAMT,
        EV_FL_MAXSPEED => PEV_MAXSPEED,
        EV_FL_FOV => PEV_FOV,
        EV_FL_FL_FALL_VELOCITY => PEV_FL_FALL_VELOCITY,
        EV_FL_FUSER1 => PEV_FUSER1,
        EV_FL_FUSER2 => PEV_FUSER2,
        EV_FL_FUSER3 => PEV_FUSER3,
        EV_FL_FUSER4 => PEV_FUSER4,
        _ => return None,
    })
}

fn ev_vec_to_pev(key: i32) -> Option<i32> {
    use ev_const::*;
    use pev_const::*;
    Some(match key {
        EV_VEC_ORIGIN => PEV_ORIGIN,
        EV_VEC_VELOCITY => PEV_VELOCITY,
        EV_VEC_BASEVELOCITY => PEV_BASEVELOCITY,
        EV_VEC_MOVEDIR => PEV_MOVEDIR,
        EV_VEC_ANGLES => PEV_ANGLES,
        EV_VEC_AVELOCITY => PEV_AVELOCITY,
        EV_VEC_PUNCHANGLE => PEV_PUNCHANGLE,
        EV_VEC_V_ANGLE => PEV_V_ANGLE,
        EV_VEC_ENDPOS => PEV_ENDPOS,
        EV_VEC_STARTPOS => PEV_STARTPOS,
        EV_VEC_ABSMIN => PEV_ABSMIN,
        EV_VEC_ABSMAX => PEV_ABSMAX,
        EV_VEC_MINS => PEV_MINS,
        EV_VEC_MAXS => PEV_MAXS,
        EV_VEC_SIZE => PEV_SIZE,
        EV_VEC_RENDERCOLOR => PEV_RENDERCOLOR,
        EV_VEC_VIEW_OFS => PEV_VIEW_OFS,
        EV_VEC_VUSER1 => PEV_VUSER1,
        EV_VEC_VUSER2 => PEV_VUSER2,
        EV_VEC_VUSER3 => PEV_VUSER3,
        EV_VEC_VUSER4 => PEV_VUSER4,
        _ => return None,
    })
}

fn ev_sz_to_pev(key: i32) -> Option<i32> {
    use ev_const::*;
    use pev_const::*;
    Some(match key {
        EV_SZ_CLASSNAME => PEV_CLASSNAME,
        EV_SZ_MODEL => PEV_MODEL,
        EV_SZ_TARGET => PEV_TARGET,
        EV_SZ_TARGETNAME => PEV_TARGETNAME,
        _ => return None,
    })
}

fn read_ev_int(v: &cs_engine::EntVars, key: i32) -> Option<i32> {
    read_pev_int(v, ev_int_to_pev(key)?)
}

fn write_ev_int(v: &mut cs_engine::EntVars, key: i32, val: i32) -> bool {
    ev_int_to_pev(key).is_some_and(|p| write_pev_int(v, p, val))
}

fn read_ev_float(v: &cs_engine::EntVars, key: i32) -> Option<f32> {
    read_pev_float(v, ev_fl_to_pev(key)?)
}

fn write_ev_float(v: &mut cs_engine::EntVars, key: i32, val: f32) -> bool {
    ev_fl_to_pev(key).is_some_and(|p| write_pev_float(v, p, val))
}

fn read_ev_vec(v: &cs_engine::EntVars, key: i32) -> Option<Vec3> {
    read_pev_vec(v, ev_vec_to_pev(key)?)
}

fn write_ev_vec(v: &mut cs_engine::EntVars, key: i32, val: Vec3) -> bool {
    ev_vec_to_pev(key).is_some_and(|p| write_pev_vec(v, p, val))
}

fn read_ev_string(v: &cs_engine::EntVars, key: i32) -> Option<&str> {
    read_pev_string(v, ev_sz_to_pev(key)?)
}

fn write_ev_string(v: &mut cs_engine::EntVars, key: i32, val: &str) -> bool {
    ev_sz_to_pev(key).is_some_and(|p| write_pev_string(v, p, val))
}

/// Lightweight [`NativeDispatch`] used when an external polyglot caller (e.g. a `.wasm` mod inside
/// a `wasmi` host import) synchronously invokes an AMXX `register_native` handler without a live
/// `&mut EngineApi<'_>` borrow. Supports param/string/float/services (`nvault`, `LangCatalog`,
/// `AdminRegistry`, `LeaderboardStore`, `MenuManager`, `RpcBus`) and nested cross-plugin calls.
pub(crate) struct PureRpcDispatch<'a> {
    pub plugin_idx: usize,
    pub state: &'a mut AmxxHostState,
    pub peers: &'a mut [Option<AmxxPlugin>],
}

impl NativeDispatch for PureRpcDispatch<'_> {
    fn call_native(
        &mut self,
        vm: &mut PawnVm,
        _native_idx: usize,
        name: &str,
        params_addr: i32,
    ) -> Result<i32, AmxError> {
        match name {
            "get_param" => Ok(self.state.read_rpc_param(vm.param(params_addr, 1)?)),
            "get_param_f" => Ok(self.state.read_rpc_param_f(vm.param(params_addr, 1)?)),
            "get_string" => {
                let idx = vm.param(params_addr, 1)?;
                let out_addr = vm.param(params_addr, 2)?;
                let max_len = vm.param(params_addr, 3)?.max(0) as usize;
                let s = self.state.read_rpc_string(idx);
                let written = vm.set_amx_string(out_addr, &s, max_len)?;
                Ok(written as i32)
            }
            "set_string" => {
                let idx = vm.param(params_addr, 1)?;
                let src = vm.param_string(params_addr, 2)?;
                let argc = vm.param_count(params_addr)?;
                let max_len = if argc >= 3 {
                    vm.param(params_addr, 3)?.max(0) as usize
                } else {
                    src.len()
                };
                Ok(self.state.write_rpc_string(idx, &src, max_len))
            }
            "format" | "formatex" => {
                let out_addr = vm.param(params_addr, 1)?;
                let max_len = vm.param(params_addr, 2)?.max(0) as usize;
                let text = {
                    let svc = self.state.services.lock();
                    vm.format_amx_string_with_lang(params_addr, 3, 4, None, Some(&svc.lang))?
                };
                let written = vm.set_amx_string(out_addr, &text, max_len)?;
                Ok(written as i32)
            }
            "copy" => {
                let dest_addr = vm.param(params_addr, 1)?;
                let max_len = vm.param(params_addr, 2)?.max(0) as usize;
                let src = vm.param_string(params_addr, 3)?;
                let written = vm.set_amx_string(dest_addr, &src, max_len)?;
                Ok(written as i32)
            }
            "equal" | "equali" => {
                let argc = vm.param_count(params_addr)?;
                let a = vm.param_string(params_addr, 1)?;
                let b = vm.param_string(params_addr, 2)?;
                let limit = if argc >= 3 {
                    vm.param(params_addr, 3)?.max(0) as usize
                } else {
                    0
                };
                let (sa, sb) = if limit > 0 {
                    (
                        a.chars().take(limit).collect::<String>(),
                        b.chars().take(limit).collect::<String>(),
                    )
                } else {
                    (a, b)
                };
                let eq = if name == "equali" {
                    sa.eq_ignore_ascii_case(&sb)
                } else {
                    sa == sb
                };
                Ok(i32::from(eq))
            }
            "contain" | "containi" => {
                let hay = vm.param_string(params_addr, 1)?;
                let needle = vm.param_string(params_addr, 2)?;
                let pos = if name == "containi" {
                    hay.to_ascii_lowercase()
                        .find(&needle.to_ascii_lowercase())
                } else {
                    hay.find(&needle)
                };
                Ok(pos.map_or(-1, |i| i as i32))
            }
            "strlen" => Ok(vm.param_string(params_addr, 1)?.len() as i32),
            "str_to_num" => Ok(parse_cvar_i32(&vm.param_string(params_addr, 1)?)),
            "num_to_str" => {
                let num = vm.param(params_addr, 1)?;
                let out_addr = vm.param(params_addr, 2)?;
                let max_len = vm.param(params_addr, 3)?.max(0) as usize;
                let written = vm.set_amx_string(out_addr, &num.to_string(), max_len)?;
                Ok(written as i32)
            }
            "float" => Ok((vm.param(params_addr, 1)? as f32).to_bits() as i32),
            "floatround" => {
                let f = vm.param_f32(params_addr, 1)?;
                let argc = vm.param_count(params_addr)?;
                let method = if argc >= 2 {
                    vm.param(params_addr, 2)?
                } else {
                    0
                };
                let rounded = match method {
                    1 => f.floor(),
                    2 => f.ceil(),
                    3 => f.trunc(),
                    _ => f.round(),
                };
                Ok(rounded as i32)
            }
            "floatadd" => Ok((vm.param_f32(params_addr, 1)? + vm.param_f32(params_addr, 2)?).to_bits() as i32),
            "floatsub" => Ok((vm.param_f32(params_addr, 1)? - vm.param_f32(params_addr, 2)?).to_bits() as i32),
            "floatmul" => Ok((vm.param_f32(params_addr, 1)? * vm.param_f32(params_addr, 2)?).to_bits() as i32),
            "floatdiv" => Ok((vm.param_f32(params_addr, 1)? / vm.param_f32(params_addr, 2)?).to_bits() as i32),
            "floatcmp" => {
                let a = vm.param_f32(params_addr, 1)?;
                let b = vm.param_f32(params_addr, 2)?;
                Ok(if a == b { 0 } else if a > b { 1 } else { -1 })
            }
            "server_print" => {
                let text = {
                    let svc = self.state.services.lock();
                    vm.format_amx_string_with_lang(params_addr, 1, 2, None, Some(&svc.lang))?
                };
                self.state.server_prints.push(text);
                Ok(1)
            }
            "log_amx" => {
                let text = {
                    let svc = self.state.services.lock();
                    vm.format_amx_string_with_lang(params_addr, 1, 2, None, Some(&svc.lang))?
                };
                self.state.amx_logs.push(text);
                Ok(1)
            }
            "server_cmd" => {
                let text = vm.format_amx_string(params_addr, 1, 2)?;
                self.state.pending_server_cmds.push(text);
                Ok(1)
            }
            "get_user_flags" => {
                let id = vm.param(params_addr, 1)?;
                if (1..=32).contains(&id) {
                    Ok(self.state.effective_user_flags((id - 1) as u8))
                } else {
                    Ok(0)
                }
            }
            "set_user_flags" => {
                let id = vm.param(params_addr, 1)?;
                let flags = vm.param(params_addr, 2)?;
                if (1..=32).contains(&id) {
                    self.state.set_user_flags((id - 1) as u8, flags);
                    Ok(1)
                } else {
                    Ok(0)
                }
            }
            "LookupLangKey" => {
                let out_addr = vm.param(params_addr, 1)?;
                let out_size = vm.param(params_addr, 2)?.max(0) as usize;
                let key = vm.param_string(params_addr, 3)?;
                let id = vm.param(params_addr, 4)?;
                let slot = if (1..=32).contains(&id) {
                    Some((id - 1) as u8)
                } else {
                    None
                };
                let found = {
                    let svc = self.state.services.lock();
                    let lang = svc.lang.player_lang(slot);
                    svc.lang.lookup_key(lang, &key).map(str::to_string)
                };
                if let Some(val) = found {
                    vm.set_amx_string(out_addr, &val, out_size)?;
                    Ok(1)
                } else {
                    Ok(0)
                }
            }
            "nvault_open" => {
                let vname = vm.param_string(params_addr, 1)?;
                let mut svc = self.state.services.lock();
                Ok(svc.nvault.open(&vname))
            }
            "nvault_get" => {
                let argc = vm.param_count(params_addr)?;
                let handle = vm.param(params_addr, 1)?;
                let key = vm.param_string(params_addr, 2)?;
                let val = {
                    let svc = self.state.services.lock();
                    svc.nvault.get(handle, &key).map(str::to_string)
                };
                let val_str = val.as_deref().unwrap_or("");
                if argc >= 4 {
                    let out_addr = vm.param(params_addr, 3)?;
                    let max_len = vm.param(params_addr, 4)?.max(0) as usize;
                    let written = vm.set_amx_string(out_addr, val_str, max_len)?;
                    Ok(written as i32)
                } else if argc == 3 {
                    let out_addr = vm.param(params_addr, 3)?;
                    vm.write_cell(out_addr, parse_cvar_f32(val_str).to_bits() as i32)?;
                    Ok(1)
                } else {
                    Ok(parse_cvar_i32(val_str))
                }
            }
            "nvault_lookup" => {
                let handle = vm.param(params_addr, 1)?;
                let key = vm.param_string(params_addr, 2)?;
                let out_addr = vm.param(params_addr, 3)?;
                let max_len = vm.param(params_addr, 4)?.max(0) as usize;
                let ts_addr = vm.param(params_addr, 5)?;
                let found = {
                    let svc = self.state.services.lock();
                    svc.nvault.lookup(handle, &key).map(|(v, ts)| (v.to_string(), ts))
                };
                if let Some((val, ts)) = found {
                    vm.set_amx_string(out_addr, &val, max_len)?;
                    vm.write_cell(ts_addr, ts as i32)?;
                    Ok(1)
                } else {
                    Ok(0)
                }
            }
            "nvault_set" => {
                let handle = vm.param(params_addr, 1)?;
                let key = vm.param_string(params_addr, 2)?;
                let val = vm.param_string(params_addr, 3)?;
                let mut svc = self.state.services.lock();
                Ok(i32::from(svc.nvault.set(handle, key, val, 1)))
            }
            "nvault_pset" => {
                let handle = vm.param(params_addr, 1)?;
                let key = vm.param_string(params_addr, 2)?;
                let val = vm.param_string(params_addr, 3)?;
                let mut svc = self.state.services.lock();
                Ok(i32::from(svc.nvault.pset(handle, key, val)))
            }
            "nvault_remove" => {
                let handle = vm.param(params_addr, 1)?;
                let key = vm.param_string(params_addr, 2)?;
                let mut svc = self.state.services.lock();
                Ok(i32::from(svc.nvault.remove(handle, &key)))
            }
            "nvault_prune" => {
                let handle = vm.param(params_addr, 1)?;
                let start = i64::from(vm.param(params_addr, 2)?);
                let end = i64::from(vm.param(params_addr, 3)?);
                let mut svc = self.state.services.lock();
                Ok(svc.nvault.prune(handle, start, end) as i32)
            }
            "nvault_close" => {
                let handle = vm.param(params_addr, 1)?;
                let mut svc = self.state.services.lock();
                Ok(i32::from(svc.nvault.close(handle)))
            }
            "get_statsnum" => {
                let svc = self.state.services.lock();
                Ok(svc.leaderboards.count("csx") as i32)
            }
            other => {
                let resolved = {
                    let svc = self.state.services.lock();
                    svc.rpc.resolve_fn(other).cloned()
                };
                if let Some(entry) = resolved
                    && entry.runtime == PluginRuntimeKind::Amxx
                {
                    let argc = vm.param_count(params_addr)?;
                    let mut raw_params = Vec::with_capacity(argc);
                    for i in 1..=argc {
                        raw_params.push(vm.param(params_addr, i)?);
                    }
                    let str_snapshots: Vec<Option<String>> = raw_params
                        .iter()
                        .map(|&raw| {
                            if raw >= 0 && raw % 4 == 0 {
                                vm.get_amx_string(raw).ok()
                            } else {
                                None
                            }
                        })
                        .collect();
                    self.state.amxx_native_frames.push(AmxxNativeCallFrame {
                        raw_cells: raw_params.clone(),
                        str_snapshots,
                        str_writes: vec![None; argc],
                    });
                    let rc = invoke_plugin_pure_in_slots(
                        self.peers,
                        self.state,
                        entry.owner_index,
                        &entry.callback_name,
                        &[self.plugin_idx as i32, argc as i32],
                    )
                    .unwrap_or(None)
                    .unwrap_or(0);
                    if let Some(frame) = self.state.amxx_native_frames.pop() {
                        for (idx, write) in frame.str_writes.into_iter().enumerate() {
                            if let Some((s, max_len)) = write
                                && let Some(&dest_addr) = raw_params.get(idx)
                                && dest_addr >= 0
                            {
                                let _ = vm.set_amx_string(dest_addr, &s, max_len);
                            }
                        }
                    }
                    return Ok(rc);
                }
                Err(AmxError::InvalidNative(other.to_string()))
            }
        }
    }
}
