//! Server-side mod host ([`ServerModHost`]) and [`ModdedGame<G>`] decorator wrapping any
//! [`cs_engine::GameDll`] (including [`cs_game::CsGame`]) with ABI v1 + v2 ReAPI pre/post hook
//! chains, CS/fun/engine/cstrike natives, custom weapons, 1-line VFX emission, paginated menus,
//! multilingual i18n, RBAC admin registry, synchronous polyglot RPC, `nvault` + $O(\log N)$
//! leaderboards, non-blocking allowlisted webhooks, hot reload, and client mod auto-download.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use cs_amxx::{AmxxCsHooks, AmxxExternalRpc, AmxxRuntime, HAM_SUPERCEDE, MENU_EXIT, NoCsHooks};
use cs_engine::{EdictId, EngineApi, FullPackCtx, GameDll, MsgDest};
use cs_game::amxx_hooks::GameStateAmxxHooks;
use cs_game::mod_hooks::{self as cs_hooks, CsHookSink};
use cs_game::player::{ArmorType, Team};
use cs_game::{CsGame, GameState};
use cs_mod_api::services::{
    MenuSelectOutcome, PluginRuntimeKind, SharedPlatformServices,
};
use cs_mod_api::{
    BombEventCtx, CMD_MOD_REQUEST, CanBuyItemCtx, CanHavePlayerItemCtx, ClientModEntry,
    CustomWeaponSpec, GrenadeThrowCtx, HookAction, HookId, HookPhase, HostageEventCtx,
    MenuSelectCtx, ModChunkPacket, ModManifestPacket, PlayerJumpCtx, PlayerKilledCtx,
    PlayerPostThinkCtx, PlayerPreThinkCtx, PlayerSayCtx, PlayerSpawnCtx, PlayerTakeDamageCtx,
    PlayersShouldCollideCtx, RoundEndCtx, RoundStartCtx, RpcValue, USERMSG_MOD_CHUNK,
    USERMSG_MOD_EVENT, USERMSG_MOD_MANIFEST, WeaponEventCtx,
};
use cs_platform::{ConfigStorage, MemoryStorage};
use cs_proto48::delta::{
    DeltaMarks, GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd, GoldSrcWeaponData,
};
use cs_proto48::usermsg::{CsUserMsg, UserMsgRegistry};
use glam::Vec3;
use tracing::warn;

use crate::admins::{load_admins_from_dir, sync_player_admin};
use crate::crash_dump::{
    DEFAULT_RNG_SEED, FAULT_DUMP_SCHEMA_VERSION, FaultDumpStore, FaultSummary, InvocationTrigger,
    ModFaultDump, PluginLanguage, catch_mod_call, current_timestamp_unix_ms, format_stack_trace,
    infer_fuel_consumed, plugin_sha256_hex,
};
use crate::hookchain::HookTable;
use crate::lang::{handle_lang_chat_command, load_lang_dir, sync_player_lang_from_userinfo};
use crate::menu::send_rendered_menu;
use crate::natives::{
    ActiveEvent, ClientSnap, EngineOp, EngineSnapshot, EntitySnap, RpcDispatcher,
};
use crate::package::{LoadedPackage, PackageError, discover_packages};
use crate::runtime::{
    ModCallOutcome, ModExecContext, ModInstance, ModRuntime, PythonModRuntime, WasmiRuntime,
};
use crate::webhook::WebhookClient;

const MAX_HOOK_DEPTH: u8 = 4;

/// Live lifecycle status of one server mod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModStatus {
    Active,
    Faulted(String),
    Unloaded,
}

struct ServerTimer {
    id: u32,
    mod_id: String,
    due_secs: f64,
    interval_secs: Option<f64>,
    cb_id: i32,
    arg: i32,
}

struct HostedMod {
    package: LoadedPackage,
    binary_sha256: String,
    instance: Option<Arc<Mutex<Box<dyn ModInstance>>>>,
    status: ModStatus,
    storage_cache: BTreeMap<String, String>,
    registered_commands: BTreeSet<String>,
    total_fuel_used: u64,
}

/// Summary row for `mod_list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModInfo {
    pub id: String,
    pub version: String,
    pub status: ModStatus,
    pub total_fuel_used: u64,
}

struct ServerModHostCore {
    runtime: Box<dyn ModRuntime>,
    python_runtime: Option<Box<dyn PythonModRuntime>>,
    mods: BTreeMap<String, HostedMod>,
    load_order: Vec<String>,
    hooks: HookTable,
    timers: Vec<ServerTimer>,
    next_timer_id: u32,
    storage: Box<dyn ConfigStorage + Send>,
    mods_dir: Option<PathBuf>,
    configs_dir: Option<PathBuf>,
    custom_usermsgs: BTreeSet<String>,
    pending_client_requests: Vec<(u8, u8, u32)>,
    pending_custom_weapons: Vec<CustomWeaponSpec>,
    services: SharedPlatformServices,
    webhooks: WebhookClient,
    rpc_side_effects: Arc<Mutex<Vec<(String, ModCallOutcome)>>>,
    wasi_logs: Vec<(String, String)>,
    hook_depth: Arc<AtomicU8>,
    amxx: Option<Arc<Mutex<AmxxRuntime>>>,
    #[cfg(not(target_arch = "wasm32"))]
    py_watch: BTreeMap<String, (PathBuf, Option<std::time::SystemTime>, u64)>,
    #[cfg(not(target_arch = "wasm32"))]
    last_autoreload_poll_secs: Option<f64>,
    faults: FaultDumpStore,
}

struct HostRpcDispatcher {
    core: Arc<Mutex<ServerModHostCore>>,
    services: SharedPlatformServices,
    side_effects: Arc<Mutex<Vec<(String, ModCallOutcome)>>>,
}

impl RpcDispatcher for HostRpcDispatcher {
    fn call_rpc(&self, fn_name: &str, args: &[RpcValue]) -> Option<RpcValue> {
        let entry = {
            let svc = self.services.lock();
            svc.rpc.resolve_fn(fn_name).cloned()?
        };
        if entry.runtime == PluginRuntimeKind::Host {
            let svc = self.services.lock();
            return svc.rpc.call_host_fn(fn_name, args);
        }
        if entry.runtime == PluginRuntimeKind::Amxx {
            let amxx_arc = {
                let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                core.amxx.clone()
            }?;
            let Ok(mut amxx) = amxx_arc.try_lock() else {
                return None;
            };
            return amxx.invoke_registered_native_pure(
                entry.owner_index,
                &entry.callback_name,
                args,
            );
        }
        if !matches!(
            entry.runtime,
            PluginRuntimeKind::Wasm | PluginRuntimeKind::Python
        ) {
            return None;
        }
        let (inst_arc, storage, fuel_budget) = {
            let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            let m = core.mods.get(&entry.owner_id)?;
            if m.status != ModStatus::Active {
                return None;
            }
            (
                m.instance.clone()?,
                m.storage_cache.clone(),
                m.package.manifest.fuel_per_frame,
            )
        };

        // Guard against self-reentrant RPC calls without deadlocking.
        let Ok(mut inst) = inst_arc.try_lock() else {
            return None;
        };

        let mut ev = ActiveEvent::default();
        let mut i32_cursor = 0usize;
        let mut f32_cursor = 0usize;
        let mut str_cursor = 0usize;
        for v in args {
            match v {
                RpcValue::Int(n) => {
                    if i32_cursor < ev.i32_args.len() {
                        ev.i32_args[i32_cursor] = *n;
                        i32_cursor += 1;
                    }
                }
                RpcValue::Float(f) => {
                    if f32_cursor < ev.f32_args.len() {
                        ev.f32_args[f32_cursor] = *f;
                        f32_cursor += 1;
                    }
                }
                RpcValue::Str(s) => {
                    if str_cursor < ev.str_args.len() {
                        ev.str_args[str_cursor] = s.clone();
                        str_cursor += 1;
                    }
                }
                RpcValue::Bytes(b) => {
                    ev.payload = b.clone();
                }
            }
        }
        let snap = EngineSnapshot::default();
        let ctx = ModExecContext {
            snapshot: &snap,
            event: &ev,
            storage: &storage,
            fuel_budget,
        };
        let call_res = catch_mod_call(|| inst.call_rpc(entry.callback_id, &ctx));
        drop(inst);
        let (rc, outcome) = match call_res {
            Ok(v) => v,
            Err(err) => {
                let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                record_and_fault_mod_core(
                    &mut core,
                    &entry.owner_id,
                    InvocationTrigger::Rpc {
                        cb_id: entry.callback_id,
                        fn_name: fn_name.to_string(),
                    },
                    &ev,
                    &snap,
                    &storage,
                    fuel_budget,
                    err.to_string(),
                );
                return None;
            }
        };
        let ret = match outcome.rpc_return_str.clone() {
            Some(s) => RpcValue::Str(s),
            None => RpcValue::Int(rc),
        };
        let mut se = self.side_effects.lock().unwrap_or_else(|e| e.into_inner());
        se.push((entry.owner_id, outcome));
        Some(ret)
    }
}

/// Bridge handed to [`AmxxRuntime`] so `.amxx` dynamic natives and `ExecuteForward` invoke
/// `.wasm` / `.py` mods hosted by [`ServerModHost`] synchronously without deadlocking.
struct HostAmxxExternalRpc {
    core: Arc<Mutex<ServerModHostCore>>,
    services: SharedPlatformServices,
    side_effects: Arc<Mutex<Vec<(String, ModCallOutcome)>>>,
}

impl AmxxExternalRpc for HostAmxxExternalRpc {
    fn call_external_rpc(&self, fn_name: &str, args: &[RpcValue]) -> Option<RpcValue> {
        let dispatcher = HostRpcDispatcher {
            core: Arc::clone(&self.core),
            services: self.services.clone(),
            side_effects: Arc::clone(&self.side_effects),
        };
        dispatcher.call_rpc(fn_name, args)
    }

    fn emit_external_event(&self, event_name: &str, args: &[RpcValue]) {
        let subs = {
            let svc = self.services.lock();
            svc.rpc.subscribers(event_name)
        };
        if subs.is_empty() {
            return;
        }
        let mut ev = ActiveEvent::default();
        ev.str_args[0] = event_name.to_string();
        let mut i32_cursor = 0usize;
        let mut f32_cursor = 0usize;
        let mut first_str: Option<String> = None;
        for v in args {
            match v {
                RpcValue::Int(n) => {
                    if i32_cursor < ev.i32_args.len() {
                        ev.i32_args[i32_cursor] = *n;
                        i32_cursor += 1;
                    }
                }
                RpcValue::Float(f) => {
                    if f32_cursor < ev.f32_args.len() {
                        ev.f32_args[f32_cursor] = *f;
                        f32_cursor += 1;
                    }
                }
                RpcValue::Str(s) => {
                    if first_str.is_none() {
                        first_str = Some(s.clone());
                    }
                }
                RpcValue::Bytes(b) => {
                    ev.payload = b.clone();
                }
            }
        }
        if let Some(s) = first_str {
            ev.payload = s.as_bytes().to_vec();
            ev.str_args[1] = s;
        }
        let snap = EngineSnapshot::default();
        for sub in subs {
            if !matches!(
                sub.runtime,
                PluginRuntimeKind::Wasm | PluginRuntimeKind::Python
            ) {
                continue;
            }
            let target = {
                let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                core.mods.get(&sub.owner_id).and_then(|m| {
                    if m.status != ModStatus::Active {
                        return None;
                    }
                    m.instance.clone().map(|inst| {
                        (
                            inst,
                            m.storage_cache.clone(),
                            m.package.manifest.fuel_per_frame,
                        )
                    })
                })
            };
            let Some((inst_arc, storage, fuel_budget)) = target else {
                continue;
            };
            let Ok(mut inst) = inst_arc.try_lock() else {
                continue;
            };
            let ctx = ModExecContext {
                snapshot: &snap,
                event: &ev,
                storage: &storage,
                fuel_budget,
            };
            let res = catch_mod_call(|| inst.call_event(sub.callback_id, &ctx));
            drop(inst);
            match res {
                Ok(outcome) => {
                    let mut se = self.side_effects.lock().unwrap_or_else(|e| e.into_inner());
                    se.push((sub.owner_id, outcome));
                }
                Err(err) => {
                    let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                    record_and_fault_mod_core(
                        &mut core,
                        &sub.owner_id,
                        InvocationTrigger::RpcEvent {
                            cb_id: sub.callback_id,
                            event_name: event_name.to_string(),
                        },
                        &ev,
                        &snap,
                        &storage,
                        fuel_budget,
                        err.to_string(),
                    );
                }
            }
        }
    }
}

/// Coordinates sandboxed server WASM mods, hook chains, timers, storage, shared platform
/// services (`MenuManager`, `LangCatalog`, `AdminRegistry`, `NVaultStore`, `LeaderboardStore`,
/// `RpcBus`), non-blocking webhooks, and client catalogs.
pub struct ServerModHost {
    core: Arc<Mutex<ServerModHostCore>>,
}

impl Default for ServerModHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerModHost {
    pub fn new() -> Self {
        Self::with_runtime(
            Box::new(WasmiRuntime::new()),
            Box::new(MemoryStorage::default()),
        )
    }

    pub fn with_runtime(
        runtime: Box<dyn ModRuntime>,
        storage: Box<dyn ConfigStorage + Send>,
    ) -> Self {
        Self::with_runtime_and_services(runtime, storage, SharedPlatformServices::new())
    }

    pub fn with_runtime_and_services(
        runtime: Box<dyn ModRuntime>,
        storage: Box<dyn ConfigStorage + Send>,
        services: SharedPlatformServices,
    ) -> Self {
        let mut faults = FaultDumpStore::new();
        faults.load_from_storage(&*storage);
        let core = ServerModHostCore {
            runtime,
            python_runtime: None,
            mods: BTreeMap::new(),
            load_order: Vec::new(),
            hooks: HookTable::default(),
            timers: Vec::new(),
            next_timer_id: 1,
            storage,
            mods_dir: None,
            configs_dir: None,
            custom_usermsgs: BTreeSet::new(),
            pending_client_requests: Vec::new(),
            pending_custom_weapons: Vec::new(),
            services,
            webhooks: WebhookClient::new(),
            rpc_side_effects: Arc::new(Mutex::new(Vec::new())),
            wasi_logs: Vec::new(),
            hook_depth: Arc::new(AtomicU8::new(0)),
            amxx: None,
            #[cfg(not(target_arch = "wasm32"))]
            py_watch: BTreeMap::new(),
            #[cfg(not(target_arch = "wasm32"))]
            last_autoreload_poll_secs: None,
            faults,
        };
        Self {
            core: Arc::new(Mutex::new(core)),
        }
    }

    /// Registers a pure-Rust Python scripting runtime (`cs-mod-py`) so `.py` plugins can be
    /// discovered, loaded, executed, and hot-reloaded seamlessly alongside `.wasm` and `.amxx`.
    pub fn set_python_runtime(&mut self, runtime: Box<dyn PythonModRuntime>) {
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.python_runtime = Some(runtime);
    }

    pub fn has_python_runtime(&self) -> bool {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.python_runtime.is_some()
    }

    pub fn services(&self) -> SharedPlatformServices {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.services.clone()
    }

    pub fn set_mods_dir(&mut self, dir: impl Into<PathBuf>) {
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.mods_dir = Some(dir.into());
    }

    /// Configures the disk directory (e.g. `logs/mod_faults`) where bounded JSON black-box
    /// crash dumps are written and ring-evicted (`<= 32` dumps).
    pub fn set_fault_dump_dir(&mut self, dir: impl Into<PathBuf>) {
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.faults.set_disk_dir(Some(dir.into()));
    }

    /// Returns the configured disk directory for black-box crash dumps, if any.
    pub fn fault_dump_dir(&self) -> Option<PathBuf> {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.faults.disk_dir().map(Path::to_path_buf)
    }

