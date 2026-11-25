//! Runtime value representation and mark-and-sweep GC heap (`Send`, zero `unsafe`).

use std::collections::{BTreeMap, HashSet};

use cs_mod_api::{CustomWeaponSpec, HookId, HookPhase, MenuItemSpec};

use crate::ast::{Params, Stmt};

/// Handle into the VM's owned object arena.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjId(pub usize);

/// Runtime Python value.
#[derive(Debug, Clone)]
pub enum PyVal {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Tuple(Vec<PyVal>),
    Handle(ObjId),
}

impl PyVal {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            Self::Bool(b) => Some(i64::from(*b)),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(f) => Some(*f),
            Self::Int(n) => Some(*n as f64),
            Self::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn approx_size(&self) -> usize {
        match self {
            Self::None | Self::Bool(_) | Self::Int(_) | Self::Float(_) | Self::Handle(_) => 24,
            Self::Str(s) => 24 + s.len(),
            Self::Tuple(items) => 24 + items.iter().map(Self::approx_size).sum::<usize>(),
        }
    }
}

/// Hashable key for Python `dict` and `set`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PyKey {
    None,
    Int(i64),
    FloatBits(u64),
    Str(String),
    Tuple(Vec<PyKey>),
}

impl PyKey {
    pub fn from_val(val: &PyVal) -> Option<Self> {
        match val {
            PyVal::None => Some(Self::None),
            PyVal::Bool(b) => Some(Self::Int(i64::from(*b))),
            PyVal::Int(n) => Some(Self::Int(*n)),
            PyVal::Float(f) => {
                if f.fract() == 0.0 && f.is_finite() && *f >= i64::MIN as f64 && *f <= i64::MAX as f64 {
                    Some(Self::Int(*f as i64))
                } else {
                    let bits = if *f == 0.0 { 0.0f64.to_bits() } else { f.to_bits() };
                    Some(Self::FloatBits(bits))
                }
            }
            PyVal::Str(s) => Some(Self::Str(s.clone())),
            PyVal::Tuple(items) => {
                let mut keys = Vec::with_capacity(items.len());
                for it in items {
                    keys.push(Self::from_val(it)?);
                }
                Some(Self::Tuple(keys))
            }
            PyVal::Handle(_) => None,
        }
    }

    pub fn to_val(&self) -> PyVal {
        match self {
            Self::None => PyVal::None,
            Self::Int(n) => PyVal::Int(*n),
            Self::FloatBits(b) => PyVal::Float(f64::from_bits(*b)),
            Self::Str(s) => PyVal::Str(s.clone()),
            Self::Tuple(items) => PyVal::Tuple(items.iter().map(Self::to_val).collect()),
        }
    }
}

/// Metadata extracted from `@mod.info(...)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PyPluginMeta {
    pub id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub permissions: Vec<String>,
    pub fuel_per_frame: Option<u64>,
}

/// User-defined Python function or lambda.
#[derive(Debug, Clone)]
pub struct PyFunc {
    pub name: String,
    pub params: Params,
    pub defaults: Vec<Option<PyVal>>,
    pub body: Vec<Stmt>,
    pub closure: Option<ObjId>,
    pub line: usize,
}

/// User-defined Python class.
#[derive(Debug, Clone)]
pub struct PyClass {
    pub name: String,
    pub bases: Vec<ObjId>,
    pub attrs: BTreeMap<String, PyVal>,
}

/// Instance of a user-defined Python class.
#[derive(Debug, Clone)]
pub struct PyInstance {
    pub class_id: ObjId,
    pub attrs: BTreeMap<String, PyVal>,
}

/// Mutable hook event object exposed to `@mod.hook(...)` callbacks.
#[derive(Debug, Clone)]
pub struct PyEventObj {
    pub hook: HookId,
    pub phase: HookPhase,
    pub fields: BTreeMap<String, PyVal>,
    pub single_player_slot: Option<u8>,
    pub mutated_damage: Option<f32>,
    pub mutated_hitgroup: Option<i32>,
    pub mutated_message: Option<String>,
    pub mutated_price: Option<i32>,
    pub mutated_should_collide: Option<bool>,
}

