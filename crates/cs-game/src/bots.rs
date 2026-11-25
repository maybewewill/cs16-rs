// SPDX-License-Identifier: GPL-3.0-or-later
//! Server-side CS 1.6 / Condition Zero ZBot driver over [`cs_bot`] and ReHLDS fake clients
//! (`EngineApi::create_fake_client`, `EngineApi::run_player_move`, ReGameDLL_CS
//! `bot/cs_bot_manager.cpp`, `bot/cs_bot.cpp`, `bot/cs_bot_nav.cpp`, `gs/bot.cpp`).
//!
//! `quota_enabled` defaults to `false` so existing unit/integration tests and the legacy offline
//! registry (which registers `bot_quota = "9"`) do not spawn bots behind a test's back; dedicated
//! `--proto48` servers and `LocalGame` call [`crate::CsGame::enable_cvar_bot_quota`], and
//! `bot_add*` / `bot_kick` / `bot_nav_*` console commands work everywhere.

use cs_assets::nav::{HidingSpotFlags, NavArea, NavAttribute, NavCorner, NavDir, NavFile, UNDEFINED_PLACE};
use cs_bot::buy::{BuyContext, BuyPlan, decide_bot_buy};
use cs_bot::combat::CombatInventory;
use cs_bot::nav_gen::{NavGenConfig, NavLadderHint, NavWorld, NavZoneHint, NavZoneKind, generate_nav_mesh};
use cs_bot::nav_mesh::NavMesh;
use cs_bot::path::{NavPath, PathAction, PathCostParams, PathObstacles, find_path};
use cs_bot::profile::{BotDifficulty, BotProfile, BotProfileDb, BotTeam, BotWeapon};
use cs_bot::tactics::{
    BotJoinTeam, BotSettings, BotTacticalBrain, BotTacticalState, IncomingRadio,
    PerceivedEnemyCandidate, RadioMessage, SmokeCloud, SoundEvent, SoundKind, TacticalContext,
    TacticalDecision, TacticalHostage, TacticalSpot,
};
use cs_engine::collision::{Hull, TraceResult};
use cs_engine::consts::{FL_DUCKING, FL_FROZEN, FL_ONGROUND};
use cs_engine::{EngineApi, MoveFilter};
use cs_sim::pm::consts::{
    IN_ATTACK, IN_ATTACK2, IN_BACK, IN_DUCK, IN_FORWARD, IN_JUMP, IN_MOVELEFT, IN_MOVERIGHT,
    IN_RELOAD, IN_USE, PITCH_NORM,
};
use glam::Vec3;

use crate::GameState;
use crate::buy::{SIGNAL_BOMB, SIGNAL_BUY};
use crate::entity::GameEntity;
use crate::inventory::{KNIFE_SLOT, PISTOL_SLOT, PRIMARY_WEAPON_SLOT};
use crate::player::{self, DEAD_NO, MAX_PLAYERS, Team};
use crate::teams::{
    self, JoinState, MENU_SLOT_TEAM_CT, MENU_SLOT_TEAM_TERRORIST, handle_menu_choose_appearance,
    handle_menu_choose_team,
};
use crate::weapons::{AmmoType, WeaponId, item_info};

const MAX_SOUND_EVENTS: usize = 64;
const MAX_RADIO_EVENTS: usize = 32;

/// Engine-backed [`NavWorld`] bridge for Condition Zero automatic navmesh generation.
struct EngineNavWorld<'a, 'b>(&'a EngineApi<'b>);

impl NavWorld for EngineNavWorld<'_, '_> {
    fn trace_hull(&self, start: Vec3, end: Vec3, hull: Hull) -> TraceResult {
        self.0
            .trace_hull_ents(start, end, hull, MoveFilter::NoMonsters, None)
            .trace
    }

    fn trace_line(&self, start: Vec3, end: Vec3) -> TraceResult {
        self.0
            .trace_line(start, end, MoveFilter::NoMonsters, None)
            .trace
    }

    fn point_contents(&self, pos: Vec3, hull: Hull) -> i32 {
        self.0.point_contents(pos, hull)
    }

    fn seed_points(&self) -> Vec<Vec3> {
        let mut seeds = Vec::new();
        for class in [
            "info_player_start",
            "info_player_deathmatch",
            "info_vip_start",
            "hostage_entity",
            "info_bomb_target",
            "info_hostage_rescue",
        ] {
            let mut cursor = None;
            while let Some(id) = self.0.find_by_classname(cursor, class) {
                if let Some(v) = self.0.ent(id) {
                    seeds.push(v.origin + Vec3::new(0.0, 0.0, 4.0));
                }
                cursor = Some(id);
            }
        }
        for class in [
            "func_bomb_target",
            "func_hostage_rescue",
            "func_vip_safetyzone",
            "func_escapezone",
            "func_buyzone",
        ] {
            let mut cursor = None;
            while let Some(id) = self.0.find_by_classname(cursor, class) {
                if let Some(v) = self.0.ent(id) {
                    let c = zone_floor_point(v.absmin, v.absmax);
                    let hx = ((v.absmax.x - v.absmin.x) * 0.5 - 16.0).clamp(0.0, 64.0);
                    let hy = ((v.absmax.y - v.absmin.y) * 0.5 - 16.0).clamp(0.0, 64.0);
                    for (ox, oy) in [(0.0, 0.0), (hx, 0.0), (-hx, 0.0), (0.0, hy), (0.0, -hy)] {
                        seeds.push(c + Vec3::new(ox, oy, 4.0));
                    }
                }
                cursor = Some(id);
            }
        }
        seeds
    }

    fn ladders(&self) -> Vec<NavLadderHint> {
        let mut out = Vec::new();
        let mut cursor = None;
        while let Some(id) = self.0.find_by_classname(cursor, "func_ladder") {
            if let Some(v) = self.0.ent(id) {
                out.push(NavLadderHint {
                    mins: v.absmin,
                    maxs: v.absmax,
                });
            }
            cursor = Some(id);
        }
        out
    }

    fn zone_hints(&self) -> Vec<NavZoneHint> {
        let mut hints = Vec::new();
        let mapping = [
            ("info_player_start", NavZoneKind::CtSpawn, false),
            ("info_player_deathmatch", NavZoneKind::TSpawn, false),
            ("func_bomb_target", NavZoneKind::BombSite, true),
            ("info_bomb_target", NavZoneKind::BombSite, false),
            ("hostage_entity", NavZoneKind::Hostage, false),
            ("func_hostage_rescue", NavZoneKind::HostageRescue, true),
            ("info_hostage_rescue", NavZoneKind::HostageRescue, false),
            ("func_vip_safetyzone", NavZoneKind::VipRescue, true),
        ];
        for (class, kind, is_brush) in mapping {
            let mut cursor = None;
            while let Some(id) = self.0.find_by_classname(cursor, class) {
                if let Some(v) = self.0.ent(id) {
                    hints.push(NavZoneHint {
                        kind,
                        origin: if is_brush {
                            zone_floor_point(v.absmin, v.absmax)
                        } else {
                            v.origin
                        },
                        mins: is_brush.then_some(v.absmin),
                        maxs: is_brush.then_some(v.absmax),
                        label: None,
                    });
                }
                cursor = Some(id);
            }
        }
        hints
    }

    fn is_walkable_obstacle(&self, start: Vec3, end: Vec3) -> bool {
        let tr = self.0.trace_line(start, end, MoveFilter::NoMonsters, None);
        tr.ent
            .and_then(|id| self.0.ent(id))
            .is_some_and(|v| matches!(v.classname.as_str(), "func_door" | "func_door_rotating" | "func_breakable"))
    }
}

/// Flattened per-frame movement and action output emitted to `run_player_move` at 30 Hz.
#[derive(Debug, Clone, Copy, Default)]
struct BotCmd {
    yaw_deg: f32,
    pitch_deg: f32,
    forward: f32,
    right: f32,
    attack: bool,
    secondary_attack: bool,
    jump: bool,
    duck: bool,
    use_key: bool,
    reload: bool,
    walk: bool,
    desired_weapon: Option<BotWeapon>,
    want_c4: bool,
}

/// Per-bot ZBot personality, tactical brain, path state, and buy tracking.
#[derive(Debug, Clone)]
struct BotSlot {
    profile: BotProfile,
    brain: BotTacticalBrain,
    path: Option<NavPath>,
    path_wp_idx: usize,
    path_goal: Option<Vec3>,
    path_replan_time: f64,
    next_cmd_time: f64,
    next_full_think: f64,
    prev_cmd_time: f64,
    cmd: BotCmd,
    done_buying: bool,
    buy_retries: u8,
    last_pos: Vec3,
    stuck_ticks: u8,
    hostage_use_cooldown: f64,
    hostage_use_pulse: bool,
}

impl BotSlot {
    fn new(now: f64, profile: BotProfile, settings: &BotSettings, seed: u64) -> Self {
        let mut brain = BotTacticalBrain::default();
        brain.on_round_reset(&profile, settings, seed);
        Self {
            profile,
            brain,
            path: None,
            path_wp_idx: 0,
            path_goal: None,
            path_replan_time: 0.0,
            next_cmd_time: now,
            next_full_think: now,
            prev_cmd_time: now,
            cmd: BotCmd::default(),
            done_buying: false,
            buy_retries: 0,
            last_pos: Vec3::ZERO,
            stuck_ticks: 0,
            hostage_use_cooldown: 0.0,
            hostage_use_pulse: false,
        }
    }
}

/// Server-wide ZBot manager held on [`GameState`].
pub(crate) struct BotManager {
    pub(crate) quota_enabled: bool,
    pub(crate) settings: BotSettings,
    pub(crate) profile_db: BotProfileDb,
    loaded_profile_db_name: String,
    pub(crate) nav_mesh: Option<NavMesh>,
    nav_map_name: String,
    pub(crate) nav_loaded_from_file: bool,
    nav_edit_marked_area: Option<usize>,
    nav_edit_begin_pos: Option<Vec3>,
    nav_edit_selected_corner: NavCorner,
    nav_paint_place: Option<String>,
    nav_place_mode: bool,
    nav_place_painting: bool,
    incoming_radios: Vec<IncomingRadio>,
    sound_events: Vec<SoundEvent>,
    next_quota_check: f64,
    slots: [Option<BotSlot>; MAX_PLAYERS],
}

impl Default for BotManager {
    fn default() -> Self {
        Self {
            quota_enabled: false,
            settings: BotSettings::default(),
            profile_db: BotProfileDb::builtin(),
            loaded_profile_db_name: String::from("BotProfile.db"),
            nav_mesh: None,
            nav_map_name: String::new(),
            nav_loaded_from_file: false,
            nav_edit_marked_area: None,
            nav_edit_begin_pos: None,
            nav_edit_selected_corner: NavCorner::NorthWest,
            nav_paint_place: None,
            nav_place_mode: false,
            nav_place_painting: false,
            incoming_radios: Vec::new(),
            sound_events: Vec::new(),
            next_quota_check: 0.0,
            slots: std::array::from_fn(|_| None),
        }
    }
}

impl BotManager {
    pub(crate) fn reset(&mut self) {
        let quota_enabled = self.quota_enabled;
        let settings = self.settings.clone();
        let profile_db = self.profile_db.clone();
        let loaded_profile_db_name = self.loaded_profile_db_name.clone();
        *self = Self::default();
        self.quota_enabled = quota_enabled;
        self.settings = settings;
        self.profile_db = profile_db;
        self.loaded_profile_db_name = loaded_profile_db_name;
    }

    pub(crate) fn load_profile_db(&mut self, vfs: &cs_vfs::Vfs, path: &str) {
        let trimmed = path.trim();
        let candidates = [
            trimmed.to_string(),
            format!("cstrike/{trimmed}"),
            "BotProfile.db".to_string(),
            "cstrike/BotProfile.db".to_string(),
        ];
        for cand in candidates {
            if cand.is_empty() {
                continue;
            }
            if let Ok(bytes) = vfs.read_bytes(&cand)
                && let Ok(text) = std::str::from_utf8(&bytes)
            {
                match BotProfileDb::parse(text) {
                    Ok(db) if !db.profiles.is_empty() => {
                        self.profile_db = db;
                        self.loaded_profile_db_name = trimmed.to_string();
                        return;
                    }
                    Ok(_) => {}
                    Err(err) => {
                        tracing::warn!(%err, path = %cand, "failed to parse BotProfile.db, using builtin");
                    }
                }
            }
        }
        self.profile_db = BotProfileDb::builtin();
        self.loaded_profile_db_name = trimmed.to_string();
    }
}

