//! `PyModInstance` (`cs_mod_host::ModInstance`), `PyModRuntime` (`cs_mod_host::ModRuntime`),
//! syntax checker (`PySourceSyntaxCheck`), and standalone `.py` manifest synthesizer.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cs_mod_api::services::{
    PluginRuntimeKind, RpcEventSubscriber, RpcExportEntry, SharedPlatformServices,
};
use cs_mod_api::{
    ABI_VERSION, DEFAULT_FUEL_PER_FRAME, DEFAULT_MEMORY_BYTES, HookAction, HookId, HookPhase,
    ModManifest, ModSide, Permission, Permissions,
};
use cs_mod_host::natives::{EngineOp, HookRegistration, RpcDispatcher, TimerRequest};
use cs_mod_host::runtime::{
    ModCallOutcome, ModExecContext, ModInstance, ModRuntime, PythonModRuntime, RuntimeError,
};

use crate::bindings::build_hook_event;
use crate::error::PyError;
use crate::parser::parse_source;
use crate::value::{HeapObj, NativeObjKind, PyPluginMeta, PyVal};
use crate::vm::{HostBridge, PyVm};

/// Default permissions granted to standalone `mods/*.py` scripts that omit an explicit
/// `permissions = [...]` list in `@mod.info(...)` (excludes `Http` unless explicitly allowlisted!).
pub fn default_python_permissions() -> Permissions {
    let mut p = Permissions::NONE;
    for perm in [
        Permission::Entities,
        Permission::Messages,
        Permission::Cvars,
        Permission::Commands,
        Permission::Menus,
        Permission::Timers,
        Permission::Traces,
        Permission::Sound,
        Permission::Storage,
        Permission::Vfx,
        Permission::Weapons,
        Permission::Admins,
        Permission::Rpc,
    ] {
        p.insert(perm);
    }
    p
}

/// Result returned by [`PySourceSyntaxCheck::check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PySyntaxReport {
    pub file_name: String,
    pub meta: PyPluginMeta,
    pub hook_count: usize,
    pub chat_command_count: usize,
    pub console_command_count: usize,
    pub timer_count: usize,
}

/// Static + dry-load syntax validator for Python plugins (used by `xtask mod build` & `/admin`).
pub struct PySourceSyntaxCheck;

impl PySourceSyntaxCheck {
    /// Parses `source` and executes top-level declarations inside a bounded dry-run VM to verify
    /// syntax, decorator arguments, and `@mod.info(...)` metadata without touching any live server.
    pub fn check(file_name: &str, source: &str) -> Result<PySyntaxReport, PyError> {
        let program = parse_source(file_name, source)?;
        let mut vm = PyVm::new("syntax_check", file_name, DEFAULT_MEMORY_BYTES)?;
        vm.exec_program(&program, DEFAULT_FUEL_PER_FRAME, None)?;
        Ok(PySyntaxReport {
            file_name: file_name.to_string(),
            meta: vm.meta,
            hook_count: vm.hooks.len(),
            chat_command_count: vm.chat_commands.len(),
            console_command_count: vm.console_commands.len(),
            timer_count: vm.startup_timers.len(),
        })
    }

    /// Parse-only syntax check (faster, executes zero user code).
    pub fn parse_only(file_name: &str, source: &str) -> Result<(), PyError> {
        let _ = parse_source(file_name, source)?;
        Ok(())
    }
}

