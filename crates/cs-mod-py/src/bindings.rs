//! Rich `cs16` Python runtime bindings: `mod`, `vfx`, `Player`, `Entity`, `Event`, `Menu`,
//! `CustomWeapon`, `Storage`, `Leaderboard`, constants, and decorators.

use std::collections::BTreeMap;

use cs_mod_api::services::{RpcEventSubscriber, RpcExportEntry};
use cs_mod_api::{
    AdminFlags, ArmorKind, BarTimeSpec, CustomWeaponSpec, DhudMessageSpec, HookAction, HookId,
    HookPhase, HttpPostJsonSpec, HudMessageSpec, MenuCreateSpec, MenuDisplaySpec, MenuItemSpec,
    Permission, RpcEmitEventSpec, RpcValue, ScreenFadeSpec, ScreenShakeSpec, ShowMotdSpec,
    TeamKind, VfxOp,
};
use cs_mod_host::natives::{EngineOp, TimerRequest};
use cs_mod_host::webhook::{WEBHOOK_MAX_REQUEST_BYTES, is_host_allowed, parse_webhook_url};

use crate::error::PyError;
use crate::json_codec::json_dumps;
use crate::value::{
    HeapObj, NativeObjKind, ObjId, PyDecoratorKind, PyEventObj, PyKey, PyMenuObj, PyPluginMeta,
    PyVal,
};
use crate::vm::{
    HostBridge, PyVm, RegisteredChatCmd, RegisteredConsoleCmd, RegisteredPyHook,
    RegisteredStartupTimer,
};

pub fn install_cs16_module(vm: &mut PyVm) -> Result<(), PyError> {
    let mod_id = vm.alloc(HeapObj::Native(NativeObjKind::Mod))?;
    let vfx_id = vm.alloc(HeapObj::Native(NativeObjKind::Vfx))?;
    let player_ctor = vm.alloc(HeapObj::NativeFn("cs16.Player".to_string()))?;
    let menu_ctor = vm.alloc(HeapObj::NativeFn("cs16.Menu".to_string()))?;
    let cwpn_ctor = vm.alloc(HeapObj::NativeFn("cs16.CustomWeapon".to_string()))?;

    let color_mod = make_namespace(
        vm,
        "Color",
        vec![
            ("DEFAULT", PyVal::Str("\x01".to_string())),
            ("NORMAL", PyVal::Str("\x01".to_string())),
            ("YELLOW", PyVal::Str("\x01".to_string())),
            ("TEAM", PyVal::Str("\x03".to_string())),
            ("GREEN", PyVal::Str("\x04".to_string())),
            ("MENU_YELLOW", PyVal::Str("\\y".to_string())),
            ("MENU_WHITE", PyVal::Str("\\w".to_string())),
            ("MENU_RED", PyVal::Str("\\r".to_string())),
            ("MENU_GREY", PyVal::Str("\\d".to_string())),
            ("MENU_RIGHT", PyVal::Str("\\R".to_string())),
        ],
    )?;

    let hook_action_mod = make_namespace(
        vm,
        "HookAction",
        vec![
            ("CONTINUE", PyVal::Int(HookAction::Continue as i64)),
            ("HANDLED", PyVal::Int(HookAction::Handled as i64)),
            ("OVERRIDE", PyVal::Int(HookAction::Override as i64)),
            ("SUPERCEDE", PyVal::Int(HookAction::Supercede as i64)),
        ],
    )?;

    let hook_phase_mod = make_namespace(
        vm,
        "HookPhase",
        vec![
            ("PRE", PyVal::Int(HookPhase::Pre as i64)),
            ("POST", PyVal::Int(HookPhase::Post as i64)),
        ],
    )?;

    let armor_mod = make_namespace(
        vm,
        "Armor",
        vec![
            ("NONE", PyVal::Int(ArmorKind::None as i64)),
            ("KEVLAR", PyVal::Int(ArmorKind::Kevlar as i64)),
            ("VESTHELM", PyVal::Int(ArmorKind::VestHelm as i64)),
        ],
    )?;

    let team_mod = make_namespace(
        vm,
        "Team",
        vec![
            ("UNASSIGNED", PyVal::Int(TeamKind::Unassigned as i64)),
            ("TERRORIST", PyVal::Int(TeamKind::Terrorist as i64)),
            ("T", PyVal::Int(TeamKind::Terrorist as i64)),
            ("CT", PyVal::Int(TeamKind::Ct as i64)),
            ("COUNTER_TERRORIST", PyVal::Int(TeamKind::Ct as i64)),
            ("SPECTATOR", PyVal::Int(TeamKind::Spectator as i64)),
            ("SPEC", PyVal::Int(TeamKind::Spectator as i64)),
        ],
    )?;

    let weapon_id_mod = make_namespace(
        vm,
        "WeaponId",
        vec![
            ("NONE", PyVal::Int(0)),
            ("P228", PyVal::Int(1)),
            ("GLOCK", PyVal::Int(2)),
            ("SCOUT", PyVal::Int(3)),
            ("HEGRENADE", PyVal::Int(4)),
            ("XM1014", PyVal::Int(5)),
            ("C4", PyVal::Int(6)),
            ("MAC10", PyVal::Int(7)),
            ("AUG", PyVal::Int(8)),
            ("SMOKEGRENADE", PyVal::Int(9)),
            ("ELITE", PyVal::Int(10)),
            ("FIVESEVEN", PyVal::Int(11)),
            ("UMP45", PyVal::Int(12)),
            ("SG550", PyVal::Int(13)),
            ("GALIL", PyVal::Int(14)),
            ("FAMAS", PyVal::Int(15)),
            ("USP", PyVal::Int(16)),
            ("GLOCK18", PyVal::Int(17)),
            ("AWP", PyVal::Int(18)),
            ("MP5N", PyVal::Int(19)),
            ("MP5NAVY", PyVal::Int(19)),
            ("M249", PyVal::Int(20)),
            ("M3", PyVal::Int(21)),
            ("M4A1", PyVal::Int(22)),
            ("TMP", PyVal::Int(23)),
            ("G3SG1", PyVal::Int(24)),
            ("FLASHBANG", PyVal::Int(25)),
            ("DEAGLE", PyVal::Int(26)),
            ("SG552", PyVal::Int(27)),
            ("AK47", PyVal::Int(28)),
            ("KNIFE", PyVal::Int(29)),
            ("P90", PyVal::Int(30)),
            ("SHIELD", PyVal::Int(99)),
            ("SHIELDGUN", PyVal::Int(99)),
        ],
    )?;

    let button_mod = make_namespace(
        vm,
        "Button",
        vec![
            ("ATTACK", PyVal::Int(1 << 0)),
            ("JUMP", PyVal::Int(1 << 1)),
            ("DUCK", PyVal::Int(1 << 2)),
            ("FORWARD", PyVal::Int(1 << 3)),
            ("BACK", PyVal::Int(1 << 4)),
            ("USE", PyVal::Int(1 << 5)),
            ("CANCEL", PyVal::Int(1 << 6)),
            ("LEFT", PyVal::Int(1 << 7)),
            ("RIGHT", PyVal::Int(1 << 8)),
            ("MOVELEFT", PyVal::Int(1 << 9)),
            ("MOVERIGHT", PyVal::Int(1 << 10)),
            ("ATTACK2", PyVal::Int(1 << 11)),
            ("RUN", PyVal::Int(1 << 12)),
            ("RELOAD", PyVal::Int(1 << 13)),
        ],
    )?;

    let render_fx_mod = make_namespace(
        vm,
        "RenderFx",
        vec![
            ("NONE", PyVal::Int(0)),
            ("PULSE_SLOW", PyVal::Int(1)),
            ("PULSE_FAST", PyVal::Int(2)),
            ("HOLOGRAM", PyVal::Int(16)),
            ("GLOW_SHELL", PyVal::Int(19)),
        ],
    )?;

    let render_mode_mod = make_namespace(
        vm,
        "RenderMode",
        vec![
            ("NORMAL", PyVal::Int(0)),
            ("TRANS_COLOR", PyVal::Int(1)),
            ("TRANS_TEXTURE", PyVal::Int(2)),
            ("GLOW", PyVal::Int(3)),
            ("TRANS_ALPHA", PyVal::Int(4)),
            ("TRANS_ADD", PyVal::Int(5)),
        ],
    )?;

    let hook_id_mod = make_namespace(
        vm,
        "HookId",
        vec![
            ("SERVER_ACTIVATE", PyVal::Int(HookId::ServerActivate as i64)),
            ("SERVER_DEACTIVATE", PyVal::Int(HookId::ServerDeactivate as i64)),
            ("SPAWN_ENTITY", PyVal::Int(HookId::SpawnEntity as i64)),
            ("CLIENT_CONNECT", PyVal::Int(HookId::ClientConnect as i64)),
            ("CLIENT_PUT_IN_SERVER", PyVal::Int(HookId::ClientPutInServer as i64)),
            ("CLIENT_DISCONNECT", PyVal::Int(HookId::ClientDisconnect as i64)),
            ("CLIENT_COMMAND", PyVal::Int(HookId::ClientCommand as i64)),
            ("CLIENT_USERINFO_CHANGED", PyVal::Int(HookId::ClientUserinfoChanged as i64)),
            ("RUN_USERCMD", PyVal::Int(HookId::RunUsercmd as i64)),
            ("POST_USERCMD", PyVal::Int(HookId::PostUsercmd as i64)),
            ("START_FRAME", PyVal::Int(HookId::StartFrame as i64)),
            ("THINK", PyVal::Int(HookId::Think as i64)),
            ("TOUCH", PyVal::Int(HookId::Touch as i64)),
            ("BLOCKED", PyVal::Int(HookId::Blocked as i64)),
            ("SERVER_COMMAND", PyVal::Int(HookId::ServerCommand as i64)),
            ("PLAYER_DEATH", PyVal::Int(HookId::PlayerDeath as i64)),
            ("ROUND_START", PyVal::Int(HookId::RoundStart as i64)),
            ("PLAYER_SPAWN", PyVal::Int(HookId::PlayerSpawn as i64)),
            ("PLAYER_TAKE_DAMAGE", PyVal::Int(HookId::PlayerTakeDamage as i64)),
            ("PLAYER_KILLED", PyVal::Int(HookId::PlayerKilled as i64)),
            ("PLAYER_PRE_THINK", PyVal::Int(HookId::PlayerPreThink as i64)),
            ("PLAYER_POST_THINK", PyVal::Int(HookId::PlayerPostThink as i64)),
            ("PLAYER_JUMP", PyVal::Int(HookId::PlayerJump as i64)),
            ("PLAYERS_SHOULD_COLLIDE", PyVal::Int(HookId::PlayersShouldCollide as i64)),
            ("CAN_BUY_ITEM", PyVal::Int(HookId::CanBuyItem as i64)),
            ("CAN_HAVE_PLAYER_ITEM", PyVal::Int(HookId::CanHavePlayerItem as i64)),
            ("WEAPON_DEPLOY", PyVal::Int(HookId::WeaponDeploy as i64)),
            ("WEAPON_PRIMARY_ATTACK", PyVal::Int(HookId::WeaponPrimaryAttack as i64)),
            ("WEAPON_RELOAD", PyVal::Int(HookId::WeaponReload as i64)),
            ("PLAYER_SAY", PyVal::Int(HookId::PlayerSay as i64)),
            ("MENU_SELECT", PyVal::Int(HookId::MenuSelect as i64)),
            ("FREEZE_TIME_END", PyVal::Int(HookId::FreezeTimeEnd as i64)),
            ("ROUND_END", PyVal::Int(HookId::RoundEnd as i64)),
            ("BOMB_PLANTING", PyVal::Int(HookId::BombPlanting as i64)),
            ("BOMB_PLANTED", PyVal::Int(HookId::BombPlanted as i64)),
            ("BOMB_DEFUSING", PyVal::Int(HookId::BombDefusing as i64)),
            ("BOMB_DEFUSED", PyVal::Int(HookId::BombDefused as i64)),
            ("BOMB_EXPLODED", PyVal::Int(HookId::BombExploded as i64)),
            ("HOSTAGE_TOUCHED", PyVal::Int(HookId::HostageTouched as i64)),
            ("HOSTAGE_RESCUED", PyVal::Int(HookId::HostageRescued as i64)),
            ("GRENADE_THROW", PyVal::Int(HookId::GrenadeThrow as i64)),
        ],
    )?;

    let mut cs16_attrs = BTreeMap::new();
    cs16_attrs.insert("mod".to_string(), PyVal::Handle(mod_id));
    cs16_attrs.insert("vfx".to_string(), PyVal::Handle(vfx_id));
    cs16_attrs.insert("Player".to_string(), PyVal::Handle(player_ctor));
    cs16_attrs.insert("Menu".to_string(), PyVal::Handle(menu_ctor));
    cs16_attrs.insert("CustomWeapon".to_string(), PyVal::Handle(cwpn_ctor));
    cs16_attrs.insert("Color".to_string(), color_mod);
    cs16_attrs.insert("HookAction".to_string(), hook_action_mod);
    cs16_attrs.insert("HookPhase".to_string(), hook_phase_mod);
    cs16_attrs.insert("HookId".to_string(), hook_id_mod);
    cs16_attrs.insert("Armor".to_string(), armor_mod);
    cs16_attrs.insert("Team".to_string(), team_mod);
    cs16_attrs.insert("WeaponId".to_string(), weapon_id_mod);
    cs16_attrs.insert("Button".to_string(), button_mod);
    cs16_attrs.insert("RenderFx".to_string(), render_fx_mod);
    cs16_attrs.insert("RenderMode".to_string(), render_mode_mod);

    let cs16_id = vm.alloc(HeapObj::Native(NativeObjKind::Module {
        name: "cs16".to_string(),
        attrs: cs16_attrs,
    }))?;
    vm.modules.insert("cs16".to_string(), PyVal::Handle(cs16_id));
    Ok(())
}

fn make_namespace(
    vm: &mut PyVm,
    name: &str,
    entries: Vec<(&str, PyVal)>,
) -> Result<PyVal, PyError> {
    let mut attrs = BTreeMap::new();
    for (k, v) in entries {
        attrs.insert(k.to_string(), v);
    }
    let id = vm.alloc(HeapObj::Native(NativeObjKind::Module {
        name: name.to_string(),
        attrs,
    }))?;
    Ok(PyVal::Handle(id))
}

pub fn call_cs16_fn(
    vm: &mut PyVm,
    name: &str,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
    _host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    match name {
        "Player" => {
            let slot_val = args
                .first()
                .or_else(|| kwargs.get("slot"))
                .and_then(PyVal::as_int)
                .ok_or_else(|| vm.runtime_err("TypeError", "Player(slot) expects integer slot"))?;
            let slot = u8::try_from(slot_val)
                .map_err(|_| vm.runtime_err("ValueError", "player slot out of range"))?;
            let id = vm.alloc(HeapObj::Native(NativeObjKind::Player(slot)))?;
            Ok(PyVal::Handle(id))
        }
        "Menu" => make_menu_obj(vm, args, kwargs),
        "CustomWeapon" => make_custom_weapon_obj(vm, args, kwargs),
        _ => Err(vm.runtime_err("RuntimeError", format!("unknown cs16 callable '{name}'"))),
    }
}

fn make_menu_obj(
    vm: &mut PyVm,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
) -> Result<PyVal, PyError> {
    let title = args
        .first()
        .or_else(|| kwargs.get("title"))
        .map(|v| vm.val_to_str(v))
        .unwrap_or_default();
    let callback = args
        .get(1)
        .or_else(|| kwargs.get("callback"))
        .cloned()
        .filter(|v| !matches!(v, PyVal::None));
    let per_page = kwargs
        .get("per_page")
        .and_then(PyVal::as_int)
        .map(|n| n.clamp(0, 7) as u8)
        .unwrap_or(7);
    let allow_exit = kwargs
        .get("allow_exit")
        .map(|v| vm.is_truthy(v))
        .unwrap_or(true);
    let back_label = kwargs.get("back_label").map(|v| vm.val_to_str(v));
    let next_label = kwargs.get("next_label").map(|v| vm.val_to_str(v));
    let exit_label = kwargs.get("exit_label").map(|v| vm.val_to_str(v));
    let number_color = kwargs
        .get("number_color")
        .map(|v| vm.val_to_str(v))
        .unwrap_or_else(|| "\\r".to_string());

    let mut items = Vec::new();
    if let Some(items_val) = kwargs.get("items") {
        for it in vm.to_vec(items_val)? {
            match it {
                PyVal::Str(label) => items.push(MenuItemSpec::selectable(label.clone(), label, true)),
                PyVal::Tuple(t) if t.len() >= 2 => {
                    let label = vm.val_to_str(&t[0]);
                    let info = vm.val_to_str(&t[1]);
                    let enabled = t.get(2).map(|v| vm.is_truthy(v)).unwrap_or(true);
                    items.push(MenuItemSpec::selectable(label, info, enabled));
                }
                _ => {}
            }
        }
    }

    let id = vm.alloc(HeapObj::Native(NativeObjKind::Menu(PyMenuObj {
        title,
        items,
        callback,
        per_page,
        allow_exit,
        back_label,
        next_label,
        exit_label,
        number_color,
        created_id: None,
    })))?;
    Ok(PyVal::Handle(id))
}

