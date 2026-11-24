//! Sandboxed mod execution engine seam ([`ModRuntime`] / [`ModInstance`]) and its
//! deterministic `wasmi` 2.0 implementation ([`WasmiRuntime`]).

use std::collections::BTreeMap;

use cs_mod_api::{
    HOST_MODULE, HookAction, HookId, HookPhase, HudDrawCmd, Permission, Permissions,
};
use thiserror::Error;
use wasmi::{Caller, Config, Engine, Instance, Linker, Module, Store, StoreLimitsBuilder};

use crate::natives::{
    ActiveEvent, EngineOp, EngineSnapshot, HostCallState, HookRegistration, MAX_HOST_PAYLOAD_BYTES,
    TimerRequest, read_guest_bytes, read_guest_str, write_guest_bytes, write_guest_f32_array3,
};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    #[error("failed to compile WASM module `{0}`: {1}")]
    Compile(String, String),
    #[error("failed to instantiate WASM module `{0}`: {1}")]
    Instantiate(String, String),
    #[error("WASM mod trapped or exceeded limits: {0}")]
    Fault(String),
}

/// Inputs passed into a single hook/timer/client invocation.
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
    pub override_str: Option<String>,
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
            override_str: None,
            fuel_used: 0,
        }
    }
}

/// Live instantiated WASM (or future JIT) mod instance.
pub trait ModInstance: Send {
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

/// Pluggable execution runtime seam (`wasmi` interpreter in v1, future JIT backend later).
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
            .table_elements(2048)
            .instances(1)
            .memories(1)
            .tables(2)
            .trap_on_grow_failure(true)
            .build();

        let state = HostCallState {
            mod_id: mod_id.to_string(),
            permissions,
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
            override_str: None,
        };

        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limits);
        // Supply startup fuel so WASM start sections / table initialization cannot loop infinitely.
        let _ = store.set_fuel(250_000);

        let mut linker = <Linker<HostCallState>>::new(&self.engine);
        register_host_natives(&mut linker)
            .map_err(|e| RuntimeError::Instantiate(mod_id.to_string(), e.to_string()))?;

        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| RuntimeError::Instantiate(mod_id.to_string(), e.to_string()))?;

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
        s.override_str = None;
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
            override_str: s.override_str.take(),
            fuel_used,
        }
    }
}

impl ModInstance for WasmiInstance {
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

fn register_host_natives(linker: &mut Linker<HostCallState>) -> Result<(), wasmi::Error> {
    // --- Lifecycle & hook context ---
    linker.func_wrap(
        HOST_MODULE,
        "hook_register",
        |mut caller: Caller<'_, HostCallState>, hook_id: i32, phase: i32, priority: i32| -> i32 {
            let (Some(hook), Some(ph)) = (HookId::from_i32(hook_id), HookPhase::from_i32(phase)) else {
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
                .and_then(|s| {
                    caller
                        .data()
                        .snapshot
                        .clients
                        .iter()
                        .find(|c| c.slot == s)
                        .map(|c| c.name.clone())
                })
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
            let target = if slot < 0 {
                None
            } else {
                Some(match u8::try_from(slot) {
                    Ok(v) => v,
                    Err(_) => return -1,
                })
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
            let target = if slot < 0 {
                None
            } else {
                Some(match u8::try_from(slot) {
                    Ok(v) => v,
                    Err(_) => return -1,
                })
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
            let target = if slot < 0 {
                None
            } else {
                Some(match u8::try_from(slot) {
                    Ok(v) => v,
                    Err(_) => return -1,
                })
            };
            let Some(name) = read_guest_str(&caller, name_ptr, name_len) else {
                return -1;
            };
            let Some(data) = read_guest_bytes(&caller, data_ptr, data_len, MAX_HOST_PAYLOAD_BYTES) else {
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
        |mut caller: Caller<'_, HostCallState>, ptr: i32, len: i32, out_ptr: i32, cap: i32| -> i32 {
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
            if !caller.data().has_perm(Permission::Hud) {
                return -1;
            }
            if !x.is_finite() || !y.is_finite() || !w.is_finite() || !h.is_finite() {
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
