//! Server-side mod host ([`ServerModHost`]) and [`ModdedGame<G>`] decorator wrapping any
//! [`cs_engine::GameDll`] with ReAPI-style pre/post hook chains, per-mod isolated storage, timers,
//! hot reload, and capability-gated client mod auto-download.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use cs_engine::{EngineApi, EdictId, FullPackCtx, GameDll, MsgDest};
use cs_mod_api::{
    CMD_MOD_REQUEST, ClientModEntry, HookAction, HookId, HookPhase, ModChunkPacket,
    ModManifestPacket, USERMSG_MOD_CHUNK, USERMSG_MOD_EVENT, USERMSG_MOD_MANIFEST,
};
use cs_platform::{ConfigStorage, MemoryStorage};
use cs_proto48::delta::{DeltaMarks, GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd, GoldSrcWeaponData};
use cs_proto48::usermsg::{CsUserMsg, UserMsgRegistry};
use glam::Vec3;
use tracing::warn;

use crate::hookchain::HookTable;
use crate::natives::{
    ActiveEvent, ClientSnap, EngineOp, EngineSnapshot, EntitySnap,
};
use crate::package::{LoadedPackage, PackageError, discover_packages};
use crate::runtime::{ModCallOutcome, ModExecContext, ModInstance, ModRuntime, WasmiRuntime};

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
    instance: Option<Box<dyn ModInstance>>,
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

/// Coordinates sandboxed server WASM mods, hook chains, timers, storage, and client catalogs.
pub struct ServerModHost {
    runtime: Box<dyn ModRuntime>,
    mods: BTreeMap<String, HostedMod>,
    load_order: Vec<String>,
    hooks: HookTable,
    timers: Vec<ServerTimer>,
    next_timer_id: u32,
    storage: Box<dyn ConfigStorage>,
    mods_dir: Option<PathBuf>,
    custom_usermsgs: BTreeSet<String>,
    pending_client_requests: Vec<(u8, u8, u32)>,
}

impl Default for ServerModHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerModHost {
    pub fn new() -> Self {
        Self::with_runtime(Box::new(WasmiRuntime::new()), Box::new(MemoryStorage::default()))
    }

    pub fn with_runtime(runtime: Box<dyn ModRuntime>, storage: Box<dyn ConfigStorage>) -> Self {
        Self {
            runtime,
            mods: BTreeMap::new(),
            load_order: Vec::new(),
            hooks: HookTable::default(),
            timers: Vec::new(),
            next_timer_id: 1,
            storage,
            mods_dir: None,
            custom_usermsgs: BTreeSet::new(),
            pending_client_requests: Vec::new(),
        }
    }

    pub fn set_mods_dir(&mut self, dir: impl Into<PathBuf>) {
        self.mods_dir = Some(dir.into());
    }

    /// Discovers and loads all mods from `mods_dir` in dependency order.
    pub fn load_dir(&mut self, mods_dir: impl Into<PathBuf>) -> Result<usize, PackageError> {
        let dir = mods_dir.into();
        self.mods_dir = Some(dir.clone());
        let packages = discover_packages(&dir)?;
        let mut count = 0usize;
        for pkg in packages {
            if let Err(err) = self.load_package(pkg) {
                warn!("failed to load mod package: {err}");
            } else {
                count += 1;
            }
        }
        Ok(count)
    }