fn make_custom_weapon_obj(
    vm: &mut PyVm,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
) -> Result<PyVal, PyError> {
    let mut spec = CustomWeaponSpec::default();
    if let Some(id_v) = args.first().or_else(|| kwargs.get("id_name")).or_else(|| kwargs.get("id")) {
        spec.id_name = vm.val_to_str(id_v);
    }
    if let Some(bw) = args
        .get(1)
        .or_else(|| kwargs.get("base_weapon"))
        .or_else(|| kwargs.get("base"))
        && let Some(n) = bw.as_int()
    {
        spec.base_weapon = n.clamp(0, 255) as u8;
    }
    if let Some(dn) = args.get(2).or_else(|| kwargs.get("display_name")).or_else(|| kwargs.get("name")) {
        spec.display_name = vm.val_to_str(dn);
    }
    if spec.display_name.is_empty() {
        spec.display_name = spec.id_name.clone();
    }
    if let Some(v) = kwargs.get("v_model")
        && !matches!(v, PyVal::None)
    {
        spec.v_model = Some(vm.val_to_str(v));
    }
    if let Some(v) = kwargs.get("p_model")
        && !matches!(v, PyVal::None)
    {
        spec.p_model = Some(vm.val_to_str(v));
    }
    if let Some(v) = kwargs.get("w_model")
        && !matches!(v, PyVal::None)
    {
        spec.w_model = Some(vm.val_to_str(v));
    }
    if let Some(v) = kwargs.get("fire_sound")
        && !matches!(v, PyVal::None)
    {
        spec.fire_sound = Some(vm.val_to_str(v));
    }
    if let Some(f) = kwargs.get("damage_mult").and_then(PyVal::as_float) {
        spec.damage_mult = f as f32;
    }
    if let Some(c) = kwargs.get("clip_size").and_then(PyVal::as_int) {
        spec.clip_size = Some(c.max(0) as i32);
    }
    if let Some(f) = kwargs.get("deploy_speed_mult").and_then(PyVal::as_float) {
        spec.deploy_speed_mult = f as f32;
    }
    if let Some(f) = kwargs.get("recoil_mult").and_then(PyVal::as_float) {
        spec.recoil_mult = f as f32;
    }
    if let Some(f) = kwargs.get("knockback").and_then(PyVal::as_float) {
        spec.knockback = f as f32;
    }
    if let Some(c) = kwargs.get("cost").and_then(PyVal::as_int) {
        spec.cost = c.max(0) as i32;
    }
    let id = vm.alloc(HeapObj::Native(NativeObjKind::CustomWeapon(spec)))?;
    Ok(PyVal::Handle(id))
}

pub fn apply_decorator(
    vm: &mut PyVm,
    dec: PyDecoratorKind,
    args: Vec<PyVal>,
    mut host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    let target = args
        .into_iter()
        .next()
        .ok_or_else(|| vm.runtime_err("TypeError", "decorator expects a function"))?;
    match dec {
        PyDecoratorKind::Info(patch) => {
            merge_meta(&mut vm.meta, patch);
            Ok(target)
        }
        PyDecoratorKind::Hook {
            hook,
            phase,
            priority,
        } => {
            vm.hooks.push(RegisteredPyHook {
                hook,
                phase,
                priority,
                func: target.clone(),
            });
            Ok(target)
        }
        PyDecoratorKind::ChatCommand { commands, suppress } => {
            vm.chat_commands.push(RegisteredChatCmd {
                commands,
                suppress,
                func: target.clone(),
            });
            Ok(target)
        }
        PyDecoratorKind::ConsoleCommand { commands } => {
            vm.console_commands.push(RegisteredConsoleCmd {
                commands,
                func: target.clone(),
            });
            Ok(target)
        }
        PyDecoratorKind::Timer {
            interval_sec,
            repeat,
            arg,
        } => {
            let cb_id = vm.next_callback_id();
            let delay_ms = (interval_sec.max(0.001) * 1000.0).round() as u32;
            vm.timer_callbacks.insert(cb_id, target.clone());
            vm.startup_timers.push(RegisteredStartupTimer {
                delay_ms,
                repeat,
                cb_id,
                arg,
            });
            Ok(target)
        }
        PyDecoratorKind::ExportFn { name } => {
            let cb_id = vm.next_callback_id();
            vm.rpc_exports.insert(cb_id, (name.clone(), target.clone()));
            if let Some(h) = host.as_deref_mut()
                && h.has_perm(Permission::Rpc)
            {
                let mut svc = h.services.lock();
                svc.rpc.export_fn(RpcExportEntry {
                    fn_name: name.clone(),
                    owner_id: h.mod_id.clone(),
                    runtime: h.runtime_kind,
                    owner_index: 0,
                    callback_id: cb_id,
                    callback_name: name,
                });
            }
            Ok(target)
        }
        PyDecoratorKind::OnEvent { event_name } => {
            let cb_id = vm.next_callback_id();
            vm.event_callbacks
                .insert(cb_id, (event_name.clone(), target.clone()));
            if let Some(h) = host
                && h.has_perm(Permission::Rpc)
            {
                let mut svc = h.services.lock();
                svc.rpc.subscribe_event(
                    event_name.clone(),
                    RpcEventSubscriber {
                        owner_id: h.mod_id.clone(),
                        runtime: h.runtime_kind,
                        owner_index: 0,
                        callback_id: cb_id,
                        callback_name: event_name,
                    },
                );
            }
            Ok(target)
        }
    }
}

fn merge_meta(dst: &mut PyPluginMeta, patch: PyPluginMeta) {
    if patch.id.is_some() {
        dst.id = patch.id;
    }
    if patch.name.is_some() {
        dst.name = patch.name;
    }
    if patch.version.is_some() {
        dst.version = patch.version;
    }
    if patch.author.is_some() {
        dst.author = patch.author;
    }
    if patch.description.is_some() {
        dst.description = patch.description;
    }
    if !patch.permissions.is_empty() {
        dst.permissions = patch.permissions;
    }
    if patch.fuel_per_frame.is_some() {
        dst.fuel_per_frame = patch.fuel_per_frame;
    }
}

pub fn get_native_attr(
    vm: &mut PyVm,
    obj_id: ObjId,
    native: NativeObjKind,
    attr: &str,
    host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    match native {
        NativeObjKind::Module { attrs, name } => attrs.get(attr).cloned().ok_or_else(|| {
            vm.runtime_err(
                "AttributeError",
                format!("module '{name}' has no attribute '{attr}'"),
            )
        }),
        NativeObjKind::Mod => match attr {
            "vfx" => {
                let id = vm.alloc(HeapObj::Native(NativeObjKind::Vfx))?;
                Ok(PyVal::Handle(id))
            }
            "storage" => {
                let id = vm.alloc(HeapObj::Native(NativeObjKind::Storage))?;
                Ok(PyVal::Handle(id))
            }
            "leaderboard" => {
                let id = vm.alloc(HeapObj::Native(NativeObjKind::Leaderboard))?;
                Ok(PyVal::Handle(id))
            }
            "id" => Ok(PyVal::Str(vm.mod_id.clone())),
            "time" => Ok(PyVal::Float(
                host.as_deref().map_or(0.0, |h| h.snapshot.time_secs),
            )),
            "frametime" => Ok(PyVal::Float(
                host.as_deref().map_or(0.01, |h| f64::from(h.snapshot.frametime)),
            )),
            "max_clients" => Ok(PyVal::Int(
                host.as_deref().map_or(32, |h| i64::from(h.snapshot.max_clients)),
            )),
            "map_name" => Ok(PyVal::Str(
                host.as_deref()
                    .map(|h| h.snapshot.map_name.clone())
                    .unwrap_or_default(),
            )),
            _ => {
                let mid = vm.alloc(HeapObj::BoundNativeMethod {
                    receiver: PyVal::Handle(obj_id),
                    method: format!("mod.{attr}"),
                })?;
                Ok(PyVal::Handle(mid))
            }
        },
        NativeObjKind::Vfx => {
            let mid = vm.alloc(HeapObj::BoundNativeMethod {
                receiver: PyVal::Handle(obj_id),
                method: format!("vfx.{attr}"),
            })?;
            Ok(PyVal::Handle(mid))
        }
        NativeObjKind::Storage => {
            let mid = vm.alloc(HeapObj::BoundNativeMethod {
                receiver: PyVal::Handle(obj_id),
                method: format!("storage.{attr}"),
            })?;
            Ok(PyVal::Handle(mid))
        }
        NativeObjKind::Leaderboard => {
            let mid = vm.alloc(HeapObj::BoundNativeMethod {
                receiver: PyVal::Handle(obj_id),
                method: format!("leaderboard.{attr}"),
            })?;
            Ok(PyVal::Handle(mid))
        }
        NativeObjKind::Player(slot) => get_player_attr(vm, obj_id, slot, attr, host.as_deref()),
        NativeObjKind::Entity(index) => get_entity_attr(vm, obj_id, index, attr, host.as_deref()),
        NativeObjKind::Event(ev) => {
            if let Some(v) = ev.fields.get(attr) {
                return Ok(v.clone());
            }
            if attr == "hook" {
                return Ok(PyVal::Int(ev.hook as i64));
            }
            if attr == "phase" {
                return Ok(PyVal::Int(ev.phase as i64));
            }
            // Ergonomic delegation for single-player hooks so `def on_spawn(p): p.give_item(...)` works!
            if let Some(slot) = ev.single_player_slot {
                let p_id = vm.alloc(HeapObj::Native(NativeObjKind::Player(slot)))?;
                return get_player_attr(vm, p_id, slot, attr, host.as_deref());
            }
            Err(vm.runtime_err("AttributeError", format!("'Event' has no attribute '{attr}'")))
        }
        NativeObjKind::Menu(m) => match attr {
            "title" => Ok(PyVal::Str(m.title)),
            "per_page" => Ok(PyVal::Int(i64::from(m.per_page))),
            "allow_exit" => Ok(PyVal::Bool(m.allow_exit)),
            "id" => Ok(m.created_id.map_or(PyVal::None, |i| PyVal::Int(i64::from(i)))),
            _ => {
                let mid = vm.alloc(HeapObj::BoundNativeMethod {
                    receiver: PyVal::Handle(obj_id),
                    method: format!("menu.{attr}"),
                })?;
                Ok(PyVal::Handle(mid))
            }
        },
        NativeObjKind::CustomWeapon(spec) => match attr {
            "id_name" | "id" => Ok(PyVal::Str(spec.id_name)),
            "base_weapon" => Ok(PyVal::Int(i64::from(spec.base_weapon))),
            "display_name" => Ok(PyVal::Str(spec.display_name)),
            "damage_mult" => Ok(PyVal::Float(f64::from(spec.damage_mult))),
            "cost" => Ok(PyVal::Int(i64::from(spec.cost))),
            _ => Err(vm.runtime_err("AttributeError", format!("'CustomWeapon' has no attribute '{attr}'"))),
        },
        NativeObjKind::ExceptionObj { kind, message } => match attr {
            "kind" => Ok(PyVal::Str(kind)),
            "message" => Ok(PyVal::Str(message)),
            "args" => Ok(PyVal::Tuple(vec![PyVal::Str(message)])),
            _ => Err(vm.runtime_err("AttributeError", format!("exception has no attribute '{attr}'"))),
        },
        _ => Err(vm.runtime_err("AttributeError", format!("object has no attribute '{attr}'"))),
    }
}

fn get_player_attr(
    vm: &mut PyVm,
    player_id: ObjId,
    slot: u8,
    attr: &str,
    host: Option<&HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    let ent_idx = u16::from(slot) + 1;
    let client = host.and_then(|h| h.snapshot.client(slot));
    let ent = host.and_then(|h| h.snapshot.entities.get(&ent_idx));
    match attr {
        "slot" => Ok(PyVal::Int(i64::from(slot))),
        "index" | "ent_index" => Ok(PyVal::Int(i64::from(ent_idx))),
        "active" => Ok(PyVal::Bool(client.is_some_and(|c| c.active))),
        "name" => Ok(PyVal::Str(client.map(|c| c.name.clone()).unwrap_or_default())),
        "steamid" => Ok(PyVal::Str(
            client.map(|c| c.steamid.clone()).unwrap_or_default(),
        )),
        "ip" => Ok(PyVal::Str(client.map(|c| c.ip.clone()).unwrap_or_default())),
        "ping" => Ok(PyVal::Int(client.map_or(0, |c| i64::from(c.ping)))),
        "loss" => Ok(PyVal::Int(client.map_or(0, |c| i64::from(c.loss)))),
        "health" => {
            let hp = client
                .map(|c| c.health)
                .filter(|&h| h > 0.0)
                .or_else(|| ent.map(|e| e.health))
                .unwrap_or(0.0);
            Ok(PyVal::Float(f64::from(hp)))
        }
        "max_health" => Ok(PyVal::Float(
            client.map_or(100.0, |c| f64::from(c.max_health.max(1.0))),
        )),
        "armor" => Ok(PyVal::Int(client.map_or(0, |c| i64::from(c.armor)))),
        "armor_kind" => Ok(PyVal::Int(client.map_or(0, |c| i64::from(c.armor_kind)))),
        "money" => Ok(PyVal::Int(client.map_or(0, |c| i64::from(c.money)))),
        "frags" => Ok(PyVal::Int(client.map_or(0, |c| i64::from(c.frags)))),
        "deaths" => Ok(PyVal::Int(client.map_or(0, |c| i64::from(c.deaths)))),
        "score" => Ok(PyVal::Tuple(vec![
            PyVal::Int(client.map_or(0, |c| i64::from(c.frags))),
            PyVal::Int(client.map_or(0, |c| i64::from(c.deaths))),
        ])),
        "team" => Ok(PyVal::Int(client.map_or(0, |c| i64::from(c.team)))),
        "alive" | "is_alive" => Ok(PyVal::Bool(client.is_some_and(|c| c.alive))),
        "active_weapon" => Ok(PyVal::Int(
            client.map_or(0, |c| i64::from(c.active_weapon)),
        )),
        "clip" => Ok(PyVal::Int(client.map_or(0, |c| i64::from(c.clip)))),
        "bpammo" => Ok(PyVal::Int(client.map_or(0, |c| i64::from(c.bpammo)))),
        "has_defuser" => Ok(PyVal::Bool(client.is_some_and(|c| c.has_defuser))),
        "has_nvg" => Ok(PyVal::Bool(client.is_some_and(|c| c.has_nvg))),
        "admin_flags" => {
            let snap_bits = client.map_or(0, |c| c.admin_flags);
            let svc_bits = host.map_or(0, |h| h.services.lock().admins.player_flags(slot).bits());
            Ok(PyVal::Int(i64::from(snap_bits | svc_bits)))
        }
        "lang" => {
            let code = host
                .map(|h| h.services.lock().lang.player_lang(Some(slot)).to_string())
                .or_else(|| client.map(|c| c.lang.clone()))
                .unwrap_or_else(|| "en".to_string());
            Ok(PyVal::Str(code))
        }
        "origin" => {
            let o = ent.map_or([0.0; 3], |e| e.origin);
            Ok(PyVal::Tuple(vec![
                PyVal::Float(f64::from(o[0])),
                PyVal::Float(f64::from(o[1])),
                PyVal::Float(f64::from(o[2])),
            ]))
        }
        "velocity" => {
            let v = ent.map_or([0.0; 3], |e| e.velocity);
            Ok(PyVal::Tuple(vec![
                PyVal::Float(f64::from(v[0])),
                PyVal::Float(f64::from(v[1])),
                PyVal::Float(f64::from(v[2])),
            ]))
        }
        "aim_origin" => {
            let a = client.map_or([0.0; 3], |c| c.aim_origin);
            Ok(PyVal::Tuple(vec![
                PyVal::Float(f64::from(a[0])),
                PyVal::Float(f64::from(a[1])),
                PyVal::Float(f64::from(a[2])),
            ]))
        }
        "flags" => Ok(PyVal::Int(ent.map_or(0, |e| i64::from(e.flags)))),
        "on_ground" => {
            let flags = ent.map_or(0, |e| e.flags);
            Ok(PyVal::Bool((flags & (1 << 9)) != 0))
        }
        "buttons" => {
            let btn = host
                .filter(|h| h.event.i32_args[0] == i32::from(slot))
                .map_or(0, |h| i64::from(h.event.i32_args[1]));
            Ok(PyVal::Int(btn))
        }
        _ => {
            let mid = vm.alloc(HeapObj::BoundNativeMethod {
                receiver: PyVal::Handle(player_id),
                method: format!("player.{attr}"),
            })?;
            Ok(PyVal::Handle(mid))
        }
    }
}