/// Paginated GoldSrc menu builder exposed as `cs16.Menu`.
#[derive(Debug, Clone)]
pub struct PyMenuObj {
    pub title: String,
    pub items: Vec<MenuItemSpec>,
    pub callback: Option<PyVal>,
    pub per_page: u8,
    pub allow_exit: bool,
    pub back_label: Option<String>,
    pub next_label: Option<String>,
    pub exit_label: Option<String>,
    pub number_color: String,
    pub created_id: Option<u32>,
}

/// Decorator objects produced by `@mod.info(...)`, `@mod.hook(...)`, etc.
#[derive(Debug, Clone)]
pub enum PyDecoratorKind {
    Info(PyPluginMeta),
    Hook {
        hook: HookId,
        phase: HookPhase,
        priority: i32,
    },
    ChatCommand {
        commands: Vec<String>,
        suppress: bool,
    },
    ConsoleCommand {
        commands: Vec<String>,
    },
    Timer {
        interval_sec: f64,
        repeat: bool,
        arg: i32,
    },
    ExportFn {
        name: String,
    },
    OnEvent {
        event_name: String,
    },
}

/// Native host / stdlib objects backed by Rust.
#[derive(Debug, Clone)]
pub enum NativeObjKind {
    Mod,
    Vfx,
    Storage,
    Leaderboard,
    Player(u8),
    Entity(u16),
    Event(PyEventObj),
    Menu(PyMenuObj),
    CustomWeapon(CustomWeaponSpec),
    Module {
        name: String,
        attrs: BTreeMap<String, PyVal>,
    },
    Decorator(PyDecoratorKind),
    ExceptionType(String),
    ExceptionObj {
        kind: String,
        message: String,
    },
}

/// Heap-allocated Python object stored in [`Heap`].
#[derive(Debug, Clone)]
pub enum HeapObj {
    List(Vec<PyVal>),
    Dict(Vec<(PyKey, PyVal)>),
    Set(Vec<PyKey>),
    Function(PyFunc),
    BoundMethod {
        receiver: PyVal,
        func: PyVal,
    },
    NativeFn(String),
    BoundNativeMethod {
        receiver: PyVal,
        method: String,
    },
    Class(PyClass),
    Instance(PyInstance),
    Native(NativeObjKind),
    EnvScope {
        vars: BTreeMap<String, PyVal>,
        parent: Option<ObjId>,
    },
}

impl HeapObj {
    pub fn approx_size(&self) -> usize {
        match self {
            Self::List(items) => 48 + items.iter().map(PyVal::approx_size).sum::<usize>(),
            Self::Dict(entries) => {
                64 + entries
                    .iter()
                    .map(|(k, v)| k.to_val().approx_size() + v.approx_size())
                    .sum::<usize>()
            }
            Self::Set(items) => 48 + items.len() * 32,
            Self::Function(f) => 128 + f.body.len() * 48,
            Self::BoundMethod { receiver, func } => 48 + receiver.approx_size() + func.approx_size(),
            Self::NativeFn(s) => 32 + s.len(),
            Self::BoundNativeMethod { receiver, method } => {
                48 + receiver.approx_size() + method.len()
            }
            Self::Class(c) => {
                96 + c.attrs.values().map(PyVal::approx_size).sum::<usize>()
            }
            Self::Instance(i) => {
                64 + i.attrs.values().map(PyVal::approx_size).sum::<usize>()
            }
            Self::Native(NativeObjKind::Module { attrs, .. }) => {
                64 + attrs.values().map(PyVal::approx_size).sum::<usize>()
            }
            Self::Native(_) => 128,
            Self::EnvScope { vars, .. } => {
                64 + vars.values().map(PyVal::approx_size).sum::<usize>()
            }
        }
    }

