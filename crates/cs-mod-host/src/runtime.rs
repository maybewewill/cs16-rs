//! Sandboxed mod execution engine seam ([`ModRuntime`] / [`ModInstance`]) and its
//! deterministic `wasmi` 2.0 implementation ([`WasmiRuntime`]) supporting ABI v1 + v2
//! `"cs16_mod"` natives and minimal safe `"wasi_snapshot_preview1"` shims (Go/C++ WASI).

use std::collections::BTreeMap;
use std::sync::Arc;

use cs_mod_api::services::{
    MenuManager, PluginRuntimeKind, RpcEventSubscriber, RpcExportEntry, SharedPlatformServices,
};
use cs_mod_api::{
    ArmorKind, BarTimeSpec, DhudMessageSpec, HOST_MODULE, HookAction, HookId, HookPhase,
    HttpPostJsonSpec, HudDrawCmd, HudMessageSpec, MenuCreateSpec, MenuDisplaySpec, MenuItemSpec,
    Permission, Permissions, RpcEmitEventSpec, RpcValue, ScreenFadeSpec, ScreenShakeSpec,
    ShowMotdSpec, TeamKind, VfxOp,
};
use thiserror::Error;
use tracing::info;
use wasmi::{Caller, Config, Engine, Instance, Linker, Module, Store, StoreLimitsBuilder};

use crate::natives::{
    ActiveEvent, EngineOp, EngineSnapshot, HostCallState, HookRegistration, MAX_HOST_PAYLOAD_BYTES,
    RpcDispatcher, TimerRequest, parse_custom_weapon_spec, parse_lang_args, read_guest_bytes,
    read_guest_str, read_guest_u32, write_guest_bytes, write_guest_f32_array3, write_guest_u32,
    write_guest_u64,
};
use crate::webhook::{WEBHOOK_MAX_REQUEST_BYTES, is_host_allowed, parse_webhook_url};

pub const WASI_PREVIEW1_MODULE: &str = "wasi_snapshot_preview1";

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    #[error("failed to compile WASM module `{0}`: {1}")]
    Compile(String, String),
    #[error("failed to instantiate WASM module `{0}`: {1}")]
    Instantiate(String, String),
    #[error("WASM mod trapped or exceeded limits: {0}")]
    Fault(String),
}

/// Inputs passed into a single hook/timer/menu/RPC/webhook/client invocation.
pub struct ModExecContext<'a> {
    pub snapshot: &'a EngineSnapshot,
    pub event: &'a ActiveEvent,
    pub storage: &'a BTreeMap<String, String>,
    pub fuel_budget: u64,
}

/// Side-effects and control decision collected from one successful guest invocation.
#[derive(Debug, Clone)]
pub struct ModCallOutcome {
    pub action: HookAction,
    pub ops: Vec<EngineOp>,
    pub timers: Vec<TimerRequest>,
    pub cancel_timers: Vec<u32>,
    pub storage_writes: Vec<(String, Option<String>)>,
    pub hud_cmds: Vec<HudDrawCmd>,
    pub hooks_registered: Vec<HookRegistration>,
    pub override_i32: Option<i32>,
    pub override_i32_2: Option<i32>,
    pub override_f32: Option<f32>,
    pub override_str: Option<String>,
    pub rpc_return_str: Option<String>,
    pub wasi_logs: Vec<String>,
    pub fuel_used: u64,
}

impl Default for ModCallOutcome {
    fn default() -> Self {
        Self {
            action: HookAction::Continue,
            ops: Vec::new(),
            timers: Vec::new(),
            cancel_timers: Vec::new(),
            storage_writes: Vec::new(),
            hud_cmds: Vec::new(),
            hooks_registered: Vec::new(),
            override_i32: None,
            override_i32_2: None,
            override_f32: None,
            override_str: None,
            rpc_return_str: None,
            wasi_logs: Vec::new(),
            fuel_used: 0,
        }
    }
}

/// Live instantiated WASM (or future JIT) mod instance.
pub trait ModInstance: Send {
    fn configure_v2(
        &mut self,
        _services: SharedPlatformServices,
        _http_hosts: Vec<String>,
        _rpc_dispatcher: Option<Arc<dyn RpcDispatcher>>,
    ) {
    }
    fn call_init(&mut self, ctx: &ModExecContext<'_>) -> Result<ModCallOutcome, RuntimeError>;
    fn call_hook(
        &mut self,
        hook: HookId,
        phase: HookPhase,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError>;
    fn call_timer(
        &mut self,
        cb_id: i32,
        arg: i32,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError>;
    fn call_menu(
        &mut self,
        _cb_id: i32,
        _slot: u8,
        _menu_id: u32,
        _item_index: i32,
        _ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        Ok(ModCallOutcome::default())
    }
    fn call_rpc(
        &mut self,
        _cb_id: i32,
        _ctx: &ModExecContext<'_>,
    ) -> Result<(i32, ModCallOutcome), RuntimeError> {
        Ok((0, ModCallOutcome::default()))
    }
    fn call_event(
        &mut self,
        _cb_id: i32,
        _ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        Ok(ModCallOutcome::default())
    }
    fn call_webhook(
        &mut self,
        _cb_id: i32,
        _status: i32,
        _ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        Ok(ModCallOutcome::default())
    }
    fn call_client_hud(
        &mut self,
        dt_ms: i32,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError>;
    fn call_client_usermsg(&mut self, ctx: &ModExecContext<'_>) -> Result<ModCallOutcome, RuntimeError>;
    fn call_client_input(
        &mut self,
        key: i32,
        pressed: bool,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError>;
    fn memory_bytes(&self) -> usize;
}

/// Pluggable execution runtime seam (`wasmi` interpreter in v1/v2, Python VM, future JIT later).
pub trait ModRuntime: Send + Sync {
    fn name(&self) -> &'static str;
    fn instantiate(
        &self,
        mod_id: &str,
        wasm_or_wat: &[u8],
        permissions: Permissions,
        max_memory_bytes: usize,
    ) -> Result<Box<dyn ModInstance>, RuntimeError>;
}

/// Extension of [`ModRuntime`] implemented by pure-Rust Python runtimes (`cs-mod-py`) so
/// [`ServerModHost`](crate::ServerModHost) can discover, validate, synthesize manifests for, and
/// hot-reload `.py` plugins without introducing a crate cycle.
pub trait PythonModRuntime: ModRuntime {
    fn synthesize_manifest(
        &self,
        mod_id_fallback: &str,
        file_name: &str,
        source: &str,
    ) -> Result<cs_mod_api::ModManifest, String>;
}

/// Deterministic `wasmi` interpreter runtime with per-call fuel accounting and memory caps.
pub struct WasmiRuntime {
    engine: Engine,
}

impl Default for WasmiRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmiRuntime {
    pub fn new() -> Self {
        let mut cfg = Config::default();
        cfg.consume_fuel(true);
        cfg.set_max_recursion_depth(256);
        let engine = Engine::new(&cfg);
        Self { engine }
    }
}

impl ModRuntime for WasmiRuntime {
    fn name(&self) -> &'static str {
        "wasmi"
    }

    fn instantiate(
        &self,
        mod_id: &str,
        wasm_or_wat: &[u8],
        permissions: Permissions,
        max_memory_bytes: usize,
    ) -> Result<Box<dyn ModInstance>, RuntimeError> {
        let module = Module::new(&self.engine, wasm_or_wat)
            .map_err(|e| RuntimeError::Compile(mod_id.to_string(), e.to_string()))?;

        let limits = StoreLimitsBuilder::new()
            .memory_size(max_memory_bytes)
            .table_elements(4096)
            .instances(1)
            .memories(1)
            .tables(2)
            .trap_on_grow_failure(true)
            .build();

        let state = HostCallState {
            mod_id: mod_id.to_string(),
            permissions,
            http_hosts: Vec::new(),
            limits,
            snapshot: EngineSnapshot::default(),
            active_event: ActiveEvent::default(),
            storage_cache: BTreeMap::new(),
            storage_writes: Vec::new(),
            ops: Vec::new(),
            hooks: Vec::new(),
            timers: Vec::new(),
            cancel_timers: Vec::new(),
            next_local_timer_id: 1,
            hud_cmds: Vec::new(),
            override_i32: None,
            override_i32_2: None,
            override_f32: None,
            override_str: None,
            rpc_return_str: None,
            services: SharedPlatformServices::new(),
            rpc_dispatcher: None,
            wasi_logs: Vec::new(),
            wasi_rng: 0x2545_F491_4F6C_DD1D,
        };

        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limits);
        // Supply startup fuel so WASM start sections / WASI _initialize (including Go wasip1
        // reactor runtime initialization) succeed cleanly while still halting infinite loops.
        const STARTUP_FUEL: u64 = 25_000_000;
        let _ = store.set_fuel(STARTUP_FUEL);

        let mut linker = <Linker<HostCallState>>::new(&self.engine);
        register_host_natives(&mut linker)
            .map_err(|e| RuntimeError::Instantiate(mod_id.to_string(), e.to_string()))?;
        register_wasi_preview1(&mut linker)
            .map_err(|e| RuntimeError::Instantiate(mod_id.to_string(), e.to_string()))?;

        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| RuntimeError::Instantiate(mod_id.to_string(), e.to_string()))?;

        // WASI reactor entrypoint used by Go (`GOOS=wasip1`) / C++ WASI modules.
        if let Ok(init_fn) = instance.get_typed_func::<(), ()>(&store, "_initialize") {
            let _ = store.set_fuel(STARTUP_FUEL);
            init_fn
                .call(&mut store, ())
                .map_err(|e| RuntimeError::Instantiate(mod_id.to_string(), e.to_string()))?;
        }

        Ok(Box::new(WasmiInstance { store, instance }))
    }
}

struct WasmiInstance {
    store: Store<HostCallState>,
    instance: Instance,
}

impl WasmiInstance {
    fn prepare_call(&mut self, ctx: &ModExecContext<'_>) -> Result<(), RuntimeError> {
        let s = self.store.data_mut();
        s.snapshot = ctx.snapshot.clone();
        s.active_event = ctx.event.clone();
        s.storage_cache = ctx.storage.clone();
        s.storage_writes.clear();
        s.ops.clear();
        s.hooks.clear();
        s.timers.clear();
        s.cancel_timers.clear();
        s.hud_cmds.clear();
        s.override_i32 = None;
        s.override_i32_2 = None;
        s.override_f32 = None;
        s.override_str = None;
        s.rpc_return_str = None;
        s.wasi_logs.clear();
        self.store
            .set_fuel(ctx.fuel_budget)
            .map_err(|e| RuntimeError::Fault(e.to_string()))
    }

    fn finish_call(&mut self, budget: u64, action: HookAction) -> ModCallOutcome {
        let remaining = self.store.get_fuel().unwrap_or(0);
        let fuel_used = budget.saturating_sub(remaining);
        let s = self.store.data_mut();
        ModCallOutcome {
            action,
            ops: std::mem::take(&mut s.ops),
            timers: std::mem::take(&mut s.timers),
            cancel_timers: std::mem::take(&mut s.cancel_timers),
            storage_writes: std::mem::take(&mut s.storage_writes),
            hud_cmds: std::mem::take(&mut s.hud_cmds),
            hooks_registered: std::mem::take(&mut s.hooks),
            override_i32: s.override_i32.take(),
            override_i32_2: s.override_i32_2.take(),
            override_f32: s.override_f32.take(),
            override_str: s.override_str.take(),
            rpc_return_str: s.rpc_return_str.take(),
            wasi_logs: std::mem::take(&mut s.wasi_logs),
            fuel_used,
        }
    }
}

impl ModInstance for WasmiInstance {
    fn configure_v2(
        &mut self,
        services: SharedPlatformServices,
        http_hosts: Vec<String>,
        rpc_dispatcher: Option<Arc<dyn RpcDispatcher>>,
    ) {
        let s = self.store.data_mut();
        s.services = services;
        s.http_hosts = http_hosts;
        s.rpc_dispatcher = rpc_dispatcher;
    }

    fn call_init(&mut self, ctx: &ModExecContext<'_>) -> Result<ModCallOutcome, RuntimeError> {
        let Ok(func) = self.instance.get_typed_func::<(), i32>(&self.store, "cs_mod_init") else {
            return Ok(ModCallOutcome::default());
        };
        self.prepare_call(ctx)?;
        let rc = func
            .call(&mut self.store, ())
            .map_err(|e| RuntimeError::Fault(e.to_string()))?;
        if rc < 0 {
            return Err(RuntimeError::Fault(format!("cs_mod_init returned error {rc}")));
        }
        Ok(self.finish_call(ctx.fuel_budget, HookAction::Continue))
    }

    fn call_hook(
        &mut self,
        hook: HookId,
        phase: HookPhase,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        let Ok(func) = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&self.store, "cs_mod_hook")
        else {
            return Ok(ModCallOutcome::default());
        };
        self.prepare_call(ctx)?;
        let rc = func
            .call(&mut self.store, (hook as i32, phase as i32))
            .map_err(|e| RuntimeError::Fault(e.to_string()))?;
        Ok(self.finish_call(ctx.fuel_budget, HookAction::from_i32(rc)))
    }

    fn call_timer(
        &mut self,
        cb_id: i32,
        arg: i32,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        let Ok(func) = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&self.store, "cs_mod_timer")
        else {
            return Ok(ModCallOutcome::default());
        };
        self.prepare_call(ctx)?;
        let rc = func
            .call(&mut self.store, (cb_id, arg))
            .map_err(|e| RuntimeError::Fault(e.to_string()))?;
        Ok(self.finish_call(ctx.fuel_budget, HookAction::from_i32(rc)))
    }

    fn call_menu(
        &mut self,
        cb_id: i32,
        slot: u8,
        menu_id: u32,
        item_index: i32,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        let Ok(func) = self
            .instance
            .get_typed_func::<(i32, i32, i32, i32), i32>(&self.store, "cs_mod_menu")
        else {
            return Ok(ModCallOutcome::default());
        };
        self.prepare_call(ctx)?;
        let rc = func
            .call(&mut self.store, (cb_id, i32::from(slot), menu_id as i32, item_index))
            .map_err(|e| RuntimeError::Fault(e.to_string()))?;
        Ok(self.finish_call(ctx.fuel_budget, HookAction::from_i32(rc)))
    }

    fn call_rpc(
        &mut self,
        cb_id: i32,
        ctx: &ModExecContext<'_>,
    ) -> Result<(i32, ModCallOutcome), RuntimeError> {
        let Ok(func) = self
            .instance
            .get_typed_func::<i32, i32>(&self.store, "cs_mod_rpc")
        else {
            return Ok((0, ModCallOutcome::default()));
        };
        self.prepare_call(ctx)?;
        let rc = func
            .call(&mut self.store, cb_id)
            .map_err(|e| RuntimeError::Fault(e.to_string()))?;
        Ok((rc, self.finish_call(ctx.fuel_budget, HookAction::Continue)))
    }

    fn call_event(
        &mut self,
        cb_id: i32,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        let Ok(func) = self
            .instance
            .get_typed_func::<i32, i32>(&self.store, "cs_mod_event")
        else {
            return Ok(ModCallOutcome::default());
        };
        self.prepare_call(ctx)?;
        let rc = func
            .call(&mut self.store, cb_id)
            .map_err(|e| RuntimeError::Fault(e.to_string()))?;
        Ok(self.finish_call(ctx.fuel_budget, HookAction::from_i32(rc)))
    }

    fn call_webhook(
        &mut self,
        cb_id: i32,
        status: i32,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        let Ok(func) = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&self.store, "cs_mod_webhook")
        else {
            return Ok(ModCallOutcome::default());
        };
        self.prepare_call(ctx)?;
        let rc = func
            .call(&mut self.store, (cb_id, status))
            .map_err(|e| RuntimeError::Fault(e.to_string()))?;
        Ok(self.finish_call(ctx.fuel_budget, HookAction::from_i32(rc)))
    }