/// Synthesizes a valid [`ModManifest`] from a standalone `.py` plugin's `@mod.info(...)` metadata
/// so `ServerModHost` can manage `mods/*.py` and `mods/<dir>/main.py` uniformly with `.wasm` mods.
pub fn synthesize_manifest_from_py(
    mod_id_fallback: &str,
    file_name: &str,
    source: &str,
) -> Result<ModManifest, PyError> {
    let report = PySourceSyntaxCheck::check(file_name, source)?;
    let raw_id = report
        .meta
        .id
        .as_deref()
        .unwrap_or(mod_id_fallback)
        .trim()
        .to_ascii_lowercase();
    let id: String = raw_id
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    let id = if id.is_empty() {
        "py_plugin".to_string()
    } else {
        id
    };

    let (permissions, http_hosts) = if report.meta.permissions.is_empty() {
        (default_python_permissions(), Vec::new())
    } else {
        parse_py_permissions(&report.meta.permissions)
    };

    Ok(ModManifest {
        id: id.clone(),
        name: report.meta.name.unwrap_or_else(|| id.clone()),
        version: report.meta.version.unwrap_or_else(|| "1.0.0".to_string()),
        api_version: ABI_VERSION,
        side: ModSide::Server,
        entry_server: Some(file_name.to_string()),
        entry_client: None,
        dependencies: Vec::new(),
        permissions,
        http_hosts,
        assets: Vec::new(),
        custom_usermsgs: Vec::new(),
        fuel_per_frame: report
            .meta
            .fuel_per_frame
            .unwrap_or(DEFAULT_FUEL_PER_FRAME)
            .clamp(1_000, cs_mod_api::MAX_FUEL_PER_FRAME),
        max_memory_bytes: DEFAULT_MEMORY_BYTES,
    })
}

fn parse_py_permissions(items: &[String]) -> (Permissions, Vec<String>) {
    let mut perms = Permissions::NONE;
    let mut hosts = Vec::new();
    for raw in items {
        let lower = raw.trim().to_ascii_lowercase();
        if let Some(host) = lower.strip_prefix("http:") {
            if cs_mod_api::manifest::is_valid_http_host(host) {
                perms.insert(Permission::Http);
                if !hosts.iter().any(|h| h == host) {
                    hosts.push(host.to_string());
                }
            }
        } else if let Some(p) = Permission::parse(&lower) {
            perms.insert(p);
        }
    }
    (perms, hosts)
}

/// Pluggable [`ModRuntime`] implementation that instantiates UTF-8 Python 3 scripts.
#[derive(Debug, Default, Clone)]
pub struct PyModRuntime;

impl PyModRuntime {
    pub fn new() -> Self {
        Self
    }
}

impl ModRuntime for PyModRuntime {
    fn name(&self) -> &'static str {
        "python"
    }

    fn instantiate(
        &self,
        mod_id: &str,
        source_bytes: &[u8],
        permissions: Permissions,
        max_memory_bytes: usize,
    ) -> Result<Box<dyn ModInstance>, RuntimeError> {
        let source = std::str::from_utf8(source_bytes)
            .map_err(|e| RuntimeError::Compile(mod_id.to_string(), format!("invalid UTF-8: {e}")))?;
        let file_name = format!("{mod_id}.py");
        let inst = PyModInstance::from_source(
            mod_id,
            &file_name,
            source,
            permissions,
            max_memory_bytes,
        )
        .map_err(|e| e.into_host_error(mod_id))?;
        Ok(Box::new(inst))
    }
}

impl PythonModRuntime for PyModRuntime {
    fn synthesize_manifest(
        &self,
        mod_id_fallback: &str,
        file_name: &str,
        source: &str,
    ) -> Result<ModManifest, String> {
        synthesize_manifest_from_py(mod_id_fallback, file_name, source).map_err(|e| e.to_string())
    }
}

/// Live sandboxed Python 3 plugin instance implementing [`cs_mod_host::ModInstance`].
pub struct PyModInstance {
    mod_id: String,
    file_name: String,
    permissions: Permissions,
    max_memory_bytes: usize,
    http_hosts: Vec<String>,
    services: SharedPlatformServices,
    rpc_dispatcher: Option<Arc<dyn RpcDispatcher>>,
    runtime_kind: PluginRuntimeKind,
    vm: PyVm,
    last_error: Option<PyError>,
}