/// Syncs `BotSettings` from registered `bot_*` console variables.
fn sync_settings_from_cvars(g: &mut GameState, eng: &EngineApi<'_>) {
    const CVARS: [&str; 29] = [
        "bot_enable",
        "bot_quota",
        "bot_quota_match",
        "bot_difficulty",
        "bot_prefix",
        "bot_profile_db",
        "bot_join_after_player",
        "bot_join_team",
        "bot_auto_vacate",
        "bot_chatter",
        "bot_defer_to_human",
        "bot_walk",
        "bot_stop",
        "bot_zombie",
        "bot_debug",
        "bot_nav_edit",
        "bot_nav_zdraw",
        "bot_quicksave",
        "bot_show_danger",
        "bot_show_nav",
        "bot_traceview",
        "bot_allow_pistols",
        "bot_allow_shotguns",
        "bot_allow_sub_machine_guns",
        "bot_allow_rifles",
        "bot_allow_snipers",
        "bot_allow_machine_guns",
        "bot_allow_grenades",
        "bot_allow_shield",
    ];
    for name in CVARS {
        if let Some(val) = eng.cvar_str(name) {
            g.bots.settings.apply_cvar(name, &val);
        }
    }
    if let Some(val) = eng.cvar_str("bot_allow_rogues") {
        g.bots.settings.apply_cvar("bot_allow_rogues", &val);
    }
    if let Some(vfs) = g.vfs.clone()
        && g.bots.settings.bot_profile_db != g.bots.loaded_profile_db_name
    {
        let db_name = g.bots.settings.bot_profile_db.clone();
        g.bots.load_profile_db(&vfs, &db_name);
    }
}

fn sync_weapon_restriction_cvars(settings: &BotSettings, eng: &mut EngineApi<'_>) {
    let r = settings.weapon_restrictions;
    let b = |v: bool| if v { "1" } else { "0" };
    eng.set_cvar("bot_allow_pistols", b(r.allow_pistols));
    eng.set_cvar("bot_allow_shotguns", b(r.allow_shotguns));
    eng.set_cvar("bot_allow_sub_machine_guns", b(r.allow_sub_machine_guns));
    eng.set_cvar("bot_allow_rifles", b(r.allow_rifles));
    eng.set_cvar("bot_allow_snipers", b(r.allow_snipers));
    eng.set_cvar("bot_allow_machine_guns", b(r.allow_machine_guns));
    eng.set_cvar("bot_allow_grenades", b(r.allow_grenades));
    eng.set_cvar("bot_allow_shield", b(r.allow_shield));
}

fn zone_floor_point(absmin: Vec3, absmax: Vec3) -> Vec3 {
    Vec3::new(
        (absmin.x + absmax.x) * 0.5,
        (absmin.y + absmax.y) * 0.5,
        absmin.z + ((absmax.z - absmin.z) * 0.5).min(36.0),
    )
}

fn try_load_nav_from_vfs(vfs: &cs_vfs::Vfs, map_name: &str) -> Option<NavMesh> {
    let candidates = [
        format!("maps/{map_name}.nav"),
        format!("cstrike/maps/{map_name}.nav"),
    ];
    for path in candidates {
        if let Ok(bytes) = vfs.read_bytes(&path) {
            match NavFile::parse(&bytes) {
                Ok(file) => return Some(NavMesh::from_nav_file(file)),
                Err(err) => {
                    tracing::warn!(%err, %path, "invalid .nav file in VFS, regenerating");
                }
            }
        }
    }
    None
}

fn build_nav_config(eng: &EngineApi<'_>, quicksave: bool, full_manual: bool) -> NavGenConfig {
    let minimal_arena = !full_manual && eng.entity_ids().len() < 12;
    if minimal_arena {
        NavGenConfig {
            max_nodes: 2048,
            compute_hiding_spots: true,
            compute_sniper_spots: false,
            compute_encounters: false,
            tag_places: true,
            ..NavGenConfig::default()
        }
    } else {
        NavGenConfig {
            max_nodes: 65_536,
            compute_hiding_spots: true,
            compute_sniper_spots: !quicksave,
            compute_encounters: !quicksave && full_manual,
            tag_places: true,
            ..NavGenConfig::default()
        }
    }
}

fn ensure_nav(g: &mut GameState, eng: &EngineApi<'_>) {
    let map_name = eng.map_name().to_string();
    if g.bots.nav_mesh.is_some() && g.bots.nav_map_name == map_name {
        return;
    }
    if let Some(vfs) = g.vfs.as_ref()
        && let Some(mesh) = try_load_nav_from_vfs(vfs, &map_name)
    {
        g.bots.nav_mesh = Some(mesh);
        g.bots.nav_map_name = map_name;
        g.bots.nav_loaded_from_file = true;
        return;
    }
    let cfg = build_nav_config(eng, g.bots.settings.bot_quicksave, false);
    let mesh = generate_nav_mesh(&EngineNavWorld(eng), &cfg);
    g.bots.nav_mesh = Some(mesh);
    g.bots.nav_map_name = map_name;
    g.bots.nav_loaded_from_file = false;
}

fn to_bot_team(team: Team) -> Option<BotTeam> {
    match team {
        Team::Terrorist => Some(BotTeam::Terrorist),
        Team::Ct => Some(BotTeam::CounterTerrorist),
        Team::Unassigned | Team::Spectator => None,
    }
}

fn to_bot_weapon(id: WeaponId) -> Option<BotWeapon> {
    Some(match id {
        WeaponId::P228 => BotWeapon::P228,
        WeaponId::Scout => BotWeapon::Scout,
        WeaponId::HeGrenade => BotWeapon::HeGrenade,
        WeaponId::Xm1014 => BotWeapon::Xm1014,
        WeaponId::C4 => BotWeapon::C4,
        WeaponId::Mac10 => BotWeapon::Mac10,
        WeaponId::Aug => BotWeapon::Aug,
        WeaponId::SmokeGrenade => BotWeapon::SmokeGrenade,
        WeaponId::Elite => BotWeapon::Elite,
        WeaponId::FiveSeven => BotWeapon::FiveSeven,
        WeaponId::Ump45 => BotWeapon::Ump45,
        WeaponId::Sg550 => BotWeapon::Sg550,
        WeaponId::Galil => BotWeapon::Galil,
        WeaponId::Famas => BotWeapon::Famas,
        WeaponId::Usp => BotWeapon::Usp,
        WeaponId::Glock18 | WeaponId::Glock => BotWeapon::Glock18,
        WeaponId::Awp => BotWeapon::Awp,
        WeaponId::Mp5n => BotWeapon::Mp5n,
        WeaponId::M249 => BotWeapon::M249,
        WeaponId::M3 => BotWeapon::M3,
        WeaponId::M4a1 => BotWeapon::M4a1,
        WeaponId::Tmp => BotWeapon::Tmp,
        WeaponId::G3sg1 => BotWeapon::G3sg1,
        WeaponId::Flashbang => BotWeapon::Flashbang,
        WeaponId::Deagle => BotWeapon::Deagle,
        WeaponId::Sg552 => BotWeapon::Sg552,
        WeaponId::Ak47 => BotWeapon::Ak47,
        WeaponId::Knife => BotWeapon::Knife,
        WeaponId::P90 => BotWeapon::P90,
        WeaponId::ShieldGun => BotWeapon::ShieldGun,
        WeaponId::None => return None,
    })
}

fn to_weapon_id(w: BotWeapon) -> WeaponId {
    match w {
        BotWeapon::Glock18 => WeaponId::Glock18,
        BotWeapon::Usp => WeaponId::Usp,
        BotWeapon::P228 => WeaponId::P228,
        BotWeapon::Deagle => WeaponId::Deagle,
        BotWeapon::FiveSeven => WeaponId::FiveSeven,
        BotWeapon::Elite => WeaponId::Elite,
        BotWeapon::M3 => WeaponId::M3,
        BotWeapon::Xm1014 => WeaponId::Xm1014,
        BotWeapon::Mac10 => WeaponId::Mac10,
        BotWeapon::Tmp => WeaponId::Tmp,
        BotWeapon::Mp5n => WeaponId::Mp5n,
        BotWeapon::Ump45 => WeaponId::Ump45,
        BotWeapon::P90 => WeaponId::P90,
        BotWeapon::Galil => WeaponId::Galil,
        BotWeapon::Famas => WeaponId::Famas,
        BotWeapon::Ak47 => WeaponId::Ak47,
        BotWeapon::M4a1 => WeaponId::M4a1,
        BotWeapon::Aug => WeaponId::Aug,
        BotWeapon::Sg552 => WeaponId::Sg552,
        BotWeapon::Scout => WeaponId::Scout,
        BotWeapon::Awp => WeaponId::Awp,
        BotWeapon::Sg550 => WeaponId::Sg550,
        BotWeapon::G3sg1 => WeaponId::G3sg1,
        BotWeapon::M249 => WeaponId::M249,
        BotWeapon::ShieldGun => WeaponId::ShieldGun,
        BotWeapon::HeGrenade => WeaponId::HeGrenade,
        BotWeapon::Flashbang => WeaponId::Flashbang,
        BotWeapon::SmokeGrenade => WeaponId::SmokeGrenade,
        BotWeapon::Knife => WeaponId::Knife,
        BotWeapon::C4 => WeaponId::C4,
    }
}

fn build_inventory_snapshot(p: &player::Player) -> CombatInventory {
    let read_slot = |slot_idx: usize| -> (Option<BotWeapon>, u16, u16) {
        let Some(item) = p.items[slot_idx].first() else {
            return (None, 0, 0);
        };
        let bw = to_bot_weapon(item.weapon.id);
        let clip = item.weapon.clip.max(0) as u16;
        let reserve = usize::try_from(item.weapon.primary_ammo_type)
            .ok()
            .and_then(|t| p.ammo.get(t).copied())
            .unwrap_or(0)
            .max(0) as u16;
        (bw, clip, reserve)
    };
    let (primary, primary_clip, primary_reserve) = read_slot(PRIMARY_WEAPON_SLOT);
    let (secondary, secondary_clip, secondary_reserve) = read_slot(PISTOL_SLOT);
    let active_weapon = p.active_item.and_then(to_bot_weapon);
    let ammo_of = |t: AmmoType| -> u8 {
        p.ammo.get(t as usize).copied().unwrap_or(0).clamp(0, 255) as u8
    };
    CombatInventory {
        primary,
        primary_clip,
        primary_reserve,
        secondary,
        secondary_clip,
        secondary_reserve,
        active_weapon,
        has_knife: !p.items[KNIFE_SLOT].is_empty(),
        has_shield: p.owns_shield,
        shield_drawn: p.shield_drawn,
        he_count: ammo_of(AmmoType::HeGrenade),
        flash_count: ammo_of(AmmoType::Flashbang),
        smoke_count: ammo_of(AmmoType::SmokeGrenade),
    }
}

fn select_bot_profile_and_name(
    g: &mut GameState,
    eng: &EngineApi<'_>,
    requested_diff: Option<BotDifficulty>,
    requested_name: Option<&str>,
) -> (BotProfile, String) {
    let diff = requested_diff.unwrap_or(g.bots.settings.bot_difficulty);
    let max_clients = usize::from(eng.max_clients());
    let prefix = g.bots.settings.bot_prefix.trim();
    let active_names: Vec<String> = (0..max_clients)
        .filter_map(|s| eng.client_key(s as u8, "name").map(str::to_string))
        .collect();
    let stripped_names: Vec<String> = active_names
        .iter()
        .map(|n| {
            if !prefix.is_empty() && n.starts_with(prefix) {
                n[prefix.len()..].trim().to_string()
            } else {
                n.clone()
            }
        })
        .collect();
    let used_refs: Vec<&str> = stripped_names.iter().map(String::as_str).collect();

    let profile = if let Some(req) = requested_name.filter(|s| !s.trim().is_empty()) {
        g.bots
            .profile_db
            .find_by_name(req)
            .cloned()
            .unwrap_or_else(|| BotProfile::fallback(req, diff))
    } else {
        let seed = u64::from(g.rng) ^ ((eng.time() * 1000.0) as u64);
        g.rng = g.rng.wrapping_add(1);
        g.bots.profile_db.select_profile(diff, &used_refs, seed)
    };

    let base_name = requested_name
        .filter(|s| !s.trim().is_empty())
        .map_or_else(|| profile.name.clone(), ToString::to_string);

    let full_name = if prefix.is_empty() {
        base_name
    } else {
        format!("{prefix} {base_name}")
    };

    (profile, full_name)
}