fn get_entity_attr(
    vm: &mut PyVm,
    ent_id: ObjId,
    index: u16,
    attr: &str,
    host: Option<&HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    let ent = host.and_then(|h| h.snapshot.entities.get(&index));
    match attr {
        "index" => Ok(PyVal::Int(i64::from(index))),
        "classname" => Ok(PyVal::Str(
            ent.map(|e| e.classname.clone()).unwrap_or_default(),
        )),
        "health" => Ok(PyVal::Float(ent.map_or(0.0, |e| f64::from(e.health)))),
        "flags" => Ok(PyVal::Int(ent.map_or(0, |e| i64::from(e.flags)))),
        "origin" => {
            let o = ent.map_or([0.0; 3], |e| e.origin);
            Ok(PyVal::Tuple(vec![
                PyVal::Float(f64::from(o[0])),
                PyVal::Float(f64::from(o[1])),
                PyVal::Float(f64::from(o[2])),
            ]))
        }
        "velocity" => {
            let v = ent.map_or([0.0; 3], |e| e.velocity);
            Ok(PyVal::Tuple(vec![
                PyVal::Float(f64::from(v[0])),
                PyVal::Float(f64::from(v[1])),
                PyVal::Float(f64::from(v[2])),
            ]))
        }
        _ => {
            let mid = vm.alloc(HeapObj::BoundNativeMethod {
                receiver: PyVal::Handle(ent_id),
                method: format!("entity.{attr}"),
            })?;
            Ok(PyVal::Handle(mid))
        }
    }
}

pub fn set_native_attr(
    vm: &mut PyVm,
    obj_id: ObjId,
    attr: &str,
    val: PyVal,
    host: Option<&mut HostBridge<'_>>,
) -> Result<(), PyError> {
    let kind_clone = match vm.heap.get(obj_id) {
        Some(HeapObj::Native(n)) => n.clone(),
        _ => return Err(vm.runtime_err("AttributeError", "not a native object")),
    };
    match kind_clone {
        NativeObjKind::Event(_) => {
            let str_val = vm.val_to_str(&val);
            let float_val = val.as_float();
            let int_val = val.as_int();
            let bool_val = vm.is_truthy(&val);
            if let Some(HeapObj::Native(NativeObjKind::Event(ev))) = vm.heap.get_mut(obj_id) {
                ev.fields.insert(attr.to_string(), val);
                match attr {
                    "damage" => {
                        if let Some(f) = float_val {
                            ev.mutated_damage = Some(f as f32);
                        }
                    }
                    "hitgroup" => {
                        if let Some(i) = int_val {
                            ev.mutated_hitgroup = Some(i as i32);
                        }
                    }
                    "message" => {
                        ev.mutated_message = Some(str_val);
                    }
                    "price" => {
                        if let Some(i) = int_val {
                            ev.mutated_price = Some(i as i32);
                        }
                    }
                    "should_collide" => {
                        ev.mutated_should_collide = Some(bool_val);
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        NativeObjKind::Menu(_) => {
            let s = vm.val_to_str(&val);
            let n = val.as_int();
            let b = vm.is_truthy(&val);
            if let Some(HeapObj::Native(NativeObjKind::Menu(m))) = vm.heap.get_mut(obj_id) {
                match attr {
                    "title" => m.title = s,
                    "per_page" => {
                        if let Some(v) = n {
                            m.per_page = v.clamp(0, 7) as u8;
                        }
                    }
                    "allow_exit" => m.allow_exit = b,
                    "back_label" => m.back_label = Some(s),
                    "next_label" => m.next_label = Some(s),
                    "exit_label" => m.exit_label = Some(s),
                    "number_color" => m.number_color = s,
                    "callback" => m.callback = Some(val),
                    _ => {
                        return Err(vm.runtime_err(
                            "AttributeError",
                            format!("'Menu' has no writable attribute '{attr}'"),
                        ));
                    }
                }
            }
            Ok(())
        }
        NativeObjKind::Player(slot) => set_player_attr(vm, slot, attr, val, host),
        NativeObjKind::Entity(index) => set_entity_attr(vm, index, attr, val, host),
        _ => Err(vm.runtime_err(
            "AttributeError",
            format!("cannot set attribute '{attr}' on native object"),
        )),
    }
}

fn set_player_attr(
    vm: &mut PyVm,
    slot: u8,
    attr: &str,
    val: PyVal,
    host: Option<&mut HostBridge<'_>>,
) -> Result<(), PyError> {
    let Some(h) = host else {
        return Err(vm.runtime_err("RuntimeError", "cannot mutate player outside active hook"));
    };
    let ent_idx = u16::from(slot) + 1;
    match attr {
        "health" => {
            require_perm(vm, h, Permission::Entities)?;
            let hp = val
                .as_float()
                .ok_or_else(|| vm.runtime_err("TypeError", "health must be numeric"))?;
            h.ops.push(EngineOp::SetPlayerHealth {
                slot,
                health: hp as f32,
            });
        }
        "max_health" => {
            require_perm(vm, h, Permission::Entities)?;
            let hp = val
                .as_float()
                .ok_or_else(|| vm.runtime_err("TypeError", "max_health must be numeric"))?;
            h.ops.push(EngineOp::SetPlayerMaxHealth {
                slot,
                max_health: hp as f32,
            });
        }
        "armor" => {
            require_perm(vm, h, Permission::Entities)?;
            let a = val
                .as_int()
                .ok_or_else(|| vm.runtime_err("TypeError", "armor must be int"))?;
            h.ops.push(EngineOp::SetPlayerArmor {
                slot,
                armor: a.max(0) as i32,
                kind: if a > 0 { ArmorKind::VestHelm } else { ArmorKind::None },
            });
        }
        "money" => {
            require_perm(vm, h, Permission::Entities)?;
            let m = val
                .as_int()
                .ok_or_else(|| vm.runtime_err("TypeError", "money must be int"))?;
            h.ops.push(EngineOp::SetPlayerMoney {
                slot,
                amount: m.max(0) as i32,
                flash: true,
            });
        }
        "frags" => {
            require_perm(vm, h, Permission::Entities)?;
            let frags = val
                .as_int()
                .ok_or_else(|| vm.runtime_err("TypeError", "frags must be int"))? as i32;
            let deaths = h.snapshot.client(slot).map_or(0, |c| c.deaths);
            h.ops.push(EngineOp::SetPlayerScore {
                slot,
                frags,
                deaths,
            });
        }
        "deaths" => {
            require_perm(vm, h, Permission::Entities)?;
            let deaths = val
                .as_int()
                .ok_or_else(|| vm.runtime_err("TypeError", "deaths must be int"))? as i32;
            let frags = h.snapshot.client(slot).map_or(0, |c| c.frags);
            h.ops.push(EngineOp::SetPlayerScore {
                slot,
                frags,
                deaths,
            });
        }
        "score" => {
            require_perm(vm, h, Permission::Entities)?;
            let pair = vm.to_vec(&val)?;
            if pair.len() != 2 {
                return Err(vm.runtime_err("ValueError", "score expects (frags, deaths)"));
            }
            let frags = pair[0].as_int().unwrap_or(0) as i32;
            let deaths = pair[1].as_int().unwrap_or(0) as i32;
            h.ops.push(EngineOp::SetPlayerScore {
                slot,
                frags,
                deaths,
            });
        }
        "team" => {
            require_perm(vm, h, Permission::Entities)?;
            let t = val.as_int().unwrap_or(0) as i32;
            h.ops.push(EngineOp::SetPlayerTeam {
                slot,
                team: TeamKind::from_i32(t),
                model_class: None,
            });
        }
        "origin" => {
            require_perm(vm, h, Permission::Entities)?;
            let vec3 = extract_vec3(vm, &val)?;
            h.ops.push(EngineOp::SetOrigin {
                index: ent_idx,
                origin: vec3,
            });
        }
        "velocity" => {
            require_perm(vm, h, Permission::Entities)?;
            let vec3 = extract_vec3(vm, &val)?;
            h.ops.push(EngineOp::SetVelocity {
                index: ent_idx,
                velocity: vec3,
            });
        }
        "gravity" => {
            require_perm(vm, h, Permission::Entities)?;
            let g = val
                .as_float()
                .ok_or_else(|| vm.runtime_err("TypeError", "gravity must be numeric"))?;
            h.ops.push(EngineOp::SetPlayerGravity {
                slot,
                gravity: g as f32,
            });
        }
        "maxspeed" => {
            require_perm(vm, h, Permission::Entities)?;
            let sp = val
                .as_float()
                .ok_or_else(|| vm.runtime_err("TypeError", "maxspeed must be numeric"))?;
            h.ops.push(EngineOp::SetPlayerMaxspeed {
                slot,
                maxspeed: sp as f32,
            });
        }
        "godmode" => {
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerGodmode {
                slot,
                enabled: vm.is_truthy(&val),
            });
        }
        "noclip" => {
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerNoclip {
                slot,
                enabled: vm.is_truthy(&val),
            });
        }
        "footsteps" => {
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerFootsteps {
                slot,
                silent: !vm.is_truthy(&val),
            });
        }
        "has_defuser" => {
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetUserDefuser {
                slot,
                has: vm.is_truthy(&val),
            });
        }
        "has_nvg" => {
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetUserNvg {
                slot,
                has: vm.is_truthy(&val),
            });
        }
        "model" => {
            require_perm(vm, h, Permission::Entities)?;
            if matches!(val, PyVal::None) {
                h.ops.push(EngineOp::ResetPlayerModel { slot });
            } else {
                h.ops.push(EngineOp::SetPlayerModel {
                    slot,
                    model: vm.val_to_str(&val),
                });
            }
        }
        "lang" => {
            let code = vm.val_to_str(&val);
            let mut svc = h.services.lock();
            let _ = svc.lang.set_player_lang(slot, None, &code);
        }
        _ => {
            return Err(vm.runtime_err(
                "AttributeError",
                format!("'Player' has no writable attribute '{attr}'"),
            ));
        }
    }
    Ok(())
}

fn set_entity_attr(
    vm: &mut PyVm,
    index: u16,
    attr: &str,
    val: PyVal,
    host: Option<&mut HostBridge<'_>>,
) -> Result<(), PyError> {
    let Some(h) = host else {
        return Err(vm.runtime_err("RuntimeError", "cannot mutate entity outside active hook"));
    };
    require_perm(vm, h, Permission::Entities)?;
    match attr {
        "health" => {
            let hp = val
                .as_float()
                .ok_or_else(|| vm.runtime_err("TypeError", "health must be numeric"))?;
            h.ops.push(EngineOp::SetHealth {
                index,
                health: hp as f32,
            });
        }
        "origin" => {
            h.ops.push(EngineOp::SetOrigin {
                index,
                origin: extract_vec3(vm, &val)?,
            });
        }
        "velocity" => {
            h.ops.push(EngineOp::SetVelocity {
                index,
                velocity: extract_vec3(vm, &val)?,
            });
        }
        "flags" => {
            let fl = val.as_int().unwrap_or(0) as u32;
            h.ops.push(EngineOp::SetFlags { index, flags: fl });
        }
        _ => {
            return Err(vm.runtime_err(
                "AttributeError",
                format!("'Entity' has no writable attribute '{attr}'"),
            ));
        }
    }
    Ok(())
}

pub fn call_native_method(
    vm: &mut PyVm,
    receiver: PyVal,
    method: &str,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
    host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    if let Some(m) = method.strip_prefix("mod.") {
        return call_mod_method(vm, m, args, kwargs, host);
    }
    if let Some(m) = method.strip_prefix("vfx.") {
        return call_vfx_method(vm, m, args, kwargs, host);
    }
    if let Some(m) = method.strip_prefix("storage.") {
        return call_storage_method(vm, m, args, host);
    }
    if let Some(m) = method.strip_prefix("leaderboard.") {
        return call_leaderboard_method(vm, m, args, kwargs, host);
    }
    if let Some(m) = method.strip_prefix("player.") {
        let slot = match receiver {
            PyVal::Handle(id) => match vm.heap.get(id) {
                Some(HeapObj::Native(NativeObjKind::Player(s))) => *s,
                _ => return Err(vm.runtime_err("TypeError", "expected Player")),
            },
            _ => return Err(vm.runtime_err("TypeError", "expected Player")),
        };
        return call_player_method(vm, slot, m, args, kwargs, host);
    }
    if let Some(m) = method.strip_prefix("menu.") {
        let PyVal::Handle(menu_obj_id) = receiver else {
            return Err(vm.runtime_err("TypeError", "expected Menu"));
        };
        return call_menu_method(vm, menu_obj_id, m, args, kwargs, host);
    }
    if let Some(m) = method.strip_prefix("entity.") {
        let idx = match receiver {
            PyVal::Handle(id) => match vm.heap.get(id) {
                Some(HeapObj::Native(NativeObjKind::Entity(i))) => *i,
                _ => return Err(vm.runtime_err("TypeError", "expected Entity")),
            },
            _ => return Err(vm.runtime_err("TypeError", "expected Entity")),
        };
        if m == "remove" {
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::RemoveEntity { index: idx });
            return Ok(PyVal::None);
        }
    }
    Err(vm.runtime_err("AttributeError", format!("unknown native method '{method}'")))
}