    fn trace_children(&self, stack: &mut Vec<ObjId>) {
        let push_val = |v: &PyVal, s: &mut Vec<ObjId>| match v {
            PyVal::Handle(id) => s.push(*id),
            PyVal::Tuple(items) => {
                for it in items {
                    if let PyVal::Handle(id) = it {
                        s.push(*id);
                    }
                }
            }
            _ => {}
        };
        match self {
            Self::List(items) => {
                for v in items {
                    push_val(v, stack);
                }
            }
            Self::Dict(entries) => {
                for (_, v) in entries {
                    push_val(v, stack);
                }
            }
            Self::Set(_) | Self::NativeFn(_) => {}
            Self::Function(f) => {
                if let Some(c) = f.closure {
                    stack.push(c);
                }
                for d in f.defaults.iter().flatten() {
                    push_val(d, stack);
                }
            }
            Self::BoundMethod { receiver, func } => {
                push_val(receiver, stack);
                push_val(func, stack);
            }
            Self::BoundNativeMethod { receiver, .. } => {
                push_val(receiver, stack);
            }
            Self::Class(c) => {
                stack.extend(c.bases.iter().copied());
                for v in c.attrs.values() {
                    push_val(v, stack);
                }
            }
            Self::Instance(i) => {
                stack.push(i.class_id);
                for v in i.attrs.values() {
                    push_val(v, stack);
                }
            }
            Self::Native(NativeObjKind::Module { attrs, .. }) => {
                for v in attrs.values() {
                    push_val(v, stack);
                }
            }
            Self::Native(NativeObjKind::Event(ev)) => {
                for v in ev.fields.values() {
                    push_val(v, stack);
                }
            }
            Self::Native(NativeObjKind::Menu(m)) => {
                if let Some(ref cb) = m.callback {
                    push_val(cb, stack);
                }
            }
            Self::Native(_) => {}
            Self::EnvScope { vars, parent } => {
                if let Some(p) = parent {
                    stack.push(*p);
                }
                for v in vars.values() {
                    push_val(v, stack);
                }
            }
        }
    }
}

/// Owned object arena with exact memory cap tracking and between-call mark-and-sweep GC.
#[derive(Debug)]
pub struct Heap {
    slots: Vec<Option<HeapObj>>,
    free_list: Vec<usize>,
    live_count: usize,
    approx_bytes: usize,
    max_bytes: usize,
}

impl Heap {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            slots: Vec::with_capacity(256),
            free_list: Vec::new(),
            live_count: 0,
            approx_bytes: 0,
            max_bytes: max_bytes.max(65_536),
        }
    }

    pub fn approx_bytes(&self) -> usize {
        self.approx_bytes + self.slots.len() * 16
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn live_count(&self) -> usize {
        self.live_count
    }

    pub fn alloc(&mut self, obj: HeapObj) -> Result<ObjId, usize> {
        let sz = obj.approx_size();
        if self.approx_bytes().saturating_add(sz) > self.max_bytes {
            return Err(self.max_bytes);
        }
        self.approx_bytes = self.approx_bytes.saturating_add(sz);
        self.live_count += 1;
        if let Some(idx) = self.free_list.pop() {
            self.slots[idx] = Some(obj);
            Ok(ObjId(idx))
        } else {
            let idx = self.slots.len();
            self.slots.push(Some(obj));
            Ok(ObjId(idx))
        }
    }

    pub fn get(&self, id: ObjId) -> Option<&HeapObj> {
        self.slots.get(id.0).and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, id: ObjId) -> Option<&mut HeapObj> {
        self.slots.get_mut(id.0).and_then(Option::as_mut)
    }

    /// Recomputes live bytes and sweeps unreachable objects when called between top-level calls.
    pub fn gc_collect(&mut self, roots: &[PyVal]) {
        let mut marked = HashSet::with_capacity(self.live_count.min(1024));
        let mut stack = Vec::new();
        for r in roots {
            collect_val_handles(r, &mut stack);
        }
        while let Some(id) = stack.pop() {
            if !marked.insert(id) {
                continue;
            }
            if let Some(obj) = self.get(id) {
                obj.trace_children(&mut stack);
            }
        }

        let mut new_bytes = 0usize;
        let mut new_live = 0usize;
        self.free_list.clear();
        for (idx, slot) in self.slots.iter_mut().enumerate() {
            let id = ObjId(idx);
            if marked.contains(&id) {
                if let Some(ref obj) = *slot {
                    new_bytes = new_bytes.saturating_add(obj.approx_size());
                    new_live += 1;
                }
            } else {
                *slot = None;
                self.free_list.push(idx);
            }
        }
        self.approx_bytes = new_bytes;
        self.live_count = new_live;
    }
}

fn collect_val_handles(val: &PyVal, out: &mut Vec<ObjId>) {
    match val {
        PyVal::Handle(id) => out.push(*id),
        PyVal::Tuple(items) => {
            for it in items {
                collect_val_handles(it, out);
            }
        }
        _ => {}
    }
}
