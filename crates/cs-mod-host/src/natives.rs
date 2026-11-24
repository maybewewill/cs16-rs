//! Host snapshot, transactional mutations, and sandboxed memory I/O shared by server and client
//! WASM mod runtimes (`cs16_mod` namespace).

use std::collections::BTreeMap;
use std::sync::Arc;

use cs_mod_api::{HookId, HookPhase, HudDrawCmd, Permission, Permissions};
use cs_pm::collision::{Hull, WorldCollision};
use glam::Vec3;
use wasmi::{Caller, Extern, Memory, StoreLimits};

pub const MAX_HOST_STR_BYTES: usize = 4096;
pub const MAX_HOST_PAYLOAD_BYTES: usize = 65_536;

/// Read-only view of one entity captured before calling a WASM hook or timer.
#[derive(Debug, Clone)]
pub struct EntitySnap {
    pub index: u16,
    pub classname: String,
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
    pub health: f32,
    pub flags: u32,
}

/// Read-only view of one client slot captured before calling a WASM hook or timer.
#[derive(Debug, Clone)]
pub struct ClientSnap {
    pub slot: u8,
    pub active: bool,
    pub supports_mods: bool,
    pub name: String,
}

/// Snapshot of engine state exposed read-only to WASM natives during a single invocation.
#[derive(Debug, Clone, Default)]
pub struct EngineSnapshot {
    pub time_secs: f64,
    pub frametime: f32,
    pub max_clients: u8,
    pub map_name: String,
    pub entities: BTreeMap<u16, EntitySnap>,
    pub clients: Vec<ClientSnap>,
    pub cvars: BTreeMap<String, String>,
    pub collision: Option<Arc<WorldCollision>>,
    pub client_health: i32,
    pub client_armor: i32,
    pub client_money: i32,
}

/// Event arguments passed to the currently executing mod hook or client usermsg callback.
#[derive(Debug, Clone, Default)]
pub struct ActiveEvent {
    pub i32_args: [i32; 6],
    pub f32_args: [f32; 4],
    pub str_args: [String; 3],
    pub payload: Vec<u8>,
}

/// Declarative hook registration emitted by a WASM mod during `cs_mod_init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookRegistration {
    pub hook: HookId,
    pub phase: HookPhase,
    pub priority: i32,
}

/// Timer scheduling request emitted by a WASM mod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimerRequest {
    pub delay_ms: u32,
    pub repeat: bool,
    pub cb_id: i32,
    pub arg: i32,
}

/// Transactional engine mutation queued by a mod during execution and replayed after return.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineOp {
    CreateEntity { classname: String },
    RemoveEntity { index: u16 },
    SetHealth { index: u16, health: f32 },
    SetOrigin { index: u16, origin: [f32; 3] },
    SetVelocity { index: u16, velocity: [f32; 3] },
    SetFlags { index: u16, flags: u32 },
    PrintClient { slot: u8, text: String },
    PrintAll { text: String },
    CenterPrint { slot: Option<u8>, text: String },
    SayText { slot: Option<u8>, text: String },
    ShowMenu { slot: u8, keys: u16, time_sec: i8, text: String },
    SendCustomUserMsg { slot: Option<u8>, name: String, data: Vec<u8> },
    SetCvar { name: String, value: String },
    EmitSound { ent_index: u16, channel: u8, sample: String, volume: f32 },
    PrecacheSound { sample: String },
    Kick { slot: u8, reason: String },
    ChangeLevel { map: String },
    RegisterCommand { name: String },
    ServerExec { line: String },
    ClientExec { line: String },
}

/// Per-store host state accessible to `"cs16_mod"` native imports.
pub struct HostCallState {
    pub mod_id: String,
    pub permissions: Permissions,
    pub limits: StoreLimits,
    pub snapshot: EngineSnapshot,
    pub active_event: ActiveEvent,
    pub storage_cache: BTreeMap<String, String>,
    pub storage_writes: Vec<(String, Option<String>)>,
    pub ops: Vec<EngineOp>,
    pub hooks: Vec<HookRegistration>,
    pub timers: Vec<TimerRequest>,
    pub cancel_timers: Vec<u32>,
    pub next_local_timer_id: u32,
    pub hud_cmds: Vec<HudDrawCmd>,
    pub override_i32: Option<i32>,
    pub override_str: Option<String>,
}

impl HostCallState {
    pub fn has_perm(&self, perm: Permission) -> bool {
        self.permissions.contains(perm)
    }

    pub fn trace_fraction(&self, start: [f32; 3], end: [f32; 3]) -> f32 {
        let Some(ref coll) = self.snapshot.collision else {
            return 1.0;
        };
        let p0 = Vec3::from_array(start);
        let p1 = Vec3::from_array(end);
        if !p0.is_finite() || !p1.is_finite() {
            return 1.0;
        }
        coll.trace_world(p0, p1, Hull::Point).fraction
    }
}

pub fn guest_memory(caller: &Caller<'_, HostCallState>) -> Option<Memory> {
    caller.get_export("memory").and_then(Extern::into_memory)
}

pub fn read_guest_bytes(caller: &Caller<'_, HostCallState>, ptr: i32, len: i32, max_len: usize) -> Option<Vec<u8>> {
    let uptr = usize::try_from(ptr).ok()?;
    let ulen = usize::try_from(len).ok()?;
    if ulen > max_len {
        return None;
    }
    let mem = guest_memory(caller)?;
    let mut buf = vec![0u8; ulen];
    mem.read(caller, uptr, &mut buf).ok()?;
    Some(buf)
}

pub fn read_guest_str(caller: &Caller<'_, HostCallState>, ptr: i32, len: i32) -> Option<String> {
    let bytes = read_guest_bytes(caller, ptr, len, MAX_HOST_STR_BYTES)?;
    String::from_utf8(bytes).ok()
}

pub fn write_guest_bytes(caller: &mut Caller<'_, HostCallState>, ptr: i32, cap: i32, src: &[u8]) -> i32 {
    let Ok(uptr) = usize::try_from(ptr) else {
        return -1;
    };
    let Ok(ucap) = usize::try_from(cap) else {
        return -1;
    };
    let Some(mem) = guest_memory(caller) else {
        return -1;
    };
    let copy_len = src.len().min(ucap);
    if mem.write(caller, uptr, &src[..copy_len]).is_err() {
        return -1;
    }
    i32::try_from(src.len()).unwrap_or(i32::MAX)
}

pub fn write_guest_f32_array3(caller: &mut Caller<'_, HostCallState>, ptr: i32, vals: [f32; 3]) -> i32 {
    let mut raw = [0u8; 12];
    raw[0..4].copy_from_slice(&vals[0].to_le_bytes());
    raw[4..8].copy_from_slice(&vals[1].to_le_bytes());
    raw[8..12].copy_from_slice(&vals[2].to_le_bytes());
    if write_guest_bytes(caller, ptr, 12, &raw) == 12 {
        1
    } else {
        -1
    }
}
