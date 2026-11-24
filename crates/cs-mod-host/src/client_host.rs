//! Client-side sandboxed WASM mod host ([`ClientModHost`]) running custom HUD draw commands,
//! custom usermsg listeners, and client input hooks.

use std::collections::BTreeMap;

use cs_mod_api::{HookAction, HudDrawCmd, ModManifest};
use tracing::warn;

use crate::natives::{ActiveEvent, EngineOp, EngineSnapshot};
use crate::runtime::{ModExecContext, ModInstance, ModRuntime, WasmiRuntime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientModStatus {
    Active,
    Faulted(String),
}

struct ClientMod {
    manifest: ModManifest,
    instance: Option<Box<dyn ModInstance>>,
    status: ClientModStatus,
    storage: BTreeMap<String, String>,
    last_hud_cmds: Vec<HudDrawCmd>,
}

/// Runs approved client-side WASM mods safely inside `cs-client` and `cs-web`.
pub struct ClientModHost {
    runtime: Box<dyn ModRuntime>,
    mods: BTreeMap<String, ClientMod>,
    order: Vec<String>,
    pending_execs: Vec<String>,
}

impl Default for ClientModHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientModHost {
    pub fn new() -> Self {
        Self {
            runtime: Box::new(WasmiRuntime::new()),
            mods: BTreeMap::new(),
            order: Vec::new(),
            pending_execs: Vec::new(),
        }
    }

    pub fn load_client_mod(&mut self, manifest: ModManifest, wasm_or_wat: &[u8]) -> Result<(), String> {
        let id = manifest.id.clone();
        self.unload_mod(&id);
        let mut instance = self
            .runtime
            .instantiate(&id, wasm_or_wat, manifest.permissions, manifest.max_memory_bytes)
            .map_err(|e| e.to_string())?;
        let snap = EngineSnapshot::default();
        let ev = ActiveEvent::default();
        let storage = BTreeMap::new();
        let ctx = ModExecContext {
            snapshot: &snap,
            event: &ev,
            storage: &storage,
            fuel_budget: manifest.fuel_per_frame,
        };
        instance.call_init(&ctx).map_err(|e| e.to_string())?;

        self.mods.insert(
            id.clone(),
            ClientMod {
                manifest,
                instance: Some(instance),
                status: ClientModStatus::Active,
                storage,
                last_hud_cmds: Vec::new(),
            },
        );
        self.order.push(id);
        Ok(())
    }

    pub fn unload_mod(&mut self, id: &str) -> bool {
        self.order.retain(|x| x != id);
        self.mods.remove(id).is_some()
    }

    pub fn status(&self, id: &str) -> Option<&ClientModStatus> {
        self.mods.get(id).map(|m| &m.status)
    }

    pub fn on_usermsg(&mut self, msg_name: &str, payload: &[u8]) {
        let snap = EngineSnapshot::default();
        let ev = ActiveEvent {
            str_args: [msg_name.to_string(), String::new(), String::new()],
            payload: payload.to_vec(),
            ..Default::default()
        };
        let ids = self.order.clone();
        for id in ids {
            let res = {
                let Some(m) = self.mods.get_mut(&id) else {
                    continue;
                };
                let Some(ref mut inst) = m.instance else {
                    continue;
                };
                let ctx = ModExecContext {
                    snapshot: &snap,
                    event: &ev,
                    storage: &m.storage,
                    fuel_budget: m.manifest.fuel_per_frame,
                };
                inst.call_client_usermsg(&ctx)
            };
            match res {
                Ok(out) => {
                    if let Some(m) = self.mods.get_mut(&id) {
                        for (k, v) in out.storage_writes {
                            match v {
                                Some(val) => {
                                    m.storage.insert(k, val);
                                }
                                None => {
                                    m.storage.remove(&k);
                                }
                            }
                        }
                        for op in out.ops {
                            if let EngineOp::ClientExec { line } = op {
                                self.pending_execs.push(line);
                            }
                        }
                    }
                }
                Err(err) => self.fault_mod(&id, err.to_string()),
            }
        }
    }

    pub fn on_input(&mut self, key: i32, pressed: bool) -> HookAction {
        let snap = EngineSnapshot::default();
        let ev = ActiveEvent::default();
        let ids = self.order.clone();
        let mut final_action = HookAction::Continue;
        for id in ids {
            let res = {
                let Some(m) = self.mods.get_mut(&id) else {
                    continue;
                };
                let Some(ref mut inst) = m.instance else {
                    continue;
                };
                let ctx = ModExecContext {
                    snapshot: &snap,
                    event: &ev,
                    storage: &m.storage,
                    fuel_budget: m.manifest.fuel_per_frame,
                };
                inst.call_client_input(key, pressed, &ctx)
            };
            match res {
                Ok(out) => {
                    for op in out.ops {
                        if let EngineOp::ClientExec { line } = op {
                            self.pending_execs.push(line);
                        }
                    }
                    if out.action == HookAction::Supercede {
                        return HookAction::Supercede;
                    }
                    if out.action == HookAction::Handled {
                        final_action = HookAction::Handled;
                    }
                }
                Err(err) => self.fault_mod(&id, err.to_string()),
            }
        }
        final_action
    }

    /// Runs `cs_mod_client_hud` across all active client mods and returns combined draw commands.
    pub fn render_hud(
        &mut self,
        dt_ms: i32,
        health: i32,
        armor: i32,
        money: i32,
    ) -> Vec<HudDrawCmd> {
        let snap = EngineSnapshot {
            client_health: health,
            client_armor: armor,
            client_money: money,
            ..Default::default()
        };
        let ev = ActiveEvent::default();
        let mut all_cmds = Vec::new();
        let ids = self.order.clone();
        for id in ids {
            let res = {
                let Some(m) = self.mods.get_mut(&id) else {
                    continue;
                };
                let Some(ref mut inst) = m.instance else {
                    continue;
                };
                let ctx = ModExecContext {
                    snapshot: &snap,
                    event: &ev,
                    storage: &m.storage,
                    fuel_budget: m.manifest.fuel_per_frame,
                };
                inst.call_client_hud(dt_ms, &ctx)
            };
            match res {
                Ok(out) => {
                    if let Some(m) = self.mods.get_mut(&id) {
                        m.last_hud_cmds = out.hud_cmds.clone();
                    }
                    all_cmds.extend(out.hud_cmds);
                }
                Err(err) => self.fault_mod(&id, err.to_string()),
            }
        }
        all_cmds
    }

    pub fn take_execs(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_execs)
    }

    fn fault_mod(&mut self, id: &str, reason: String) {
        warn!(mod_id = id, %reason, "client mod faulted; disabling");
        if let Some(m) = self.mods.get_mut(id) {
            m.instance = None;
            m.status = ClientModStatus::Faulted(reason);
        }
    }
}
