//! Deterministic in-memory [`MockHostBridge`] for native `cargo test` execution of Rust mods
//! without booting a server or compiling WASM.

use std::collections::BTreeMap;

use cs_mod_api::{
    AdminFlags, ArmorKind, BarTimeSpec, CustomWeaponSpec, DhudMessageSpec, HookId, HookPhase,
    HttpPostJsonSpec, HudDrawCmd, HudMessageSpec, LeaderboardEntry, MenuItemSpec, ScreenFadeSpec,
    ScreenShakeSpec, ShowMotdSpec, TeamKind, VfxOp,
};

use crate::bridge::HostBridge;

/// Simulated player slot inside [`MockHostBridge`].
#[derive(Debug, Clone)]
pub struct MockPlayer {
    pub slot: u8,
    pub active: bool,
    pub alive: bool,
    pub name: String,
    pub health: f32,
    pub max_health: f32,
    pub armor: i32,
    pub armor_kind: ArmorKind,
    pub money: i32,
    pub frags: i32,
    pub deaths: i32,
    pub team: TeamKind,
    pub godmode: bool,
    pub noclip: bool,
    pub maxspeed: f32,
    pub gravity: f32,
    pub silent_footsteps: bool,
    pub rendering: (u8, [u8; 3], u8, u8),
    pub model: Option<String>,
    pub ip: String,
    pub steamid: String,
    pub ping: i32,
    pub loss: i32,
    pub aim_origin: [f32; 3],
    pub items: Vec<String>,
    pub weapon_clips: BTreeMap<u8, i32>,
    pub weapon_bpammo: BTreeMap<u8, i32>,
    pub has_defuser: bool,
    pub has_nvg: bool,
    pub admin_flags: AdminFlags,
    pub lang: String,
}

impl MockPlayer {
    pub fn new(slot: u8, name: impl Into<String>) -> Self {
        Self {
            slot,
            active: true,
            alive: true,
            name: name.into(),
            health: 100.0,
            max_health: 100.0,
            armor: 0,
            armor_kind: ArmorKind::None,
            money: 800,
            frags: 0,
            deaths: 0,
            team: TeamKind::Ct,
            godmode: false,
            noclip: false,
            maxspeed: 250.0,
            gravity: 1.0,
            silent_footsteps: false,
            rendering: (0, [255, 255, 255], 0, 255),
            model: None,
            ip: format!("127.0.0.{}", slot.saturating_add(1)),
            steamid: format!("STEAM_0:0:{}", 1000 + u32::from(slot)),
            ping: 15,
            loss: 0,
            aim_origin: [0.0; 3],
            items: vec!["weapon_knife".to_string()],
            weapon_clips: BTreeMap::new(),
            weapon_bpammo: BTreeMap::new(),
            has_defuser: false,
            has_nvg: false,
            admin_flags: AdminFlags::USER,
            lang: "en".to_string(),
        }
    }
}

/// Simulated entity inside [`MockHostBridge`].
#[derive(Debug, Clone)]
pub struct MockEntity {
    pub index: u16,
    pub classname: String,
    pub health: f32,
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
    pub flags: u32,
}

/// Recorded paginated menu created via `menu_create` / `menu_add_*`.
#[derive(Debug, Clone)]
pub struct MockPaginatedMenu {
    pub id: u32,
    pub title: String,
    pub callback_id: i32,
    pub items: Vec<MenuItemSpec>,
    pub props_int: BTreeMap<i32, i32>,
    pub props_str: BTreeMap<i32, String>,
}

/// Recorded timer scheduled via `timer_after` or `timer_repeat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockTimer {
    pub id: u32,
    pub delay_ms: u32,
    pub repeat: bool,
    pub cb_id: i32,
    pub arg: i32,
    pub cancelled: bool,
}

/// Deterministic in-memory [`HostBridge`] for native unit testing mod logic in milliseconds.
#[derive(Debug)]
pub struct MockHostBridge {
    pub now_ms: i64,
    pub max_clients: u8,
    pub prints: Vec<String>,
    pub client_prints: Vec<(u8, String)>,
    pub center_prints: Vec<(Option<u8>, String)>,
    pub say_texts: Vec<(Option<u8>, String)>,
    pub menus: Vec<(u8, u16, i8, String)>,
    pub health_sets: BTreeMap<u16, f32>,
    pub storage: BTreeMap<String, String>,
    pub usermsgs: Vec<(Option<u8>, String, Vec<u8>)>,