/// Spawns one fake-client bot, joins a team + class, and queues it to spawn.
pub(crate) fn add_bot(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    team_pref: Option<Team>,
    arg1: Option<&str>,
    arg2: Option<&str>,
) -> Option<u8> {
    sync_settings_from_cvars(g, eng);
    if !g.bots.settings.bot_enable {
        return None;
    }

    let eff_team_pref = team_pref.or(match g.bots.settings.bot_join_team {
        BotJoinTeam::Terrorist => Some(Team::Terrorist),
        BotJoinTeam::CounterTerrorist => Some(Team::Ct),
        BotJoinTeam::Any => None,
    });

    // Support `bot_add [easy|normal|hard|expert] [name]` as well as `bot_add <name>`.
    let (req_diff, req_name) = match (arg1, arg2) {
        (Some(a1), Some(a2)) => (BotDifficulty::parse(a1), Some(a2)),
        (Some(a1), None) => match BotDifficulty::parse(a1) {
            Some(d) => (Some(d), None),
            None => (None, Some(a1)),
        },
        (None, _) => (None, None),
    };

    // Settle the team before creating the fake client so a full team costs no slot.
    let team = teams::bot_team_slot(g, eng, eff_team_pref)?;
    let (profile, chosen_name) = select_bot_profile_and_name(g, eng, req_diff, req_name);

    let slot = eng.create_fake_client(&chosen_name)?;
    player::put_in_server(g, eng, slot);

    if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        p.joining = JoinState::PickingTeam;
    }

    let choice = if team == Team::Terrorist {
        MENU_SLOT_TEAM_TERRORIST
    } else {
        MENU_SLOT_TEAM_CT
    };

    if !handle_menu_choose_team(g, eng, slot, choice) {
        eng.kick(slot, "Could not join team");
        return None;
    }

    let skin = if (1..=4).contains(&profile.skin) {
        i32::from(profile.skin)
    } else {
        g.random_range(1, 4)
    };
    handle_menu_choose_appearance(g, eng, slot, skin);

    let now = eng.time();
    let seed = u64::from(g.rng) ^ u64::from(slot);
    g.bots.slots[usize::from(slot)] = Some(BotSlot::new(now, profile, &g.bots.settings, seed));
    Some(slot)
}

/// Kicks matching bot(s): `"all"`/`None`, `"t"`, `"ct"`, or a bot's name.
pub(crate) fn kick_bots(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    target: Option<&str>,
) -> usize {
    let filter = target.map(str::trim).filter(|s| !s.is_empty());
    let mut kicked = 0;
    let max_clients = usize::from(eng.max_clients());
    for slot in 0..max_clients.min(MAX_PLAYERS) {
        let slot_u8 = slot as u8;
        if !eng.is_fake_client(slot_u8) || g.bots.slots[slot].is_none() {
            continue;
        }
        let team = g.players[slot].as_ref().map(|p| p.team);
        let name = eng.client_key(slot_u8, "name").unwrap_or("");
        let matches = match filter {
            None => true,
            Some(f) if f.eq_ignore_ascii_case("all") => true,
            Some(f) if f.eq_ignore_ascii_case("t") => team == Some(Team::Terrorist),
            Some(f) if f.eq_ignore_ascii_case("ct") => team == Some(Team::Ct),
            Some(f) => name.eq_ignore_ascii_case(f),
        };
        if matches {
            eng.kick(slot_u8, "Kicked");
            kicked += 1;
        }
    }
    kicked
}

/// Slays matching living bot(s): `"all"`/`None`, `"t"`, `"ct"`, or a bot's name.
pub(crate) fn kill_bots(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    target: Option<&str>,
) -> usize {
    let filter = target.map(str::trim).filter(|s| !s.is_empty());
    let mut killed = 0;
    let max_clients = usize::from(eng.max_clients());
    for slot in 0..max_clients.min(MAX_PLAYERS) {
        let slot_u8 = slot as u8;
        if !eng.is_fake_client(slot_u8) || g.bots.slots[slot].is_none() {
            continue;
        }
        let Some(p) = g.players[slot].as_ref() else {
            continue;
        };
        if !p.alive {
            continue;
        }
        let team = p.team;
        let name = eng.client_key(slot_u8, "name").unwrap_or("");
        let matches = match filter {
            None => true,
            Some(f) if f.eq_ignore_ascii_case("all") => true,
            Some(f) if f.eq_ignore_ascii_case("t") => team == Team::Terrorist,
            Some(f) if f.eq_ignore_ascii_case("ct") => team == Team::Ct,
            Some(f) => name.eq_ignore_ascii_case(f),
        };
        if matches {
            player::client_command(g, eng, slot_u8, &["kill".to_string()]);
            killed += 1;
        }
    }
    killed
}

fn maintain_quota(g: &mut GameState, eng: &mut EngineApi<'_>) {
    if !g.bots.settings.bot_enable {
        return;
    }
    let max_clients = usize::from(eng.max_clients()).min(MAX_PLAYERS);
    let mut humans = 0usize;
    let mut bots = 0usize;
    for (slot, player) in g.players.iter().enumerate().take(max_clients) {
        if player.is_none() {
            continue;
        }
        if eng.is_fake_client(slot as u8) {
            bots += 1;
        } else {
            humans += 1;
        }
    }

    if g.bots.settings.bot_join_after_player && humans == 0 {
        return;
    }

    let raw_quota = if g.bots.settings.bot_quota_match > 0.0 {
        ((humans as f32) * g.bots.settings.bot_quota_match).round().max(0.0) as usize
    } else {
        eng.cvar_f32("bot_quota").max(0.0) as usize
    };
    let desired = raw_quota.min(max_clients.saturating_sub(humans));

    if bots < desired {
        add_bot(g, eng, None, None, None);
    } else if bots > desired && (g.bots.settings.bot_auto_vacate || humans + bots > max_clients) {
        for slot in (0..max_clients).rev() {
            if eng.is_fake_client(slot as u8) && g.bots.slots[slot].is_some() {
                eng.kick(slot as u8, "Bot quota reduced");
                break;
            }
        }
    }
}

fn team_average_money(g: &GameState, team: Team) -> i32 {
    let mut sum = 0i32;
    let mut count = 0i32;
    for p in g.players.iter().flatten() {
        if p.team == team {
            sum += p.money;
            count += 1;
        }
    }
    if count > 0 { sum / count } else { 800 }
}

fn maybe_buy(g: &mut GameState, eng: &mut EngineApi<'_>, slot: usize) {
    let Some(bot) = g.bots.slots[slot].as_ref() else {
        return;
    };
    if bot.done_buying {
        return;
    }
    let Some(p) = g.players[slot].as_ref() else {
        return;
    };
    if !p.alive || p.joining != JoinState::Joined || p.signals.state() & SIGNAL_BUY == 0 {
        return;
    }
    let Some(bot_team) = to_bot_team(p.team) else {
        return;
    };

    let inv = build_inventory_snapshot(p);
    let total_rounds = g.rules.total_rounds_played().max(0) as u32;
    let consecutive_losses = match p.team {
        Team::Ct => g.rules.economy().consecutive_ct_loses().max(0) as u32,
        Team::Terrorist => g.rules.economy().consecutive_terrorist_loses().max(0) as u32,
        _ => 0,
    };
    let team_avg_money = team_average_money(g, p.team);
    let seed = u64::from(g.rng) ^ ((eng.time() * 100.0) as u64).wrapping_add(slot as u64);

    let plan: BuyPlan = {
        let ctx = BuyContext {
            team: bot_team,
            money: p.money,
            round_number: total_rounds,
            consecutive_losses,
            team_avg_money,
            is_pistol_round: total_rounds == 0 && p.money <= 1000,
            is_bomb_map: g.rules.map_has_bomb_target(),
            is_vip_map: g.rules.map_has_vip_safety_zone(),
            is_vip: p.is_vip,
            has_primary: inv.primary,
            has_secondary: inv.secondary,
            has_shield: p.owns_shield,
            armor: p.armor,
            has_helmet: p.kevlar == player::ArmorType::VestHelm,
            has_defuser: p.has_defuser,
            he_count: inv.he_count,
            flash_count: inv.flash_count,
            smoke_count: inv.smoke_count,
            needs_primary_ammo: inv.primary.is_some(),
            needs_secondary_ammo: inv.secondary.is_some(),
            profile: &bot.profile,
            restrictions: &g.bots.settings.weapon_restrictions,
            seed,
        };
        decide_bot_buy(&ctx)
    };

    let slot_u8 = slot as u8;
    for item in &plan.items {
        player::client_command(g, eng, slot_u8, &[item.to_buy_command().to_string()]);
    }

    let now_primary = g.players[slot]
        .as_ref()
        .is_some_and(|p| p.has_primary || p.owns_shield);
    if let Some(bot) = g.bots.slots[slot].as_mut() {
        bot.buy_retries += 1;
        if plan.items.is_empty() || now_primary || bot.buy_retries >= 3 {
            bot.done_buying = true;
        }
    }
}

fn collect_entities_floor(eng: &EngineApi<'_>, brush_class: &str, point_class: Option<&str>) -> Vec<Vec3> {
    let mut out = Vec::new();
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, brush_class) {
        if let Some(v) = eng.ent(id) {
            out.push(zone_floor_point(v.absmin, v.absmax));
        }
        cursor = Some(id);
    }
    if let Some(pc) = point_class {
        let mut cursor = None;
        while let Some(id) = eng.find_by_classname(cursor, pc) {
            if let Some(v) = eng.ent(id) {
                out.push(v.origin);
            }
            cursor = Some(id);
        }
    }
    out
}

fn find_planted_c4(eng: &EngineApi<'_>, now_f32: f32) -> (Option<Vec3>, Option<f32>) {
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "grenade") {
        if let Some(ent) = eng.private::<Box<dyn GameEntity>>(id)
            && ent.is_planted_live_c4()
            && let Some(v) = eng.ent(id)
        {
            let center = if v.size.length_squared() > 0.0 {
                v.absmin + v.size * 0.5
            } else {
                v.origin
            };
            let rem = ent.c4_blow_time().map(|blow| (blow - now_f32).max(0.0));
            return (Some(center), rem);
        }
        cursor = Some(id);
    }
    (None, None)
}

fn find_dropped_c4(eng: &EngineApi<'_>) -> Option<Vec3> {
    for id in eng.entity_ids() {
        if let Some(ent) = eng.private::<Box<dyn GameEntity>>(id)
            && let Some(v) = eng.ent(id)
            && let Some(pos) = ent.dropped_bomb_origin(v.origin)
        {
            return Some(pos);
        }
    }
    None
}

fn find_smoke_clouds(eng: &EngineApi<'_>) -> Vec<SmokeCloud> {
    let mut clouds = Vec::new();
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "grenade") {
        if let Some(ent) = eng.private::<Box<dyn GameEntity>>(id)
            && let Some(center) = ent.smoke_cloud_center()
        {
            clouds.push(SmokeCloud {
                center,
                radius: 140.0,
            });
        }
        cursor = Some(id);
    }
    clouds
}

fn collect_obstacles(eng: &EngineApi<'_>) -> PathObstacles {
    let mut obs = PathObstacles::default();
    for class in ["func_door", "func_door_rotating"] {
        let mut cursor = None;
        while let Some(id) = eng.find_by_classname(cursor, class) {
            if let Some(v) = eng.ent(id)
                && v.solid != 0
            {
                obs.doors.push((v.absmin, v.absmax));
            }
            cursor = Some(id);
        }
    }
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "func_breakable") {
        if let Some(v) = eng.ent(id)
            && v.solid != 0
            && v.health > 0.0
        {
            obs.breakables.push((v.absmin, v.absmax));
        }
        cursor = Some(id);
    }
    obs
}