    /// Hydrates retained crash dumps from a disk directory (e.g. `logs/mod_faults`).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_fault_dumps_from_dir(&mut self, dir: &Path) -> Result<usize, String> {
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.faults.load_from_dir(dir)
    }

    /// Lists retained black-box fault dump summaries ordered newest-first (`<= 32` entries).
    pub fn list_faults(&self) -> Vec<FaultSummary> {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.faults.list_faults()
    }

    /// Serializes [`list_faults`](Self::list_faults) as JSON for `GET /api/mods/faults`.
    pub fn list_faults_json(&self) -> String {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.faults.list_faults_json()
    }

    /// Returns the exact bounded JSON payload for dump `id` (`<id>` or `<id>.json`).
    pub fn get_fault_json(&self, id: &str) -> Option<String> {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.faults.get_fault_json(id).map(str::to_string)
    }

    /// Returns a parsed clone of fault dump `id` (`<id>` or `<id>.json`).
    pub fn get_fault(&self, id: &str) -> Option<ModFaultDump> {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.faults.get_fault(id).cloned()
    }

    /// Returns the most recently captured [`ModFaultDump`], if any.
    pub fn latest_fault(&self) -> Option<ModFaultDump> {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.faults.latest().cloned()
    }

    /// Clears all retained fault dumps from memory, storage, and configured disk directory.
    pub fn clear_faults(&mut self) {
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        let ServerModHostCore {
            faults, storage, ..
        } = &mut *core;
        faults.clear(Some(&mut **storage));
    }

    /// Records an externally produced fault dump (e.g. from `.py` or `.amxx` runtimes) into the
    /// unified bounded ring buffer and persistence backend.
    pub fn record_external_fault(&mut self, dump: ModFaultDump) -> FaultSummary {
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        let ServerModHostCore {
            faults, storage, ..
        } = &mut *core;
        faults.record(dump, Some(&mut **storage))
    }

    /// Loads `configs/lang/*.toml` + `data/lang/*.txt` and `configs/admins.toml` + `users.ini`
    /// rooted at `base_dir`.
    pub fn load_configs_dir(&mut self, base_dir: impl Into<PathBuf>) -> (usize, usize) {
        let dir = base_dir.into();
        let (services, amxx_opt) = {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            core.configs_dir = Some(dir.clone());
            (core.services.clone(), core.amxx.clone())
        };
        let lang_dirs = [
            dir.join("configs").join("lang"),
            dir.join("lang"),
            dir.join("data").join("lang"),
            dir.join("addons").join("amxmodx").join("data").join("lang"),
        ];
        if let Some(amxx_arc) = amxx_opt {
            let mut amxx = amxx_arc.lock().unwrap_or_else(|e| e.into_inner());
            for ld in &lang_dirs {
                amxx.add_lang_search_dir(ld);
            }
        }
        let mut svc = services.lock();
        let mut lang_entries = 0usize;
        for ld in &lang_dirs {
            lang_entries += load_lang_dir(&mut svc.lang, ld);
        }
        let admin_entries = load_admins_from_dir(&mut svc.admins, &dir);
        (lang_entries, admin_entries)
    }

    fn ensure_amxx(&self) -> Arc<Mutex<AmxxRuntime>> {
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref existing) = core.amxx {
            return Arc::clone(existing);
        }
        let mut rt = AmxxRuntime::with_services(core.services.clone());
        rt.set_external_rpc(Arc::new(HostAmxxExternalRpc {
            core: Arc::clone(&self.core),
            services: core.services.clone(),
            side_effects: Arc::clone(&core.rpc_side_effects),
        }));
        if let Some(ref cfg_dir) = core.configs_dir {
            rt.add_lang_search_dir(cfg_dir.join("configs").join("lang"));
            rt.add_lang_search_dir(cfg_dir.join("lang"));
            rt.add_lang_search_dir(cfg_dir.join("data").join("lang"));
            rt.add_lang_search_dir(
                cfg_dir
                    .join("addons")
                    .join("amxmodx")
                    .join("data")
                    .join("lang"),
            );
        }
        let arc = Arc::new(Mutex::new(rt));
        core.amxx = Some(Arc::clone(&arc));
        arc
    }

    /// Loads one `.amxx` plugin into the unified runtime sharing `SharedPlatformServices`,
    /// `MenuManager`, `LangCatalog`, `AdminRegistry`, `NVaultStore`, `LeaderboardStore`, and `RpcBus`.
    pub fn load_amxx_plugin(
        &mut self,
        name: impl Into<String>,
        bytes: &[u8],
    ) -> Result<usize, cs_amxx::AmxError> {
        let amxx_arc = self.ensure_amxx();
        let mut amxx = amxx_arc.lock().unwrap_or_else(|e| e.into_inner());
        amxx.load_plugin(name, bytes)
    }

    /// Loads a batch of `(name, bytes)` `.amxx` plugins into the unified runtime.
    pub fn load_amxx_plugins(&mut self, plugins: Vec<(String, Vec<u8>)>) {
        if plugins.is_empty() {
            return;
        }
        let amxx_arc = self.ensure_amxx();
        let mut amxx = amxx_arc.lock().unwrap_or_else(|e| e.into_inner());
        amxx.load_plugins(plugins);
    }

    /// Scans `dir` for `.amxx` / `.amx` plugins in lexicographical order and loads them into the
    /// unified runtime.
    pub fn load_amxx_dir(&mut self, dir: &Path) -> Result<usize, String> {
        let entries = std::fs::read_dir(dir)
            .map_err(|e| format!("failed to scan amxx dir `{}`: {e}", dir.display()))?;
        let mut paths = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|e| format!("failed to read amxx dir `{}`: {e}", dir.display()))?;
            let path = entry.path();
            let is_amxx = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("amxx") || e.eq_ignore_ascii_case("amx"));
            if path.is_file() && is_amxx {
                paths.push(path);
            }
        }
        paths.sort();
        let mut batch = Vec::with_capacity(paths.len());
        for path in paths {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("plugin.amxx")
                .to_string();
            let bytes = std::fs::read(&path)
                .map_err(|e| format!("failed to read AMXX plugin `{}`: {e}", path.display()))?;
            batch.push((name, bytes));
        }
        let count = batch.len();
        self.load_amxx_plugins(batch);
        Ok(count)
    }

    /// Parses a `configs/plugins.toml` manifest and loads declared WASM/package mods, `.amxx`
    /// plugins, localization catalogs, and admin registries into this unified [`ServerModHost`].
    /// Returns `(wasm_mods_loaded, amxx_plugins_loaded)`.
    pub fn load_plugins_toml(&mut self, path: &Path) -> Result<(usize, usize), String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read `{}`: {e}", path.display()))?;
        let toml_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let _ = self.load_configs_dir(toml_dir);
        if toml_dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case("configs"))
            && let Some(parent) = toml_dir.parent()
        {
            let _ = self.load_configs_dir(parent);
        }

        let cfg = parse_plugins_toml(&raw);
        if let Some(ref extra_cfg) = cfg.configs_dir {
            let _ = self.load_configs_dir(resolve_rel(toml_dir, extra_cfg));
        }

        let mut wasm_count = 0usize;
        let mut amxx_count = 0usize;

        if let Some(ref mods_dir) = cfg.mods_dir {
            let resolved = resolve_rel(toml_dir, mods_dir);
            if resolved.is_dir() {
                wasm_count += self
                    .load_dir(&resolved)
                    .map_err(|e| format!("plugins.toml mods_dir `{}`: {e}", resolved.display()))?;
            }
        }
        if let Some(ref amxx_dir) = cfg.amxx_dir {
            let resolved = resolve_rel(toml_dir, amxx_dir);
            if resolved.is_dir() {
                amxx_count += self.load_amxx_dir(&resolved)?;
            }
        }

        for entry_path in cfg.mod_entries {
            let p = resolve_rel(toml_dir, &entry_path);
            if p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("py"))
            {
                self.load_python_file(&p)?;
                wasm_count += 1;
                continue;
            }
            if p.is_dir() && !p.join("mod.toml").is_file() && p.join("main.py").is_file() {
                self.load_python_file(&p.join("main.py"))?;
                wasm_count += 1;
                continue;
            }
            let pkg = if p.is_dir() {
                crate::package::load_package_dir(&p).map_err(|e| e.to_string())?
            } else {
                let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                crate::package::load_package_csmod(&bytes, Some(p)).map_err(|e| e.to_string())?
            };
            self.load_package(pkg)?;
            wasm_count += 1;
        }

        for entry_path in cfg.amxx_entries {
            let p = resolve_rel(toml_dir, &entry_path);
            let bytes = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("plugin.amxx")
                .to_string();
            self.load_amxx_plugin(name, &bytes)
                .map_err(|e| format!("failed to load AMXX plugin `{}`: {e}", p.display()))?;
            amxx_count += 1;
        }

        Ok((wasm_count, amxx_count))
    }

    pub fn has_amxx(&self) -> bool {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.amxx.is_some()
    }

    pub fn amxx_plugin_count(&self) -> usize {
        let amxx_opt = {
            let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            core.amxx.clone()
        };
        amxx_opt.map_or(0, |a| {
            a.lock().unwrap_or_else(|e| e.into_inner()).plugin_count()
        })
    }

    pub fn amxx_server_prints(&self) -> Vec<String> {
        let amxx_opt = {
            let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            core.amxx.clone()
        };
        amxx_opt.map_or_else(Vec::new, |a| {
            a.lock()
                .unwrap_or_else(|e| e.into_inner())
                .state
                .server_prints()
                .to_vec()
        })
    }

    pub fn amxx_runtime(&self) -> Option<Arc<Mutex<AmxxRuntime>>> {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.amxx.clone()
    }

    /// Returns all bundled VFS assets across currently active mod packages so servers can mount
    /// them into [`cs_vfs::Vfs`].
    #[allow(clippy::type_complexity)]
    pub fn loaded_mod_assets(&self) -> Vec<(String, Vec<(String, Vec<u8>)>)> {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        for id in &core.load_order {
            if let Some(m) = core.mods.get(id)
                && !m.package.assets.is_empty()
            {
                let entries = m
                    .package
                    .assets
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                out.push((m.package.manifest.id.clone(), entries));
            }
        }
        out
    }

    /// Drains captured WASI `stdout`/`stderr` log lines `(mod_id, line)`.
    pub fn take_wasi_logs(&mut self) -> Vec<(String, String)> {
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut core.wasi_logs)
    }

    /// Discovers and loads all mods from `mods_dir` in dependency order (both WASM packages and
    /// standalone Python `*.py` / `<dir>/main.py` plugins when a Python runtime is attached).
    pub fn load_dir(&mut self, mods_dir: impl Into<PathBuf>) -> Result<usize, PackageError> {
        let dir = mods_dir.into();
        let py_enabled = {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            core.mods_dir = Some(dir.clone());
            core.python_runtime.is_some()
        };
        let packages = discover_packages(&dir)?;
        let mut count = 0usize;
        for pkg in packages {
            if let Err(err) = self.load_package(pkg) {
                warn!("failed to load mod package: {err}");
            } else {
                count += 1;
            }
        }
        if py_enabled && dir.is_dir() {
            let mut py_paths = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file()
                        && p.extension()
                            .and_then(|e| e.to_str())
                            .is_some_and(|e| e.eq_ignore_ascii_case("py"))
                    {
                        py_paths.push(p);
                    } else if p.is_dir() && !p.join("mod.toml").is_file() && p.join("main.py").is_file()
                    {
                        py_paths.push(p.join("main.py"));
                    }
                }
            }
            py_paths.sort();
            for py_path in py_paths {
                match self.load_python_file(&py_path) {
                    Ok(_) => count += 1,
                    Err(err) => warn!(path = %py_path.display(), %err, "failed to load Python mod"),
                }
            }
        }
        Ok(count)
    }

    /// Loads or hot-reloads a standalone Python 3 script directly from source text while
    /// preserving any persisted `mod.storage`.
    ///
    /// Syntax and metadata are validated BEFORE replacing any previously running instance, so a
    /// broken edit leaves the previous live plugin intact.
    pub fn load_python_source(
        &mut self,
        mod_id_fallback: &str,
        file_name: &str,
        source: &str,
        source_path: Option<PathBuf>,
    ) -> Result<String, String> {
        let manifest = {
            let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            let py_rt = core
                .python_runtime
                .as_ref()
                .ok_or_else(|| "Python mod runtime is not enabled on ServerModHost".to_string())?;
            py_rt.synthesize_manifest(mod_id_fallback, file_name, source)?
        };
        let id = manifest.id.clone();
        let pkg = LoadedPackage {
            manifest,
            source_path,
            server_wasm: Some(source.as_bytes().to_vec()),
            client_wasm: None,
            client_bundle: None,
            assets: BTreeMap::new(),
        };
        self.load_package(pkg)?;
        Ok(id)
    }

    /// Reads and loads (or hot-reloads) a `.py` script from disk, synthesizing its manifest from
    /// `@mod.info(...)` (or using an adjacent `mod.toml` when loading `<dir>/main.py`).
    pub fn load_python_file(&mut self, path: &Path) -> Result<String, String> {
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("plugin.py");
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("py_plugin");
        let fallback_id = if stem.eq_ignore_ascii_case("main") {
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("py_plugin")
        } else {
            stem
        };
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read Python mod `{}`: {e}", path.display()))?;
        if stem.eq_ignore_ascii_case("main")
            && let Some(parent) = path.parent()
            && parent.join("mod.toml").is_file()
        {
            let pkg = crate::package::load_package_dir(parent).map_err(|e| e.to_string())?;
            let id = pkg.manifest.id.clone();
            // Validate syntax before replacing the running instance.
            {
                let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                let py_rt = core.python_runtime.as_ref().ok_or_else(|| {
                    "Python mod runtime is not enabled on ServerModHost".to_string()
                })?;
                let _ = py_rt.synthesize_manifest(&id, file_name, &source)?;
            }
            self.load_package(pkg)?;
            return Ok(id);
        }
        self.load_python_source(fallback_id, file_name, &source, Some(path.to_path_buf()))
    }

    fn make_rpc_dispatcher(&self) -> Arc<dyn RpcDispatcher> {
        let (services, side_effects) = {
            let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            (core.services.clone(), Arc::clone(&core.rpc_side_effects))
        };
        Arc::new(HostRpcDispatcher {
            core: Arc::clone(&self.core),
            services,
            side_effects,
        })
    }

    /// Loads or replaces a single mod package (`cs_mod_init` runs immediately).
    pub fn load_package(&mut self, pkg: LoadedPackage) -> Result<(), String> {
        let id = pkg.manifest.id.clone();
        let is_py = is_python_manifest(&pkg.manifest);
        let rpc_dispatcher = self.make_rpc_dispatcher();

        // Compile/instantiate before tearing down any existing live instance so broken reloads
        // cannot knock out an active plugin.
        let mut instance = {
            let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            match &pkg.server_wasm {
                Some(payload) => {
                    let mut boxed = if is_py {
                        let py_rt = core.python_runtime.as_ref().ok_or_else(|| {
                            format!(
                                "cannot load Python mod `{id}`: Python runtime is not enabled"
                            )
                        })?;
                        py_rt
                            .instantiate(
                                &id,
                                payload,
                                pkg.manifest.permissions,
                                pkg.manifest.max_memory_bytes,
                            )
                            .map_err(|e| e.to_string())?
                    } else {
                        core.runtime
                            .instantiate(
                                &id,
                                payload,
                                pkg.manifest.permissions,
                                pkg.manifest.max_memory_bytes,
                            )
                            .map_err(|e| e.to_string())?
                    };
                    boxed.configure_v2(
                        core.services.clone(),
                        pkg.manifest.http_hosts.clone(),
                        Some(Arc::clone(&rpc_dispatcher)),
                    );
                    Some(boxed)
                }
                None => None,
            }
        };

        self.unload_mod(&id);

        let (storage_cache, services) = {
            let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            for um in &pkg.manifest.custom_usermsgs {
                core.custom_usermsgs.insert(um.clone());
            }

            let mut storage_cache = BTreeMap::new();
            let index_key = format!("mod:{id}:__keys");
            if let Some(key_list) = core.storage.load(&index_key) {
                for k in key_list.lines().filter(|l| !l.is_empty()) {
                    let full = format!("mod:{id}:{k}");
                    if let Some(val) = core.storage.load(&full) {
                        storage_cache.insert(k.to_string(), val);
                    }
                }
            }
            (storage_cache, core.services.clone())
        };

        let binary_sha256 = plugin_sha256_hex(pkg.server_wasm.as_deref());
        let mut init_outcome = ModCallOutcome::default();

        if let Some(ref mut inst) = instance {
            let snap = EngineSnapshot::default();
            let ev = ActiveEvent::default();
            let fuel_budget = pkg.manifest.fuel_per_frame;
            let ctx = ModExecContext {
                snapshot: &snap,
                event: &ev,
                storage: &storage_cache,
                fuel_budget,
            };
            match catch_mod_call(|| inst.call_init(&ctx)) {
                Ok(out) => {
                    init_outcome = out;
                }
                Err(err) => {
                    let msg = err.to_string();
                    let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                    let storage_snap = storage_cache.clone();
                    core.mods.insert(
                        id.clone(),
                        HostedMod {
                            package: pkg,
                            binary_sha256,
                            instance: None,
                            status: ModStatus::Faulted(msg.clone()),
                            storage_cache,
                            registered_commands: BTreeSet::new(),
                            total_fuel_used: 0,
                        },
                    );
                    core.load_order.push(id.clone());
                    record_and_fault_mod_core(
                        &mut core,
                        &id,
                        InvocationTrigger::Init,
                        &ev,
                        &snap,
                        &storage_snap,
                        fuel_budget,
                        msg.clone(),
                    );
                    return Err(msg);
                }
            }
        }

        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        for h in &init_outcome.hooks_registered {
            core.hooks.register(&id, h.hook, h.phase, h.priority);
        }
        let mut registered_commands = BTreeSet::new();
        for op in &init_outcome.ops {
            match op {
                EngineOp::RegisterCommand { name } => {
                    registered_commands.insert(name.clone());
                }
                EngineOp::RegisterCustomWeapon(spec) => {
                    core.pending_custom_weapons.push(spec.clone());
                }
                _ => {}
            }
        }
        for line in &init_outcome.wasi_logs {
            core.wasi_logs.push((id.clone(), line.clone()));
        }

        #[cfg(not(target_arch = "wasm32"))]
        if is_py
            && let Some(py_path) = resolve_python_watch_path(&pkg)
        {
            let (mtime, len) = py_file_stamp(&py_path);
            core.py_watch.insert(id.clone(), (py_path, mtime, len));
        }

        let _ = services;
        core.mods.insert(
            id.clone(),
            HostedMod {
                package: pkg,
                binary_sha256,
                instance: instance.map(|i| Arc::new(Mutex::new(i))),
                status: ModStatus::Active,
                storage_cache,
                registered_commands,
                total_fuel_used: init_outcome.fuel_used,
            },
        );
        core.load_order.push(id.clone());
        schedule_timers_core(&mut core, &id, 0.0, &init_outcome);
        persist_storage_writes_core(&mut core, &id, &init_outcome.storage_writes);
        Ok(())
    }

    pub fn unload_mod(&mut self, mod_id: &str) -> bool {
        let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.hooks.unregister_mod(mod_id);
        core.timers.retain(|t| t.mod_id != mod_id);
        core.load_order.retain(|id| id != mod_id);
        #[cfg(not(target_arch = "wasm32"))]
        core.py_watch.remove(mod_id);
        {
            let mut svc = core.services.lock();
            svc.menus.clear_owner(mod_id);
            svc.rpc.unregister_owner(mod_id);
        }
        if let Some(m) = core.mods.get_mut(mod_id) {
            m.instance = None;
            m.status = ModStatus::Unloaded;
            true
        } else {
            false
        }
    }

    /// Hot-reloads one mod from disk (`source_path` or `<mods_dir>/<id>`).
    pub fn reload_mod(&mut self, mod_id: &str) -> Result<(), String> {
        let src = {
            let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
            core.mods
                .get(mod_id)
                .and_then(|m| m.package.source_path.clone())
                .or_else(|| core.mods_dir.as_ref().map(|d| d.join(mod_id)))
                .ok_or_else(|| format!("unknown source path for mod `{mod_id}`"))?
        };

        if src.is_file()
            && src
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("py"))
        {
            return self.load_python_file(&src).map(|_| ());
        }
        if src.is_dir() && !src.join("mod.toml").is_file() && src.join("main.py").is_file() {
            return self.load_python_file(&src.join("main.py")).map(|_| ());
        }

        let pkg = if src.is_dir() {
            crate::package::load_package_dir(&src).map_err(|e| e.to_string())?
        } else {
            let bytes = std::fs::read(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            crate::package::load_package_csmod(&bytes, Some(src)).map_err(|e| e.to_string())?
        };
        self.load_package(pkg)
    }

    /// Polls watched `.py` plugin files for `mtime`/size changes (at most once per `1.0 s` unless
    /// `force == true`) and hot-reloads any modified scripts while preserving `mod.storage`.
    pub fn poll_autoreload(&mut self, now_secs: f64, force: bool) -> Vec<(String, Result<(), String>)> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (now_secs, force);
            Vec::new()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let changed: Vec<(String, PathBuf)> = {
                let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                if core.py_watch.is_empty() {
                    return Vec::new();
                }
                if !force
                    && let Some(prev) = core.last_autoreload_poll_secs
                    && now_secs >= prev
                    && (now_secs - prev) < 1.0
                {
                    return Vec::new();
                }
                core.last_autoreload_poll_secs = Some(now_secs);
                let mut dirty = Vec::new();
                for (mod_id, (path, last_mtime, last_len)) in &mut core.py_watch {
                    let (mtime, len) = py_file_stamp(path);
                    if mtime != *last_mtime || len != *last_len {
                        *last_mtime = mtime;
                        *last_len = len;
                        dirty.push((mod_id.clone(), path.clone()));
                    }
                }
                dirty
            };
            let mut results = Vec::with_capacity(changed.len());
            for (mod_id, path) in changed {
                let res = self.load_python_file(&path).map(|_| ());
                results.push((mod_id, res));
            }
            results
        }
    }

    pub fn list_mods(&self) -> Vec<ModInfo> {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.load_order
            .iter()
            .filter_map(|id| core.mods.get(id))
            .map(|m| ModInfo {
                id: m.package.manifest.id.clone(),
                version: m.package.manifest.version.clone(),
                status: m.status.clone(),
                total_fuel_used: m.total_fuel_used,
            })
            .collect()
    }

    pub fn declared_custom_usermsgs(&self) -> Vec<String> {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        core.custom_usermsgs.iter().cloned().collect()
    }

    /// Ordered client-downloadable mod packages (having client WASM and/or assets).
    pub fn client_catalog(&self) -> Vec<(ClientModEntry, Vec<u8>)> {
        let core = self.core.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        for id in &core.load_order {
            let Some(m) = core.mods.get(id) else {
                continue;
            };
            if m.status != ModStatus::Active {
                continue;
            }
            if let Some((ref bundle, ref sha)) = m.package.client_bundle {
                out.push((
                    ClientModEntry {
                        id: m.package.manifest.id.clone(),
                        version: m.package.manifest.version.clone(),
                        sha256_hex: sha.clone(),
                        byte_len: bundle.len() as u32,
                        permissions_mask: m.package.manifest.permissions.bits(),
                    },
                    bundle.clone(),
                ));
            }
        }
        out
    }

    /// Dispatches a hook chain (`HookId`, `HookPhase`), replaying queued `EngineOp`s cleanly.
    pub fn dispatch_hook(
        &mut self,
        eng: &mut EngineApi<'_>,
        hook: HookId,
        phase: HookPhase,
        event: &ActiveEvent,
    ) -> HookDispatchResult {
        dispatch_hook_core(&self.core, None, eng, hook, phase, event)
    }

    pub fn dispatch_hook_with_game(
        &mut self,
        game: Option<&mut GameState>,
        eng: &mut EngineApi<'_>,
        hook: HookId,
        phase: HookPhase,
        event: &ActiveEvent,
    ) -> HookDispatchResult {
        dispatch_hook_core(&self.core, game, eng, hook, phase, event)
    }

    pub fn tick_timers(&mut self, eng: &mut EngineApi<'_>) {
        tick_timers_core(&self.core, None, eng);
    }

    pub fn tick_timers_with_game(
        &mut self,
        game: Option<&mut GameState>,
        eng: &mut EngineApi<'_>,
    ) {
        tick_timers_core(&self.core, game, eng);
    }

    pub fn poll_webhooks_with_game(
        &mut self,
        game: Option<&mut GameState>,
        eng: &mut EngineApi<'_>,
    ) {
        poll_webhooks_core(&self.core, game, eng);
    }

    fn core_arc(&self) -> Arc<Mutex<ServerModHostCore>> {
        Arc::clone(&self.core)
    }
}