fn call_mod_method(
    vm: &mut PyVm,
    method: &str,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
    mut host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    match method {
        // --- Decorators ---
        "info" => {
            let mut patch = PyPluginMeta::default();
            if let Some(v) = kwargs.get("id").or_else(|| args.first()) {
                patch.id = Some(vm.val_to_str(v));
            }
            if let Some(v) = kwargs.get("name").or_else(|| args.get(1)) {
                patch.name = Some(vm.val_to_str(v));
            }
            if let Some(v) = kwargs.get("version").or_else(|| args.get(2)) {
                patch.version = Some(vm.val_to_str(v));
            }
            if let Some(v) = kwargs.get("author") {
                patch.author = Some(vm.val_to_str(v));
            }
            if let Some(v) = kwargs.get("description") {
                patch.description = Some(vm.val_to_str(v));
            }
            if let Some(v) = kwargs.get("permissions") {
                let mut perms = Vec::new();
                for item in vm.to_vec(v)? {
                    perms.push(vm.val_to_str(&item));
                }
                patch.permissions = perms;
            }
            if let Some(v) = kwargs.get("fuel_per_frame").and_then(PyVal::as_int) {
                patch.fuel_per_frame = Some(v.max(1_000) as u64);
            }
            // Apply immediately so standalone `mod.info(...)` works, AND return identity decorator!
            merge_meta(&mut vm.meta, patch.clone());
            let did = vm.alloc(HeapObj::Native(NativeObjKind::Decorator(
                PyDecoratorKind::Info(patch),
            )))?;
            Ok(PyVal::Handle(did))
        }
        "hook" => {
            let raw_hook = args
                .first()
                .or_else(|| kwargs.get("hook"))
                .ok_or_else(|| vm.runtime_err("TypeError", "mod.hook() requires a hook name or HookId"))?;
            let hook = parse_hook_id(vm, raw_hook)?;
            let phase = if let Some(p) = kwargs.get("phase").or_else(|| args.get(1)) {
                parse_hook_phase(vm, p)?
            } else if let Some(post) = kwargs.get("post") {
                if vm.is_truthy(post) {
                    HookPhase::Post
                } else {
                    HookPhase::Pre
                }
            } else {
                default_phase_for_hook(hook)
            };
            let priority = kwargs
                .get("priority")
                .or_else(|| args.get(2))
                .and_then(PyVal::as_int)
                .unwrap_or(0) as i32;
            let did = vm.alloc(HeapObj::Native(NativeObjKind::Decorator(
                PyDecoratorKind::Hook {
                    hook,
                    phase,
                    priority,
                },
            )))?;
            Ok(PyVal::Handle(did))
        }
        "chat_command" => {
            let mut commands = Vec::new();
            for a in &args {
                match a {
                    PyVal::Str(s) => commands.push(s.clone()),
                    _ => {
                        for item in vm.to_vec(a)? {
                            commands.push(vm.val_to_str(&item));
                        }
                    }
                }
            }
            if let Some(c) = kwargs.get("commands") {
                for item in vm.to_vec(c)? {
                    commands.push(vm.val_to_str(&item));
                }
            }
            if commands.is_empty() {
                return Err(vm.runtime_err("ValueError", "chat_command() requires at least one command"));
            }
            let suppress = kwargs
                .get("suppress")
                .or_else(|| kwargs.get("silent"))
                .map(|v| vm.is_truthy(v))
                .unwrap_or(true);
            let did = vm.alloc(HeapObj::Native(NativeObjKind::Decorator(
                PyDecoratorKind::ChatCommand { commands, suppress },
            )))?;
            Ok(PyVal::Handle(did))
        }
        "console_command" => {
            let mut commands = Vec::new();
            for a in &args {
                match a {
                    PyVal::Str(s) => commands.push(s.clone()),
                    _ => {
                        for item in vm.to_vec(a)? {
                            commands.push(vm.val_to_str(&item));
                        }
                    }
                }
            }
            if commands.is_empty() {
                return Err(vm.runtime_err("ValueError", "console_command() requires at least one command"));
            }
            let did = vm.alloc(HeapObj::Native(NativeObjKind::Decorator(
                PyDecoratorKind::ConsoleCommand { commands },
            )))?;
            Ok(PyVal::Handle(did))
        }
        "timer" => {
            let interval = args
                .first()
                .or_else(|| kwargs.get("interval"))
                .or_else(|| kwargs.get("delay"))
                .and_then(PyVal::as_float)
                .ok_or_else(|| vm.runtime_err("TypeError", "mod.timer() expects interval seconds"))?;
            let repeat = args
                .get(1)
                .or_else(|| kwargs.get("repeat"))
                .map(|v| vm.is_truthy(v))
                .unwrap_or(true);
            let arg = args
                .get(2)
                .or_else(|| kwargs.get("arg"))
                .and_then(PyVal::as_int)
                .unwrap_or(0) as i32;
            let did = vm.alloc(HeapObj::Native(NativeObjKind::Decorator(
                PyDecoratorKind::Timer {
                    interval_sec: interval,
                    repeat,
                    arg,
                },
            )))?;
            Ok(PyVal::Handle(did))
        }
        "export_fn" => {
            let fn_name = args
                .first()
                .or_else(|| kwargs.get("name"))
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "export_fn() requires function name"))?;
            let dec = PyDecoratorKind::ExportFn { name: fn_name };
            if let Some(cb) = args.get(1).or_else(|| kwargs.get("callback")).cloned() {
                return apply_decorator(vm, dec, vec![cb], host);
            }
            let did = vm.alloc(HeapObj::Native(NativeObjKind::Decorator(dec)))?;
            Ok(PyVal::Handle(did))
        }
        "on_event" | "subscribe_event" => {
            let ev_name = args
                .first()
                .or_else(|| kwargs.get("name"))
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "on_event() requires event name"))?;
            let dec = PyDecoratorKind::OnEvent {
                event_name: ev_name,
            };
            if let Some(cb) = args.get(1).or_else(|| kwargs.get("callback")).cloned() {
                return apply_decorator(vm, dec, vec![cb], host);
            }
            let did = vm.alloc(HeapObj::Native(NativeObjKind::Decorator(dec)))?;
            Ok(PyVal::Handle(did))
        }

        // --- Runtime Queries & Actions ---
        "players" | "all_players" => {
            let only_active = method == "players";
            let alive_only = kwargs.get("alive_only").is_some_and(|v| vm.is_truthy(v));
            let team_filter = kwargs.get("team").and_then(PyVal::as_int);
            let slots: Vec<u8> = match host.as_deref() {
                Some(h) => h
                    .snapshot
                    .clients
                    .iter()
                    .filter(|c| !only_active || c.active)
                    .filter(|c| !alive_only || c.alive)
                    .filter(|c| team_filter.is_none_or(|t| i64::from(c.team) == t))
                    .map(|c| c.slot)
                    .collect(),
                None => Vec::new(),
            };
            let mut out = Vec::with_capacity(slots.len());
            for s in slots {
                let id = vm.alloc(HeapObj::Native(NativeObjKind::Player(s)))?;
                out.push(PyVal::Handle(id));
            }
            let lid = vm.alloc(HeapObj::List(out))?;
            Ok(PyVal::Handle(lid))
        }
        "player" => {
            let slot_n = args
                .first()
                .and_then(PyVal::as_int)
                .ok_or_else(|| vm.runtime_err("TypeError", "mod.player(slot) expects int"))?;
            let Ok(slot) = u8::try_from(slot_n) else {
                return Ok(PyVal::None);
            };
            let exists = host
                .as_deref()
                .is_some_and(|h| h.snapshot.client(slot).is_some());
            if !exists {
                return Ok(PyVal::None);
            }
            let id = vm.alloc(HeapObj::Native(NativeObjKind::Player(slot)))?;
            Ok(PyVal::Handle(id))
        }
        "entity" => {
            let idx_n = args
                .first()
                .and_then(PyVal::as_int)
                .ok_or_else(|| vm.runtime_err("TypeError", "mod.entity(index) expects int"))?;
            let Ok(idx) = u16::try_from(idx_n) else {
                return Ok(PyVal::None);
            };
            let exists = host
                .as_deref()
                .is_some_and(|h| h.snapshot.entities.contains_key(&idx));
            if !exists {
                return Ok(PyVal::None);
            }
            let id = vm.alloc(HeapObj::Native(NativeObjKind::Entity(idx)))?;
            Ok(PyVal::Handle(id))
        }
        "entities" => {
            let cls_filter = args
                .first()
                .or_else(|| kwargs.get("classname"))
                .and_then(PyVal::as_str)
                .map(str::to_string);
            let indices: Vec<u16> = match host.as_deref() {
                Some(h) => h
                    .snapshot
                    .entities
                    .values()
                    .filter(|e| cls_filter.as_deref().is_none_or(|c| e.classname == c))
                    .map(|e| e.index)
                    .collect(),
                None => Vec::new(),
            };
            let mut out = Vec::with_capacity(indices.len());
            for idx in indices {
                let id = vm.alloc(HeapObj::Native(NativeObjKind::Entity(idx)))?;
                out.push(PyVal::Handle(id));
            }
            let lid = vm.alloc(HeapObj::List(out))?;
            Ok(PyVal::Handle(lid))
        }
        "create_entity" => {
            let cls = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "create_entity(classname) requires classname"))?;
            let Some(h) = host else {
                vm.init_ops.push(EngineOp::CreateEntity { classname: cls });
                return Ok(PyVal::None);
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::CreateEntity { classname: cls });
            Ok(PyVal::None)
        }
        "remove_entity" => {
            let idx = args
                .first()
                .and_then(PyVal::as_int)
                .and_then(|n| u16::try_from(n).ok())
                .ok_or_else(|| vm.runtime_err("TypeError", "remove_entity(index) expects entity index"))?;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::RemoveEntity { index: idx });
            Ok(PyVal::None)
        }
        "menu" => make_menu_obj(vm, args, kwargs),
        "chat_all" | "say_all" => {
            let text = args.first().map(|v| vm.val_to_str(v)).unwrap_or_default();
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Messages)?;
            h.ops.push(EngineOp::SayText { slot: None, text });
            Ok(PyVal::None)
        }
        "print_all" => {
            let text = args.first().map(|v| vm.val_to_str(v)).unwrap_or_default();
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Messages)?;
            h.ops.push(EngineOp::PrintAll { text });
            Ok(PyVal::None)
        }
        "center_print" => {
            let text = args.first().map(|v| vm.val_to_str(v)).unwrap_or_default();
            let slot = args
                .get(1)
                .or_else(|| kwargs.get("slot"))
                .and_then(|v| extract_opt_slot(vm, v));
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Messages)?;
            h.ops.push(EngineOp::CenterPrint { slot, text });
            Ok(PyVal::None)
        }
        "hud" => {
            let slot = kwargs.get("slot").and_then(|v| extract_opt_slot(vm, v));
            emit_hud_message(vm, slot, &args, &kwargs, host)
        }
        "dhud" => {
            let slot = kwargs.get("slot").and_then(|v| extract_opt_slot(vm, v));
            emit_dhud_message(vm, slot, &args, &kwargs, host)
        }
        "screen_fade" => {
            let slot = kwargs.get("slot").and_then(|v| extract_opt_slot(vm, v));
            emit_screen_fade(vm, slot, &args, &kwargs, host)
        }
        "screen_shake" => {
            let slot = kwargs.get("slot").and_then(|v| extract_opt_slot(vm, v));
            emit_screen_shake(vm, slot, &args, &kwargs, host)
        }
        "tr" => {
            let key = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "mod.tr(key, ...) requires key"))?;
            let slot = kwargs.get("slot").and_then(|v| extract_opt_slot(vm, v));
            let pos_args = if args.len() > 1 { &args[1..] } else { &[] };
            let mut kw_clean = kwargs;
            kw_clean.remove("slot");
            translate_for_slot(vm, slot, &key, pos_args, &kw_clean, host.as_deref())
        }
        "load_lang_toml" => {
            let src = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "load_lang_toml(src) requires str"))?;
            let Some(h) = host else {
                return Ok(PyVal::Int(0));
            };
            let n = h.services.lock().lang.load_toml_str(&src);
            Ok(PyVal::Int(n as i64))
        }
        "load_lang_amxx" => {
            let src = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "load_lang_amxx(src) requires str"))?;
            let Some(h) = host else {
                return Ok(PyVal::Int(0));
            };
            let n = h.services.lock().lang.load_amxx_txt_str(&src);
            Ok(PyVal::Int(n as i64))
        }
        "cvar" => {
            let name = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "mod.cvar(name) requires name"))?;
            let default = args
                .get(1)
                .or_else(|| kwargs.get("default"))
                .map(|v| vm.val_to_str(v))
                .unwrap_or_default();
            let val = host
                .as_deref()
                .and_then(|h| h.snapshot.cvars.get(&name).cloned())
                .unwrap_or(default);
            Ok(PyVal::Str(val))
        }
        "cvar_float" => {
            let name = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "mod.cvar_float(name) requires name"))?;
            let default = args
                .get(1)
                .or_else(|| kwargs.get("default"))
                .and_then(PyVal::as_float)
                .unwrap_or(0.0);
            let val = host
                .as_deref()
                .and_then(|h| h.snapshot.cvars.get(&name))
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(default);
            Ok(PyVal::Float(val))
        }
        "cvar_int" => {
            let name = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "mod.cvar_int(name) requires name"))?;
            let default = args
                .get(1)
                .or_else(|| kwargs.get("default"))
                .and_then(PyVal::as_int)
                .unwrap_or(0);
            let val = host
                .as_deref()
                .and_then(|h| h.snapshot.cvars.get(&name))
                .and_then(|s| s.trim().parse::<i64>().ok())
                .unwrap_or(default);
            Ok(PyVal::Int(val))
        }
        "set_cvar" => {
            let name = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "set_cvar(name, value) requires 2 args"))?;
            let value = args
                .get(1)
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "set_cvar(name, value) requires 2 args"))?;
            let Some(h) = host else {
                vm.init_ops.push(EngineOp::SetCvar { name, value });
                return Ok(PyVal::None);
            };
            require_perm(vm, h, Permission::Cvars)?;
            h.ops.push(EngineOp::SetCvar { name, value });
            Ok(PyVal::None)
        }
        "set_timer" => {
            let delay = args
                .first()
                .and_then(PyVal::as_float)
                .ok_or_else(|| vm.runtime_err("TypeError", "set_timer(delay, callback) expects float"))?;
            let cb = args
                .get(1)
                .or_else(|| kwargs.get("callback"))
                .cloned()
                .ok_or_else(|| vm.runtime_err("TypeError", "set_timer() requires callback"))?;
            let repeat = args
                .get(2)
                .or_else(|| kwargs.get("repeat"))
                .is_some_and(|v| vm.is_truthy(v));
            let arg = args
                .get(3)
                .or_else(|| kwargs.get("arg"))
                .and_then(PyVal::as_int)
                .unwrap_or(0) as i32;
            let cb_id = vm.next_callback_id();
            let delay_ms = (delay.max(0.001) * 1000.0).round() as u32;
            vm.timer_callbacks.insert(cb_id, cb);
            if let Some(h) = host {
                require_perm(vm, h, Permission::Timers)?;
                let local_id = h.next_local_timer_id;
                h.next_local_timer_id = h.next_local_timer_id.wrapping_add(1).max(1);
                h.timers.push(TimerRequest {
                    delay_ms,
                    repeat,
                    cb_id,
                    arg,
                });
                Ok(PyVal::Int(i64::from(local_id)))
            } else {
                vm.startup_timers.push(RegisteredStartupTimer {
                    delay_ms,
                    repeat,
                    cb_id,
                    arg,
                });
                Ok(PyVal::Int(i64::from(cb_id)))
            }
        }
        "cancel_timer" => {
            let tid = args
                .first()
                .and_then(PyVal::as_int)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| vm.runtime_err("TypeError", "cancel_timer(id) expects positive int"))?;
            if let Some(h) = host {
                require_perm(vm, h, Permission::Timers)?;
                h.cancel_timers.push(tid);
            }
            Ok(PyVal::None)
        }
        "register_custom_weapon" => {
            let spec = if let Some(PyVal::Handle(id)) = args.first()
                && let Some(HeapObj::Native(NativeObjKind::CustomWeapon(s))) = vm.heap.get(*id)
            {
                s.clone()
            } else {
                let obj = make_custom_weapon_obj(vm, args, kwargs)?;
                if let PyVal::Handle(id) = obj
                    && let Some(HeapObj::Native(NativeObjKind::CustomWeapon(s))) = vm.heap.get(id)
                {
                    s.clone()
                } else {
                    return Err(vm.runtime_err("TypeError", "invalid CustomWeapon spec"));
                }
            };
            if let Some(h) = host {
                if !(h.has_perm(Permission::Weapons) || h.has_perm(Permission::Entities)) {
                    return Err(vm.runtime_err("PermissionError", "weapons permission required"));
                }
                h.ops.push(EngineOp::RegisterCustomWeapon(spec));
            } else {
                vm.init_custom_weapons.push(spec);
            }
            Ok(PyVal::None)
        }
        "call_fn" => {
            let fn_name = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "call_fn(name, *args) requires name"))?;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "call_fn unavailable outside active call"));
            };
            require_perm(vm, h, Permission::Rpc)?;
            let rpc_args: Vec<RpcValue> = args[1..]
                .iter()
                .map(|v| pyval_to_rpc(vm, v))
                .collect();
            let host_direct = {
                let svc = h.services.lock();
                svc.rpc.call_host_fn(&fn_name, &rpc_args)
            };
            let res = host_direct
                .or_else(|| h.rpc_dispatcher.and_then(|d| d.call_rpc(&fn_name, &rpc_args)));
            Ok(match res {
                Some(RpcValue::Int(n)) => PyVal::Int(i64::from(n)),
                Some(RpcValue::Float(f)) => PyVal::Float(f64::from(f)),
                Some(RpcValue::Str(s)) => PyVal::Str(s),
                Some(RpcValue::Bytes(b)) => PyVal::Str(String::from_utf8_lossy(&b).into_owned()),
                None => PyVal::None,
            })
        }
        "emit_event" => {
            let ev_name = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "emit_event(name, payload) requires name"))?;
            let payload_val = args
                .get(1)
                .or_else(|| kwargs.get("payload"))
                .cloned()
                .unwrap_or(PyVal::Str(String::new()));
            let payload_json = match &payload_val {
                PyVal::Str(s) => s.clone(),
                other => json_dumps(&vm.heap, other, None, false)
                    .unwrap_or_else(|_| vm.val_to_str(other)),
            };
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "emit_event unavailable outside active call"));
            };
            require_perm(vm, h, Permission::Rpc)?;
            h.ops.push(EngineOp::RpcEmitEvent(RpcEmitEventSpec {
                event_name: ev_name,
                payload_json: payload_json.clone(),
                args: vec![RpcValue::Str(payload_json)],
            }));
            Ok(PyVal::None)
        }
        "http_post_json" => {
            let url = args
                .first()
                .or_else(|| kwargs.get("url"))
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "http_post_json(url, body) requires url"))?;
            let body_val = args
                .get(1)
                .or_else(|| kwargs.get("body"))
                .cloned()
                .unwrap_or(PyVal::Str("{}".to_string()));
            let body = match &body_val {
                PyVal::Str(s) => s.clone(),
                other => json_dumps(&vm.heap, other, None, false)
                    .map_err(|e| vm.runtime_err("TypeError", e))?,
            };
            if body.len() > WEBHOOK_MAX_REQUEST_BYTES {
                return Err(vm.runtime_err("ValueError", "webhook JSON body exceeds 64 KiB cap"));
            }
            let callback = args
                .get(2)
                .or_else(|| kwargs.get("callback"))
                .cloned()
                .filter(|v| !matches!(v, PyVal::None));
            let Some(h) = host.as_deref_mut() else {
                return Err(vm.runtime_err("RuntimeError", "http_post_json unavailable outside active call"));
            };
            require_perm(vm, h, Permission::Http)?;
            let parsed = parse_webhook_url(&url).map_err(|e| vm.runtime_err("ValueError", e))?;
            if !is_host_allowed(h.http_hosts, &parsed.host, parsed.port) {
                return Err(vm.runtime_err(
                    "PermissionError",
                    format!("host '{}' is not in http:<host> allowlist", parsed.host),
                ));
            }
            let cb_id = if let Some(cb) = callback {
                let id = vm.next_callback_id();
                vm.webhook_callbacks.insert(id, cb);
                id
            } else {
                0
            };
            h.ops.push(EngineOp::HttpPostJson(HttpPostJsonSpec {
                url,
                body,
                callback_id: cb_id,
            }));
            Ok(PyVal::Bool(true))
        }
        "precache_sound" | "register_sound" => {
            let sample = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "precache_sound(sample) requires str"))?;
            if let Some(h) = host {
                require_perm(vm, h, Permission::Sound)?;
                h.ops.push(EngineOp::PrecacheSound { sample });
            } else {
                vm.init_ops.push(EngineOp::PrecacheSound { sample });
            }
            Ok(PyVal::None)
        }
        "emit_sound" => {
            let ent_index = args
                .first()
                .and_then(|v| match v {
                    PyVal::Handle(id) => match vm.heap.get(*id) {
                        Some(HeapObj::Native(NativeObjKind::Player(s))) => Some(u16::from(*s) + 1),
                        Some(HeapObj::Native(NativeObjKind::Entity(e))) => Some(*e),
                        _ => None,
                    },
                    _ => v.as_int().and_then(|n| u16::try_from(n).ok()),
                })
                .ok_or_else(|| vm.runtime_err("TypeError", "emit_sound() expects entity/player"))?;
            let sample = args
                .get(1)
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "emit_sound() expects sample path"))?;
            let channel = args
                .get(2)
                .or_else(|| kwargs.get("channel"))
                .and_then(PyVal::as_int)
                .unwrap_or(0)
                .clamp(0, 255) as u8;
            let volume = args
                .get(3)
                .or_else(|| kwargs.get("volume"))
                .and_then(PyVal::as_float)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0) as f32;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Sound)?;
            h.ops.push(EngineOp::EmitSound {
                ent_index,
                channel,
                sample,
                volume,
            });
            Ok(PyVal::None)
        }
        "change_level" => {
            let map = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "change_level(map) requires map name"))?;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Commands)?;
            h.ops.push(EngineOp::ChangeLevel { map });
            Ok(PyVal::None)
        }
        "server_exec" => {
            let line = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "server_exec(line) requires str"))?;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Commands)?;
            h.ops.push(EngineOp::ServerExec { line });
            Ok(PyVal::None)
        }
        "trace_fraction" => {
            let start = args
                .first()
                .ok_or_else(|| vm.runtime_err("TypeError", "trace_fraction(start, end) requires 2 points"))?;
            let end = args
                .get(1)
                .ok_or_else(|| vm.runtime_err("TypeError", "trace_fraction(start, end) requires 2 points"))?;
            let p0 = extract_vec3(vm, start)?;
            let p1 = extract_vec3(vm, end)?;
            let Some(h) = host else {
                return Ok(PyVal::Float(1.0));
            };
            require_perm(vm, h, Permission::Traces)?;
            let frac = if let Some(ref coll) = h.snapshot.collision {
                coll.trace_world(
                    glam::Vec3::from_array(p0),
                    glam::Vec3::from_array(p1),
                    cs_pm::collision::Hull::Point,
                )
                .fraction
            } else {
                1.0
            };
            Ok(PyVal::Float(f64::from(frac)))
        }
        "log" => {
            let msg = args
                .iter()
                .map(|v| vm.val_to_str(v))
                .collect::<Vec<_>>()
                .join(" ");
            if let Some(h) = host {
                if h.logs.len() < 128 {
                    h.logs.push(msg);
                }
            } else if vm.pending_logs.len() < 128 {
                vm.pending_logs.push(msg);
            }
            Ok(PyVal::None)
        }
        _ => Err(vm.runtime_err(
            "AttributeError",
            format!("'Mod' object has no attribute '{method}'"),
        )),
    }
}