fn find_hostages(g: &GameState, eng: &EngineApi<'_>) -> Vec<TacticalHostage> {
    let mut out = Vec::new();
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "hostage_entity") {
        if let Some(e) = eng.private::<Box<dyn GameEntity>>(id)
            && e.is_hostage()
            && e.is_alive(eng, id)
            && !e.is_hostage_rescued()
            && let Some(v) = eng.ent(id)
        {
            out.push(TacticalHostage {
                id: u32::from(id.index),
                pos: v.origin,
                follower_id: e
                    .hostage_follower()
                    .and_then(|fid| player::player_slot(g, eng, fid))
                    .map(|s| s as u32),
                rescued: false,
            });
        }
        cursor = Some(id);
    }
    out
}

fn find_vip_pos(g: &GameState, eng: &EngineApi<'_>, max_clients: usize) -> Option<Vec3> {
    for (slot, slot_player) in g.players.iter().enumerate().take(max_clients) {
        if let Some(p) = slot_player.as_ref()
            && p.alive
            && p.is_vip
            && let Some(v) = eng.ent(eng.client_edict(slot as u8))
        {
            return Some(v.origin);
        }
    }
    None
}

fn gather_enemy_candidates(
    g: &GameState,
    eng: &EngineApi<'_>,
    my_slot: usize,
    my_team: Team,
    my_eye: Vec3,
    max_clients: usize,
) -> Vec<PerceivedEnemyCandidate> {
    let mut out = Vec::new();
    for (other, slot_player) in g.players.iter().enumerate().take(max_clients) {
        if other == my_slot {
            continue;
        }
        let Some(op) = slot_player.as_ref() else {
            continue;
        };
        if !op.alive || !op.team.plays() || op.team == my_team {
            continue;
        }
        let oid = eng.client_edict(other as u8);
        if let Some(ov) = eng.ent(oid)
            && ov.deadflag == DEAD_NO
            && (ov.flags & cs_engine::consts::FL_NOTARGET) == 0
        {
            let enemy_eye = ov.origin + ov.view_ofs;
            let aim = ov.origin.lerp(enemy_eye, 0.72);
            let has_los = eng
                .trace_line(my_eye, aim, MoveFilter::NoMonsters, None)
                .trace
                .fraction
                >= 1.0;
            out.push(PerceivedEnemyCandidate {
                id: other as u32,
                pos: ov.origin,
                eye: enemy_eye,
                vel: ov.velocity,
                has_los,
                has_primary: op.has_primary,
                is_vip: op.is_vip,
            });
        }
    }
    out
}

fn find_cover_spots_near(
    mesh: Option<&NavMesh>,
    origin: Vec3,
    focus_target: Vec3,
) -> Vec<TacticalSpot> {
    let Some(mesh) = mesh else {
        return Vec::new();
    };
    let flags = HidingSpotFlags::IN_COVER
        | HidingSpotFlags::GOOD_SNIPER_SPOT
        | HidingSpotFlags::IDEAL_SNIPER_SPOT;
    mesh.find_hiding_spots(origin, 1400.0, flags, None, true)
        .into_iter()
        .take(16)
        .map(|s| TacticalSpot {
            pos: s.pos,
            look_at: focus_target,
            is_sniper_spot: s
                .flags
                .intersects(HidingSpotFlags::GOOD_SNIPER_SPOT | HidingSpotFlags::IDEAL_SNIPER_SPOT),
            is_cover: s.flags.contains(HidingSpotFlags::IN_COVER),
        })
        .collect()
}

fn select_desired_weapon(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, cmd: &BotCmd) {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    if cmd.want_c4 && p.has_c4 {
        if p.active_item != Some(WeaponId::C4) {
            player::client_command(g, eng, slot, &["weapon_c4".to_string()]);
        }
        return;
    }
    if let Some(desired) = cmd.desired_weapon {
        let wid = if desired == BotWeapon::ShieldGun {
            p.items[PISTOL_SLOT]
                .first()
                .or_else(|| p.items[KNIFE_SLOT].first())
                .map(|i| i.weapon.id)
        } else {
            Some(to_weapon_id(desired))
        };
        if let Some(id) = wid
            && p.active_item != Some(id)
            && p.items.iter().flatten().any(|i| i.weapon.id == id)
            && let Some(info) = item_info(id)
        {
            player::client_command(g, eng, slot, &[info.name.to_string()]);
            return;
        }
    }
    if matches!(p.active_item, Some(WeaponId::C4) | None) {
        let best = p.items[PRIMARY_WEAPON_SLOT]
            .first()
            .or_else(|| p.items[PISTOL_SLOT].first())
            .or_else(|| p.items[KNIFE_SLOT].first())
            .map(|item| item.weapon.id);
        if let Some(id) = best
            && p.active_item != Some(id)
            && let Some(info) = item_info(id)
        {
            player::client_command(g, eng, slot, &[info.name.to_string()]);
        }
    }
}

fn encode_buttons(cmd: &BotCmd) -> u16 {
    let mut b = 0i32;
    if cmd.forward > 0.1 {
        b |= IN_FORWARD;
    } else if cmd.forward < -0.1 {
        b |= IN_BACK;
    }
    if cmd.right > 0.1 {
        b |= IN_MOVERIGHT;
    } else if cmd.right < -0.1 {
        b |= IN_MOVELEFT;
    }
    if cmd.attack {
        b |= IN_ATTACK;
    }
    if cmd.secondary_attack {
        b |= IN_ATTACK2;
    }
    if cmd.jump {
        b |= IN_JUMP;
    }
    if cmd.duck {
        b |= IN_DUCK;
    }
    if cmd.use_key {
        b |= IN_USE;
    }
    if cmd.reload {
        b |= IN_RELOAD;
    }
    b as u16
}

fn world_dir_to_move(dir_xy: glam::Vec2, yaw_deg: f32) -> (f32, f32) {
    let len = dir_xy.length();
    if len < 1e-3 {
        return (0.0, 0.0);
    }
    let d = dir_xy / len;
    let rad = yaw_deg.to_radians();
    let (sy, cy) = (rad.sin(), rad.cos());
    let fwd = glam::Vec2::new(cy, sy);
    let right = glam::Vec2::new(sy, -cy);
    (d.dot(fwd).clamp(-1.0, 1.0), d.dot(right).clamp(-1.0, 1.0))
}

fn string_pull_engine_path(mesh: &NavMesh, eng: &EngineApi<'_>, path: &mut NavPath) {
    if path.waypoints.len() < 3 {
        return;
    }
    let is_pullable = |w: &cs_bot::path::PathWaypoint| {
        w.action == PathAction::Walk
            && !w.has_door
            && !w.has_breakable
            && !matches!(
                w.traverse,
                cs_bot::path::TraverseKind::JumpDown(_)
                    | cs_bot::path::TraverseKind::Jump
                    | cs_bot::path::TraverseKind::LadderUp
                    | cs_bot::path::TraverseKind::LadderDown
            )
    };

    let mut pulled = Vec::with_capacity(path.waypoints.len());
    let mut anchor = 0usize;
    pulled.push(path.waypoints[0].clone());

    while anchor + 1 < path.waypoints.len() {
        let mut furthest = anchor + 1;
        for cand in (anchor + 2)..path.waypoints.len() {
            let mid = &path.waypoints[cand - 1];
            let target = &path.waypoints[cand];
            if !is_pullable(mid) || !is_pullable(target) {
                break;
            }
            let a = path.waypoints[anchor].pos;
            let b = target.pos;
            let clear_los = eng
                .trace_line(a + Vec3::new(0.0, 0.0, 22.0), b + Vec3::new(0.0, 0.0, 22.0), MoveFilter::NoMonsters, None)
                .trace
                .fraction
                >= 1.0
                && eng
                    .trace_line(a + Vec3::new(0.0, 0.0, 54.0), b + Vec3::new(0.0, 0.0, 54.0), MoveFilter::NoMonsters, None)
                    .trace
                    .fraction
                    >= 1.0;
            if !clear_los {
                break;
            }
            let dist = a.distance(b);
            let steps = ((dist / 24.0).ceil() as usize).max(2);
            let mut corridor_ok = true;
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let p = a.lerp(b, t);
                let Some(idx) = mesh.get_nav_area(p + Vec3::new(0.0, 0.0, 12.0), 48.0) else {
                    corridor_ok = false;
                    break;
                };
                let area = &mesh.areas()[idx];
                if area.attributes.contains(NavAttribute::PRECISE)
                    || area.attributes.contains(NavAttribute::JUMP)
                    || area.attributes.contains(NavAttribute::CROUCH)
                    || mesh.is_area_blocked(idx)
                    || (area.z_at(p.x, p.y) - p.z).abs() > 36.0
                {
                    corridor_ok = false;
                    break;
                }
            }
            if corridor_ok {
                furthest = cand;
            } else {
                break;
            }
        }
        pulled.push(path.waypoints[furthest].clone());
        anchor = furthest;
    }

    path.waypoints = pulled;
}

#[allow(clippy::too_many_arguments)]
fn plan_and_steer_path(
    bot: &mut BotSlot,
    mesh: Option<&NavMesh>,
    eng: &EngineApi<'_>,
    obstacles: &PathObstacles,
    origin: Vec3,
    goal: Vec3,
    my_team: BotTeam,
    now: f64,
    yaw_deg: f32,
) -> (f32, f32, bool, bool, bool, bool) {
    let Some(mesh) = mesh.filter(|m| !m.areas().is_empty()) else {
        let (fwd, right) = world_dir_to_move((goal - origin).truncate(), yaw_deg);
        return (fwd, right, false, false, false, false);
    };

    let need_replan = bot.path.is_none()
        || bot.path_goal.is_none_or(|g| g.distance_squared(goal) > 64.0 * 64.0)
        || now >= bot.path_replan_time;

    if need_replan {
        let params = PathCostParams {
            team: usize::from(my_team == BotTeam::CounterTerrorist),
            now,
            danger_weight: 150.0 * (1.0 - bot.profile.aggression * 0.5),
            ..PathCostParams::default()
        };
        if let Some(mut p) = find_path(mesh, origin, goal, &params, obstacles) {
            string_pull_engine_path(mesh, eng, &mut p);
            bot.path_wp_idx = usize::from(p.waypoints.len() > 1);
            bot.path = Some(p);
        } else {
            bot.path = None;
            bot.path_wp_idx = 0;
        }
        bot.path_goal = Some(goal);
        bot.path_replan_time = now + 1.0;
    }

    let Some(path) = bot.path.as_ref() else {
        let (fwd, right) = world_dir_to_move((goal - origin).truncate(), yaw_deg);
        return (fwd, right, false, false, false, false);
    };

    while bot.path_wp_idx + 1 < path.waypoints.len() {
        let wp = &path.waypoints[bot.path_wp_idx];
        let tol = match wp.action {
            PathAction::LadderTraverse { .. } => 24.0,
            _ => 34.0,
        };
        if origin.truncate().distance(wp.pos.truncate()) <= tol
            && (origin.z - wp.pos.z).abs() < 64.0
        {
            bot.path_wp_idx += 1;
        } else {
            break;
        }
    }

    let Some(wp) = path.waypoints.get(bot.path_wp_idx) else {
        let (fwd, right) = world_dir_to_move((goal - origin).truncate(), yaw_deg);
        return (fwd, right, false, false, false, false);
    };

    if origin.truncate().distance(goal.truncate()) < 22.0 && (origin.z - goal.z).abs() < 48.0 {
        return (0.0, 0.0, false, false, false, false);
    }

    let (fwd, right) = world_dir_to_move((wp.pos - origin).truncate(), yaw_deg);
    let crouch = matches!(wp.action, PathAction::CrouchRequired);
    let jump = matches!(wp.action, PathAction::JumpRequired)
        && origin.truncate().distance(wp.pos.truncate()) < 72.0;
    let ladder_up = matches!(
        wp.action,
        PathAction::LadderTraverse {
            ascending: true,
            ..
        }
    );
    let door_use = wp.has_door && origin.distance(wp.pos) < 88.0;

    (fwd, right, crouch, jump, door_use, ladder_up)
}