    // ABI v2 state & recordings
    pub hooks: Vec<(HookId, HookPhase, i32)>,
    pub active_i32_args: [i32; 6],
    pub active_f32_args: [f32; 4],
    pub active_str_args: [String; 3],
    pub active_bytes: Vec<u8>,
    pub override_i32: Option<i32>,
    pub override_i32_2: Option<i32>,
    pub override_f32: Option<f32>,
    pub override_str: Option<String>,
    pub rpc_return_str: Option<String>,

    pub players: BTreeMap<u8, MockPlayer>,
    pub entities: BTreeMap<u16, MockEntity>,
    pub kicked: Vec<(u8, String)>,
    pub respawns: Vec<u8>,
    pub slaps: Vec<(u8, i32, bool)>,
    pub slays: Vec<(u8, bool)>,
    pub damages: Vec<(u8, Option<u8>, f32, u32)>,
    pub dropped_slots: Vec<(u8, u8)>,
    pub custom_weapons: Vec<CustomWeaponSpec>,
    pub given_custom_weapons: Vec<(u8, String)>,
    pub vfx_ops: Vec<VfxOp>,

    pub paginated_menus: BTreeMap<u32, MockPaginatedMenu>,
    pub menu_displays: Vec<(u8, u32, u16, i8)>,
    pub hud_messages: Vec<HudMessageSpec>,
    pub dhud_messages: Vec<DhudMessageSpec>,
    pub motds: Vec<ShowMotdSpec>,
    pub screen_fades: Vec<ScreenFadeSpec>,
    pub screen_shakes: Vec<ScreenShakeSpec>,
    pub bar_times: Vec<BarTimeSpec>,
    pub hud_cmds: Vec<HudDrawCmd>,

    pub cvars: BTreeMap<String, String>,
    pub registered_commands: Vec<String>,
    pub server_execs: Vec<String>,
    pub client_execs: Vec<String>,
    pub level_changes: Vec<String>,
    pub timers: Vec<MockTimer>,
    pub precached_sounds: Vec<String>,
    pub emitted_sounds: Vec<(u16, u8, String, f32)>,
    pub trace_fraction: f32,

    pub server_lang: String,
    pub lang_tables: BTreeMap<String, BTreeMap<String, String>>,
    pub rpc_exports: BTreeMap<String, i32>,
    pub rpc_subscriptions: BTreeMap<String, Vec<i32>>,
    pub rpc_events: Vec<(String, String)>,
    pub rpc_mock_responses: BTreeMap<String, (i32, Option<String>)>,
    pub nvaults: BTreeMap<i32, (String, BTreeMap<String, String>)>,
    pub leaderboards: BTreeMap<String, BTreeMap<String, (f64, String)>>,
    pub webhooks: Vec<HttpPostJsonSpec>,

    next_menu_id: u32,
    next_timer_id: u32,
    next_nvault_handle: i32,
    next_entity_id: u16,
}

impl Default for MockHostBridge {
    fn default() -> Self {
        Self {
            now_ms: 0,
            max_clients: 32,
            prints: Vec::new(),
            client_prints: Vec::new(),
            center_prints: Vec::new(),
            say_texts: Vec::new(),
            menus: Vec::new(),
            health_sets: BTreeMap::new(),
            storage: BTreeMap::new(),
            usermsgs: Vec::new(),
            hooks: Vec::new(),
            active_i32_args: [0; 6],
            active_f32_args: [0.0; 4],
            active_str_args: Default::default(),
            active_bytes: Vec::new(),
            override_i32: None,
            override_i32_2: None,
            override_f32: None,
            override_str: None,
            rpc_return_str: None,
            players: BTreeMap::new(),
            entities: BTreeMap::new(),
            kicked: Vec::new(),
            respawns: Vec::new(),
            slaps: Vec::new(),
            slays: Vec::new(),
            damages: Vec::new(),
            dropped_slots: Vec::new(),
            custom_weapons: Vec::new(),
            given_custom_weapons: Vec::new(),
            vfx_ops: Vec::new(),
            paginated_menus: BTreeMap::new(),
            menu_displays: Vec::new(),
            hud_messages: Vec::new(),
            dhud_messages: Vec::new(),
            motds: Vec::new(),
            screen_fades: Vec::new(),
            screen_shakes: Vec::new(),
            bar_times: Vec::new(),
            hud_cmds: Vec::new(),
            cvars: BTreeMap::new(),
            registered_commands: Vec::new(),
            server_execs: Vec::new(),
            client_execs: Vec::new(),
            level_changes: Vec::new(),
            timers: Vec::new(),
            precached_sounds: Vec::new(),
            emitted_sounds: Vec::new(),
            trace_fraction: 1.0,
            server_lang: "en".to_string(),
            lang_tables: BTreeMap::new(),
            rpc_exports: BTreeMap::new(),
            rpc_subscriptions: BTreeMap::new(),
            rpc_events: Vec::new(),
            rpc_mock_responses: BTreeMap::new(),
            nvaults: BTreeMap::new(),
            leaderboards: BTreeMap::new(),
            webhooks: Vec::new(),
            next_menu_id: 1,
            next_timer_id: 1,
            next_nvault_handle: 1,
            next_entity_id: 33,
        }
    }
}