fn call_vfx_method(
    vm: &mut PyVm,
    method: &str,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
    host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    let Some(h) = host else {
        return Err(vm.runtime_err("RuntimeError", "vfx cannot be emitted outside active hook"));
    };
    require_perm(vm, h, Permission::Vfx)?;
    let slot = kwargs.get("slot").and_then(|v| extract_opt_slot(vm, v));

    let op = match method {
        "shockwave_ring" => {
            let origin = extract_vec3(
                vm,
                args.first()
                    .or_else(|| kwargs.get("origin"))
                    .ok_or_else(|| vm.runtime_err("TypeError", "shockwave_ring() requires origin"))?,
            )?;
            let radius = args
                .get(1)
                .or_else(|| kwargs.get("radius"))
                .and_then(PyVal::as_float)
                .unwrap_or(200.0) as f32;
            let rgba = args
                .get(2)
                .or_else(|| kwargs.get("rgba"))
                .or_else(|| kwargs.get("color"))
                .map(|v| extract_rgba(vm, v))
                .transpose()?
                .unwrap_or([0, 200, 255, 255]);
            let life = args
                .get(3)
                .or_else(|| kwargs.get("life"))
                .or_else(|| kwargs.get("life_decisec"))
                .and_then(PyVal::as_int)
                .unwrap_or(10)
                .clamp(1, 255) as u8;
            let width = args
                .get(4)
                .or_else(|| kwargs.get("width"))
                .and_then(PyVal::as_int)
                .unwrap_or(16)
                .clamp(1, 255) as u8;
            VfxOp::ShockwaveRing {
                origin,
                radius,
                rgba,
                life_decisec: life,
                width,
                slot,
            }
        }
        "laser_beam" => {
            let start = extract_vec3(
                vm,
                args.first()
                    .or_else(|| kwargs.get("start"))
                    .ok_or_else(|| vm.runtime_err("TypeError", "laser_beam() requires start"))?,
            )?;
            let end = extract_vec3(
                vm,
                args.get(1)
                    .or_else(|| kwargs.get("end"))
                    .ok_or_else(|| vm.runtime_err("TypeError", "laser_beam() requires end"))?,
            )?;
            let rgba = args
                .get(2)
                .or_else(|| kwargs.get("rgba"))
                .or_else(|| kwargs.get("color"))
                .map(|v| extract_rgba(vm, v))
                .transpose()?
                .unwrap_or([255, 0, 0, 255]);
            let life = args
                .get(3)
                .or_else(|| kwargs.get("life"))
                .and_then(PyVal::as_int)
                .unwrap_or(10)
                .clamp(1, 255) as u8;
            let width = args
                .get(4)
                .or_else(|| kwargs.get("width"))
                .and_then(PyVal::as_int)
                .unwrap_or(10)
                .clamp(1, 255) as u8;
            let noise = args
                .get(5)
                .or_else(|| kwargs.get("noise"))
                .and_then(PyVal::as_int)
                .unwrap_or(0)
                .clamp(0, 255) as u8;
            VfxOp::LaserBeam {
                start,
                end,
                rgba,
                life_decisec: life,
                width,
                noise,
                slot,
            }
        }
        "follow_trail" => {
            let ent_index = extract_ent_index(
                vm,
                args.first()
                    .or_else(|| kwargs.get("ent_index"))
                    .ok_or_else(|| vm.runtime_err("TypeError", "follow_trail() requires ent_index"))?,
            )?;
            let rgba = args
                .get(1)
                .or_else(|| kwargs.get("rgba"))
                .or_else(|| kwargs.get("color"))
                .map(|v| extract_rgba(vm, v))
                .transpose()?
                .unwrap_or([0, 255, 0, 255]);
            let life = args
                .get(2)
                .or_else(|| kwargs.get("life"))
                .and_then(PyVal::as_int)
                .unwrap_or(15)
                .clamp(1, 255) as u8;
            let width = args
                .get(3)
                .or_else(|| kwargs.get("width"))
                .and_then(PyVal::as_int)
                .unwrap_or(6)
                .clamp(1, 255) as u8;
            VfxOp::FollowTrail {
                ent_index,
                rgba,
                life_decisec: life,
                width,
                slot,
            }
        }
        "kill_beam" => {
            let ent_index = extract_ent_index(
                vm,
                args.first()
                    .or_else(|| kwargs.get("ent_index"))
                    .ok_or_else(|| vm.runtime_err("TypeError", "kill_beam() requires ent_index"))?,
            )?;
            VfxOp::KillBeam { ent_index, slot }
        }
        "aura_light" => {
            let origin = extract_vec3(
                vm,
                args.first()
                    .or_else(|| kwargs.get("origin"))
                    .ok_or_else(|| vm.runtime_err("TypeError", "aura_light() requires origin"))?,
            )?;
            let radius = args
                .get(1)
                .or_else(|| kwargs.get("radius"))
                .and_then(PyVal::as_int)
                .unwrap_or(20)
                .clamp(1, 255) as u8;
            let rgb = args
                .get(2)
                .or_else(|| kwargs.get("rgb"))
                .or_else(|| kwargs.get("color"))
                .map(|v| extract_rgb(vm, v))
                .transpose()?
                .unwrap_or([255, 215, 0]);
            let life = args
                .get(3)
                .or_else(|| kwargs.get("life"))
                .and_then(PyVal::as_int)
                .unwrap_or(10)
                .clamp(1, 255) as u8;
            let decay = args
                .get(4)
                .or_else(|| kwargs.get("decay"))
                .and_then(PyVal::as_int)
                .unwrap_or(15)
                .clamp(0, 255) as u8;
            VfxOp::AuraLight {
                origin,
                radius,
                rgb,
                life_decisec: life,
                decay,
                slot,
            }
        }
        "sparks" => {
            let origin = extract_vec3(
                vm,
                args.first()
                    .or_else(|| kwargs.get("origin"))
                    .ok_or_else(|| vm.runtime_err("TypeError", "sparks() requires origin"))?,
            )?;
            VfxOp::Sparks { origin, slot }
        }
        "implosion" => {
            let origin = extract_vec3(
                vm,
                args.first()
                    .or_else(|| kwargs.get("origin"))
                    .ok_or_else(|| vm.runtime_err("TypeError", "implosion() requires origin"))?,
            )?;
            let radius = args
                .get(1)
                .or_else(|| kwargs.get("radius"))
                .and_then(PyVal::as_int)
                .unwrap_or(100)
                .clamp(1, 255) as u8;
            let count = args
                .get(2)
                .or_else(|| kwargs.get("count"))
                .and_then(PyVal::as_int)
                .unwrap_or(20)
                .clamp(1, 255) as u8;
            let life = args
                .get(3)
                .or_else(|| kwargs.get("life"))
                .and_then(PyVal::as_int)
                .unwrap_or(10)
                .clamp(1, 255) as u8;
            VfxOp::Implosion {
                origin,
                radius,
                count,
                life_decisec: life,
                slot,
            }
        }
        "teleport_splash" => {
            let origin = extract_vec3(
                vm,
                args.first()
                    .or_else(|| kwargs.get("origin"))
                    .ok_or_else(|| vm.runtime_err("TypeError", "teleport_splash() requires origin"))?,
            )?;
            VfxOp::TeleportSplash { origin, slot }
        }
        "lava_splash" => {
            let origin = extract_vec3(
                vm,
                args.first()
                    .or_else(|| kwargs.get("origin"))
                    .ok_or_else(|| vm.runtime_err("TypeError", "lava_splash() requires origin"))?,
            )?;
            VfxOp::LavaSplash { origin, slot }
        }
        "explosion" => {
            let origin = extract_vec3(
                vm,
                args.first()
                    .or_else(|| kwargs.get("origin"))
                    .ok_or_else(|| vm.runtime_err("TypeError", "explosion() requires origin"))?,
            )?;
            let scale = args
                .get(1)
                .or_else(|| kwargs.get("scale"))
                .and_then(PyVal::as_int)
                .unwrap_or(30)
                .clamp(1, 255) as u8;
            let flags = args
                .get(2)
                .or_else(|| kwargs.get("flags"))
                .and_then(PyVal::as_int)
                .unwrap_or(0)
                .clamp(0, 255) as u8;
            VfxOp::Explosion {
                origin,
                scale,
                flags,
                slot,
            }
        }
        "blood_stream" => {
            let origin = extract_vec3(
                vm,
                args.first()
                    .or_else(|| kwargs.get("origin"))
                    .ok_or_else(|| vm.runtime_err("TypeError", "blood_stream() requires origin"))?,
            )?;
            let dir = extract_vec3(
                vm,
                args.get(1)
                    .or_else(|| kwargs.get("direction"))
                    .or_else(|| kwargs.get("dir"))
                    .ok_or_else(|| vm.runtime_err("TypeError", "blood_stream() requires direction"))?,
            )?;
            let color = args
                .get(2)
                .or_else(|| kwargs.get("color"))
                .and_then(PyVal::as_int)
                .unwrap_or(70)
                .clamp(0, 255) as u8;
            let speed = args
                .get(3)
                .or_else(|| kwargs.get("speed"))
                .and_then(PyVal::as_int)
                .unwrap_or(120)
                .clamp(0, 255) as u8;
            VfxOp::BloodStream {
                origin,
                dir,
                color,
                speed,
                slot,
            }
        }
        "world_decal" => {
            let origin = extract_vec3(
                vm,
                args.first()
                    .or_else(|| kwargs.get("origin"))
                    .ok_or_else(|| vm.runtime_err("TypeError", "world_decal() requires origin"))?,
            )?;
            let decal_name = args
                .get(1)
                .or_else(|| kwargs.get("decal_name"))
                .or_else(|| kwargs.get("name"))
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "world_decal() requires decal_name"))?;
            VfxOp::WorldDecal {
                origin,
                decal_name,
                slot,
            }
        }
        _ => {
            return Err(vm.runtime_err(
                "AttributeError",
                format!("'Vfx' has no attribute '{method}'"),
            ));
        }
    };
    h.ops.push(EngineOp::EmitVfx(op));
    Ok(PyVal::None)
}

fn call_storage_method(
    vm: &mut PyVm,
    method: &str,
    args: Vec<PyVal>,
    host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    let Some(h) = host else {
        return Err(vm.runtime_err("RuntimeError", "mod.storage unavailable outside active call"));
    };
    require_perm(vm, h, Permission::Storage)?;
    match method {
        "get" => {
            let key = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "storage.get(key) requires key"))?;
            let default = args.get(1).cloned().unwrap_or(PyVal::None);
            Ok(h.storage_get(&key).map_or(default, PyVal::Str))
        }
        "set" => {
            let key = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "storage.set(key, val) requires 2 args"))?;
            let val = args
                .get(1)
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "storage.set(key, val) requires 2 args"))?;
            h.storage_set(key, val);
            Ok(PyVal::None)
        }
        "delete" | "remove" | "pop" => {
            let key = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "storage.delete(key) requires key"))?;
            let existed = h.storage_delete(key);
            Ok(PyVal::Bool(existed))
        }
        "keys" => {
            let keys: Vec<PyVal> = h
                .storage_snapshot_map()
                .into_keys()
                .map(PyVal::Str)
                .collect();
            let id = vm.alloc(HeapObj::List(keys))?;
            Ok(PyVal::Handle(id))
        }
        "items" => {
            let pairs: Vec<PyVal> = h
                .storage_snapshot_map()
                .into_iter()
                .map(|(k, v)| PyVal::Tuple(vec![PyVal::Str(k), PyVal::Str(v)]))
                .collect();
            let id = vm.alloc(HeapObj::List(pairs))?;
            Ok(PyVal::Handle(id))
        }
        _ => Err(vm.runtime_err(
            "AttributeError",
            format!("'Storage' has no attribute '{method}'"),
        )),
    }
}