/// Emits an acoustic event into the ZBot world perception buffer.
pub(crate) fn emit_sound_event(
    g: &mut GameState,
    source_slot: Option<u8>,
    team: Option<Team>,
    pos: Vec3,
    kind: SoundKind,
    now: f64,
) {
    if g.bots.slots.iter().all(Option::is_none) {
        return;
    }
    if g.bots.sound_events.len() >= MAX_SOUND_EVENTS {
        g.bots.sound_events.remove(0);
    }
    g.bots.sound_events.push(SoundEvent {
        source_id: source_slot.map(u32::from),
        team: team.and_then(to_bot_team),
        pos,
        kind,
        radius: kind.default_radius(),
        time: now as f32,
    });
}

/// Propagates navmesh danger heat when a player is killed.
pub(crate) fn on_player_killed(g: &mut GameState, eng: &EngineApi<'_>, victim_slot: u8) {
    let Some(victim_team) = g
        .players
        .get(usize::from(victim_slot))
        .and_then(Option::as_ref)
        .and_then(|p| to_bot_team(p.team))
    else {
        return;
    };
    let Some(pos) = eng.ent(eng.client_edict(victim_slot)).map(|v| v.origin) else {
        return;
    };
    let now = eng.time();
    if let Some(mesh) = g.bots.nav_mesh.as_mut()
        && let Some(area_idx) = mesh.get_nearest_nav_area(pos, true)
    {
        let team_idx = usize::from(victim_team == BotTeam::CounterTerrorist);
        mesh.increase_danger_nearby(team_idx, 1.0, area_idx, pos, 600.0, now);
    }
}

/// Delivers a human or bot radio call to teammate ZBot brains.
pub(crate) fn on_player_radio(g: &mut GameState, eng: &EngineApi<'_>, slot: u8, alias: &str) {
    let Some(message) = RadioMessage::from_alias(alias) else {
        return;
    };
    let Some(sender_team) = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .and_then(|p| to_bot_team(p.team))
    else {
        return;
    };
    let sender_pos = eng
        .ent(eng.client_edict(slot))
        .map_or(Vec3::ZERO, |v| v.origin);
    let sender_place = g
        .bots
        .nav_mesh
        .as_ref()
        .and_then(|m| m.get_place(sender_pos))
        .map(ToString::to_string);
    let is_human = !eng.is_fake_client(slot);
    if g.bots.incoming_radios.len() >= MAX_RADIO_EVENTS {
        g.bots.incoming_radios.remove(0);
    }
    g.bots.incoming_radios.push(IncomingRadio {
        sender_id: u32::from(slot),
        sender_team,
        sender_pos,
        sender_place,
        message,
        time: eng.time() as f32,
        is_human,
    });
}

fn dispatch_bot_radio(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    team: BotTeam,
    pos: Vec3,
    pitch: u16,
    out_radio: &cs_bot::tactics::OutgoingRadio,
) {
    if let Some((sentence, text)) = crate::hud_misc::radio_payload(out_radio.message.alias()) {
        crate::hud_misc::radio_with_place(
            g,
            eng,
            slot,
            sentence,
            text,
            out_radio.place_name.as_deref(),
            i16::try_from(pitch).unwrap_or(PITCH_NORM as i16),
        );
    }
    if g.bots.incoming_radios.len() >= MAX_RADIO_EVENTS {
        g.bots.incoming_radios.remove(0);
    }
    g.bots.incoming_radios.push(IncomingRadio {
        sender_id: u32::from(slot),
        sender_team: team,
        sender_pos: pos,
        sender_place: out_radio.place_name.clone(),
        message: out_radio.message,
        time: eng.time() as f32,
        is_human: false,
    });
}

