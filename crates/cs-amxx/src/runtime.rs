//! AMX Mod X plugin runtime, task scheduler, forward/Ham/CSX/menu dispatcher, shared platform
//! services integration, and [`AmxxGame`] `GameDll` decorator.

use std::path::PathBuf;
use std::sync::Arc;

use cs_cvar::parser::parse_commands;
use cs_engine::{EdictId, EngineApi, FullPackCtx, GameDll};
use cs_mod_api::RpcValue;
use cs_mod_api::services::{MenuSelectOutcome, PluginRuntimeKind, SharedPlatformServices};
use cs_proto48::delta::{
    DeltaMarks, GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd, GoldSrcWeaponData,
};
use cs_proto48::usermsg::UserMsgRegistry;
use glam::Vec3;
use tracing::warn;

use crate::amxxfile::ParsedAmx;
use crate::natives::{
    AmxxCsHooks, AmxxExternalRpc, AmxxHostState, AmxxNativeDispatch, HAM_IGNORED, HAM_SUPERCEDE,
    HamFunction, MENU_EXIT, PLUGIN_CONTINUE, PLUGIN_HANDLED, PluginMeta, PureRpcDispatch,
    ScheduledTask, send_amxx_rendered_menu,
};
use crate::vm::{AmxError, ExecEntry, PawnVm};

/// Rolling per-plugin execution profiler (128-sample ring buffer for avg/p99 microseconds).
#[derive(Debug, Clone)]
pub struct AmxxPluginProfiler {
    samples_us: [u64; 128],
    sample_len: usize,
    sample_cursor: usize,
    max_us: u64,
    total_steps: u64,
    hook_calls: u64,
    fault_count: u64,
}

impl Default for AmxxPluginProfiler {
    fn default() -> Self {
        Self {
            samples_us: [0; 128],
            sample_len: 0,
            sample_cursor: 0,
            max_us: 0,
            total_steps: 0,
            hook_calls: 0,
            fault_count: 0,
        }
    }
}

impl AmxxPluginProfiler {
    pub fn record_call(&mut self, duration_us: u64, steps: u64) {
        self.samples_us[self.sample_cursor] = duration_us;
        self.sample_cursor = (self.sample_cursor + 1) % self.samples_us.len();
        if self.sample_len < self.samples_us.len() {
            self.sample_len += 1;
        }
        if duration_us > self.max_us {
            self.max_us = duration_us;
        }
        self.total_steps = self.total_steps.saturating_add(steps);
        self.hook_calls = self.hook_calls.saturating_add(1);
    }

    pub fn record_fault(&mut self) {
        self.fault_count = self.fault_count.saturating_add(1);
    }

    pub fn avg_us(&self) -> f64 {
        if self.sample_len == 0 {
            return 0.0;
        }
        let sum: u64 = self.samples_us[..self.sample_len].iter().copied().sum();
        sum as f64 / self.sample_len as f64
    }

    pub fn p99_us(&self) -> u64 {
        if self.sample_len == 0 {
            return 0;
        }
        let mut sorted: Vec<u64> = self.samples_us[..self.sample_len].to_vec();
        sorted.sort_unstable();
        let idx = ((self.sample_len * 99).div_ceil(100)).saturating_sub(1);
        sorted[idx.min(sorted.len() - 1)]
    }

    pub fn max_us(&self) -> u64 {
        self.max_us
    }

    pub fn total_steps(&self) -> u64 {
        self.total_steps
    }

    pub fn hook_calls(&self) -> u64 {
        self.hook_calls
    }

    pub fn fault_count(&self) -> u64 {
        self.fault_count
    }
}

/// Live per-plugin ResMon snapshot for `.amxx` plugins.
#[derive(Debug, Clone, PartialEq)]
pub struct AmxxPluginResmon {
    pub index: usize,
    pub name: String,
    pub title: String,
    pub version: String,
    pub status: PluginStatus,
    pub avg_us_per_tick: f64,
    pub p99_us_per_tick: u64,
    pub max_us: u64,
    pub fuel_used: u64,
    pub memory_bytes: usize,
    pub hook_calls: u64,
    pub fault_count: u64,
}

#[cfg(not(target_arch = "wasm32"))]
struct PerfTimer(std::time::Instant);

#[cfg(not(target_arch = "wasm32"))]
impl PerfTimer {
    fn start() -> Self {
        Self(std::time::Instant::now())
    }

    fn elapsed_us(&self) -> u64 {
        self.0.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
    }
}

#[cfg(target_arch = "wasm32")]
struct PerfTimer;

#[cfg(target_arch = "wasm32")]
impl PerfTimer {
    fn start() -> Self {
        Self
    }

    fn elapsed_us(&self) -> u64 {
        0
    }
}

/// Runtime status of a loaded AMX Mod X plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStatus {
    Running,
    Paused,
    Stopped,
    BadLoad(String),
}

/// One loaded `.amxx` plugin instance with its [`PawnVm`], lifecycle status, and profiler.
#[derive(Debug, Clone)]
pub struct AmxxPlugin {
    pub name: String,
    pub status: PluginStatus,
    pub vm: PawnVm,
    pub profiler: AmxxPluginProfiler,
}

impl AmxxPlugin {
    /// Parses a `.amxx` container (or raw `.amx` image) and instantiates its [`PawnVm`].
    pub fn from_amxx_bytes(name: impl Into<String>, bytes: &[u8]) -> Result<Self, AmxError> {
        let name = name.into();
        let parsed = ParsedAmx::parse_amxx(bytes)?;
        let vm = PawnVm::from_parsed(&parsed)?;
        Ok(Self {
            name,
            status: PluginStatus::Running,
            vm,
            profiler: AmxxPluginProfiler::default(),
        })
    }

    pub fn is_running(&self) -> bool {
        self.status == PluginStatus::Running
    }
}

/// Safely executes `func_name` on `slots[plugin_idx]` while passing the remaining `slots` as
/// peers so synchronous cross-plugin dynamic natives & multi-forwards work without deadlocks.
pub(crate) fn invoke_plugin_in_slots(
    slots: &mut [Option<AmxxPlugin>],
    state: &mut AmxxHostState,
    plugin_idx: usize,
    eng: &mut EngineApi<'_>,
    hooks: &mut dyn AmxxCsHooks,
    func_name: &str,
    args: &[i32],
) -> Result<Option<i32>, AmxError> {
    let Some(mut plugin) = slots.get_mut(plugin_idx).and_then(Option::take) else {
        return Ok(None);
    };
    let result = if !plugin.is_running() {
        Ok(None)
    } else if let Some(pub_idx) = plugin.vm.find_public(func_name) {
        let timer = PerfTimer::start();
        let mut dispatch = AmxxNativeDispatch {
            plugin_idx,
            state,
            eng,
            hooks,
            peers: slots,
        };
        let exec_res = plugin
            .vm
            .exec(ExecEntry::Public(pub_idx), args, &mut dispatch)
            .map(Some);
        plugin
            .profiler
            .record_call(timer.elapsed_us(), plugin.vm.last_steps());
        if exec_res.is_err() {
            plugin.profiler.record_fault();
        }
        exec_res
    } else {
        Ok(None)
    };
    slots[plugin_idx] = Some(plugin);
    result
}