fn call_leaderboard_method(
    vm: &mut PyVm,
    method: &str,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
    host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    let Some(h) = host else {
        return Err(vm.runtime_err("RuntimeError", "mod.leaderboard unavailable outside active call"));
    };
    require_perm(vm, h, Permission::Storage)?;
    match method {
        "upsert" => {
            let table = args
                .first()
                .or_else(|| kwargs.get("table"))
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "leaderboard.upsert() requires table"))?;
            let key = args
                .get(1)
                .or_else(|| kwargs.get("key"))
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "leaderboard.upsert() requires key"))?;
            let score = args
                .get(2)
                .or_else(|| kwargs.get("score"))
                .and_then(PyVal::as_float)
                .ok_or_else(|| vm.runtime_err("TypeError", "leaderboard.upsert() requires score"))?;
            let payload_val = args
                .get(3)
                .or_else(|| kwargs.get("payload"))
                .cloned()
                .unwrap_or(PyVal::Str(String::new()));
            let payload_json = match &payload_val {
                PyVal::Str(s) => s.clone(),
                other => json_dumps(&vm.heap, other, None, false)
                    .unwrap_or_else(|_| vm.val_to_str(other)),
            };
            h.services
                .lock()
                .leaderboards
                .upsert(&table, key, score, payload_json);
            Ok(PyVal::None)
        }
        "rank" => {
            let table = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "leaderboard.rank(table, key) requires 2 args"))?;
            let key = args
                .get(1)
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "leaderboard.rank(table, key) requires 2 args"))?;
            let found = h.services.lock().leaderboards.rank(&table, &key);
            let Some((entry, total)) = found else {
                return Ok(PyVal::None);
            };
            let dict = vm.alloc(HeapObj::Dict(vec![
                (PyKey::Str("rank".to_string()), PyVal::Int(entry.rank as i64)),
                (PyKey::Str("total".to_string()), PyVal::Int(total as i64)),
                (PyKey::Str("key".to_string()), PyVal::Str(entry.key)),
                (PyKey::Str("score".to_string()), PyVal::Float(entry.score)),
                (
                    PyKey::Str("payload".to_string()),
                    PyVal::Str(entry.payload_json),
                ),
            ]))?;
            Ok(PyVal::Handle(dict))
        }
        "top" => {
            let table = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "leaderboard.top(table, ...) requires table"))?;
            let offset = args
                .get(1)
                .or_else(|| kwargs.get("offset"))
                .and_then(PyVal::as_int)
                .unwrap_or(0)
                .max(0) as usize;
            let limit = args
                .get(2)
                .or_else(|| kwargs.get("limit"))
                .and_then(PyVal::as_int)
                .unwrap_or(15)
                .clamp(1, 200) as usize;
            let rows = h.services.lock().leaderboards.top(&table, offset, limit);
            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let d = vm.alloc(HeapObj::Dict(vec![
                    (PyKey::Str("rank".to_string()), PyVal::Int(row.rank as i64)),
                    (PyKey::Str("key".to_string()), PyVal::Str(row.key)),
                    (PyKey::Str("score".to_string()), PyVal::Float(row.score)),
                    (
                        PyKey::Str("payload".to_string()),
                        PyVal::Str(row.payload_json),
                    ),
                ]))?;
                out.push(PyVal::Handle(d));
            }
            let lid = vm.alloc(HeapObj::List(out))?;
            Ok(PyVal::Handle(lid))
        }
        "count" => {
            let table = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "leaderboard.count(table) requires table"))?;
            let c = h.services.lock().leaderboards.count(&table);
            Ok(PyVal::Int(c as i64))
        }
        "remove" => {
            let table = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "leaderboard.remove(table, key) requires 2 args"))?;
            let key = args
                .get(1)
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "leaderboard.remove(table, key) requires 2 args"))?;
            let ok = h.services.lock().leaderboards.remove(&table, &key);
            Ok(PyVal::Bool(ok))
        }
        _ => Err(vm.runtime_err(
            "AttributeError",
            format!("'Leaderboard' has no attribute '{method}'"),
        )),
    }
}

fn call_menu_method(
    vm: &mut PyVm,
    menu_obj_id: ObjId,
    method: &str,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
    host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    match method {
        "add_item" => {
            let label = args
                .first()
                .or_else(|| kwargs.get("label"))
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "menu.add_item() requires label"))?;
            let info = args
                .get(1)
                .or_else(|| kwargs.get("info"))
                .map(|v| vm.val_to_str(v))
                .unwrap_or_else(|| label.clone());
            let enabled = args
                .get(2)
                .or_else(|| kwargs.get("enabled"))
                .map(|v| vm.is_truthy(v))
                .unwrap_or(true);
            if let Some(HeapObj::Native(NativeObjKind::Menu(m))) = vm.heap.get_mut(menu_obj_id) {
                m.items.push(MenuItemSpec::selectable(label, info, enabled));
            }
            Ok(PyVal::Handle(menu_obj_id))
        }
        "add_blank" => {
            if let Some(HeapObj::Native(NativeObjKind::Menu(m))) = vm.heap.get_mut(menu_obj_id) {
                m.items.push(MenuItemSpec::blank());
            }
            Ok(PyVal::Handle(menu_obj_id))
        }
        "add_text" => {
            let label = args
                .first()
                .map(|v| vm.val_to_str(v))
                .unwrap_or_default();
            if let Some(HeapObj::Native(NativeObjKind::Menu(m))) = vm.heap.get_mut(menu_obj_id) {
                m.items.push(MenuItemSpec::text(label));
            }
            Ok(PyVal::Handle(menu_obj_id))
        }
        "set_labels" => {
            let back = kwargs.get("back").map(|v| vm.val_to_str(v));
            let next = kwargs.get("next").map(|v| vm.val_to_str(v));
            let exit = kwargs.get("exit").map(|v| vm.val_to_str(v));
            if let Some(HeapObj::Native(NativeObjKind::Menu(m))) = vm.heap.get_mut(menu_obj_id) {
                if back.is_some() {
                    m.back_label = back;
                }
                if next.is_some() {
                    m.next_label = next;
                }
                if exit.is_some() {
                    m.exit_label = exit;
                }
            }
            Ok(PyVal::Handle(menu_obj_id))
        }
        "show" | "display" => {
            let target_val = args
                .first()
                .or_else(|| kwargs.get("player"))
                .or_else(|| kwargs.get("slot"))
                .ok_or_else(|| vm.runtime_err("TypeError", "menu.show(player) requires player or slot"))?;
            let slot = extract_slot(vm, target_val)?;
            let page = args
                .get(1)
                .or_else(|| kwargs.get("page"))
                .and_then(PyVal::as_int)
                .unwrap_or(0)
                .max(0) as u16;
            let time_sec = args
                .get(2)
                .or_else(|| kwargs.get("time"))
                .and_then(PyVal::as_int)
                .unwrap_or(-1)
                .clamp(-1, 127) as i8;
            show_menu_to_slot(vm, menu_obj_id, slot, page, time_sec, host)
        }
        "destroy" => {
            let existing = match vm.heap.get(menu_obj_id) {
                Some(HeapObj::Native(NativeObjKind::Menu(m))) => m.created_id,
                _ => None,
            };
            if let (Some(mid), Some(h)) = (existing, host) {
                h.services.lock().menus.destroy_menu(mid);
            }
            Ok(PyVal::None)
        }
        _ => Err(vm.runtime_err("AttributeError", format!("'Menu' has no attribute '{method}'"))),
    }
}

fn show_menu_to_slot(
    vm: &mut PyVm,
    menu_obj_id: ObjId,
    slot: u8,
    page: u16,
    time_sec: i8,
    host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    let Some(h) = host else {
        return Err(vm.runtime_err("RuntimeError", "cannot display menu outside active hook"));
    };
    require_perm(vm, h, Permission::Menus)?;
    let m = match vm.heap.get(menu_obj_id) {
        Some(HeapObj::Native(NativeObjKind::Menu(m))) => m.clone(),
        _ => return Err(vm.runtime_err("TypeError", "expected Menu object")),
    };
    let cb_id = if let Some(cb) = m.callback {
        let id = vm.next_callback_id();
        vm.menu_callbacks.insert(id, cb);
        id
    } else {
        0
    };
    let spec = MenuCreateSpec {
        title: m.title,
        items: m.items,
        callback_id: cb_id,
        callback_name: String::new(),
        back_label: m.back_label,
        next_label: m.next_label,
        exit_label: m.exit_label,
        per_page: m.per_page,
        allow_exit: m.allow_exit,
        number_color: m.number_color,
    };
    let menu_id = {
        let mut svc = h.services.lock();
        svc.menus.create_menu(h.mod_id.clone(), h.runtime_kind, 0, spec)
    };
    if let Some(HeapObj::Native(NativeObjKind::Menu(mm))) = vm.heap.get_mut(menu_obj_id) {
        mm.created_id = Some(menu_id);
    }
    h.ops.push(EngineOp::MenuDisplay(MenuDisplaySpec {
        slot,
        menu_id,
        page,
        time_sec,
    }));
    Ok(PyVal::Int(i64::from(menu_id)))
}