/// Drives bot quota maintenance, 30 Hz command generation, and 10 Hz ZBot AI updates every
/// server frame (`CCSBotManager::StartFrame`).
pub(crate) fn start_frame(g: &mut GameState, eng: &mut EngineApi<'_>) {
    sync_settings_from_cvars(g, eng);
    let now = eng.time();
    if g.bots.quota_enabled && now >= g.bots.next_quota_check {
        g.bots.next_quota_check = now + 0.5;
        maintain_quota(g, eng);
    }

    // A server without bots (every human match) pays nothing here: no navmesh generation, no
    // scan of the objective entities.
    let max_clients = usize::from(eng.max_clients()).min(MAX_PLAYERS);
    if !(0..max_clients).any(|slot| eng.is_fake_client(slot as u8)) {
        return;
    }

    ensure_nav(g, eng);

    let now_f32 = now as f32;
    g.bots.sound_events.retain(|s| now_f32 - s.time <= 2.5);
    g.bots.incoming_radios.retain(|r| now_f32 - r.time <= 4.0);

    // Emit periodic footstep sound events for sprinting players.
    for slot in 0..max_clients {
        let Some(p) = g.players[slot].as_ref() else {
            continue;
        };
        if !p.alive {
            continue;
        }
        if let Some(v) = eng.ent(eng.client_edict(slot as u8))
            && (v.flags & FL_ONGROUND) != 0
            && (v.flags & FL_DUCKING) == 0
            && v.velocity.truncate().length_squared() > 155.0 * 155.0
            && ((now * 100.0) as u64 + slot as u64).is_multiple_of(30)
        {
            emit_sound_event(g, Some(slot as u8), Some(p.team), v.origin, SoundKind::Footstep, now);
        }
    }

    let bomb_sites = collect_entities_floor(eng, "func_bomb_target", Some("info_bomb_target"));
    let (planted_pos, bomb_rem) = find_planted_c4(eng, now_f32);
    let dropped_bomb_pos = if planted_pos.is_none() {
        find_dropped_c4(eng)
    } else {
        None
    };
    let hostages = find_hostages(g, eng);
    let rescue_zones = if hostages.is_empty() {
        Vec::new()
    } else {
        let mut zones = collect_entities_floor(eng, "func_hostage_rescue", Some("info_hostage_rescue"));
        if zones.is_empty() {
            zones = collect_entities_floor(eng, "info_player_start", None);
        }
        zones
    };
    let vip_safety_zones = collect_entities_floor(eng, "func_vip_safetyzone", None);
    let vip_pos = if vip_safety_zones.is_empty() {
        None
    } else {
        find_vip_pos(g, eng, max_clients)
    };
    let smokes = find_smoke_clouds(eng);
    let obstacles = collect_obstacles(eng);

    let mut patrol_waypoints = Vec::new();
    patrol_waypoints.extend_from_slice(&bomb_sites);
    patrol_waypoints.extend_from_slice(&vip_safety_zones);
    patrol_waypoints.extend(hostages.iter().map(|h| h.pos));
    if patrol_waypoints.is_empty() {
        patrol_waypoints.extend(collect_entities_floor(eng, "info_player_start", None));
        patrol_waypoints.extend(collect_entities_floor(eng, "info_player_deathmatch", None));
    }

    for slot in 0..max_clients {
        let slot_u8 = slot as u8;
        if !eng.is_fake_client(slot_u8) {
            continue;
        }
        let Some(next_cmd) = g.bots.slots[slot].as_ref().map(|b| b.next_cmd_time) else {
            continue;
        };
        if now < next_cmd {
            continue;
        }

        let Some((alive, joining, my_team)) = g.players[slot]
            .as_ref()
            .map(|p| (p.alive, p.joining, p.team))
        else {
            continue;
        };

        if let Some(b) = g.bots.slots[slot].as_mut() {
            b.next_cmd_time = now + 1.0 / 30.0;
        }

        if !alive || joining != JoinState::Joined {
            let prev = g.bots.slots[slot].as_ref().map_or(now, |b| b.prev_cmd_time);
            let msec = ((now - prev) * 1000.0).clamp(1.0, 255.0) as u8;
            if let Some(b) = g.bots.slots[slot].as_mut() {
                b.prev_cmd_time = now;
            }
            eng.run_player_move(slot_u8, Vec3::ZERO, 0.0, 0.0, 0.0, 0, 0, msec);
            continue;
        }

        maybe_buy(g, eng, slot);

        let full_due = g.bots.slots[slot]
            .as_ref()
            .is_some_and(|b| now >= b.next_full_think);
        if full_due {
            let id = eng.client_edict(slot_u8);
            let (Some(hv), Some(p)) = (eng.ent(id), g.players[slot].as_ref()) else {
                continue;
            };
            let Some(bot_team) = to_bot_team(my_team) else {
                continue;
            };

            let eye = hv.origin + hv.view_ofs;
            let inv = build_inventory_snapshot(p);
            let enemy_candidates = gather_enemy_candidates(g, eng, slot, my_team, eye, max_clients);
            let place_name = g
                .bots
                .nav_mesh
                .as_ref()
                .and_then(|m| m.get_place(hv.origin))
                .map(ToString::to_string);
            let focus_pt = planted_pos
                .or_else(|| bomb_sites.first().copied())
                .or_else(|| hostages.first().map(|h| h.pos))
                .or(vip_pos)
                .unwrap_or(hv.origin + Vec3::X * 300.0);
            let cover_spots = find_cover_spots_near(g.bots.nav_mesh.as_ref(), hv.origin, focus_pt);

            let (human_teammate_alive, human_near_objective) = (0..max_clients)
                .filter(|&s| !eng.is_fake_client(s as u8))
                .filter_map(|s| g.players[s].as_ref().map(|hp| (s, hp)))
                .filter(|(_, hp)| hp.alive && hp.team == my_team)
                .fold((false, false), |(_, near), (s, _)| {
                    let h_pos = eng.ent(eng.client_edict(s as u8)).map_or(Vec3::ZERO, |v| v.origin);
                    let close_to_obj = bomb_sites.iter().any(|b| b.distance(h_pos) < 350.0)
                        || dropped_bomb_pos.is_some_and(|d| d.distance(h_pos) < 350.0)
                        || hostages.iter().any(|h| h.pos.distance(h_pos) < 350.0);
                    (true, near || close_to_obj)
                });

            let seed = u64::from(g.rng) ^ ((now * 100.0) as u64).wrapping_add(slot as u64);
            let flash_alpha = if now_f32 < p.blind_until_time {
                ((p.blind_until_time - now_f32) / 2.5).clamp(0.6, 1.0)
            } else {
                0.0
            };

            let (mut brain, profile, hostage_use_cooldown) = {
                let b = g.bots.slots[slot].as_ref().unwrap();
                (b.brain.clone(), b.profile.clone(), b.hostage_use_cooldown)
            };

            let decision: TacticalDecision = {
                let ctx = TacticalContext {
                    self_id: slot as u32,
                    team: bot_team,
                    now: now_f32,
                    dt: 0.1,
                    pos: hv.origin,
                    eye,
                    vel: hv.velocity,
                    yaw_deg: hv.v_angle.y,
                    pitch_deg: hv.v_angle.x,
                    punch_angle_deg: hv.punchangle,
                    fov_deg: if p.fov == 0 { 90 } else { p.fov },
                    on_ground: (hv.flags & FL_ONGROUND) != 0,
                    is_crouching: (hv.flags & FL_DUCKING) != 0,
                    health: p.health,
                    flash_alpha,
                    place_name: place_name.as_deref(),
                    inventory: inv,
                    profile: &profile,
                    settings: &g.bots.settings,
                    enemy_candidates: &enemy_candidates,
                    smokes: &smokes,
                    sounds: &g.bots.sound_events,
                    incoming_radio: &g.bots.incoming_radios,
                    human_teammate_alive,
                    human_near_objective,
                    has_c4: p.has_c4,
                    in_bomb_zone: (p.signals.state() & SIGNAL_BOMB) != 0,
                    bomb_sites: &bomb_sites,
                    dropped_bomb_pos,
                    planted_bomb_pos: planted_pos,
                    bomb_time_remaining: bomb_rem,
                    hostages: &hostages,
                    rescue_zones: &rescue_zones,
                    is_vip: p.is_vip,
                    vip_pos,
                    vip_safety_zones: &vip_safety_zones,
                    cover_spots: &cover_spots,
                    patrol_waypoints: &patrol_waypoints,
                    rng_seed: seed,
                };
                brain.step(&ctx)
            };

            let mut cmd = BotCmd {
                yaw_deg: decision.combat.yaw_deg,
                pitch_deg: decision.combat.pitch_deg,
                forward: 0.0,
                right: 0.0,
                attack: decision.combat.primary_attack,
                secondary_attack: decision.combat.secondary_attack,
                jump: false,
                duck: decision.duck,
                use_key: decision.use_key,
                reload: decision.combat.reload,
                walk: decision.walk,
                desired_weapon: decision.combat.desired_weapon,
                want_c4: decision.want_c4,
            };

            // Plant C4 by holding +attack once inside bomb site.
            if decision.want_c4 && p.has_c4 && (p.signals.state() & SIGNAL_BOMB) != 0 {
                cmd.attack = true;
                cmd.forward = 0.0;
                cmd.right = 0.0;
            } else if let Some(goal) = decision.move_goal {
                let mut temp_bot = g.bots.slots[slot].take().unwrap();
                let (mut fwd, mut right, path_crouch, path_jump, door_use, ladder_up) =
                    plan_and_steer_path(
                        &mut temp_bot,
                        g.bots.nav_mesh.as_ref(),
                        eng,
                        &obstacles,
                        hv.origin,
                        goal,
                        bot_team,
                        now,
                        cmd.yaw_deg,
                    );
                g.bots.slots[slot] = Some(temp_bot);

                if decision.combat.want_stop_to_shoot
                    && decision.visible_enemy_count > 0
                    && !matches!(
                        decision.state,
                        BotTacticalState::EscapeFromBomb | BotTacticalState::EscortVip
                    )
                {
                    fwd = 0.0;
                    right = 0.0;
                } else if decision.visible_enemy_count > 0
                    && decision.combat.strafe_bias.abs() > 0.01
                {
                    right = (right + decision.combat.strafe_bias).clamp(-1.0, 1.0);
                }

                cmd.forward = fwd;
                cmd.right = right;
                cmd.duck |= path_crouch;
                cmd.jump |= path_jump;
                cmd.use_key |= door_use;
                if ladder_up && decision.visible_enemy_count == 0 {
                    cmd.pitch_deg = -45.0;
                }
            }

            // Defuse C4: aim directly at the planted bomb so `+use` passes VIEW_FIELD_NARROW.
            if decision.state == BotTacticalState::DefuseBomb
                && decision.visible_enemy_count == 0
                && let Some(bpos) = planted_pos
                && hv.origin.distance(bpos) < 96.0
            {
                let (by, bp) = cs_bot::tactics::yaw_pitch_to(eye, bpos);
                cmd.yaw_deg = by;
                cmd.pitch_deg = bp;
                if cmd.use_key {
                    cmd.forward = 0.0;
                    cmd.right = 0.0;
                }
            }

            // Hostage escort pacing & +use pulse cooldown.
            let mut pulse_hostage_use = false;
            if decision.state == BotTacticalState::RescueHostage && decision.visible_enemy_count == 0 {
                let following_me: Vec<&TacticalHostage> = hostages
                    .iter()
                    .filter(|h| h.follower_id == Some(slot as u32))
                    .collect();
                if !following_me.is_empty() {
                    let max_dist = following_me
                        .iter()
                        .map(|h| h.pos.distance(hv.origin))
                        .fold(0.0_f32, f32::max);
                    if max_dist > 220.0
                        || rescue_zones.iter().any(|rz| hv.origin.distance(*rz) < 64.0)
                    {
                        cmd.forward = 0.0;
                        cmd.right = 0.0;
                    }
                } else if cmd.use_key {
                    if now >= hostage_use_cooldown {
                        pulse_hostage_use = true;
                        cmd.forward = 0.0;
                        cmd.right = 0.0;
                        if let Some(h) = hostages
                            .iter()
                            .min_by(|a, b| a.pos.distance_squared(hv.origin).total_cmp(&b.pos.distance_squared(hv.origin)))
                        {
                            let (hy, hp) = cs_bot::tactics::yaw_pitch_to(eye, h.pos + Vec3::new(0.0, 0.0, 31.0));
                            cmd.yaw_deg = hy;
                            cmd.pitch_deg = hp;
                        }
                    } else {
                        cmd.use_key = false;
                    }
                }
            }

            // Stuck detection and detour recovery.
            let mut want_stuck_recenter = false;
            let mut stuck_tick_val = 0usize;
            if let Some(b) = g.bots.slots[slot].as_mut() {
                let moving_unblocked = (hv.flags & FL_FROZEN) == 0
                    && hv.max_speed > 5.0
                    && !cmd.use_key
                    && !(cmd.want_c4 && cmd.attack)
                    && (cmd.forward.abs() > 0.2 || cmd.right.abs() > 0.2);

                if moving_unblocked {
                    let moved = hv.origin.truncate().distance(b.last_pos.truncate());
                    if moved < 4.0 {
                        b.stuck_ticks = b.stuck_ticks.saturating_add(1);
                    } else {
                        b.stuck_ticks = 0;
                    }
                    if b.stuck_ticks >= 4 {
                        cmd.jump = b.stuck_ticks.is_multiple_of(4);
                        cmd.right = if ((slot as u8) ^ (b.stuck_ticks / 6)) & 1 == 0 {
                            1.0
                        } else {
                            -1.0
                        };
                    }
                    if b.stuck_ticks >= 8 {
                        want_stuck_recenter = true;
                        stuck_tick_val = usize::from(b.stuck_ticks);
                        b.path_replan_time = 0.0;
                    }
                    if b.stuck_ticks >= 16 {
                        b.path = None;
                        b.stuck_ticks = 0;
                    }
                } else {
                    b.stuck_ticks = 0;
                }

                b.last_pos = hv.origin;
                if pulse_hostage_use {
                    b.hostage_use_cooldown = now + 1.2;
                    b.hostage_use_pulse = true;
                }
                b.brain = brain;
                b.cmd = cmd;
                b.next_full_think = now + 0.1;
            }

            if want_stuck_recenter
                && let Some(mesh) = g.bots.nav_mesh.as_ref()
                && let Some(curr) = mesh.get_nearest_nav_area(hv.origin, true)
            {
                let mut candidates = Vec::new();
                for dir in NavDir::ALL {
                    for &adj in mesh.adjacent_indices(curr, dir) {
                        if let Some(area) = mesh.area(adj) {
                            candidates.push(area.center());
                        }
                    }
                }
                if let Some(&target_pt) =
                    (!candidates.is_empty()).then(|| &candidates[(slot + stuck_tick_val) % candidates.len()])
                {
                    let (fwd, right) = world_dir_to_move((target_pt - hv.origin).truncate(), cmd.yaw_deg);
                    if let Some(b) = g.bots.slots[slot].as_mut() {
                        b.cmd.forward = fwd;
                        b.cmd.right = right;
                    }
                }
            }

            if let Some(out_radio) = decision.outgoing_radio.as_ref() {
                dispatch_bot_radio(g, eng, slot_u8, bot_team, hv.origin, profile.voice_pitch, out_radio);
            }
        }

        let cmd_copy = g.bots.slots[slot].as_ref().map(|b| b.cmd).unwrap_or_default();
        select_desired_weapon(g, eng, slot_u8, &cmd_copy);

        let id = eng.client_edict(slot_u8);
        let Some(hv) = eng.ent(id) else {
            continue;
        };
        let frozen = hv.flags & FL_FROZEN != 0 || g.bots.settings.bot_stop || g.bots.settings.bot_zombie;
        let max_speed = if hv.max_speed > 1.0 { hv.max_speed } else { 250.0 };
        let speed_scale = if cmd_copy.walk { 0.5 } else { 1.0 };
        let Some(b) = g.bots.slots[slot].as_mut() else {
            continue;
        };
        let cmd = b.cmd;
        let msec = if hv.flags & FL_FROZEN != 0 {
            0
        } else {
            ((now - b.prev_cmd_time) * 1000.0).clamp(1.0, 255.0) as u8
        };
        b.prev_cmd_time = now;

        let viewangles = Vec3::new(cmd.pitch_deg, cmd.yaw_deg, 0.0);
        let forwardmove = if frozen { 0.0 } else { cmd.forward * max_speed * speed_scale };
        let sidemove = if frozen { 0.0 } else { cmd.right * max_speed * speed_scale };
        let buttons = if frozen { 0 } else { encode_buttons(&cmd) };

        if let Some(v) = eng.ent_mut(id) {
            v.v_angle = viewangles;
            v.angles = Vec3::new(-viewangles.x / 3.0, viewangles.y, 0.0);
        }
        eng.run_player_move(slot_u8, viewangles, forwardmove, sidemove, 0.0, buttons, 0, msec);
        if let Some(b) = g.bots.slots[slot].as_mut()
            && b.hostage_use_pulse
        {
            b.cmd.use_key = false;
            b.hostage_use_pulse = false;
        }
    }
}

/// Resets per-round bot tactical, path, and buy state (`RestartRound`).
pub(crate) fn on_round_restart(g: &mut GameState) {
    let settings = g.bots.settings.clone();
    g.bots.incoming_radios.clear();
    g.bots.sound_events.clear();
    if let Some(mesh) = g.bots.nav_mesh.as_mut() {
        mesh.clear_blocked();
    }
    for (slot, entry) in g.bots.slots.iter_mut().enumerate() {
        if let Some(b) = entry.as_mut() {
            let seed = u64::from(g.rng) ^ (slot as u64);
            b.brain.on_round_reset(&b.profile, &settings, seed);
            b.path = None;
            b.path_wp_idx = 0;
            b.path_goal = None;
            b.path_replan_time = 0.0;
            b.done_buying = false;
            b.buy_retries = 0;
            b.cmd = BotCmd::default();
            b.last_pos = Vec3::ZERO;
            b.stuck_ticks = 0;
            b.hostage_use_cooldown = 0.0;
            b.hostage_use_pulse = false;
        }
    }
}

/// Releases bot bookkeeping when a fake client leaves (`ClientDisconnect`).
pub(crate) fn on_disconnect(g: &mut GameState, slot: u8) {
    if let Some(entry) = g.bots.slots.get_mut(usize::from(slot)) {
        *entry = None;
    }
}

fn active_editor_pos(g: &GameState, eng: &EngineApi<'_>) -> Vec3 {
    let max_clients = usize::from(eng.max_clients()).min(MAX_PLAYERS);
    let mut fallback = None;
    for (slot, slot_player) in g.players.iter().enumerate().take(max_clients) {
        if let Some(p) = slot_player.as_ref()
            && p.alive
            && let Some(v) = eng.ent(eng.client_edict(slot as u8))
        {
            if !eng.is_fake_client(slot as u8) {
                return v.origin;
            }
            fallback.get_or_insert(v.origin);
        }
    }
    fallback.unwrap_or(Vec3::ZERO)
}

fn target_edit_area_idx(g: &BotManager, _eng: &EngineApi<'_>, arg: Option<&str>, fallback_pos: Vec3) -> Option<usize> {
    let mesh = g.nav_mesh.as_ref()?;
    if let Some(id) = arg.and_then(|s| s.parse::<u32>().ok()) {
        return mesh.area_index_by_id(id);
    }
    if let Some(marked) = g.nav_edit_marked_area
        && marked < mesh.areas().len()
    {
        return Some(marked);
    }
    mesh.get_nearest_nav_area(fallback_pos, true)
}

fn rebuild_nav_mesh(mesh: &mut Option<NavMesh>, mut file: NavFile) {
    for (idx, area) in file.areas.iter_mut().enumerate() {
        if area.id == 0 {
            area.id = (idx + 1) as u32;
        }
    }
    *mesh = Some(NavMesh::from_nav_file(file));
}