impl PyModInstance {
    /// Parses and loads a Python 3 plugin script with strict startup fuel & memory limits.
    pub fn from_source(
        mod_id: &str,
        file_name: &str,
        source: &str,
        permissions: Permissions,
        max_memory_bytes: usize,
    ) -> Result<Self, PyError> {
        let program = parse_source(file_name, source)?;
        let mut vm = PyVm::new(mod_id, file_name, max_memory_bytes)?;
        vm.exec_program(&program, DEFAULT_FUEL_PER_FRAME, None)?;
        let mut effective_perms = permissions;
        let mut extra_hosts = Vec::new();
        if !vm.meta.permissions.is_empty() && effective_perms == Permissions::NONE {
            let (p, h) = parse_py_permissions(&vm.meta.permissions);
            effective_perms = p;
            extra_hosts = h;
        }
        Ok(Self {
            mod_id: mod_id.to_string(),
            file_name: file_name.to_string(),
            permissions: effective_perms,
            max_memory_bytes,
            http_hosts: extra_hosts,
            services: SharedPlatformServices::new(),
            rpc_dispatcher: None,
            runtime_kind: PluginRuntimeKind::Python,
            vm,
            last_error: None,
        })
    }

    pub fn mod_id(&self) -> &str {
        &self.mod_id
    }

    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    pub fn meta(&self) -> &PyPluginMeta {
        &self.vm.meta
    }

    pub fn last_error(&self) -> Option<&PyError> {
        self.last_error.as_ref()
    }

    pub fn set_runtime_kind(&mut self, kind: PluginRuntimeKind) {
        self.runtime_kind = kind;
    }

    /// Hot-reloads this instance from updated Python source while keeping its host `services`,
    /// `http_hosts`, `permissions`, and external `mod.storage` intact!
    ///
    /// If the new source has a syntax or load error, the previous live VM remains untouched and
    /// the error is returned cleanly.
    pub fn reload_source(&mut self, new_source: &str) -> Result<(), PyError> {
        let program = parse_source(&self.file_name, new_source)?;
        let mut fresh_vm = PyVm::new(&self.mod_id, &self.file_name, self.max_memory_bytes)?;
        fresh_vm.exec_program(&program, DEFAULT_FUEL_PER_FRAME, None)?;
        {
            let mut svc = self.services.lock();
            svc.menus.clear_owner(&self.mod_id);
            svc.rpc.unregister_owner(&self.mod_id);
        }
        self.vm = fresh_vm;
        self.last_error = None;
        Ok(())
    }

    fn record_err(&mut self, err: PyError) -> RuntimeError {
        self.last_error = Some(err.clone());
        err.into_host_error(&self.mod_id)
    }
}

fn make_bridge<'a>(
    mod_id: &str,
    permissions: Permissions,
    http_hosts: &'a [String],
    services: &'a SharedPlatformServices,
    rpc_dispatcher: Option<&'a Arc<dyn RpcDispatcher>>,
    runtime_kind: PluginRuntimeKind,
    ctx: &'a ModExecContext<'a>,
) -> HostBridge<'a> {
    HostBridge {
        mod_id: mod_id.to_string(),
        snapshot: ctx.snapshot,
        event: ctx.event,
        storage_read: ctx.storage,
        storage_overlay: BTreeMap::new(),
        storage_writes: Vec::new(),
        permissions,
        http_hosts,
        services,
        rpc_dispatcher,
        ops: Vec::new(),
        timers: Vec::new(),
        cancel_timers: Vec::new(),
        next_local_timer_id: 1,
        override_i32: None,
        override_i32_2: None,
        override_f32: None,
        override_str: None,
        rpc_return_str: None,
        logs: Vec::new(),
        runtime_kind,
    }
}

fn finish_outcome(
    vm: &mut PyVm,
    bridge: HostBridge<'_>,
    action: HookAction,
    hooks_registered: Vec<HookRegistration>,
) -> ModCallOutcome {
    let fuel_used = vm.fuel_used();
    let mut wasi_logs = std::mem::take(&mut vm.pending_logs);
    wasi_logs.extend(bridge.logs);
    let outcome = ModCallOutcome {
        action,
        ops: bridge.ops,
        timers: bridge.timers,
        cancel_timers: bridge.cancel_timers,
        storage_writes: bridge.storage_writes,
        hud_cmds: Vec::new(),
        hooks_registered,
        override_i32: bridge.override_i32,
        override_i32_2: bridge.override_i32_2,
        override_f32: bridge.override_f32,
        override_str: bridge.override_str,
        rpc_return_str: bridge.rpc_return_str,
        wasi_logs,
        fuel_used,
    };
    vm.maybe_gc();
    outcome
}