fn persist_storage_writes_core(
    core: &mut ServerModHostCore,
    mod_id: &str,
    writes: &[(String, Option<String>)],
) {
    if writes.is_empty() {
        return;
    }
    let Some(m) = core.mods.get_mut(mod_id) else {
        return;
    };
    for (k, val_opt) in writes {
        if !is_safe_storage_subkey(k) {
            continue;
        }
        let full = format!("mod:{mod_id}:{k}");
        match val_opt {
            Some(v) => {
                m.storage_cache.insert(k.clone(), v.clone());
                let _ = core.storage.save(&full, v);
            }
            None => {
                m.storage_cache.remove(k);
                let _ = core.storage.remove(&full);
            }
        }
    }
    let index_val: String = m
        .storage_cache
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let _ = core
        .storage
        .save(&format!("mod:{mod_id}:__keys"), &index_val);
}

fn schedule_timers_core(
    core: &mut ServerModHostCore,
    mod_id: &str,
    now_secs: f64,
    out: &ModCallOutcome,
) {
    if !out.cancel_timers.is_empty() {
        core.timers
            .retain(|t| !(t.mod_id == mod_id && out.cancel_timers.contains(&t.id)));
    }
    for req in &out.timers {
        if core.timers.len() >= 256 {
            break;
        }
        let delay = (f64::from(req.delay_ms) / 1000.0).max(0.001);
        let id = core.next_timer_id;
        core.next_timer_id = core.next_timer_id.wrapping_add(1).max(1);
        core.timers.push(ServerTimer {
            id,
            mod_id: mod_id.to_string(),
            due_secs: now_secs + delay,
            interval_secs: req.repeat.then_some(delay),
            cb_id: req.cb_id,
            arg: req.arg,
        });
    }
}

fn fault_mod_core(core: &mut ServerModHostCore, mod_id: &str, reason: String) {
    warn!(mod_id, %reason, "mod faulted; unloading");
    core.hooks.unregister_mod(mod_id);
    core.timers.retain(|t| t.mod_id != mod_id);
    {
        let mut svc = core.services.lock();
        svc.menus.clear_owner(mod_id);
        svc.rpc.unregister_owner(mod_id);
    }
    if let Some(m) = core.mods.get_mut(mod_id) {
        m.instance = None;
        m.status = ModStatus::Faulted(reason);
    }
}

#[allow(clippy::too_many_arguments)]
fn record_and_fault_mod_core(
    core: &mut ServerModHostCore,
    mod_id: &str,
    trigger: InvocationTrigger,
    event: &ActiveEvent,
    snapshot: &EngineSnapshot,
    storage_cache: &BTreeMap<String, String>,
    fuel_budget: u64,
    reason: String,
) {
    let timestamp_unix_ms = current_timestamp_unix_ms(snapshot.time_secs);
    let (
        version,
        language,
        source_path,
        binary_sha256,
        permissions_mask,
        http_hosts,
        max_memory_bytes,
    ) = match core.mods.get(mod_id) {
        Some(m) => (
            m.package.manifest.version.clone(),
            PluginLanguage::infer_from_entry(
                m.package.manifest.entry_server.as_deref(),
                m.package.server_wasm.is_some(),
            ),
            m.package
                .source_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            m.binary_sha256.clone(),
            m.package.manifest.permissions.bits(),
            m.package.manifest.http_hosts.clone(),
            m.package.manifest.max_memory_bytes,
        ),
        None => (
            String::new(),
            PluginLanguage::Unknown,
            None,
            String::new(),
            0,
            Vec::new(),
            cs_mod_api::DEFAULT_MEMORY_BYTES,
        ),
    };

    let fuel_consumed = infer_fuel_consumed(&reason, fuel_budget);
    let stack_trace = format_stack_trace(
        mod_id,
        language,
        &trigger,
        &reason,
        fuel_consumed,
        fuel_budget,
    );
    let dump_id = ModFaultDump::make_dump_id(timestamp_unix_ms, mod_id, &trigger);
    let dump = ModFaultDump {
        schema_version: FAULT_DUMP_SCHEMA_VERSION,
        dump_id,
        timestamp_unix_ms,
        mod_id: mod_id.to_string(),
        version,
        language,
        source_path,
        binary_sha256,
        permissions_mask,
        http_hosts,
        fuel_budget,
        max_memory_bytes,
        trigger,
        event: event.clone(),
        snapshot: snapshot.clone(),
        storage_cache: storage_cache.clone(),
        rng_seed: DEFAULT_RNG_SEED,
        fuel_consumed_before_trap: fuel_consumed,
        error: reason.clone(),
        stack_trace,
        truncated: false,
    };

    let ServerModHostCore {
        faults, storage, ..
    } = &mut *core;
    let _ = faults.record(dump, Some(&mut **storage));
    fault_mod_core(core, mod_id, reason);
}

fn flush_rpc_side_effects_core(
    core_arc: &Arc<Mutex<ServerModHostCore>>,
    mut game: Option<&mut GameState>,
    eng: &mut EngineApi<'_>,
) {
    let now_secs = eng.time();
    let queued = {
        let mut core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
        let side_effects = {
            let mut se = core
                .rpc_side_effects
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *se)
        };
        if side_effects.is_empty() {
            return;
        }
        let mut ops_out = Vec::new();
        for (callee_id, callee_out) in side_effects {
            if let Some(m) = core.mods.get_mut(&callee_id) {
                m.total_fuel_used = m.total_fuel_used.saturating_add(callee_out.fuel_used);
            }
            for line in &callee_out.wasi_logs {
                if core.wasi_logs.len() < 256 {
                    core.wasi_logs.push((callee_id.clone(), line.clone()));
                }
            }
            persist_storage_writes_core(&mut core, &callee_id, &callee_out.storage_writes);
            schedule_timers_core(&mut core, &callee_id, now_secs, &callee_out);
            if !callee_out.ops.is_empty() {
                ops_out.push((callee_id, callee_out.ops));
            }
        }
        ops_out
    };
    for (callee_id, ops) in queued {
        apply_ops_core(core_arc, game.as_deref_mut(), eng, &callee_id, ops);
    }
}

fn with_amxx_core<R>(
    core_arc: &Arc<Mutex<ServerModHostCore>>,
    mut game: Option<&mut GameState>,
    eng: &mut EngineApi<'_>,
    f: impl FnOnce(&mut AmxxRuntime, &mut EngineApi<'_>, &mut dyn AmxxCsHooks) -> R,
) -> Option<R> {
    let amxx_arc = {
        let core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
        core.amxx.clone()
    }?;
    let res = {
        let Ok(mut amxx) = amxx_arc.try_lock() else {
            return None;
        };
        if let Some(g) = game.as_deref_mut() {
            let mut hooks = GameStateAmxxHooks(g);
            f(&mut amxx, eng, &mut hooks)
        } else {
            let mut hooks = NoCsHooks;
            f(&mut amxx, eng, &mut hooks)
        }
    };
    flush_rpc_side_effects_core(core_arc, game, eng);
    Some(res)
}

fn finalize_outcome_core(
    core_arc: &Arc<Mutex<ServerModHostCore>>,
    mod_id: &str,
    now_secs: f64,
    outcome: &ModCallOutcome,
) -> Vec<(String, Vec<EngineOp>)> {
    let mut queued = Vec::new();
    let mut core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = core.mods.get_mut(mod_id) {
        m.total_fuel_used = m.total_fuel_used.saturating_add(outcome.fuel_used);
    }
    for line in &outcome.wasi_logs {
        if core.wasi_logs.len() < 256 {
            core.wasi_logs.push((mod_id.to_string(), line.clone()));
        }
    }
    persist_storage_writes_core(&mut core, mod_id, &outcome.storage_writes);
    schedule_timers_core(&mut core, mod_id, now_secs, outcome);

    let side_effects = {
        let mut se = core
            .rpc_side_effects
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *se)
    };
    for (callee_id, callee_out) in side_effects {
        if let Some(m) = core.mods.get_mut(&callee_id) {
            m.total_fuel_used = m.total_fuel_used.saturating_add(callee_out.fuel_used);
        }
        for line in &callee_out.wasi_logs {
            if core.wasi_logs.len() < 256 {
                core.wasi_logs.push((callee_id.clone(), line.clone()));
            }
        }
        persist_storage_writes_core(&mut core, &callee_id, &callee_out.storage_writes);
        schedule_timers_core(&mut core, &callee_id, now_secs, &callee_out);
        if !callee_out.ops.is_empty() {
            queued.push((callee_id, callee_out.ops));
        }
    }
    queued
}