fn handle_nav_command(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    cmd: &str,
    argv: &[String],
) -> Option<String> {
    let pos = active_editor_pos(g, eng);
    match cmd {
        "bot_nav_generate" | "bot_nav_analyze" => {
            sync_settings_from_cvars(g, eng);
            let cfg = build_nav_config(eng, g.bots.settings.bot_quicksave, true);
            let mesh = generate_nav_mesh(&EngineNavWorld(eng), &cfg);
            let area_count = mesh.areas().len();
            let ladder_count = mesh.ladders().len();
            let place_count = mesh.places().len();
            g.bots.nav_mesh = Some(mesh);
            g.bots.nav_map_name = eng.map_name().to_string();
            g.bots.nav_loaded_from_file = false;
            Some(format!(
                "Generated navmesh for '{}': {area_count} areas, {ladder_count} ladders, {place_count} places\n",
                eng.map_name()
            ))
        }
        "bot_nav_load" => {
            let map_name = eng.map_name().to_string();
            if let Some(path) = argv.get(1) {
                match std::fs::read(path).ok().and_then(|b| NavFile::parse(&b).ok()) {
                    Some(file) => {
                        let n = file.areas.len();
                        g.bots.nav_mesh = Some(NavMesh::from_nav_file(file));
                        g.bots.nav_map_name = map_name;
                        g.bots.nav_loaded_from_file = true;
                        return Some(format!("Loaded {n} nav areas from {path}\n"));
                    }
                    None => return Some(format!("Failed to load navmesh from {path}\n")),
                }
            }
            if let Some(vfs) = g.vfs.as_ref()
                && let Some(mesh) = try_load_nav_from_vfs(vfs, &map_name)
            {
                let n = mesh.areas().len();
                g.bots.nav_mesh = Some(mesh);
                g.bots.nav_map_name = map_name;
                g.bots.nav_loaded_from_file = true;
                Some(format!("Loaded {n} nav areas from VFS\n"))
            } else {
                Some(format!("No .nav file found for '{map_name}'\n"))
            }
        }
        "bot_nav_save" => {
            ensure_nav(g, eng);
            let Some(mesh) = g.bots.nav_mesh.as_ref() else {
                return Some("No navigation mesh to save\n".to_string());
            };
            let file = mesh.to_nav_file();
            let Ok(bytes) = file.to_bytes() else {
                return Some("Failed to serialize navigation mesh\n".to_string());
            };
            let target_path = if let Some(explicit) = argv.get(1) {
                Some(std::path::PathBuf::from(explicit))
            } else if let Some(vfs) = g.vfs.as_ref()
                && !vfs.root().as_os_str().is_empty()
                && vfs.root().join("cstrike").is_dir()
            {
                Some(
                    vfs.root()
                        .join("cstrike")
                        .join("maps")
                        .join(format!("{}.nav", eng.map_name())),
                )
            } else {
                None
            };
            let Some(path) = target_path else {
                return Some(format!(
                    "Serialized {} bytes ({} areas); pass a path when using in-memory VFS\n",
                    bytes.len(),
                    file.areas.len()
                ));
            };
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&path, &bytes) {
                Ok(()) => Some(format!(
                    "Saved navigation mesh ({} areas, {} bytes) to {}\n",
                    file.areas.len(),
                    bytes.len(),
                    path.display()
                )),
                Err(err) => Some(format!("Failed to write {}: {err}\n", path.display())),
            }
        }
        "bot_nav_mark" => {
            ensure_nav(g, eng);
            let idx = if let Some(id) = argv.get(1).and_then(|s| s.parse::<u32>().ok()) {
                g.bots.nav_mesh.as_ref().and_then(|m| m.area_index_by_id(id))
            } else {
                g.bots.nav_mesh.as_ref().and_then(|m| m.get_nearest_nav_area(pos, true))
            };
            g.bots.nav_edit_marked_area = idx;
            match idx.and_then(|i| g.bots.nav_mesh.as_ref()?.area(i)) {
                Some(a) => Some(format!("Marked nav area #{}\n", a.id)),
                None => Some("No nav area to mark\n".to_string()),
            }
        }
        "bot_nav_delete" => {
            let Some(idx) = target_edit_area_idx(&g.bots, eng, argv.get(1).map(String::as_str), pos) else {
                return Some("No nav area selected\n".to_string());
            };
            let mut file = g.bots.nav_mesh.as_ref()?.to_nav_file();
            let removed_id = file.areas.remove(idx).id;
            for area in &mut file.areas {
                for dir_list in &mut area.connections {
                    dir_list.retain(|&id| id != removed_id);
                }
            }
            rebuild_nav_mesh(&mut g.bots.nav_mesh, file);
            g.bots.nav_edit_marked_area = None;
            Some(format!("Deleted nav area #{removed_id}\n"))
        }
        "bot_nav_split" => {
            let Some(idx) = target_edit_area_idx(&g.bots, eng, argv.get(1).map(String::as_str), pos) else {
                return Some("No nav area selected\n".to_string());
            };
            let mut file = g.bots.nav_mesh.as_ref()?.to_nav_file();
            let orig = file.areas[idx].clone();
            let next_id = file.areas.iter().map(|a| a.id).max().unwrap_or(0) + 1;
            let sx = (orig.se_corner.x - orig.nw_corner.x).abs();
            let sy = (orig.se_corner.y - orig.nw_corner.y).abs();
            let (mut a, mut b) = (orig.clone(), orig.clone());
            b.id = next_id;
            if sx >= sy {
                let mid_x = (orig.nw_corner.x + orig.se_corner.x) * 0.5;
                let mid_nz = (orig.nw_corner.z + orig.ne_z) * 0.5;
                let mid_sz = (orig.sw_z + orig.se_corner.z) * 0.5;
                a.se_corner.x = mid_x;
                a.ne_z = mid_nz;
                a.se_corner.z = mid_sz;
                b.nw_corner.x = mid_x;
                b.nw_corner.z = mid_nz;
                b.sw_z = mid_sz;
                a.connections[NavDir::East.as_index()] = vec![b.id];
                b.connections[NavDir::West.as_index()] = vec![a.id];
            } else {
                let mid_y = (orig.nw_corner.y + orig.se_corner.y) * 0.5;
                let mid_wz = (orig.nw_corner.z + orig.sw_z) * 0.5;
                let mid_ez = (orig.ne_z + orig.se_corner.z) * 0.5;
                a.se_corner.y = mid_y;
                a.sw_z = mid_wz;
                a.se_corner.z = mid_ez;
                b.nw_corner.y = mid_y;
                b.nw_corner.z = mid_wz;
                b.ne_z = mid_ez;
                a.connections[NavDir::South.as_index()] = vec![b.id];
                b.connections[NavDir::North.as_index()] = vec![a.id];
            }
            file.areas[idx] = a;
            file.areas.push(b);
            rebuild_nav_mesh(&mut g.bots.nav_mesh, file);
            Some(format!("Split area #{} creating #{next_id}\n", orig.id))
        }
        "bot_nav_merge" => {
            let mesh = g.bots.nav_mesh.as_ref()?;
            let idx_a = argv
                .get(1)
                .and_then(|s| s.parse::<u32>().ok())
                .and_then(|id| mesh.area_index_by_id(id))
                .or(g.bots.nav_edit_marked_area)?;
            let idx_b = argv
                .get(2)
                .and_then(|s| s.parse::<u32>().ok())
                .and_then(|id| mesh.area_index_by_id(id))
                .or_else(|| mesh.get_nearest_nav_area(pos, true))?;
            if idx_a == idx_b {
                return Some("Select two distinct areas to merge\n".to_string());
            }
            let mut file = mesh.to_nav_file();
            let a = file.areas[idx_a].clone();
            let b = file.areas[idx_b].clone();
            let mut merged = a.clone();
            merged.nw_corner.x = a.nw_corner.x.min(b.nw_corner.x);
            merged.nw_corner.y = a.nw_corner.y.min(b.nw_corner.y);
            merged.se_corner.x = a.se_corner.x.max(b.se_corner.x);
            merged.se_corner.y = a.se_corner.y.max(b.se_corner.y);
            file.areas[idx_a] = merged;
            let removed_id = b.id;
            file.areas.remove(idx_b);
            for area in &mut file.areas {
                for dir_list in &mut area.connections {
                    for cid in dir_list.iter_mut() {
                        if *cid == removed_id {
                            *cid = a.id;
                        }
                    }
                    dir_list.sort_unstable();
                    dir_list.dedup();
                }
            }
            rebuild_nav_mesh(&mut g.bots.nav_mesh, file);
            Some(format!("Merged area #{removed_id} into #{}\n", a.id))
        }
        "bot_nav_connect" | "bot_nav_disconnect" => {
            let mesh = g.bots.nav_mesh.as_ref()?;
            let idx_a = argv
                .get(1)
                .and_then(|s| s.parse::<u32>().ok())
                .and_then(|id| mesh.area_index_by_id(id))
                .or(g.bots.nav_edit_marked_area)?;
            let idx_b = argv
                .get(2)
                .and_then(|s| s.parse::<u32>().ok())
                .and_then(|id| mesh.area_index_by_id(id))
                .or_else(|| mesh.get_nearest_nav_area(pos, true))?;
            if idx_a == idx_b {
                return Some("Cannot connect an area to itself\n".to_string());
            }
            let mut file = mesh.to_nav_file();
            let id_a = file.areas[idx_a].id;
            let id_b = file.areas[idx_b].id;
            let delta = file.areas[idx_b].center() - file.areas[idx_a].center();
            let dir = if delta.x.abs() >= delta.y.abs() {
                if delta.x >= 0.0 { NavDir::East } else { NavDir::West }
            } else if delta.y >= 0.0 {
                NavDir::South
            } else {
                NavDir::North
            };
            if cmd == "bot_nav_connect" {
                let list_a = &mut file.areas[idx_a].connections[dir.as_index()];
                if !list_a.contains(&id_b) {
                    list_a.push(id_b);
                }
                let list_b = &mut file.areas[idx_b].connections[dir.opposite().as_index()];
                if !list_b.contains(&id_a) {
                    list_b.push(id_a);
                }
                rebuild_nav_mesh(&mut g.bots.nav_mesh, file);
                Some(format!("Connected #{id_a} <-> #{id_b}\n"))
            } else {
                for list in &mut file.areas[idx_a].connections {
                    list.retain(|&id| id != id_b);
                }
                for list in &mut file.areas[idx_b].connections {
                    list.retain(|&id| id != id_a);
                }
                rebuild_nav_mesh(&mut g.bots.nav_mesh, file);
                Some(format!("Disconnected #{id_a} and #{id_b}\n"))
            }
        }
        "bot_nav_begin_area" => {
            let p = parse_vec3_args(argv.get(1..)).unwrap_or(pos);
            g.bots.nav_edit_begin_pos = Some(p);
            Some(format!("Nav area start corner set to ({:.1}, {:.1}, {:.1})\n", p.x, p.y, p.z))
        }
        "bot_nav_end_area" => {
            ensure_nav(g, eng);
            let p1 = g.bots.nav_edit_begin_pos.take().unwrap_or(pos);
            let p2 = parse_vec3_args(argv.get(1..)).unwrap_or(pos + Vec3::new(100.0, 100.0, 0.0));
            let mut file = g.bots.nav_mesh.as_ref()?.to_nav_file();
            let next_id = file.areas.iter().map(|a| a.id).max().unwrap_or(0) + 1;
            file.areas.push(NavArea {
                id: next_id,
                attributes: NavAttribute::NONE,
                nw_corner: Vec3::new(p1.x.min(p2.x), p1.y.min(p2.y), p1.z),
                se_corner: Vec3::new(p1.x.max(p2.x), p1.y.max(p2.y), p2.z),
                ne_z: p1.z,
                sw_z: p2.z,
                connections: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
                hiding_spots: Vec::new(),
                approach_spots: Vec::new(),
                encounters: Vec::new(),
                place_id: UNDEFINED_PLACE,
                ladders_up: Vec::new(),
                ladders_down: Vec::new(),
            });
            rebuild_nav_mesh(&mut g.bots.nav_mesh, file);
            Some(format!("Created nav area #{next_id}\n"))
        }
        "bot_nav_splice" => {
            let mesh = g.bots.nav_mesh.as_ref()?;
            let idx_a = g.bots.nav_edit_marked_area?;
            let idx_b = mesh.get_nearest_nav_area(pos, true)?;
            let mut file = mesh.to_nav_file();
            let ca = file.areas[idx_a].center();
            let cb = file.areas[idx_b].center();
            let next_id = file.areas.iter().map(|a| a.id).max().unwrap_or(0) + 1;
            let mid = (ca + cb) * 0.5;
            file.areas.push(NavArea {
                id: next_id,
                attributes: NavAttribute::NONE,
                nw_corner: Vec3::new(mid.x - 25.0, mid.y - 25.0, mid.z),
                se_corner: Vec3::new(mid.x + 25.0, mid.y + 25.0, mid.z),
                ne_z: mid.z,
                sw_z: mid.z,
                connections: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
                hiding_spots: Vec::new(),
                approach_spots: Vec::new(),
                encounters: Vec::new(),
                place_id: UNDEFINED_PLACE,
                ladders_up: Vec::new(),
                ladders_down: Vec::new(),
            });
            rebuild_nav_mesh(&mut g.bots.nav_mesh, file);
            Some(format!("Spliced new nav area #{next_id}\n"))
        }
        "bot_nav_crouch" | "bot_nav_jump" | "bot_nav_precise" | "bot_nav_no_jump" | "bot_nav_avoid" => {
            ensure_nav(g, eng);
            let Some(idx) = target_edit_area_idx(&g.bots, eng, argv.get(1).map(String::as_str), pos) else {
                return Some("No nav area selected\n".to_string());
            };
            let attr = match cmd {
                "bot_nav_crouch" => NavAttribute::CROUCH,
                "bot_nav_jump" => NavAttribute::JUMP,
                "bot_nav_precise" => NavAttribute::PRECISE,
                "bot_nav_no_jump" => NavAttribute::NO_JUMP,
                _ => NavAttribute::AVOID,
            };
            let mut file = g.bots.nav_mesh.as_ref()?.to_nav_file();
            let id = file.areas[idx].id;
            if file.areas[idx].attributes.contains(attr) {
                file.areas[idx].attributes.remove(attr);
            } else {
                file.areas[idx].attributes.insert(attr);
            }
            let active = file.areas[idx].attributes.contains(attr);
            rebuild_nav_mesh(&mut g.bots.nav_mesh, file);
            Some(format!("Area #{id} {cmd}: {active}\n"))
        }
        "bot_nav_corner_select" => {
            g.bots.nav_edit_selected_corner = match argv.get(1).map(|s| s.to_ascii_lowercase()).as_deref() {
                Some("ne") => NavCorner::NorthEast,
                Some("se") => NavCorner::SouthEast,
                Some("sw") => NavCorner::SouthWest,
                _ => NavCorner::NorthWest,
            };
            Some(format!("Selected corner {:?}\n", g.bots.nav_edit_selected_corner))
        }
        "bot_nav_corner_raise" | "bot_nav_corner_lower" => {
            let Some(idx) = target_edit_area_idx(&g.bots, eng, None, pos) else {
                return Some("No nav area selected\n".to_string());
            };
            let step = argv.get(1).and_then(|s| s.parse::<f32>().ok()).unwrap_or(5.0);
            let delta = if cmd == "bot_nav_corner_raise" { step } else { -step };
            let mut file = g.bots.nav_mesh.as_ref()?.to_nav_file();
            let area = &mut file.areas[idx];
            match g.bots.nav_edit_selected_corner {
                NavCorner::NorthWest => area.nw_corner.z += delta,
                NavCorner::NorthEast => area.ne_z += delta,
                NavCorner::SouthEast => area.se_corner.z += delta,
                NavCorner::SouthWest => area.sw_z += delta,
            }
            let id = area.id;
            rebuild_nav_mesh(&mut g.bots.nav_mesh, file);
            Some(format!("Adjusted corner {:?} of area #{id} by {delta}\n", g.bots.nav_edit_selected_corner))
        }
        "bot_nav_place_pick" => {
            ensure_nav(g, eng);
            let picked = g
                .bots
                .nav_mesh
                .as_ref()
                .and_then(|m| m.get_place(pos))
                .map(ToString::to_string);
            g.bots.nav_paint_place = picked.clone();
            Some(format!("Picked place: {}\n", picked.as_deref().unwrap_or("<none>")))
        }
        "bot_nav_place_set" => {
            ensure_nav(g, eng);
            let Some(place) = argv.get(1).cloned().or_else(|| g.bots.nav_paint_place.clone()) else {
                return Some("Usage: bot_nav_place_set <name>\n".to_string());
            };
            let Some(idx) = target_edit_area_idx(&g.bots, eng, argv.get(2).map(String::as_str), pos) else {
                return Some("No nav area selected\n".to_string());
            };
            let mesh = g.bots.nav_mesh.as_mut()?;
            let pid = mesh.set_area_place(idx, &place);
            Some(format!("Assigned place '{place}' (id {pid}) to area index {idx}\n"))
        }
        "bot_nav_place_floodfill" => {
            ensure_nav(g, eng);
            let place = argv
                .get(1)
                .cloned()
                .or_else(|| g.bots.nav_paint_place.clone())
                .unwrap_or_else(|| "CustomPlace".to_string());
            let Some(start_idx) = target_edit_area_idx(&g.bots, eng, None, pos) else {
                return Some("No nav area selected\n".to_string());
            };
            let mesh = g.bots.nav_mesh.as_mut()?;
            let orig_place = mesh.area(start_idx)?.place_id;
            let mut queue = std::collections::VecDeque::from([start_idx]);
            let mut visited = std::collections::HashSet::from([start_idx]);
            let mut count = 0usize;
            while let Some(idx) = queue.pop_front() {
                mesh.set_area_place(idx, &place);
                count += 1;
                for dir in NavDir::ALL {
                    for &adj in mesh.adjacent_indices(idx, dir).to_vec().iter() {
                        if visited.insert(adj)
                            && mesh.area(adj).is_some_and(|a| a.place_id == orig_place)
                        {
                            queue.push_back(adj);
                        }
                    }
                }
            }
            Some(format!("Flood-filled place '{place}' across {count} areas\n"))
        }
        "bot_nav_place_list" => {
            ensure_nav(g, eng);
            let places = g.bots.nav_mesh.as_ref().map_or(&[][..], NavMesh::places);
            Some(format!("Places ({}): {}\n", places.len(), places.join(", ")))
        }
        "bot_nav_toggle_place_mode" => {
            g.bots.nav_place_mode = !g.bots.nav_place_mode;
            Some(format!("Place mode: {}\n", g.bots.nav_place_mode))
        }
        "bot_nav_toggle_place_painting" => {
            g.bots.nav_place_painting = !g.bots.nav_place_painting;
            Some(format!("Place painting: {}\n", g.bots.nav_place_painting))
        }
        "bot_nav_strip" => {
            let mut file = g.bots.nav_mesh.as_ref()?.to_nav_file();
            for area in &mut file.areas {
                area.hiding_spots.clear();
                area.approach_spots.clear();
                area.encounters.clear();
            }
            rebuild_nav_mesh(&mut g.bots.nav_mesh, file);
            Some("Stripped hiding spots, approach spots, and encounters from navmesh\n".to_string())
        }
        "bot_nav_check_consistency" => {
            ensure_nav(g, eng);
            let mesh = g.bots.nav_mesh.as_ref()?;
            let mut missing_links = 0usize;
            for area in mesh.areas() {
                for dir_list in &area.connections {
                    for &target_id in dir_list {
                        if mesh.area_index_by_id(target_id).is_none() {
                            missing_links += 1;
                        }
                    }
                }
            }
            Some(format!(
                "NavMesh consistency: {} areas, {} ladders, {} dangling links\n",
                mesh.areas().len(),
                mesh.ladders().len(),
                missing_links
            ))
        }
        _ => None,
    }
}