impl MockHostBridge {
    pub fn add_player(&mut self, slot: u8, name: impl Into<String>) -> &mut MockPlayer {
        self.players
            .entry(slot)
            .or_insert_with(|| MockPlayer::new(slot, name))
    }

    pub fn set_player_lang(&mut self, slot: u8, code: impl Into<String>) {
        if let Some(p) = self.players.get_mut(&slot) {
            p.lang = code.into().to_ascii_lowercase();
        }
    }

    pub fn set_player_admin_flags(&mut self, slot: u8, flags: AdminFlags) {
        if let Some(p) = self.players.get_mut(&slot) {
            p.admin_flags = flags;
        }
    }

    pub fn insert_lang(&mut self, lang: &str, key: impl Into<String>, value: impl Into<String>) {
        self.lang_tables
            .entry(lang.trim().to_ascii_lowercase())
            .or_default()
            .insert(key.into(), value.into());
    }

    fn parse_lang_kv(&mut self, src: &str, strip_quotes: bool) -> usize {
        let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
        let mut cur_lang = "en".to_string();
        let mut count = 0usize;
        for raw in src.lines() {
            let line = raw.trim();
            if line.is_empty()
                || line.starts_with('#')
                || line.starts_with(';')
                || line.starts_with("//")
            {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                cur_lang = line[1..line.len() - 1]
                    .trim()
                    .trim_matches('"')
                    .to_ascii_lowercase();
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim().trim_matches('"');
                let mut val = v.trim();
                if strip_quotes && val.len() >= 2 && val.starts_with('"') && val.ends_with('"') {
                    val = &val[1..val.len() - 1];
                }
                if !key.is_empty() {
                    self.insert_lang(&cur_lang, key, val.replace("\\n", "\n"));
                    count += 1;
                }
            }
        }
        count
    }

    fn sorted_leaderboard_rows(&self, table: &str) -> Vec<LeaderboardEntry> {
        let Some(entries) = self.leaderboards.get(table) else {
            return Vec::new();
        };
        let mut rows: Vec<(String, f64, String)> = entries
            .iter()
            .map(|(k, (s, p))| (k.clone(), *s, p.clone()))
            .collect();
        rows.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        rows.into_iter()
            .enumerate()
            .map(|(idx, (key, score, payload_json))| LeaderboardEntry {
                rank: idx + 1,
                key,
                score,
                payload_json,
            })
            .collect()
    }
}

impl HostBridge for MockHostBridge {
    fn time_ms(&self) -> i64 {
        self.now_ms
    }

    fn print_all(&mut self, text: &str) {
        self.prints.push(text.to_string());
    }

    fn center_print(&mut self, slot: Option<u8>, text: &str) {
        self.center_prints.push((slot, text.to_string()));
    }

    fn say_text(&mut self, slot: Option<u8>, text: &str) {
        self.say_texts.push((slot, text.to_string()));
    }

    fn show_menu(&mut self, slot: u8, keys_mask: u16, time_sec: i8, text: &str) {
        self.menus.push((slot, keys_mask, time_sec, text.to_string()));
    }

    fn set_health(&mut self, ent_index: u16, health: f32) {
        self.health_sets.insert(ent_index, health);
        if let Some(ent) = self.entities.get_mut(&ent_index) {
            ent.health = health;
        }
    }

    fn storage_get(&self, key: &str) -> Option<String> {
        self.storage.get(key).cloned()
    }

    fn storage_set(&mut self, key: &str, value: &str) {
        self.storage.insert(key.to_string(), value.to_string());
    }

    fn send_custom_usermsg(&mut self, slot: Option<u8>, name: &str, data: &[u8]) {
        self.usermsgs.push((slot, name.to_string(), data.to_vec()));
    }

    fn hook_register(&mut self, hook: HookId, phase: HookPhase, priority: i32) -> i32 {
        self.hooks.push((hook, phase, priority));
        0
    }

    fn hook_arg_i32(&self, idx: i32) -> i32 {
        usize::try_from(idx)
            .ok()
            .and_then(|i| self.active_i32_args.get(i).copied())
            .unwrap_or(0)
    }