fn call_player_method(
    vm: &mut PyVm,
    slot: u8,
    method: &str,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
    host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    let ent_idx = u16::from(slot) + 1;
    match method {
        "tr" => {
            let key = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "player.tr(key, ...) requires key"))?;
            let pos = if args.len() > 1 { &args[1..] } else { &[] };
            translate_for_slot(vm, Some(slot), &key, pos, &kwargs, host.as_deref())
        }
        "has_flag" => {
            let flag_str = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "player.has_flag(flag) requires str"))?;
            let Some(h) = host else {
                return Ok(PyVal::Bool(false));
            };
            let snap_flags = AdminFlags::from_bits(
                h.snapshot.client(slot).map_or(0, |c| c.admin_flags),
            );
            let svc_flags = h.services.lock().admins.player_flags(slot);
            let combined = AdminFlags::from_bits(snap_flags.bits() | svc_flags.bits());
            let ok = !flag_str.is_empty() && flag_str.chars().all(|ch| combined.has_char(ch));
            Ok(PyVal::Bool(ok))
        }
        "has_weapon" => {
            let wid = args
                .first()
                .and_then(PyVal::as_int)
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| vm.runtime_err("TypeError", "has_weapon(weapon_id) expects int"))?;
            let has = host
                .as_deref()
                .and_then(|h| h.snapshot.client(slot))
                .is_some_and(|c| {
                    c.active_weapon == wid
                        || c.weapon_clips.contains_key(&wid)
                        || c.weapon_bpammo.get(&wid).copied().unwrap_or(0) > 0
                });
            Ok(PyVal::Bool(has))
        }
        "weapon_clip" => {
            let wid = args.first().and_then(PyVal::as_int).unwrap_or(0);
            let clip = host
                .as_deref()
                .and_then(|h| h.snapshot.client(slot))
                .map_or(-1, |c| {
                    if wid <= 0 {
                        c.clip
                    } else {
                        u8::try_from(wid)
                            .ok()
                            .and_then(|w| c.weapon_clips.get(&w).copied())
                            .unwrap_or(-1)
                    }
                });
            Ok(PyVal::Int(i64::from(clip)))
        }
        "bpammo_of" | "get_bpammo" => {
            let wid = args.first().and_then(PyVal::as_int).unwrap_or(0);
            let ammo = host
                .as_deref()
                .and_then(|h| h.snapshot.client(slot))
                .map_or(0, |c| {
                    if wid <= 0 {
                        c.bpammo
                    } else {
                        u8::try_from(wid)
                            .ok()
                            .and_then(|w| c.weapon_bpammo.get(&w).copied())
                            .unwrap_or(0)
                    }
                });
            Ok(PyVal::Int(i64::from(ammo)))
        }
        "set_health" => {
            let hp = args
                .first()
                .and_then(PyVal::as_float)
                .ok_or_else(|| vm.runtime_err("TypeError", "set_health(hp) expects float"))?;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerHealth {
                slot,
                health: hp as f32,
            });
            Ok(PyVal::None)
        }
        "set_max_health" => {
            let hp = args
                .first()
                .and_then(PyVal::as_float)
                .ok_or_else(|| vm.runtime_err("TypeError", "set_max_health(hp) expects float"))?;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerMaxHealth {
                slot,
                max_health: hp as f32,
            });
            Ok(PyVal::None)
        }
        "set_armor" => {
            let armor = args
                .first()
                .or_else(|| kwargs.get("armor"))
                .and_then(PyVal::as_int)
                .ok_or_else(|| vm.runtime_err("TypeError", "set_armor(amount, kind) expects int"))?;
            let kind_n = args
                .get(1)
                .or_else(|| kwargs.get("kind"))
                .and_then(PyVal::as_int)
                .unwrap_or(if armor > 0 { 2 } else { 0 });
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerArmor {
                slot,
                armor: armor.max(0) as i32,
                kind: ArmorKind::from_i32(kind_n as i32),
            });
            Ok(PyVal::None)
        }
        "set_money" => {
            let amount = args
                .first()
                .and_then(PyVal::as_int)
                .ok_or_else(|| vm.runtime_err("TypeError", "set_money(amount) expects int"))?;
            let flash = args
                .get(1)
                .or_else(|| kwargs.get("flash"))
                .map(|v| vm.is_truthy(v))
                .unwrap_or(true);
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerMoney {
                slot,
                amount: amount.max(0) as i32,
                flash,
            });
            Ok(PyVal::None)
        }
        "add_money" => {
            let delta = args
                .first()
                .and_then(PyVal::as_int)
                .ok_or_else(|| vm.runtime_err("TypeError", "add_money(delta) expects int"))?;
            let flash = args
                .get(1)
                .or_else(|| kwargs.get("flash"))
                .map(|v| vm.is_truthy(v))
                .unwrap_or(true);
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::AddPlayerMoney {
                slot,
                delta: delta as i32,
                flash,
            });
            Ok(PyVal::None)
        }
        "set_score" => {
            let frags = args
                .first()
                .or_else(|| kwargs.get("frags"))
                .and_then(PyVal::as_int)
                .ok_or_else(|| vm.runtime_err("TypeError", "set_score(frags, deaths) expects ints"))?;
            let deaths = args
                .get(1)
                .or_else(|| kwargs.get("deaths"))
                .and_then(PyVal::as_int)
                .ok_or_else(|| vm.runtime_err("TypeError", "set_score(frags, deaths) expects ints"))?;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerScore {
                slot,
                frags: frags as i32,
                deaths: deaths as i32,
            });
            Ok(PyVal::None)
        }
        "set_team" => {
            let team = args
                .first()
                .and_then(PyVal::as_int)
                .ok_or_else(|| vm.runtime_err("TypeError", "set_team(team) expects Team int"))?;
            let model_class = args
                .get(1)
                .or_else(|| kwargs.get("model_class"))
                .and_then(PyVal::as_int)
                .map(|n| n.clamp(0, 255) as u8);
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerTeam {
                slot,
                team: TeamKind::from_i32(team as i32),
                model_class,
            });
            Ok(PyVal::None)
        }
        "give_item" => {
            let item = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "give_item(name) requires str"))?;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::GiveItem { slot, item });
            Ok(PyVal::None)
        }
        "give_custom_weapon" => {
            let id_name = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "give_custom_weapon(id_name) requires str"))?;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            if !(h.has_perm(Permission::Weapons) || h.has_perm(Permission::Entities)) {
                return Err(vm.runtime_err("PermissionError", "weapons permission required"));
            }
            h.ops.push(EngineOp::GiveCustomWeapon { slot, id_name });
            Ok(PyVal::None)
        }
        "strip_weapons" => {
            let keep_knife = args
                .first()
                .or_else(|| kwargs.get("keep_knife"))
                .is_some_and(|v| vm.is_truthy(v));
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::StripUserWeapons { slot });
            if keep_knife {
                h.ops.push(EngineOp::GiveItem {
                    slot,
                    item: "weapon_knife".to_string(),
                });
            }
            Ok(PyVal::None)
        }
        "set_weapon_clip" => {
            let wid = args
                .first()
                .and_then(PyVal::as_int)
                .unwrap_or(0)
                .clamp(0, 255) as u8;
            let clip = args.get(1).and_then(PyVal::as_int).unwrap_or(0).max(0) as i32;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetWeaponClip {
                slot,
                weapon_id: wid,
                clip,
            });
            Ok(PyVal::None)
        }
        "set_bpammo" => {
            let wid = args
                .first()
                .and_then(PyVal::as_int)
                .unwrap_or(0)
                .clamp(0, 255) as u8;
            let amount = args.get(1).and_then(PyVal::as_int).unwrap_or(0).max(0) as i32;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetUserBpAmmo {
                slot,
                weapon_id: wid,
                amount,
            });
            Ok(PyVal::None)
        }
        "set_defuser" => {
            let has = args.first().map(|v| vm.is_truthy(v)).unwrap_or(true);
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetUserDefuser { slot, has });
            Ok(PyVal::None)
        }
        "set_nvg" => {
            let has = args.first().map(|v| vm.is_truthy(v)).unwrap_or(true);
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetUserNvg { slot, has });
            Ok(PyVal::None)
        }
        "drop_slot" => {
            let inv_slot = args
                .first()
                .and_then(PyVal::as_int)
                .ok_or_else(|| vm.runtime_err("TypeError", "drop_slot(slot_num) expects int"))?
                .clamp(0, 255) as u8;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::DropSlot { slot, inv_slot });
            Ok(PyVal::None)
        }
        "slap" => {
            let power = args
                .first()
                .or_else(|| kwargs.get("damage"))
                .or_else(|| kwargs.get("power"))
                .and_then(PyVal::as_int)
                .unwrap_or(0)
                .max(0) as i32;
            let random_dir = args
                .get(1)
                .or_else(|| kwargs.get("random_dir"))
                .or_else(|| kwargs.get("random_vel"))
                .map(|v| vm.is_truthy(v))
                .unwrap_or(true);
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SlapPlayer {
                slot,
                power,
                random_dir,
            });
            Ok(PyVal::None)
        }
        "slay" => {
            let preserve_frags = args
                .first()
                .or_else(|| kwargs.get("preserve_frags"))
                .is_some_and(|v| vm.is_truthy(v));
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SlayPlayer {
                slot,
                preserve_frags,
            });
            Ok(PyVal::None)
        }
        "respawn" => {
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::RespawnPlayer { slot });
            Ok(PyVal::None)
        }
        "damage" => {
            let amount = args
                .first()
                .or_else(|| kwargs.get("damage"))
                .and_then(PyVal::as_float)
                .ok_or_else(|| vm.runtime_err("TypeError", "player.damage(amount) expects float"))?;
            let attacker = args
                .get(1)
                .or_else(|| kwargs.get("attacker"))
                .and_then(|v| extract_opt_slot(vm, v));
            let bits = args
                .get(2)
                .or_else(|| kwargs.get("bits"))
                .and_then(PyVal::as_int)
                .unwrap_or(0) as u32;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::DamagePlayer {
                victim: slot,
                attacker,
                damage: amount.max(0.0) as f32,
                bits,
            });
            Ok(PyVal::None)
        }
        "set_godmode" => {
            let enabled = args.first().map(|v| vm.is_truthy(v)).unwrap_or(true);
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerGodmode { slot, enabled });
            Ok(PyVal::None)
        }
        "set_noclip" => {
            let enabled = args.first().map(|v| vm.is_truthy(v)).unwrap_or(true);
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerNoclip { slot, enabled });
            Ok(PyVal::None)
        }
        "set_maxspeed" => {
            let maxspeed = args
                .first()
                .and_then(PyVal::as_float)
                .ok_or_else(|| vm.runtime_err("TypeError", "set_maxspeed(speed) expects float"))?
                as f32;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerMaxspeed { slot, maxspeed });
            Ok(PyVal::None)
        }
        "set_gravity" => {
            let gravity = args
                .first()
                .and_then(PyVal::as_float)
                .ok_or_else(|| vm.runtime_err("TypeError", "set_gravity(g) expects float"))?
                as f32;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerGravity { slot, gravity });
            Ok(PyVal::None)
        }
        "set_footsteps" => {
            let silent = args
                .first()
                .or_else(|| kwargs.get("silent"))
                .map(|v| vm.is_truthy(v))
                .unwrap_or(true);
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerFootsteps { slot, silent });
            Ok(PyVal::None)
        }
        "set_rendering" => {
            // Supports both (fx, rgb=(r,g,b), mode=0, amount=255) AND (fx, r, g, b, mode, amount)
            let fx = args
                .first()
                .or_else(|| kwargs.get("fx"))
                .and_then(PyVal::as_int)
                .unwrap_or(0)
                .clamp(0, 255) as u8;
            let (rgb, mode, amount) = if args.len() >= 6 {
                let r = args[1].as_int().unwrap_or(255).clamp(0, 255) as u8;
                let g = args[2].as_int().unwrap_or(255).clamp(0, 255) as u8;
                let b = args[3].as_int().unwrap_or(255).clamp(0, 255) as u8;
                let mode = args[4].as_int().unwrap_or(0).clamp(0, 255) as u8;
                let amt = args[5].as_int().unwrap_or(255).clamp(0, 255) as u8;
                ([r, g, b], mode, amt)
            } else {
                let rgb = if let Some(c) = args.get(1).or_else(|| kwargs.get("rgb")).or_else(|| kwargs.get("color")) {
                    extract_rgb(vm, c)?
                } else {
                    let r = kwargs.get("r").and_then(PyVal::as_int).unwrap_or(255).clamp(0, 255) as u8;
                    let g = kwargs.get("g").and_then(PyVal::as_int).unwrap_or(255).clamp(0, 255) as u8;
                    let b = kwargs.get("b").and_then(PyVal::as_int).unwrap_or(255).clamp(0, 255) as u8;
                    [r, g, b]
                };
                let mode = args
                    .get(2)
                    .or_else(|| kwargs.get("mode"))
                    .and_then(PyVal::as_int)
                    .unwrap_or(0)
                    .clamp(0, 255) as u8;
                let amount = args
                    .get(3)
                    .or_else(|| kwargs.get("amount"))
                    .and_then(PyVal::as_int)
                    .unwrap_or(255)
                    .clamp(0, 255) as u8;
                (rgb, mode, amount)
            };
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerRendering {
                slot,
                fx,
                rgb,
                mode,
                amount,
            });
            Ok(PyVal::None)
        }
        "set_model" => {
            let model = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "set_model(name) requires str"))?;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetPlayerModel { slot, model });
            Ok(PyVal::None)
        }
        "reset_model" => {
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::ResetPlayerModel { slot });
            Ok(PyVal::None)
        }
        "set_origin" => {
            let origin = extract_vec3(
                vm,
                args.first()
                    .ok_or_else(|| vm.runtime_err("TypeError", "set_origin(origin) requires 3-tuple"))?,
            )?;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetOrigin {
                index: ent_idx,
                origin,
            });
            Ok(PyVal::None)
        }
        "set_velocity" => {
            let velocity = extract_vec3(
                vm,
                args.first()
                    .ok_or_else(|| vm.runtime_err("TypeError", "set_velocity(vel) requires 3-tuple"))?,
            )?;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Entities)?;
            h.ops.push(EngineOp::SetVelocity {
                index: ent_idx,
                velocity,
            });
            Ok(PyVal::None)
        }
        "chat" | "say" => {
            let text = args.first().map(|v| vm.val_to_str(v)).unwrap_or_default();
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Messages)?;
            h.ops.push(EngineOp::SayText {
                slot: Some(slot),
                text,
            });
            Ok(PyVal::None)
        }
        "print" | "console" => {
            let text = args.first().map(|v| vm.val_to_str(v)).unwrap_or_default();
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Messages)?;
            h.ops.push(EngineOp::PrintClient { slot, text });
            Ok(PyVal::None)
        }
        "center_print" => {
            let text = args.first().map(|v| vm.val_to_str(v)).unwrap_or_default();
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Messages)?;
            h.ops.push(EngineOp::CenterPrint {
                slot: Some(slot),
                text,
            });
            Ok(PyVal::None)
        }
        "hud" => emit_hud_message(vm, Some(slot), &args, &kwargs, host),
        "dhud" => emit_dhud_message(vm, Some(slot), &args, &kwargs, host),
        "show_motd" => {
            let title = args
                .first()
                .or_else(|| kwargs.get("title"))
                .map(|v| vm.val_to_str(v))
                .unwrap_or_default();
            let body = args
                .get(1)
                .or_else(|| kwargs.get("body"))
                .or_else(|| kwargs.get("html"))
                .map(|v| vm.val_to_str(v))
                .unwrap_or_default();
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Messages)?;
            h.ops.push(EngineOp::ShowMotd(ShowMotdSpec {
                slot,
                title,
                body,
            }));
            Ok(PyVal::None)
        }
        "screen_fade" => emit_screen_fade(vm, Some(slot), &args, &kwargs, host),
        "screen_shake" => emit_screen_shake(vm, Some(slot), &args, &kwargs, host),
        "bar_time" => {
            let seconds = args
                .first()
                .and_then(PyVal::as_int)
                .unwrap_or(0)
                .clamp(0, i64::from(u16::MAX)) as u16;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Messages)?;
            h.ops.push(EngineOp::BarTime(BarTimeSpec { slot, seconds }));
            Ok(PyVal::None)
        }
        "show_menu" => {
            let Some(PyVal::Handle(menu_id)) = args.first() else {
                return Err(vm.runtime_err("TypeError", "show_menu(menu) expects Menu"));
            };
            let page = args
                .get(1)
                .or_else(|| kwargs.get("page"))
                .and_then(PyVal::as_int)
                .unwrap_or(0)
                .max(0) as u16;
            let time_sec = args
                .get(2)
                .or_else(|| kwargs.get("time"))
                .and_then(PyVal::as_int)
                .unwrap_or(-1)
                .clamp(-1, 127) as i8;
            show_menu_to_slot(vm, *menu_id, slot, page, time_sec, host)
        }
        "emit_sound" => {
            let sample = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "emit_sound(sample) requires str"))?;
            let channel = args
                .get(1)
                .or_else(|| kwargs.get("channel"))
                .and_then(PyVal::as_int)
                .unwrap_or(0)
                .clamp(0, 255) as u8;
            let volume = args
                .get(2)
                .or_else(|| kwargs.get("volume"))
                .and_then(PyVal::as_float)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0) as f32;
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Sound)?;
            h.ops.push(EngineOp::EmitSound {
                ent_index: ent_idx,
                channel,
                sample,
                volume,
            });
            Ok(PyVal::None)
        }
        "kick" => {
            let reason = args
                .first()
                .or_else(|| kwargs.get("reason"))
                .map(|v| vm.val_to_str(v))
                .unwrap_or_default();
            let Some(h) = host else {
                return Err(vm.runtime_err("RuntimeError", "no active host context"));
            };
            require_perm(vm, h, Permission::Commands)?;
            h.ops.push(EngineOp::Kick { slot, reason });
            Ok(PyVal::None)
        }
        "set_lang" => {
            let code = args
                .first()
                .map(|v| vm.val_to_str(v))
                .ok_or_else(|| vm.runtime_err("TypeError", "set_lang(code) requires str"))?;
            let Some(h) = host else {
                return Ok(PyVal::Bool(false));
            };
            let ok = h.services.lock().lang.set_player_lang(slot, None, &code).is_some();
            Ok(PyVal::Bool(ok))
        }
        _ => Err(vm.runtime_err(
            "AttributeError",
            format!("'Player' object has no attribute '{method}'"),
        )),
    }
}

fn translate_for_slot(
    vm: &PyVm,
    slot: Option<u8>,
    key: &str,
    pos_args: &[PyVal],
    kwargs: &BTreeMap<String, PyVal>,
    host: Option<&HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    let Some(h) = host else {
        return Ok(PyVal::Str(key.to_string()));
    };
    let mut pairs: Vec<(String, String)> = Vec::with_capacity(pos_args.len() + kwargs.len());
    for (idx, v) in pos_args.iter().enumerate() {
        pairs.push((idx.to_string(), vm.val_to_str(v)));
    }
    for (k, v) in kwargs {
        pairs.push((k.clone(), vm.val_to_str(v)));
    }
    let borrowed: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let svc = h.services.lock();
    Ok(PyVal::Str(svc.lang.tr(slot, key, &borrowed)))
}

fn emit_hud_message(
    vm: &PyVm,
    slot: Option<u8>,
    args: &[PyVal],
    kwargs: &BTreeMap<String, PyVal>,
    host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    let text = args
        .first()
        .or_else(|| kwargs.get("text"))
        .map(|v| vm.val_to_str(v))
        .unwrap_or_default();
    let x = args
        .get(1)
        .or_else(|| kwargs.get("x"))
        .and_then(PyVal::as_float)
        .unwrap_or(-1.0) as f32;
    let y = args
        .get(2)
        .or_else(|| kwargs.get("y"))
        .and_then(PyVal::as_float)
        .unwrap_or(0.35) as f32;
    let channel = kwargs
        .get("channel")
        .and_then(PyVal::as_int)
        .unwrap_or(1)
        .clamp(0, 4) as u8;
    let rgba1 = kwargs
        .get("rgba")
        .or_else(|| kwargs.get("rgba1"))
        .or_else(|| kwargs.get("color"))
        .map(|v| extract_rgba(vm, v))
        .transpose()?
        .unwrap_or([255, 255, 255, 255]);
    let rgba2 = kwargs
        .get("rgba2")
        .map(|v| extract_rgba(vm, v))
        .transpose()?
        .unwrap_or(rgba1);
    let effect = kwargs
        .get("effect")
        .and_then(PyVal::as_int)
        .unwrap_or(0)
        .clamp(0, 255) as u8;
    let fadein = kwargs.get("fadein").and_then(PyVal::as_float).unwrap_or(0.1).max(0.0) as f32;
    let hold = kwargs.get("hold").and_then(PyVal::as_float).unwrap_or(2.0).max(0.0) as f32;
    let fadeout = kwargs.get("fadeout").and_then(PyVal::as_float).unwrap_or(0.2).max(0.0) as f32;
    let fxtime = kwargs.get("fxtime").and_then(PyVal::as_float).unwrap_or(0.0).max(0.0) as f32;
    let Some(h) = host else {
        return Err(vm.runtime_err("RuntimeError", "no active host context"));
    };
    require_perm(vm, h, Permission::Messages)?;
    h.ops.push(EngineOp::HudMessage(HudMessageSpec {
        slot,
        channel,
        x,
        y,
        rgba1,
        rgba2,
        effect,
        fadein,
        hold,
        fadeout,
        fxtime,
        text,
    }));
    Ok(PyVal::None)
}

fn emit_dhud_message(
    vm: &PyVm,
    slot: Option<u8>,
    args: &[PyVal],
    kwargs: &BTreeMap<String, PyVal>,
    host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    let text = args
        .first()
        .or_else(|| kwargs.get("text"))
        .map(|v| vm.val_to_str(v))
        .unwrap_or_default();
    let x = args
        .get(1)
        .or_else(|| kwargs.get("x"))
        .and_then(PyVal::as_float)
        .unwrap_or(-1.0) as f32;
    let y = args
        .get(2)
        .or_else(|| kwargs.get("y"))
        .and_then(PyVal::as_float)
        .unwrap_or(0.25) as f32;
    let rgb = kwargs
        .get("rgb")
        .or_else(|| kwargs.get("color"))
        .map(|v| extract_rgb(vm, v))
        .transpose()?
        .unwrap_or([255, 255, 255]);
    let effect = kwargs
        .get("effect")
        .and_then(PyVal::as_int)
        .unwrap_or(0)
        .clamp(0, 255) as u8;
    let fadein = kwargs.get("fadein").and_then(PyVal::as_float).unwrap_or(0.1).max(0.0) as f32;
    let hold = kwargs.get("hold").and_then(PyVal::as_float).unwrap_or(2.0).max(0.0) as f32;
    let fadeout = kwargs.get("fadeout").and_then(PyVal::as_float).unwrap_or(0.2).max(0.0) as f32;
    let fxtime = kwargs.get("fxtime").and_then(PyVal::as_float).unwrap_or(0.0).max(0.0) as f32;
    let Some(h) = host else {
        return Err(vm.runtime_err("RuntimeError", "no active host context"));
    };
    require_perm(vm, h, Permission::Messages)?;
    h.ops.push(EngineOp::DhudMessage(DhudMessageSpec {
        slot,
        x,
        y,
        rgb,
        effect,
        fadein,
        hold,
        fadeout,
        fxtime,
        text,
    }));
    Ok(PyVal::None)
}

fn emit_screen_fade(
    vm: &PyVm,
    slot: Option<u8>,
    args: &[PyVal],
    kwargs: &BTreeMap<String, PyVal>,
    host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    let duration_sec = args
        .first()
        .or_else(|| kwargs.get("duration"))
        .and_then(PyVal::as_float)
        .unwrap_or(0.5)
        .max(0.0) as f32;
    let hold_sec = args
        .get(1)
        .or_else(|| kwargs.get("hold"))
        .and_then(PyVal::as_float)
        .unwrap_or(0.1)
        .max(0.0) as f32;
    let flags = args
        .get(2)
        .or_else(|| kwargs.get("flags"))
        .and_then(PyVal::as_int)
        .unwrap_or(0)
        .clamp(0, i64::from(u16::MAX)) as u16;
    let rgba = args
        .get(3)
        .or_else(|| kwargs.get("rgba"))
        .or_else(|| kwargs.get("color"))
        .map(|v| extract_rgba(vm, v))
        .transpose()?
        .unwrap_or([0, 0, 0, 128]);
    let Some(h) = host else {
        return Err(vm.runtime_err("RuntimeError", "no active host context"));
    };
    require_perm(vm, h, Permission::Messages)?;
    h.ops.push(EngineOp::ScreenFade(ScreenFadeSpec {
        slot,
        duration_sec,
        hold_sec,
        flags,
        rgba,
    }));
    Ok(PyVal::None)
}