impl ModInstance for PyModInstance {
    fn configure_v2(
        &mut self,
        services: SharedPlatformServices,
        http_hosts: Vec<String>,
        rpc_dispatcher: Option<Arc<dyn RpcDispatcher>>,
    ) {
        self.services = services;
        if !http_hosts.is_empty() {
            self.http_hosts = http_hosts;
        }
        self.rpc_dispatcher = rpc_dispatcher;
    }

    fn call_init(&mut self, ctx: &ModExecContext<'_>) -> Result<ModCallOutcome, RuntimeError> {
        self.vm.reset_fuel(ctx.fuel_budget);
        let mut bridge = make_bridge(
            &self.mod_id,
            self.permissions,
            &self.http_hosts,
            &self.services,
            self.rpc_dispatcher.as_ref(),
            self.runtime_kind,
            ctx,
        );

        // Flush ops & custom weapons queued at top-level import time
        bridge.ops.extend(std::mem::take(&mut self.vm.init_ops));
        for cw in std::mem::take(&mut self.vm.init_custom_weapons) {
            bridge.ops.push(EngineOp::RegisterCustomWeapon(cw));
        }

        // Collect all unique hook registrations
        let mut hooks_reg = Vec::new();
        let mut seen_hooks = BTreeSet::new();
        for h in &self.vm.hooks {
            if seen_hooks.insert((h.hook, h.phase, h.priority)) {
                hooks_reg.push(HookRegistration {
                    hook: h.hook,
                    phase: h.phase,
                    priority: h.priority,
                });
            }
        }
        if !self.vm.chat_commands.is_empty()
            && seen_hooks.insert((HookId::PlayerSay, HookPhase::Pre, 0))
        {
            hooks_reg.push(HookRegistration {
                hook: HookId::PlayerSay,
                phase: HookPhase::Pre,
                priority: 0,
            });
        }
        if !self.vm.console_commands.is_empty() {
            if seen_hooks.insert((HookId::ClientCommand, HookPhase::Pre, 0)) {
                hooks_reg.push(HookRegistration {
                    hook: HookId::ClientCommand,
                    phase: HookPhase::Pre,
                    priority: 0,
                });
            }
            if seen_hooks.insert((HookId::ServerCommand, HookPhase::Pre, 0)) {
                hooks_reg.push(HookRegistration {
                    hook: HookId::ServerCommand,
                    phase: HookPhase::Pre,
                    priority: 0,
                });
            }
            for cc in &self.vm.console_commands {
                for name in &cc.commands {
                    bridge.ops.push(EngineOp::RegisterCommand { name: name.clone() });
                }
            }
        }

        // Schedule decorator-registered timers
        for t in &self.vm.startup_timers {
            bridge.timers.push(TimerRequest {
                delay_ms: t.delay_ms,
                repeat: t.repeat,
                cb_id: t.cb_id,
                arg: t.arg,
            });
        }

        // Register decorator-exported RPC functions and event subscribers in shared services
        if self.permissions.contains(Permission::Rpc) {
            let mut svc = bridge.services.lock();
            for (&cb_id, (fn_name, _)) in &self.vm.rpc_exports {
                svc.rpc.export_fn(RpcExportEntry {
                    fn_name: fn_name.clone(),
                    owner_id: self.mod_id.clone(),
                    runtime: self.runtime_kind,
                    owner_index: 0,
                    callback_id: cb_id,
                    callback_name: fn_name.clone(),
                });
            }
            for (&cb_id, (ev_name, _)) in &self.vm.event_callbacks {
                svc.rpc.subscribe_event(
                    ev_name.clone(),
                    RpcEventSubscriber {
                        owner_id: self.mod_id.clone(),
                        runtime: self.runtime_kind,
                        owner_index: 0,
                        callback_id: cb_id,
                        callback_name: ev_name.clone(),
                    },
                );
            }
        }

        // Invoke optional `on_init()` function if present
        if let Some(init_fn) = self.vm.globals.get("on_init").cloned() {
            let init_res = self
                .vm
                .call_callback_flexible(init_fn, Vec::new(), Some(&mut bridge));
            if let Err(err) = init_res {
                drop(bridge);
                return Err(self.record_err(err));
            }
        }

        Ok(finish_outcome(
            &mut self.vm,
            bridge,
            HookAction::Continue,
            hooks_reg,
        ))
    }