    fn hook_arg_f32(&self, idx: i32) -> f32 {
        usize::try_from(idx)
            .ok()
            .and_then(|i| self.active_f32_args.get(i).copied())
            .unwrap_or(0.0)
    }

    fn hook_arg_str(&self, idx: i32) -> Option<String> {
        usize::try_from(idx)
            .ok()
            .and_then(|i| self.active_str_args.get(i).cloned())
    }

    fn hook_arg_bytes(&self) -> Vec<u8> {
        self.active_bytes.clone()
    }

    fn hook_set_override_i32(&mut self, val: i32) -> i32 {
        self.override_i32 = Some(val);
        0
    }

    fn hook_set_override_i32_2(&mut self, val: i32) -> i32 {
        self.override_i32_2 = Some(val);
        0
    }

    fn hook_set_override_f32(&mut self, val: f32) -> i32 {
        if !val.is_finite() {
            return -1;
        }
        self.override_f32 = Some(val);
        0
    }

    fn hook_set_override_str(&mut self, val: &str) -> i32 {
        self.override_str = Some(val.to_string());
        0
    }

    fn rpc_set_return_str(&mut self, val: &str) -> i32 {
        self.rpc_return_str = Some(val.to_string());
        0
    }

    fn ent_count(&self) -> i32 {
        self.entities.len() as i32
    }

    fn ent_create(&mut self, classname: &str) -> i32 {
        let idx = self.next_entity_id;
        self.next_entity_id = self.next_entity_id.saturating_add(1);
        self.entities.insert(
            idx,
            MockEntity {
                index: idx,
                classname: classname.to_string(),
                health: 0.0,
                origin: [0.0; 3],
                velocity: [0.0; 3],
                flags: 0,
            },
        );
        0
    }

    fn ent_remove(&mut self, index: u16) -> i32 {
        self.entities.remove(&index);
        0
    }

    fn ent_get_health(&self, index: u16) -> f32 {
        self.entities.get(&index).map_or(-1.0, |e| e.health)
    }

    fn ent_get_origin(&self, index: u16) -> Option<[f32; 3]> {
        self.entities.get(&index).map(|e| e.origin)
    }

    fn ent_set_origin(&mut self, index: u16, origin: [f32; 3]) -> i32 {
        if let Some(ent) = self.entities.get_mut(&index) {
            ent.origin = origin;
        }
        0
    }

    fn ent_get_velocity(&self, index: u16) -> Option<[f32; 3]> {
        self.entities.get(&index).map(|e| e.velocity)
    }

    fn ent_set_velocity(&mut self, index: u16, velocity: [f32; 3]) -> i32 {
        if let Some(ent) = self.entities.get_mut(&index) {
            ent.velocity = velocity;
        }
        0
    }

    fn ent_get_flags(&self, index: u16) -> i32 {
        self.entities.get(&index).map_or(-1, |e| e.flags as i32)
    }

    fn ent_set_flags(&mut self, index: u16, flags: u32) -> i32 {
        if let Some(ent) = self.entities.get_mut(&index) {
            ent.flags = flags;
        }
        0
    }

    fn ent_classname(&self, index: u16) -> Option<String> {
        self.entities.get(&index).map(|e| e.classname.clone())
    }

    fn player_max_clients(&self) -> u8 {
        self.max_clients
    }

    fn player_is_active(&self, slot: u8) -> bool {
        self.players.get(&slot).is_some_and(|p| p.active)
    }

    fn player_name(&self, slot: u8) -> Option<String> {
        self.players.get(&slot).map(|p| p.name.clone())
    }

    fn player_kick(&mut self, slot: u8, reason: &str) -> i32 {
        self.kicked.push((slot, reason.to_string()));
        if let Some(p) = self.players.get_mut(&slot) {
            p.active = false;
        }
        0
    }

    fn print_client(&mut self, slot: u8, text: &str) -> i32 {
        self.client_prints.push((slot, text.to_string()));
        0
    }

    fn player_get_health(&self, slot: u8) -> f32 {
        self.players.get(&slot).map_or(-1.0, |p| p.health)
    }