    /// Loads or replaces a single mod package (`cs_mod_init` runs immediately).
    pub fn load_package(&mut self, pkg: LoadedPackage) -> Result<(), String> {
        let id = pkg.manifest.id.clone();
        self.unload_mod(&id);

        for um in &pkg.manifest.custom_usermsgs {
            self.custom_usermsgs.insert(um.clone());
        }

        let mut instance = match &pkg.server_wasm {
            Some(wasm) => Some(
                self.runtime
                    .instantiate(&id, wasm, pkg.manifest.permissions, pkg.manifest.max_memory_bytes)
                    .map_err(|e| e.to_string())?,
            ),
            None => None,
        };

        let mut storage_cache = BTreeMap::new();
        let mut hooks_registered = Vec::new();
        let mut ops = Vec::new();
        let mut init_fuel = 0u64;

        // Hydrate known persisted storage keys if any were recorded earlier.
        let index_key = format!("mod:{id}:__keys");
        if let Some(key_list) = self.storage.load(&index_key) {
            for k in key_list.lines().filter(|l| !l.is_empty()) {
                let full = format!("mod:{id}:{k}");
                if let Some(val) = self.storage.load(&full) {
                    storage_cache.insert(k.to_string(), val);
                }
            }
        }

        if let Some(ref mut inst) = instance {
            let snap = EngineSnapshot::default();
            let ev = ActiveEvent::default();
            let ctx = ModExecContext {
                snapshot: &snap,
                event: &ev,
                storage: &storage_cache,
                fuel_budget: pkg.manifest.fuel_per_frame,
            };
            match inst.call_init(&ctx) {
                Ok(out) => {
                    hooks_registered = out.hooks_registered;
                    ops = out.ops;
                    init_fuel = out.fuel_used;
                }
                Err(err) => {
                    let msg = err.to_string();
                    self.mods.insert(
                        id.clone(),
                        HostedMod {
                            package: pkg,
                            instance: None,
                            status: ModStatus::Faulted(msg.clone()),
                            storage_cache,
                            registered_commands: BTreeSet::new(),
                            total_fuel_used: 0,
                        },
                    );
                    self.load_order.push(id);
                    return Err(msg);
                }
            }
        }

        for h in hooks_registered {
            self.hooks.register(&id, h.hook, h.phase, h.priority);
        }
        let mut registered_commands = BTreeSet::new();
        for op in ops {
            if let EngineOp::RegisterCommand { name } = op {
                registered_commands.insert(name);
            }
        }

        self.mods.insert(
            id.clone(),
            HostedMod {
                package: pkg,
                instance,
                status: ModStatus::Active,
                storage_cache,
                registered_commands,
                total_fuel_used: init_fuel,
            },
        );
        self.load_order.push(id);
        Ok(())
    }

    pub fn unload_mod(&mut self, mod_id: &str) -> bool {
        self.hooks.unregister_mod(mod_id);
        self.timers.retain(|t| t.mod_id != mod_id);
        self.load_order.retain(|id| id != mod_id);
        if let Some(m) = self.mods.get_mut(mod_id) {
            m.instance = None;
            m.status = ModStatus::Unloaded;
            true
        } else {
            false
        }
    }