    fn call_hook(
        &mut self,
        hook: HookId,
        phase: HookPhase,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        self.vm.reset_fuel(ctx.fuel_budget);
        let mut bridge = make_bridge(
            &self.mod_id,
            self.permissions,
            &self.http_hosts,
            &self.services,
            self.rpc_dispatcher.as_ref(),
            self.runtime_kind,
            ctx,
        );
        let mut combined_action = HookAction::Continue;

        // 1. Dispatch `@mod.chat_command(...)` on `PlayerSay` Pre
        if hook == HookId::PlayerSay && phase == HookPhase::Pre && !self.vm.chat_commands.is_empty()
        {
            let raw_msg = ctx.event.str_args[0].trim();
            let (cmd_tok, rest_args) = match raw_msg.find(char::is_whitespace) {
                Some(idx) => (&raw_msg[..idx], raw_msg[idx..].trim()),
                None => (raw_msg, ""),
            };
            let matched: Vec<(bool, PyVal)> = self
                .vm
                .chat_commands
                .iter()
                .filter(|cc| {
                    cc.commands
                        .iter()
                        .any(|alias| alias.eq_ignore_ascii_case(cmd_tok))
                })
                .map(|cc| (cc.suppress, cc.func.clone()))
                .collect();

            if !matched.is_empty() {
                let slot = u8::try_from(ctx.event.i32_args[0]).unwrap_or(0);
                let player_id = match self.vm.alloc(HeapObj::Native(NativeObjKind::Player(slot))) {
                    Ok(id) => id,
                    Err(e) => {
                        drop(bridge);
                        return Err(self.record_err(e));
                    }
                };
                for (suppress, func) in matched {
                    let call_res = self.vm.call_callback_flexible(
                        func,
                        vec![
                            PyVal::Handle(player_id),
                            PyVal::Str(rest_args.to_string()),
                            PyVal::Str(raw_msg.to_string()),
                        ],
                        Some(&mut bridge),
                    );
                    let ret = match call_res {
                        Ok(v) => v,
                        Err(err) => {
                            drop(bridge);
                            return Err(self.record_err(err));
                        }
                    };
                    let act = match ret {
                        PyVal::None => {
                            if suppress {
                                HookAction::Supercede
                            } else {
                                HookAction::Handled
                            }
                        }
                        other => extract_hook_action(&other),
                    };
                    combined_action = merge_hook_action(combined_action, act);
                    if combined_action == HookAction::Supercede {
                        return Ok(finish_outcome(
                            &mut self.vm,
                            bridge,
                            combined_action,
                            Vec::new(),
                        ));
                    }
                }
            }
        }

        // 2. Dispatch `@mod.console_command(...)` on `ClientCommand` / `ServerCommand` Pre
        if matches!(hook, HookId::ClientCommand | HookId::ServerCommand)
            && phase == HookPhase::Pre
            && !self.vm.console_commands.is_empty()
        {
            let cmd_name = ctx.event.str_args[0].trim();
            let matched: Vec<PyVal> = self
                .vm
                .console_commands
                .iter()
                .filter(|cc| {
                    cc.commands
                        .iter()
                        .any(|alias| alias.eq_ignore_ascii_case(cmd_name))
                })
                .map(|cc| cc.func.clone())
                .collect();
            if !matched.is_empty() {
                let player_val = if hook == HookId::ClientCommand {
                    let slot = u8::try_from(ctx.event.i32_args[0]).unwrap_or(0);
                    let id = match self.vm.alloc(HeapObj::Native(NativeObjKind::Player(slot))) {
                        Ok(id) => id,
                        Err(e) => {
                            drop(bridge);
                            return Err(self.record_err(e));
                        }
                    };
                    PyVal::Handle(id)
                } else {
                    PyVal::None
                };
                let full_line = ctx.event.str_args[2].as_str();
                let arg_list: Vec<PyVal> = full_line
                    .split_whitespace()
                    .skip(1)
                    .map(|s| PyVal::Str(s.to_string()))
                    .collect();
                let list_id = match self.vm.alloc(HeapObj::List(arg_list)) {
                    Ok(id) => id,
                    Err(e) => {
                        drop(bridge);
                        return Err(self.record_err(e));
                    }
                };
                for func in matched {
                    let ret = match self.vm.call_callback_flexible(
                        func,
                        vec![player_val.clone(), PyVal::Handle(list_id)],
                        Some(&mut bridge),
                    ) {
                        Ok(v) => v,
                        Err(err) => {
                            drop(bridge);
                            return Err(self.record_err(err));
                        }
                    };
                    let act = match ret {
                        PyVal::Str(reply) => {
                            bridge.override_str = Some(reply);
                            HookAction::Handled
                        }
                        PyVal::None => HookAction::Handled,
                        other => extract_hook_action(&other),
                    };
                    combined_action = merge_hook_action(combined_action, act);
                    if combined_action == HookAction::Supercede {
                        return Ok(finish_outcome(
                            &mut self.vm,
                            bridge,
                            combined_action,
                            Vec::new(),
                        ));
                    }
                }
            }
        }

        // 3. Dispatch `@mod.hook(...)` callbacks matching `(hook, phase)` sorted by priority
        let mut matching_hooks: Vec<(i32, PyVal)> = self
            .vm
            .hooks
            .iter()
            .filter(|h| h.hook == hook && h.phase == phase)
            .map(|h| (h.priority, h.func.clone()))
            .collect();
        matching_hooks.sort_by_key(|(prio, _)| *prio);

        if !matching_hooks.is_empty() {
            let ev_id = match build_hook_event(&mut self.vm, hook, phase, ctx.event) {
                Ok(id) => id,
                Err(e) => {
                    drop(bridge);
                    return Err(self.record_err(e));
                }
            };
            for (_, func) in matching_hooks {
                let ret = match self.vm.call_callback_flexible(
                    func,
                    vec![PyVal::Handle(ev_id)],
                    Some(&mut bridge),
                ) {
                    Ok(v) => v,
                    Err(err) => {
                        drop(bridge);
                        return Err(self.record_err(err));
                    }
                };
                let mut act = extract_hook_action(&ret);

                // Sync mutable event field overrides back into HostBridge
                let mut mutated = false;
                if let Some(HeapObj::Native(NativeObjKind::Event(ev_obj))) =
                    self.vm.heap.get(ev_id)
                {
                    if let Some(dmg) = ev_obj.mutated_damage {
                        bridge.override_f32 = Some(dmg);
                        mutated = true;
                    }
                    if let Some(hg) = ev_obj.mutated_hitgroup {
                        bridge.override_i32 = Some(hg);
                        mutated = true;
                    }
                    if let Some(ref msg) = ev_obj.mutated_message {
                        bridge.override_str = Some(msg.clone());
                        mutated = true;
                    }
                    if let Some(price) = ev_obj.mutated_price {
                        bridge.override_i32 = Some(price);
                        mutated = true;
                    }
                    if let Some(sc) = ev_obj.mutated_should_collide {
                        bridge.override_i32 = Some(i32::from(sc));
                        mutated = true;
                    }
                }
                if mutated && act == HookAction::Continue {
                    act = HookAction::Override;
                }
                combined_action = merge_hook_action(combined_action, act);
                if combined_action == HookAction::Supercede {
                    break;
                }
            }
        }

        Ok(finish_outcome(
            &mut self.vm,
            bridge,
            combined_action,
            Vec::new(),
        ))
    }