/// Executes `func_name` on `slots[plugin_idx]` with [`PureRpcDispatch`] (no live `EngineApi`
/// borrow required), enabling synchronous WASM -> AMXX `rpc_call` inside `wasmi` imports.
pub(crate) fn invoke_plugin_pure_in_slots(
    slots: &mut [Option<AmxxPlugin>],
    state: &mut AmxxHostState,
    plugin_idx: usize,
    func_name: &str,
    args: &[i32],
) -> Result<Option<i32>, AmxError> {
    let Some(mut plugin) = slots.get_mut(plugin_idx).and_then(Option::take) else {
        return Ok(None);
    };
    let result = if !plugin.is_running() {
        Ok(None)
    } else if let Some(pub_idx) = plugin.vm.find_public(func_name) {
        let timer = PerfTimer::start();
        let mut dispatch = PureRpcDispatch {
            plugin_idx,
            state,
            peers: slots,
        };
        let exec_res = plugin
            .vm
            .exec(ExecEntry::Public(pub_idx), args, &mut dispatch)
            .map(Some);
        plugin
            .profiler
            .record_call(timer.elapsed_us(), plugin.vm.last_steps());
        if exec_res.is_err() {
            plugin.profiler.record_fault();
        }
        exec_res
    } else {
        Ok(None)
    };
    slots[plugin_idx] = Some(plugin);
    result
}

/// Coordinates all loaded `.amxx` plugins, timers, commands, forwards, Ham hooks, CSX stats,
/// paginated menus, localization, nvault, and cross-plugin RPC.
#[derive(Debug, Default)]
pub struct AmxxRuntime {
    pub plugins: Vec<AmxxPlugin>,
    pub state: AmxxHostState,
}

impl AmxxRuntime {
    pub fn new() -> Self {
        Self::with_services(SharedPlatformServices::new())
    }

    pub fn with_services(services: SharedPlatformServices) -> Self {
        Self {
            plugins: Vec::new(),
            state: AmxxHostState::with_services(services),
        }
    }

    pub fn services(&self) -> SharedPlatformServices {
        self.state.services.clone()
    }

    pub fn set_external_rpc(&mut self, rpc: Arc<dyn AmxxExternalRpc>) {
        self.state.external_rpc = Some(rpc);
    }

    pub fn add_lang_search_dir(&mut self, dir: impl Into<PathBuf>) {
        self.state.lang_search_dirs.push(dir.into());
    }

    /// Loads one `.amxx` plugin from raw file bytes and returns its plugin index.
    pub fn load_plugin(
        &mut self,
        name: impl Into<String>,
        bytes: &[u8],
    ) -> Result<usize, AmxError> {
        let name = name.into();
        let plugin = AmxxPlugin::from_amxx_bytes(name.clone(), bytes)?;
        let idx = self.plugins.len();
        self.plugins.push(plugin);
        self.state.plugin_metas.push(PluginMeta {
            file_name: name,
            ..PluginMeta::default()
        });
        Ok(idx)
    }