fn dispatch_hook_core(
    core_arc: &Arc<Mutex<ServerModHostCore>>,
    mut game: Option<&mut GameState>,
    eng: &mut EngineApi<'_>,
    hook: HookId,
    phase: HookPhase,
    event: &ActiveEvent,
) -> HookDispatchResult {
    let (depth_counter, subscribers, services) = {
        let core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
        let subs = core.hooks.subscribers(hook, phase);
        if subs.is_empty() {
            return HookDispatchResult::default();
        }
        (Arc::clone(&core.hook_depth), subs, core.services.clone())
    };

    let depth = depth_counter.fetch_add(1, Ordering::SeqCst);
    if depth >= MAX_HOOK_DEPTH {
        depth_counter.fetch_sub(1, Ordering::SeqCst);
        return HookDispatchResult::default();
    }

    let mut combined = HookDispatchResult::default();
    let snapshot = capture_snapshot(game.as_deref(), eng, &services);
    let now_secs = eng.time();

    for mod_id in subscribers {
        let target = {
            let core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
            core.mods.get(&mod_id).and_then(|m| {
                m.instance.clone().map(|inst| {
                    (
                        inst,
                        m.storage_cache.clone(),
                        m.package.manifest.fuel_per_frame,
                    )
                })
            })
        };
        let Some((inst_arc, storage_cache, fuel_budget)) = target else {
            continue;
        };

        let call_res = {
            let Ok(mut inst) = inst_arc.try_lock() else {
                continue;
            };
            let ctx = ModExecContext {
                snapshot: &snapshot,
                event,
                storage: &storage_cache,
                fuel_budget,
            };
            catch_mod_call(|| inst.call_hook(hook, phase, &ctx))
        };

        let outcome = match call_res {
            Ok(o) => o,
            Err(err) => {
                let mut core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
                record_and_fault_mod_core(
                    &mut core,
                    &mod_id,
                    InvocationTrigger::Hook { hook, phase },
                    event,
                    &snapshot,
                    &storage_cache,
                    fuel_budget,
                    err.to_string(),
                );
                continue;
            }
        };

        let side_ops = finalize_outcome_core(core_arc, &mod_id, now_secs, &outcome);
        for (callee_id, ops) in side_ops {
            apply_ops_core(core_arc, game.as_deref_mut(), eng, &callee_id, ops);
        }
        apply_ops_core(core_arc, game.as_deref_mut(), eng, &mod_id, outcome.ops);

        if outcome.action == HookAction::Override {
            combined.action = HookAction::Override;
            if outcome.override_i32.is_some() {
                combined.override_i32 = outcome.override_i32;
            }
            if outcome.override_i32_2.is_some() {
                combined.override_i32_2 = outcome.override_i32_2;
            }
            if outcome.override_f32.is_some() {
                combined.override_f32 = outcome.override_f32;
            }
            if outcome.override_str.is_some() {
                combined.override_str = outcome.override_str;
            }
        } else if outcome.action == HookAction::Supercede {
            combined.action = HookAction::Supercede;
            if outcome.override_i32.is_some() {
                combined.override_i32 = outcome.override_i32;
            }
            if outcome.override_i32_2.is_some() {
                combined.override_i32_2 = outcome.override_i32_2;
            }
            if outcome.override_f32.is_some() {
                combined.override_f32 = outcome.override_f32;
            }
            if outcome.override_str.is_some() {
                combined.override_str = outcome.override_str;
            }
            break;
        } else if outcome.action == HookAction::Handled && combined.action == HookAction::Continue {
            combined.action = HookAction::Handled;
        }
    }

    depth_counter.fetch_sub(1, Ordering::SeqCst);
    combined
}

fn tick_timers_core(
    core_arc: &Arc<Mutex<ServerModHostCore>>,
    mut game: Option<&mut GameState>,
    eng: &mut EngineApi<'_>,
) {
    let now = eng.time();
    let (due, services) = {
        let mut core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
        let mut due = Vec::new();
        let mut keep = Vec::with_capacity(core.timers.len());
        for mut t in core.timers.drain(..) {
            if t.due_secs <= now {
                due.push((t.mod_id.clone(), t.cb_id, t.arg));
                if let Some(interval) = t.interval_secs {
                    t.due_secs = now + interval;
                    keep.push(t);
                }
            } else {
                keep.push(t);
            }
        }
        core.timers = keep;
        (due, core.services.clone())
    };
    if due.is_empty() {
        return;
    }

    let snapshot = capture_snapshot(game.as_deref(), eng, &services);
    let ev = ActiveEvent::default();
    for (mod_id, cb_id, arg) in due {
        let target = {
            let core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
            core.mods.get(&mod_id).and_then(|m| {
                m.instance.clone().map(|inst| {
                    (
                        inst,
                        m.storage_cache.clone(),
                        m.package.manifest.fuel_per_frame,
                    )
                })
            })
        };
        let Some((inst_arc, storage_cache, fuel_budget)) = target else {
            continue;
        };
        let res = {
            let Ok(mut inst) = inst_arc.try_lock() else {
                continue;
            };
            let ctx = ModExecContext {
                snapshot: &snapshot,
                event: &ev,
                storage: &storage_cache,
                fuel_budget,
            };
            catch_mod_call(|| inst.call_timer(cb_id, arg, &ctx))
        };
        let outcome = match res {
            Ok(o) => o,
            Err(err) => {
                let mut core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
                record_and_fault_mod_core(
                    &mut core,
                    &mod_id,
                    InvocationTrigger::Timer { cb_id, arg },
                    &ev,
                    &snapshot,
                    &storage_cache,
                    fuel_budget,
                    err.to_string(),
                );
                continue;
            }
        };
        let side_ops = finalize_outcome_core(core_arc, &mod_id, now, &outcome);
        for (callee_id, ops) in side_ops {
            apply_ops_core(core_arc, game.as_deref_mut(), eng, &callee_id, ops);
        }
        apply_ops_core(core_arc, game.as_deref_mut(), eng, &mod_id, outcome.ops);
    }
}

fn poll_webhooks_core(
    core_arc: &Arc<Mutex<ServerModHostCore>>,
    mut game: Option<&mut GameState>,
    eng: &mut EngineApi<'_>,
) {
    let (results, services) = {
        let mut core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
        let results = core.webhooks.poll_results();
        (results, core.services.clone())
    };
    if results.is_empty() {
        return;
    }

    let now = eng.time();
    let snapshot = capture_snapshot(game.as_deref(), eng, &services);
    for result in results {
        let target = {
            let core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
            core.mods.get(&result.mod_id).and_then(|m| {
                m.instance.clone().map(|inst| {
                    (
                        inst,
                        m.storage_cache.clone(),
                        m.package.manifest.fuel_per_frame,
                    )
                })
            })
        };
        let Some((inst_arc, storage_cache, fuel_budget)) = target else {
            continue;
        };
        let ev = ActiveEvent {
            i32_args: [i32::from(result.status_code), result.callback_id, 0, 0, 0, 0],
            str_args: [
                result.body.clone(),
                result.error.clone().unwrap_or_default(),
                String::new(),
            ],
            payload: result.body.as_bytes().to_vec(),
            ..Default::default()
        };
        let res = {
            let Ok(mut inst) = inst_arc.try_lock() else {
                continue;
            };
            let ctx = ModExecContext {
                snapshot: &snapshot,
                event: &ev,
                storage: &storage_cache,
                fuel_budget,
            };
            catch_mod_call(|| {
                inst.call_webhook(result.callback_id, i32::from(result.status_code), &ctx)
            })
        };
        let outcome = match res {
            Ok(o) => o,
            Err(err) => {
                let mut core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
                record_and_fault_mod_core(
                    &mut core,
                    &result.mod_id,
                    InvocationTrigger::Webhook {
                        cb_id: result.callback_id,
                        status_code: i32::from(result.status_code),
                    },
                    &ev,
                    &snapshot,
                    &storage_cache,
                    fuel_budget,
                    err.to_string(),
                );
                continue;
            }
        };
        let side_ops = finalize_outcome_core(core_arc, &result.mod_id, now, &outcome);
        for (callee_id, ops) in side_ops {
            apply_ops_core(core_arc, game.as_deref_mut(), eng, &callee_id, ops);
        }
        apply_ops_core(core_arc, game.as_deref_mut(), eng, &result.mod_id, outcome.ops);
    }
}

#[allow(clippy::too_many_arguments)]
fn dispatch_menu_callback_core(
    core_arc: &Arc<Mutex<ServerModHostCore>>,
    mut game: Option<&mut GameState>,
    eng: &mut EngineApi<'_>,
    owner_id: &str,
    cb_id: i32,
    slot: u8,
    menu_id: u32,
    item_index: i32,
    info: &str,
) {
    let (target, services) = {
        let core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
        let t = core.mods.get(owner_id).and_then(|m| {
            m.instance.clone().map(|inst| {
                (
                    inst,
                    m.storage_cache.clone(),
                    m.package.manifest.fuel_per_frame,
                )
            })
        });
        (t, core.services.clone())
    };
    let Some((inst_arc, storage_cache, fuel_budget)) = target else {
        return;
    };
    let now = eng.time();
    let snapshot = capture_snapshot(game.as_deref(), eng, &services);
    let ev = ActiveEvent {
        i32_args: [i32::from(slot), menu_id as i32, item_index, cb_id, 0, 0],
        str_args: [info.to_string(), String::new(), String::new()],
        ..Default::default()
    };
    let res = {
        let Ok(mut inst) = inst_arc.try_lock() else {
            return;
        };
        let ctx = ModExecContext {
            snapshot: &snapshot,
            event: &ev,
            storage: &storage_cache,
            fuel_budget,
        };
        catch_mod_call(|| inst.call_menu(cb_id, slot, menu_id, item_index, &ctx))
    };
    let outcome = match res {
        Ok(o) => o,
        Err(err) => {
            let mut core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
            record_and_fault_mod_core(
                &mut core,
                owner_id,
                InvocationTrigger::MenuSelect {
                    cb_id,
                    slot,
                    menu_id,
                    item_index,
                    info: info.to_string(),
                },
                &ev,
                &snapshot,
                &storage_cache,
                fuel_budget,
                err.to_string(),
            );
            return;
        }
    };
    let side_ops = finalize_outcome_core(core_arc, owner_id, now, &outcome);
    for (callee_id, ops) in side_ops {
        apply_ops_core(core_arc, game.as_deref_mut(), eng, &callee_id, ops);
    }
    apply_ops_core(core_arc, game, eng, owner_id, outcome.ops);
}

fn dispatch_rpc_event_core(
    core_arc: &Arc<Mutex<ServerModHostCore>>,
    mut game: Option<&mut GameState>,
    eng: &mut EngineApi<'_>,
    event_name: &str,
    payload_json: &str,
    args: Vec<RpcValue>,
) {
    let (subs, services) = {
        let core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
        let mut svc = core.services.lock();
        svc.rpc.record_event(
            event_name.to_string(),
            payload_json.to_string(),
            args.clone(),
        );
        let subs = svc.rpc.subscribers(event_name);
        drop(svc);
        (subs, core.services.clone())
    };

    if !subs.is_empty() {
        let now = eng.time();
        let snapshot = capture_snapshot(game.as_deref(), eng, &services);
        let ev = ActiveEvent {
            str_args: [
                event_name.to_string(),
                payload_json.to_string(),
                String::new(),
            ],
            payload: payload_json.as_bytes().to_vec(),
            ..Default::default()
        };

        for sub in subs {
            if !matches!(
                sub.runtime,
                PluginRuntimeKind::Wasm | PluginRuntimeKind::Python
            ) {
                continue;
            }
            let target = {
                let core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
                core.mods.get(&sub.owner_id).and_then(|m| {
                    m.instance.clone().map(|inst| {
                        (
                            inst,
                            m.storage_cache.clone(),
                            m.package.manifest.fuel_per_frame,
                        )
                    })
                })
            };
            let Some((inst_arc, storage_cache, fuel_budget)) = target else {
                continue;
            };
            let res = {
                let Ok(mut inst) = inst_arc.try_lock() else {
                    continue;
                };
                let ctx = ModExecContext {
                    snapshot: &snapshot,
                    event: &ev,
                    storage: &storage_cache,
                    fuel_budget,
                };
                catch_mod_call(|| inst.call_event(sub.callback_id, &ctx))
            };
            let outcome = match res {
                Ok(o) => o,
                Err(err) => {
                    let mut core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
                    record_and_fault_mod_core(
                        &mut core,
                        &sub.owner_id,
                        InvocationTrigger::RpcEvent {
                            cb_id: sub.callback_id,
                            event_name: event_name.to_string(),
                        },
                        &ev,
                        &snapshot,
                        &storage_cache,
                        fuel_budget,
                        err.to_string(),
                    );
                    continue;
                }
            };
            let side_ops = finalize_outcome_core(core_arc, &sub.owner_id, now, &outcome);
            for (callee_id, ops) in side_ops {
                apply_ops_core(core_arc, game.as_deref_mut(), eng, &callee_id, ops);
            }
            apply_ops_core(core_arc, game.as_deref_mut(), eng, &sub.owner_id, outcome.ops);
        }
    }

    let _ = with_amxx_core(core_arc, game, eng, |amxx, eng, hooks| {
        amxx.dispatch_rpc_event(eng, hooks, event_name, &args);
    });
}