    fn call_client_hud(
        &mut self,
        dt_ms: i32,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        let Ok(func) = self
            .instance
            .get_typed_func::<(i32, i32, i32, i32), ()>(&self.store, "cs_mod_client_hud")
        else {
            return Ok(ModCallOutcome::default());
        };
        self.prepare_call(ctx)?;
        func.call(
            &mut self.store,
            (
                dt_ms,
                ctx.snapshot.client_health,
                ctx.snapshot.client_armor,
                ctx.snapshot.client_money,
            ),
        )
        .map_err(|e| RuntimeError::Fault(e.to_string()))?;
        Ok(self.finish_call(ctx.fuel_budget, HookAction::Continue))
    }

    fn call_client_usermsg(&mut self, ctx: &ModExecContext<'_>) -> Result<ModCallOutcome, RuntimeError> {
        let Ok(func) = self
            .instance
            .get_typed_func::<(), i32>(&self.store, "cs_mod_client_usermsg")
        else {
            return Ok(ModCallOutcome::default());
        };
        self.prepare_call(ctx)?;
        let rc = func
            .call(&mut self.store, ())
            .map_err(|e| RuntimeError::Fault(e.to_string()))?;
        Ok(self.finish_call(ctx.fuel_budget, HookAction::from_i32(rc)))
    }

    fn call_client_input(
        &mut self,
        key: i32,
        pressed: bool,
        ctx: &ModExecContext<'_>,
    ) -> Result<ModCallOutcome, RuntimeError> {
        let Ok(func) = self
            .instance
            .get_typed_func::<(i32, i32), i32>(&self.store, "cs_mod_client_input")
        else {
            return Ok(ModCallOutcome::default());
        };
        self.prepare_call(ctx)?;
        let rc = func
            .call(&mut self.store, (key, i32::from(pressed)))
            .map_err(|e| RuntimeError::Fault(e.to_string()))?;
        Ok(self.finish_call(ctx.fuel_budget, HookAction::from_i32(rc)))
    }

    fn memory_bytes(&self) -> usize {
        self.instance
            .get_memory(&self.store, "memory")
            .map_or(0, |m| m.data_size(&self.store))
    }
}

fn rgba_u32(packed: i32) -> [u8; 4] {
    let u = packed as u32;
    [
        ((u >> 24) & 0xFF) as u8,
        ((u >> 16) & 0xFF) as u8,
        ((u >> 8) & 0xFF) as u8,
        (u & 0xFF) as u8,
    ]
}

fn rgb_u24(packed: i32) -> [u8; 3] {
    let u = packed as u32;
    [((u >> 16) & 0xFF) as u8, ((u >> 8) & 0xFF) as u8, (u & 0xFF) as u8]
}

fn opt_slot(slot: i32) -> Result<Option<u8>, ()> {
    if slot < 0 {
        Ok(None)
    } else {
        u8::try_from(slot).map(Some).map_err(|_| ())
    }
}

fn register_host_natives(linker: &mut Linker<HostCallState>) -> Result<(), wasmi::Error> {
    register_v1_natives(linker)?;
    register_v2_player_and_weapon_natives(linker)?;
    register_v2_vfx_and_ui_natives(linker)?;
    register_v2_platform_natives(linker)?;
    Ok(())
}

