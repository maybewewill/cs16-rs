//! Fuel-metered Python 3 AST evaluator, scoping, closures, classes, and host bridge seam.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cs_mod_api::services::{PluginRuntimeKind, SharedPlatformServices};
use cs_mod_api::{CustomWeaponSpec, HookId, HookPhase, Permissions};
use cs_mod_host::natives::{ActiveEvent, EngineOp, EngineSnapshot, RpcDispatcher, TimerRequest};

use crate::ast::{
    BinOp, BoolOp, CallArg, CmpOp, Comprehension, Expr, ExprKind, FStringPart, Program, SlicePart,
    Stmt, StmtKind, Target, UnaryOp,
};
use crate::bindings;
use crate::error::{CallFrame, PyError};
use crate::stdlib;
use crate::value::{
    Heap, HeapObj, NativeObjKind, ObjId, PyClass, PyFunc, PyInstance, PyKey, PyPluginMeta, PyVal,
};

pub const MAX_CALL_DEPTH: usize = 64;
pub const MAX_STR_LEN: usize = 256 * 1024;
pub const MAX_SEQ_LEN: usize = 65_536;

/// Registered `@mod.hook(...)` callback.
#[derive(Debug, Clone)]
pub struct RegisteredPyHook {
    pub hook: HookId,
    pub phase: HookPhase,
    pub priority: i32,
    pub func: PyVal,
}

/// Registered `@mod.chat_command(...)` handler.
#[derive(Debug, Clone)]
pub struct RegisteredChatCmd {
    pub commands: Vec<String>,
    pub suppress: bool,
    pub func: PyVal,
}

/// Registered `@mod.console_command(...)` handler.
#[derive(Debug, Clone)]
pub struct RegisteredConsoleCmd {
    pub commands: Vec<String>,
    pub func: PyVal,
}

/// Timer registered at import time via `@mod.timer(...)`.
#[derive(Debug, Clone)]
pub struct RegisteredStartupTimer {
    pub delay_ms: u32,
    pub repeat: bool,
    pub cb_id: i32,
    pub arg: i32,
}

/// Transient per-invocation bridge to `ServerModHost` snapshot, services, and transactional ops.
pub struct HostBridge<'a> {
    pub mod_id: String,
    pub snapshot: &'a EngineSnapshot,
    pub event: &'a ActiveEvent,
    pub storage_read: &'a BTreeMap<String, String>,
    pub storage_overlay: BTreeMap<String, Option<String>>,
    pub storage_writes: Vec<(String, Option<String>)>,
    pub permissions: Permissions,
    pub http_hosts: &'a [String],
    pub services: &'a SharedPlatformServices,
    pub rpc_dispatcher: Option<&'a Arc<dyn RpcDispatcher>>,
    pub ops: Vec<EngineOp>,
    pub timers: Vec<TimerRequest>,
    pub cancel_timers: Vec<u32>,
    pub next_local_timer_id: u32,
    pub override_i32: Option<i32>,
    pub override_i32_2: Option<i32>,
    pub override_f32: Option<f32>,
    pub override_str: Option<String>,
    pub rpc_return_str: Option<String>,
    pub logs: Vec<String>,
    pub runtime_kind: PluginRuntimeKind,
}

impl<'a> HostBridge<'a> {
    pub fn has_perm(&self, perm: cs_mod_api::Permission) -> bool {
        self.permissions.contains(perm)
    }

    pub fn storage_get(&self, key: &str) -> Option<String> {
        if let Some(ov) = self.storage_overlay.get(key) {
            return ov.clone();
        }
        self.storage_read.get(key).cloned()
    }

    pub fn storage_set(&mut self, key: String, val: String) {
        self.storage_overlay.insert(key.clone(), Some(val.clone()));
        self.storage_writes.push((key, Some(val)));
    }

    pub fn storage_delete(&mut self, key: String) -> bool {
        let existed = self.storage_get(&key).is_some();
        self.storage_overlay.insert(key.clone(), None);
        self.storage_writes.push((key, None));
        existed
    }

    pub fn storage_snapshot_map(&self) -> BTreeMap<String, String> {
        let mut merged = self.storage_read.clone();
        for (k, v) in &self.storage_overlay {
            match v {
                Some(val) => {
                    merged.insert(k.clone(), val.clone());
                }
                None => {
                    merged.remove(k);
                }
            }
        }
        merged
    }
}

/// Local function call scope frame.
#[derive(Debug, Clone)]
pub struct LocalScope {
    pub env_id: ObjId,
    pub globals_declared: BTreeSet<String>,
    pub nonlocals_declared: BTreeSet<String>,
}

/// Control flow signal returned internally by statement evaluation.
#[derive(Debug, Clone)]
pub enum Flow {
    None,
    Return(PyVal),
    Break,
    Continue,
}

/// Self-contained pure-Rust Python 3 VM for a single plugin instance.
pub struct PyVm {
    pub file_name: String,
    pub mod_id: String,
    pub heap: Heap,
    pub globals: BTreeMap<String, PyVal>,
    pub builtins: BTreeMap<String, PyVal>,
    pub modules: BTreeMap<String, PyVal>,
    pub scopes: Vec<LocalScope>,
    pub frames: Vec<CallFrame>,
    pub fuel_remaining: u64,
    pub fuel_budget: u64,
    pub rng_state: u64,
    pub active_exc: Option<PyError>,
    pub pending_logs: Vec<String>,

    // Plugin registrations collected via decorators or imperative calls
    pub meta: PyPluginMeta,
    pub hooks: Vec<RegisteredPyHook>,
    pub chat_commands: Vec<RegisteredChatCmd>,
    pub console_commands: Vec<RegisteredConsoleCmd>,
    pub startup_timers: Vec<RegisteredStartupTimer>,
    pub timer_callbacks: BTreeMap<i32, PyVal>,
    pub menu_callbacks: BTreeMap<i32, PyVal>,
    pub rpc_exports: BTreeMap<i32, (String, PyVal)>,
    pub event_callbacks: BTreeMap<i32, (String, PyVal)>,
    pub webhook_callbacks: BTreeMap<i32, PyVal>,
    pub next_cb_id: i32,
    pub init_ops: Vec<EngineOp>,
    pub init_custom_weapons: Vec<CustomWeaponSpec>,
}

impl PyVm {
    pub fn new(mod_id: &str, file_name: &str, max_memory_bytes: usize) -> Result<Self, PyError> {
        let mut vm = Self {
            file_name: file_name.to_string(),
            mod_id: mod_id.to_string(),
            heap: Heap::new(max_memory_bytes),
            globals: BTreeMap::new(),
            builtins: BTreeMap::new(),
            modules: BTreeMap::new(),
            scopes: Vec::new(),
            frames: Vec::new(),
            fuel_remaining: 500_000,
            fuel_budget: 500_000,
            rng_state: 0x2545_F491_4F6C_DD1D,
            active_exc: None,
            pending_logs: Vec::new(),
            meta: PyPluginMeta::default(),
            hooks: Vec::new(),
            chat_commands: Vec::new(),
            console_commands: Vec::new(),
            startup_timers: Vec::new(),
            timer_callbacks: BTreeMap::new(),
            menu_callbacks: BTreeMap::new(),
            rpc_exports: BTreeMap::new(),
            event_callbacks: BTreeMap::new(),
            webhook_callbacks: BTreeMap::new(),
            next_cb_id: 1,
            init_ops: Vec::new(),
            init_custom_weapons: Vec::new(),
        };
        stdlib::install_builtins_and_stdlib(&mut vm)?;
        bindings::install_cs16_module(&mut vm)?;
        vm.globals
            .insert("__name__".to_string(), PyVal::Str("__main__".to_string()));
        vm.globals.insert(
            "__file__".to_string(),
            PyVal::Str(file_name.to_string()),
        );
        Ok(vm)
    }

    pub fn alloc(&mut self, obj: HeapObj) -> Result<ObjId, PyError> {
        self.heap
            .alloc(obj)
            .map_err(|limit| PyError::MemoryLimit {
                limit,
                frames: self.frames.clone(),
            })
    }

    pub fn next_callback_id(&mut self) -> i32 {
        let id = self.next_cb_id;
        self.next_cb_id = self.next_cb_id.wrapping_add(1).max(1);
        id
    }

    pub fn reset_fuel(&mut self, budget: u64) {
        self.fuel_budget = budget;
        self.fuel_remaining = budget;
    }

    pub fn fuel_used(&self) -> u64 {
        self.fuel_budget.saturating_sub(self.fuel_remaining)
    }

    #[inline]
    pub fn consume_fuel(&mut self, cost: u64) -> Result<(), PyError> {
        if self.fuel_remaining < cost {
            self.fuel_remaining = 0;
            return Err(PyError::OutOfFuel {
                budget: self.fuel_budget,
                frames: self.frames.clone(),
            });
        }
        self.fuel_remaining -= cost;
        Ok(())
    }

    pub fn runtime_err(&self, kind: &str, msg: impl Into<String>) -> PyError {
        PyError::runtime(kind, msg, self.frames.clone())
    }