fn apply_ops_core(
    core_arc: &Arc<Mutex<ServerModHostCore>>,
    mut game: Option<&mut GameState>,
    eng: &mut EngineApi<'_>,
    mod_id: &str,
    ops: Vec<EngineOp>,
) {
    for op in ops {
        match op {
            EngineOp::RegisterCommand { name } => {
                let mut core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(m) = core.mods.get_mut(mod_id) {
                    m.registered_commands.insert(name);
                }
            }
            EngineOp::CreateEntity { classname } => {
                let ev = ActiveEvent {
                    str_args: [classname.clone(), String::new(), String::new()],
                    ..Default::default()
                };
                let res = dispatch_hook_core(
                    core_arc,
                    game.as_deref_mut(),
                    eng,
                    HookId::EngCreateEntity,
                    HookPhase::Pre,
                    &ev,
                );
                if res.action == HookAction::Supercede {
                    continue;
                }
                let created = eng.create_entity(&classname);
                let ev_post = ActiveEvent {
                    i32_args: [created.map_or(-1, |id| i32::from(id.index)), 0, 0, 0, 0, 0],
                    str_args: [classname, String::new(), String::new()],
                    ..Default::default()
                };
                let _ = dispatch_hook_core(
                    core_arc,
                    game.as_deref_mut(),
                    eng,
                    HookId::EngCreateEntity,
                    HookPhase::Post,
                    &ev_post,
                );
            }
            EngineOp::RemoveEntity { index } => {
                let ev = ActiveEvent {
                    i32_args: [i32::from(index), 0, 0, 0, 0, 0],
                    ..Default::default()
                };
                let res = dispatch_hook_core(
                    core_arc,
                    game.as_deref_mut(),
                    eng,
                    HookId::EngRemoveEntity,
                    HookPhase::Pre,
                    &ev,
                );
                if res.action == HookAction::Supercede {
                    continue;
                }
                if let Some(id) = resolve_edict(eng, index) {
                    eng.remove_entity(id);
                }
            }
            EngineOp::SetHealth { index, health } => {
                if let Some(id) = resolve_edict(eng, index)
                    && let Some(vars) = eng.ent_mut(id)
                {
                    vars.health = health;
                }
            }
            EngineOp::SetOrigin { index, origin } => {
                if let Some(id) = resolve_edict(eng, index)
                    && let Some(vars) = eng.ent_mut(id)
                {
                    vars.origin = Vec3::from_array(origin);
                }
            }
            EngineOp::SetVelocity { index, velocity } => {
                if let Some(id) = resolve_edict(eng, index)
                    && let Some(vars) = eng.ent_mut(id)
                {
                    vars.velocity = Vec3::from_array(velocity);
                }
            }
            EngineOp::SetFlags { index, flags } => {
                if let Some(id) = resolve_edict(eng, index)
                    && let Some(vars) = eng.ent_mut(id)
                {
                    vars.flags = flags;
                }
            }
            EngineOp::PrintClient { slot, text } => {
                let ev = ActiveEvent {
                    i32_args: [i32::from(slot), 0, 0, 0, 0, 0],
                    str_args: [text.clone(), String::new(), String::new()],
                    ..Default::default()
                };
                let res = dispatch_hook_core(
                    core_arc,
                    game.as_deref_mut(),
                    eng,
                    HookId::EngClientPrint,
                    HookPhase::Pre,
                    &ev,
                );
                if res.action == HookAction::Supercede {
                    continue;
                }
                eng.client_print(slot, &text);
            }
            EngineOp::PrintAll { text } => {
                eng.print_all(&text);
            }
            EngineOp::CenterPrint { slot, text } => {
                let ev = ActiveEvent {
                    i32_args: [slot.map_or(-1, i32::from), 0, 0, 0, 0, 0],
                    str_args: [text.clone(), String::new(), String::new()],
                    ..Default::default()
                };
                let res = dispatch_hook_core(
                    core_arc,
                    game.as_deref_mut(),
                    eng,
                    HookId::EngCenterPrint,
                    HookPhase::Pre,
                    &ev,
                );
                if res.action == HookAction::Supercede {
                    continue;
                }
                match slot {
                    Some(s) => eng.center_print(s, &text),
                    None => {
                        for s in 0..eng.max_clients() {
                            if eng.is_client_active(s) {
                                eng.center_print(s, &text);
                            }
                        }
                    }
                }
            }
            EngineOp::SayText { slot, text } => {
                let msg = CsUserMsg::SayText {
                    sender_id: 0,
                    text,
                    params: Vec::new(),
                };
                let dest = slot.map_or(MsgDest::All, MsgDest::One);
                let _ = eng.send_usermsg(dest, &msg);
            }
            EngineOp::ShowMenu {
                slot,
                keys,
                time_sec,
                text,
            } => {
                let msg = CsUserMsg::ShowMenu {
                    keys_bitmask: keys,
                    display_time: time_sec,
                    multipart: false,
                    text,
                };
                let _ = eng.send_usermsg(MsgDest::One(slot), &msg);
            }
            EngineOp::SendCustomUserMsg { slot, name, data } => {
                let ev = ActiveEvent {
                    i32_args: [slot.map_or(-1, i32::from), 0, 0, 0, 0, 0],
                    str_args: [name.clone(), String::new(), String::new()],
                    payload: data.clone(),
                    ..Default::default()
                };
                let res = dispatch_hook_core(
                    core_arc,
                    game.as_deref_mut(),
                    eng,
                    HookId::EngSendUserMsg,
                    HookPhase::Pre,
                    &ev,
                );
                if res.action == HookAction::Supercede {
                    continue;
                }
                let dest = slot.map_or(MsgDest::All, MsgDest::One);
                let _ = eng.send_mod_usermsg(dest, &name, &data);
            }
            EngineOp::SetCvar { name, value } => {
                eng.set_cvar(&name, &value);
            }
            EngineOp::EmitSound {
                ent_index,
                channel,
                sample,
                volume,
            } => {
                let ev = ActiveEvent {
                    i32_args: [i32::from(ent_index), i32::from(channel), 0, 0, 0, 0],
                    f32_args: [volume, 0.0, 0.0, 0.0],
                    str_args: [sample.clone(), String::new(), String::new()],
                    ..Default::default()
                };
                let res = dispatch_hook_core(
                    core_arc,
                    game.as_deref_mut(),
                    eng,
                    HookId::EngEmitSound,
                    HookPhase::Pre,
                    &ev,
                );
                if res.action == HookAction::Supercede {
                    continue;
                }
                if let Some(id) = resolve_edict(eng, ent_index) {
                    let _ = eng.emit_sound(id, channel, &sample, volume, 0.8, 100);
                }
            }
            EngineOp::PrecacheSound { sample } => {
                let _ = eng.precache_sound(&sample);
            }
            EngineOp::Kick { slot, reason } => {
                let ev = ActiveEvent {
                    i32_args: [i32::from(slot), 0, 0, 0, 0, 0],
                    str_args: [reason.clone(), String::new(), String::new()],
                    ..Default::default()
                };
                let res = dispatch_hook_core(
                    core_arc,
                    game.as_deref_mut(),
                    eng,
                    HookId::EngKick,
                    HookPhase::Pre,
                    &ev,
                );
                if res.action == HookAction::Supercede {
                    continue;
                }
                eng.kick(slot, &reason);
            }
            EngineOp::ChangeLevel { map } => {
                let ev = ActiveEvent {
                    str_args: [map.clone(), String::new(), String::new()],
                    ..Default::default()
                };
                let res = dispatch_hook_core(
                    core_arc,
                    game.as_deref_mut(),
                    eng,
                    HookId::EngChangeLevel,
                    HookPhase::Pre,
                    &ev,
                );
                if res.action == HookAction::Supercede {
                    continue;
                }
                let _ = eng.change_level(&map);
            }
            EngineOp::ServerExec { line } => {
                if let Some(rest) = line.strip_prefix("changelevel ") {
                    let _ = eng.change_level(rest.trim());
                } else if let Some((k, v)) = line.split_once(' ') {
                    eng.set_cvar(k.trim(), v.trim());
                }
            }
            EngineOp::ClientExec { .. } => {}

            // --- ABI v2 CS / Fun / Engine / CStrike Natives ---
            EngineOp::SetPlayerHealth { slot, health } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::set_player_health(g, eng, slot, health);
                } else {
                    let id = eng.client_edict(slot);
                    if let Some(v) = eng.ent_mut(id) {
                        v.health = health;
                    }
                }
            }
            EngineOp::SetPlayerMaxHealth { slot, max_health } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::set_player_max_health(g, slot, max_health);
                }
            }
            EngineOp::SetPlayerArmor { slot, armor, kind } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::set_player_armor(g, eng, slot, armor, kind);
                }
            }
            EngineOp::SetPlayerMoney {
                slot,
                amount,
                flash,
            } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::set_player_money(g, eng, slot, amount, flash);
                }
            }
            EngineOp::AddPlayerMoney { slot, delta, flash } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::add_player_money(g, eng, slot, delta, flash);
                }
            }
            EngineOp::SetPlayerScore {
                slot,
                frags,
                deaths,
            } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::set_player_score(g, eng, slot, frags, deaths);
                }
            }
            EngineOp::SetPlayerTeam {
                slot,
                team,
                model_class,
            } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::set_player_team(g, eng, slot, team, model_class);
                }
            }
            EngineOp::RespawnPlayer { slot } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::respawn_player(g, eng, slot);
                }
            }
            EngineOp::SlapPlayer {
                slot,
                power,
                random_dir,
            } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::slap_player(g, eng, slot, power, random_dir);
                }
            }
            EngineOp::SlayPlayer {
                slot,
                preserve_frags,
            } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::slay_player(g, eng, slot, preserve_frags);
                }
            }
            EngineOp::DamagePlayer {
                victim,
                attacker,
                damage,
                bits,
            } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::apply_damage(g, eng, victim, attacker, damage, bits);
                }
            }
            EngineOp::SetPlayerGodmode { slot, enabled } => {
                let _ = cs_hooks::set_player_godmode(eng, slot, enabled);
            }
            EngineOp::SetPlayerNoclip { slot, enabled } => {
                let _ = cs_hooks::set_player_noclip(eng, slot, enabled);
            }
            EngineOp::SetPlayerMaxspeed { slot, maxspeed } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::set_player_maxspeed(g, eng, slot, maxspeed);
                } else {
                    let id = eng.client_edict(slot);
                    if let Some(v) = eng.ent_mut(id) {
                        v.max_speed = maxspeed;
                    }
                }
            }
            EngineOp::SetPlayerGravity { slot, gravity } => {
                let _ = cs_hooks::set_player_gravity(eng, slot, gravity);
            }
            EngineOp::SetPlayerFootsteps { slot, silent } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::set_player_footsteps(g, slot, silent);
                }
            }
            EngineOp::SetPlayerRendering {
                slot,
                fx,
                rgb,
                mode,
                amount,
            } => {
                let _ = cs_hooks::set_player_rendering(eng, slot, fx, rgb, mode, amount);
            }
            EngineOp::SetPlayerModel { slot, model } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::set_player_model(g, eng, slot, &model);
                }
            }
            EngineOp::ResetPlayerModel { slot } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::reset_player_model(g, eng, slot);
                }
            }
            EngineOp::GiveItem { slot, item } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::give_item(g, eng, slot, &item);
                }
            }
            EngineOp::StripUserWeapons { slot } => {
                if let Some(g) = game.as_deref_mut() {
                    cs_hooks::strip_user_weapons(g, eng, slot);
                }
            }
            EngineOp::SetWeaponClip {
                slot,
                weapon_id,
                clip,
            } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::set_weapon_clip(g, slot, weapon_id, clip);
                }
            }
            EngineOp::SetUserBpAmmo {
                slot,
                weapon_id,
                amount,
            } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::set_user_bpammo(g, eng, slot, weapon_id, amount);
                }
            }
            EngineOp::SetUserDefuser { slot, has } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::set_user_defuser(g, eng, slot, has);
                }
            }
            EngineOp::SetUserNvg { slot, has } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::set_user_nvg(g, eng, slot, has);
                }
            }
            EngineOp::DropSlot { slot, inv_slot } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::drop_slot(g, eng, slot, inv_slot);
                }
            }
            EngineOp::RegisterCustomWeapon(spec) => {
                if let Some(g) = game.as_deref_mut() {
                    cs_hooks::register_custom_weapon(g, Some(eng), spec);
                } else {
                    let mut core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
                    core.pending_custom_weapons.push(spec);
                }
            }
            EngineOp::GiveCustomWeapon { slot, id_name } => {
                if let Some(g) = game.as_deref_mut() {
                    let _ = cs_hooks::give_custom_weapon(g, eng, slot, &id_name);
                }
            }
            EngineOp::EmitVfx(vfx) => {
                cs_hooks::emit_vfx(eng, &vfx);
            }
            EngineOp::MenuDisplay(spec) => {
                let rendered = {
                    let core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
                    let mut svc = core.services.lock();
                    svc.menus
                        .display(spec.slot, spec.menu_id, spec.page, spec.time_sec)
                };
                if let Some(rendered) = rendered {
                    send_rendered_menu(eng, &rendered);
                }
            }
            EngineOp::HudMessage(spec) => {
                cs_hooks::send_hud_message(eng, &spec);
            }
            EngineOp::DhudMessage(spec) => {
                cs_hooks::send_dhud_message(eng, &spec);
            }
            EngineOp::ShowMotd(spec) => {
                cs_hooks::send_motd_window(eng, &spec);
            }
            EngineOp::ScreenFade(spec) => {
                cs_hooks::send_screen_fade(eng, &spec);
            }
            EngineOp::ScreenShake(spec) => {
                cs_hooks::send_screen_shake(eng, &spec);
            }
            EngineOp::BarTime(spec) => {
                if let Some(g) = game.as_deref_mut() {
                    cs_hooks::send_bar_time(g, eng, &spec);
                } else {
                    let _ = eng.send_usermsg(
                        MsgDest::One(spec.slot),
                        &CsUserMsg::BarTime(spec.seconds),
                    );
                }
            }
            EngineOp::RpcEmitEvent(ev) => {
                dispatch_rpc_event_core(
                    core_arc,
                    game.as_deref_mut(),
                    eng,
                    &ev.event_name,
                    &ev.payload_json,
                    ev.args,
                );
            }
            EngineOp::HttpPostJson(spec) => {
                let mut core = core_arc.lock().unwrap_or_else(|e| e.into_inner());
                let allowed = core
                    .mods
                    .get(mod_id)
                    .map(|m| m.package.manifest.http_hosts.clone())
                    .unwrap_or_default();
                if let Err(err) = core.webhooks.enqueue(mod_id, &allowed, spec) {
                    warn!(mod_id, %err, "rejected webhook request");
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct HookDispatchResult {
    pub action: HookAction,
    pub override_i32: Option<i32>,
    pub override_i32_2: Option<i32>,
    pub override_f32: Option<f32>,
    pub override_str: Option<String>,
}

impl Default for HookDispatchResult {
    fn default() -> Self {
        Self {
            action: HookAction::Continue,
            override_i32: None,
            override_i32_2: None,
            override_f32: None,
            override_str: None,
        }
    }
}

/// [`CsHookSink`] installed into [`CsGame`] by [`ModdedGame<CsGame>`] to dispatch every ReAPI
/// Pre/Post gameplay hook into [`ServerModHost`].
struct ServerModHostSink {
    core: Arc<Mutex<ServerModHostCore>>,
}

impl CsHookSink for ServerModHostSink {
    fn on_player_spawn(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut PlayerSpawnCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [i32::from(ctx.slot), 0, 0, 0, 0, 0],
            ..Default::default()
        };
        let wasm_act =
            dispatch_hook_core(&self.core, Some(g), eng, HookId::PlayerSpawn, phase, &ev).action;
        if phase == HookPhase::Pre && wasm_act == HookAction::Supercede {
            return HookAction::Supercede;
        }
        let post = phase == HookPhase::Post;
        let amxx_ret = with_amxx_core(&self.core, Some(g), eng, |amxx, eng, hooks| {
            amxx.dispatch_ham_spawn(eng, hooks, u16::from(ctx.slot) + 1, "player", post)
        })
        .unwrap_or(0);
        if !post && amxx_ret >= HAM_SUPERCEDE {
            return HookAction::Supercede;
        }
        wasm_act
    }

    fn on_player_take_damage(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut PlayerTakeDamageCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [
                i32::from(ctx.victim),
                ctx.attacker.map_or(-1, i32::from),
                i32::from(ctx.hitgroup),
                i32::from(ctx.weapon_id),
                ctx.damage_bits as i32,
                i32::from(ctx.team_attack),
            ],
            f32_args: [ctx.damage, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        let res = dispatch_hook_core(
            &self.core,
            Some(g),
            eng,
            HookId::PlayerTakeDamage,
            phase,
            &ev,
        );
        let mut overridden = false;
        if phase == HookPhase::Pre {
            if res.action == HookAction::Supercede {
                return HookAction::Supercede;
            }
            if res.action == HookAction::Override {
                overridden = true;
                if let Some(dmg) = res.override_f32 {
                    ctx.damage = dmg.max(0.0);
                }
                if let Some(hg) = res.override_i32 {
                    ctx.hitgroup = hg.clamp(0, 15) as u8;
                }
            }
        }
        let post = phase == HookPhase::Post;
        let victim_ent = u16::from(ctx.victim) + 1;
        let attacker_ent = ctx.attacker.map_or(0, |a| u16::from(a) + 1);
        let amxx_res = with_amxx_core(&self.core, Some(g), eng, |amxx, eng, hooks| {
            let r = amxx.dispatch_ham_take_damage_mut(
                eng,
                hooks,
                victim_ent,
                "player",
                attacker_ent,
                attacker_ent,
                ctx.damage,
                ctx.damage_bits as i32,
                post,
            );
            if post {
                amxx.dispatch_client_damage(
                    eng,
                    hooks,
                    ctx.attacker,
                    ctx.victim,
                    ctx.damage.round() as i32,
                    i32::from(ctx.weapon_id),
                    i32::from(ctx.hitgroup),
                    ctx.team_attack,
                );
            }
            r
        });
        if !post
            && let Some((ham_ret, new_dmg, new_bits)) = amxx_res
        {
            if ham_ret >= HAM_SUPERCEDE {
                return HookAction::Supercede;
            }
            if let Some(d) = new_dmg {
                ctx.damage = d.max(0.0);
                overridden = true;
            }
            if let Some(b) = new_bits {
                ctx.damage_bits = b as u32;
                overridden = true;
            }
        }
        if overridden {
            HookAction::Override
        } else {
            res.action
        }
    }

    fn on_player_killed(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut PlayerKilledCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [
                i32::from(ctx.victim),
                ctx.killer.map_or(-1, i32::from),
                i32::from(ctx.weapon_id),
                i32::from(ctx.hitgroup),
                i32::from(ctx.headshot),
                i32::from(ctx.team_kill),
            ],
            str_args: [ctx.weapon_name.clone(), String::new(), String::new()],
            ..Default::default()
        };
        let res = dispatch_hook_core(&self.core, Some(g), eng, HookId::PlayerKilled, phase, &ev);
        if phase == HookPhase::Pre && res.action == HookAction::Supercede {
            return HookAction::Supercede;
        }
        if phase == HookPhase::Post {
            let _ = dispatch_hook_core(&self.core, Some(g), eng, HookId::PlayerDeath, phase, &ev);
        }
        let post = phase == HookPhase::Post;
        let victim_ent = u16::from(ctx.victim) + 1;
        let killer_ent = ctx.killer.map_or(0, |k| u16::from(k) + 1);
        let amxx_ret = with_amxx_core(&self.core, Some(g), eng, |amxx, eng, hooks| {
            let r = amxx.dispatch_ham_killed(
                eng,
                hooks,
                victim_ent,
                "player",
                killer_ent,
                0,
                post,
            );
            if post {
                amxx.dispatch_client_death(
                    eng,
                    hooks,
                    ctx.killer,
                    ctx.victim,
                    i32::from(ctx.weapon_id),
                    i32::from(ctx.hitgroup),
                    ctx.team_kill,
                );
            }
            r
        })
        .unwrap_or(0);
        if !post && amxx_ret >= HAM_SUPERCEDE {
            return HookAction::Supercede;
        }
        res.action
    }

    fn on_player_pre_think(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut PlayerPreThinkCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [
                i32::from(ctx.slot),
                i32::from(ctx.buttons),
                i32::from(ctx.oldbuttons),
                0,
                0,
                0,
            ],
            ..Default::default()
        };
        dispatch_hook_core(&self.core, Some(g), eng, HookId::PlayerPreThink, phase, &ev).action
    }

    fn on_player_post_think(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut PlayerPostThinkCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [i32::from(ctx.slot), 0, 0, 0, 0, 0],
            ..Default::default()
        };
        dispatch_hook_core(
            &self.core,
            Some(g),
            eng,
            HookId::PlayerPostThink,
            phase,
            &ev,
        )
        .action
    }

    fn on_player_jump(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut PlayerJumpCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [i32::from(ctx.slot), 0, 0, 0, 0, 0],
            ..Default::default()
        };
        let wasm_act =
            dispatch_hook_core(&self.core, Some(g), eng, HookId::PlayerJump, phase, &ev).action;
        if phase == HookPhase::Pre && wasm_act == HookAction::Supercede {
            return HookAction::Supercede;
        }
        let post = phase == HookPhase::Post;
        let amxx_ret = with_amxx_core(&self.core, Some(g), eng, |amxx, eng, hooks| {
            amxx.dispatch_ham_player_jump(eng, hooks, u16::from(ctx.slot) + 1, post)
        })
        .unwrap_or(0);
        if !post && amxx_ret >= HAM_SUPERCEDE {
            return HookAction::Supercede;
        }
        wasm_act
    }

    fn on_players_should_collide(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        ctx: &mut PlayersShouldCollideCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [
                i32::from(ctx.slot_a),
                i32::from(ctx.slot_b),
                i32::from(ctx.should_collide),
                0,
                0,
                0,
            ],
            ..Default::default()
        };
        let res = dispatch_hook_core(
            &self.core,
            Some(g),
            eng,
            HookId::PlayersShouldCollide,
            HookPhase::Pre,
            &ev,
        );
        if matches!(res.action, HookAction::Override | HookAction::Supercede) {
            if let Some(v) = res.override_i32 {
                ctx.should_collide = v != 0;
            } else if res.action == HookAction::Supercede {
                ctx.should_collide = false;
            }
        }
        res.action
    }

    fn on_can_buy_item(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut CanBuyItemCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [i32::from(ctx.slot), i32::from(ctx.item_id), ctx.price, 0, 0, 0],
            ..Default::default()
        };
        let res = dispatch_hook_core(&self.core, Some(g), eng, HookId::CanBuyItem, phase, &ev);
        if phase == HookPhase::Pre
            && res.action == HookAction::Override
            && let Some(new_price) = res.override_i32
        {
            ctx.price = new_price.max(0);
        }
        res.action
    }

    fn on_can_have_player_item(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut CanHavePlayerItemCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [i32::from(ctx.slot), i32::from(ctx.weapon_id), 0, 0, 0, 0],
            ..Default::default()
        };
        dispatch_hook_core(
            &self.core,
            Some(g),
            eng,
            HookId::CanHavePlayerItem,
            phase,
            &ev,
        )
        .action
    }

    fn on_weapon_deploy(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut WeaponEventCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [i32::from(ctx.slot), i32::from(ctx.weapon_id), 0, 0, 0, 0],
            ..Default::default()
        };
        let wasm_act =
            dispatch_hook_core(&self.core, Some(g), eng, HookId::WeaponDeploy, phase, &ev).action;
        if phase == HookPhase::Pre && wasm_act == HookAction::Supercede {
            return HookAction::Supercede;
        }
        let post = phase == HookPhase::Post;
        let cls = weapon_id_to_classname(ctx.weapon_id);
        let amxx_ret = with_amxx_core(&self.core, Some(g), eng, |amxx, eng, hooks| {
            amxx.dispatch_ham_item_deploy(eng, hooks, u16::from(ctx.slot) + 1, cls, post)
        })
        .unwrap_or(0);
        if !post && amxx_ret >= HAM_SUPERCEDE {
            return HookAction::Supercede;
        }
        wasm_act
    }

    fn on_weapon_primary_attack(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut WeaponEventCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [i32::from(ctx.slot), i32::from(ctx.weapon_id), 0, 0, 0, 0],
            ..Default::default()
        };
        let wasm_act = dispatch_hook_core(
            &self.core,
            Some(g),
            eng,
            HookId::WeaponPrimaryAttack,
            phase,
            &ev,
        )
        .action;
        if phase == HookPhase::Pre && wasm_act == HookAction::Supercede {
            return HookAction::Supercede;
        }
        let post = phase == HookPhase::Post;
        let cls = weapon_id_to_classname(ctx.weapon_id);
        let amxx_ret = with_amxx_core(&self.core, Some(g), eng, |amxx, eng, hooks| {
            amxx.dispatch_ham_weapon_primary_attack(eng, hooks, u16::from(ctx.slot) + 1, cls, post)
        })
        .unwrap_or(0);
        if !post && amxx_ret >= HAM_SUPERCEDE {
            return HookAction::Supercede;
        }
        wasm_act
    }

    fn on_weapon_reload(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut WeaponEventCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [i32::from(ctx.slot), i32::from(ctx.weapon_id), 0, 0, 0, 0],
            ..Default::default()
        };
        dispatch_hook_core(&self.core, Some(g), eng, HookId::WeaponReload, phase, &ev).action
    }

    fn on_player_say(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut PlayerSayCtx,
    ) -> HookAction {
        if phase == HookPhase::Pre {
            let handled_lang = {
                let mut core = self.core.lock().unwrap_or_else(|e| e.into_inner());
                let services = core.services.clone();
                let mut svc = services.lock();
                handle_lang_chat_command(
                    &mut svc.lang,
                    eng,
                    ctx.slot,
                    &ctx.message,
                    &mut *core.storage,
                )
            };
            if handled_lang {
                return HookAction::Supercede;
            }
        }
        let ev = ActiveEvent {
            i32_args: [i32::from(ctx.slot), i32::from(ctx.team_only), 0, 0, 0, 0],
            str_args: [ctx.message.clone(), String::new(), String::new()],
            ..Default::default()
        };
        let res = dispatch_hook_core(&self.core, Some(g), eng, HookId::PlayerSay, phase, &ev);
        if phase == HookPhase::Pre
            && res.action == HookAction::Override
            && let Some(rewritten) = res.override_str
        {
            ctx.message = rewritten;
        }
        res.action
    }

    fn on_menu_select(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut MenuSelectCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [
                i32::from(ctx.slot),
                ctx.menu_id as i32,
                ctx.item_index as i32,
                0,
                0,
                0,
            ],
            str_args: [ctx.info.clone(), String::new(), String::new()],
            ..Default::default()
        };
        dispatch_hook_core(&self.core, Some(g), eng, HookId::MenuSelect, phase, &ev).action
    }

    fn on_freeze_time_end(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
    ) -> HookAction {
        let ev = ActiveEvent::default();
        dispatch_hook_core(
            &self.core,
            Some(g),
            eng,
            HookId::FreezeTimeEnd,
            phase,
            &ev,
        )
        .action
    }

    fn on_round_start(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut RoundStartCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [ctx.round_num as i32, 0, 0, 0, 0, 0],
            ..Default::default()
        };
        dispatch_hook_core(&self.core, Some(g), eng, HookId::RoundStart, phase, &ev).action
    }

    fn on_round_end(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut RoundEndCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [i32::from(ctx.winner), i32::from(ctx.reason), ctx.bonus, 0, 0, 0],
            ..Default::default()
        };
        dispatch_hook_core(&self.core, Some(g), eng, HookId::RoundEnd, phase, &ev).action
    }

    fn on_bomb_planting(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut BombEventCtx,
    ) -> HookAction {
        dispatch_bomb_hook(&self.core, g, eng, HookId::BombPlanting, phase, ctx)
    }

    fn on_bomb_planted(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut BombEventCtx,
    ) -> HookAction {
        dispatch_bomb_hook(&self.core, g, eng, HookId::BombPlanted, phase, ctx)
    }

    fn on_bomb_defusing(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut BombEventCtx,
    ) -> HookAction {
        dispatch_bomb_hook(&self.core, g, eng, HookId::BombDefusing, phase, ctx)
    }

    fn on_bomb_defused(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut BombEventCtx,
    ) -> HookAction {
        dispatch_bomb_hook(&self.core, g, eng, HookId::BombDefused, phase, ctx)
    }

    fn on_bomb_exploded(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut BombEventCtx,
    ) -> HookAction {
        dispatch_bomb_hook(&self.core, g, eng, HookId::BombExploded, phase, ctx)
    }

    fn on_hostage_touched(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut HostageEventCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [i32::from(ctx.slot), i32::from(ctx.ent_index), 0, 0, 0, 0],
            ..Default::default()
        };
        dispatch_hook_core(&self.core, Some(g), eng, HookId::HostageTouched, phase, &ev).action
    }

    fn on_hostage_rescued(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut HostageEventCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [i32::from(ctx.slot), i32::from(ctx.ent_index), 0, 0, 0, 0],
            ..Default::default()
        };
        dispatch_hook_core(&self.core, Some(g), eng, HookId::HostageRescued, phase, &ev).action
    }

    fn on_grenade_throw(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        ctx: &mut GrenadeThrowCtx,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [
                i32::from(ctx.slot),
                i32::from(ctx.weapon_id),
                i32::from(ctx.ent_index),
                0,
                0,
                0,
            ],
            ..Default::default()
        };
        let act =
            dispatch_hook_core(&self.core, Some(g), eng, HookId::GrenadeThrow, phase, &ev).action;
        if phase == HookPhase::Post {
            let _ = with_amxx_core(&self.core, Some(g), eng, |amxx, eng, hooks| {
                amxx.dispatch_grenade_throw(eng, hooks, ctx.slot, ctx.ent_index, ctx.weapon_id);
            });
        }
        act
    }

    fn on_entity_touch(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        phase: HookPhase,
        toucher: EdictId,
        other: EdictId,
    ) -> HookAction {
        let ev = ActiveEvent {
            i32_args: [i32::from(toucher.index), i32::from(other.index), 0, 0, 0, 0],
            ..Default::default()
        };
        dispatch_hook_core(&self.core, Some(g), eng, HookId::Touch, phase, &ev).action
    }
}

