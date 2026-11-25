//! Host snapshot, transactional mutations, synchronous polyglot RPC seam, and sandboxed memory
//! I/O shared by server and client WASM mod runtimes (`cs16_mod` + `wasi_snapshot_preview1`).

use std::collections::BTreeMap;
use std::sync::Arc;

use cs_mod_api::services::SharedPlatformServices;
use cs_mod_api::{
    ArmorKind, BarTimeSpec, CustomWeaponSpec, DhudMessageSpec, HookId, HookPhase, HttpPostJsonSpec,
    HudDrawCmd, HudMessageSpec, MenuDisplaySpec, Permission, Permissions, RpcEmitEventSpec,
    RpcValue, ScreenFadeSpec, ScreenShakeSpec, ShowMotdSpec, TeamKind, VfxOp,
};
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
#[derive(Debug, Clone, Default)]
pub struct ClientSnap {
    pub slot: u8,
    pub active: bool,
    pub supports_mods: bool,
    pub name: String,
    pub health: f32,
    pub max_health: f32,
    pub armor: i32,
    pub armor_kind: u8,
    pub money: i32,
    pub frags: i32,
    pub deaths: i32,
    pub team: u8,
    pub alive: bool,
    pub active_weapon: u8,
    pub clip: i32,
    pub bpammo: i32,
    pub weapon_clips: BTreeMap<u8, i32>,
    pub weapon_bpammo: BTreeMap<u8, i32>,
    pub has_defuser: bool,
    pub has_nvg: bool,
    pub steamid: String,
    pub ip: String,
    pub ping: i32,
    pub loss: i32,
    pub aim_origin: [f32; 3],
    pub admin_flags: u32,
    pub lang: String,
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

impl EngineSnapshot {
    pub fn client(&self, slot: u8) -> Option<&ClientSnap> {
        self.clients.iter().find(|c| c.slot == slot)
    }
}

/// Event arguments passed to the currently executing mod hook, RPC, or client usermsg callback.
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

/// Synchronous cross-plugin RPC dispatcher injected into guest runtimes.
pub trait RpcDispatcher: Send + Sync {
    fn call_rpc(&self, fn_name: &str, args: &[RpcValue]) -> Option<RpcValue>;
}

/// Transactional engine mutation queued by a mod during execution and replayed after return.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineOp {
    // --- ABI v1 ops ---
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
    // --- ABI v2 CS / Fun / Engine / CStrike Player Natives ---
    SetPlayerHealth { slot: u8, health: f32 },
    SetPlayerMaxHealth { slot: u8, max_health: f32 },
    SetPlayerArmor { slot: u8, armor: i32, kind: ArmorKind },
    SetPlayerMoney { slot: u8, amount: i32, flash: bool },
    AddPlayerMoney { slot: u8, delta: i32, flash: bool },
    SetPlayerScore { slot: u8, frags: i32, deaths: i32 },
    SetPlayerTeam { slot: u8, team: TeamKind, model_class: Option<u8> },
    RespawnPlayer { slot: u8 },
    SlapPlayer { slot: u8, power: i32, random_dir: bool },
    SlayPlayer { slot: u8, preserve_frags: bool },
    DamagePlayer { victim: u8, attacker: Option<u8>, damage: f32, bits: u32 },
    SetPlayerGodmode { slot: u8, enabled: bool },
    SetPlayerNoclip { slot: u8, enabled: bool },
    SetPlayerMaxspeed { slot: u8, maxspeed: f32 },
    SetPlayerGravity { slot: u8, gravity: f32 },
    SetPlayerFootsteps { slot: u8, silent: bool },
    SetPlayerRendering { slot: u8, fx: u8, rgb: [u8; 3], mode: u8, amount: u8 },
    SetPlayerModel { slot: u8, model: String },
    ResetPlayerModel { slot: u8 },
    GiveItem { slot: u8, item: String },
    StripUserWeapons { slot: u8 },
    SetWeaponClip { slot: u8, weapon_id: u8, clip: i32 },
    SetUserBpAmmo { slot: u8, weapon_id: u8, amount: i32 },
    SetUserDefuser { slot: u8, has: bool },
    SetUserNvg { slot: u8, has: bool },
    DropSlot { slot: u8, inv_slot: u8 },
    // --- ABI v2 Custom Weapons, VFX, Menus, HUD/DHUD/MOTD, RPC Events & Webhooks ---
    RegisterCustomWeapon(CustomWeaponSpec),
    GiveCustomWeapon { slot: u8, id_name: String },
    EmitVfx(VfxOp),
    MenuDisplay(MenuDisplaySpec),
    HudMessage(HudMessageSpec),
    DhudMessage(DhudMessageSpec),
    ShowMotd(ShowMotdSpec),
    ScreenFade(ScreenFadeSpec),
    ScreenShake(ScreenShakeSpec),
    BarTime(BarTimeSpec),
    RpcEmitEvent(RpcEmitEventSpec),
    HttpPostJson(HttpPostJsonSpec),
}

