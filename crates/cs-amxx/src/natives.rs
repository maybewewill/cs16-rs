//! AMX Mod X natives, Counter-Strike hook trait, and host state.
//!
//! Ported from AMX Mod X GPL-3 (`amxmodx.cpp`, `string.cpp`, `float.cpp`, `cstrike`, `fun`,
//! `engine`, `fakemeta`, `hamsandwich` modules).

use std::collections::HashMap;

use cs_engine::consts::{FL_FAKECLIENT, FL_GODMODE, FL_PROXY};
use cs_engine::{EdictId, EngineApi, MsgDest};
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;
use tracing::{info, warn};

use crate::vm::{AmxError, NativeDispatch, PawnVm};

/// AMX Mod X forward return codes (`amxconst.inc`).
pub const PLUGIN_CONTINUE: i32 = 0;
pub const PLUGIN_HANDLED: i32 = 1;
pub const PLUGIN_HANDLED_MAIN: i32 = 2;

/// Hamsandwich forward return codes (`ham_const.inc`).
pub const HAM_IGNORED: i32 = 1;
pub const HAM_HANDLED: i32 = 2;
pub const HAM_OVERRIDE: i32 = 3;
pub const HAM_SUPERCEDE: i32 = 4;

/// `client_print` destination constants (`amxconst.inc`).
pub const PRINT_NOTIFY: i32 = 1;
pub const PRINT_CONSOLE: i32 = 2;
pub const PRINT_CHAT: i32 = 3;
pub const PRINT_CENTER: i32 = 4;

/// `cs_set_user_armor` / `cs_get_user_armor` armor types (`cstrike.inc`).
pub const CS_ARMOR_NONE: i32 = 0;
pub const CS_ARMOR_KEVLAR: i32 = 1;
pub const CS_ARMOR_VESTHELM: i32 = 2;

/// Hamsandwich virtual hook identifiers (`ham_const.inc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum HamFunction {
    Spawn = 0,
    TakeDamage = 9,
    Killed = 11,
}

impl TryFrom<i32> for HamFunction {
    type Error = ();

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Spawn),
            9 => Ok(Self::TakeDamage),
            11 => Ok(Self::Killed),
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

/// Shared AMX Mod X host state kept separately from the plugin VMs to allow disjoint mutable
/// borrowing during native execution.
#[derive(Debug, Default)]
pub struct AmxxHostState {
    pub plugin_metas: Vec<PluginMeta>,
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
    pub(crate) pending_server_cmds: Vec<String>,
    pub(crate) server_prints: Vec<String>,
    pub(crate) amx_logs: Vec<String>,
    pub(crate) current_argv: Vec<String>,
    pub(crate) connected_slots: [bool; 32],
    pub(crate) admin_flags: [i32; 32],
    pub(crate) armor_types: [i32; 32],
}

impl AmxxHostState {
    pub fn new() -> Self {
        Self {
            next_task_handle: 1,
            ..Self::default()
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

    /// Sets admin flags bitmask (`get_user_flags`) for 0-based `slot`.
    pub fn set_user_flags(&mut self, slot: u8, flags: i32) {
        if let Some(f) = self.admin_flags.get_mut(usize::from(slot)) {
            *f = flags;
        }
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
/// and [`AmxxCsHooks`].
pub struct AmxxNativeDispatch<'a, 'e> {
    pub plugin_idx: usize,
    pub state: &'a mut AmxxHostState,
    pub eng: &'a mut EngineApi<'e>,
    pub hooks: &'a mut dyn AmxxCsHooks,
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

            // --- Printing / Server Commands / Logging ---
            "client_print" => self.native_client_print(vm, params_addr),
            "console_print" => self.native_console_print(vm, params_addr),
            "server_print" => {
                let text = vm.format_amx_string(params_addr, 1, 2)?;
                info!(target: "amxx", "{text}");
                self.state.server_prints.push(text);
                Ok(1)
            }
            "server_cmd" => {
                let cmd = vm.format_amx_string(params_addr, 1, 2)?;
                self.state.pending_server_cmds.push(cmd);
                Ok(1)
            }
            "log_amx" => {
                let text = vm.format_amx_string(params_addr, 1, 2)?;
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
                    .map_or(0, |slot| self.state.admin_flags[usize::from(slot)]))
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

            // --- String / Conversion ---
            "format" | "formatex" => {
                let out_addr = vm.param(params_addr, 1)?;
                let max_len = vm.param(params_addr, 2)?.max(0) as usize;
                let text = vm.format_amx_string(params_addr, 3, 4)?;
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

            other => {
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
            None // infinite until remove_task
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
        let text = vm.format_amx_string(params_addr, 3, 4)?;
        if target == 0 {
            for slot in 0..self.eng.max_clients() {
                if self.eng.is_client_active(slot) || self.state.connected_slots[usize::from(slot)]
                {
                    send_client_print(self.eng, slot, print_type, &text);
                }
            }
            return Ok(1);
        }
        if let Some(slot) = self.connected_slot(target) {
            send_client_print(self.eng, slot, print_type, &text);
            Ok(1)
        } else {
            Ok(0)
        }
    }

    fn native_console_print(&mut self, vm: &mut PawnVm, params_addr: i32) -> Result<i32, AmxError> {
        let target = vm.param(params_addr, 1)?;
        let text = vm.format_amx_string(params_addr, 2, 3)?;
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
        let _id = vm.param(params_addr, 1)?;
        let argc = vm.param_count(params_addr)?;
        if argc >= 2 {
            let clip_addr = vm.param(params_addr, 2)?;
            let _ = vm.write_cell(clip_addr, 0);
        }
        if argc >= 3 {
            let ammo_addr = vm.param(params_addr, 3)?;
            let _ = vm.write_cell(ammo_addr, 0);
        }
        Ok(0)
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

        // Pre-read vector or string before borrowing `eng.ent_mut(eid)`.
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
    matches!(field, PEV_CLASSNAME | PEV_MODEL | PEV_TARGET | PEV_TARGETNAME)
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