fn dispatch_bomb_hook(
    core: &Arc<Mutex<ServerModHostCore>>,
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    hook: HookId,
    phase: HookPhase,
    ctx: &BombEventCtx,
) -> HookAction {
    let ev = ActiveEvent {
        i32_args: [
            ctx.slot.map_or(-1, i32::from),
            i32::from(ctx.has_kit),
            0,
            0,
            0,
            0,
        ],
        f32_args: [ctx.origin[0], ctx.origin[1], ctx.origin[2], 0.0],
        ..Default::default()
    };
    let act = dispatch_hook_core(core, Some(g), eng, hook, phase, &ev).action;
    if phase == HookPhase::Post {
        let _ = with_amxx_core(core, Some(g), eng, |amxx, eng, hooks| match hook {
            HookId::BombPlanting => {
                if let Some(s) = ctx.slot {
                    amxx.dispatch_bomb_planting(eng, hooks, s);
                }
            }
            HookId::BombPlanted => {
                if let Some(s) = ctx.slot {
                    amxx.dispatch_bomb_planted(eng, hooks, s);
                }
            }
            HookId::BombDefusing => {
                if let Some(s) = ctx.slot {
                    amxx.dispatch_bomb_defusing(eng, hooks, s);
                }
            }
            HookId::BombDefused => {
                if let Some(s) = ctx.slot {
                    amxx.dispatch_bomb_defused(eng, hooks, s);
                }
            }
            HookId::BombExploded => {
                amxx.dispatch_bomb_explode(eng, hooks, ctx.slot, None);
            }
            _ => {}
        });
    }
    act
}

/// [`GameDll`] wrapper weaving [`ServerModHost`] pre/post hookchains (and automatic
/// [`CsHookSink`] attachment when wrapping [`CsGame`]) around any inner game.
pub struct ModdedGame<G: GameDll + 'static> {
    pub inner: G,
    pub host: ServerModHost,
    was_client_command_claimed: bool,
    pending_catalog_slots: BTreeSet<u8>,
}

