//! AMX Mod X plugin runtime, task scheduler, forward dispatcher, and [`AmxxGame`] `GameDll`
//! decorator.

use cs_cvar::parser::parse_commands;
use cs_engine::{EdictId, EngineApi, FullPackCtx, GameDll};
use cs_proto48::delta::{DeltaMarks, GoldSrcClientData, GoldSrcEntityState, GoldSrcUserCmd, GoldSrcWeaponData};
use cs_proto48::usermsg::UserMsgRegistry;
use glam::Vec3;
use tracing::warn;

use crate::amxxfile::ParsedAmx;
use crate::natives::{
    AmxxCsHooks, AmxxHostState, AmxxNativeDispatch, HAM_IGNORED, HAM_SUPERCEDE, HamFunction,
    PLUGIN_CONTINUE, PLUGIN_HANDLED, PluginMeta, ScheduledTask,
};
use crate::vm::{AmxError, ExecEntry, PawnVm};

/// Runtime status of a loaded AMX Mod X plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStatus {
    Running,
    Paused,
    Stopped,
    BadLoad(String),
}

/// One loaded `.amxx` plugin instance with its [`PawnVm`] and lifecycle status.
#[derive(Debug, Clone)]
pub struct AmxxPlugin {
    pub name: String,
    pub status: PluginStatus,
    pub vm: PawnVm,
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
        })
    }

    pub fn is_running(&self) -> bool {
        self.status == PluginStatus::Running
    }
}

/// Coordinates all loaded `.amxx` plugins, timers, commands, forwards, and hooks.
#[derive(Debug, Default)]
pub struct AmxxRuntime {
    pub plugins: Vec<AmxxPlugin>,
    pub state: AmxxHostState,
}

impl AmxxRuntime {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
            state: AmxxHostState::new(),
        }
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
        let Some(plugin) = self.plugins.get_mut(plugin_idx) else {
            return Ok(None);
        };
        if !plugin.is_running() {
            return Ok(None);
        }
        let Some(pub_idx) = plugin.vm.find_public(func_name) else {
            return Ok(None);
        };
        let mut dispatch = AmxxNativeDispatch {
            plugin_idx,
            state: &mut self.state,
            eng,
            hooks,
        };
        let ret = plugin
            .vm
            .exec(ExecEntry::Public(pub_idx), args, &mut dispatch)?;
        Ok(Some(ret))
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

    /// Dispatches `client_damage(attacker, victim, damage, wpnindex, hitplace, TA)`.
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

    /// Dispatches `client_death(killer, victim, wpnindex, hitplace, TK)`.
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
        let killer = killer_slot.map_or(0, |s| i32::from(s) + 1);
        let victim = i32::from(victim_slot) + 1;
        let args = [killer, victim, weapon, hitplace, i32::from(team_kill)];
        self.dispatch_forward_all(eng, hooks, "client_death", &args);
    }

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
        self.dispatch_ham(
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

            // Reschedule or remove before executing so `remove_task` inside the callback works.
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
                && let Some(pub_idx) = plugin.vm.find_public(&task.callback)
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
                let mut dispatch = AmxxNativeDispatch {
                    plugin_idx: task.plugin_idx,
                    state: &mut self.state,
                    eng,
                    hooks,
                };
                if let Err(err) = plugin
                    .vm
                    .exec(ExecEntry::Public(pub_idx), &args, &mut dispatch)
                {
                    warn!(
                        plugin = %plugin.name,
                        callback = %task.callback,
                        error = %err,
                        "AMXX task callback error"
                    );
                }
            }
        }
    }

    /// Handles a client command (`register_clcmd`, `register_concmd`, then `client_command`
    /// forward). Returns `true` if a plugin handled the command (`>= PLUGIN_HANDLED`).
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
        self.state.current_argv = argv.to_vec();

        let player_id = i32::from(slot) + 1;
        let user_flags = self.state.admin_flags[usize::from(slot)];

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

        // Check plugin-registered AMXX cvars not in engine's CVarRegistry.
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
            _ => String::from("AMX Mod X (cs-amxx pure-Rust Pawn 3.2 host)\nUsage: amxx <plugins|cvars>\n"),
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
        self.runtime.on_server_activate_precache(eng, &mut self.inner);
        self.inner.server_activate(eng);
        self.runtime.on_server_activate_init(eng, &mut self.inner);
        let unhandled = self.runtime.drain_pending_server_cmds(eng, &mut self.inner);
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
        let ret = self
            .runtime
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
        let unhandled = self.runtime.drain_pending_server_cmds(eng, &mut self.inner);
        for argv in unhandled {
            let _ = self.inner.server_command(eng, &argv);
        }
        self.inner.start_frame(eng, frametime);
    }

    fn think(&mut self, eng: &mut EngineApi<'_>, id: EdictId) {
        self.inner.think(eng, id);
    }

    fn touch(&mut self, eng: &mut EngineApi<'_>, toucher: EdictId, other: EdictId) {
        self.inner.touch(eng, toucher, other);
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