    /// Loads a batch of `(name, bytes)` plugins; malformed plugins are logged and skipped.
    pub fn load_plugins(&mut self, plugins: Vec<(String, Vec<u8>)>) {
        for (name, bytes) in plugins {
            if let Err(err) = self.load_plugin(name.clone(), &bytes) {
                warn!(plugin = %name, error = %err, "failed to load AMXX plugin");
            }
        }
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    fn matches_plugin_query(&self, idx: usize, query: &str) -> bool {
        let Some(p) = self.plugins.get(idx) else {
            return false;
        };
        if p.name.eq_ignore_ascii_case(query) {
            return true;
        }
        let stem = p
            .name
            .strip_suffix(".amxx")
            .or_else(|| p.name.strip_suffix(".amx"))
            .unwrap_or(&p.name);
        if stem.eq_ignore_ascii_case(query) {
            return true;
        }
        self.state
            .plugin_metas
            .get(idx)
            .is_some_and(|m| !m.title.is_empty() && m.title.eq_ignore_ascii_case(query))
    }

    /// Pauses a running `.amxx` plugin matched by filename, stem, or registered title.
    pub fn pause_plugin(&mut self, query: &str) -> bool {
        let Some(idx) = (0..self.plugins.len()).find(|&i| self.matches_plugin_query(i, query))
        else {
            return false;
        };
        if self.plugins[idx].status == PluginStatus::Running {
            self.plugins[idx].status = PluginStatus::Paused;
            true
        } else {
            false
        }
    }

    /// Resumes a paused `.amxx` plugin matched by filename, stem, or registered title.
    pub fn unpause_plugin(&mut self, query: &str) -> bool {
        let Some(idx) = (0..self.plugins.len()).find(|&i| self.matches_plugin_query(i, query))
        else {
            return false;
        };
        if self.plugins[idx].status == PluginStatus::Paused {
            self.plugins[idx].status = PluginStatus::Running;
            true
        } else {
            false
        }
    }

    /// Returns live per-plugin ResMon metrics for every loaded `.amxx` plugin.
    pub fn plugin_resmon(&self) -> Vec<AmxxPluginResmon> {
        self.plugins
            .iter()
            .enumerate()
            .map(|(index, plugin)| {
                let meta = self.state.plugin_metas.get(index);
                let title = meta
                    .map(|m| m.title.clone())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| plugin.name.clone());
                let version = meta
                    .map(|m| m.version.clone())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "amxx".to_string());
                AmxxPluginResmon {
                    index,
                    name: plugin.name.clone(),
                    title,
                    version,
                    status: plugin.status.clone(),
                    avg_us_per_tick: plugin.profiler.avg_us(),
                    p99_us_per_tick: plugin.profiler.p99_us(),
                    max_us: plugin.profiler.max_us(),
                    fuel_used: plugin.profiler.total_steps(),
                    memory_bytes: plugin.vm.memory_bytes(),
                    hook_calls: plugin.profiler.hook_calls(),
                    fault_count: plugin.profiler.fault_count(),
                }
            })
            .collect()
    }

    /// Populates standard AMXX public variables (`MaxClients`, `MapName`) and dispatches
    /// `plugin_precache` while the engine precache tables are still unlocked.
    pub fn on_server_activate_precache(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
    ) {
        let max_clients = i32::from(eng.max_clients());
        let map_name = eng.map_name().to_string();
        for plugin in &mut self.plugins {
            if !plugin.is_running() {
                continue;
            }
            if let Some(addr) = plugin.vm.find_pubvar("MaxClients") {
                let _ = plugin.vm.write_cell(addr, max_clients);
            }
            if let Some(addr) = plugin.vm.find_pubvar("MapName") {
                let _ = plugin.vm.set_amx_string(addr, &map_name, 63);
            }
        }
        self.dispatch_forward_all(eng, hooks, "plugin_precache", &[]);
    }

    /// Dispatches `plugin_init` and `plugin_cfg` after map entities & game activation.
    pub fn on_server_activate_init(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
    ) {
        self.dispatch_forward_all(eng, hooks, "plugin_init", &[]);
        self.dispatch_forward_all(eng, hooks, "plugin_cfg", &[]);
    }

    /// Dispatches `plugin_end` and clears per-map registrations and scheduled tasks.
    pub fn on_server_deactivate(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
    ) {
        self.dispatch_forward_all(eng, hooks, "plugin_end", &[]);
        self.state.tasks.clear();
        self.state.clcmds.clear();
        self.state.concmds.clear();
        self.state.srvcmds.clear();
        self.state.events.clear();
        self.state.messages.clear();
        self.state.ham_hooks.clear();
        self.state.ham_param_overrides.clear();
        self.state.pending_server_cmds.clear();
        self.state.connected_slots.fill(false);
    }

    /// Executes a public function by name on one specific plugin index.
    pub fn call_public(
        &mut self,
        plugin_idx: usize,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        func_name: &str,
        args: &[i32],
    ) -> Result<Option<i32>, AmxError> {
        if plugin_idx >= self.plugins.len() {
            return Ok(None);
        }
        let mut slots: Vec<Option<AmxxPlugin>> = self.plugins.drain(..).map(Some).collect();
        let res = invoke_plugin_in_slots(
            &mut slots,
            &mut self.state,
            plugin_idx,
            eng,
            hooks,
            func_name,
            args,
        );
        self.plugins = slots.into_iter().flatten().collect();
        res
    }

    /// Synchronously invokes an AMXX `register_native` handler on `plugin_idx` from an external
    /// polyglot caller (e.g. a `.wasm` or `.py` plugin via `RpcBus`).
    pub fn invoke_registered_native_from_rpc(
        &mut self,
        plugin_idx: usize,
        callback: &str,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        args: &[RpcValue],
    ) -> Option<RpcValue> {
        let original_args = args.to_vec();
        {
            let mut svc = self.state.services.lock();
            svc.rpc.push_call_frame(args.to_vec());
        }
        let rc = self
            .call_public(plugin_idx, eng, hooks, callback, &[-1, args.len() as i32])
            .ok()
            .flatten()?;
        let popped = {
            let mut svc = self.state.services.lock();
            svc.rpc.pop_call_frame()
        };
        // If the handler mutated a string param via `set_string`, return that string if changed!
        if let Some(frame) = popped {
            for (i, val) in frame.into_iter().enumerate() {
                if let RpcValue::Str(s) = val
                    && original_args.get(i) != Some(&RpcValue::Str(s.clone()))
                {
                    return Some(RpcValue::Str(s));
                }
            }
        }
        Some(RpcValue::Int(rc))
    }

    /// Synchronously invokes an AMXX `register_native` handler without requiring `&mut EngineApi`
    /// (used by `cs-mod-host` when a `.wasm` plugin calls `rpc_call` into an `.amxx` plugin).
    pub fn invoke_registered_native_pure(
        &mut self,
        plugin_idx: usize,
        callback: &str,
        args: &[RpcValue],
    ) -> Option<RpcValue> {
        if plugin_idx >= self.plugins.len() {
            return None;
        }
        let original_args = args.to_vec();
        {
            let mut svc = self.state.services.lock();
            svc.rpc.push_call_frame(args.to_vec());
        }
        let mut slots: Vec<Option<AmxxPlugin>> = self.plugins.drain(..).map(Some).collect();
        let rc = invoke_plugin_pure_in_slots(
            &mut slots,
            &mut self.state,
            plugin_idx,
            callback,
            &[-1, args.len() as i32],
        )
        .ok()
        .flatten();
        self.plugins = slots.into_iter().flatten().collect();
        let popped = {
            let mut svc = self.state.services.lock();
            svc.rpc.pop_call_frame()
        };
        let rc = rc?;
        if let Some(frame) = popped {
            for (i, val) in frame.into_iter().enumerate() {
                if let RpcValue::Str(s) = val
                    && original_args.get(i) != Some(&RpcValue::Str(s.clone()))
                {
                    return Some(RpcValue::Str(s));
                }
            }
        }
        Some(RpcValue::Int(rc))
    }

    /// Delivers a polyglot RPC event (e.g. emitted by a `.wasm` mod via `rpc_emit_event`) to all
    /// running `.amxx` plugins exposing `public <event_name>(...)`.
    pub fn dispatch_rpc_event(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        event_name: &str,
        args: &[RpcValue],
    ) {
        let count = self.plugins.len();
        for idx in 0..count {
            let Some(plugin) = self.plugins.get_mut(idx) else {
                continue;
            };
            if !plugin.is_running() || plugin.vm.find_public(event_name).is_none() {
                continue;
            }
            let mut pawn_args = Vec::with_capacity(args.len());
            for v in args {
                match v {
                    RpcValue::Int(n) => pawn_args.push(*n),
                    RpcValue::Float(f) => pawn_args.push(f.to_bits() as i32),
                    RpcValue::Str(s) => {
                        if let Ok(addr) = plugin.vm.allot_string(s) {
                            pawn_args.push(addr);
                        } else {
                            pawn_args.push(0);
                        }
                    }
                    RpcValue::Bytes(b) => {
                        let s = String::from_utf8_lossy(b);
                        if let Ok(addr) = plugin.vm.allot_string(&s) {
                            pawn_args.push(addr);
                        } else {
                            pawn_args.push(0);
                        }
                    }
                }
            }
            if let Err(err) = self.call_public(idx, eng, hooks, event_name, &pawn_args) {
                warn!(
                    plugin = %self.plugins[idx].name,
                    event = event_name,
                    error = %err,
                    "AMXX RPC event callback error"
                );
            }
        }
    }

    /// Dispatches `client_connect(id)`; returns `Err(...)` if a plugin returns `>= PLUGIN_HANDLED`.
    pub fn on_client_connect(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        slot: u8,
    ) -> Result<(), String> {
        let player_id = i32::from(slot) + 1;
        let ret = self.dispatch_forward_stop(eng, hooks, "client_connect", &[player_id]);
        if ret >= PLUGIN_HANDLED {
            Err("Rejected by AMX Mod X plugin".to_string())
        } else {
            Ok(())
        }
    }

    /// Marks `slot` connected and dispatches `client_putinserver(id)`.
    pub fn on_client_put_in_server(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        slot: u8,
    ) {
        if let Some(flag) = self.state.connected_slots.get_mut(usize::from(slot)) {
            *flag = true;
        }
        let player_id = i32::from(slot) + 1;
        self.dispatch_forward_all(eng, hooks, "client_putinserver", &[player_id]);
    }

    /// Dispatches `client_disconnect(id)` and marks `slot` disconnected.
    pub fn on_client_disconnect(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        slot: u8,
    ) {
        let player_id = i32::from(slot) + 1;
        self.dispatch_forward_all(eng, hooks, "client_disconnect", &[player_id]);
        if let Some(flag) = self.state.connected_slots.get_mut(usize::from(slot)) {
            *flag = false;
        }
    }

    /// Dispatches `client_infochanged(id)`.
    pub fn on_client_userinfo_changed(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        slot: u8,
    ) {
        let player_id = i32::from(slot) + 1;
        self.dispatch_forward_all(eng, hooks, "client_infochanged", &[player_id]);
    }

    /// Invokes an AMXX `newmenus.inc` handler `public handler(id, menu, item)` directly (used by
    /// unified `ModdedGame` after resolving `MenuManager::handle_menuselect`).
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_amxx_menu_callback(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        plugin_idx: usize,
        callback_name: &str,
        slot: u8,
        menu_id: u32,
        item_index: i32,
    ) {
        let player_id = i32::from(slot) + 1;
        let _ = self.call_public(
            plugin_idx,
            eng,
            hooks,
            callback_name,
            &[player_id, menu_id as i32, item_index],
        );
    }

    /// Calls `func_name` across all running plugins (`ET_IGNORE`: runs all plugins).
    pub fn dispatch_forward_all(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        func_name: &str,
        args: &[i32],
    ) -> i32 {
        let mut highest = PLUGIN_CONTINUE;
        for idx in 0..self.plugins.len() {
            match self.call_public(idx, eng, hooks, func_name, args) {
                Ok(Some(ret)) => highest = highest.max(ret),
                Ok(None) => {}
                Err(err) => {
                    warn!(
                        plugin = %self.plugins[idx].name,
                        func = func_name,
                        error = %err,
                        "AMXX forward error"
                    );
                }
            }
        }
        highest
    }

    /// Calls `func_name` across running plugins, stopping if any returns `>= PLUGIN_HANDLED`.
    pub fn dispatch_forward_stop(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        func_name: &str,
        args: &[i32],
    ) -> i32 {
        let mut result = PLUGIN_CONTINUE;
        for idx in 0..self.plugins.len() {
            match self.call_public(idx, eng, hooks, func_name, args) {
                Ok(Some(ret)) if ret >= PLUGIN_HANDLED => {
                    return ret;
                }
                Ok(Some(ret)) => result = result.max(ret),
                Ok(None) => {}
                Err(err) => {
                    warn!(
                        plugin = %self.plugins[idx].name,
                        func = func_name,
                        error = %err,
                        "AMXX forward error"
                    );
                }
            }
        }
        result
    }

    // --- CSX Forwards (`csx.inc`) ---

    /// Dispatches `client_damage(attacker, victim, damage, wpnindex, hitplace, TA)` and updates
    /// shared `"csx"` leaderboard stats.
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_client_damage(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        attacker_slot: Option<u8>,
        victim_slot: u8,
        damage: i32,
        weapon: i32,
        hitplace: i32,
        team_attack: bool,
    ) {
        self.state
            .record_csx_damage(eng, attacker_slot, damage, hitplace);
        let attacker = attacker_slot.map_or(0, |s| i32::from(s) + 1);
        let victim = i32::from(victim_slot) + 1;
        let args = [
            attacker,
            victim,
            damage,
            weapon,
            hitplace,
            i32::from(team_attack),
        ];
        self.dispatch_forward_all(eng, hooks, "client_damage", &args);
    }

    /// Dispatches `client_death(killer, victim, wpnindex, hitplace, TK)` and updates shared `"csx"`
    /// leaderboard stats.
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_client_death(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        killer_slot: Option<u8>,
        victim_slot: u8,
        weapon: i32,
        hitplace: i32,
        team_kill: bool,
    ) {
        self.state
            .record_csx_death(eng, killer_slot, victim_slot, hitplace, team_kill);
        let killer = killer_slot.map_or(0, |s| i32::from(s) + 1);
        let victim = i32::from(victim_slot) + 1;
        let args = [killer, victim, weapon, hitplace, i32::from(team_kill)];
        self.dispatch_forward_all(eng, hooks, "client_death", &args);
    }

    pub fn dispatch_bomb_planting(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        planter: u8,
    ) {
        self.dispatch_forward_all(eng, hooks, "bomb_planting", &[i32::from(planter) + 1]);
    }

    pub fn dispatch_bomb_planted(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        planter: u8,
    ) {
        self.dispatch_forward_all(eng, hooks, "bomb_planted", &[i32::from(planter) + 1]);
    }

    pub fn dispatch_bomb_explode(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        planter: Option<u8>,
        defuser: Option<u8>,
    ) {
        let p = planter.map_or(0, |s| i32::from(s) + 1);
        let d = defuser.map_or(0, |s| i32::from(s) + 1);
        self.dispatch_forward_all(eng, hooks, "bomb_explode", &[p, d]);
    }

    pub fn dispatch_bomb_defusing(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        defuser: u8,
    ) {
        self.dispatch_forward_all(eng, hooks, "bomb_defusing", &[i32::from(defuser) + 1]);
    }

    pub fn dispatch_bomb_defused(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        defuser: u8,
    ) {
        self.dispatch_forward_all(eng, hooks, "bomb_defused", &[i32::from(defuser) + 1]);
    }

    pub fn dispatch_grenade_throw(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        slot: u8,
        grenade_ent: u16,
        weapon_id: u8,
    ) {
        self.dispatch_forward_all(
            eng,
            hooks,
            "grenade_throw",
            &[i32::from(slot) + 1, i32::from(grenade_ent), i32::from(weapon_id)],
        );
    }

    // --- Hamsandwich Forwards ---

    /// Dispatches registered `Ham_Spawn` pre/post hooks for `classname`.
    pub fn dispatch_ham_spawn(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        edict_index: u16,
        classname: &str,
        post: bool,
    ) -> i32 {
        self.dispatch_ham(
            eng,
            hooks,
            HamFunction::Spawn,
            classname,
            post,
            &[i32::from(edict_index)],
        )
    }

    /// Dispatches registered `Ham_Killed` pre/post hooks for `classname`.
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_ham_killed(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        victim: u16,
        classname: &str,
        attacker: u16,
        should_gib: i32,
        post: bool,
    ) -> i32 {
        self.dispatch_ham(
            eng,
            hooks,
            HamFunction::Killed,
            classname,
            post,
            &[i32::from(victim), i32::from(attacker), should_gib],
        )
    }

    /// Dispatches registered `Ham_TakeDamage` pre/post hooks for `classname`.
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_ham_take_damage(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        victim: u16,
        classname: &str,
        inflictor: u16,
        attacker: u16,
        damage: f32,
        damage_bits: i32,
        post: bool,
    ) -> i32 {
        self.dispatch_ham_take_damage_mut(
            eng,
            hooks,
            victim,
            classname,
            inflictor,
            attacker,
            damage,
            damage_bits,
            post,
        )
        .0
    }

    /// Dispatches `Ham_TakeDamage` and returns `(ham_result, overridden_damage, overridden_bits)`
    /// populated by `SetHamParamFloat(4, ...)` / `SetHamParamInteger(5, ...)`.
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_ham_take_damage_mut(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        victim: u16,
        classname: &str,
        inflictor: u16,
        attacker: u16,
        damage: f32,
        damage_bits: i32,
        post: bool,
    ) -> (i32, Option<f32>, Option<i32>) {
        self.state.ham_param_overrides.clear();
        let ret = self.dispatch_ham(
            eng,
            hooks,
            HamFunction::TakeDamage,
            classname,
            post,
            &[
                i32::from(victim),
                i32::from(inflictor),
                i32::from(attacker),
                damage.to_bits() as i32,
                damage_bits,
            ],
        );
        let overrides = std::mem::take(&mut self.state.ham_param_overrides);
        let new_dmg = overrides.get(&4).map(|&bits| f32::from_bits(bits as u32));
        let new_bits = overrides.get(&5).copied();
        (ret, new_dmg, new_bits)
    }

    pub fn dispatch_ham_touch(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        toucher: u16,
        classname: &str,
        other: u16,
        post: bool,
    ) -> i32 {
        self.dispatch_ham(
            eng,
            hooks,
            HamFunction::Touch,
            classname,
            post,
            &[i32::from(toucher), i32::from(other)],
        )
    }

    pub fn dispatch_ham_item_deploy(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        ent_index: u16,
        classname: &str,
        post: bool,
    ) -> i32 {
        self.dispatch_ham(
            eng,
            hooks,
            HamFunction::ItemDeploy,
            classname,
            post,
            &[i32::from(ent_index)],
        )
    }

    pub fn dispatch_ham_weapon_primary_attack(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        ent_index: u16,
        classname: &str,
        post: bool,
    ) -> i32 {
        self.dispatch_ham(
            eng,
            hooks,
            HamFunction::WeaponPrimaryAttack,
            classname,
            post,
            &[i32::from(ent_index)],
        )
    }

    pub fn dispatch_ham_player_jump(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        ent_index: u16,
        post: bool,
    ) -> i32 {
        self.dispatch_ham(
            eng,
            hooks,
            HamFunction::PlayerJump,
            "player",
            post,
            &[i32::from(ent_index)],
        )
    }

    fn dispatch_ham(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        function: HamFunction,
        classname: &str,
        post: bool,
        args: &[i32],
    ) -> i32 {
        let matching: Vec<(usize, String)> = self
            .state
            .ham_hooks
            .iter()
            .filter(|h| h.function == function && h.post == post && h.classname == classname)
            .map(|h| (h.plugin_idx, h.callback.clone()))
            .collect();

        let mut highest = HAM_IGNORED;
        for (plugin_idx, callback) in matching {
            match self.call_public(plugin_idx, eng, hooks, &callback, args) {
                Ok(Some(ret)) => {
                    highest = highest.max(ret);
                    if !post && ret >= HAM_SUPERCEDE {
                        return HAM_SUPERCEDE;
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    warn!(plugin_idx, callback = %callback, error = %err, "Ham callback error");
                }
            }
        }
        highest
    }

    /// Dispatches registered `register_event` hooks matching `event_name`.
    pub fn dispatch_event(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        event_name: &str,
        player_slot: Option<u8>,
    ) {
        let matching: Vec<(usize, String)> = self
            .state
            .events
            .iter()
            .filter(|e| e.event_name == event_name)
            .map(|e| (e.plugin_idx, e.callback.clone()))
            .collect();
        let arg = player_slot.map_or(0, |s| i32::from(s) + 1);
        for (plugin_idx, callback) in matching {
            if let Err(err) = self.call_public(plugin_idx, eng, hooks, &callback, &[arg]) {
                warn!(plugin_idx, callback = %callback, error = %err, "event callback error");
            }
        }
    }

    /// Dispatches registered `register_message` hooks matching `msg_id`.
    pub fn dispatch_message(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        msg_id: i32,
        msg_dest: i32,
        ent_index: i32,
    ) -> i32 {
        let matching: Vec<(usize, String)> = self
            .state
            .messages
            .iter()
            .filter(|m| m.msg_id == msg_id)
            .map(|m| (m.plugin_idx, m.callback.clone()))
            .collect();
        let mut highest = PLUGIN_CONTINUE;
        for (plugin_idx, callback) in matching {
            match self.call_public(
                plugin_idx,
                eng,
                hooks,
                &callback,
                &[msg_id, msg_dest, ent_index],
            ) {
                Ok(Some(ret)) => {
                    highest = highest.max(ret);
                    if ret >= PLUGIN_HANDLED {
                        return ret;
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    warn!(plugin_idx, callback = %callback, error = %err, "message hook error");
                }
            }
        }
        highest
    }

    /// Ticks due `set_task` timers against `eng.time()`.
    pub fn tick_tasks(&mut self, eng: &mut EngineApi<'_>, hooks: &mut dyn AmxxCsHooks) {
        let now = eng.time();
        let mut due_indices = Vec::new();
        for task in &self.state.tasks {
            if task.next_run <= now {
                due_indices.push(task.handle);
            }
        }
        if due_indices.is_empty() {
            return;
        }

        for handle in due_indices {
            let Some(pos) = self.state.tasks.iter().position(|t| t.handle == handle) else {
                continue;
            };
            let task: ScheduledTask = self.state.tasks[pos].clone();

            match task.remaining_repeats {
                None => {
                    self.state.tasks[pos].next_run = now + task.interval;
                }
                Some(n) if n > 1 => {
                    self.state.tasks[pos].remaining_repeats = Some(n - 1);
                    self.state.tasks[pos].next_run = now + task.interval;
                }
                _ => {
                    self.state.tasks.remove(pos);
                }
            }

            if let Some(plugin) = self.plugins.get_mut(task.plugin_idx)
                && plugin.is_running()
                && plugin.vm.find_public(&task.callback).is_some()
            {
                let args: Vec<i32> = if task.param_data.is_empty() {
                    vec![task.task_id]
                } else {
                    match plugin.vm.allot_array(&task.param_data) {
                        Ok(addr) => vec![addr, task.task_id],
                        Err(err) => {
                            warn!(error = %err, "failed to allot set_task param array");
                            continue;
                        }
                    }
                };
                if let Err(err) =
                    self.call_public(task.plugin_idx, eng, hooks, &task.callback, &args)
                {
                    warn!(
                        plugin_idx = task.plugin_idx,
                        callback = %task.callback,
                        error = %err,
                        "AMXX task callback error"
                    );
                }
            }
        }
    }

    /// Routes `menuselect <choice>` for standalone `AmxxGame` via the shared `MenuManager`.
    pub fn dispatch_menuselect(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        slot: u8,
        choice: i32,
    ) -> bool {
        let outcome = {
            let mut svc = self.state.services.lock();
            svc.menus.handle_menuselect(slot, choice)
        };
        let player_id = i32::from(slot) + 1;
        match outcome {
            MenuSelectOutcome::NotOwned => false,
            MenuSelectOutcome::PageTurned(rendered) => {
                send_amxx_rendered_menu(eng, &rendered);
                true
            }
            MenuSelectOutcome::Ignored => true,
            MenuSelectOutcome::Selected {
                owner_runtime,
                owner_index,
                menu_id,
                callback_name,
                item_index,
                ..
            } => {
                if owner_runtime == PluginRuntimeKind::Amxx {
                    let _ = self.call_public(
                        owner_index,
                        eng,
                        hooks,
                        &callback_name,
                        &[player_id, menu_id as i32, item_index as i32],
                    );
                }
                true
            }
            MenuSelectOutcome::Exited {
                owner_runtime,
                owner_index,
                menu_id,
                callback_name,
                ..
            } => {
                if owner_runtime == PluginRuntimeKind::Amxx {
                    let _ = self.call_public(
                        owner_index,
                        eng,
                        hooks,
                        &callback_name,
                        &[player_id, menu_id as i32, MENU_EXIT],
                    );
                }
                true
            }
        }
    }

    /// Handles a client command (`menuselect`, `register_clcmd`, `register_concmd`, then
    /// `client_command` forward). Returns `true` if a plugin handled the command.
    pub fn dispatch_client_command(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        slot: u8,
        argv: &[String],
    ) -> bool {
        let Some(cmd_name) = argv.first().map(|s| s.to_ascii_lowercase()) else {
            return false;
        };
        if cmd_name == "menuselect"
            && let Some(Ok(choice)) = argv.get(1).map(|s| s.parse::<i32>())
            && self.dispatch_menuselect(eng, hooks, slot, choice)
        {
            return true;
        }
        self.dispatch_client_command_after_menu(eng, hooks, slot, argv)
    }

    /// Handles a client command after `MenuManager` has already been consulted (`register_clcmd`,
    /// `register_concmd`, then `client_command` forward).
    pub fn dispatch_client_command_after_menu(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        slot: u8,
        argv: &[String],
    ) -> bool {
        let Some(cmd_name) = argv.first().map(|s| s.to_ascii_lowercase()) else {
            return false;
        };

        self.state.current_argv = argv.to_vec();
        let player_id = i32::from(slot) + 1;
        let user_flags = self.state.effective_user_flags(slot);

        let matching: Vec<(usize, String, i32)> = self
            .state
            .clcmds
            .iter()
            .chain(self.state.concmds.iter())
            .filter(|c| c.command == cmd_name)
            .map(|c| (c.plugin_idx, c.callback.clone(), c.flags))
            .collect();

        let mut handled = false;
        for (plugin_idx, callback, req_flags) in matching {
            let level = if req_flags == -1 { 0 } else { req_flags };
            let cid = 0;
            match self.call_public(
                plugin_idx,
                eng,
                hooks,
                &callback,
                &[player_id, level, cid],
            ) {
                Ok(Some(ret)) if ret >= PLUGIN_HANDLED => {
                    handled = true;
                    break;
                }
                Ok(_) => {}
                Err(err) => {
                    warn!(plugin_idx, callback = %callback, error = %err, "clcmd callback error");
                }
            }
            let _ = user_flags;
        }

        if !handled {
            let fwd = self.dispatch_forward_stop(eng, hooks, "client_command", &[player_id]);
            if fwd >= PLUGIN_HANDLED {
                handled = true;
            }
        }

        self.state.current_argv.clear();
        handled
    }

    /// Handles a server console command (`amxx` built-ins, `register_srvcmd`, `register_concmd`,
    /// or plugin-registered CVars).
    pub fn dispatch_server_command(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
        argv: &[String],
    ) -> Option<String> {
        let cmd = argv.first()?.to_ascii_lowercase();

        if cmd == "amxx" {
            return Some(self.handle_amxx_meta_cmd(eng, &argv[1..]));
        }

        self.state.current_argv = argv.to_vec();
        let matching: Vec<(usize, String, i32)> = self
            .state
            .srvcmds
            .iter()
            .chain(self.state.concmds.iter())
            .filter(|c| c.command == cmd)
            .map(|c| (c.plugin_idx, c.callback.clone(), c.flags))
            .collect();

        let mut handled = false;
        let print_start = self.state.server_prints.len();
        for (plugin_idx, callback, req_flags) in matching {
            let level = if req_flags == -1 { 0 } else { req_flags };
            match self.call_public(plugin_idx, eng, hooks, &callback, &[0, level, 0]) {
                Ok(Some(ret)) if ret >= PLUGIN_HANDLED => {
                    handled = true;
                    break;
                }
                Ok(_) => {}
                Err(err) => {
                    warn!(plugin_idx, callback = %callback, error = %err, "srvcmd callback error");
                }
            }
        }
        self.state.current_argv.clear();

        if handled {
            let captured = self.state.server_prints[print_start..].join("\n");
            return Some(if captured.is_empty() {
                String::new()
            } else {
                format!("{captured}\n")
            });
        }

        if let Some(&idx) = self.state.cvar_by_name.get(&cmd)
            && self.state.cvars[idx].local_value.is_some()
        {
            if argv.len() >= 2 {
                self.state.cvars[idx].local_value = Some(argv[1].clone());
                return Some(String::new());
            } else if let Some(val) = &self.state.cvars[idx].local_value {
                return Some(format!("\"{cmd}\" is \"{val}\"\n"));
            }
        }

        None
    }

    /// Drains queued `server_cmd()` strings, applying CVar updates and running registered
    /// server commands; returns any remaining unhandled commands for the inner game.
    pub fn drain_pending_server_cmds(
        &mut self,
        eng: &mut EngineApi<'_>,
        hooks: &mut dyn AmxxCsHooks,
    ) -> Vec<Vec<String>> {
        let mut unhandled = Vec::new();
        let mut budget = 64usize;
        while !self.state.pending_server_cmds.is_empty() && budget > 0 {
            let batch = std::mem::take(&mut self.state.pending_server_cmds);
            for line in batch {
                for tokens in parse_commands(&line) {
                    if budget == 0 {
                        break;
                    }
                    budget -= 1;
                    if tokens.is_empty() {
                        continue;
                    }
                    if self.dispatch_server_command(eng, hooks, &tokens).is_some() {
                        continue;
                    }
                    if tokens.len() >= 2 && eng.cvar_str(&tokens[0]).is_some() {
                        eng.set_cvar(&tokens[0], &tokens[1]);
                        continue;
                    }
                    if tokens[0].eq_ignore_ascii_case("echo") {
                        self.state.server_prints.push(tokens[1..].join(" "));
                        continue;
                    }
                    unhandled.push(tokens);
                }
            }
        }
        unhandled
    }

    fn handle_amxx_meta_cmd(&self, eng: &EngineApi<'_>, args: &[String]) -> String {
        let sub = args.first().map(|s| s.to_ascii_lowercase());
        match sub.as_deref() {
            Some("plugins") => {
                let mut out = String::from("Currently loaded plugins:\n");
                for (i, p) in self.plugins.iter().enumerate() {
                    let meta = &self.state.plugin_metas[i];
                    let title = if meta.title.is_empty() {
                        &p.name
                    } else {
                        &meta.title
                    };
                    let ver = if meta.version.is_empty() {
                        "unknown"
                    } else {
                        &meta.version
                    };
                    let author = if meta.author.is_empty() {
                        "unknown"
                    } else {
                        &meta.author
                    };
                    out.push_str(&format!(
                        " [{:2}] {:<24} {:<8} {:<16} {:<18} {:?}\n",
                        i + 1,
                        title,
                        ver,
                        author,
                        p.name,
                        p.status
                    ));
                }
                out.push_str(&format!("{} plugins\n", self.plugins.len()));
                out
            }
            Some("cvars") => {
                let mut out = String::from("AMXX registered cvars:\n");
                for c in &self.state.cvars {
                    let val = c
                        .local_value
                        .clone()
                        .or_else(|| eng.cvar_str(&c.name))
                        .unwrap_or_default();
                    out.push_str(&format!("  {} = \"{}\"\n", c.name, val));
                }
                out
            }
            _ => String::from(
                "AMX Mod X (cs-amxx pure-Rust Pawn 3.2 host)\nUsage: amxx <plugins|cvars>\n",
            ),
        }
    }
}

/// [`GameDll`] decorator wrapping any inner `G: GameDll + AmxxCsHooks` with an
/// [`AmxxRuntime`].
pub struct AmxxGame<G: GameDll + AmxxCsHooks> {
    inner: G,
    runtime: AmxxRuntime,
}

impl<G: GameDll + AmxxCsHooks> AmxxGame<G> {
    /// Creates an [`AmxxGame`] wrapping `inner` and loading all `(name, bytes)` `.amxx` plugins.
    pub fn new(inner: G, plugins: Vec<(String, Vec<u8>)>) -> Self {
        let mut runtime = AmxxRuntime::new();
        runtime.load_plugins(plugins);
        Self { inner, runtime }
    }

    /// Creates an [`AmxxGame`] with a pre-populated [`AmxxRuntime`].
    pub fn with_runtime(inner: G, runtime: AmxxRuntime) -> Self {
        Self { inner, runtime }
    }

    pub fn inner(&self) -> &G {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut G {
        &mut self.inner
    }

    pub fn runtime(&self) -> &AmxxRuntime {
        &self.runtime
    }

    pub fn runtime_mut(&mut self) -> &mut AmxxRuntime {
        &mut self.runtime
    }

    pub fn parts_mut(&mut self) -> (&mut AmxxRuntime, &mut G) {
        (&mut self.runtime, &mut self.inner)
    }

    pub fn into_inner(self) -> G {
        self.inner
    }

    /// Helper to dispatch `client_damage` from game damage handlers.
    #[allow(clippy::too_many_arguments)]
    pub fn notify_client_damage(
        &mut self,
        eng: &mut EngineApi<'_>,
        attacker_slot: Option<u8>,
        victim_slot: u8,
        damage: i32,
        weapon: i32,
        hitplace: i32,
        team_attack: bool,
    ) {
        self.runtime.dispatch_client_damage(
            eng,
            &mut self.inner,
            attacker_slot,
            victim_slot,
            damage,
            weapon,
            hitplace,
            team_attack,
        );
    }

    /// Helper to dispatch `client_death` from game death handlers.
    #[allow(clippy::too_many_arguments)]
    pub fn notify_client_death(
        &mut self,
        eng: &mut EngineApi<'_>,
        killer_slot: Option<u8>,
        victim_slot: u8,
        weapon: i32,
        hitplace: i32,
        team_kill: bool,
    ) {
        self.runtime.dispatch_client_death(
            eng,
            &mut self.inner,
            killer_slot,
            victim_slot,
            weapon,
            hitplace,
            team_kill,
        );
    }

    /// Helper to dispatch `Ham_TakeDamage` pre/post hooks.
    #[allow(clippy::too_many_arguments)]
    pub fn notify_ham_take_damage(
        &mut self,
        eng: &mut EngineApi<'_>,
        victim: u16,
        classname: &str,
        inflictor: u16,
        attacker: u16,
        damage: f32,
        damage_bits: i32,
        post: bool,
    ) -> i32 {
        self.runtime.dispatch_ham_take_damage(
            eng,
            &mut self.inner,
            victim,
            classname,
            inflictor,
            attacker,
            damage,
            damage_bits,
            post,
        )
    }

    /// Helper to dispatch `Ham_Killed` pre/post hooks.
    #[allow(clippy::too_many_arguments)]
    pub fn notify_ham_killed(
        &mut self,
        eng: &mut EngineApi<'_>,
        victim: u16,
        classname: &str,
        attacker: u16,
        should_gib: i32,
        post: bool,
    ) -> i32 {
        self.runtime.dispatch_ham_killed(
            eng,
            &mut self.inner,
            victim,
            classname,
            attacker,
            should_gib,
            post,
        )
    }
}

impl<G: GameDll + AmxxCsHooks> GameDll for AmxxGame<G> {
    fn usermsgs(&self) -> UserMsgRegistry {
        self.inner.usermsgs()
    }

    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        self.runtime
            .on_server_activate_precache(eng, &mut self.inner);
        self.inner.server_activate(eng);
        self.runtime.on_server_activate_init(eng, &mut self.inner);
        let unhandled = self
            .runtime
            .drain_pending_server_cmds(eng, &mut self.inner);
        for argv in unhandled {
            let _ = self.inner.server_command(eng, &argv);
        }
    }

    fn server_deactivate(&mut self, eng: &mut EngineApi<'_>) {
        self.runtime.on_server_deactivate(eng, &mut self.inner);
        self.inner.server_deactivate(eng);
    }

    fn spawn_entity(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        classname: &str,
        keys: &[(String, String)],
    ) -> bool {
        let pre = self
            .runtime
            .dispatch_ham_spawn(eng, &mut self.inner, id.index, classname, false);
        if pre >= HAM_SUPERCEDE {
            return false;
        }
        let keep = self.inner.spawn_entity(eng, id, classname, keys);
        if keep {
            let _ = self
                .runtime
                .dispatch_ham_spawn(eng, &mut self.inner, id.index, classname, true);
        }
        keep
    }

    fn client_connect(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Result<(), String> {
        let player_id = i32::from(slot) + 1;
        let ret =
            self.runtime
                .dispatch_forward_stop(eng, &mut self.inner, "client_connect", &[player_id]);
        if ret >= PLUGIN_HANDLED {
            return Err("Rejected by AMX Mod X plugin".to_string());
        }
        self.inner.client_connect(eng, slot)
    }

    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        if let Some(flag) = self.runtime.state.connected_slots.get_mut(usize::from(slot)) {
            *flag = true;
        }
        self.inner.client_put_in_server(eng, slot);
        let player_id = i32::from(slot) + 1;
        self.runtime.dispatch_forward_all(
            eng,
            &mut self.inner,
            "client_putinserver",
            &[player_id],
        );
    }

    fn client_disconnect(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        let player_id = i32::from(slot) + 1;
        self.runtime
            .dispatch_forward_all(eng, &mut self.inner, "client_disconnect", &[player_id]);
        if let Some(flag) = self.runtime.state.connected_slots.get_mut(usize::from(slot)) {
            *flag = false;
        }
        {
            let mut svc = self.runtime.state.services.lock();
            svc.menus.clear_slot(slot);
        }
        self.inner.client_disconnect(eng, slot);
    }

    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
        if !self
            .runtime
            .dispatch_client_command(eng, &mut self.inner, slot, argv)
        {
            self.inner.client_command(eng, slot, argv);
        }
    }

    fn client_userinfo_changed(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        let player_id = i32::from(slot) + 1;
        self.runtime.dispatch_forward_all(
            eng,
            &mut self.inner,
            "client_infochanged",
            &[player_id],
        );
        self.inner.client_userinfo_changed(eng, slot);
    }

    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        random_seed: u32,
    ) {
        self.inner.run_usercmd(eng, slot, cmd, random_seed);
    }

    fn post_usercmd(&mut self, eng: &mut EngineApi<'_>, slot: u8, cmd: &GoldSrcUserCmd) {
        self.inner.post_usercmd(eng, slot, cmd);
    }

    fn start_frame(&mut self, eng: &mut EngineApi<'_>, frametime: f32) {
        self.runtime.tick_tasks(eng, &mut self.inner);
        let unhandled = self
            .runtime
            .drain_pending_server_cmds(eng, &mut self.inner);
        for argv in unhandled {
            let _ = self.inner.server_command(eng, &argv);
        }
        self.inner.start_frame(eng, frametime);
    }

    fn think(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        self.inner.think(eng, id);
    }

    fn touch(&mut self, eng: &mut EngineApi<'_>, toucher: EdictId, other: EdictId) {
        let classname = eng
            .ent(toucher)
            .map(|v| v.classname.clone())
            .unwrap_or_default();
        let pre = self.runtime.dispatch_ham_touch(
            eng,
            &mut self.inner,
            toucher.index,
            &classname,
            other.index,
            false,
        );
        if pre >= HAM_SUPERCEDE {
            return;
        }
        self.inner.touch(eng, toucher, other);
        let _ = self.runtime.dispatch_ham_touch(
            eng,
            &mut self.inner,
            toucher.index,
            &classname,
            other.index,
            true,
        );
    }

    fn blocked(&mut self, eng: &mut EngineApi<'_>, pusher: EdictId, other: EdictId) {
        self.inner.blocked(eng, pusher, other);
    }

    fn update_client_data(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> GoldSrcClientData {
        self.inner.update_client_data(eng, slot)
    }

    fn get_weapon_data(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        out: &mut [GoldSrcWeaponData; 64],
    ) -> bool {
        self.inner.get_weapon_data(eng, slot, out)
    }

    fn server_command(&mut self, eng: &mut EngineApi<'_>, argv: &[String]) -> Option<String> {
        if let Some(reply) = self
            .runtime
            .dispatch_server_command(eng, &mut self.inner, argv)
        {
            return Some(reply);
        }
        self.inner.server_command(eng, argv)
    }

    fn setup_visibility(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Option<Vec3> {
        self.inner.setup_visibility(eng, slot)
    }

    fn add_to_full_pack(
        &mut self,
        eng: &mut EngineApi<'_>,
        ctx: &FullPackCtx<'_>,
        state: &mut GoldSrcEntityState,
    ) -> bool {
        self.inner.add_to_full_pack(eng, ctx, state)
    }

    fn delta_encode(
        &mut self,
        encoder: &str,
        current_player: u8,
        from: &GoldSrcEntityState,
        to: &GoldSrcEntityState,
        marks: &mut DeltaMarks<'_>,
    ) {
        self.inner
            .delta_encode(encoder, current_player, from, to, marks);
    }

    fn has_delta_encoder(&self) -> bool {
        self.inner.has_delta_encoder()
    }
}