    fn call_timer(
        &mut self,
        cb_id: i32,
        arg: i32,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        let Some(func) = self.vm.timer_callbacks.get(&cb_id).cloned() else {
            return Ok(ModCallOutcome::default());
        };
        self.vm.reset_fuel(ctx.fuel_budget);
        let mut bridge = make_bridge(
            &self.mod_id,
            self.permissions,
            &self.http_hosts,
            &self.services,
            self.rpc_dispatcher.as_ref(),
            self.runtime_kind,
            ctx,
        );
        let ret = match self
            .vm
            .call_callback_flexible(func, vec![PyVal::Int(i64::from(arg))], Some(&mut bridge))
        {
            Ok(v) => v,
            Err(err) => {
                drop(bridge);
                return Err(self.record_err(err));
            }
        };
        let act = extract_hook_action(&ret);
        Ok(finish_outcome(&mut self.vm, bridge, act, Vec::new()))
    }

    fn call_menu(
        &mut self,
        cb_id: i32,
        slot: u8,
        menu_id: u32,
        item_index: i32,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        let Some(func) = self.vm.menu_callbacks.get(&cb_id).cloned() else {
            return Ok(ModCallOutcome::default());
        };
        self.vm.reset_fuel(ctx.fuel_budget);
        let mut bridge = make_bridge(
            &self.mod_id,
            self.permissions,
            &self.http_hosts,
            &self.services,
            self.rpc_dispatcher.as_ref(),
            self.runtime_kind,
            ctx,
        );
        let player_id = match self.vm.alloc(HeapObj::Native(NativeObjKind::Player(slot))) {
            Ok(id) => id,
            Err(e) => {
                drop(bridge);
                return Err(self.record_err(e));
            }
        };
        let info = ctx.event.str_args[0].clone();
        // Support both `cb(player, info)` (2 params), `cb(player, item_index, info)` (3 params),
        // and `cb(player, menu_id, item_index, info)` (4 params)!
        let param_count = match &func {
            PyVal::Handle(fid) => match self.vm.heap.get(*fid) {
                Some(HeapObj::Function(f)) => Some(f.params.positional.len()),
                _ => None,
            },
            _ => None,
        };
        let call_args = match param_count {
            Some(1) => vec![PyVal::Handle(player_id)],
            Some(2) => vec![PyVal::Handle(player_id), PyVal::Str(info)],
            Some(4) => vec![
                PyVal::Handle(player_id),
                PyVal::Int(i64::from(menu_id)),
                PyVal::Int(i64::from(item_index)),
                PyVal::Str(info),
            ],
            _ => vec![
                PyVal::Handle(player_id),
                PyVal::Int(i64::from(item_index)),
                PyVal::Str(info),
            ],
        };
        let ret = match self
            .vm
            .call_callback_flexible(func, call_args, Some(&mut bridge))
        {
            Ok(v) => v,
            Err(err) => {
                drop(bridge);
                return Err(self.record_err(err));
            }
        };
        let act = extract_hook_action(&ret);
        Ok(finish_outcome(&mut self.vm, bridge, act, Vec::new()))
    }