fn parse_vec3_args(args: Option<&[String]>) -> Option<Vec3> {
    let a = args?;
    if a.len() < 3 {
        return None;
    }
    Some(Vec3::new(
        a[0].parse::<f32>().ok()?,
        a[1].parse::<f32>().ok()?,
        a[2].parse::<f32>().ok()?,
    ))
}

#[cfg(feature = "test-hooks")]
pub(crate) fn debug_summary(g: &GameState, eng: &EngineApi<'_>) -> String {
    let mut out = String::new();
    if let Some(m) = g.bots.nav_mesh.as_ref() {
        out.push_str(&format!(
            "nav(areas={}, ladders={}, places={:?})\n",
            m.areas().len(),
            m.ladders().len(),
            m.places()
        ));
    } else {
        out.push_str("nav=None\n");
    }
    for (slot, b_opt) in g.bots.slots.iter().enumerate() {
        let Some(b) = b_opt.as_ref() else { continue };
        let pos = eng.ent(eng.client_edict(slot as u8)).map(|v| v.origin);
        let p = g.players[slot].as_ref();
        let path_info = b.path.as_ref().map(|path| {
            (
                path.reached_goal,
                path.waypoints.len(),
                b.path_wp_idx,
                path.waypoints.get(b.path_wp_idx).map(|w| w.pos),
                path.endpoint(),
            )
        });
        let start_area = g.bots.nav_mesh.as_ref().and_then(|m| {
            let pos = pos?;
            let idx = m.get_nearest_nav_area(pos, false)?;
            let a = &m.areas()[idx];
            Some((idx, a.id, a.nw_corner, a.se_corner, a.connections.clone()))
        });
        let goal_area = g.bots.nav_mesh.as_ref().and_then(|m| {
            let goal = b.brain.move_goal?;
            let idx = m.get_nav_area(goal, 120.0)?;
            let a = &m.areas()[idx];
            Some((idx, a.id, a.nw_corner, a.se_corner))
        });
        out.push_str(&format!(
            "  slot={slot} name={} pos={pos:?} state={:?} goal={:?} has_c4={:?} in_bomb={:?} stuck={} cmd=({:.2},{:.2},yaw={:.1}) path={path_info:?}\n    start_area={start_area:?}\n    goal_area={goal_area:?}\n",
            b.profile.name,
            b.brain.state,
            b.brain.move_goal,
            p.map(|x| x.has_c4),
            p.map(|x| x.signals.state() & SIGNAL_BOMB != 0),
            b.stuck_ticks,
            b.cmd.forward,
            b.cmd.right,
            b.cmd.yaw_deg,
        ));
    }
    out
}

/// `GameDll::server_command` handler for `bot_add*`, `bot_kick`, `bot_kill`, weapon presets,
/// `bot_about`, and `bot_nav_*` mesh commands.
pub(crate) fn server_command(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    argv: &[String],
) -> Option<String> {
    let cmd = argv.first()?.to_ascii_lowercase();
    let arg1 = argv.get(1).map(String::as_str);
    let arg2 = argv.get(2).map(String::as_str);
    match cmd.as_str() {
        "bot_add" => {
            if add_bot(g, eng, None, arg1, arg2).is_some() {
                Some(String::new())
            } else {
                Some("Failed to add bot\n".to_string())
            }
        }
        "bot_add_t" => {
            if add_bot(g, eng, Some(Team::Terrorist), arg1, arg2).is_some() {
                Some(String::new())
            } else {
                Some("Failed to add terrorist bot\n".to_string())
            }
        }
        "bot_add_ct" => {
            if add_bot(g, eng, Some(Team::Ct), arg1, arg2).is_some() {
                Some(String::new())
            } else {
                Some("Failed to add CT bot\n".to_string())
            }
        }
        "bot_kick" => {
            let n = kick_bots(g, eng, arg1);
            if g.bots.quota_enabled && arg1.is_none_or(|t| t.eq_ignore_ascii_case("all")) {
                eng.set_cvar("bot_quota", "0");
            }
            Some(format!("Kicked {n} bot(s)\n"))
        }
        "bot_kill" => {
            let n = kill_bots(g, eng, arg1);
            Some(format!("Killed {n} bot(s)\n"))
        }
        "bot_knives_only" | "bot_pistols_only" | "bot_snipers_only" | "bot_all_weapons" => {
            sync_settings_from_cvars(g, eng);
            g.bots.settings.apply_cvar(&cmd, "1");
            sync_weapon_restriction_cvars(&g.bots.settings, eng);
            Some(String::new())
        }
        "bot_about" => Some(format!(
            "cs16-rs ZBot AI (cs-bot, GPL-3.0-or-later) — {} profiles loaded\n",
            g.bots.profile_db.profiles.len()
        )),
        c if c.starts_with("bot_nav_") => handle_nav_command(g, eng, c, argv),
        _ => None,
    }
}