impl<G: GameDll + 'static> ModdedGame<G> {
    pub fn new(mut inner: G, host: ServerModHost) -> Self {
        if let Some(cs_game) = (&mut inner as &mut dyn Any).downcast_mut::<CsGame>() {
            cs_game.set_hook_sink(Box::new(ServerModHostSink {
                core: host.core_arc(),
            }));
        }
        Self {
            inner,
            host,
            was_client_command_claimed: false,
            pending_catalog_slots: BTreeSet::new(),
        }
    }

    fn cs_state_mut(&mut self) -> Option<&mut GameState> {
        (&mut self.inner as &mut dyn Any)
            .downcast_mut::<CsGame>()
            .map(CsGame::state_mut)
    }

    fn is_cs_game(&self) -> bool {
        (&self.inner as &dyn Any).is::<CsGame>()
    }

    fn send_client_catalog(&self, eng: &mut EngineApi<'_>, slot: u8) {
        if !eng.client_supports_mods(slot) {
            return;
        }
        let catalog = self.host.client_catalog();
        let total = u8::try_from(catalog.len()).unwrap_or(u8::MAX);
        for (idx, (entry, _)) in catalog.into_iter().take(usize::from(u8::MAX)).enumerate() {
            let pkt = ModManifestPacket {
                mod_index: idx as u8,
                total_mods: total,
                entry,
            };
            if let Some(payload) = pkt.encode() {
                let _ = eng.send_mod_usermsg(MsgDest::One(slot), USERMSG_MOD_MANIFEST, &payload);
            }
        }
    }

    fn flush_client_chunk_requests(&mut self, eng: &mut EngineApi<'_>) {
        let reqs = {
            let mut core = self.host.core.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut core.pending_client_requests)
        };
        if reqs.is_empty() {
            return;
        }
        let catalog = self.host.client_catalog();
        for (slot, mod_index, offset) in reqs {
            if !eng.client_supports_mods(slot) {
                continue;
            }
            let Some((_, bundle)) = catalog.get(usize::from(mod_index)) else {
                continue;
            };
            let off = offset as usize;
            if off >= bundle.len() {
                continue;
            }
            let end = (off + cs_mod_api::MOD_CHUNK_MAX_DATA).min(bundle.len());
            let pkt = ModChunkPacket {
                mod_index,
                offset,
                data: bundle[off..end].to_vec(),
            };
            if let Some(payload) = pkt.encode() {
                let _ = eng.send_mod_usermsg(MsgDest::One(slot), USERMSG_MOD_CHUNK, &payload);
            }
        }
    }
}

impl<G: GameDll + 'static> GameDll for ModdedGame<G> {
    fn usermsgs(&self) -> UserMsgRegistry {
        let mut reg = self.inner.usermsgs();
        let _ = reg.register_next(-1, USERMSG_MOD_MANIFEST);
        let _ = reg.register_next(-1, USERMSG_MOD_CHUNK);
        let _ = reg.register_next(-1, USERMSG_MOD_EVENT);
        for name in self.host.declared_custom_usermsgs() {
            let _ = reg.register_next(-1, &name);
        }
        reg
    }

    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        let core_arc = self.host.core_arc();
        let _ = with_amxx_core(&core_arc, self.cs_state_mut(), eng, |amxx, eng, hooks| {
            amxx.on_server_activate_precache(eng, hooks);
        });

        let ev = ActiveEvent {
            i32_args: [i32::from(eng.max_clients()), 0, 0, 0, 0, 0],
            str_args: [eng.map_name().to_string(), String::new(), String::new()],
            ..Default::default()
        };
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ServerActivate,
            HookPhase::Pre,
            &ev,
        );
        if pre.action != HookAction::Supercede {
            self.inner.server_activate(eng);
        }

        // Flush queued custom weapons and precache mod VFX sprites + custom weapon assets while
        // the engine's precache tables are still open (vanilla servers never hit this path).
        if let Some(cs_game) = (&mut self.inner as &mut dyn Any).downcast_mut::<CsGame>() {
            let pending_weapons = {
                let mut core = self.host.core.lock().unwrap_or_else(|e| e.into_inner());
                std::mem::take(&mut core.pending_custom_weapons)
            };
            for spec in pending_weapons {
                cs_hooks::register_custom_weapon(cs_game.state_mut(), Some(eng), spec);
            }
            cs_game.precache_mod_resources(eng);
        }

        if let Some(unhandled) =
            with_amxx_core(&core_arc, self.cs_state_mut(), eng, |amxx, eng, hooks| {
                amxx.on_server_activate_init(eng, hooks);
                amxx.drain_pending_server_cmds(eng, hooks)
            })
        {
            for argv in unhandled {
                let _ = self.inner.server_command(eng, &argv);
            }
        }

        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ServerActivate,
            HookPhase::Post,
            &ev,
        );
        if !self.is_cs_game() {
            let _ = dispatch_hook_core(
                &core_arc,
                None,
                eng,
                HookId::RoundStart,
                HookPhase::Post,
                &ev,
            );
        }
    }

    fn server_deactivate(&mut self, eng: &mut EngineApi<'_>) {
        let ev = ActiveEvent::default();
        let core_arc = self.host.core_arc();
        let _ = with_amxx_core(&core_arc, self.cs_state_mut(), eng, |amxx, eng, hooks| {
            amxx.on_server_deactivate(eng, hooks);
        });
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ServerDeactivate,
            HookPhase::Pre,
            &ev,
        );
        if pre.action != HookAction::Supercede {
            self.inner.server_deactivate(eng);
        }
        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ServerDeactivate,
            HookPhase::Post,
            &ev,
        );
    }

    fn spawn_entity(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        classname: &str,
        keys: &[(String, String)],
    ) -> bool {
        let core_arc = self.host.core_arc();
        let amxx_pre = with_amxx_core(&core_arc, self.cs_state_mut(), eng, |amxx, eng, hooks| {
            amxx.dispatch_ham_spawn(eng, hooks, id.index, classname, false)
        })
        .unwrap_or(0);
        if amxx_pre >= HAM_SUPERCEDE {
            return false;
        }
        let ev = ActiveEvent {
            i32_args: [i32::from(id.index), 0, 0, 0, 0, 0],
            str_args: [classname.to_string(), String::new(), String::new()],
            ..Default::default()
        };
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::SpawnEntity,
            HookPhase::Pre,
            &ev,
        );
        let keep = if pre.action == HookAction::Supercede {
            pre.override_i32.is_some_and(|v| v != 0)
        } else {
            self.inner.spawn_entity(eng, id, classname, keys)
        };
        if keep {
            let _ = with_amxx_core(&core_arc, self.cs_state_mut(), eng, |amxx, eng, hooks| {
                amxx.dispatch_ham_spawn(eng, hooks, id.index, classname, true)
            });
        }
        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::SpawnEntity,
            HookPhase::Post,
            &ev,
        );
        if pre.action == HookAction::Override
            && let Some(v) = pre.override_i32
        {
            return v != 0;
        }
        keep
    }

    fn client_connect(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Result<(), String> {
        // Authenticate admin entry early so invalid `_pw` with kick flag rejects cleanly.
        {
            let services = self.host.services();
            let mut svc = services.lock();
            sync_player_admin(&mut svc.admins, eng, slot)?;
        }
        let core_arc = self.host.core_arc();
        if let Some(Err(reason)) =
            with_amxx_core(&core_arc, self.cs_state_mut(), eng, |amxx, eng, hooks| {
                amxx.on_client_connect(eng, hooks, slot)
            })
        {
            return Err(reason);
        }
        let ev = ActiveEvent {
            i32_args: [i32::from(slot), 0, 0, 0, 0, 0],
            str_args: [
                eng.client_key(slot, "name").unwrap_or("").to_string(),
                String::new(),
                String::new(),
            ],
            ..Default::default()
        };
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ClientConnect,
            HookPhase::Pre,
            &ev,
        );
        let res = if pre.action == HookAction::Supercede {
            Err("rejected by server mod".to_string())
        } else {
            self.inner.client_connect(eng, slot)
        };
        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ClientConnect,
            HookPhase::Post,
            &ev,
        );
        res
    }

    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        {
            let core = self.host.core.lock().unwrap_or_else(|e| e.into_inner());
            let mut svc = core.services.lock();
            sync_player_lang_from_userinfo(&mut svc.lang, eng, slot, &*core.storage);
            let _ = sync_player_admin(&mut svc.admins, eng, slot);
        }
        let ev = ActiveEvent {
            i32_args: [i32::from(slot), i32::from(slot) + 1, 0, 0, 0, 0],
            ..Default::default()
        };
        let core_arc = self.host.core_arc();
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ClientPutInServer,
            HookPhase::Pre,
            &ev,
        );
        if pre.action != HookAction::Supercede {
            self.inner.client_put_in_server(eng, slot);
        }
        let _ = with_amxx_core(&core_arc, self.cs_state_mut(), eng, |amxx, eng, hooks| {
            amxx.on_client_put_in_server(eng, hooks, slot);
        });
        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ClientPutInServer,
            HookPhase::Post,
            &ev,
        );
        if eng.client_supports_mods(slot) {
            self.pending_catalog_slots.insert(slot);
        }
    }

    fn client_disconnect(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.pending_catalog_slots.remove(&slot);
        let core_arc = self.host.core_arc();
        let _ = with_amxx_core(&core_arc, self.cs_state_mut(), eng, |amxx, eng, hooks| {
            amxx.on_client_disconnect(eng, hooks, slot);
        });
        {
            let services = self.host.services();
            let mut svc = services.lock();
            svc.menus.clear_slot(slot);
            svc.lang.clear_slot(slot);
            svc.admins.clear_slot(slot);
        }
        let ev = ActiveEvent {
            i32_args: [i32::from(slot), 0, 0, 0, 0, 0],
            ..Default::default()
        };
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ClientDisconnect,
            HookPhase::Pre,
            &ev,
        );
        if pre.action != HookAction::Supercede {
            self.inner.client_disconnect(eng, slot);
        }
        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ClientDisconnect,
            HookPhase::Post,
            &ev,
        );
    }

    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
        if let Some(first) = argv.first()
            && first == CMD_MOD_REQUEST
            && argv.len() >= 3
            && let (Ok(mod_idx), Ok(offset)) = (argv[1].parse::<u8>(), argv[2].parse::<u32>())
        {
            {
                let mut core = self.host.core.lock().unwrap_or_else(|e| e.into_inner());
                core.pending_client_requests.push((slot, mod_idx, offset));
            }
            self.flush_client_chunk_requests(eng);
            return;
        }

        let cmd = argv.first().cloned().unwrap_or_default();
        let arg1 = argv.get(1).cloned().unwrap_or_default();

        // Route `menuselect` through host `MenuManager` first so WASM & AMXX plugins share menu
        // ownership cleanly without stealing presses or leaking into vanilla CS 1.6 menus.
        if cmd.eq_ignore_ascii_case("menuselect")
            && let Ok(choice) = arg1.parse::<i32>()
        {
            let outcome = {
                let services = self.host.services();
                let mut svc = services.lock();
                svc.menus.handle_menuselect(slot, choice)
            };
            match outcome {
                MenuSelectOutcome::NotOwned => {}
                MenuSelectOutcome::PageTurned(rendered) => {
                    send_rendered_menu(eng, &rendered);
                    return;
                }
                MenuSelectOutcome::Ignored => return,
                MenuSelectOutcome::Selected {
                    owner_id,
                    owner_runtime,
                    owner_index,
                    menu_id,
                    callback_id,
                    callback_name,
                    item_index,
                    info,
                } => {
                    let core_arc = self.host.core_arc();
                    let ev = ActiveEvent {
                        i32_args: [
                            i32::from(slot),
                            menu_id as i32,
                            item_index as i32,
                            0,
                            0,
                            0,
                        ],
                        str_args: [info.clone(), owner_id.clone(), String::new()],
                        ..Default::default()
                    };
                    let pre = dispatch_hook_core(
                        &core_arc,
                        self.cs_state_mut(),
                        eng,
                        HookId::MenuSelect,
                        HookPhase::Pre,
                        &ev,
                    );
                    if pre.action != HookAction::Supercede {
                        match owner_runtime {
                            PluginRuntimeKind::Wasm | PluginRuntimeKind::Python => {
                                dispatch_menu_callback_core(
                                    &core_arc,
                                    self.cs_state_mut(),
                                    eng,
                                    &owner_id,
                                    callback_id,
                                    slot,
                                    menu_id,
                                    item_index as i32,
                                    &info,
                                );
                            }
                            PluginRuntimeKind::Amxx => {
                                let _ = with_amxx_core(
                                    &core_arc,
                                    self.cs_state_mut(),
                                    eng,
                                    |amxx, eng, hooks| {
                                        amxx.dispatch_amxx_menu_callback(
                                            eng,
                                            hooks,
                                            owner_index,
                                            &callback_name,
                                            slot,
                                            menu_id,
                                            item_index as i32,
                                        );
                                    },
                                );
                            }
                            _ => {}
                        }
                    }
                    let _ = dispatch_hook_core(
                        &core_arc,
                        self.cs_state_mut(),
                        eng,
                        HookId::MenuSelect,
                        HookPhase::Post,
                        &ev,
                    );
                    return;
                }
                MenuSelectOutcome::Exited {
                    owner_id,
                    owner_runtime,
                    owner_index,
                    menu_id,
                    callback_id,
                    callback_name,
                } => {
                    let core_arc = self.host.core_arc();
                    match owner_runtime {
                        PluginRuntimeKind::Wasm | PluginRuntimeKind::Python => {
                            dispatch_menu_callback_core(
                                &core_arc,
                                self.cs_state_mut(),
                                eng,
                                &owner_id,
                                callback_id,
                                slot,
                                menu_id,
                                MENU_EXIT,
                                "",
                            );
                        }
                        PluginRuntimeKind::Amxx => {
                            let _ = with_amxx_core(
                                &core_arc,
                                self.cs_state_mut(),
                                eng,
                                |amxx, eng, hooks| {
                                    amxx.dispatch_amxx_menu_callback(
                                        eng,
                                        hooks,
                                        owner_index,
                                        &callback_name,
                                        slot,
                                        menu_id,
                                        MENU_EXIT,
                                    );
                                },
                            );
                        }
                        _ => {}
                    }
                    return;
                }
            }
        }

        // Built-in `/lang <code>` chat command (works identically on both CsGame & WalkGame).
        if (cmd.eq_ignore_ascii_case("say") || cmd.eq_ignore_ascii_case("say_team"))
            && argv.len() >= 2
        {
            let msg_text = argv[1..].join(" ");
            let trimmed = msg_text.trim().trim_matches('"');
            let handled_lang = {
                let mut core = self.host.core.lock().unwrap_or_else(|e| e.into_inner());
                let services = core.services.clone();
                let mut svc = services.lock();
                handle_lang_chat_command(&mut svc.lang, eng, slot, trimmed, &mut *core.storage)
            };
            if handled_lang {
                return;
            }
        }

        let ev = ActiveEvent {
            i32_args: [i32::from(slot), argv.len() as i32, 0, 0, 0, 0],
            str_args: [cmd.clone(), arg1, argv.join(" ")],
            ..Default::default()
        };
        let core_arc = self.host.core_arc();
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ClientCommand,
            HookPhase::Pre,
            &ev,
        );
        self.was_client_command_claimed = {
            let core = self.host.core.lock().unwrap_or_else(|e| e.into_inner());
            core.mods
                .values()
                .any(|m| m.status == ModStatus::Active && m.registered_commands.contains(&cmd))
        };
        if pre.action != HookAction::Supercede && pre.action != HookAction::Handled {
            let amxx_handled =
                with_amxx_core(&core_arc, self.cs_state_mut(), eng, |amxx, eng, hooks| {
                    amxx.dispatch_client_command_after_menu(eng, hooks, slot, argv)
                })
                .unwrap_or(false);
            if !amxx_handled {
                self.inner.client_command(eng, slot, argv);
            }
        }
        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ClientCommand,
            HookPhase::Post,
            &ev,
        );
    }

    fn client_userinfo_changed(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        {
            let core = self.host.core.lock().unwrap_or_else(|e| e.into_inner());
            let mut svc = core.services.lock();
            sync_player_lang_from_userinfo(&mut svc.lang, eng, slot, &*core.storage);
            let _ = sync_player_admin(&mut svc.admins, eng, slot);
        }
        let core_arc = self.host.core_arc();
        let _ = with_amxx_core(&core_arc, self.cs_state_mut(), eng, |amxx, eng, hooks| {
            amxx.on_client_userinfo_changed(eng, hooks, slot);
        });
        let ev = ActiveEvent {
            i32_args: [i32::from(slot), 0, 0, 0, 0, 0],
            ..Default::default()
        };
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ClientUserinfoChanged,
            HookPhase::Pre,
            &ev,
        );
        if pre.action != HookAction::Supercede {
            self.inner.client_userinfo_changed(eng, slot);
        }
        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ClientUserinfoChanged,
            HookPhase::Post,
            &ev,
        );
    }

    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        random_seed: u32,
    ) {
        let ev = ActiveEvent {
            i32_args: [
                i32::from(slot),
                i32::from(cmd.buttons),
                i32::from(cmd.msec),
                0,
                0,
                0,
            ],
            ..Default::default()
        };
        let core_arc = self.host.core_arc();
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::RunUsercmd,
            HookPhase::Pre,
            &ev,
        );
        if pre.action != HookAction::Supercede {
            self.inner.run_usercmd(eng, slot, cmd, random_seed);
        }
        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::RunUsercmd,
            HookPhase::Post,
            &ev,
        );
    }

    fn post_usercmd(&mut self, eng: &mut EngineApi<'_>, slot: u8, cmd: &GoldSrcUserCmd) {
        let ev = ActiveEvent {
            i32_args: [i32::from(slot), i32::from(cmd.buttons), 0, 0, 0, 0],
            ..Default::default()
        };
        let core_arc = self.host.core_arc();
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::PostUsercmd,
            HookPhase::Pre,
            &ev,
        );
        if pre.action != HookAction::Supercede {
            self.inner.post_usercmd(eng, slot, cmd);
        }
        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::PostUsercmd,
            HookPhase::Post,
            &ev,
        );
    }

    fn start_frame(&mut self, eng: &mut EngineApi<'_>, frametime: f32) {
        if !self.pending_catalog_slots.is_empty() {
            let slots: Vec<u8> = self.pending_catalog_slots.iter().copied().collect();
            for slot in slots {
                if eng.is_client_active(slot) {
                    self.pending_catalog_slots.remove(&slot);
                    self.send_client_catalog(eng, slot);
                }
            }
        }
        let core_arc = self.host.core_arc();
        if eng.cvar_f32("sv_mod_autoreload") >= 1.0 {
            for (mod_id, res) in self.host.poll_autoreload(eng.time(), false) {
                if let Err(err) = res {
                    warn!(%mod_id, %err, "auto-reload of Python mod failed; keeping previous instance");
                }
            }
        }
        poll_webhooks_core(&core_arc, self.cs_state_mut(), eng);
        tick_timers_core(&core_arc, self.cs_state_mut(), eng);
        if let Some(unhandled) =
            with_amxx_core(&core_arc, self.cs_state_mut(), eng, |amxx, eng, hooks| {
                amxx.tick_tasks(eng, hooks);
                amxx.drain_pending_server_cmds(eng, hooks)
            })
        {
            for argv in unhandled {
                let _ = self.inner.server_command(eng, &argv);
            }
        }
        self.flush_client_chunk_requests(eng);

        let ev = ActiveEvent {
            f32_args: [frametime, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::StartFrame,
            HookPhase::Pre,
            &ev,
        );
        if pre.action != HookAction::Supercede {
            self.inner.start_frame(eng, frametime);
        }
        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::StartFrame,
            HookPhase::Post,
            &ev,
        );
    }

    fn think(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        let ev = ActiveEvent {
            i32_args: [i32::from(id.index), 0, 0, 0, 0, 0],
            ..Default::default()
        };
        let core_arc = self.host.core_arc();
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::Think,
            HookPhase::Pre,
            &ev,
        );
        if pre.action != HookAction::Supercede {
            self.inner.think(eng, id);
        }
        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::Think,
            HookPhase::Post,
            &ev,
        );
    }

    fn touch(&mut self, eng: &mut EngineApi<'_>, toucher: EdictId, other: EdictId) {
        let core_arc = self.host.core_arc();
        let classname = eng
            .ent(toucher)
            .map(|v| v.classname.clone())
            .unwrap_or_default();
        let amxx_pre = with_amxx_core(&core_arc, self.cs_state_mut(), eng, |amxx, eng, hooks| {
            amxx.dispatch_ham_touch(eng, hooks, toucher.index, &classname, other.index, false)
        })
        .unwrap_or(0);
        if amxx_pre >= HAM_SUPERCEDE {
            return;
        }
        let ev = ActiveEvent {
            i32_args: [i32::from(toucher.index), i32::from(other.index), 0, 0, 0, 0],
            ..Default::default()
        };
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::Touch,
            HookPhase::Pre,
            &ev,
        );
        if pre.action != HookAction::Supercede {
            self.inner.touch(eng, toucher, other);
            let _ = with_amxx_core(&core_arc, self.cs_state_mut(), eng, |amxx, eng, hooks| {
                amxx.dispatch_ham_touch(eng, hooks, toucher.index, &classname, other.index, true)
            });
        }
        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::Touch,
            HookPhase::Post,
            &ev,
        );
    }

    fn blocked(&mut self, eng: &mut EngineApi<'_>, pusher: EdictId, other: EdictId) {
        let ev = ActiveEvent {
            i32_args: [i32::from(pusher.index), i32::from(other.index), 0, 0, 0, 0],
            ..Default::default()
        };
        let core_arc = self.host.core_arc();
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::Blocked,
            HookPhase::Pre,
            &ev,
        );
        if pre.action != HookAction::Supercede {
            self.inner.blocked(eng, pusher, other);
        }
        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::Blocked,
            HookPhase::Post,
            &ev,
        );
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
        let cmd = argv.first()?.as_str();
        match cmd {
            "mod_list" => {
                let mut out = String::new();
                for info in self.host.list_mods() {
                    out.push_str(&format!(
                        "{} v{} status={:?} fuel={}\n",
                        info.id, info.version, info.status, info.total_fuel_used
                    ));
                }
                let amxx_n = self.host.amxx_plugin_count();
                if amxx_n > 0 {
                    out.push_str(&format!("amxx plugins loaded: {amxx_n}\n"));
                }
                if out.is_empty() {
                    out.push_str("no mods loaded\n");
                }
                return Some(out);
            }
            "mod_unload" => {
                let target = argv.get(1)?;
                return Some(if self.host.unload_mod(target) {
                    format!("unloaded `{target}`\n")
                } else {
                    format!("mod `{target}` not found\n")
                });
            }
            "mod_reload" => {
                let target = argv.get(1)?;
                return Some(match self.host.reload_mod(target) {
                    Ok(()) => format!("reloaded `{target}`\n"),
                    Err(e) => format!("failed to reload `{target}`: {e}\n"),
                });
            }
            _ => {}
        }

        let core_arc = self.host.core_arc();
        if let Some(Some(reply)) =
            with_amxx_core(&core_arc, self.cs_state_mut(), eng, |amxx, eng, hooks| {
                amxx.dispatch_server_command(eng, hooks, argv)
            })
        {
            return Some(reply);
        }

        let ev = ActiveEvent {
            str_args: [
                cmd.to_string(),
                argv.get(1).cloned().unwrap_or_default(),
                argv.join(" "),
            ],
            ..Default::default()
        };
        let pre = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ServerCommand,
            HookPhase::Pre,
            &ev,
        );
        if pre.action == HookAction::Supercede || pre.action == HookAction::Handled {
            return Some(pre.override_str.unwrap_or_default());
        }
        let inner_res = self.inner.server_command(eng, argv);
        let _ = dispatch_hook_core(
            &core_arc,
            self.cs_state_mut(),
            eng,
            HookId::ServerCommand,
            HookPhase::Post,
            &ev,
        );
        inner_res
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