fn emit_screen_shake(
    vm: &PyVm,
    slot: Option<u8>,
    args: &[PyVal],
    kwargs: &BTreeMap<String, PyVal>,
    host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    let amplitude = args
        .first()
        .or_else(|| kwargs.get("amplitude"))
        .and_then(PyVal::as_float)
        .unwrap_or(4.0)
        .max(0.0) as f32;
    let duration_sec = args
        .get(1)
        .or_else(|| kwargs.get("duration"))
        .and_then(PyVal::as_float)
        .unwrap_or(1.0)
        .max(0.0) as f32;
    let frequency = args
        .get(2)
        .or_else(|| kwargs.get("frequency"))
        .and_then(PyVal::as_float)
        .unwrap_or(2.0)
        .max(0.0) as f32;
    let Some(h) = host else {
        return Err(vm.runtime_err("RuntimeError", "no active host context"));
    };
    require_perm(vm, h, Permission::Messages)?;
    h.ops.push(EngineOp::ScreenShake(ScreenShakeSpec {
        slot,
        amplitude,
        duration_sec,
        frequency,
    }));
    Ok(PyVal::None)
}

/// Builds a [`PyEventObj`] for a dispatched hook invocation.
pub fn build_hook_event(
    vm: &mut PyVm,
    hook: HookId,
    phase: HookPhase,
    ev: &cs_mod_host::natives::ActiveEvent,
) -> Result<ObjId, PyError> {
    let mut fields = BTreeMap::new();
    let mut single_player_slot = None;

    let make_player = |vm: &mut PyVm, raw: i32| -> Result<PyVal, PyError> {
        if let Ok(s) = u8::try_from(raw) {
            let id = vm.alloc(HeapObj::Native(NativeObjKind::Player(s)))?;
            Ok(PyVal::Handle(id))
        } else {
            Ok(PyVal::None)
        }
    };

    match hook {
        HookId::PlayerSpawn
        | HookId::PlayerPostThink
        | HookId::PlayerJump
        | HookId::ClientPutInServer
        | HookId::ClientDisconnect
        | HookId::ClientUserinfoChanged => {
            if let Ok(s) = u8::try_from(ev.i32_args[0]) {
                single_player_slot = Some(s);
                fields.insert("slot".to_string(), PyVal::Int(i64::from(s)));
                fields.insert("player".to_string(), make_player(vm, ev.i32_args[0])?);
            }
        }
        HookId::ClientConnect => {
            if let Ok(s) = u8::try_from(ev.i32_args[0]) {
                single_player_slot = Some(s);
                fields.insert("slot".to_string(), PyVal::Int(i64::from(s)));
                fields.insert("player".to_string(), make_player(vm, ev.i32_args[0])?);
            }
            fields.insert("name".to_string(), PyVal::Str(ev.str_args[0].clone()));
        }
        HookId::PlayerTakeDamage => {
            fields.insert("victim".to_string(), make_player(vm, ev.i32_args[0])?);
            fields.insert("victim_slot".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("attacker".to_string(), make_player(vm, ev.i32_args[1])?);
            fields.insert("attacker_slot".to_string(), PyVal::Int(i64::from(ev.i32_args[1])));
            fields.insert("hitgroup".to_string(), PyVal::Int(i64::from(ev.i32_args[2])));
            fields.insert("weapon_id".to_string(), PyVal::Int(i64::from(ev.i32_args[3])));
            fields.insert("damage_bits".to_string(), PyVal::Int(i64::from(ev.i32_args[4])));
            fields.insert("team_attack".to_string(), PyVal::Bool(ev.i32_args[5] != 0));
            fields.insert("damage".to_string(), PyVal::Float(f64::from(ev.f32_args[0])));
        }
        HookId::PlayerKilled | HookId::PlayerDeath => {
            fields.insert("victim".to_string(), make_player(vm, ev.i32_args[0])?);
            fields.insert("killer".to_string(), make_player(vm, ev.i32_args[1])?);
            fields.insert("attacker".to_string(), make_player(vm, ev.i32_args[1])?);
            fields.insert("weapon_id".to_string(), PyVal::Int(i64::from(ev.i32_args[2])));
            fields.insert("hitgroup".to_string(), PyVal::Int(i64::from(ev.i32_args[3])));
            fields.insert("headshot".to_string(), PyVal::Bool(ev.i32_args[4] != 0));
            fields.insert("team_kill".to_string(), PyVal::Bool(ev.i32_args[5] != 0));
            fields.insert("weapon_name".to_string(), PyVal::Str(ev.str_args[0].clone()));
        }
        HookId::PlayerPreThink | HookId::RunUsercmd | HookId::PostUsercmd => {
            if let Ok(s) = u8::try_from(ev.i32_args[0]) {
                single_player_slot = Some(s);
            }
            fields.insert("slot".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("player".to_string(), make_player(vm, ev.i32_args[0])?);
            fields.insert("buttons".to_string(), PyVal::Int(i64::from(ev.i32_args[1])));
            fields.insert("oldbuttons".to_string(), PyVal::Int(i64::from(ev.i32_args[2])));
        }
        HookId::PlayersShouldCollide => {
            fields.insert("slot_a".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("slot_b".to_string(), PyVal::Int(i64::from(ev.i32_args[1])));
            fields.insert("player_a".to_string(), make_player(vm, ev.i32_args[0])?);
            fields.insert("player_b".to_string(), make_player(vm, ev.i32_args[1])?);
            fields.insert(
                "should_collide".to_string(),
                PyVal::Bool(ev.i32_args[2] != 0),
            );
        }
        HookId::CanBuyItem => {
            fields.insert("slot".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("player".to_string(), make_player(vm, ev.i32_args[0])?);
            fields.insert("item_id".to_string(), PyVal::Int(i64::from(ev.i32_args[1])));
            fields.insert("price".to_string(), PyVal::Int(i64::from(ev.i32_args[2])));
        }
        HookId::CanHavePlayerItem
        | HookId::WeaponDeploy
        | HookId::WeaponPrimaryAttack
        | HookId::WeaponReload => {
            fields.insert("slot".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("player".to_string(), make_player(vm, ev.i32_args[0])?);
            fields.insert("weapon_id".to_string(), PyVal::Int(i64::from(ev.i32_args[1])));
        }
        HookId::PlayerSay => {
            fields.insert("slot".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("player".to_string(), make_player(vm, ev.i32_args[0])?);
            fields.insert("team_only".to_string(), PyVal::Bool(ev.i32_args[1] != 0));
            fields.insert("message".to_string(), PyVal::Str(ev.str_args[0].clone()));
        }
        HookId::MenuSelect => {
            fields.insert("slot".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("player".to_string(), make_player(vm, ev.i32_args[0])?);
            fields.insert("menu_id".to_string(), PyVal::Int(i64::from(ev.i32_args[1])));
            fields.insert("item_index".to_string(), PyVal::Int(i64::from(ev.i32_args[2])));
            fields.insert("info".to_string(), PyVal::Str(ev.str_args[0].clone()));
        }
        HookId::RoundStart => {
            fields.insert("round_num".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
        }
        HookId::RoundEnd => {
            fields.insert("winner".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("reason".to_string(), PyVal::Int(i64::from(ev.i32_args[1])));
            fields.insert("bonus".to_string(), PyVal::Int(i64::from(ev.i32_args[2])));
        }
        HookId::BombPlanting
        | HookId::BombPlanted
        | HookId::BombDefusing
        | HookId::BombDefused
        | HookId::BombExploded => {
            fields.insert("slot".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("player".to_string(), make_player(vm, ev.i32_args[0])?);
            fields.insert("has_kit".to_string(), PyVal::Bool(ev.i32_args[1] != 0));
            fields.insert(
                "origin".to_string(),
                PyVal::Tuple(vec![
                    PyVal::Float(f64::from(ev.f32_args[0])),
                    PyVal::Float(f64::from(ev.f32_args[1])),
                    PyVal::Float(f64::from(ev.f32_args[2])),
                ]),
            );
        }
        HookId::HostageTouched | HookId::HostageRescued => {
            fields.insert("slot".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("player".to_string(), make_player(vm, ev.i32_args[0])?);
            fields.insert("ent_index".to_string(), PyVal::Int(i64::from(ev.i32_args[1])));
        }
        HookId::GrenadeThrow => {
            fields.insert("slot".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("player".to_string(), make_player(vm, ev.i32_args[0])?);
            fields.insert("weapon_id".to_string(), PyVal::Int(i64::from(ev.i32_args[1])));
            fields.insert("ent_index".to_string(), PyVal::Int(i64::from(ev.i32_args[2])));
        }
        HookId::ClientCommand => {
            fields.insert("slot".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("player".to_string(), make_player(vm, ev.i32_args[0])?);
            fields.insert("command".to_string(), PyVal::Str(ev.str_args[0].clone()));
            fields.insert("arg1".to_string(), PyVal::Str(ev.str_args[1].clone()));
            fields.insert("args".to_string(), PyVal::Str(ev.str_args[2].clone()));
        }
        HookId::ServerCommand => {
            fields.insert("command".to_string(), PyVal::Str(ev.str_args[0].clone()));
            fields.insert("arg1".to_string(), PyVal::Str(ev.str_args[1].clone()));
            fields.insert("args".to_string(), PyVal::Str(ev.str_args[2].clone()));
        }
        HookId::StartFrame => {
            fields.insert("frametime".to_string(), PyVal::Float(f64::from(ev.f32_args[0])));
        }
        HookId::ServerActivate => {
            fields.insert("max_clients".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("map_name".to_string(), PyVal::Str(ev.str_args[0].clone()));
        }
        HookId::SpawnEntity => {
            fields.insert("ent_index".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("classname".to_string(), PyVal::Str(ev.str_args[0].clone()));
        }
        HookId::Touch | HookId::Blocked => {
            fields.insert("ent_a".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("ent_b".to_string(), PyVal::Int(i64::from(ev.i32_args[1])));
        }
        _ => {
            fields.insert("arg0".to_string(), PyVal::Int(i64::from(ev.i32_args[0])));
            fields.insert("arg1".to_string(), PyVal::Int(i64::from(ev.i32_args[1])));
            fields.insert("str0".to_string(), PyVal::Str(ev.str_args[0].clone()));
        }
    }

    vm.alloc(HeapObj::Native(NativeObjKind::Event(PyEventObj {
        hook,
        phase,
        fields,
        single_player_slot,
        mutated_damage: None,
        mutated_hitgroup: None,
        mutated_message: None,
        mutated_price: None,
        mutated_should_collide: None,
    })))
}

fn require_perm(vm: &PyVm, host: &HostBridge<'_>, perm: Permission) -> Result<(), PyError> {
    if host.has_perm(perm) {
        Ok(())
    } else {
        Err(vm.runtime_err(
            "PermissionError",
            format!("missing required permission '{}'", perm.as_str()),
        ))
    }
}

fn parse_hook_id(vm: &PyVm, val: &PyVal) -> Result<HookId, PyError> {
    match val {
        PyVal::Int(n) => HookId::from_i32(*n as i32)
            .ok_or_else(|| vm.runtime_err("ValueError", format!("unknown HookId {n}"))),
        PyVal::Str(s) => HookId::from_name(s)
            .ok_or_else(|| vm.runtime_err("ValueError", format!("unknown hook name '{s}'"))),
        _ => Err(vm.runtime_err("TypeError", "hook must be HookId int or hook name str")),
    }
}

fn parse_hook_phase(vm: &PyVm, val: &PyVal) -> Result<HookPhase, PyError> {
    match val {
        PyVal::Int(n) => HookPhase::from_i32(*n as i32)
            .ok_or_else(|| vm.runtime_err("ValueError", format!("invalid HookPhase {n}"))),
        PyVal::Str(s) => match s.trim().to_ascii_lowercase().as_str() {
            "pre" => Ok(HookPhase::Pre),
            "post" => Ok(HookPhase::Post),
            _ => Err(vm.runtime_err("ValueError", format!("invalid hook phase '{s}'"))),
        },
        _ => Err(vm.runtime_err("TypeError", "phase must be 'pre', 'post', or HookPhase")),
    }
}

fn default_phase_for_hook(hook: HookId) -> HookPhase {
    match hook {
        HookId::PlayerSpawn
        | HookId::PlayerKilled
        | HookId::PlayerDeath
        | HookId::RoundStart
        | HookId::RoundEnd
        | HookId::FreezeTimeEnd
        | HookId::PlayerPostThink
        | HookId::ClientPutInServer
        | HookId::ClientDisconnect
        | HookId::BombPlanted
        | HookId::BombDefused
        | HookId::BombExploded
        | HookId::HostageRescued
        | HookId::GrenadeThrow => HookPhase::Post,
        _ => HookPhase::Pre,
    }
}

fn extract_slot(vm: &PyVm, val: &PyVal) -> Result<u8, PyError> {
    match val {
        PyVal::Handle(id) => match vm.heap.get(*id) {
            Some(HeapObj::Native(NativeObjKind::Player(s))) => Ok(*s),
            _ => Err(vm.runtime_err("TypeError", "expected Player or slot int")),
        },
        _ => val
            .as_int()
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(|| vm.runtime_err("TypeError", "expected Player or slot int")),
    }
}

fn extract_opt_slot(vm: &PyVm, val: &PyVal) -> Option<u8> {
    if matches!(val, PyVal::None) {
        return None;
    }
    extract_slot(vm, val).ok()
}

fn extract_ent_index(vm: &PyVm, val: &PyVal) -> Result<u16, PyError> {
    match val {
        PyVal::Handle(id) => match vm.heap.get(*id) {
            Some(HeapObj::Native(NativeObjKind::Player(s))) => Ok(u16::from(*s) + 1),
            Some(HeapObj::Native(NativeObjKind::Entity(e))) => Ok(*e),
            _ => Err(vm.runtime_err("TypeError", "expected entity index, Entity, or Player")),
        },
        _ => val
            .as_int()
            .and_then(|n| u16::try_from(n).ok())
            .ok_or_else(|| vm.runtime_err("TypeError", "expected entity index")),
    }
}

fn extract_vec3(vm: &PyVm, val: &PyVal) -> Result<[f32; 3], PyError> {
    let items = vm.to_vec(val)?;
    if items.len() != 3 {
        return Err(vm.runtime_err("ValueError", "expected 3D vector (x, y, z)"));
    }
    let x = items[0]
        .as_float()
        .ok_or_else(|| vm.runtime_err("TypeError", "vector component must be numeric"))? as f32;
    let y = items[1]
        .as_float()
        .ok_or_else(|| vm.runtime_err("TypeError", "vector component must be numeric"))? as f32;
    let z = items[2]
        .as_float()
        .ok_or_else(|| vm.runtime_err("TypeError", "vector component must be numeric"))? as f32;
    if !x.is_finite() || !y.is_finite() || !z.is_finite() {
        return Err(vm.runtime_err("ValueError", "vector components must be finite"));
    }
    Ok([x, y, z])
}

fn extract_rgb(vm: &PyVm, val: &PyVal) -> Result<[u8; 3], PyError> {
    if let Some(packed) = val.as_int() {
        let u = packed as u32;
        return Ok([
            ((u >> 16) & 0xFF) as u8,
            ((u >> 8) & 0xFF) as u8,
            (u & 0xFF) as u8,
        ]);
    }
    let items = vm.to_vec(val)?;
    if items.len() < 3 {
        return Err(vm.runtime_err("ValueError", "expected (r, g, b) tuple"));
    }
    Ok([
        items[0].as_int().unwrap_or(255).clamp(0, 255) as u8,
        items[1].as_int().unwrap_or(255).clamp(0, 255) as u8,
        items[2].as_int().unwrap_or(255).clamp(0, 255) as u8,
    ])
}

fn extract_rgba(vm: &PyVm, val: &PyVal) -> Result<[u8; 4], PyError> {
    if let Some(packed) = val.as_int() {
        let u = packed as u32;
        return Ok([
            ((u >> 24) & 0xFF) as u8,
            ((u >> 16) & 0xFF) as u8,
            ((u >> 8) & 0xFF) as u8,
            (u & 0xFF) as u8,
        ]);
    }
    let items = vm.to_vec(val)?;
    if items.len() < 3 {
        return Err(vm.runtime_err("ValueError", "expected (r, g, b[, a]) tuple"));
    }
    Ok([
        items[0].as_int().unwrap_or(255).clamp(0, 255) as u8,
        items[1].as_int().unwrap_or(255).clamp(0, 255) as u8,
        items[2].as_int().unwrap_or(255).clamp(0, 255) as u8,
        items
            .get(3)
            .and_then(PyVal::as_int)
            .unwrap_or(255)
            .clamp(0, 255) as u8,
    ])
}

fn pyval_to_rpc(vm: &PyVm, val: &PyVal) -> RpcValue {
    match val {
        PyVal::Int(n) => RpcValue::Int(*n as i32),
        PyVal::Bool(b) => RpcValue::Int(i32::from(*b)),
        PyVal::Float(f) => RpcValue::Float(*f as f32),
        PyVal::Str(s) => RpcValue::Str(s.clone()),
        other => RpcValue::Str(vm.val_to_str(other)),
    }
}