    fn call_rpc(
        &mut self,
        cb_id: i32,
        ctx: &ModExecContext<'_>,
    ) -> Result<(i32, ModCallOutcome), RuntimeError> {
        let Some((_, func)) = self.vm.rpc_exports.get(&cb_id).cloned() else {
            return Ok((0, ModCallOutcome::default()));
        };
        self.vm.reset_fuel(ctx.fuel_budget);
        let mut bridge = make_bridge(
            &self.mod_id,
            self.permissions,
            &self.http_hosts,
            &self.services,
            self.rpc_dispatcher.as_ref(),
            self.runtime_kind,
            ctx,
        );
        let mut args = Vec::new();
        args.push(PyVal::Int(i64::from(ctx.event.i32_args[0])));
        if !ctx.event.str_args[0].is_empty() {
            args.push(PyVal::Str(ctx.event.str_args[0].clone()));
        }
        let ret = match self.vm.call_callback_flexible(func, args, Some(&mut bridge)) {
            Ok(v) => v,
            Err(err) => {
                drop(bridge);
                return Err(self.record_err(err));
            }
        };
        let rc = match ret {
            PyVal::Int(n) => n as i32,
            PyVal::Bool(b) => i32::from(b),
            PyVal::Float(f) => f as i32,
            PyVal::Str(s) => {
                bridge.rpc_return_str = Some(s);
                0
            }
            PyVal::None => 0,
            other => {
                bridge.rpc_return_str = Some(self.vm.val_to_str(&other));
                0
            }
        };
        Ok((
            rc,
            finish_outcome(&mut self.vm, bridge, HookAction::Continue, Vec::new()),
        ))
    }