fn capture_snapshot(
    game: Option<&GameState>,
    eng: &EngineApi<'_>,
    services: &SharedPlatformServices,
) -> EngineSnapshot {
    let mut entities = BTreeMap::new();
    for id in eng.entity_ids() {
        if let Some(vars) = eng.ent(id) {
            entities.insert(
                id.index,
                EntitySnap {
                    index: id.index,
                    classname: vars.classname.clone(),
                    origin: vars.origin.to_array(),
                    velocity: vars.velocity.to_array(),
                    health: vars.health,
                    flags: vars.flags,
                },
            );
        }
    }

    let svc = services.lock();
    let mut clients = Vec::new();
    for slot in 0..eng.max_clients() {
        let active = eng.is_client_active(slot);
        let name = eng.client_key(slot, "name").unwrap_or("").to_string();
        let has_player = game
            .and_then(|g| g.players.get(usize::from(slot)))
            .and_then(Option::as_ref)
            .is_some();
        if active || has_player || !name.is_empty() {
            let mut snap = ClientSnap {
                slot,
                active,
                supports_mods: eng.client_supports_mods(slot),
                name,
                steamid: eng
                    .client_key(slot, "*sid")
                    .or_else(|| eng.client_key(slot, "steamid"))
                    .unwrap_or("")
                    .to_string(),
                ip: eng.client_key(slot, "ip").unwrap_or("").to_string(),
                ping: 0,
                loss: 0,
                aim_origin: cs_hooks::get_aim_origin(eng, slot).unwrap_or([0.0; 3]),
                admin_flags: svc.admins.player_flags(slot).bits(),
                lang: svc.lang.player_lang(Some(slot)).to_string(),
                ..ClientSnap::default()
            };
            if let Some(g) = game
                && let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref)
            {
                snap.health = p.health;
                snap.max_health = p.max_health;
                snap.armor = p.armor.round() as i32;
                snap.armor_kind = match p.kevlar {
                    ArmorType::None => 0,
                    ArmorType::Vest => 1,
                    ArmorType::VestHelm => 2,
                };
                snap.money = p.money;
                snap.frags = p.frags;
                snap.deaths = p.deaths;
                snap.team = match p.team {
                    Team::Unassigned => 0,
                    Team::Terrorist => 1,
                    Team::Ct => 2,
                    Team::Spectator => 3,
                };
                snap.alive = cs_hooks::is_player_alive(g, eng, slot);
                snap.active_weapon = p.active_item().map_or(0, |w| w as i32 as u8);
                snap.clip = cs_hooks::get_weapon_clip(g, slot, 0);
                snap.bpammo = cs_hooks::get_user_bpammo(g, slot, 0);
                for (wid, clip, bpammo) in cs_hooks::snapshot_player_weapons(g, slot) {
                    snap.weapon_clips.insert(wid, clip);
                    snap.weapon_bpammo.insert(wid, bpammo);
                }
                snap.has_defuser = p.has_defuser;
                snap.has_nvg = p.has_night_vision();
            } else if let Some(v) = eng.ent(eng.client_edict(slot)) {
                snap.health = v.health;
                snap.alive = v.deadflag == 0 && v.health > 0.0;
            }
            clients.push(snap);
        }
    }
    drop(svc);

    let mut cvars = BTreeMap::new();
    for name in [
        "mp_roundtime",
        "mp_freezetime",
        "mp_timelimit",
        "mp_friendlyfire",
        "sv_gravity",
        "hostname",
        "bot_quota",
        "amx_language",
        "sv_mod_autoreload",
    ] {
        if let Some(val) = eng.cvar_str(name) {
            cvars.insert(name.to_string(), val);
        }
    }

    EngineSnapshot {
        time_secs: eng.time(),
        frametime: eng.frametime(),
        max_clients: eng.max_clients(),
        map_name: eng.map_name().to_string(),
        entities,
        clients,
        cvars,
        collision: eng.world_collision_arc(),
        client_health: 0,
        client_armor: 0,
        client_money: 0,
    }
}

fn resolve_edict(eng: &EngineApi<'_>, index: u16) -> Option<EdictId> {
    eng.entity_ids().into_iter().find(|id| id.index == index)
}

fn is_safe_storage_subkey(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b'.'))
}

fn weapon_id_to_classname(weapon_id: u8) -> &'static str {
    match weapon_id {
        1 => "weapon_p228",
        3 => "weapon_scout",
        4 => "weapon_hegrenade",
        5 => "weapon_xm1014",
        6 => "weapon_c4",
        7 => "weapon_mac10",
        8 => "weapon_aug",
        9 => "weapon_smokegrenade",
        10 => "weapon_elite",
        11 => "weapon_fiveseven",
        12 => "weapon_ump45",
        13 => "weapon_sg550",
        14 => "weapon_galil",
        15 => "weapon_famas",
        16 => "weapon_usp",
        17 => "weapon_glock18",
        18 => "weapon_awp",
        19 => "weapon_mp5navy",
        20 => "weapon_m249",
        21 => "weapon_m3",
        22 => "weapon_m4a1",
        23 => "weapon_tmp",
        24 => "weapon_g3sg1",
        25 => "weapon_flashbang",
        26 => "weapon_deagle",
        27 => "weapon_sg552",
        28 => "weapon_ak47",
        29 => "weapon_knife",
        30 => "weapon_p90",
        _ => "weapon_knife",
    }
}

#[derive(Default)]
struct PluginsTomlConfig {
    mods_dir: Option<String>,
    amxx_dir: Option<String>,
    configs_dir: Option<String>,
    mod_entries: Vec<String>,
    amxx_entries: Vec<String>,
}

fn resolve_rel(base: &Path, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

fn parse_plugins_toml(src: &str) -> PluginsTomlConfig {
    let mut cfg = PluginsTomlConfig::default();
    let mut in_plugin_table = false;
    let mut cur_path: Option<String> = None;
    let mut cur_enabled = true;
    let mut cur_kind: Option<String> = None;

    let flush = |cfg: &mut PluginsTomlConfig,
                 path: &mut Option<String>,
                 enabled: &mut bool,
                 kind: &mut Option<String>| {
        if let Some(p) = path.take()
            && *enabled
        {
            let lower = p.to_ascii_lowercase();
            let is_amxx = kind
                .as_deref()
                .is_some_and(|k| k.eq_ignore_ascii_case("amxx"))
                || lower.ends_with(".amxx")
                || lower.ends_with(".amx");
            if is_amxx {
                cfg.amxx_entries.push(p);
            } else {
                cfg.mod_entries.push(p);
            }
        }
        *enabled = true;
        *kind = None;
    };

    for raw_line in src.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line == "[[plugins]]" {
            flush(&mut cfg, &mut cur_path, &mut cur_enabled, &mut cur_kind);
            in_plugin_table = true;
            continue;
        }
        if line.starts_with('[') {
            flush(&mut cfg, &mut cur_path, &mut cur_enabled, &mut cur_kind);
            in_plugin_table = false;
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let val = v.trim();
        let unquoted = val.trim_matches('"').trim_matches('\'').to_string();
        if in_plugin_table {
            match key {
                "path" => cur_path = Some(unquoted),
                "enabled" => cur_enabled = !val.eq_ignore_ascii_case("false"),
                "type" | "runtime" => cur_kind = Some(unquoted),
                _ => {}
            }
        } else {
            match key {
                "mods_dir" => cfg.mods_dir = Some(unquoted),
                "amxx_dir" => cfg.amxx_dir = Some(unquoted),
                "configs_dir" => cfg.configs_dir = Some(unquoted),
                "mods" | "python" => {
                    for item in parse_toml_string_array(val) {
                        cfg.mod_entries.push(item);
                    }
                }
                "amxx" => {
                    for item in parse_toml_string_array(val) {
                        cfg.amxx_entries.push(item);
                    }
                }
                _ => {}
            }
        }
    }
    flush(&mut cfg, &mut cur_path, &mut cur_enabled, &mut cur_kind);
    cfg
}

fn is_python_manifest(manifest: &cs_mod_api::ModManifest) -> bool {
    manifest
        .entry_server
        .as_deref()
        .is_some_and(|e| e.to_ascii_lowercase().ends_with(".py"))
}

#[cfg(not(target_arch = "wasm32"))]
fn resolve_python_watch_path(pkg: &LoadedPackage) -> Option<PathBuf> {
    let src = pkg.source_path.as_ref()?;
    if src.is_file()
        && src
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("py"))
    {
        return Some(src.clone());
    }
    if src.is_dir()
        && let Some(ref entry) = pkg.manifest.entry_server
    {
        let cand = src.join(entry);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn py_file_stamp(path: &Path) -> (Option<std::time::SystemTime>, u64) {
    match std::fs::metadata(path) {
        Ok(meta) => (meta.modified().ok(), meta.len()),
        Err(_) => (None, 0),
    }
}

fn parse_toml_string_array(val: &str) -> Vec<String> {
    let trimmed = val.trim().trim_start_matches('[').trim_end_matches(']');
    trimmed
        .split(',')
        .map(|s| s.trim().trim_matches('"').trim_matches('\'').trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