/// Per-store host state accessible to `"cs16_mod"` and `"wasi_snapshot_preview1"` native imports.
pub struct HostCallState {
    pub mod_id: String,
    pub permissions: Permissions,
    pub http_hosts: Vec<String>,
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
    pub override_i32_2: Option<i32>,
    pub override_f32: Option<f32>,
    pub override_str: Option<String>,
    pub rpc_return_str: Option<String>,
    pub services: SharedPlatformServices,
    pub rpc_dispatcher: Option<Arc<dyn RpcDispatcher>>,
    pub wasi_logs: Vec<String>,
    pub wasi_rng: u64,
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

    pub fn next_wasi_rand_byte(&mut self) -> u8 {
        let mut x = self.wasi_rng;
        if x == 0 {
            x = 0x9E37_79B9_7F4A_7C15;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.wasi_rng = x;
        (x & 0xFF) as u8
    }
}

pub fn guest_memory(caller: &Caller<'_, HostCallState>) -> Option<Memory> {
    caller.get_export("memory").and_then(Extern::into_memory)
}

pub fn read_guest_bytes(
    caller: &Caller<'_, HostCallState>,
    ptr: i32,
    len: i32,
    max_len: usize,
) -> Option<Vec<u8>> {
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

pub fn read_guest_u32(caller: &Caller<'_, HostCallState>, ptr: i32) -> Option<u32> {
    let raw = read_guest_bytes(caller, ptr, 4, 4)?;
    Some(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

pub fn write_guest_bytes(
    caller: &mut Caller<'_, HostCallState>,
    ptr: i32,
    cap: i32,
    src: &[u8],
) -> i32 {
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

pub fn write_guest_u32(caller: &mut Caller<'_, HostCallState>, ptr: i32, val: u32) -> bool {
    write_guest_bytes(caller, ptr, 4, &val.to_le_bytes()) == 4
}

pub fn write_guest_u64(caller: &mut Caller<'_, HostCallState>, ptr: i32, val: u64) -> bool {
    write_guest_bytes(caller, ptr, 8, &val.to_le_bytes()) == 8
}

pub fn write_guest_f32_array3(
    caller: &mut Caller<'_, HostCallState>,
    ptr: i32,
    vals: [f32; 3],
) -> i32 {
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

/// Parses a key=value newline-delimited [`CustomWeaponSpec`] from guest memory.
pub fn parse_custom_weapon_spec(raw: &str) -> Option<CustomWeaponSpec> {
    let mut spec = CustomWeaponSpec::default();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((k, v)) = trimmed.split_once('=') else {
            continue;
        };
        let key = k.trim().to_ascii_lowercase();
        let val = v.trim();
        match key.as_str() {
            "id" | "id_name" => spec.id_name = val.to_string(),
            "base_weapon" | "base" => {
                if let Ok(n) = val.parse::<u8>() {
                    spec.base_weapon = n;
                }
            }
            "display_name" | "name" => spec.display_name = val.to_string(),
            "v_model" => spec.v_model = (!val.is_empty()).then(|| val.to_string()),
            "p_model" => spec.p_model = (!val.is_empty()).then(|| val.to_string()),
            "w_model" => spec.w_model = (!val.is_empty()).then(|| val.to_string()),
            "fire_sound" | "sound" => spec.fire_sound = (!val.is_empty()).then(|| val.to_string()),
            "damage_mult" => {
                if let Ok(f) = val.parse::<f32>()
                    && f.is_finite()
                {
                    spec.damage_mult = f;
                }
            }
            "clip_size" | "clip" => {
                if let Ok(n) = val.parse::<i32>() {
                    spec.clip_size = Some(n.max(0));
                }
            }
            "deploy_speed_mult" => {
                if let Ok(f) = val.parse::<f32>()
                    && f.is_finite()
                {
                    spec.deploy_speed_mult = f;
                }
            }
            "recoil_mult" => {
                if let Ok(f) = val.parse::<f32>()
                    && f.is_finite()
                {
                    spec.recoil_mult = f;
                }
            }
            "knockback" => {
                if let Ok(f) = val.parse::<f32>()
                    && f.is_finite()
                {
                    spec.knockback = f;
                }
            }
            "cost" => {
                if let Ok(n) = val.parse::<i32>() {
                    spec.cost = n.max(0);
                }
            }
            _ => {}
        }
    }
    if spec.id_name.is_empty() {
        return None;
    }
    if spec.display_name.is_empty() {
        spec.display_name = spec.id_name.clone();
    }
    Some(spec)
}

/// Parses `\n`-delimited `key=val` (or positional) arguments for `lang_tr`.
pub fn parse_lang_args(raw: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (idx, line) in raw.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.push((k.trim().to_string(), v.to_string()));
        } else {
            out.push((idx.to_string(), line.to_string()));
        }
    }
    out
}