    fn call_event(
        &mut self,
        cb_id: i32,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        let Some((_, func)) = self.vm.event_callbacks.get(&cb_id).cloned() else {
            return Ok(ModCallOutcome::default());
        };
        self.vm.reset_fuel(ctx.fuel_budget);
        let mut bridge = make_bridge(
            &self.mod_id,
            self.permissions,
            &self.http_hosts,
            &self.services,
            self.rpc_dispatcher.as_ref(),
            self.runtime_kind,
            ctx,
        );
        let payload = ctx.event.str_args[1].clone();
        let ev_name = ctx.event.str_args[0].clone();
        let ret = match self.vm.call_callback_flexible(
            func,
            vec![PyVal::Str(payload), PyVal::Str(ev_name)],
            Some(&mut bridge),
        ) {
            Ok(v) => v,
            Err(err) => {
                drop(bridge);
                return Err(self.record_err(err));
            }
        };
        let act = extract_hook_action(&ret);
        Ok(finish_outcome(&mut self.vm, bridge, act, Vec::new()))
    }

    fn call_webhook(
        &mut self,
        cb_id: i32,
        status: i32,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        let Some(func) = self.vm.webhook_callbacks.remove(&cb_id) else {
            return Ok(ModCallOutcome::default());
        };
        self.vm.reset_fuel(ctx.fuel_budget);
        let mut bridge = make_bridge(
            &self.mod_id,
            self.permissions,
            &self.http_hosts,
            &self.services,
            self.rpc_dispatcher.as_ref(),
            self.runtime_kind,
            ctx,
        );
        let body = ctx.event.str_args[0].clone();
        let err_str = if ctx.event.str_args[1].is_empty() {
            PyVal::None
        } else {
            PyVal::Str(ctx.event.str_args[1].clone())
        };
        let ret = match self.vm.call_callback_flexible(
            func,
            vec![PyVal::Int(i64::from(status)), PyVal::Str(body), err_str],
            Some(&mut bridge),
        ) {
            Ok(v) => v,
            Err(err) => {
                drop(bridge);
                return Err(self.record_err(err));
            }
        };
        let act = extract_hook_action(&ret);
        Ok(finish_outcome(&mut self.vm, bridge, act, Vec::new()))
    }

    fn call_client_hud(
        &mut self,
        _dt_ms: i32,
        _ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        Ok(ModCallOutcome::default())
    }

    fn call_client_usermsg(
        &mut self,
        _ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        Ok(ModCallOutcome::default())
    }

    fn call_client_input(
        &mut self,
        _key: i32,
        _pressed: bool,
        _ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        Ok(ModCallOutcome::default())
    }

    fn memory_bytes(&self) -> usize {
        self.vm.heap.approx_bytes()
    }
}

fn extract_hook_action(val: &PyVal) -> HookAction {
    match val {
        PyVal::Int(n) => HookAction::from_i32(*n as i32),
        _ => HookAction::Continue,
    }
}

fn merge_hook_action(cur: HookAction, next: HookAction) -> HookAction {
    if cur == HookAction::Supercede || next == HookAction::Supercede {
        HookAction::Supercede
    } else if cur == HookAction::Override || next == HookAction::Override {
        HookAction::Override
    } else if cur == HookAction::Handled || next == HookAction::Handled {
        HookAction::Handled
    } else {
        HookAction::Continue
    }
}