    pub fn next_rand_u64(&mut self) -> u64 {
        let mut x = self.rng_state;
        if x == 0 {
            x = 0x9E37_79B9_7F4A_7C15;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Runs between-call GC if heap has grown significantly.
    pub fn maybe_gc(&mut self) {
        if self.heap.live_count() < 1024 {
            return;
        }
        let mut roots = Vec::with_capacity(
            self.globals.len() + self.builtins.len() + self.modules.len() + 64,
        );
        roots.extend(self.globals.values().cloned());
        roots.extend(self.builtins.values().cloned());
        roots.extend(self.modules.values().cloned());
        for h in &self.hooks {
            roots.push(h.func.clone());
        }
        for c in &self.chat_commands {
            roots.push(c.func.clone());
        }
        for c in &self.console_commands {
            roots.push(c.func.clone());
        }
        roots.extend(self.timer_callbacks.values().cloned());
        roots.extend(self.menu_callbacks.values().cloned());
        for (_, f) in self.rpc_exports.values() {
            roots.push(f.clone());
        }
        for (_, f) in self.event_callbacks.values() {
            roots.push(f.clone());
        }
        roots.extend(self.webhook_callbacks.values().cloned());
        self.heap.gc_collect(&roots);
    }

    /// Executes top-level module code.
    pub fn exec_program(
        &mut self,
        program: &Program,
        fuel_budget: u64,
        host: Option<&mut HostBridge<'_>>,
    ) -> Result<(), PyError> {
        self.reset_fuel(fuel_budget);
        self.frames.clear();
        self.scopes.clear();
        self.frames.push(CallFrame {
            file: self.file_name.clone(),
            func_name: "<module>".to_string(),
            line: 1,
        });
        let res = self.exec_block(&program.body, host);
        self.frames.pop();
        match res? {
            Flow::None => Ok(()),
            Flow::Return(_) => Err(self.runtime_err("SyntaxError", "'return' outside function")),
            Flow::Break => Err(self.runtime_err("SyntaxError", "'break' outside loop")),
            Flow::Continue => Err(self.runtime_err("SyntaxError", "'continue' not properly in loop")),
        }
    }

    pub fn exec_block(
        &mut self,
        stmts: &[Stmt],
        mut host: Option<&mut HostBridge<'_>>,
    ) -> Result<Flow, PyError> {
        for stmt in stmts {
            if let Some(frame) = self.frames.last_mut() {
                frame.line = stmt.line;
            }
            self.consume_fuel(1)?;
            let flow = self.exec_stmt(stmt, host.as_deref_mut())?;
            if !matches!(flow, Flow::None) {
                return Ok(flow);
            }
        }
        Ok(Flow::None)
    }

    fn exec_stmt(
        &mut self,
        stmt: &Stmt,
        mut host: Option<&mut HostBridge<'_>>,
    ) -> Result<Flow, PyError> {
        match &stmt.kind {
            StmtKind::Pass => Ok(Flow::None),
            StmtKind::Break => Ok(Flow::Break),
            StmtKind::Continue => Ok(Flow::Continue),
            StmtKind::Return(expr) => {
                let val = match expr {
                    Some(e) => self.eval_expr(e, host)?,
                    None => PyVal::None,
                };
                Ok(Flow::Return(val))
            }
            StmtKind::Expr(e) => {
                let _ = self.eval_expr(e, host)?;
                Ok(Flow::None)
            }
            StmtKind::Global(names) => {
                if let Some(scope) = self.scopes.last_mut() {
                    for n in names {
                        scope.globals_declared.insert(n.clone());
                    }
                }
                Ok(Flow::None)
            }
            StmtKind::Nonlocal(names) => {
                if let Some(scope) = self.scopes.last_mut() {
                    for n in names {
                        scope.nonlocals_declared.insert(n.clone());
                    }
                }
                Ok(Flow::None)
            }
            StmtKind::Import(names) => {
                for (mod_name, alias) in names {
                    let mod_val = self.load_module(mod_name)?;
                    let bind = alias
                        .as_deref()
                        .unwrap_or_else(|| mod_name.split('.').next().unwrap_or(mod_name));
                    self.set_var(bind, mod_val);
                }
                Ok(Flow::None)
            }
            StmtKind::FromImport {
                module,
                names,
                star,
            } => {
                let mod_val = self.load_module(module)?;
                let attrs = match mod_val {
                    PyVal::Handle(id) => match self.heap.get(id) {
                        Some(HeapObj::Native(NativeObjKind::Module { attrs, .. })) => attrs.clone(),
                        _ => {
                            return Err(self.runtime_err(
                                "ImportError",
                                format!("'{module}' is not a module"),
                            ));
                        }
                    },
                    _ => {
                        return Err(self.runtime_err(
                            "ImportError",
                            format!("'{module}' is not a module"),
                        ));
                    }
                };
                if *star {
                    for (k, v) in attrs {
                        if !k.starts_with('_') {
                            self.set_var(&k, v);
                        }
                    }
                } else {
                    for (name, alias) in names {
                        let Some(val) = attrs.get(name).cloned() else {
                            return Err(self.runtime_err(
                                "ImportError",
                                format!("cannot import name '{name}' from '{module}'"),
                            ));
                        };
                        let bind = alias.as_deref().unwrap_or(name.as_str());
                        self.set_var(bind, val);
                    }
                }
                Ok(Flow::None)
            }
            StmtKind::Assign { targets, value } => {
                let val = self.eval_expr(value, host.as_deref_mut())?;
                for t in targets {
                    self.assign_target(t, val.clone(), host.as_deref_mut())?;
                }
                Ok(Flow::None)
            }
            StmtKind::AugAssign { target, op, value } => {
                let rhs = self.eval_expr(value, host.as_deref_mut())?;
                match target {
                    Target::Name(name) => {
                        let lhs = self.get_var(name)?;
                        let res = self.eval_binop(*op, lhs, rhs, host.as_deref_mut())?;
                        self.set_var(name, res);
                    }
                    Target::Attr { value: obj_e, attr } => {
                        let obj = self.eval_expr(obj_e, host.as_deref_mut())?;
                        let lhs = self.get_attr(&obj, attr, host.as_deref_mut())?;
                        let res = self.eval_binop(*op, lhs, rhs, host.as_deref_mut())?;
                        self.set_attr(&obj, attr, res, host.as_deref_mut())?;
                    }
                    Target::Subscript {
                        value: obj_e,
                        index: idx_e,
                    } => {
                        let obj = self.eval_expr(obj_e, host.as_deref_mut())?;
                        let idx = self.eval_expr(idx_e, host.as_deref_mut())?;
                        let lhs = self.get_item(&obj, &idx, host.as_deref_mut())?;
                        let res = self.eval_binop(*op, lhs, rhs, host.as_deref_mut())?;
                        self.set_item(&obj, idx, res, host.as_deref_mut())?;
                    }
                    _ => {
                        return Err(self.runtime_err(
                            "SyntaxError",
                            "illegal target for augmented assignment",
                        ));
                    }
                }
                Ok(Flow::None)
            }
            StmtKind::Delete(targets) => {
                for t in targets {
                    self.delete_target(t, host.as_deref_mut())?;
                }
                Ok(Flow::None)
            }
            StmtKind::If {
                test,
                body,
                elifs,
                orelse,
            } => {
                let cond = self.eval_expr(test, host.as_deref_mut())?;
                if self.is_truthy(&cond) {
                    return self.exec_block(body, host);
                }
                for (et, eb) in elifs {
                    let c = self.eval_expr(et, host.as_deref_mut())?;
                    if self.is_truthy(&c) {
                        return self.exec_block(eb, host);
                    }
                }
                if !orelse.is_empty() {
                    return self.exec_block(orelse, host);
                }
                Ok(Flow::None)
            }
            StmtKind::While { test, body, orelse } => {
                let mut broke = false;
                loop {
                    self.consume_fuel(1)?;
                    let cond = self.eval_expr(test, host.as_deref_mut())?;
                    if !self.is_truthy(&cond) {
                        break;
                    }
                    match self.exec_block(body, host.as_deref_mut())? {
                        Flow::None | Flow::Continue => {}
                        Flow::Break => {
                            broke = true;
                            break;
                        }
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                    }
                }
                if !broke && !orelse.is_empty() {
                    return self.exec_block(orelse, host);
                }
                Ok(Flow::None)
            }
            StmtKind::For {
                target,
                iter,
                body,
                orelse,
            } => {
                let iter_val = self.eval_expr(iter, host.as_deref_mut())?;
                let items = self.to_vec(&iter_val)?;
                let mut broke = false;
                for item in items {
                    self.consume_fuel(1)?;
                    self.assign_target(target, item, host.as_deref_mut())?;
                    match self.exec_block(body, host.as_deref_mut())? {
                        Flow::None | Flow::Continue => {}
                        Flow::Break => {
                            broke = true;
                            break;
                        }
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                    }
                }
                if !broke && !orelse.is_empty() {
                    return self.exec_block(orelse, host);
                }
                Ok(Flow::None)
            }
            StmtKind::FuncDef {
                name,
                params,
                decorators,
                body,
            } => {
                let mut dec_vals = Vec::with_capacity(decorators.len());
                for d in decorators {
                    dec_vals.push(self.eval_expr(d, host.as_deref_mut())?);
                }
                let mut defaults = Vec::with_capacity(params.positional.len());
                for p in &params.positional {
                    match &p.default {
                        Some(de) => defaults.push(Some(self.eval_expr(de, host.as_deref_mut())?)),
                        None => defaults.push(None),
                    }
                }
                let closure = self.scopes.last().map(|s| s.env_id);
                let func_id = self.alloc(HeapObj::Function(PyFunc {
                    name: name.clone(),
                    params: params.clone(),
                    defaults,
                    body: body.clone(),
                    closure,
                    line: stmt.line,
                }))?;
                let mut val = PyVal::Handle(func_id);
                // Apply decorators bottom-to-top
                for dec in dec_vals.into_iter().rev() {
                    val = self.call_value(dec, vec![val], BTreeMap::new(), host.as_deref_mut())?;
                }
                self.set_var(name, val);
                Ok(Flow::None)
            }
            StmtKind::ClassDef {
                name,
                bases,
                decorators,
                body,
            } => {
                let mut dec_vals = Vec::with_capacity(decorators.len());
                for d in decorators {
                    dec_vals.push(self.eval_expr(d, host.as_deref_mut())?);
                }
                let mut base_ids = Vec::new();
                for b in bases {
                    let bv = self.eval_expr(b, host.as_deref_mut())?;
                    if let PyVal::Handle(id) = bv
                        && matches!(self.heap.get(id), Some(HeapObj::Class(_)))
                    {
                        base_ids.push(id);
                    }
                }
                let env_id = self.alloc(HeapObj::EnvScope {
                    vars: BTreeMap::new(),
                    parent: self.scopes.last().map(|s| s.env_id),
                })?;
                self.scopes.push(LocalScope {
                    env_id,
                    globals_declared: BTreeSet::new(),
                    nonlocals_declared: BTreeSet::new(),
                });
                let body_res = self.exec_block(body, host.as_deref_mut());
                self.scopes.pop();
                body_res?;

                let attrs = match self.heap.get(env_id) {
                    Some(HeapObj::EnvScope { vars, .. }) => vars.clone(),
                    _ => BTreeMap::new(),
                };
                let cls_id = self.alloc(HeapObj::Class(PyClass {
                    name: name.clone(),
                    bases: base_ids,
                    attrs,
                }))?;
                let mut val = PyVal::Handle(cls_id);
                for dec in dec_vals.into_iter().rev() {
                    val = self.call_value(dec, vec![val], BTreeMap::new(), host.as_deref_mut())?;
                }
                self.set_var(name, val);
                Ok(Flow::None)
            }
            StmtKind::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                let body_res = self.exec_block(body, host.as_deref_mut());
                let outcome = match body_res {
                    Ok(flow) => {
                        if matches!(flow, Flow::None) && !orelse.is_empty() {
                            self.exec_block(orelse, host.as_deref_mut())
                        } else {
                            Ok(flow)
                        }
                    }
                    Err(err) => {
                        // NEVER allow catching OutOfFuel or MemoryLimit!
                        if matches!(err, PyError::OutOfFuel { .. } | PyError::MemoryLimit { .. }) {
                            return Err(err);
                        }
                        let mut handled: Option<Result<Flow, PyError>> = None;
                        for h in handlers {
                            let matches_handler = match &h.exc_type {
                                None => true,
                                Some(ty_expr) => {
                                    let ty_val = self.eval_expr(ty_expr, host.as_deref_mut())?;
                                    self.exception_matches(&err, &ty_val)
                                }
                            };
                            if matches_handler {
                                let prev_exc = self.active_exc.replace(err.clone());
                                if let Some(ref var_name) = h.name {
                                    let exc_id = self.alloc(HeapObj::Native(
                                        NativeObjKind::ExceptionObj {
                                            kind: err.exception_kind().to_string(),
                                            message: err.message_text(),
                                        },
                                    ))?;
                                    self.set_var(var_name, PyVal::Handle(exc_id));
                                }
                                let h_res = self.exec_block(&h.body, host.as_deref_mut());
                                self.active_exc = prev_exc;
                                handled = Some(h_res);
                                break;
                            }
                        }
                        match handled {
                            Some(res) => res,
                            None => Err(err),
                        }
                    }
                };

                if !finalbody.is_empty() {
                    if let Err(ref e) = outcome
                        && matches!(e, PyError::OutOfFuel { .. } | PyError::MemoryLimit { .. })
                    {
                        return outcome;
                    }
                    let fin_res = self.exec_block(finalbody, host)?;
                    if !matches!(fin_res, Flow::None) {
                        return Ok(fin_res);
                    }
                }
                outcome
            }
            StmtKind::Raise(exc_expr) => match exc_expr {
                None => {
                    if let Some(err) = self.active_exc.clone() {
                        Err(err)
                    } else {
                        Err(self.runtime_err("RuntimeError", "No active exception to reraise"))
                    }
                }
                Some(e) => {
                    let val = self.eval_expr(e, host)?;
                    match val {
                        PyVal::Handle(id) => match self.heap.get(id) {
                            Some(HeapObj::Native(NativeObjKind::ExceptionObj { kind, message })) => {
                                Err(self.runtime_err(kind, message.clone()))
                            }
                            Some(HeapObj::Native(NativeObjKind::ExceptionType(kind))) => {
                                Err(self.runtime_err(kind, ""))
                            }
                            _ => Err(self.runtime_err("RuntimeError", self.val_to_str(&val))),
                        },
                        other => Err(self.runtime_err("RuntimeError", self.val_to_str(&other))),
                    }
                }
            },
            StmtKind::Assert { test, msg } => {
                let cond = self.eval_expr(test, host.as_deref_mut())?;
                if !self.is_truthy(&cond) {
                    let text = match msg {
                        Some(m) => {
                            let mv = self.eval_expr(m, host)?;
                            self.val_to_str(&mv)
                        }
                        None => "assertion failed".to_string(),
                    };
                    return Err(self.runtime_err("AssertionError", text));
                }
                Ok(Flow::None)
            }
        }
    }

    pub fn eval_expr(
        &mut self,
        expr: &Expr,
        mut host: Option<&mut HostBridge<'_>>,
    ) -> Result<PyVal, PyError> {
        self.consume_fuel(1)?;
        match &expr.kind {
            ExprKind::None => Ok(PyVal::None),
            ExprKind::Bool(b) => Ok(PyVal::Bool(*b)),
            ExprKind::Int(n) => Ok(PyVal::Int(*n)),
            ExprKind::Float(f) => Ok(PyVal::Float(*f)),
            ExprKind::Str(s) => Ok(PyVal::Str(s.clone())),
            ExprKind::Name(name) => self.get_var(name),
            ExprKind::FString(parts) => {
                let mut out = String::new();
                for part in parts {
                    match part {
                        FStringPart::Literal(s) => out.push_str(s),
                        FStringPart::Formatted {
                            expr: sub,
                            format_spec,
                            conversion,
                        } => {
                            let val = self.eval_expr(sub, host.as_deref_mut())?;
                            let piece = match conversion {
                                Some('r') => {
                                    let r = self.val_to_repr(&val);
                                    match format_spec {
                                        Some(spec) => stdlib::format_py_val(self, &PyVal::Str(r), spec)?,
                                        None => r,
                                    }
                                }
                                Some('s') => {
                                    let s = self.val_to_str(&val);
                                    match format_spec {
                                        Some(spec) => stdlib::format_py_val(self, &PyVal::Str(s), spec)?,
                                        None => s,
                                    }
                                }
                                _ => match format_spec {
                                    Some(spec) => stdlib::format_py_val(self, &val, spec)?,
                                    None => self.val_to_str(&val),
                                },
                            };
                            if out.len() + piece.len() > MAX_STR_LEN {
                                return Err(self.runtime_err("MemoryError", "string length limit exceeded"));
                            }
                            out.push_str(&piece);
                        }
                    }
                }
                Ok(PyVal::Str(out))
            }
            ExprKind::List(elts) => {
                let mut items = Vec::with_capacity(elts.len());
                for e in elts {
                    if let ExprKind::Starred(inner) = &e.kind {
                        let v = self.eval_expr(inner, host.as_deref_mut())?;
                        items.extend(self.to_vec(&v)?);
                    } else {
                        items.push(self.eval_expr(e, host.as_deref_mut())?);
                    }
                    if items.len() > MAX_SEQ_LEN {
                        return Err(self.runtime_err("MemoryError", "list length limit exceeded"));
                    }
                }
                let id = self.alloc(HeapObj::List(items))?;
                Ok(PyVal::Handle(id))
            }
            ExprKind::Tuple(elts) => {
                let mut items = Vec::with_capacity(elts.len());
                for e in elts {
                    if let ExprKind::Starred(inner) = &e.kind {
                        let v = self.eval_expr(inner, host.as_deref_mut())?;
                        items.extend(self.to_vec(&v)?);
                    } else {
                        items.push(self.eval_expr(e, host.as_deref_mut())?);
                    }
                }
                Ok(PyVal::Tuple(items))
            }
            ExprKind::Set(elts) => {
                let mut keys = Vec::new();
                for e in elts {
                    let v = self.eval_expr(e, host.as_deref_mut())?;
                    let k = PyKey::from_val(&v)
                        .ok_or_else(|| self.runtime_err("TypeError", "unhashable type in set"))?;
                    if !keys.contains(&k) {
                        keys.push(k);
                    }
                }
                let id = self.alloc(HeapObj::Set(keys))?;
                Ok(PyVal::Handle(id))
            }
            ExprKind::Dict(pairs) => {
                let mut entries: Vec<(PyKey, PyVal)> = Vec::with_capacity(pairs.len());
                for (ke, ve) in pairs {
                    let kv = self.eval_expr(ke, host.as_deref_mut())?;
                    let vv = self.eval_expr(ve, host.as_deref_mut())?;
                    let k = PyKey::from_val(&kv)
                        .ok_or_else(|| self.runtime_err("TypeError", "unhashable dict key"))?;
                    if let Some(pos) = entries.iter().position(|(ek, _)| ek == &k) {
                        entries[pos].1 = vv;
                    } else {
                        entries.push((k, vv));
                    }
                }
                let id = self.alloc(HeapObj::Dict(entries))?;
                Ok(PyVal::Handle(id))
            }
            ExprKind::ListComp { elt, generators } => {
                let mut out = Vec::new();
                let env_id = self.alloc(HeapObj::EnvScope {
                    vars: BTreeMap::new(),
                    parent: self.scopes.last().map(|s| s.env_id),
                })?;
                self.scopes.push(LocalScope {
                    env_id,
                    globals_declared: BTreeSet::new(),
                    nonlocals_declared: BTreeSet::new(),
                });
                let res = self.eval_comp_generators(
                    generators,
                    0,
                    host.as_deref_mut(),
                    &mut |vm, h| {
                        let v = vm.eval_expr(elt, h)?;
                        if out.len() >= MAX_SEQ_LEN {
                            return Err(vm.runtime_err("MemoryError", "list comprehension too large"));
                        }
                        out.push(v);
                        Ok(())
                    },
                );
                self.scopes.pop();
                res?;
                let id = self.alloc(HeapObj::List(out))?;
                Ok(PyVal::Handle(id))
            }
            ExprKind::SetComp { elt, generators } => {
                let mut out = Vec::new();
                let env_id = self.alloc(HeapObj::EnvScope {
                    vars: BTreeMap::new(),
                    parent: self.scopes.last().map(|s| s.env_id),
                })?;
                self.scopes.push(LocalScope {
                    env_id,
                    globals_declared: BTreeSet::new(),
                    nonlocals_declared: BTreeSet::new(),
                });
                let res = self.eval_comp_generators(
                    generators,
                    0,
                    host.as_deref_mut(),
                    &mut |vm, h| {
                        let v = vm.eval_expr(elt, h)?;
                        let k = PyKey::from_val(&v)
                            .ok_or_else(|| vm.runtime_err("TypeError", "unhashable type in set"))?;
                        if !out.contains(&k) {
                            out.push(k);
                        }
                        Ok(())
                    },
                );
                self.scopes.pop();
                res?;
                let id = self.alloc(HeapObj::Set(out))?;
                Ok(PyVal::Handle(id))
            }
            ExprKind::DictComp {
                key,
                value,
                generators,
            } => {
                let mut out: Vec<(PyKey, PyVal)> = Vec::new();
                let env_id = self.alloc(HeapObj::EnvScope {
                    vars: BTreeMap::new(),
                    parent: self.scopes.last().map(|s| s.env_id),
                })?;
                self.scopes.push(LocalScope {
                    env_id,
                    globals_declared: BTreeSet::new(),
                    nonlocals_declared: BTreeSet::new(),
                });
                let res = self.eval_comp_generators(
                    generators,
                    0,
                    host.as_deref_mut(),
                    &mut |vm, mut h| {
                        let kv = vm.eval_expr(key, h.as_deref_mut())?;
                        let vv = vm.eval_expr(value, h)?;
                        let k = PyKey::from_val(&kv)
                            .ok_or_else(|| vm.runtime_err("TypeError", "unhashable dict key"))?;
                        if let Some(pos) = out.iter().position(|(ek, _)| ek == &k) {
                            out[pos].1 = vv;
                        } else {
                            out.push((k, vv));
                        }
                        Ok(())
                    },
                );
                self.scopes.pop();
                res?;
                let id = self.alloc(HeapObj::Dict(out))?;
                Ok(PyVal::Handle(id))
            }
            ExprKind::Unary { op, operand } => {
                let v = self.eval_expr(operand, host)?;
                match op {
                    UnaryOp::Not => Ok(PyVal::Bool(!self.is_truthy(&v))),
                    UnaryOp::Pos => match v {
                        PyVal::Int(n) => Ok(PyVal::Int(n)),
                        PyVal::Bool(b) => Ok(PyVal::Int(i64::from(b))),
                        PyVal::Float(f) => Ok(PyVal::Float(f)),
                        _ => Err(self.runtime_err("TypeError", "bad operand type for unary +")),
                    },
                    UnaryOp::Neg => match v {
                        PyVal::Int(n) => Ok(PyVal::Int(n.wrapping_neg())),
                        PyVal::Bool(b) => Ok(PyVal::Int(-i64::from(b))),
                        PyVal::Float(f) => Ok(PyVal::Float(-f)),
                        _ => Err(self.runtime_err("TypeError", "bad operand type for unary -")),
                    },
                    UnaryOp::BitNot => match v.as_int() {
                        Some(n) => Ok(PyVal::Int(!n)),
                        None => Err(self.runtime_err("TypeError", "bad operand type for unary ~")),
                    },
                }
            }
            ExprKind::Binary { left, op, right } => {
                let lv = self.eval_expr(left, host.as_deref_mut())?;
                let rv = self.eval_expr(right, host.as_deref_mut())?;
                self.eval_binop(*op, lv, rv, host)
            }
            ExprKind::BoolOp { op, values } => {
                let mut last = PyVal::None;
                for v_expr in values {
                    let v = self.eval_expr(v_expr, host.as_deref_mut())?;
                    let truth = self.is_truthy(&v);
                    match op {
                        BoolOp::And if !truth => return Ok(v),
                        BoolOp::Or if truth => return Ok(v),
                        _ => last = v,
                    }
                }
                Ok(last)
            }
            ExprKind::Compare {
                left,
                ops,
                comparators,
            } => {
                let mut cur = self.eval_expr(left, host.as_deref_mut())?;
                for (op, rhs_expr) in ops.iter().zip(comparators.iter()) {
                    let rhs = self.eval_expr(rhs_expr, host.as_deref_mut())?;
                    if !self.eval_cmp(*op, &cur, &rhs, host.as_deref_mut())? {
                        return Ok(PyVal::Bool(false));
                    }
                    cur = rhs;
                }
                Ok(PyVal::Bool(true))
            }
            ExprKind::IfExp { test, body, orelse } => {
                let c = self.eval_expr(test, host.as_deref_mut())?;
                if self.is_truthy(&c) {
                    self.eval_expr(body, host)
                } else {
                    self.eval_expr(orelse, host)
                }
            }
            ExprKind::Lambda { params, body } => {
                let mut defaults = Vec::with_capacity(params.positional.len());
                for p in &params.positional {
                    match &p.default {
                        Some(de) => defaults.push(Some(self.eval_expr(de, host.as_deref_mut())?)),
                        None => defaults.push(None),
                    }
                }
                let closure = self.scopes.last().map(|s| s.env_id);
                let ret_stmt = Stmt {
                    line: expr.line,
                    kind: StmtKind::Return(Some((**body).clone())),
                };
                let id = self.alloc(HeapObj::Function(PyFunc {
                    name: "<lambda>".to_string(),
                    params: params.clone(),
                    defaults,
                    body: vec![ret_stmt],
                    closure,
                    line: expr.line,
                }))?;
                Ok(PyVal::Handle(id))
            }
            ExprKind::Call { func, args } => {
                let callee = self.eval_expr(func, host.as_deref_mut())?;
                let mut pos_args = Vec::new();
                let mut kw_args = BTreeMap::new();
                for arg in args {
                    match arg {
                        CallArg::Positional(e) => {
                            pos_args.push(self.eval_expr(e, host.as_deref_mut())?);
                        }
                        CallArg::Starred(e) => {
                            let v = self.eval_expr(e, host.as_deref_mut())?;
                            pos_args.extend(self.to_vec(&v)?);
                        }
                        CallArg::Keyword(k, e) => {
                            let v = self.eval_expr(e, host.as_deref_mut())?;
                            kw_args.insert(k.clone(), v);
                        }
                        CallArg::KwStarred(e) => {
                            let v = self.eval_expr(e, host.as_deref_mut())?;
                            match v {
                                PyVal::Handle(id) => {
                                    let entries = match self.heap.get(id) {
                                        Some(HeapObj::Dict(d)) => d.clone(),
                                        _ => {
                                            return Err(self.runtime_err(
                                                "TypeError",
                                                "** argument must be a mapping",
                                            ));
                                        }
                                    };
                                    for (k, val) in entries {
                                        if let PyKey::Str(ks) = k {
                                            kw_args.insert(ks, val);
                                        } else {
                                            return Err(self.runtime_err(
                                                "TypeError",
                                                "keywords must be strings",
                                            ));
                                        }
                                    }
                                }
                                _ => {
                                    return Err(self.runtime_err(
                                        "TypeError",
                                        "** argument must be a mapping",
                                    ));
                                }
                            }
                        }
                    }
                }
                self.call_value(callee, pos_args, kw_args, host)
            }
            ExprKind::Attribute { value, attr } => {
                let obj = self.eval_expr(value, host.as_deref_mut())?;
                self.get_attr(&obj, attr, host)
            }
            ExprKind::Subscript { value, slice } => {
                let obj = self.eval_expr(value, host.as_deref_mut())?;
                match slice.as_ref() {
                    SlicePart::Index(idx_expr) => {
                        let idx = self.eval_expr(idx_expr, host.as_deref_mut())?;
                        self.get_item(&obj, &idx, host)
                    }
                    SlicePart::Slice { lower, upper, step } => {
                        let lo = match lower {
                            Some(e) => Some(self.eval_expr(e, host.as_deref_mut())?),
                            None => None,
                        };
                        let hi = match upper {
                            Some(e) => Some(self.eval_expr(e, host.as_deref_mut())?),
                            None => None,
                        };
                        let st = match step {
                            Some(e) => Some(self.eval_expr(e, host.as_deref_mut())?),
                            None => None,
                        };
                        self.eval_slice(&obj, lo, hi, st)
                    }
                }
            }
            ExprKind::Named { target, value } => {
                let v = self.eval_expr(value, host)?;
                self.set_var(target, v.clone());
                Ok(v)
            }
            ExprKind::Starred(inner) => self.eval_expr(inner, host),
        }
    }

    fn eval_comp_generators<F>(
        &mut self,
        gens: &[Comprehension],
        idx: usize,
        mut host: Option<&mut HostBridge<'_>>,
        emit: &mut F,
    ) -> Result<(), PyError>
    where
        F: FnMut(&mut PyVm, Option<&mut HostBridge<'_>>) -> Result<(), PyError>,
    {
        let Some(comp) = gens.get(idx) else {
            return emit(self, host);
        };
        let iter_val = self.eval_expr(&comp.iter, host.as_deref_mut())?;
        let items = self.to_vec(&iter_val)?;
        for item in items {
            self.consume_fuel(1)?;
            self.assign_target(&comp.target, item, host.as_deref_mut())?;
            let mut keep = true;
            for cond_expr in &comp.ifs {
                let cv = self.eval_expr(cond_expr, host.as_deref_mut())?;
                if !self.is_truthy(&cv) {
                    keep = false;
                    break;
                }
            }
            if keep {
                self.eval_comp_generators(gens, idx + 1, host.as_deref_mut(), emit)?;
            }
        }
        Ok(())
    }

    pub fn call_value(
        &mut self,
        callee: PyVal,
        args: Vec<PyVal>,
        kwargs: BTreeMap<String, PyVal>,
        host: Option<&mut HostBridge<'_>>,
    ) -> Result<PyVal, PyError> {
        self.consume_fuel(2)?;
        if self.frames.len() >= MAX_CALL_DEPTH {
            return Err(self.runtime_err("RecursionError", "maximum recursion depth exceeded"));
        }
        let PyVal::Handle(id) = callee else {
            return Err(self.runtime_err(
                "TypeError",
                format!("'{}' object is not callable", self.type_name(&callee)),
            ));
        };
        let obj = self
            .heap
            .get(id)
            .cloned()
            .ok_or_else(|| self.runtime_err("RuntimeError", "dangling object handle"))?;

        match obj {
            HeapObj::Function(func) => self.call_py_func(func, args, kwargs, host),
            HeapObj::BoundMethod { receiver, func } => {
                let mut full_args = Vec::with_capacity(args.len() + 1);
                full_args.push(receiver);
                full_args.extend(args);
                self.call_value(func, full_args, kwargs, host)
            }
            HeapObj::NativeFn(name) => stdlib::call_native_fn(self, &name, args, kwargs, host),
            HeapObj::BoundNativeMethod { receiver, method } => {
                stdlib::call_bound_native_method(self, receiver, &method, args, kwargs, host)
            }
            HeapObj::Class(cls) => {
                let inst_id = self.alloc(HeapObj::Instance(PyInstance {
                    class_id: id,
                    attrs: BTreeMap::new(),
                }))?;
                let inst_val = PyVal::Handle(inst_id);
                if let Some(init_fn) = self.lookup_class_attr(id, "__init__") {
                    let mut init_args = Vec::with_capacity(args.len() + 1);
                    init_args.push(inst_val.clone());
                    init_args.extend(args);
                    let _ = self.call_value(init_fn, init_args, kwargs, host)?;
                } else if !args.is_empty() || !kwargs.is_empty() {
                    return Err(self.runtime_err(
                        "TypeError",
                        format!("{}() takes no arguments", cls.name),
                    ));
                }
                Ok(inst_val)
            }
            HeapObj::Instance(inst) => {
                if let Some(call_fn) = self.lookup_class_attr(inst.class_id, "__call__") {
                    let mut full_args = Vec::with_capacity(args.len() + 1);
                    full_args.push(PyVal::Handle(id));
                    full_args.extend(args);
                    self.call_value(call_fn, full_args, kwargs, host)
                } else {
                    Err(self.runtime_err("TypeError", "instance object is not callable"))
                }
            }
            HeapObj::Native(NativeObjKind::Decorator(dec)) => {
                bindings::apply_decorator(self, dec, args, host)
            }
            HeapObj::Native(NativeObjKind::ExceptionType(kind)) => {
                let msg = args
                    .first()
                    .map(|v| self.val_to_str(v))
                    .unwrap_or_default();
                let eid = self.alloc(HeapObj::Native(NativeObjKind::ExceptionObj {
                    kind,
                    message: msg,
                }))?;
                Ok(PyVal::Handle(eid))
            }
            _ => Err(self.runtime_err(
                "TypeError",
                format!("'{}' object is not callable", self.type_name(&PyVal::Handle(id))),
            )),
        }
    }

    pub fn call_py_func(
        &mut self,
        func: PyFunc,
        args: Vec<PyVal>,
        mut kwargs: BTreeMap<String, PyVal>,
        host: Option<&mut HostBridge<'_>>,
    ) -> Result<PyVal, PyError> {
        let mut local_vars = BTreeMap::new();
        let pos_count = func.params.positional.len();
        let mut extra_pos = Vec::new();

        for (i, val) in args.into_iter().enumerate() {
            if i < pos_count {
                local_vars.insert(func.params.positional[i].name.clone(), val);
            } else if func.params.vararg.is_some() {
                extra_pos.push(val);
            } else {
                return Err(self.runtime_err(
                    "TypeError",
                    format!(
                        "{}() takes at most {} positional arguments",
                        func.name, pos_count
                    ),
                ));
            }
        }

        for (i, param) in func.params.positional.iter().enumerate() {
            if let Some(kw_val) = kwargs.remove(&param.name) {
                if local_vars.contains_key(&param.name) {
                    return Err(self.runtime_err(
                        "TypeError",
                        format!(
                            "{}() got multiple values for argument '{}'",
                            func.name, param.name
                        ),
                    ));
                }
                local_vars.insert(param.name.clone(), kw_val);
            } else if !local_vars.contains_key(&param.name) {
                if let Some(ref def_val) = func.defaults[i] {
                    local_vars.insert(param.name.clone(), def_val.clone());
                } else {
                    return Err(self.runtime_err(
                        "TypeError",
                        format!(
                            "{}() missing required positional argument: '{}'",
                            func.name, param.name
                        ),
                    ));
                }
            }
        }

        if let Some(ref vararg_name) = func.params.vararg {
            local_vars.insert(vararg_name.clone(), PyVal::Tuple(extra_pos));
        }

        if let Some(ref kwarg_name) = func.params.kwarg {
            let entries: Vec<(PyKey, PyVal)> = kwargs
                .into_iter()
                .map(|(k, v)| (PyKey::Str(k), v))
                .collect();
            let d_id = self.alloc(HeapObj::Dict(entries))?;
            local_vars.insert(kwarg_name.clone(), PyVal::Handle(d_id));
        } else if let Some((unexpected, _)) = kwargs.into_iter().next() {
            return Err(self.runtime_err(
                "TypeError",
                format!(
                    "{}() got an unexpected keyword argument '{}'",
                    func.name, unexpected
                ),
            ));
        }

        let env_id = self.alloc(HeapObj::EnvScope {
            vars: local_vars,
            parent: func.closure,
        })?;
        self.scopes.push(LocalScope {
            env_id,
            globals_declared: BTreeSet::new(),
            nonlocals_declared: BTreeSet::new(),
        });
        self.frames.push(CallFrame {
            file: self.file_name.clone(),
            func_name: func.name.clone(),
            line: func.line,
        });

        let res = self.exec_block(&func.body, host);

        self.frames.pop();
        self.scopes.pop();

        match res? {
            Flow::Return(v) => Ok(v),
            Flow::None => Ok(PyVal::None),
            Flow::Break => Err(self.runtime_err("SyntaxError", "'break' outside loop")),
            Flow::Continue => Err(self.runtime_err("SyntaxError", "'continue' not properly in loop")),
        }
    }

    /// Calls a Python callback passing at most as many positional args as it accepts (when it
    /// doesn't take `*args`), so hooks/commands/timers with fewer params work ergonomically.
    pub fn call_callback_flexible(
        &mut self,
        cb: PyVal,
        args: Vec<PyVal>,
        host: Option<&mut HostBridge<'_>>,
    ) -> Result<PyVal, PyError> {
        let trimmed = if let PyVal::Handle(id) = &cb
            && let Some(HeapObj::Function(f)) = self.heap.get(*id)
            && f.params.vararg.is_none()
            && args.len() > f.params.positional.len()
        {
            args.into_iter().take(f.params.positional.len()).collect()
        } else {
            args
        };
        self.call_value(cb, trimmed, BTreeMap::new(), host)
    }

    pub fn load_module(&self, name: &str) -> Result<PyVal, PyError> {
        self.modules
            .get(name)
            .cloned()
            .ok_or_else(|| self.runtime_err("ModuleNotFoundError", format!("No module named '{name}'")))
    }

    pub fn get_var(&self, name: &str) -> Result<PyVal, PyError> {
        if let Some(scope) = self.scopes.last()
            && !scope.globals_declared.contains(name)
        {
            let mut cur = Some(scope.env_id);
            while let Some(eid) = cur {
                if let Some(HeapObj::EnvScope { vars, parent }) = self.heap.get(eid) {
                    if let Some(v) = vars.get(name) {
                        return Ok(v.clone());
                    }
                    cur = *parent;
                } else {
                    break;
                }
            }
        }
        if let Some(v) = self.globals.get(name) {
            return Ok(v.clone());
        }
        if let Some(v) = self.builtins.get(name) {
            return Ok(v.clone());
        }
        Err(self.runtime_err("NameError", format!("name '{name}' is not defined")))
    }

    pub fn set_var(&mut self, name: &str, val: PyVal) {
        if let Some(scope) = self.scopes.last().cloned() {
            if scope.globals_declared.contains(name) {
                self.globals.insert(name.to_string(), val);
                return;
            }
            if scope.nonlocals_declared.contains(name) {
                // Walk parent closure scopes to update existing binding
                let mut cur = match self.heap.get(scope.env_id) {
                    Some(HeapObj::EnvScope { parent, .. }) => *parent,
                    _ => None,
                };
                while let Some(eid) = cur {
                    let next_parent = match self.heap.get_mut(eid) {
                        Some(HeapObj::EnvScope { vars, parent }) => {
                            if vars.contains_key(name) {
                                vars.insert(name.to_string(), val);
                                return;
                            }
                            *parent
                        }
                        _ => None,
                    };
                    cur = next_parent;
                }
            }
            if let Some(HeapObj::EnvScope { vars, .. }) = self.heap.get_mut(scope.env_id) {
                vars.insert(name.to_string(), val);
                return;
            }
        }
        self.globals.insert(name.to_string(), val);
    }

    fn assign_target(
        &mut self,
        target: &Target,
        val: PyVal,
        mut host: Option<&mut HostBridge<'_>>,
    ) -> Result<(), PyError> {
        match target {
            Target::Name(name) => {
                self.set_var(name, val);
                Ok(())
            }
            Target::Attr { value, attr } => {
                let obj = self.eval_expr(value, host.as_deref_mut())?;
                self.set_attr(&obj, attr, val, host)
            }
            Target::Subscript { value, index } => {
                let obj = self.eval_expr(value, host.as_deref_mut())?;
                let idx = self.eval_expr(index, host.as_deref_mut())?;
                self.set_item(&obj, idx, val, host)
            }
            Target::Tuple(targets) => {
                let items = self.to_vec(&val)?;
                if let Some(star_pos) = targets.iter().position(|t| matches!(t, Target::Starred(_)))
                {
                    let min_len = targets.len() - 1;
                    if items.len() < min_len {
                        return Err(self.runtime_err(
                            "ValueError",
                            format!("not enough values to unpack (expected at least {min_len})"),
                        ));
                    }
                    let tail_count = targets.len() - 1 - star_pos;
                    let star_end = items.len() - tail_count;
                    for (t, v) in targets[..star_pos].iter().zip(items[..star_pos].iter()) {
                        self.assign_target(t, v.clone(), host.as_deref_mut())?;
                    }
                    if let Target::Starred(inner) = &targets[star_pos] {
                        let list_id = self.alloc(HeapObj::List(items[star_pos..star_end].to_vec()))?;
                        self.assign_target(inner, PyVal::Handle(list_id), host.as_deref_mut())?;
                    }
                    for (t, v) in targets[star_pos + 1..].iter().zip(items[star_end..].iter()) {
                        self.assign_target(t, v.clone(), host.as_deref_mut())?;
                    }
                } else {
                    if items.len() != targets.len() {
                        return Err(self.runtime_err(
                            "ValueError",
                            format!(
                                "cannot unpack {} values into {} targets",
                                items.len(),
                                targets.len()
                            ),
                        ));
                    }
                    for (t, v) in targets.iter().zip(items) {
                        self.assign_target(t, v, host.as_deref_mut())?;
                    }
                }
                Ok(())
            }
            Target::Starred(inner) => self.assign_target(inner, val, host),
        }
    }

    fn delete_target(
        &mut self,
        target: &Target,
        mut host: Option<&mut HostBridge<'_>>,
    ) -> Result<(), PyError> {
        match target {
            Target::Name(name) => {
                if let Some(scope) = self.scopes.last().cloned()
                    && !scope.globals_declared.contains(name)
                    && let Some(HeapObj::EnvScope { vars, .. }) = self.heap.get_mut(scope.env_id)
                    && vars.remove(name).is_some()
                {
                    return Ok(());
                }
                if self.globals.remove(name).is_none() {
                    return Err(self.runtime_err("NameError", format!("name '{name}' is not defined")));
                }
                Ok(())
            }
            Target::Subscript { value, index } => {
                let obj = self.eval_expr(value, host.as_deref_mut())?;
                let idx = self.eval_expr(index, host.as_deref_mut())?;
                self.del_item(&obj, &idx, host)
            }
            Target::Attr { value, attr } => {
                let obj = self.eval_expr(value, host)?;
                if let PyVal::Handle(id) = obj
                    && let Some(HeapObj::Instance(inst)) = self.heap.get_mut(id)
                {
                    inst.attrs.remove(attr);
                    return Ok(());
                }
                Err(self.runtime_err("AttributeError", format!("cannot delete attribute '{attr}'")))
            }
            _ => Err(self.runtime_err("SyntaxError", "cannot delete expression")),
        }
    }

    pub fn lookup_class_attr(&self, class_id: ObjId, attr: &str) -> Option<PyVal> {
        let Some(HeapObj::Class(cls)) = self.heap.get(class_id) else {
            return None;
        };
        if let Some(v) = cls.attrs.get(attr) {
            return Some(v.clone());
        }
        let bases = cls.bases.clone();
        for b in bases {
            if let Some(v) = self.lookup_class_attr(b, attr) {
                return Some(v);
            }
        }
        None
    }

    pub fn get_attr(
        &mut self,
        obj: &PyVal,
        attr: &str,
        host: Option<&mut HostBridge<'_>>,
    ) -> Result<PyVal, PyError> {
        match obj {
            PyVal::Str(_) => {
                let id = self.alloc(HeapObj::BoundNativeMethod {
                    receiver: obj.clone(),
                    method: format!("str.{attr}"),
                })?;
                Ok(PyVal::Handle(id))
            }
            PyVal::Tuple(_) => {
                if matches!(attr, "count" | "index") {
                    let id = self.alloc(HeapObj::BoundNativeMethod {
                        receiver: obj.clone(),
                        method: format!("tuple.{attr}"),
                    })?;
                    Ok(PyVal::Handle(id))
                } else {
                    Err(self.runtime_err(
                        "AttributeError",
                        format!("'tuple' object has no attribute '{attr}'"),
                    ))
                }
            }
            PyVal::Handle(id) => {
                let kind_tag = self.heap.get(*id).cloned();
                match kind_tag {
                    Some(HeapObj::List(_)) => {
                        if matches!(
                            attr,
                            "append"
                                | "extend"
                                | "insert"
                                | "pop"
                                | "remove"
                                | "clear"
                                | "index"
                                | "count"
                                | "sort"
                                | "reverse"
                                | "copy"
                        ) {
                            let mid = self.alloc(HeapObj::BoundNativeMethod {
                                receiver: obj.clone(),
                                method: format!("list.{attr}"),
                            })?;
                            Ok(PyVal::Handle(mid))
                        } else {
                            Err(self.runtime_err(
                                "AttributeError",
                                format!("'list' object has no attribute '{attr}'"),
                            ))
                        }
                    }
                    Some(HeapObj::Dict(_)) => {
                        if matches!(
                            attr,
                            "get"
                                | "keys"
                                | "values"
                                | "items"
                                | "pop"
                                | "setdefault"
                                | "update"
                                | "clear"
                                | "copy"
                        ) {
                            let mid = self.alloc(HeapObj::BoundNativeMethod {
                                receiver: obj.clone(),
                                method: format!("dict.{attr}"),
                            })?;
                            Ok(PyVal::Handle(mid))
                        } else {
                            Err(self.runtime_err(
                                "AttributeError",
                                format!("'dict' object has no attribute '{attr}'"),
                            ))
                        }
                    }
                    Some(HeapObj::Set(_)) => {
                        if matches!(
                            attr,
                            "add"
                                | "remove"
                                | "discard"
                                | "pop"
                                | "clear"
                                | "copy"
                                | "union"
                                | "intersection"
                                | "difference"
                                | "issubset"
                                | "issuperset"
                                | "update"
                        ) {
                            let mid = self.alloc(HeapObj::BoundNativeMethod {
                                receiver: obj.clone(),
                                method: format!("set.{attr}"),
                            })?;
                            Ok(PyVal::Handle(mid))
                        } else {
                            Err(self.runtime_err(
                                "AttributeError",
                                format!("'set' object has no attribute '{attr}'"),
                            ))
                        }
                    }
                    Some(HeapObj::Class(_)) => self.lookup_class_attr(*id, attr).ok_or_else(|| {
                        self.runtime_err("AttributeError", format!("class has no attribute '{attr}'"))
                    }),
                    Some(HeapObj::Instance(inst)) => {
                        if let Some(v) = inst.attrs.get(attr) {
                            return Ok(v.clone());
                        }
                        if let Some(cls_attr) = self.lookup_class_attr(inst.class_id, attr) {
                            if let PyVal::Handle(fid) = &cls_attr
                                && matches!(self.heap.get(*fid), Some(HeapObj::Function(_)))
                            {
                                let bm = self.alloc(HeapObj::BoundMethod {
                                    receiver: obj.clone(),
                                    func: cls_attr,
                                })?;
                                return Ok(PyVal::Handle(bm));
                            }
                            return Ok(cls_attr);
                        }
                        Err(self.runtime_err(
                            "AttributeError",
                            format!("object has no attribute '{attr}'"),
                        ))
                    }
                    Some(HeapObj::Native(native)) => {
                        bindings::get_native_attr(self, *id, native, attr, host)
                    }
                    _ => Err(self.runtime_err(
                        "AttributeError",
                        format!("'{}' object has no attribute '{attr}'", self.type_name(obj)),
                    )),
                }
            }
            _ => Err(self.runtime_err(
                "AttributeError",
                format!("'{}' object has no attribute '{attr}'", self.type_name(obj)),
            )),
        }
    }

    pub fn set_attr(
        &mut self,
        obj: &PyVal,
        attr: &str,
        val: PyVal,
        host: Option<&mut HostBridge<'_>>,
    ) -> Result<(), PyError> {
        let PyVal::Handle(id) = obj else {
            return Err(self.runtime_err(
                "AttributeError",
                format!("'{}' object has no attribute '{attr}'", self.type_name(obj)),
            ));
        };
        let is_native = matches!(self.heap.get(*id), Some(HeapObj::Native(_)));
        if is_native {
            return bindings::set_native_attr(self, *id, attr, val, host);
        }
        match self.heap.get_mut(*id) {
            Some(HeapObj::Instance(inst)) => {
                inst.attrs.insert(attr.to_string(), val);
                Ok(())
            }
            Some(HeapObj::Class(cls)) => {
                cls.attrs.insert(attr.to_string(), val);
                Ok(())
            }
            _ => Err(self.runtime_err(
                "AttributeError",
                format!("cannot set attribute '{attr}'"),
            )),
        }
    }

    pub fn get_item(
        &mut self,
        obj: &PyVal,
        idx: &PyVal,
        host: Option<&mut HostBridge<'_>>,
    ) -> Result<PyVal, PyError> {
        match obj {
            PyVal::Str(s) => {
                let i = idx
                    .as_int()
                    .ok_or_else(|| self.runtime_err("TypeError", "string indices must be integers"))?;
                let chars: Vec<char> = s.chars().collect();
                let pos = normalize_index(i, chars.len())
                    .ok_or_else(|| self.runtime_err("IndexError", "string index out of range"))?;
                Ok(PyVal::Str(chars[pos].to_string()))
            }
            PyVal::Tuple(items) => {
                let i = idx
                    .as_int()
                    .ok_or_else(|| self.runtime_err("TypeError", "tuple indices must be integers"))?;
                let pos = normalize_index(i, items.len())
                    .ok_or_else(|| self.runtime_err("IndexError", "tuple index out of range"))?;
                Ok(items[pos].clone())
            }
            PyVal::Handle(id) => match self.heap.get(*id).cloned() {
                Some(HeapObj::List(items)) => {
                    let i = idx.as_int().ok_or_else(|| {
                        self.runtime_err("TypeError", "list indices must be integers")
                    })?;
                    let pos = normalize_index(i, items.len())
                        .ok_or_else(|| self.runtime_err("IndexError", "list index out of range"))?;
                    Ok(items[pos].clone())
                }
                Some(HeapObj::Dict(entries)) => {
                    let k = PyKey::from_val(idx)
                        .ok_or_else(|| self.runtime_err("TypeError", "unhashable dict key"))?;
                    entries
                        .iter()
                        .find(|(ek, _)| ek == &k)
                        .map(|(_, v)| v.clone())
                        .ok_or_else(|| {
                            self.runtime_err("KeyError", format!("key not found: {}", self.val_to_repr(idx)))
                        })
                }
                Some(HeapObj::Native(NativeObjKind::Storage)) => {
                    let key = idx
                        .as_str()
                        .ok_or_else(|| self.runtime_err("TypeError", "storage keys must be str"))?;
                    let Some(h) = host else {
                        return Err(self.runtime_err("RuntimeError", "storage unavailable outside call"));
                    };
                    h.storage_get(key)
                        .map(PyVal::Str)
                        .ok_or_else(|| self.runtime_err("KeyError", format!("storage key '{key}' not found")))
                }
                _ => Err(self.runtime_err(
                    "TypeError",
                    format!("'{}' object is not subscriptable", self.type_name(obj)),
                )),
            },
            _ => Err(self.runtime_err(
                "TypeError",
                format!("'{}' object is not subscriptable", self.type_name(obj)),
            )),
        }
    }

    pub fn set_item(
        &mut self,
        obj: &PyVal,
        idx: PyVal,
        val: PyVal,
        host: Option<&mut HostBridge<'_>>,
    ) -> Result<(), PyError> {
        let PyVal::Handle(id) = obj else {
            return Err(self.runtime_err(
                "TypeError",
                format!(
                    "'{}' object does not support item assignment",
                    self.type_name(obj)
                ),
            ));
        };
        if matches!(
            self.heap.get(*id),
            Some(HeapObj::Native(NativeObjKind::Storage))
        ) {
            let key = idx
                .as_str()
                .ok_or_else(|| self.runtime_err("TypeError", "storage keys must be str"))?
                .to_string();
            let v_str = self.val_to_str(&val);
            let Some(h) = host else {
                return Err(self.runtime_err("RuntimeError", "storage unavailable outside call"));
            };
            if !h.has_perm(cs_mod_api::Permission::Storage) {
                return Err(self.runtime_err("PermissionError", "storage permission required"));
            }
            h.storage_set(key, v_str);
            return Ok(());
        }
        let idx_int = idx.as_int();
        let idx_key = PyKey::from_val(&idx);
        let obj_type = self.type_name(obj);
        let frames = &self.frames;
        match self.heap.get_mut(*id) {
            Some(HeapObj::List(items)) => {
                let len = items.len();
                let i = idx_int.ok_or_else(|| {
                    PyError::runtime("TypeError", "list indices must be integers", frames.clone())
                })?;
                let pos = normalize_index(i, len).ok_or_else(|| {
                    PyError::runtime(
                        "IndexError",
                        "list assignment index out of range",
                        frames.clone(),
                    )
                })?;
                items[pos] = val;
                Ok(())
            }
            Some(HeapObj::Dict(entries)) => {
                let k = idx_key.ok_or_else(|| {
                    PyError::runtime("TypeError", "unhashable dict key", frames.clone())
                })?;
                if let Some(pos) = entries.iter().position(|(ek, _)| ek == &k) {
                    entries[pos].1 = val;
                } else {
                    if entries.len() >= MAX_SEQ_LEN {
                        return Err(PyError::runtime(
                            "MemoryError",
                            "dict size limit exceeded",
                            frames.clone(),
                        ));
                    }
                    entries.push((k, val));
                }
                Ok(())
            }
            _ => Err(PyError::runtime(
                "TypeError",
                format!("'{obj_type}' object does not support item assignment"),
                frames.clone(),
            )),
        }
    }

    fn del_item(
        &mut self,
        obj: &PyVal,
        idx: &PyVal,
        host: Option<&mut HostBridge<'_>>,
    ) -> Result<(), PyError> {
        let PyVal::Handle(id) = obj else {
            return Err(self.runtime_err("TypeError", "object does not support item deletion"));
        };
        if matches!(
            self.heap.get(*id),
            Some(HeapObj::Native(NativeObjKind::Storage))
        ) {
            let key = idx
                .as_str()
                .ok_or_else(|| self.runtime_err("TypeError", "storage keys must be str"))?
                .to_string();
            let Some(h) = host else {
                return Err(self.runtime_err("RuntimeError", "storage unavailable outside call"));
            };
            if !h.has_perm(cs_mod_api::Permission::Storage) {
                return Err(self.runtime_err("PermissionError", "storage permission required"));
            }
            h.storage_delete(key);
            return Ok(());
        }
        let idx_int = idx.as_int();
        let idx_key = PyKey::from_val(idx);
        let frames = &self.frames;
        match self.heap.get_mut(*id) {
            Some(HeapObj::List(items)) => {
                let len = items.len();
                let i = idx_int.ok_or_else(|| {
                    PyError::runtime("TypeError", "list indices must be integers", frames.clone())
                })?;
                let pos = normalize_index(i, len).ok_or_else(|| {
                    PyError::runtime("IndexError", "list index out of range", frames.clone())
                })?;
                items.remove(pos);
                Ok(())
            }
            Some(HeapObj::Dict(entries)) => {
                let k = idx_key.ok_or_else(|| {
                    PyError::runtime("TypeError", "unhashable dict key", frames.clone())
                })?;
                if let Some(pos) = entries.iter().position(|(ek, _)| ek == &k) {
                    entries.remove(pos);
                    Ok(())
                } else {
                    Err(PyError::runtime("KeyError", "key not found", frames.clone()))
                }
            }
            _ => Err(PyError::runtime(
                "TypeError",
                "object does not support item deletion",
                frames.clone(),
            )),
        }
    }

    fn eval_slice(
        &mut self,
        obj: &PyVal,
        lower: Option<PyVal>,
        upper: Option<PyVal>,
        step: Option<PyVal>,
    ) -> Result<PyVal, PyError> {
        let parse_opt_int = |v: Option<PyVal>| -> Result<Option<i64>, PyError> {
            match v {
                None | Some(PyVal::None) => Ok(None),
                Some(x) => x
                    .as_int()
                    .map(Some)
                    .ok_or_else(|| self.runtime_err("TypeError", "slice indices must be integers or None")),
            }
        };
        let lo = parse_opt_int(lower)?;
        let hi = parse_opt_int(upper)?;
        let st = parse_opt_int(step)?.unwrap_or(1);
        if st == 0 {
            return Err(self.runtime_err("ValueError", "slice step cannot be zero"));
        }

        match obj {
            PyVal::Str(s) => {
                let chars: Vec<char> = s.chars().collect();
                let indices = compute_slice_indices(chars.len(), lo, hi, st);
                let out: String = indices.into_iter().map(|i| chars[i]).collect();
                Ok(PyVal::Str(out))
            }
            PyVal::Tuple(items) => {
                let indices = compute_slice_indices(items.len(), lo, hi, st);
                Ok(PyVal::Tuple(
                    indices.into_iter().map(|i| items[i].clone()).collect(),
                ))
            }
            PyVal::Handle(id) => match self.heap.get(*id).cloned() {
                Some(HeapObj::List(items)) => {
                    let indices = compute_slice_indices(items.len(), lo, hi, st);
                    let out: Vec<PyVal> = indices.into_iter().map(|i| items[i].clone()).collect();
                    let new_id = self.alloc(HeapObj::List(out))?;
                    Ok(PyVal::Handle(new_id))
                }
                _ => Err(self.runtime_err("TypeError", "object is not sliceable")),
            },
            _ => Err(self.runtime_err("TypeError", "object is not sliceable")),
        }
    }

    pub fn eval_binop(
        &mut self,
        op: BinOp,
        left: PyVal,
        right: PyVal,
        _host: Option<&mut HostBridge<'_>>,
    ) -> Result<PyVal, PyError> {
        match op {
            BinOp::Add => match (left, right) {
                (PyVal::Int(a), PyVal::Int(b)) => Ok(PyVal::Int(a.wrapping_add(b))),
                (PyVal::Bool(a), PyVal::Int(b)) => Ok(PyVal::Int(i64::from(a).wrapping_add(b))),
                (PyVal::Int(a), PyVal::Bool(b)) => Ok(PyVal::Int(a.wrapping_add(i64::from(b)))),
                (PyVal::Bool(a), PyVal::Bool(b)) => Ok(PyVal::Int(i64::from(a) + i64::from(b))),
                (PyVal::Float(a), b) if b.as_float().is_some() => {
                    Ok(PyVal::Float(a + b.as_float().unwrap()))
                }
                (a, PyVal::Float(b)) if a.as_float().is_some() => {
                    Ok(PyVal::Float(a.as_float().unwrap() + b))
                }
                (PyVal::Str(a), PyVal::Str(b)) => {
                    if a.len() + b.len() > MAX_STR_LEN {
                        return Err(self.runtime_err("MemoryError", "string length limit exceeded"));
                    }
                    Ok(PyVal::Str(format!("{a}{b}")))
                }
                (PyVal::Tuple(mut a), PyVal::Tuple(b)) => {
                    a.extend(b);
                    Ok(PyVal::Tuple(a))
                }
                (PyVal::Handle(aid), PyVal::Handle(bid)) => {
                    match (self.heap.get(aid).cloned(), self.heap.get(bid).cloned()) {
                        (Some(HeapObj::List(mut a)), Some(HeapObj::List(b))) => {
                            if a.len() + b.len() > MAX_SEQ_LEN {
                                return Err(self.runtime_err("MemoryError", "list length limit exceeded"));
                            }
                            a.extend(b);
                            let nid = self.alloc(HeapObj::List(a))?;
                            Ok(PyVal::Handle(nid))
                        }
                        _ => Err(self.runtime_err("TypeError", "unsupported operand types for +")),
                    }
                }
                _ => Err(self.runtime_err("TypeError", "unsupported operand types for +")),
            },
            BinOp::Sub => match (&left, &right) {
                (PyVal::Handle(aid), PyVal::Handle(bid)) => {
                    if let (Some(HeapObj::Set(a)), Some(HeapObj::Set(b))) =
                        (self.heap.get(*aid).cloned(), self.heap.get(*bid).cloned())
                    {
                        let diff: Vec<PyKey> = a.into_iter().filter(|k| !b.contains(k)).collect();
                        let nid = self.alloc(HeapObj::Set(diff))?;
                        return Ok(PyVal::Handle(nid));
                    }
                    Err(self.runtime_err("TypeError", "unsupported operand types for -"))
                }
                _ => self.numeric_binop(left, right, |a, b| Ok(a.wrapping_sub(b)), |a, b| Ok(a - b)),
            },
            BinOp::Mul => match (left, right) {
                (PyVal::Str(s), n_val) | (n_val, PyVal::Str(s)) if n_val.as_int().is_some() => {
                    let n = n_val.as_int().unwrap().max(0) as usize;
                    if s.len().saturating_mul(n) > MAX_STR_LEN {
                        return Err(self.runtime_err("MemoryError", "string repetition exceeds limit"));
                    }
                    self.consume_fuel((n as u64).min(1000))?;
                    Ok(PyVal::Str(s.repeat(n)))
                }
                (PyVal::Tuple(items), n_val) | (n_val, PyVal::Tuple(items))
                    if n_val.as_int().is_some() =>
                {
                    let n = n_val.as_int().unwrap().max(0) as usize;
                    if items.len().saturating_mul(n) > MAX_SEQ_LEN {
                        return Err(self.runtime_err("MemoryError", "tuple repetition exceeds limit"));
                    }
                    let mut out = Vec::with_capacity(items.len() * n);
                    for _ in 0..n {
                        out.extend(items.iter().cloned());
                    }
                    Ok(PyVal::Tuple(out))
                }
                (PyVal::Handle(lid), n_val) | (n_val, PyVal::Handle(lid))
                    if n_val.as_int().is_some() =>
                {
                    if let Some(HeapObj::List(items)) = self.heap.get(lid).cloned() {
                        let n = n_val.as_int().unwrap().max(0) as usize;
                        if items.len().saturating_mul(n) > MAX_SEQ_LEN {
                            return Err(self.runtime_err("MemoryError", "list repetition exceeds limit"));
                        }
                        let mut out = Vec::with_capacity(items.len() * n);
                        for _ in 0..n {
                            out.extend(items.iter().cloned());
                        }
                        let nid = self.alloc(HeapObj::List(out))?;
                        return Ok(PyVal::Handle(nid));
                    }
                    Err(self.runtime_err("TypeError", "unsupported operand types for *"))
                }
                (l, r) => self.numeric_binop(l, r, |a, b| Ok(a.wrapping_mul(b)), |a, b| Ok(a * b)),
            },
            BinOp::Div => {
                let (Some(a), Some(b)) = (left.as_float(), right.as_float()) else {
                    return Err(self.runtime_err("TypeError", "unsupported operand types for /"));
                };
                if b == 0.0 {
                    return Err(self.runtime_err("ZeroDivisionError", "division by zero"));
                }
                Ok(PyVal::Float(a / b))
            }
            BinOp::FloorDiv => match (left, right) {
                (a, b) if a.as_int().is_some() && b.as_int().is_some() => {
                    let ai = a.as_int().unwrap();
                    let bi = b.as_int().unwrap();
                    if bi == 0 {
                        return Err(self.runtime_err("ZeroDivisionError", "integer division or modulo by zero"));
                    }
                    Ok(PyVal::Int(ai.div_euclid(bi) - if bi < 0 && ai % bi != 0 { 1 } else { 0 }))
                }
                (a, b) if a.as_float().is_some() && b.as_float().is_some() => {
                    let af = a.as_float().unwrap();
                    let bf = b.as_float().unwrap();
                    if bf == 0.0 {
                        return Err(self.runtime_err("ZeroDivisionError", "float floor division by zero"));
                    }
                    Ok(PyVal::Float((af / bf).floor()))
                }
                _ => Err(self.runtime_err("TypeError", "unsupported operand types for //")),
            },
            BinOp::Mod => {
                if let PyVal::Str(fmt) = &left {
                    return stdlib::printf_percent_format(self, fmt, &right);
                }
                match (left, right) {
                    (a, b) if a.as_int().is_some() && b.as_int().is_some() => {
                        let ai = a.as_int().unwrap();
                        let bi = b.as_int().unwrap();
                        if bi == 0 {
                            return Err(self.runtime_err("ZeroDivisionError", "integer modulo by zero"));
                        }
                        // Python modulo has sign of divisor
                        let r = ai % bi;
                        let py_mod = if r != 0 && (r < 0) != (bi < 0) {
                            r + bi
                        } else {
                            r
                        };
                        Ok(PyVal::Int(py_mod))
                    }
                    (a, b) if a.as_float().is_some() && b.as_float().is_some() => {
                        let af = a.as_float().unwrap();
                        let bf = b.as_float().unwrap();
                        if bf == 0.0 {
                            return Err(self.runtime_err("ZeroDivisionError", "float modulo by zero"));
                        }
                        let r = af % bf;
                        let py_mod = if r != 0.0 && (r < 0.0) != (bf < 0.0) {
                            r + bf
                        } else {
                            r
                        };
                        Ok(PyVal::Float(py_mod))
                    }
                    _ => Err(self.runtime_err("TypeError", "unsupported operand types for %")),
                }
            }
            BinOp::Pow => match (left, right) {
                (a, b) if a.as_int().is_some() && b.as_int().is_some() => {
                    let ai = a.as_int().unwrap();
                    let bi = b.as_int().unwrap();
                    if (0..=63).contains(&bi)
                        && let Some(p) = ai.checked_pow(bi as u32)
                    {
                        return Ok(PyVal::Int(p));
                    }
                    Ok(PyVal::Float((ai as f64).powf(bi as f64)))
                }
                (a, b) if a.as_float().is_some() && b.as_float().is_some() => {
                    Ok(PyVal::Float(a.as_float().unwrap().powf(b.as_float().unwrap())))
                }
                _ => Err(self.runtime_err("TypeError", "unsupported operand types for **")),
            },
            BinOp::LShift => {
                let (Some(a), Some(b)) = (left.as_int(), right.as_int()) else {
                    return Err(self.runtime_err("TypeError", "unsupported operand types for <<"));
                };
                if b < 0 {
                    return Err(self.runtime_err("ValueError", "negative shift count"));
                }
                Ok(PyVal::Int(if b >= 64 { 0 } else { a.wrapping_shl(b as u32) }))
            }
            BinOp::RShift => {
                let (Some(a), Some(b)) = (left.as_int(), right.as_int()) else {
                    return Err(self.runtime_err("TypeError", "unsupported operand types for >>"));
                };
                if b < 0 {
                    return Err(self.runtime_err("ValueError", "negative shift count"));
                }
                Ok(PyVal::Int(if b >= 64 {
                    if a < 0 { -1 } else { 0 }
                } else {
                    a.wrapping_shr(b as u32)
                }))
            }
            BinOp::BitOr => match (&left, &right) {
                (PyVal::Handle(aid), PyVal::Handle(bid)) => {
                    match (self.heap.get(*aid).cloned(), self.heap.get(*bid).cloned()) {
                        (Some(HeapObj::Set(mut a)), Some(HeapObj::Set(b))) => {
                            for k in b {
                                if !a.contains(&k) {
                                    a.push(k);
                                }
                            }
                            let nid = self.alloc(HeapObj::Set(a))?;
                            Ok(PyVal::Handle(nid))
                        }
                        (Some(HeapObj::Dict(mut a)), Some(HeapObj::Dict(b))) => {
                            for (k, v) in b {
                                if let Some(pos) = a.iter().position(|(ek, _)| ek == &k) {
                                    a[pos].1 = v;
                                } else {
                                    a.push((k, v));
                                }
                            }
                            let nid = self.alloc(HeapObj::Dict(a))?;
                            Ok(PyVal::Handle(nid))
                        }
                        _ => Err(self.runtime_err("TypeError", "unsupported operand types for |")),
                    }
                }
                _ => match (left.as_int(), right.as_int()) {
                    (Some(a), Some(b)) => Ok(PyVal::Int(a | b)),
                    _ => Err(self.runtime_err("TypeError", "unsupported operand types for |")),
                },
            },
            BinOp::BitAnd => match (&left, &right) {
                (PyVal::Handle(aid), PyVal::Handle(bid)) => {
                    if let (Some(HeapObj::Set(a)), Some(HeapObj::Set(b))) =
                        (self.heap.get(*aid).cloned(), self.heap.get(*bid).cloned())
                    {
                        let inter: Vec<PyKey> = a.into_iter().filter(|k| b.contains(k)).collect();
                        let nid = self.alloc(HeapObj::Set(inter))?;
                        return Ok(PyVal::Handle(nid));
                    }
                    Err(self.runtime_err("TypeError", "unsupported operand types for &"))
                }
                _ => match (left.as_int(), right.as_int()) {
                    (Some(a), Some(b)) => Ok(PyVal::Int(a & b)),
                    _ => Err(self.runtime_err("TypeError", "unsupported operand types for &")),
                },
            },
            BinOp::BitXor => match (&left, &right) {
                (PyVal::Handle(aid), PyVal::Handle(bid)) => {
                    if let (Some(HeapObj::Set(a)), Some(HeapObj::Set(b))) =
                        (self.heap.get(*aid).cloned(), self.heap.get(*bid).cloned())
                    {
                        let mut out: Vec<PyKey> =
                            a.iter().filter(|k| !b.contains(k)).cloned().collect();
                        out.extend(b.into_iter().filter(|k| !a.contains(k)));
                        let nid = self.alloc(HeapObj::Set(out))?;
                        return Ok(PyVal::Handle(nid));
                    }
                    Err(self.runtime_err("TypeError", "unsupported operand types for ^"))
                }
                _ => match (left.as_int(), right.as_int()) {
                    (Some(a), Some(b)) => Ok(PyVal::Int(a ^ b)),
                    _ => Err(self.runtime_err("TypeError", "unsupported operand types for ^")),
                },
            },
        }
    }

    fn numeric_binop(
        &self,
        left: PyVal,
        right: PyVal,
        int_op: impl FnOnce(i64, i64) -> Result<i64, PyError>,
        float_op: impl FnOnce(f64, f64) -> Result<f64, PyError>,
    ) -> Result<PyVal, PyError> {
        if let (Some(a), Some(b)) = (left.as_int(), right.as_int()) {
            return int_op(a, b).map(PyVal::Int);
        }
        if let (Some(a), Some(b)) = (left.as_float(), right.as_float()) {
            return float_op(a, b).map(PyVal::Float);
        }
        Err(self.runtime_err("TypeError", "unsupported operand types for arithmetic"))
    }

    pub fn eval_cmp(
        &self,
        op: CmpOp,
        left: &PyVal,
        right: &PyVal,
        host: Option<&mut HostBridge<'_>>,
    ) -> Result<bool, PyError> {
        match op {
            CmpOp::Eq => Ok(self.vals_equal(left, right)),
            CmpOp::NotEq => Ok(!self.vals_equal(left, right)),
            CmpOp::Is => Ok(self.vals_identical(left, right)),
            CmpOp::IsNot => Ok(!self.vals_identical(left, right)),
            CmpOp::In => self.val_contains(right, left, host.as_deref()),
            CmpOp::NotIn => Ok(!self.val_contains(right, left, host.as_deref())?),
            CmpOp::Lt | CmpOp::LtE | CmpOp::Gt | CmpOp::GtE => {
                let ord = self.val_cmp(left, right)?;
                Ok(match op {
                    CmpOp::Lt => ord == std::cmp::Ordering::Less,
                    CmpOp::LtE => ord != std::cmp::Ordering::Greater,
                    CmpOp::Gt => ord == std::cmp::Ordering::Greater,
                    CmpOp::GtE => ord != std::cmp::Ordering::Less,
                    _ => unreachable!(),
                })
            }
        }
    }

    pub fn vals_identical(&self, a: &PyVal, b: &PyVal) -> bool {
        match (a, b) {
            (PyVal::None, PyVal::None) => true,
            (PyVal::Bool(x), PyVal::Bool(y)) => x == y,
            (PyVal::Handle(x), PyVal::Handle(y)) => x == y,
            _ => false,
        }
    }

    pub fn vals_equal(&self, a: &PyVal, b: &PyVal) -> bool {
        match (a, b) {
            (PyVal::None, PyVal::None) => true,
            (PyVal::Str(x), PyVal::Str(y)) => x == y,
            (x, y) if x.as_int().is_some() && y.as_int().is_some() => x.as_int() == y.as_int(),
            (x, y) if x.as_float().is_some() && y.as_float().is_some() => {
                x.as_float() == y.as_float()
            }
            (PyVal::Tuple(x), PyVal::Tuple(y)) => {
                x.len() == y.len() && x.iter().zip(y.iter()).all(|(u, v)| self.vals_equal(u, v))
            }
            (PyVal::Handle(x), PyVal::Handle(y)) => {
                if x == y {
                    return true;
                }
                match (self.heap.get(*x), self.heap.get(*y)) {
                    (Some(HeapObj::List(a)), Some(HeapObj::List(b))) => {
                        a.len() == b.len()
                            && a.iter().zip(b.iter()).all(|(u, v)| self.vals_equal(u, v))
                    }
                    (Some(HeapObj::Dict(a)), Some(HeapObj::Dict(b))) => {
                        a.len() == b.len()
                            && a.iter().all(|(k, v)| {
                                b.iter()
                                    .find(|(bk, _)| bk == k)
                                    .is_some_and(|(_, bv)| self.vals_equal(v, bv))
                            })
                    }
                    (Some(HeapObj::Set(a)), Some(HeapObj::Set(b))) => {
                        a.len() == b.len() && a.iter().all(|k| b.contains(k))
                    }
                    (
                        Some(HeapObj::Native(NativeObjKind::Player(sa))),
                        Some(HeapObj::Native(NativeObjKind::Player(sb))),
                    ) => sa == sb,
                    (
                        Some(HeapObj::Native(NativeObjKind::Entity(ea))),
                        Some(HeapObj::Native(NativeObjKind::Entity(eb))),
                    ) => ea == eb,
                    _ => false,
                }
            }
            _ => false,
        }
    }

    pub fn val_cmp(&self, a: &PyVal, b: &PyVal) -> Result<std::cmp::Ordering, PyError> {
        if let (Some(x), Some(y)) = (a.as_int(), b.as_int()) {
            return Ok(x.cmp(&y));
        }
        if let (Some(x), Some(y)) = (a.as_float(), b.as_float()) {
            return x
                .partial_cmp(&y)
                .ok_or_else(|| self.runtime_err("ValueError", "cannot compare NaN"));
        }
        match (a, b) {
            (PyVal::Str(x), PyVal::Str(y)) => Ok(x.cmp(y)),
            (PyVal::Tuple(x), PyVal::Tuple(y)) => {
                for (u, v) in x.iter().zip(y.iter()) {
                    let ord = self.val_cmp(u, v)?;
                    if ord != std::cmp::Ordering::Equal {
                        return Ok(ord);
                    }
                }
                Ok(x.len().cmp(&y.len()))
            }
            (PyVal::Handle(x), PyVal::Handle(y)) => match (self.heap.get(*x), self.heap.get(*y)) {
                (Some(HeapObj::List(xs)), Some(HeapObj::List(ys))) => {
                    for (u, v) in xs.iter().zip(ys.iter()) {
                        let ord = self.val_cmp(u, v)?;
                        if ord != std::cmp::Ordering::Equal {
                            return Ok(ord);
                        }
                    }
                    Ok(xs.len().cmp(&ys.len()))
                }
                _ => Err(self.runtime_err(
                    "TypeError",
                    format!(
                        "ordering not supported between '{}' and '{}'",
                        self.type_name(a),
                        self.type_name(b)
                    ),
                )),
            },
            _ => Err(self.runtime_err(
                "TypeError",
                format!(
                    "ordering not supported between '{}' and '{}'",
                    self.type_name(a),
                    self.type_name(b)
                ),
            )),
        }
    }

    pub fn val_contains(
        &self,
        container: &PyVal,
        needle: &PyVal,
        host: Option<&HostBridge<'_>>,
    ) -> Result<bool, PyError> {
        match container {
            PyVal::Str(s) => {
                let sub = needle
                    .as_str()
                    .ok_or_else(|| self.runtime_err("TypeError", "'in <string>' requires string as left operand"))?;
                Ok(s.contains(sub))
            }
            PyVal::Tuple(items) => Ok(items.iter().any(|x| self.vals_equal(x, needle))),
            PyVal::Handle(id) => match self.heap.get(*id) {
                Some(HeapObj::List(items)) => Ok(items.iter().any(|x| self.vals_equal(x, needle))),
                Some(HeapObj::Dict(entries)) => {
                    let Some(k) = PyKey::from_val(needle) else {
                        return Ok(false);
                    };
                    Ok(entries.iter().any(|(ek, _)| ek == &k))
                }
                Some(HeapObj::Set(items)) => {
                    let Some(k) = PyKey::from_val(needle) else {
                        return Ok(false);
                    };
                    Ok(items.contains(&k))
                }
                Some(HeapObj::Native(NativeObjKind::Storage)) => {
                    let Some(key) = needle.as_str() else {
                        return Ok(false);
                    };
                    Ok(host.is_some_and(|h| h.storage_get(key).is_some()))
                }
                _ => Err(self.runtime_err(
                    "TypeError",
                    format!("argument of type '{}' is not iterable", self.type_name(container)),
                )),
            },
            _ => Err(self.runtime_err(
                "TypeError",
                format!("argument of type '{}' is not iterable", self.type_name(container)),
            )),
        }
    }

    pub fn is_truthy(&self, val: &PyVal) -> bool {
        match val {
            PyVal::None => false,
            PyVal::Bool(b) => *b,
            PyVal::Int(n) => *n != 0,
            PyVal::Float(f) => *f != 0.0,
            PyVal::Str(s) => !s.is_empty(),
            PyVal::Tuple(items) => !items.is_empty(),
            PyVal::Handle(id) => match self.heap.get(*id) {
                Some(HeapObj::List(items)) => !items.is_empty(),
                Some(HeapObj::Dict(entries)) => !entries.is_empty(),
                Some(HeapObj::Set(items)) => !items.is_empty(),
                _ => true,
            },
        }
    }

    pub fn to_vec(&self, val: &PyVal) -> Result<Vec<PyVal>, PyError> {
        match val {
            PyVal::Str(s) => Ok(s.chars().map(|c| PyVal::Str(c.to_string())).collect()),
            PyVal::Tuple(items) => Ok(items.clone()),
            PyVal::Handle(id) => match self.heap.get(*id) {
                Some(HeapObj::List(items)) => Ok(items.clone()),
                Some(HeapObj::Dict(entries)) => Ok(entries.iter().map(|(k, _)| k.to_val()).collect()),
                Some(HeapObj::Set(items)) => Ok(items.iter().map(PyKey::to_val).collect()),
                _ => Err(self.runtime_err(
                    "TypeError",
                    format!("'{}' object is not iterable", self.type_name(val)),
                )),
            },
            _ => Err(self.runtime_err(
                "TypeError",
                format!("'{}' object is not iterable", self.type_name(val)),
            )),
        }
    }

    pub fn type_name(&self, val: &PyVal) -> String {
        match val {
            PyVal::None => "NoneType".to_string(),
            PyVal::Bool(_) => "bool".to_string(),
            PyVal::Int(_) => "int".to_string(),
            PyVal::Float(_) => "float".to_string(),
            PyVal::Str(_) => "str".to_string(),
            PyVal::Tuple(_) => "tuple".to_string(),
            PyVal::Handle(id) => match self.heap.get(*id) {
                Some(HeapObj::List(_)) => "list".to_string(),
                Some(HeapObj::Dict(_)) => "dict".to_string(),
                Some(HeapObj::Set(_)) => "set".to_string(),
                Some(HeapObj::Function(_) | HeapObj::BoundMethod { .. } | HeapObj::NativeFn(_) | HeapObj::BoundNativeMethod { .. }) => {
                    "function".to_string()
                }
                Some(HeapObj::Class(_)) => "type".to_string(),
                Some(HeapObj::Instance(inst)) => match self.heap.get(inst.class_id) {
                    Some(HeapObj::Class(c)) => c.name.clone(),
                    _ => "object".to_string(),
                },
                Some(HeapObj::Native(n)) => match n {
                    NativeObjKind::Mod => "Mod".to_string(),
                    NativeObjKind::Vfx => "Vfx".to_string(),
                    NativeObjKind::Storage => "Storage".to_string(),
                    NativeObjKind::Leaderboard => "Leaderboard".to_string(),
                    NativeObjKind::Player(_) => "Player".to_string(),
                    NativeObjKind::Entity(_) => "Entity".to_string(),
                    NativeObjKind::Event(_) => "Event".to_string(),
                    NativeObjKind::Menu(_) => "Menu".to_string(),
                    NativeObjKind::CustomWeapon(_) => "CustomWeapon".to_string(),
                    NativeObjKind::Module { name, .. } => format!("module '{name}'"),
                    NativeObjKind::Decorator(_) => "decorator".to_string(),
                    NativeObjKind::ExceptionType(_) => "type".to_string(),
                    NativeObjKind::ExceptionObj { kind, .. } => kind.clone(),
                },
                _ => "object".to_string(),
            },
        }
    }

    pub fn val_to_str(&self, val: &PyVal) -> String {
        match val {
            PyVal::None => "None".to_string(),
            PyVal::Bool(true) => "True".to_string(),
            PyVal::Bool(false) => "False".to_string(),
            PyVal::Int(n) => n.to_string(),
            PyVal::Float(f) => {
                let s = format!("{f}");
                if !s.contains('.') && !s.contains('e') && !s.contains('E') && f.is_finite() {
                    format!("{s}.0")
                } else {
                    s
                }
            }
            PyVal::Str(s) => s.clone(),
            PyVal::Tuple(items) => {
                if items.len() == 1 {
                    format!("({},)", self.val_to_repr(&items[0]))
                } else {
                    let inner: Vec<String> = items.iter().map(|v| self.val_to_repr(v)).collect();
                    format!("({})", inner.join(", "))
                }
            }
            PyVal::Handle(id) => match self.heap.get(*id) {
                Some(HeapObj::List(items)) => {
                    let inner: Vec<String> = items.iter().map(|v| self.val_to_repr(v)).collect();
                    format!("[{}]", inner.join(", "))
                }
                Some(HeapObj::Dict(entries)) => {
                    let inner: Vec<String> = entries
                        .iter()
                        .map(|(k, v)| {
                            format!("{}: {}", self.val_to_repr(&k.to_val()), self.val_to_repr(v))
                        })
                        .collect();
                    format!("{{{}}}", inner.join(", "))
                }
                Some(HeapObj::Set(items)) => {
                    if items.is_empty() {
                        "set()".to_string()
                    } else {
                        let inner: Vec<String> = items
                            .iter()
                            .map(|k| self.val_to_repr(&k.to_val()))
                            .collect();
                        format!("{{{}}}", inner.join(", "))
                    }
                }
                Some(HeapObj::Native(NativeObjKind::Player(slot))) => format!("<Player slot={slot}>"),
                Some(HeapObj::Native(NativeObjKind::Entity(idx))) => format!("<Entity index={idx}>"),
                Some(HeapObj::Native(NativeObjKind::ExceptionObj { message, .. })) => {
                    message.clone()
                }
                _ => format!("<{}>", self.type_name(val)),
            },
        }
    }

    pub fn val_to_repr(&self, val: &PyVal) -> String {
        match val {
            PyVal::Str(s) => {
                let mut out = String::from("'");
                for ch in s.chars() {
                    match ch {
                        '\'' => out.push_str("\\'"),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        c => out.push(c),
                    }
                }
                out.push('\'');
                out
            }
            _ => self.val_to_str(val),
        }
    }

    fn exception_matches(&self, err: &PyError, ty_val: &PyVal) -> bool {
        let err_kind = err.exception_kind();
        match ty_val {
            PyVal::Tuple(items) => items.iter().any(|t| self.exception_matches(err, t)),
            PyVal::Handle(id) => match self.heap.get(*id) {
                Some(HeapObj::Native(NativeObjKind::ExceptionType(target))) => {
                    exception_is_subclass(err_kind, target)
                }
                Some(HeapObj::Class(c)) => exception_is_subclass(err_kind, &c.name),
                _ => false,
            },
            _ => false,
        }
    }
}

fn exception_is_subclass(kind: &str, base: &str) -> bool {
    if kind == base || base == "BaseException" || base == "Exception" {
        return true;
    }
    match base {
        "LookupError" => matches!(kind, "KeyError" | "IndexError"),
        "ArithmeticError" => matches!(kind, "ZeroDivisionError" | "OverflowError"),
        "OSError" => matches!(kind, "PermissionError" | "FileNotFoundError"),
        _ => false,
    }
}

pub fn normalize_index(idx: i64, len: usize) -> Option<usize> {
    let i = if idx < 0 {
        (len as i64).checked_add(idx)?
    } else {
        idx
    };
    if i >= 0 && (i as usize) < len {
        Some(i as usize)
    } else {
        None
    }
}

fn compute_slice_indices(len: usize, lower: Option<i64>, upper: Option<i64>, step: i64) -> Vec<usize> {
    let n = len as i64;
    let mut out = Vec::new();
    if step > 0 {
        let mut start = lower.unwrap_or(0);
        if start < 0 {
            start += n;
        }
        start = start.clamp(0, n);
        let mut stop = upper.unwrap_or(n);
        if stop < 0 {
            stop += n;
        }
        stop = stop.clamp(0, n);
        let mut i = start;
        while i < stop {
            out.push(i as usize);
            i += step;
        }
    } else {
        let mut start = lower.unwrap_or(n - 1);
        if start < 0 {
            start += n;
        }
        start = start.clamp(-1, n - 1);
        let mut stop = upper.unwrap_or(-1);
        if upper.is_some() && stop < 0 {
            stop += n;
        }
        stop = stop.clamp(-1, n - 1);
        let mut i = start;
        while i > stop {
            out.push(i as usize);
            i += step;
        }
    }
    out
}