    /// Hot-reloads one mod from disk (`source_path` or `<mods_dir>/<id>`).
    pub fn reload_mod(&mut self, mod_id: &str) -> Result<(), String> {
        let src = self
            .mods
            .get(mod_id)
            .and_then(|m| m.package.source_path.clone())
            .or_else(|| self.mods_dir.as_ref().map(|d| d.join(mod_id)))
            .ok_or_else(|| format!("unknown source path for mod `{mod_id}`"))?;

        let pkg = if src.is_dir() {
            crate::package::load_package_dir(&src).map_err(|e| e.to_string())?
        } else {
            let bytes = std::fs::read(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            crate::package::load_package_csmod(&bytes, Some(src)).map_err(|e| e.to_string())?
        };
        self.load_package(pkg)
    }

    pub fn list_mods(&self) -> Vec<ModInfo> {
        self.load_order
            .iter()
            .filter_map(|id| self.mods.get(id))
            .map(|m| ModInfo {
                id: m.package.manifest.id.clone(),
                version: m.package.manifest.version.clone(),
                status: m.status.clone(),
                total_fuel_used: m.total_fuel_used,
            })
            .collect()
    }

    pub fn declared_custom_usermsgs(&self) -> impl Iterator<Item = &str> {
        self.custom_usermsgs.iter().map(String::as_str)
    }

    /// Ordered client-downloadable mod packages (having client WASM and/or assets).
    pub fn client_catalog(&self) -> Vec<(ClientModEntry, &[u8])> {
        let mut out = Vec::new();
        for id in &self.load_order {
            let Some(m) = self.mods.get(id) else {
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
                    bundle.as_slice(),
                ));
            }
        }
        out
    }

    fn persist_storage_writes(&mut self, mod_id: &str, writes: &[(String, Option<String>)]) {
        if writes.is_empty() {
            return;
        }
        let Some(m) = self.mods.get_mut(mod_id) else {
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
                    let _ = self.storage.save(&full, v);
                }
                None => {
                    m.storage_cache.remove(k);
                    let _ = self.storage.remove(&full);
                }
            }
        }
        let index_val: String = m
            .storage_cache
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        let _ = self
            .storage
            .save(&format!("mod:{mod_id}:__keys"), &index_val);
    }

    fn schedule_timers(&mut self, mod_id: &str, now_secs: f64, out: &ModCallOutcome) {
        if !out.cancel_timers.is_empty() {
            self.timers
                .retain(|t| !(t.mod_id == mod_id && out.cancel_timers.contains(&t.id)));
        }
        for req in &out.timers {
            if self.timers.len() >= 256 {
                break;
            }
            let delay = (f64::from(req.delay_ms) / 1000.0).max(0.001);
            let id = self.next_timer_id;
            self.next_timer_id = self.next_timer_id.wrapping_add(1).max(1);
            self.timers.push(ServerTimer {
                id,
                mod_id: mod_id.to_string(),
                due_secs: now_secs + delay,
                interval_secs: req.repeat.then_some(delay),
                cb_id: req.cb_id,
                arg: req.arg,
            });
        }
    }

    fn fault_mod(&mut self, mod_id: &str, reason: String) {
        warn!(mod_id, %reason, "mod faulted; unloading");
        self.hooks.unregister_mod(mod_id);
        self.timers.retain(|t| t.mod_id != mod_id);
        if let Some(m) = self.mods.get_mut(mod_id) {
            m.instance = None;
            m.status = ModStatus::Faulted(reason);
        }
    }

    /// Dispatches a hook chain (`HookId`, `HookPhase`), replaying queued `EngineOp`s cleanly.
    pub fn dispatch_hook(
        &mut self,
        eng: &mut EngineApi<'_>,
        hook: HookId,
        phase: HookPhase,
        event: &ActiveEvent,
    ) -> HookDispatchResult {
        self.dispatch_hook_depth(eng, hook, phase, event, 0)
    }

    fn dispatch_hook_depth(
        &mut self,
        eng: &mut EngineApi<'_>,
        hook: HookId,
        phase: HookPhase,
        event: &ActiveEvent,
        depth: u8,
    ) -> HookDispatchResult {
        let subscribers = self.hooks.subscribers(hook, phase);
        if subscribers.is_empty() {
            return HookDispatchResult::default();
        }
        let mut combined = HookDispatchResult::default();
        let snapshot = capture_snapshot(eng);
        let now_secs = eng.time();

        for mod_id in subscribers {
            let outcome = {
                let Some(m) = self.mods.get_mut(&mod_id) else {
                    continue;
                };
                let Some(ref mut inst) = m.instance else {
                    continue;
                };
                let ctx = ModExecContext {
                    snapshot: &snapshot,
                    event,
                    storage: &m.storage_cache,
                    fuel_budget: m.package.manifest.fuel_per_frame,
                };
                match inst.call_hook(hook, phase, &ctx) {
                    Ok(o) => {
                        m.total_fuel_used = m.total_fuel_used.saturating_add(o.fuel_used);
                        o
                    }
                    Err(err) => {
                        self.fault_mod(&mod_id, err.to_string());
                        continue;
                    }
                }
            };

            self.persist_storage_writes(&mod_id, &outcome.storage_writes);
            self.schedule_timers(&mod_id, now_secs, &outcome);
            self.apply_ops(eng, &mod_id, outcome.ops, depth);

            if outcome.action == HookAction::Override {
                combined.action = HookAction::Override;
                if outcome.override_i32.is_some() {
                    combined.override_i32 = outcome.override_i32;
                }
                if outcome.override_str.is_some() {
                    combined.override_str = outcome.override_str;
                }
            } else if outcome.action == HookAction::Supercede {
                combined.action = HookAction::Supercede;
                if outcome.override_i32.is_some() {
                    combined.override_i32 = outcome.override_i32;
                }
                if outcome.override_str.is_some() {
                    combined.override_str = outcome.override_str;
                }
                break;
            } else if outcome.action == HookAction::Handled && combined.action == HookAction::Continue {
                combined.action = HookAction::Handled;
            }
        }
        combined
    }

    pub fn tick_timers(&mut self, eng: &mut EngineApi<'_>) {
        let now = eng.time();
        let mut due = Vec::new();
        let mut keep = Vec::with_capacity(self.timers.len());
        for mut t in self.timers.drain(..) {
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
        self.timers = keep;
        if due.is_empty() {
            return;
        }

        let snapshot = capture_snapshot(eng);
        let ev = ActiveEvent::default();
        for (mod_id, cb_id, arg) in due {
            let outcome = {
                let Some(m) = self.mods.get_mut(&mod_id) else {
                    continue;
                };
                let Some(ref mut inst) = m.instance else {
                    continue;
                };
                let ctx = ModExecContext {
                    snapshot: &snapshot,
                    event: &ev,
                    storage: &m.storage_cache,
                    fuel_budget: m.package.manifest.fuel_per_frame,
                };
                match inst.call_timer(cb_id, arg, &ctx) {
                    Ok(o) => {
                        m.total_fuel_used = m.total_fuel_used.saturating_add(o.fuel_used);
                        o
                    }
                    Err(err) => {
                        self.fault_mod(&mod_id, err.to_string());
                        continue;
                    }
                }
            };
            self.persist_storage_writes(&mod_id, &outcome.storage_writes);
            self.schedule_timers(&mod_id, now, &outcome);
            self.apply_ops(eng, &mod_id, outcome.ops, 0);
        }
    }

    fn apply_ops(&mut self, eng: &mut EngineApi<'_>, mod_id: &str, ops: Vec<EngineOp>, depth: u8) {
        for op in ops {
            match op {
                EngineOp::RegisterCommand { name } => {
                    if let Some(m) = self.mods.get_mut(mod_id) {
                        m.registered_commands.insert(name);
                    }
                }
                EngineOp::CreateEntity { classname } => {
                    if depth < 2 {
                        let ev = ActiveEvent {
                            str_args: [classname.clone(), String::new(), String::new()],
                            ..Default::default()
                        };
                        let res = self.dispatch_hook_depth(
                            eng,
                            HookId::EngCreateEntity,
                            HookPhase::Pre,
                            &ev,
                            depth + 1,
                        );
                        if res.action == HookAction::Supercede {
                            continue;
                        }
                    }
                    let created = eng.create_entity(&classname);
                    if depth < 2 {
                        let ev = ActiveEvent {
                            i32_args: [created.map_or(-1, |id| i32::from(id.index)), 0, 0, 0, 0, 0],
                            str_args: [classname, String::new(), String::new()],
                            ..Default::default()
                        };
                        let _ = self.dispatch_hook_depth(
                            eng,
                            HookId::EngCreateEntity,
                            HookPhase::Post,
                            &ev,
                            depth + 1,
                        );
                    }
                }
                EngineOp::RemoveEntity { index } => {
                    if depth < 2 {
                        let ev = ActiveEvent {
                            i32_args: [i32::from(index), 0, 0, 0, 0, 0],
                            ..Default::default()
                        };
                        let res = self.dispatch_hook_depth(
                            eng,
                            HookId::EngRemoveEntity,
                            HookPhase::Pre,
                            &ev,
                            depth + 1,
                        );
                        if res.action == HookAction::Supercede {
                            continue;
                        }
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
                    if depth < 2 {
                        let ev = ActiveEvent {
                            i32_args: [i32::from(slot), 0, 0, 0, 0, 0],
                            str_args: [text.clone(), String::new(), String::new()],
                            ..Default::default()
                        };
                        let res = self.dispatch_hook_depth(
                            eng,
                            HookId::EngClientPrint,
                            HookPhase::Pre,
                            &ev,
                            depth + 1,
                        );
                        if res.action == HookAction::Supercede {
                            continue;
                        }
                    }
                    eng.client_print(slot, &text);
                }
                EngineOp::PrintAll { text } => {
                    eng.print_all(&text);
                }
                EngineOp::CenterPrint { slot, text } => {
                    if depth < 2 {
                        let ev = ActiveEvent {
                            i32_args: [slot.map_or(-1, i32::from), 0, 0, 0, 0, 0],
                            str_args: [text.clone(), String::new(), String::new()],
                            ..Default::default()
                        };
                        let res = self.dispatch_hook_depth(
                            eng,
                            HookId::EngCenterPrint,
                            HookPhase::Pre,
                            &ev,
                            depth + 1,
                        );
                        if res.action == HookAction::Supercede {
                            continue;
                        }
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
                    if depth < 2 {
                        let ev = ActiveEvent {
                            i32_args: [slot.map_or(-1, i32::from), 0, 0, 0, 0, 0],
                            str_args: [name.clone(), String::new(), String::new()],
                            payload: data.clone(),
                            ..Default::default()
                        };
                        let res = self.dispatch_hook_depth(
                            eng,
                            HookId::EngSendUserMsg,
                            HookPhase::Pre,
                            &ev,
                            depth + 1,
                        );
                        if res.action == HookAction::Supercede {
                            continue;
                        }
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
                    if depth < 2 {
                        let ev = ActiveEvent {
                            i32_args: [i32::from(ent_index), i32::from(channel), 0, 0, 0, 0],
                            f32_args: [volume, 0.0, 0.0, 0.0],
                            str_args: [sample.clone(), String::new(), String::new()],
                            ..Default::default()
                        };
                        let res = self.dispatch_hook_depth(
                            eng,
                            HookId::EngEmitSound,
                            HookPhase::Pre,
                            &ev,
                            depth + 1,
                        );
                        if res.action == HookAction::Supercede {
                            continue;
                        }
                    }
                    if let Some(id) = resolve_edict(eng, ent_index) {
                        let _ = eng.emit_sound(id, channel, &sample, volume, 0.8, 100);
                    }
                }
                EngineOp::PrecacheSound { sample } => {
                    let _ = eng.precache_sound(&sample);
                }
                EngineOp::Kick { slot, reason } => {
                    if depth < 2 {
                        let ev = ActiveEvent {
                            i32_args: [i32::from(slot), 0, 0, 0, 0, 0],
                            str_args: [reason.clone(), String::new(), String::new()],
                            ..Default::default()
                        };
                        let res = self.dispatch_hook_depth(
                            eng,
                            HookId::EngKick,
                            HookPhase::Pre,
                            &ev,
                            depth + 1,
                        );
                        if res.action == HookAction::Supercede {
                            continue;
                        }
                    }
                    eng.kick(slot, &reason);
                }
                EngineOp::ChangeLevel { map } => {
                    if depth < 2 {
                        let ev = ActiveEvent {
                            str_args: [map.clone(), String::new(), String::new()],
                            ..Default::default()
                        };
                        let res = self.dispatch_hook_depth(
                            eng,
                            HookId::EngChangeLevel,
                            HookPhase::Pre,
                            &ev,
                            depth + 1,
                        );
                        if res.action == HookAction::Supercede {
                            continue;
                        }
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
            }
        }
    }
}

#[derive(Debug)]
pub struct HookDispatchResult {
    pub action: HookAction,
    pub override_i32: Option<i32>,
    pub override_str: Option<String>,
}

impl Default for HookDispatchResult {
    fn default() -> Self {
        Self {
            action: HookAction::Continue,
            override_i32: None,
            override_str: None,
        }
    }
}

/// [`GameDll`] wrapper weaving [`ServerModHost`] pre/post hookchains around any inner game.
pub struct ModdedGame<G: GameDll> {
    pub inner: G,
    pub host: ServerModHost,
    was_client_command_claimed: bool,
    pending_catalog_slots: BTreeSet<u8>,
}

impl<G: GameDll> ModdedGame<G> {
    pub fn new(inner: G, host: ServerModHost) -> Self {
        Self {
            inner,
            host,
            was_client_command_claimed: false,
            pending_catalog_slots: BTreeSet::new(),
        }
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
        let reqs = std::mem::take(&mut self.host.pending_client_requests);
        if reqs.is_empty() {
            return;
        }
        let catalog: Vec<(Vec<u8>,)> = self
            .host
            .client_catalog()
            .into_iter()
            .map(|(_, bytes)| (bytes.to_vec(),))
            .collect();
        for (slot, mod_index, offset) in reqs {
            if !eng.client_supports_mods(slot) {
                continue;
            }
            let Some((bundle,)) = catalog.get(usize::from(mod_index)) else {
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

impl<G: GameDll> GameDll for ModdedGame<G> {
    fn usermsgs(&self) -> UserMsgRegistry {
        let mut reg = self.inner.usermsgs();
        let _ = reg.register_next(-1, USERMSG_MOD_MANIFEST);
        let _ = reg.register_next(-1, USERMSG_MOD_CHUNK);
        let _ = reg.register_next(-1, USERMSG_MOD_EVENT);
        for name in self.host.declared_custom_usermsgs() {
            let _ = reg.register_next(-1, name);
        }
        reg
    }

    fn server_activate(&mut self, eng: &mut EngineApi<'_>) {
        let ev = ActiveEvent {
            i32_args: [i32::from(eng.max_clients()), 0, 0, 0, 0, 0],
            str_args: [eng.map_name().to_string(), String::new(), String::new()],
            ..Default::default()
        };
        let pre = self
            .host
            .dispatch_hook(eng, HookId::ServerActivate, HookPhase::Pre, &ev);
        if pre.action != HookAction::Supercede {
            self.inner.server_activate(eng);
        }
        let _ = self
            .host
            .dispatch_hook(eng, HookId::ServerActivate, HookPhase::Post, &ev);
        let _ = self
            .host
            .dispatch_hook(eng, HookId::RoundStart, HookPhase::Post, &ev);
    }

    fn server_deactivate(&mut self, eng: &mut EngineApi<'_>) {
        let ev = ActiveEvent::default();
        let pre = self
            .host
            .dispatch_hook(eng, HookId::ServerDeactivate, HookPhase::Pre, &ev);
        if pre.action != HookAction::Supercede {
            self.inner.server_deactivate(eng);
        }
        let _ = self
            .host
            .dispatch_hook(eng, HookId::ServerDeactivate, HookPhase::Post, &ev);
    }

    fn spawn_entity(
        &mut self,
        eng: &mut EngineApi<'_>,
        id: EdictId,
        classname: &str,
        keys: &[(String, String)],
    ) -> bool {
        let ev = ActiveEvent {
            i32_args: [i32::from(id.index), 0, 0, 0, 0, 0],
            str_args: [classname.to_string(), String::new(), String::new()],
            ..Default::default()
        };
        let pre = self
            .host
            .dispatch_hook(eng, HookId::SpawnEntity, HookPhase::Pre, &ev);
        let keep = if pre.action == HookAction::Supercede {
            pre.override_i32.map_or(false, |v| v != 0)
        } else {
            self.inner.spawn_entity(eng, id, classname, keys)
        };
        let _ = self
            .host
            .dispatch_hook(eng, HookId::SpawnEntity, HookPhase::Post, &ev);
        if pre.action == HookAction::Override
            && let Some(v) = pre.override_i32
        {
            return v != 0;
        }
        keep
    }

    fn client_connect(&mut self, eng: &mut EngineApi<'_>, slot: u8) -> Result<(), String> {
        let ev = ActiveEvent {
            i32_args: [i32::from(slot), 0, 0, 0, 0, 0],
            str_args: [
                eng.client_key(slot, "name").unwrap_or("").to_string(),
                String::new(),
                String::new(),
            ],
            ..Default::default()
        };
        let pre = self
            .host
            .dispatch_hook(eng, HookId::ClientConnect, HookPhase::Pre, &ev);
        let res = if pre.action == HookAction::Supercede {
            Err("rejected by server mod".to_string())
        } else {
            self.inner.client_connect(eng, slot)
        };
        let _ = self
            .host
            .dispatch_hook(eng, HookId::ClientConnect, HookPhase::Post, &ev);
        res
    }

    fn client_put_in_server(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        let ev = ActiveEvent {
            i32_args: [i32::from(slot), i32::from(slot) + 1, 0, 0, 0, 0],
            ..Default::default()
        };
        let pre = self
            .host
            .dispatch_hook(eng, HookId::ClientPutInServer, HookPhase::Pre, &ev);
        if pre.action != HookAction::Supercede {
            self.inner.client_put_in_server(eng, slot);
        }
        let _ = self
            .host
            .dispatch_hook(eng, HookId::ClientPutInServer, HookPhase::Post, &ev);
        if eng.client_supports_mods(slot) {
            self.pending_catalog_slots.insert(slot);
        }
    }

    fn client_disconnect(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        self.pending_catalog_slots.remove(&slot);
        let ev = ActiveEvent {
            i32_args: [i32::from(slot), 0, 0, 0, 0, 0],
            ..Default::default()
        };
        let pre = self
            .host
            .dispatch_hook(eng, HookId::ClientDisconnect, HookPhase::Pre, &ev);
        if pre.action != HookAction::Supercede {
            self.inner.client_disconnect(eng, slot);
        }
        let _ = self
            .host
            .dispatch_hook(eng, HookId::ClientDisconnect, HookPhase::Post, &ev);
    }

    fn client_command(&mut self, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
        if let Some(first) = argv.first()
            && first == CMD_MOD_REQUEST
            && argv.len() >= 3
            && let (Ok(mod_idx), Ok(offset)) = (argv[1].parse::<u8>(), argv[2].parse::<u32>())
        {
            self.host
                .pending_client_requests
                .push((slot, mod_idx, offset));
            self.flush_client_chunk_requests(eng);
            return;
        }

        let cmd = argv.first().cloned().unwrap_or_default();
        let arg1 = argv.get(1).cloned().unwrap_or_default();
        let ev = ActiveEvent {
            i32_args: [i32::from(slot), argv.len() as i32, 0, 0, 0, 0],
            str_args: [cmd.clone(), arg1, argv.join(" ")],
            ..Default::default()
        };
        let pre = self
            .host
            .dispatch_hook(eng, HookId::ClientCommand, HookPhase::Pre, &ev);
        self.was_client_command_claimed = self
            .host
            .mods
            .values()
            .any(|m| m.status == ModStatus::Active && m.registered_commands.contains(&cmd));
        if pre.action != HookAction::Supercede && pre.action != HookAction::Handled {
            self.inner.client_command(eng, slot, argv);
        }
        let _ = self
            .host
            .dispatch_hook(eng, HookId::ClientCommand, HookPhase::Post, &ev);
    }

    fn client_userinfo_changed(&mut self, eng: &mut EngineApi<'_>, slot: u8) {
        let ev = ActiveEvent {
            i32_args: [i32::from(slot), 0, 0, 0, 0, 0],
            ..Default::default()
        };
        let pre = self
            .host
            .dispatch_hook(eng, HookId::ClientUserinfoChanged, HookPhase::Pre, &ev);
        if pre.action != HookAction::Supercede {
            self.inner.client_userinfo_changed(eng, slot);
        }
        let _ = self
            .host
            .dispatch_hook(eng, HookId::ClientUserinfoChanged, HookPhase::Post, &ev);
    }

    fn run_usercmd(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        cmd: &GoldSrcUserCmd,
        random_seed: u32,
    ) {
        let ev = ActiveEvent {
            i32_args: [i32::from(slot), i32::from(cmd.buttons), i32::from(cmd.msec), 0, 0, 0],
            ..Default::default()
        };
        let pre = self
            .host
            .dispatch_hook(eng, HookId::RunUsercmd, HookPhase::Pre, &ev);
        if pre.action != HookAction::Supercede {
            self.inner.run_usercmd(eng, slot, cmd, random_seed);
        }
        let _ = self
            .host
            .dispatch_hook(eng, HookId::RunUsercmd, HookPhase::Post, &ev);
    }

    fn post_usercmd(&mut self, eng: &mut EngineApi<'_>, slot: u8, cmd: &GoldSrcUserCmd) {
        let ev = ActiveEvent {
            i32_args: [i32::from(slot), i32::from(cmd.buttons), 0, 0, 0, 0],
            ..Default::default()
        };
        let pre = self
            .host
            .dispatch_hook(eng, HookId::PostUsercmd, HookPhase::Pre, &ev);
        if pre.action != HookAction::Supercede {
            self.inner.post_usercmd(eng, slot, cmd);
        }
        let _ = self
            .host
            .dispatch_hook(eng, HookId::PostUsercmd, HookPhase::Post, &ev);
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
        self.host.tick_timers(eng);
        self.flush_client_chunk_requests(eng);

        let ev = ActiveEvent {
            f32_args: [frametime, 0.0, 0.0, 0.0],
            ..Default::default()
        };
        let pre = self
            .host
            .dispatch_hook(eng, HookId::StartFrame, HookPhase::Pre, &ev);
        if pre.action != HookAction::Supercede {
            self.inner.start_frame(eng, frametime);
        }
        let _ = self
            .host
            .dispatch_hook(eng, HookId::StartFrame, HookPhase::Post, &ev);
    }

    fn think(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        let ev = ActiveEvent {
            i32_args: [i32::from(id.index), 0, 0, 0, 0, 0],
            ..Default::default()
        };
        let pre = self.host.dispatch_hook(eng, HookId::Think, HookPhase::Pre, &ev);
        if pre.action != HookAction::Supercede {
            self.inner.think(eng, id);
        }
        let _ = self
            .host
            .dispatch_hook(eng, HookId::Think, HookPhase::Post, &ev);
    }

    fn touch(&mut self, eng: &mut EngineApi<'_>, toucher: EdictId, other: EdictId) {
        let ev = ActiveEvent {
            i32_args: [i32::from(toucher.index), i32::from(other.index), 0, 0, 0, 0],
            ..Default::default()
        };
        let pre = self.host.dispatch_hook(eng, HookId::Touch, HookPhase::Pre, &ev);
        if pre.action != HookAction::Supercede {
            self.inner.touch(eng, toucher, other);
        }
        let _ = self
            .host
            .dispatch_hook(eng, HookId::Touch, HookPhase::Post, &ev);
    }

    fn blocked(&mut self, eng: &mut EngineApi<'_>, pusher: EdictId, other: EdictId) {
        let ev = ActiveEvent {
            i32_args: [i32::from(pusher.index), i32::from(other.index), 0, 0, 0, 0],
            ..Default::default()
        };
        let pre = self
            .host
            .dispatch_hook(eng, HookId::Blocked, HookPhase::Pre, &ev);
        if pre.action != HookAction::Supercede {
            self.inner.blocked(eng, pusher, other);
        }
        let _ = self
            .host
            .dispatch_hook(eng, HookId::Blocked, HookPhase::Post, &ev);
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

        let ev = ActiveEvent {
            str_args: [cmd.to_string(), argv.get(1).cloned().unwrap_or_default(), argv.join(" ")],
            ..Default::default()
        };
        let pre = self
            .host
            .dispatch_hook(eng, HookId::ServerCommand, HookPhase::Pre, &ev);
        if pre.action == HookAction::Supercede || pre.action == HookAction::Handled {
            return Some(pre.override_str.unwrap_or_default());
        }
        let inner_res = self.inner.server_command(eng, argv);
        let _ = self
            .host
            .dispatch_hook(eng, HookId::ServerCommand, HookPhase::Post, &ev);
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

fn capture_snapshot(eng: &EngineApi<'_>) -> EngineSnapshot {
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

    let mut clients = Vec::new();
    for slot in 0..eng.max_clients() {
        let active = eng.is_client_active(slot);
        let name = eng.client_key(slot, "name").unwrap_or("").to_string();
        if active || !name.is_empty() {
            clients.push(ClientSnap {
                slot,
                active,
                supports_mods: eng.client_supports_mods(slot),
                name,
            });
        }
    }

    let mut cvars = BTreeMap::new();
    for name in [
        "mp_roundtime",
        "mp_freezetime",
        "mp_timelimit",
        "mp_friendlyfire",
        "sv_gravity",
        "hostname",
        "bot_quota",
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