    fn player_set_health(&mut self, slot: u8, health: f32) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.health = health;
        }
        0
    }

    fn player_set_max_health(&mut self, slot: u8, max_health: f32) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.max_health = max_health;
        }
        0
    }

    fn player_get_armor(&self, slot: u8) -> i32 {
        self.players.get(&slot).map_or(-1, |p| p.armor)
    }

    fn player_get_armor_type(&self, slot: u8) -> ArmorKind {
        self.players
            .get(&slot)
            .map_or(ArmorKind::None, |p| p.armor_kind)
    }

    fn player_set_armor(&mut self, slot: u8, armor: i32, kind: ArmorKind) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.armor = armor;
            p.armor_kind = kind;
        }
        0
    }

    fn player_get_money(&self, slot: u8) -> i32 {
        self.players.get(&slot).map_or(-1, |p| p.money)
    }

    fn player_set_money(&mut self, slot: u8, amount: i32, _flash: bool) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.money = amount.clamp(0, 16000);
        }
        0
    }

    fn player_add_money(&mut self, slot: u8, delta: i32, _flash: bool) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.money = p.money.saturating_add(delta).clamp(0, 16000);
        }
        0
    }

    fn player_get_frags(&self, slot: u8) -> i32 {
        self.players.get(&slot).map_or(0, |p| p.frags)
    }

    fn player_get_deaths(&self, slot: u8) -> i32 {
        self.players.get(&slot).map_or(0, |p| p.deaths)
    }

    fn player_set_score(&mut self, slot: u8, frags: i32, deaths: i32) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.frags = frags;
            p.deaths = deaths;
        }
        0
    }

    fn player_get_team(&self, slot: u8) -> TeamKind {
        self.players
            .get(&slot)
            .map_or(TeamKind::Unassigned, |p| p.team)
    }

    fn player_set_team(&mut self, slot: u8, team: TeamKind, _model_class: Option<u8>) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.team = team;
        }
        0
    }

    fn player_is_alive(&self, slot: u8) -> bool {
        self.players.get(&slot).is_some_and(|p| p.alive)
    }

    fn player_respawn(&mut self, slot: u8) -> i32 {
        self.respawns.push(slot);
        if let Some(p) = self.players.get_mut(&slot) {
            p.alive = true;
            p.health = p.max_health;
        }
        0
    }

    fn player_slap(&mut self, slot: u8, power: i32, random_dir: bool) -> i32 {
        self.slaps.push((slot, power, random_dir));
        if let Some(p) = self.players.get_mut(&slot) {
            p.health = (p.health - power as f32).max(0.0);
        }
        0
    }

    fn player_slay(&mut self, slot: u8, preserve_frags: bool) -> i32 {
        self.slays.push((slot, preserve_frags));
        if let Some(p) = self.players.get_mut(&slot) {
            p.alive = false;
            p.health = 0.0;
        }
        0
    }

    fn player_damage(
        &mut self,
        victim: u8,
        attacker: Option<u8>,
        damage: f32,
        bits: u32,
    ) -> i32 {
        self.damages.push((victim, attacker, damage, bits));
        if let Some(p) = self.players.get_mut(&victim)
            && !p.godmode
        {
            p.health = (p.health - damage).max(0.0);
            if p.health <= 0.0 {
                p.alive = false;
            }
        }
        0
    }

    fn player_set_godmode(&mut self, slot: u8, enabled: bool) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.godmode = enabled;
        }
        0
    }

    fn player_set_noclip(&mut self, slot: u8, enabled: bool) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.noclip = enabled;
        }
        0
    }

    fn player_set_maxspeed(&mut self, slot: u8, maxspeed: f32) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.maxspeed = maxspeed;
        }
        0
    }

    fn player_set_gravity(&mut self, slot: u8, gravity: f32) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.gravity = gravity;
        }
        0
    }

    fn player_set_footsteps(&mut self, slot: u8, silent: bool) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.silent_footsteps = silent;
        }
        0
    }

    fn player_set_rendering(
        &mut self,
        slot: u8,
        fx: u8,
        rgb: [u8; 3],
        mode: u8,
        amount: u8,
    ) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.rendering = (fx, rgb, mode, amount);
        }
        0
    }

    fn player_set_model(&mut self, slot: u8, model: &str) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.model = Some(model.to_string());
        }
        0
    }

    fn player_reset_model(&mut self, slot: u8) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.model = None;
        }
        0
    }

    fn player_get_ip(&self, slot: u8) -> Option<String> {
        self.players.get(&slot).map(|p| p.ip.clone())
    }

    fn player_get_steamid(&self, slot: u8) -> Option<String> {
        self.players.get(&slot).map(|p| p.steamid.clone())
    }

    fn player_get_ping(&self, slot: u8) -> i32 {
        self.players.get(&slot).map_or(0, |p| p.ping)
    }

    fn player_get_loss(&self, slot: u8) -> i32 {
        self.players.get(&slot).map_or(0, |p| p.loss)
    }

    fn player_get_aim_origin(&self, slot: u8) -> Option<[f32; 3]> {
        self.players.get(&slot).map(|p| p.aim_origin)
    }

    fn player_give_item(&mut self, slot: u8, item: &str) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.items.push(item.to_string());
        }
        0
    }

    fn player_strip_weapons(&mut self, slot: u8) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.items.clear();
        }
        0
    }

    fn player_get_weapon_clip(&self, slot: u8, weapon_id: u8) -> i32 {
        self.players
            .get(&slot)
            .and_then(|p| p.weapon_clips.get(&weapon_id).copied())
            .unwrap_or(-1)
    }

    fn player_set_weapon_clip(&mut self, slot: u8, weapon_id: u8, clip: i32) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.weapon_clips.insert(weapon_id, clip.max(0));
        }
        0
    }

    fn player_get_bpammo(&self, slot: u8, weapon_id: u8) -> i32 {
        self.players
            .get(&slot)
            .and_then(|p| p.weapon_bpammo.get(&weapon_id).copied())
            .unwrap_or(0)
    }

    fn player_set_bpammo(&mut self, slot: u8, weapon_id: u8, amount: i32) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.weapon_bpammo.insert(weapon_id, amount.max(0));
        }
        0
    }

    fn player_set_defuser(&mut self, slot: u8, has: bool) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.has_defuser = has;
        }
        0
    }

    fn player_set_nvg(&mut self, slot: u8, has: bool) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.has_nvg = has;
        }
        0
    }

    fn player_drop_slot(&mut self, slot: u8, inv_slot: u8) -> i32 {
        self.dropped_slots.push((slot, inv_slot));
        0
    }

    fn register_custom_weapon(&mut self, spec: &CustomWeaponSpec) -> i32 {
        self.custom_weapons.push(spec.clone());
        0
    }

    fn give_custom_weapon(&mut self, slot: u8, id_name: &str) -> i32 {
        self.given_custom_weapons.push((slot, id_name.to_string()));
        0
    }

    fn cvar_get_f32(&self, name: &str) -> f32 {
        self.cvars
            .get(name)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0)
    }

    fn cvar_set_f32(&mut self, name: &str, val: f32) -> i32 {
        self.cvars.insert(name.to_string(), val.to_string());
        0
    }

    fn cvar_get_str(&self, name: &str) -> Option<String> {
        self.cvars.get(name).cloned()
    }

    fn cvar_set_str(&mut self, name: &str, val: &str) -> i32 {
        self.cvars.insert(name.to_string(), val.to_string());
        0
    }

    fn register_command(&mut self, name: &str) -> i32 {
        self.registered_commands.push(name.to_string());
        0
    }

    fn server_exec(&mut self, line: &str) -> i32 {
        self.server_execs.push(line.to_string());
        0
    }

    fn change_level(&mut self, map: &str) -> i32 {
        self.level_changes.push(map.to_string());
        0
    }

    fn timer_after(&mut self, delay_ms: u32, cb_id: i32, arg: i32) -> i32 {
        let id = self.next_timer_id;
        self.next_timer_id = self.next_timer_id.wrapping_add(1).max(1);
        self.timers.push(MockTimer {
            id,
            delay_ms,
            repeat: false,
            cb_id,
            arg,
            cancelled: false,
        });
        id as i32
    }

    fn timer_repeat(&mut self, interval_ms: u32, cb_id: i32, arg: i32) -> i32 {
        let id = self.next_timer_id;
        self.next_timer_id = self.next_timer_id.wrapping_add(1).max(1);
        self.timers.push(MockTimer {
            id,
            delay_ms: interval_ms,
            repeat: true,
            cb_id,
            arg,
            cancelled: false,
        });
        id as i32
    }

    fn timer_cancel(&mut self, timer_id: u32) -> i32 {
        for t in &mut self.timers {
            if t.id == timer_id {
                t.cancelled = true;
            }
        }
        0
    }

    fn trace_line(&self, _start: [f32; 3], _end: [f32; 3]) -> f32 {
        self.trace_fraction
    }

    fn precache_sound(&mut self, sample: &str) -> i32 {
        self.precached_sounds.push(sample.to_string());
        0
    }

    fn emit_sound(&mut self, ent_idx: u16, channel: u8, sample: &str, volume: f32) -> i32 {
        self.emitted_sounds
            .push((ent_idx, channel, sample.to_string(), volume));
        0
    }

    fn storage_remove(&mut self, key: &str) -> i32 {
        self.storage.remove(key);
        0
    }

    fn emit_vfx(&mut self, vfx: VfxOp) -> i32 {
        self.vfx_ops.push(vfx);
        0
    }

    fn menu_create(&mut self, title: &str, cb_id: i32) -> i32 {
        let id = self.next_menu_id;
        self.next_menu_id = self.next_menu_id.wrapping_add(1).max(1);
        self.paginated_menus.insert(
            id,
            MockPaginatedMenu {
                id,
                title: title.to_string(),
                callback_id: cb_id,
                items: Vec::new(),
                props_int: BTreeMap::new(),
                props_str: BTreeMap::new(),
            },
        );
        id as i32
    }

    fn menu_add_item(&mut self, menu_id: u32, label: &str, info: &str, enabled: bool) -> i32 {
        let Some(m) = self.paginated_menus.get_mut(&menu_id) else {
            return -1;
        };
        m.items.push(MenuItemSpec::selectable(label, info, enabled));
        0
    }

    fn menu_add_blank(&mut self, menu_id: u32) -> i32 {
        let Some(m) = self.paginated_menus.get_mut(&menu_id) else {
            return -1;
        };
        m.items.push(MenuItemSpec::blank());
        0
    }

    fn menu_add_text(&mut self, menu_id: u32, text: &str) -> i32 {
        let Some(m) = self.paginated_menus.get_mut(&menu_id) else {
            return -1;
        };
        m.items.push(MenuItemSpec::text(text));
        0
    }

    fn menu_set_prop_int(&mut self, menu_id: u32, prop: i32, val: i32) -> i32 {
        let Some(m) = self.paginated_menus.get_mut(&menu_id) else {
            return -1;
        };
        m.props_int.insert(prop, val);
        0
    }

    fn menu_set_prop_str(&mut self, menu_id: u32, prop: i32, val: &str) -> i32 {
        let Some(m) = self.paginated_menus.get_mut(&menu_id) else {
            return -1;
        };
        m.props_str.insert(prop, val.to_string());
        0
    }

    fn menu_display(&mut self, slot: u8, menu_id: u32, page: u16, time_sec: i8) -> i32 {
        if !self.paginated_menus.contains_key(&menu_id) {
            return -1;
        }
        self.menu_displays.push((slot, menu_id, page, time_sec));
        0
    }

    fn menu_destroy(&mut self, menu_id: u32) -> i32 {
        if self.paginated_menus.remove(&menu_id).is_some() {
            0
        } else {
            -1
        }
    }

    fn hud_message(&mut self, spec: &HudMessageSpec) -> i32 {
        self.hud_messages.push(spec.clone());
        0
    }

    fn dhud_message(&mut self, spec: &DhudMessageSpec) -> i32 {
        self.dhud_messages.push(spec.clone());
        0
    }

    fn show_motd(&mut self, slot: u8, title: &str, body: &str) -> i32 {
        self.motds.push(ShowMotdSpec {
            slot,
            title: title.to_string(),
            body: body.to_string(),
        });
        0
    }

    fn screen_fade(&mut self, spec: &ScreenFadeSpec) -> i32 {
        self.screen_fades.push(*spec);
        0
    }

    fn screen_shake(&mut self, spec: &ScreenShakeSpec) -> i32 {
        self.screen_shakes.push(*spec);
        0
    }

    fn bar_time(&mut self, slot: u8, seconds: u16) -> i32 {
        self.bar_times.push(BarTimeSpec { slot, seconds });
        0
    }

    fn hud_draw_rect(&mut self, x: f32, y: f32, w: f32, h: f32, rgba: [u8; 4]) -> i32 {
        self.hud_cmds.push(HudDrawCmd::Rect { x, y, w, h, rgba });
        0
    }

    fn hud_draw_text(&mut self, x: f32, y: f32, size: f32, rgba: [u8; 4], text: &str) -> i32 {
        self.hud_cmds.push(HudDrawCmd::Text {
            x,
            y,
            size,
            rgba,
            text: text.to_string(),
        });
        0
    }

    fn hud_draw_bar(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        frac: f32,
        rgba: [u8; 4],
    ) -> i32 {
        self.hud_cmds.push(HudDrawCmd::ProgressBar {
            x,
            y,
            w,
            h,
            fraction: frac.clamp(0.0, 1.0),
            rgba,
        });
        0
    }

    fn hud_panel(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        rgba: [u8; 4],
        title: &str,
    ) -> i32 {
        self.hud_cmds.push(HudDrawCmd::Panel {
            x,
            y,
            w,
            h,
            rgba,
            title: title.to_string(),
        });
        0
    }

    fn client_exec(&mut self, line: &str) -> i32 {
        self.client_execs.push(line.to_string());
        0
    }

    fn lang_load_toml(&mut self, src: &str) -> i32 {
        self.parse_lang_kv(src, true) as i32
    }

    fn lang_load_amxx(&mut self, src: &str) -> i32 {
        self.parse_lang_kv(src, false) as i32
    }

    fn lang_tr(&self, slot: Option<u8>, key: &str, args: &[(&str, &str)]) -> String {
        let code = self.lang_player_code(slot);
        let template = self
            .lang_tables
            .get(&code)
            .and_then(|m| m.get(key))
            .or_else(|| self.lang_tables.get(&self.server_lang).and_then(|m| m.get(key)))
            .or_else(|| self.lang_tables.get("en").and_then(|m| m.get(key)))
            .map(String::as_str)
            .unwrap_or(key);
        let mut out = template.to_string();
        for (idx, (k, v)) in args.iter().enumerate() {
            if !k.is_empty() {
                out = out.replace(&format!("{{{k}}}"), v);
            }
            out = out.replace(&format!("{{{idx}}}"), v);
        }
        out
    }

    fn lang_player_code(&self, slot: Option<u8>) -> String {
        slot.and_then(|s| self.players.get(&s))
            .map(|p| p.lang.clone())
            .unwrap_or_else(|| self.server_lang.clone())
    }

    fn lang_set_player_code(&mut self, slot: u8, code: &str) -> i32 {
        if let Some(p) = self.players.get_mut(&slot) {
            p.lang = code.trim().to_ascii_lowercase();
            0
        } else {
            -1
        }
    }

    fn admin_player_flags(&self, slot: u8) -> AdminFlags {
        self.players
            .get(&slot)
            .map_or(AdminFlags::ALL, |p| p.admin_flags)
    }

    fn admin_has_flag(&self, slot: u8, flag: char) -> bool {
        self.admin_player_flags(slot).has_char(flag)
    }

    fn rpc_export(&mut self, fn_name: &str, cb_id: i32) -> i32 {
        self.rpc_exports.insert(fn_name.to_string(), cb_id);
        0
    }

    fn rpc_call(&mut self, fn_name: &str, _arg_i32: i32, _arg_str: &str) -> (i32, Option<String>) {
        self.rpc_mock_responses
            .get(fn_name)
            .cloned()
            .unwrap_or((-1, None))
    }

    fn rpc_subscribe(&mut self, event_name: &str, cb_id: i32) -> i32 {
        self.rpc_subscriptions
            .entry(event_name.to_string())
            .or_default()
            .push(cb_id);
        0
    }

    fn rpc_emit_event(&mut self, event_name: &str, payload_json: &str) -> i32 {
        self.rpc_events
            .push((event_name.to_string(), payload_json.to_string()));
        0
    }

    fn nvault_open(&mut self, name: &str) -> i32 {
        for (&handle, (existing_name, _)) in &self.nvaults {
            if existing_name == name {
                return handle;
            }
        }
        let h = self.next_nvault_handle;
        self.next_nvault_handle = self.next_nvault_handle.wrapping_add(1).max(1);
        self.nvaults.insert(h, (name.to_string(), BTreeMap::new()));
        h
    }

    fn nvault_get(&self, handle: i32, key: &str) -> Option<String> {
        self.nvaults
            .get(&handle)
            .and_then(|(_, map)| map.get(key).cloned())
    }

    fn nvault_set(&mut self, handle: i32, key: &str, val: &str) -> i32 {
        let Some((_, map)) = self.nvaults.get_mut(&handle) else {
            return -1;
        };
        map.insert(key.to_string(), val.to_string());
        0
    }

    fn nvault_remove(&mut self, handle: i32, key: &str) -> i32 {
        let Some((_, map)) = self.nvaults.get_mut(&handle) else {
            return -1;
        };
        if map.remove(key).is_some() { 0 } else { -1 }
    }

    fn leaderboard_upsert(
        &mut self,
        table: &str,
        key: &str,
        score: f64,
        payload_json: &str,
    ) -> i32 {
        if !score.is_finite() {
            return -1;
        }
        self.leaderboards
            .entry(table.to_string())
            .or_default()
            .insert(key.to_string(), (score, payload_json.to_string()));
        0
    }

    fn leaderboard_rank(&self, table: &str, key: &str) -> Option<(LeaderboardEntry, usize)> {
        let rows = self.sorted_leaderboard_rows(table);
        let total = rows.len();
        rows.into_iter().find(|r| r.key == key).map(|r| (r, total))
    }

    fn leaderboard_top(&self, table: &str, offset: usize, limit: usize) -> Vec<LeaderboardEntry> {
        self.sorted_leaderboard_rows(table)
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect()
    }

    fn http_post_json(&mut self, url: &str, body_json: &str, cb_id: i32) -> i32 {
        self.webhooks.push(HttpPostJsonSpec {
            url: url.to_string(),
            body: body_json.to_string(),
            callback_id: cb_id,
        });
        0
    }
}