fn register_v1_natives(linker: &mut Linker<HostCallState>) -> Result<(), wasmi::Error> {
    // --- Lifecycle & hook context ---
    linker.func_wrap(
        HOST_MODULE,
        "hook_register",
        |mut caller: Caller<'_, HostCallState>, hook_id: i32, phase: i32, priority: i32| -> i32 {
            let (Some(hook), Some(ph)) = (HookId::from_i32(hook_id), HookPhase::from_i32(phase))
            else {
                return -1;
            };
            caller.data_mut().hooks.push(HookRegistration {
                hook,
                phase: ph,
                priority,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "host_time_ms",
        |caller: Caller<'_, HostCallState>| -> i64 {
            (caller.data().snapshot.time_secs * 1000.0).round() as i64
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "hook_arg_i32",
        |caller: Caller<'_, HostCallState>, idx: i32| -> i32 {
            usize::try_from(idx)
                .ok()
                .and_then(|i| caller.data().active_event.i32_args.get(i).copied())
                .unwrap_or(0)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "hook_arg_f32",
        |caller: Caller<'_, HostCallState>, idx: i32| -> f32 {
            usize::try_from(idx)
                .ok()
                .and_then(|i| caller.data().active_event.f32_args.get(i).copied())
                .unwrap_or(0.0)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "hook_arg_str",
        |mut caller: Caller<'_, HostCallState>, idx: i32, ptr: i32, cap: i32| -> i32 {
            let Some(text) = usize::try_from(idx)
                .ok()
                .and_then(|i| caller.data().active_event.str_args.get(i))
                .cloned()
            else {
                return -1;
            };
            write_guest_bytes(&mut caller, ptr, cap, text.as_bytes())
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "hook_arg_bytes",
        |mut caller: Caller<'_, HostCallState>, ptr: i32, cap: i32| -> i32 {
            let bytes = caller.data().active_event.payload.clone();
            write_guest_bytes(&mut caller, ptr, cap, &bytes)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "hook_set_override_i32",
        |mut caller: Caller<'_, HostCallState>, val: i32| -> i32 {
            caller.data_mut().override_i32 = Some(val);
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "hook_set_override_i32_2",
        |mut caller: Caller<'_, HostCallState>, val: i32| -> i32 {
            caller.data_mut().override_i32_2 = Some(val);
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "hook_set_override_f32",
        |mut caller: Caller<'_, HostCallState>, val: f32| -> i32 {
            if !val.is_finite() {
                return -1;
            }
            caller.data_mut().override_f32 = Some(val);
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "hook_set_override_str",
        |mut caller: Caller<'_, HostCallState>, ptr: i32, len: i32| -> i32 {
            let Some(s) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().override_str = Some(s);
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "rpc_set_return_str",
        |mut caller: Caller<'_, HostCallState>, ptr: i32, len: i32| -> i32 {
            let Some(s) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().rpc_return_str = Some(s);
            0
        },
    )?;

    // --- Entities & Players (`Permission::Entities`) ---
    linker.func_wrap(
        HOST_MODULE,
        "ent_count",
        |caller: Caller<'_, HostCallState>| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            caller.data().snapshot.entities.len() as i32
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "ent_create",
        |mut caller: Caller<'_, HostCallState>, cls_ptr: i32, cls_len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Some(classname) = read_guest_str(&caller, cls_ptr, cls_len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::CreateEntity { classname });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "ent_remove",
        |mut caller: Caller<'_, HostCallState>, idx: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(index) = u16::try_from(idx) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::RemoveEntity { index });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "ent_get_health",
        |caller: Caller<'_, HostCallState>, idx: i32| -> f32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1.0;
            }
            u16::try_from(idx)
                .ok()
                .and_then(|i| caller.data().snapshot.entities.get(&i))
                .map_or(-1.0, |e| e.health)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "ent_set_health",
        |mut caller: Caller<'_, HostCallState>, idx: i32, health: f32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) || !health.is_finite() {
                return -1;
            }
            let Ok(index) = u16::try_from(idx) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetHealth { index, health });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "ent_get_origin",
        |mut caller: Caller<'_, HostCallState>, idx: i32, out_ptr: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Some(origin) = u16::try_from(idx)
                .ok()
                .and_then(|i| caller.data().snapshot.entities.get(&i))
                .map(|e| e.origin)
            else {
                return -1;
            };
            write_guest_f32_array3(&mut caller, out_ptr, origin)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "ent_set_origin",
        |mut caller: Caller<'_, HostCallState>, idx: i32, x: f32, y: f32, z: f32| -> i32 {
            if !caller.data().has_perm(Permission::Entities)
                || !x.is_finite()
                || !y.is_finite()
                || !z.is_finite()
            {
                return -1;
            }
            let Ok(index) = u16::try_from(idx) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetOrigin {
                index,
                origin: [x, y, z],
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "ent_get_velocity",
        |mut caller: Caller<'_, HostCallState>, idx: i32, out_ptr: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Some(vel) = u16::try_from(idx)
                .ok()
                .and_then(|i| caller.data().snapshot.entities.get(&i))
                .map(|e| e.velocity)
            else {
                return -1;
            };
            write_guest_f32_array3(&mut caller, out_ptr, vel)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "ent_set_velocity",
        |mut caller: Caller<'_, HostCallState>, idx: i32, x: f32, y: f32, z: f32| -> i32 {
            if !caller.data().has_perm(Permission::Entities)
                || !x.is_finite()
                || !y.is_finite()
                || !z.is_finite()
            {
                return -1;
            }
            let Ok(index) = u16::try_from(idx) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetVelocity {
                index,
                velocity: [x, y, z],
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "ent_get_flags",
        |caller: Caller<'_, HostCallState>, idx: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            u16::try_from(idx)
                .ok()
                .and_then(|i| caller.data().snapshot.entities.get(&i))
                .map_or(-1, |e| e.flags as i32)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "ent_set_flags",
        |mut caller: Caller<'_, HostCallState>, idx: i32, flags: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(index) = u16::try_from(idx) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetFlags {
                index,
                flags: flags as u32,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "ent_classname",
        |mut caller: Caller<'_, HostCallState>, idx: i32, out_ptr: i32, cap: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Some(name) = u16::try_from(idx)
                .ok()
                .and_then(|i| caller.data().snapshot.entities.get(&i))
                .map(|e| e.classname.clone())
            else {
                return -1;
            };
            write_guest_bytes(&mut caller, out_ptr, cap, name.as_bytes())
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_max_clients",
        |caller: Caller<'_, HostCallState>| -> i32 {
            i32::from(caller.data().snapshot.max_clients)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_is_active",
        |caller: Caller<'_, HostCallState>, slot: i32| -> i32 {
            let Ok(slot_u8) = u8::try_from(slot) else {
                return 0;
            };
            i32::from(
                caller
                    .data()
                    .snapshot
                    .clients
                    .iter()
                    .any(|c| c.slot == slot_u8 && c.active),
            )
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_name",
        |mut caller: Caller<'_, HostCallState>, slot: i32, out_ptr: i32, cap: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Some(name) = u8::try_from(slot)
                .ok()
                .and_then(|s| caller.data().snapshot.client(s).map(|c| c.name.clone()))
            else {
                return -1;
            };
            write_guest_bytes(&mut caller, out_ptr, cap, name.as_bytes())
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_kick",
        |mut caller: Caller<'_, HostCallState>, slot: i32, ptr: i32, len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot_u8) = u8::try_from(slot) else {
                return -1;
            };
            let Some(reason) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::Kick {
                slot: slot_u8,
                reason,
            });
            0
        },
    )?;

    // --- Messages (`Permission::Messages` / `Permission::NetCustom`) ---
    linker.func_wrap(
        HOST_MODULE,
        "print_client",
        |mut caller: Caller<'_, HostCallState>, slot: i32, ptr: i32, len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Messages) {
                return -1;
            }
            let Ok(slot_u8) = u8::try_from(slot) else {
                return -1;
            };
            let Some(text) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::PrintClient {
                slot: slot_u8,
                text,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "print_all",
        |mut caller: Caller<'_, HostCallState>, ptr: i32, len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Messages) {
                return -1;
            }
            let Some(text) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::PrintAll { text });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "center_print",
        |mut caller: Caller<'_, HostCallState>, slot: i32, ptr: i32, len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Messages) {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            let Some(text) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::CenterPrint {
                slot: target,
                text,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "say_text",
        |mut caller: Caller<'_, HostCallState>, slot: i32, ptr: i32, len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Messages) {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            let Some(text) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SayText { slot: target, text });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "send_custom_usermsg",
        |mut caller: Caller<'_, HostCallState>,
         slot: i32,
         name_ptr: i32,
         name_len: i32,
         data_ptr: i32,
         data_len: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::NetCustom) {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            let Some(name) = read_guest_str(&caller, name_ptr, name_len) else {
                return -1;
            };
            let Some(data) = read_guest_bytes(&caller, data_ptr, data_len, MAX_HOST_PAYLOAD_BYTES)
            else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SendCustomUserMsg {
                slot: target,
                name,
                data,
            });
            0
        },
    )?;

    // --- CVars (`Permission::Cvars`) ---
    linker.func_wrap(
        HOST_MODULE,
        "cvar_get_f32",
        |caller: Caller<'_, HostCallState>, ptr: i32, len: i32| -> f32 {
            if !caller.data().has_perm(Permission::Cvars) {
                return 0.0;
            }
            let Some(name) = read_guest_str(&caller, ptr, len) else {
                return 0.0;
            };
            caller
                .data()
                .snapshot
                .cvars
                .get(&name)
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(0.0)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "cvar_set_f32",
        |mut caller: Caller<'_, HostCallState>, ptr: i32, len: i32, val: f32| -> i32 {
            if !caller.data().has_perm(Permission::Cvars) || !val.is_finite() {
                return -1;
            }
            let Some(name) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetCvar {
                name,
                value: val.to_string(),
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "cvar_get_str",
        |mut caller: Caller<'_, HostCallState>,
         ptr: i32,
         len: i32,
         out_ptr: i32,
         cap: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Cvars) {
                return -1;
            }
            let Some(name) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            let Some(val) = caller.data().snapshot.cvars.get(&name).cloned() else {
                return -1;
            };
            write_guest_bytes(&mut caller, out_ptr, cap, val.as_bytes())
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "cvar_set_str",
        |mut caller: Caller<'_, HostCallState>,
         ptr: i32,
         len: i32,
         val_ptr: i32,
         val_len: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Cvars) {
                return -1;
            }
            let Some(name) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            let Some(value) = read_guest_str(&caller, val_ptr, val_len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetCvar { name, value });
            0
        },
    )?;

    // --- Commands (`Permission::Commands`) ---
    linker.func_wrap(
        HOST_MODULE,
        "register_command",
        |mut caller: Caller<'_, HostCallState>, ptr: i32, len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Commands) {
                return -1;
            }
            let Some(name) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::RegisterCommand { name });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "server_exec",
        |mut caller: Caller<'_, HostCallState>, ptr: i32, len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Commands) {
                return -1;
            }
            let Some(line) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::ServerExec { line });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "change_level",
        |mut caller: Caller<'_, HostCallState>, ptr: i32, len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Commands) {
                return -1;
            }
            let Some(map) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::ChangeLevel { map });
            0
        },
    )?;

    // --- Menus (`Permission::Menus`) ---
    linker.func_wrap(
        HOST_MODULE,
        "show_menu",
        |mut caller: Caller<'_, HostCallState>,
         slot: i32,
         keys: i32,
         time_sec: i32,
         ptr: i32,
         len: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Menus) {
                return -1;
            }
            let Ok(slot_u8) = u8::try_from(slot) else {
                return -1;
            };
            let Some(text) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::ShowMenu {
                slot: slot_u8,
                keys: keys as u16,
                time_sec: time_sec.clamp(-1, 127) as i8,
                text,
            });
            0
        },
    )?;

    // --- Timers (`Permission::Timers`) ---
    linker.func_wrap(
        HOST_MODULE,
        "timer_after",
        |mut caller: Caller<'_, HostCallState>, delay_ms: i32, cb_id: i32, arg: i32| -> i32 {
            if !caller.data().has_perm(Permission::Timers) || delay_ms < 0 {
                return -1;
            }
            let id = {
                let s = caller.data_mut();
                let id = s.next_local_timer_id;
                s.next_local_timer_id = s.next_local_timer_id.wrapping_add(1).max(1);
                s.timers.push(TimerRequest {
                    delay_ms: delay_ms as u32,
                    repeat: false,
                    cb_id,
                    arg,
                });
                id
            };
            id as i32
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "timer_repeat",
        |mut caller: Caller<'_, HostCallState>, interval_ms: i32, cb_id: i32, arg: i32| -> i32 {
            if !caller.data().has_perm(Permission::Timers) || interval_ms <= 0 {
                return -1;
            }
            let id = {
                let s = caller.data_mut();
                let id = s.next_local_timer_id;
                s.next_local_timer_id = s.next_local_timer_id.wrapping_add(1).max(1);
                s.timers.push(TimerRequest {
                    delay_ms: interval_ms as u32,
                    repeat: true,
                    cb_id,
                    arg,
                });
                id
            };
            id as i32
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "timer_cancel",
        |mut caller: Caller<'_, HostCallState>, timer_id: i32| -> i32 {
            if !caller.data().has_perm(Permission::Timers) {
                return -1;
            }
            let Ok(id) = u32::try_from(timer_id) else {
                return -1;
            };
            caller.data_mut().cancel_timers.push(id);
            0
        },
    )?;

    // --- Traces (`Permission::Traces`) ---
    linker.func_wrap(
        HOST_MODULE,
        "trace_line",
        |caller: Caller<'_, HostCallState>,
         x1: f32,
         y1: f32,
         z1: f32,
         x2: f32,
         y2: f32,
         z2: f32|
         -> f32 {
            if !caller.data().has_perm(Permission::Traces) {
                return -1.0;
            }
            caller.data().trace_fraction([x1, y1, z1], [x2, y2, z2])
        },
    )?;

    // --- Sound (`Permission::Sound`) ---
    linker.func_wrap(
        HOST_MODULE,
        "precache_sound",
        |mut caller: Caller<'_, HostCallState>, ptr: i32, len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Sound) {
                return -1;
            }
            let Some(sample) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::PrecacheSound { sample });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "emit_sound",
        |mut caller: Caller<'_, HostCallState>,
         ent_idx: i32,
         channel: i32,
         ptr: i32,
         len: i32,
         volume: f32|
         -> i32 {
            if !caller.data().has_perm(Permission::Sound) || !volume.is_finite() {
                return -1;
            }
            let Ok(ent_index) = u16::try_from(ent_idx) else {
                return -1;
            };
            let Ok(ch) = u8::try_from(channel) else {
                return -1;
            };
            let Some(sample) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::EmitSound {
                ent_index,
                channel: ch,
                sample,
                volume,
            });
            0
        },
    )?;

    // --- Per-mod Storage (`Permission::Storage`) ---
    linker.func_wrap(
        HOST_MODULE,
        "storage_get",
        |mut caller: Caller<'_, HostCallState>,
         key_ptr: i32,
         key_len: i32,
         out_ptr: i32,
         cap: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Storage) {
                return -1;
            }
            let Some(key) = read_guest_str(&caller, key_ptr, key_len) else {
                return -1;
            };
            let Some(val) = caller.data().storage_cache.get(&key).cloned() else {
                return -1;
            };
            write_guest_bytes(&mut caller, out_ptr, cap, val.as_bytes())
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "storage_set",
        |mut caller: Caller<'_, HostCallState>,
         key_ptr: i32,
         key_len: i32,
         val_ptr: i32,
         val_len: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Storage) {
                return -1;
            }
            let Some(key) = read_guest_str(&caller, key_ptr, key_len) else {
                return -1;
            };
            let Some(val) = read_guest_str(&caller, val_ptr, val_len) else {
                return -1;
            };
            let s = caller.data_mut();
            s.storage_cache.insert(key.clone(), val.clone());
            s.storage_writes.push((key, Some(val)));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "storage_remove",
        |mut caller: Caller<'_, HostCallState>, key_ptr: i32, key_len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Storage) {
                return -1;
            }
            let Some(key) = read_guest_str(&caller, key_ptr, key_len) else {
                return -1;
            };
            let s = caller.data_mut();
            s.storage_cache.remove(&key);
            s.storage_writes.push((key, None));
            0
        },
    )?;

    // --- Client HUD & Input (`Permission::Hud` / `Permission::Input`) ---
    linker.func_wrap(
        HOST_MODULE,
        "hud_draw_rect",
        |mut caller: Caller<'_, HostCallState>, x: f32, y: f32, w: f32, h: f32, rgba: i32| -> i32 {
            if !caller.data().has_perm(Permission::Hud)
                || !x.is_finite()
                || !y.is_finite()
                || !w.is_finite()
                || !h.is_finite()
            {
                return -1;
            }
            caller.data_mut().hud_cmds.push(HudDrawCmd::Rect {
                x,
                y,
                w,
                h,
                rgba: rgba_u32(rgba),
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "hud_draw_text",
        |mut caller: Caller<'_, HostCallState>,
         x: f32,
         y: f32,
         size: f32,
         rgba: i32,
         ptr: i32,
         len: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Hud) || !x.is_finite() || !y.is_finite() {
                return -1;
            }
            let Some(text) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().hud_cmds.push(HudDrawCmd::Text {
                x,
                y,
                size: size.clamp(8.0, 72.0),
                rgba: rgba_u32(rgba),
                text,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "hud_draw_bar",
        |mut caller: Caller<'_, HostCallState>,
         x: f32,
         y: f32,
         w: f32,
         h: f32,
         frac: f32,
         rgba: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Hud)
                || !x.is_finite()
                || !y.is_finite()
                || !w.is_finite()
                || !h.is_finite()
                || !frac.is_finite()
            {
                return -1;
            }
            caller.data_mut().hud_cmds.push(HudDrawCmd::ProgressBar {
                x,
                y,
                w,
                h,
                fraction: frac.clamp(0.0, 1.0),
                rgba: rgba_u32(rgba),
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "hud_panel",
        |mut caller: Caller<'_, HostCallState>,
         x: f32,
         y: f32,
         w: f32,
         h: f32,
         rgba: i32,
         ptr: i32,
         len: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Hud) || !x.is_finite() || !y.is_finite() {
                return -1;
            }
            let Some(title) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().hud_cmds.push(HudDrawCmd::Panel {
                x,
                y,
                w,
                h,
                rgba: rgba_u32(rgba),
                title,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "client_exec",
        |mut caller: Caller<'_, HostCallState>, ptr: i32, len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Input) {
                return -1;
            }
            let Some(line) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::ClientExec { line });
            0
        },
    )?;

    Ok(())
}

fn register_v2_player_and_weapon_natives(
    linker: &mut Linker<HostCallState>,
) -> Result<(), wasmi::Error> {
    linker.func_wrap(
        HOST_MODULE,
        "player_get_health",
        |caller: Caller<'_, HostCallState>, slot: i32| -> f32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1.0;
            }
            u8::try_from(slot)
                .ok()
                .and_then(|s| caller.data().snapshot.client(s))
                .map_or(-1.0, |c| c.health)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_health",
        |mut caller: Caller<'_, HostCallState>, slot: i32, health: f32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) || !health.is_finite() {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller
                .data_mut()
                .ops
                .push(EngineOp::SetPlayerHealth { slot, health });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_max_health",
        |mut caller: Caller<'_, HostCallState>, slot: i32, max_health: f32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) || !max_health.is_finite() {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller
                .data_mut()
                .ops
                .push(EngineOp::SetPlayerMaxHealth { slot, max_health });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_get_armor",
        |caller: Caller<'_, HostCallState>, slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            u8::try_from(slot)
                .ok()
                .and_then(|s| caller.data().snapshot.client(s))
                .map_or(-1, |c| c.armor)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_get_armor_type",
        |caller: Caller<'_, HostCallState>, slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            u8::try_from(slot)
                .ok()
                .and_then(|s| caller.data().snapshot.client(s))
                .map_or(0, |c| i32::from(c.armor_kind))
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_armor",
        |mut caller: Caller<'_, HostCallState>, slot: i32, armor: i32, kind: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetPlayerArmor {
                slot,
                armor,
                kind: ArmorKind::from_i32(kind),
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_get_money",
        |caller: Caller<'_, HostCallState>, slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            u8::try_from(slot)
                .ok()
                .and_then(|s| caller.data().snapshot.client(s))
                .map_or(-1, |c| c.money)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_money",
        |mut caller: Caller<'_, HostCallState>, slot: i32, amount: i32, flash: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetPlayerMoney {
                slot,
                amount,
                flash: flash != 0,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_add_money",
        |mut caller: Caller<'_, HostCallState>, slot: i32, delta: i32, flash: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::AddPlayerMoney {
                slot,
                delta,
                flash: flash != 0,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_get_frags",
        |caller: Caller<'_, HostCallState>, slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return 0;
            }
            u8::try_from(slot)
                .ok()
                .and_then(|s| caller.data().snapshot.client(s))
                .map_or(0, |c| c.frags)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_get_deaths",
        |caller: Caller<'_, HostCallState>, slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return 0;
            }
            u8::try_from(slot)
                .ok()
                .and_then(|s| caller.data().snapshot.client(s))
                .map_or(0, |c| c.deaths)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_score",
        |mut caller: Caller<'_, HostCallState>, slot: i32, frags: i32, deaths: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetPlayerScore {
                slot,
                frags,
                deaths,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_get_team",
        |caller: Caller<'_, HostCallState>, slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return 0;
            }
            u8::try_from(slot)
                .ok()
                .and_then(|s| caller.data().snapshot.client(s))
                .map_or(0, |c| i32::from(c.team))
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_team",
        |mut caller: Caller<'_, HostCallState>, slot: i32, team: i32, model_class: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            let cls = (model_class >= 0).then(|| model_class.min(255) as u8);
            caller.data_mut().ops.push(EngineOp::SetPlayerTeam {
                slot,
                team: TeamKind::from_i32(team),
                model_class: cls,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_is_alive",
        |caller: Caller<'_, HostCallState>, slot: i32| -> i32 {
            u8::try_from(slot)
                .ok()
                .and_then(|s| caller.data().snapshot.client(s))
                .map_or(0, |c| i32::from(c.alive))
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_respawn",
        |mut caller: Caller<'_, HostCallState>, slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::RespawnPlayer { slot });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_slap",
        |mut caller: Caller<'_, HostCallState>, slot: i32, power: i32, random_dir: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SlapPlayer {
                slot,
                power: power.max(0),
                random_dir: random_dir != 0,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_slay",
        |mut caller: Caller<'_, HostCallState>, slot: i32, preserve_frags: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SlayPlayer {
                slot,
                preserve_frags: preserve_frags != 0,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_damage",
        |mut caller: Caller<'_, HostCallState>,
         victim: i32,
         attacker: i32,
         damage: f32,
         bits: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Entities) || !damage.is_finite() || damage < 0.0 {
                return -1;
            }
            let Ok(victim) = u8::try_from(victim) else {
                return -1;
            };
            let Ok(attacker) = opt_slot(attacker) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::DamagePlayer {
                victim,
                attacker,
                damage,
                bits: bits as u32,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_godmode",
        |mut caller: Caller<'_, HostCallState>, slot: i32, enabled: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetPlayerGodmode {
                slot,
                enabled: enabled != 0,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_noclip",
        |mut caller: Caller<'_, HostCallState>, slot: i32, enabled: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetPlayerNoclip {
                slot,
                enabled: enabled != 0,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_maxspeed",
        |mut caller: Caller<'_, HostCallState>, slot: i32, maxspeed: f32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) || !maxspeed.is_finite() {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller
                .data_mut()
                .ops
                .push(EngineOp::SetPlayerMaxspeed { slot, maxspeed });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_gravity",
        |mut caller: Caller<'_, HostCallState>, slot: i32, gravity: f32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) || !gravity.is_finite() {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller
                .data_mut()
                .ops
                .push(EngineOp::SetPlayerGravity { slot, gravity });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_footsteps",
        |mut caller: Caller<'_, HostCallState>, slot: i32, silent: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetPlayerFootsteps {
                slot,
                silent: silent != 0,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_rendering",
        |mut caller: Caller<'_, HostCallState>,
         slot: i32,
         fx: i32,
         rgb_packed: i32,
         mode: i32,
         amount: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetPlayerRendering {
                slot,
                fx: fx.clamp(0, 255) as u8,
                rgb: rgb_u24(rgb_packed),
                mode: mode.clamp(0, 255) as u8,
                amount: amount.clamp(0, 255) as u8,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_model",
        |mut caller: Caller<'_, HostCallState>, slot: i32, ptr: i32, len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            let Some(model) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller
                .data_mut()
                .ops
                .push(EngineOp::SetPlayerModel { slot, model });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_reset_model",
        |mut caller: Caller<'_, HostCallState>, slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::ResetPlayerModel { slot });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_get_ip",
        |mut caller: Caller<'_, HostCallState>, slot: i32, out_ptr: i32, cap: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Some(ip) = u8::try_from(slot)
                .ok()
                .and_then(|s| caller.data().snapshot.client(s).map(|c| c.ip.clone()))
            else {
                return -1;
            };
            write_guest_bytes(&mut caller, out_ptr, cap, ip.as_bytes())
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_get_steamid",
        |mut caller: Caller<'_, HostCallState>, slot: i32, out_ptr: i32, cap: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Some(sid) = u8::try_from(slot)
                .ok()
                .and_then(|s| caller.data().snapshot.client(s).map(|c| c.steamid.clone()))
            else {
                return -1;
            };
            write_guest_bytes(&mut caller, out_ptr, cap, sid.as_bytes())
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_get_ping",
        |caller: Caller<'_, HostCallState>, slot: i32| -> i32 {
            u8::try_from(slot)
                .ok()
                .and_then(|s| caller.data().snapshot.client(s))
                .map_or(0, |c| c.ping)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_get_loss",
        |caller: Caller<'_, HostCallState>, slot: i32| -> i32 {
            u8::try_from(slot)
                .ok()
                .and_then(|s| caller.data().snapshot.client(s))
                .map_or(0, |c| c.loss)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_get_aim_origin",
        |mut caller: Caller<'_, HostCallState>, slot: i32, out_ptr: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Some(aim) = u8::try_from(slot)
                .ok()
                .and_then(|s| caller.data().snapshot.client(s).map(|c| c.aim_origin))
            else {
                return -1;
            };
            write_guest_f32_array3(&mut caller, out_ptr, aim)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_give_item",
        |mut caller: Caller<'_, HostCallState>, slot: i32, ptr: i32, len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            let Some(item) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::GiveItem { slot, item });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_strip_weapons",
        |mut caller: Caller<'_, HostCallState>, slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller
                .data_mut()
                .ops
                .push(EngineOp::StripUserWeapons { slot });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_get_weapon_clip",
        |caller: Caller<'_, HostCallState>, slot: i32, weapon_id: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            let Some(c) = caller.data().snapshot.client(slot) else {
                return -1;
            };
            if weapon_id <= 0 {
                c.clip
            } else {
                u8::try_from(weapon_id)
                    .ok()
                    .and_then(|w| c.weapon_clips.get(&w).copied())
                    .unwrap_or(-1)
            }
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_weapon_clip",
        |mut caller: Caller<'_, HostCallState>, slot: i32, weapon_id: i32, clip: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            let wid = u8::try_from(weapon_id.max(0)).unwrap_or(0);
            caller.data_mut().ops.push(EngineOp::SetWeaponClip {
                slot,
                weapon_id: wid,
                clip: clip.max(0),
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_get_bpammo",
        |caller: Caller<'_, HostCallState>, slot: i32, weapon_id: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            let Some(c) = caller.data().snapshot.client(slot) else {
                return -1;
            };
            if weapon_id <= 0 {
                c.bpammo
            } else {
                u8::try_from(weapon_id)
                    .ok()
                    .and_then(|w| c.weapon_bpammo.get(&w).copied())
                    .unwrap_or(0)
            }
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_bpammo",
        |mut caller: Caller<'_, HostCallState>, slot: i32, weapon_id: i32, amount: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            let wid = u8::try_from(weapon_id.max(0)).unwrap_or(0);
            caller.data_mut().ops.push(EngineOp::SetUserBpAmmo {
                slot,
                weapon_id: wid,
                amount: amount.max(0),
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_defuser",
        |mut caller: Caller<'_, HostCallState>, slot: i32, has: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetUserDefuser {
                slot,
                has: has != 0,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_set_nvg",
        |mut caller: Caller<'_, HostCallState>, slot: i32, has: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::SetUserNvg {
                slot,
                has: has != 0,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "player_drop_slot",
        |mut caller: Caller<'_, HostCallState>, slot: i32, inv_slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Entities) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            let Ok(inv_slot) = u8::try_from(inv_slot) else {
                return -1;
            };
            caller
                .data_mut()
                .ops
                .push(EngineOp::DropSlot { slot, inv_slot });
            0
        },
    )?;

    // --- Custom Weapons (`Permission::Weapons` or `Permission::Entities`) ---
    linker.func_wrap(
        HOST_MODULE,
        "register_custom_weapon",
        |mut caller: Caller<'_, HostCallState>, spec_ptr: i32, spec_len: i32| -> i32 {
            if !(caller.data().has_perm(Permission::Weapons)
                || caller.data().has_perm(Permission::Entities))
            {
                return -1;
            }
            let Some(raw) = read_guest_str(&caller, spec_ptr, spec_len) else {
                return -1;
            };
            let Some(spec) = parse_custom_weapon_spec(&raw) else {
                return -1;
            };
            caller
                .data_mut()
                .ops
                .push(EngineOp::RegisterCustomWeapon(spec));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "give_custom_weapon",
        |mut caller: Caller<'_, HostCallState>, slot: i32, id_ptr: i32, id_len: i32| -> i32 {
            if !(caller.data().has_perm(Permission::Weapons)
                || caller.data().has_perm(Permission::Entities))
            {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            let Some(id_name) = read_guest_str(&caller, id_ptr, id_len) else {
                return -1;
            };
            caller
                .data_mut()
                .ops
                .push(EngineOp::GiveCustomWeapon { slot, id_name });
            0
        },
    )?;

    Ok(())
}

fn register_v2_vfx_and_ui_natives(linker: &mut Linker<HostCallState>) -> Result<(), wasmi::Error> {
    // --- 1-Line VFX Primitives (`Permission::Vfx`) ---
    linker.func_wrap(
        HOST_MODULE,
        "vfx_shockwave_ring",
        |mut caller: Caller<'_, HostCallState>,
         x: f32,
         y: f32,
         z: f32,
         radius: f32,
         rgba: i32,
         life: i32,
         width: i32,
         slot: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Vfx)
                || !x.is_finite()
                || !y.is_finite()
                || !z.is_finite()
                || !radius.is_finite()
            {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::EmitVfx(VfxOp::ShockwaveRing {
                origin: [x, y, z],
                radius,
                rgba: rgba_u32(rgba),
                life_decisec: life.clamp(1, 255) as u8,
                width: width.clamp(1, 255) as u8,
                slot: target,
            }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "vfx_laser_beam",
        |mut caller: Caller<'_, HostCallState>,
         x1: f32,
         y1: f32,
         z1: f32,
         x2: f32,
         y2: f32,
         z2: f32,
         rgba: i32,
         life: i32,
         width: i32,
         noise: i32,
         slot: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Vfx)
                || !x1.is_finite()
                || !y1.is_finite()
                || !z1.is_finite()
                || !x2.is_finite()
                || !y2.is_finite()
                || !z2.is_finite()
            {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::EmitVfx(VfxOp::LaserBeam {
                start: [x1, y1, z1],
                end: [x2, y2, z2],
                rgba: rgba_u32(rgba),
                life_decisec: life.clamp(1, 255) as u8,
                width: width.clamp(1, 255) as u8,
                noise: noise.clamp(0, 255) as u8,
                slot: target,
            }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "vfx_follow_trail",
        |mut caller: Caller<'_, HostCallState>,
         ent_idx: i32,
         rgba: i32,
         life: i32,
         width: i32,
         slot: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Vfx) {
                return -1;
            }
            let Ok(ent_index) = u16::try_from(ent_idx) else {
                return -1;
            };
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::EmitVfx(VfxOp::FollowTrail {
                ent_index,
                rgba: rgba_u32(rgba),
                life_decisec: life.clamp(1, 255) as u8,
                width: width.clamp(1, 255) as u8,
                slot: target,
            }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "vfx_kill_beam",
        |mut caller: Caller<'_, HostCallState>, ent_idx: i32, slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Vfx) {
                return -1;
            }
            let Ok(ent_index) = u16::try_from(ent_idx) else {
                return -1;
            };
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::EmitVfx(VfxOp::KillBeam {
                ent_index,
                slot: target,
            }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "vfx_aura_light",
        |mut caller: Caller<'_, HostCallState>,
         x: f32,
         y: f32,
         z: f32,
         radius: i32,
         rgb: i32,
         life: i32,
         decay: i32,
         slot: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Vfx)
                || !x.is_finite()
                || !y.is_finite()
                || !z.is_finite()
            {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::EmitVfx(VfxOp::AuraLight {
                origin: [x, y, z],
                radius: radius.clamp(1, 255) as u8,
                rgb: rgb_u24(rgb),
                life_decisec: life.clamp(1, 255) as u8,
                decay: decay.clamp(0, 255) as u8,
                slot: target,
            }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "vfx_sparks",
        |mut caller: Caller<'_, HostCallState>, x: f32, y: f32, z: f32, slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Vfx)
                || !x.is_finite()
                || !y.is_finite()
                || !z.is_finite()
            {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::EmitVfx(VfxOp::Sparks {
                origin: [x, y, z],
                slot: target,
            }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "vfx_implosion",
        |mut caller: Caller<'_, HostCallState>,
         x: f32,
         y: f32,
         z: f32,
         radius: i32,
         count: i32,
         life: i32,
         slot: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Vfx)
                || !x.is_finite()
                || !y.is_finite()
                || !z.is_finite()
            {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::EmitVfx(VfxOp::Implosion {
                origin: [x, y, z],
                radius: radius.clamp(1, 255) as u8,
                count: count.clamp(1, 255) as u8,
                life_decisec: life.clamp(1, 255) as u8,
                slot: target,
            }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "vfx_teleport_splash",
        |mut caller: Caller<'_, HostCallState>, x: f32, y: f32, z: f32, slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Vfx)
                || !x.is_finite()
                || !y.is_finite()
                || !z.is_finite()
            {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            caller
                .data_mut()
                .ops
                .push(EngineOp::EmitVfx(VfxOp::TeleportSplash {
                    origin: [x, y, z],
                    slot: target,
                }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "vfx_lava_splash",
        |mut caller: Caller<'_, HostCallState>, x: f32, y: f32, z: f32, slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Vfx)
                || !x.is_finite()
                || !y.is_finite()
                || !z.is_finite()
            {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::EmitVfx(VfxOp::LavaSplash {
                origin: [x, y, z],
                slot: target,
            }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "vfx_explosion",
        |mut caller: Caller<'_, HostCallState>,
         x: f32,
         y: f32,
         z: f32,
         scale: i32,
         flags: i32,
         slot: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Vfx)
                || !x.is_finite()
                || !y.is_finite()
                || !z.is_finite()
            {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::EmitVfx(VfxOp::Explosion {
                origin: [x, y, z],
                scale: scale.clamp(1, 255) as u8,
                flags: flags.clamp(0, 255) as u8,
                slot: target,
            }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "vfx_blood_stream",
        |mut caller: Caller<'_, HostCallState>,
         x: f32,
         y: f32,
         z: f32,
         dx: f32,
         dy: f32,
         dz: f32,
         color: i32,
         speed: i32,
         slot: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Vfx)
                || !x.is_finite()
                || !y.is_finite()
                || !z.is_finite()
                || !dx.is_finite()
                || !dy.is_finite()
                || !dz.is_finite()
            {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::EmitVfx(VfxOp::BloodStream {
                origin: [x, y, z],
                dir: [dx, dy, dz],
                color: color.clamp(0, 255) as u8,
                speed: speed.clamp(0, 255) as u8,
                slot: target,
            }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "vfx_world_decal",
        |mut caller: Caller<'_, HostCallState>,
         x: f32,
         y: f32,
         z: f32,
         name_ptr: i32,
         name_len: i32,
         slot: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Vfx)
                || !x.is_finite()
                || !y.is_finite()
                || !z.is_finite()
            {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            let Some(decal_name) = read_guest_str(&caller, name_ptr, name_len) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::EmitVfx(VfxOp::WorldDecal {
                origin: [x, y, z],
                decal_name,
                slot: target,
            }));
            0
        },
    )?;

    // --- Paginated MenuManager (`Permission::Menus`) ---
    linker.func_wrap(
        HOST_MODULE,
        "menu_create",
        |caller: Caller<'_, HostCallState>, title_ptr: i32, title_len: i32, cb_id: i32| -> i32 {
            if !caller.data().has_perm(Permission::Menus) {
                return -1;
            }
            let Some(title) = read_guest_str(&caller, title_ptr, title_len) else {
                return -1;
            };
            let owner_id = caller.data().mod_id.clone();
            let spec = MenuCreateSpec {
                title,
                callback_id: cb_id,
                ..Default::default()
            };
            let mut svc = caller.data().services.lock();
            svc.menus
                .create_menu(owner_id, PluginRuntimeKind::Wasm, 0, spec) as i32
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "menu_add_item",
        |caller: Caller<'_, HostCallState>,
         menu_id: i32,
         label_ptr: i32,
         label_len: i32,
         info_ptr: i32,
         info_len: i32,
         enabled: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Menus) {
                return -1;
            }
            let Ok(id) = u32::try_from(menu_id) else {
                return -1;
            };
            let Some(label) = read_guest_str(&caller, label_ptr, label_len) else {
                return -1;
            };
            let info = if info_len <= 0 {
                String::new()
            } else {
                match read_guest_str(&caller, info_ptr, info_len) {
                    Some(s) => s,
                    None => return -1,
                }
            };
            let mut svc = caller.data().services.lock();
            if svc
                .menus
                .add_item(id, MenuItemSpec::selectable(label, info, enabled != 0))
            {
                0
            } else {
                -1
            }
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "menu_add_blank",
        |caller: Caller<'_, HostCallState>, menu_id: i32| -> i32 {
            if !caller.data().has_perm(Permission::Menus) {
                return -1;
            }
            let Ok(id) = u32::try_from(menu_id) else {
                return -1;
            };
            let mut svc = caller.data().services.lock();
            if svc.menus.add_item(id, MenuItemSpec::blank()) {
                0
            } else {
                -1
            }
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "menu_add_text",
        |caller: Caller<'_, HostCallState>, menu_id: i32, ptr: i32, len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Menus) {
                return -1;
            }
            let Ok(id) = u32::try_from(menu_id) else {
                return -1;
            };
            let Some(label) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            let mut svc = caller.data().services.lock();
            if svc.menus.add_item(id, MenuItemSpec::text(label)) {
                0
            } else {
                -1
            }
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "menu_set_prop_int",
        |caller: Caller<'_, HostCallState>, menu_id: i32, prop: i32, val: i32| -> i32 {
            if !caller.data().has_perm(Permission::Menus) {
                return -1;
            }
            let Ok(id) = u32::try_from(menu_id) else {
                return -1;
            };
            let mut svc = caller.data().services.lock();
            if svc.menus.set_prop_int(id, prop, val) {
                0
            } else {
                -1
            }
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "menu_set_prop_str",
        |caller: Caller<'_, HostCallState>,
         menu_id: i32,
         prop: i32,
         val_ptr: i32,
         val_len: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Menus) {
                return -1;
            }
            let Ok(id) = u32::try_from(menu_id) else {
                return -1;
            };
            let Some(val) = read_guest_str(&caller, val_ptr, val_len) else {
                return -1;
            };
            let mut svc = caller.data().services.lock();
            if svc.menus.set_prop_str(id, prop, val) {
                0
            } else {
                -1
            }
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "menu_display",
        |mut caller: Caller<'_, HostCallState>,
         slot: i32,
         menu_id: i32,
         page: i32,
         time_sec: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Menus) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            let Ok(id) = u32::try_from(menu_id) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::MenuDisplay(MenuDisplaySpec {
                slot,
                menu_id: id,
                page: page.max(0).min(i32::from(u16::MAX)) as u16,
                time_sec: time_sec.clamp(-1, 127) as i8,
            }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "menu_destroy",
        |caller: Caller<'_, HostCallState>, menu_id: i32| -> i32 {
            if !caller.data().has_perm(Permission::Menus) {
                return -1;
            }
            let Ok(id) = u32::try_from(menu_id) else {
                return -1;
            };
            let mut svc = caller.data().services.lock();
            if MenuManager::destroy_menu(&mut svc.menus, id) {
                0
            } else {
                -1
            }
        },
    )?;

    // --- HUD / DHUD / MOTD / Screen Effects (`Permission::Messages`) ---
    linker.func_wrap(
        HOST_MODULE,
        "hud_message",
        |mut caller: Caller<'_, HostCallState>,
         slot: i32,
         channel_effect: i32,
         x: f32,
         y: f32,
         rgba1: i32,
         rgba2: i32,
         fadein: f32,
         hold: f32,
         fadeout: f32,
         fxtime: f32,
         ptr: i32,
         len: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Messages)
                || !x.is_finite()
                || !y.is_finite()
                || !fadein.is_finite()
                || !hold.is_finite()
                || !fadeout.is_finite()
                || !fxtime.is_finite()
            {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            let Some(text) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            let channel = (channel_effect & 0xFF) as u8;
            let effect = ((channel_effect >> 8) & 0xFF) as u8;
            caller.data_mut().ops.push(EngineOp::HudMessage(HudMessageSpec {
                slot: target,
                channel,
                x,
                y,
                rgba1: rgba_u32(rgba1),
                rgba2: rgba_u32(rgba2),
                effect,
                fadein: fadein.max(0.0),
                hold: hold.max(0.0),
                fadeout: fadeout.max(0.0),
                fxtime: fxtime.max(0.0),
                text,
            }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "dhud_message",
        |mut caller: Caller<'_, HostCallState>,
         slot: i32,
         rgb_effect: i32,
         x: f32,
         y: f32,
         fadein: f32,
         hold: f32,
         fadeout: f32,
         fxtime: f32,
         ptr: i32,
         len: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Messages)
                || !x.is_finite()
                || !y.is_finite()
                || !fadein.is_finite()
                || !hold.is_finite()
                || !fadeout.is_finite()
                || !fxtime.is_finite()
            {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            let Some(text) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            let effect = (((rgb_effect as u32) >> 24) & 0xFF) as u8;
            caller
                .data_mut()
                .ops
                .push(EngineOp::DhudMessage(DhudMessageSpec {
                    slot: target,
                    x,
                    y,
                    rgb: rgb_u24(rgb_effect),
                    effect,
                    fadein: fadein.max(0.0),
                    hold: hold.max(0.0),
                    fadeout: fadeout.max(0.0),
                    fxtime: fxtime.max(0.0),
                    text,
                }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "show_motd",
        |mut caller: Caller<'_, HostCallState>,
         slot: i32,
         title_ptr: i32,
         title_len: i32,
         body_ptr: i32,
         body_len: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Messages) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            let Some(title) = read_guest_str(&caller, title_ptr, title_len) else {
                return -1;
            };
            let Some(body) = read_guest_str(&caller, body_ptr, body_len) else {
                return -1;
            };
            caller
                .data_mut()
                .ops
                .push(EngineOp::ShowMotd(ShowMotdSpec { slot, title, body }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "screen_fade",
        |mut caller: Caller<'_, HostCallState>,
         slot: i32,
         duration_sec: f32,
         hold_sec: f32,
         flags: i32,
         rgba: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Messages)
                || !duration_sec.is_finite()
                || !hold_sec.is_finite()
            {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::ScreenFade(ScreenFadeSpec {
                slot: target,
                duration_sec: duration_sec.max(0.0),
                hold_sec: hold_sec.max(0.0),
                flags: flags.clamp(0, i32::from(u16::MAX)) as u16,
                rgba: rgba_u32(rgba),
            }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "screen_shake",
        |mut caller: Caller<'_, HostCallState>,
         slot: i32,
         amplitude: f32,
         duration_sec: f32,
         frequency: f32|
         -> i32 {
            if !caller.data().has_perm(Permission::Messages)
                || !amplitude.is_finite()
                || !duration_sec.is_finite()
                || !frequency.is_finite()
            {
                return -1;
            }
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            caller
                .data_mut()
                .ops
                .push(EngineOp::ScreenShake(ScreenShakeSpec {
                    slot: target,
                    amplitude: amplitude.max(0.0),
                    duration_sec: duration_sec.max(0.0),
                    frequency: frequency.max(0.0),
                }));
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "bar_time",
        |mut caller: Caller<'_, HostCallState>, slot: i32, seconds: i32| -> i32 {
            if !caller.data().has_perm(Permission::Messages) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            caller.data_mut().ops.push(EngineOp::BarTime(BarTimeSpec {
                slot,
                seconds: seconds.clamp(0, i32::from(u16::MAX)) as u16,
            }));
            0
        },
    )?;

    Ok(())
}

fn register_v2_platform_natives(linker: &mut Linker<HostCallState>) -> Result<(), wasmi::Error> {
    // --- Unified Localization (`LangCatalog`) ---
    linker.func_wrap(
        HOST_MODULE,
        "lang_load_toml",
        |caller: Caller<'_, HostCallState>, ptr: i32, len: i32| -> i32 {
            let Some(src) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            let mut svc = caller.data().services.lock();
            svc.lang.load_toml_str(&src) as i32
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "lang_load_amxx",
        |caller: Caller<'_, HostCallState>, ptr: i32, len: i32| -> i32 {
            let Some(src) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            let mut svc = caller.data().services.lock();
            svc.lang.load_amxx_txt_str(&src) as i32
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "lang_tr",
        |mut caller: Caller<'_, HostCallState>,
         slot: i32,
         key_ptr: i32,
         key_len: i32,
         args_ptr: i32,
         args_len: i32,
         out_ptr: i32,
         cap: i32|
         -> i32 {
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            let Some(key) = read_guest_str(&caller, key_ptr, key_len) else {
                return -1;
            };
            let parsed_args = if args_len <= 0 {
                Vec::new()
            } else {
                match read_guest_str(&caller, args_ptr, args_len) {
                    Some(raw) => parse_lang_args(&raw),
                    None => return -1,
                }
            };
            let borrowed: Vec<(&str, &str)> = parsed_args
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let text = {
                let svc = caller.data().services.lock();
                svc.lang.tr(target, &key, &borrowed)
            };
            write_guest_bytes(&mut caller, out_ptr, cap, text.as_bytes())
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "lang_player_code",
        |mut caller: Caller<'_, HostCallState>, slot: i32, out_ptr: i32, cap: i32| -> i32 {
            let Ok(target) = opt_slot(slot) else {
                return -1;
            };
            let code = {
                let svc = caller.data().services.lock();
                svc.lang.player_lang(target).to_string()
            };
            write_guest_bytes(&mut caller, out_ptr, cap, code.as_bytes())
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "lang_set_player_code",
        |caller: Caller<'_, HostCallState>, slot: i32, ptr: i32, len: i32| -> i32 {
            let Ok(slot) = u8::try_from(slot) else {
                return -1;
            };
            let Some(code) = read_guest_str(&caller, ptr, len) else {
                return -1;
            };
            let mut svc = caller.data().services.lock();
            if svc.lang.set_player_lang(slot, None, &code).is_some() {
                0
            } else {
                -1
            }
        },
    )?;

    // --- Admin RBAC (`Permission::Admins`) ---
    linker.func_wrap(
        HOST_MODULE,
        "admin_player_flags",
        |caller: Caller<'_, HostCallState>, slot: i32| -> i32 {
            if !caller.data().has_perm(Permission::Admins) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return 0;
            };
            let svc = caller.data().services.lock();
            svc.admins.player_flags(slot).bits() as i32
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "admin_has_flag",
        |caller: Caller<'_, HostCallState>, slot: i32, flag_char: i32| -> i32 {
            if !caller.data().has_perm(Permission::Admins) {
                return -1;
            }
            let Ok(slot) = u8::try_from(slot) else {
                return 0;
            };
            let Some(ch) = u32::try_from(flag_char).ok().and_then(char::from_u32) else {
                return 0;
            };
            let svc = caller.data().services.lock();
            i32::from(svc.admins.player_has_flag(slot, ch))
        },
    )?;

    // --- Polyglot RPC & Events (`Permission::Rpc`) ---
    linker.func_wrap(
        HOST_MODULE,
        "rpc_export",
        |caller: Caller<'_, HostCallState>, name_ptr: i32, name_len: i32, cb_id: i32| -> i32 {
            if !caller.data().has_perm(Permission::Rpc) {
                return -1;
            }
            let Some(fn_name) = read_guest_str(&caller, name_ptr, name_len) else {
                return -1;
            };
            let owner_id = caller.data().mod_id.clone();
            let mut svc = caller.data().services.lock();
            svc.rpc.export_fn(RpcExportEntry {
                fn_name: fn_name.clone(),
                owner_id,
                runtime: PluginRuntimeKind::Wasm,
                owner_index: 0,
                callback_id: cb_id,
                callback_name: fn_name,
            });
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "rpc_call",
        |mut caller: Caller<'_, HostCallState>,
         name_ptr: i32,
         name_len: i32,
         arg_i32: i32,
         arg_ptr: i32,
         arg_len: i32,
         out_ptr: i32,
         out_cap: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Rpc) {
                return -1;
            }
            let Some(fn_name) = read_guest_str(&caller, name_ptr, name_len) else {
                return -1;
            };
            let mut args = vec![RpcValue::Int(arg_i32)];
            if arg_len > 0 {
                let Some(arg_str) = read_guest_str(&caller, arg_ptr, arg_len) else {
                    return -1;
                };
                args.push(RpcValue::Str(arg_str));
            }
            let dispatcher = caller.data().rpc_dispatcher.clone();
            let host_direct = {
                let svc = caller.data().services.lock();
                svc.rpc.call_host_fn(&fn_name, &args)
            };
            let res = host_direct.or_else(|| dispatcher.and_then(|d| d.call_rpc(&fn_name, &args)));
            match res {
                Some(RpcValue::Int(v)) => v,
                Some(RpcValue::Float(f)) => f as i32,
                Some(RpcValue::Str(s)) => {
                    if out_cap > 0 {
                        write_guest_bytes(&mut caller, out_ptr, out_cap, s.as_bytes())
                    } else {
                        s.len() as i32
                    }
                }
                Some(RpcValue::Bytes(b)) => {
                    if out_cap > 0 {
                        write_guest_bytes(&mut caller, out_ptr, out_cap, &b)
                    } else {
                        b.len() as i32
                    }
                }
                None => -1,
            }
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "rpc_subscribe",
        |caller: Caller<'_, HostCallState>, ev_ptr: i32, ev_len: i32, cb_id: i32| -> i32 {
            if !caller.data().has_perm(Permission::Rpc) {
                return -1;
            }
            let Some(event_name) = read_guest_str(&caller, ev_ptr, ev_len) else {
                return -1;
            };
            let owner_id = caller.data().mod_id.clone();
            let mut svc = caller.data().services.lock();
            svc.rpc.subscribe_event(
                event_name.clone(),
                RpcEventSubscriber {
                    owner_id,
                    runtime: PluginRuntimeKind::Wasm,
                    owner_index: 0,
                    callback_id: cb_id,
                    callback_name: event_name,
                },
            );
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "rpc_emit_event",
        |mut caller: Caller<'_, HostCallState>,
         ev_ptr: i32,
         ev_len: i32,
         payload_ptr: i32,
         payload_len: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Rpc) {
                return -1;
            }
            let Some(event_name) = read_guest_str(&caller, ev_ptr, ev_len) else {
                return -1;
            };
            let payload_json = if payload_len <= 0 {
                String::new()
            } else {
                match read_guest_str(&caller, payload_ptr, payload_len) {
                    Some(s) => s,
                    None => return -1,
                }
            };
            caller
                .data_mut()
                .ops
                .push(EngineOp::RpcEmitEvent(RpcEmitEventSpec {
                    event_name,
                    payload_json: payload_json.clone(),
                    args: vec![RpcValue::Str(payload_json)],
                }));
            0
        },
    )?;

    // --- Shared NVault & O(log N) LeaderboardStore (`Permission::Storage`) ---
    linker.func_wrap(
        HOST_MODULE,
        "nvault_open",
        |caller: Caller<'_, HostCallState>, name_ptr: i32, name_len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Storage) {
                return -1;
            }
            let Some(name) = read_guest_str(&caller, name_ptr, name_len) else {
                return -1;
            };
            let mut svc = caller.data().services.lock();
            svc.nvault.open(&name)
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "nvault_get",
        |mut caller: Caller<'_, HostCallState>,
         handle: i32,
         key_ptr: i32,
         key_len: i32,
         out_ptr: i32,
         cap: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Storage) {
                return -1;
            }
            let Some(key) = read_guest_str(&caller, key_ptr, key_len) else {
                return -1;
            };
            let val = {
                let svc = caller.data().services.lock();
                svc.nvault.get(handle, &key).map(str::to_string)
            };
            let Some(val) = val else {
                return -1;
            };
            write_guest_bytes(&mut caller, out_ptr, cap, val.as_bytes())
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "nvault_set",
        |caller: Caller<'_, HostCallState>,
         handle: i32,
         key_ptr: i32,
         key_len: i32,
         val_ptr: i32,
         val_len: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Storage) {
                return -1;
            }
            let Some(key) = read_guest_str(&caller, key_ptr, key_len) else {
                return -1;
            };
            let Some(val) = read_guest_str(&caller, val_ptr, val_len) else {
                return -1;
            };
            let now_ts = (caller.data().snapshot.time_secs.round() as i64).max(1);
            let mut svc = caller.data().services.lock();
            if svc.nvault.set(handle, key, val, now_ts) {
                0
            } else {
                -1
            }
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "nvault_remove",
        |caller: Caller<'_, HostCallState>, handle: i32, key_ptr: i32, key_len: i32| -> i32 {
            if !caller.data().has_perm(Permission::Storage) {
                return -1;
            }
            let Some(key) = read_guest_str(&caller, key_ptr, key_len) else {
                return -1;
            };
            let mut svc = caller.data().services.lock();
            if svc.nvault.remove(handle, &key) { 0 } else { -1 }
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "leaderboard_upsert",
        |caller: Caller<'_, HostCallState>,
         table_ptr: i32,
         table_len: i32,
         key_ptr: i32,
         key_len: i32,
         score: f64,
         payload_ptr: i32,
         payload_len: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Storage) || !score.is_finite() {
                return -1;
            }
            let Some(table) = read_guest_str(&caller, table_ptr, table_len) else {
                return -1;
            };
            let Some(key) = read_guest_str(&caller, key_ptr, key_len) else {
                return -1;
            };
            let payload = if payload_len <= 0 {
                String::new()
            } else {
                match read_guest_str(&caller, payload_ptr, payload_len) {
                    Some(s) => s,
                    None => return -1,
                }
            };
            let mut svc = caller.data().services.lock();
            svc.leaderboards.upsert(&table, key, score, payload);
            0
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "leaderboard_rank",
        |mut caller: Caller<'_, HostCallState>,
         table_ptr: i32,
         table_len: i32,
         key_ptr: i32,
         key_len: i32,
         out_ptr: i32,
         out_cap: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Storage) {
                return -1;
            }
            let Some(table) = read_guest_str(&caller, table_ptr, table_len) else {
                return -1;
            };
            let Some(key) = read_guest_str(&caller, key_ptr, key_len) else {
                return -1;
            };
            let res = {
                let svc = caller.data().services.lock();
                svc.leaderboards.rank(&table, &key)
            };
            let Some((entry, total)) = res else {
                return 0;
            };
            if out_cap > 0 {
                let formatted = format!(
                    "{}\t{}\t{}\t{}",
                    entry.rank, total, entry.score, entry.payload_json
                );
                let _ = write_guest_bytes(&mut caller, out_ptr, out_cap, formatted.as_bytes());
            }
            entry.rank as i32
        },
    )?;

    linker.func_wrap(
        HOST_MODULE,
        "leaderboard_top",
        |mut caller: Caller<'_, HostCallState>,
         table_ptr: i32,
         table_len: i32,
         offset: i32,
         limit: i32,
         out_ptr: i32,
         out_cap: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Storage) {
                return -1;
            }
            let Some(table) = read_guest_str(&caller, table_ptr, table_len) else {
                return -1;
            };
            let rows = {
                let svc = caller.data().services.lock();
                svc.leaderboards
                    .top(&table, offset.max(0) as usize, limit.clamp(1, 200) as usize)
            };
            let mut text = String::new();
            for row in rows {
                text.push_str(&format!(
                    "{}\t{}\t{}\t{}\n",
                    row.rank, row.key, row.score, row.payload_json
                ));
            }
            write_guest_bytes(&mut caller, out_ptr, out_cap, text.as_bytes())
        },
    )?;

    // --- Capability-Gated Non-Blocking Webhooks (`permissions = ["http:<host>"]`) ---
    linker.func_wrap(
        HOST_MODULE,
        "http_post_json",
        |mut caller: Caller<'_, HostCallState>,
         url_ptr: i32,
         url_len: i32,
         body_ptr: i32,
         body_len: i32,
         cb_id: i32|
         -> i32 {
            if !caller.data().has_perm(Permission::Http) {
                return -1;
            }
            let Some(url) = read_guest_str(&caller, url_ptr, url_len) else {
                return -1;
            };
            let Some(body_bytes) =
                read_guest_bytes(&caller, body_ptr, body_len, WEBHOOK_MAX_REQUEST_BYTES)
            else {
                return -1;
            };
            let Ok(body) = String::from_utf8(body_bytes) else {
                return -1;
            };
            let Ok(parsed) = parse_webhook_url(&url) else {
                return -1;
            };
            if !is_host_allowed(&caller.data().http_hosts, &parsed.host, parsed.port) {
                return -1;
            }
            caller
                .data_mut()
                .ops
                .push(EngineOp::HttpPostJson(HttpPostJsonSpec {
                    url,
                    body,
                    callback_id: cb_id,
                }));
            0
        },
    )?;

    Ok(())
}

/// Registers minimal safe `"wasi_snapshot_preview1"` shims so Go (`GOOS=wasip1 GOARCH=wasm`)
/// and C++ WASI `.wasm` plugins instantiate and run cleanly inside the deterministic sandbox.
fn register_wasi_preview1(linker: &mut Linker<HostCallState>) -> Result<(), wasmi::Error> {
    const WASI_ESUCCESS: i32 = 0;
    const WASI_EBADF: i32 = 8;
    const WASI_EFAULT: i32 = 21;
    const WASI_ESPIPE: i32 = 70;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "fd_write",
        |mut caller: Caller<'_, HostCallState>,
         fd: i32,
         iovs_ptr: i32,
         iovs_len: i32,
         nwritten_ptr: i32|
         -> i32 {
            if fd != 1 && fd != 2 {
                return WASI_EBADF;
            }
            let Ok(count) = usize::try_from(iovs_len) else {
                return WASI_EFAULT;
            };
            let mut combined = Vec::new();
            let mut total_written: u32 = 0;
            for i in 0..count.min(64) {
                let entry_ptr = match iovs_ptr.checked_add((i * 8) as i32) {
                    Some(p) => p,
                    None => return WASI_EFAULT,
                };
                let (Some(buf_ptr), Some(buf_len)) = (
                    read_guest_u32(&caller, entry_ptr),
                    read_guest_u32(&caller, entry_ptr.wrapping_add(4)),
                ) else {
                    return WASI_EFAULT;
                };
                let clamped_len = (buf_len as usize).min(4096);
                let Some(chunk) =
                    read_guest_bytes(&caller, buf_ptr as i32, clamped_len as i32, 4096)
                else {
                    return WASI_EFAULT;
                };
                total_written = total_written.saturating_add(chunk.len() as u32);
                if combined.len() < 4096 {
                    let take = (4096 - combined.len()).min(chunk.len());
                    combined.extend_from_slice(&chunk[..take]);
                }
            }
            if !write_guest_u32(&mut caller, nwritten_ptr, total_written) {
                return WASI_EFAULT;
            }
            let line = String::from_utf8_lossy(&combined).trim_end_matches(['\r', '\n']).to_string();
            if !line.is_empty() {
                let mod_id = caller.data().mod_id.clone();
                info!(mod_id = %mod_id, fd, "[wasi] {line}");
                let logs = &mut caller.data_mut().wasi_logs;
                if logs.len() < 64 {
                    logs.push(line);
                }
            }
            WASI_ESUCCESS
        },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "clock_time_get",
        |mut caller: Caller<'_, HostCallState>,
         _clock_id: i32,
         _precision: i64,
         out_ptr: i32|
         -> i32 {
            let ns = (caller.data().snapshot.time_secs.max(0.0) * 1_000_000_000.0) as u64;
            if write_guest_u64(&mut caller, out_ptr, ns) {
                WASI_ESUCCESS
            } else {
                WASI_EFAULT
            }
        },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "clock_res_get",
        |mut caller: Caller<'_, HostCallState>, _clock_id: i32, out_ptr: i32| -> i32 {
            if write_guest_u64(&mut caller, out_ptr, 1_000_000) {
                WASI_ESUCCESS
            } else {
                WASI_EFAULT
            }
        },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "environ_sizes_get",
        |mut caller: Caller<'_, HostCallState>, count_ptr: i32, buf_size_ptr: i32| -> i32 {
            if write_guest_u32(&mut caller, count_ptr, 0)
                && write_guest_u32(&mut caller, buf_size_ptr, 0)
            {
                WASI_ESUCCESS
            } else {
                WASI_EFAULT
            }
        },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "environ_get",
        |_caller: Caller<'_, HostCallState>, _environ: i32, _buf: i32| -> i32 { WASI_ESUCCESS },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "args_sizes_get",
        |mut caller: Caller<'_, HostCallState>, argc_ptr: i32, argv_buf_size_ptr: i32| -> i32 {
            if write_guest_u32(&mut caller, argc_ptr, 0)
                && write_guest_u32(&mut caller, argv_buf_size_ptr, 0)
            {
                WASI_ESUCCESS
            } else {
                WASI_EFAULT
            }
        },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "args_get",
        |_caller: Caller<'_, HostCallState>, _argv: i32, _buf: i32| -> i32 { WASI_ESUCCESS },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "proc_exit",
        |_caller: Caller<'_, HostCallState>, code: i32| -> Result<(), wasmi::Error> {
            Err(wasmi::Error::i32_exit(code))
        },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "random_get",
        |mut caller: Caller<'_, HostCallState>, buf_ptr: i32, buf_len: i32| -> i32 {
            let Ok(len) = usize::try_from(buf_len) else {
                return WASI_EFAULT;
            };
            let clamped = len.min(4096);
            let mut bytes = Vec::with_capacity(clamped);
            for _ in 0..clamped {
                bytes.push(caller.data_mut().next_wasi_rand_byte());
            }
            if write_guest_bytes(&mut caller, buf_ptr, clamped as i32, &bytes) == clamped as i32 {
                WASI_ESUCCESS
            } else {
                WASI_EFAULT
            }
        },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "fd_fdstat_get",
        |mut caller: Caller<'_, HostCallState>, fd: i32, out_ptr: i32| -> i32 {
            if !(0..=2).contains(&fd) {
                return WASI_EBADF;
            }
            // WASI fdstat is 24 bytes: filetype=2 (character_device), flags=0, rights_base, rights_inheriting.
            let mut stat = [0u8; 24];
            stat[0] = 2;
            if write_guest_bytes(&mut caller, out_ptr, 24, &stat) == 24 {
                WASI_ESUCCESS
            } else {
                WASI_EFAULT
            }
        },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "fd_fdstat_set_flags",
        |_caller: Caller<'_, HostCallState>, _fd: i32, _flags: i32| -> i32 { WASI_ESUCCESS },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "fd_close",
        |_caller: Caller<'_, HostCallState>, _fd: i32| -> i32 { WASI_EBADF },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "fd_read",
        |mut caller: Caller<'_, HostCallState>,
         _fd: i32,
         _iovs_ptr: i32,
         _iovs_len: i32,
         nread_ptr: i32|
         -> i32 {
            if write_guest_u32(&mut caller, nread_ptr, 0) {
                WASI_ESUCCESS
            } else {
                WASI_EFAULT
            }
        },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "fd_seek",
        |_caller: Caller<'_, HostCallState>,
         _fd: i32,
         _offset: i64,
         _whence: i32,
         _newoffset_ptr: i32|
         -> i32 { WASI_ESPIPE },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "fd_prestat_get",
        |_caller: Caller<'_, HostCallState>, _fd: i32, _out_ptr: i32| -> i32 { WASI_EBADF },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "fd_prestat_dir_name",
        |_caller: Caller<'_, HostCallState>, _fd: i32, _path_ptr: i32, _path_len: i32| -> i32 {
            WASI_EBADF
        },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "sched_yield",
        |_caller: Caller<'_, HostCallState>| -> i32 { WASI_ESUCCESS },
    )?;

    linker.func_wrap(
        WASI_PREVIEW1_MODULE,
        "poll_oneoff",
        |mut caller: Caller<'_, HostCallState>,
         _in_ptr: i32,
         _out_ptr: i32,
         _nsub: i32,
         nevents_ptr: i32|
         -> i32 {
            if write_guest_u32(&mut caller, nevents_ptr, 0) {
                WASI_ESUCCESS
            } else {
                WASI_EFAULT
            }
        },
    )?;

    Ok(())
}
