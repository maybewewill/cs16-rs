//! Server-side CS 1.6 bots driven on top of [`cs_sim::bots`] and ReHLDS fake clients
//! (`EngineApi::create_fake_client`, `EngineApi::run_player_move`, ReGameDLL
//! `bot/cs_bot_manager.cpp`, `gs/bot.cpp`, read 2026-11-23).
//!
//! `quota_enabled` defaults to `false` so existing unit/integration tests and the legacy offline
//! registry (which registers `bot_quota = "9"`) do not spawn bots behind a test's back; dedicated
//! `--proto48` servers call [`crate::CsGame::enable_cvar_bot_quota`], and `bot_add*` / `bot_kick`
//! console commands work everywhere.

use cs_assets::bsp::SpawnTeam;
use cs_engine::collision::{Hull, TraceResult};
use cs_engine::consts::FL_FROZEN;
use cs_engine::{EdictId, EngineApi, MoveFilter};
use cs_sim::bots::{
    BotBrain, BotDecision, BotObservation, BotWorld, CLASSIC_BOT_NAMES, NavGraph, think_bot_with,
};
use cs_sim::pm::consts::{
    IN_ATTACK, IN_BACK, IN_DUCK, IN_FORWARD, IN_JUMP, IN_MOVELEFT, IN_MOVERIGHT, IN_RELOAD, IN_USE,
};
use glam::Vec3;

use crate::GameState;
use crate::buy::{SIGNAL_BOMB, SIGNAL_BUY};
use crate::entity::GameEntity;
use crate::inventory::{KNIFE_SLOT, PISTOL_SLOT, PRIMARY_WEAPON_SLOT};
use crate::player::{self, DEAD_NO, MAX_PLAYERS, Team};
use crate::teams::{
    JoinState, MENU_SLOT_TEAM_CT, MENU_SLOT_TEAM_RANDOM, MENU_SLOT_TEAM_TERRORIST,
    handle_menu_choose_appearance, handle_menu_choose_team,
};
use crate::weapons::{WeaponId, item_info};

/// Engine-backed collision/visibility bridge for [`cs_sim::bots`].
struct EngineBotWorld<'a, 'b>(&'a EngineApi<'b>);

impl BotWorld for EngineBotWorld<'_, '_> {
    fn snap_to_floor(&self, seed: Vec3) -> Vec3 {
        let up = seed + Vec3::new(0.0, 0.0, 18.0);
        let down = seed - Vec3::new(0.0, 0.0, 128.0);
        let tr = self
            .0
            .trace_hull_ents(up, down, Hull::Standing, MoveFilter::NoMonsters, None)
            .trace;
        if !tr.start_solid && tr.fraction < 1.0 && tr.plane_normal.z >= 0.7 {
            tr.end_pos + Vec3::new(0.0, 0.0, 0.5)
        } else {
            seed
        }
    }

    fn is_solid_standing(&self, pos: Vec3) -> bool {
        self.trace_standing(pos, pos).start_solid
    }

    fn trace_standing(&self, start: Vec3, end: Vec3) -> TraceResult {
        self.0
            .trace_hull_ents(start, end, Hull::Standing, MoveFilter::NoMonsters, None)
            .trace
    }

    fn has_line_of_sight(&self, start: Vec3, end: Vec3) -> bool {
        self.0
            .trace_line(start, end, MoveFilter::NoMonsters, None)
            .trace
            .fraction
            >= 1.0
    }
}

/// Per-bot brain, pacing timers, and buy state.
#[derive(Debug, Clone)]
struct BotSlot {
    brain: BotBrain,
    next_cmd_time: f64,
    next_full_think: f64,
    prev_cmd_time: f64,
    decision: BotDecision,
    done_buying: bool,
    buy_retries: u8,
    last_pos: Vec3,
    stuck_ticks: u8,
    hostage_use_cooldown: f64,
    hostage_use_pulse: bool,
    committed_site: Option<Vec3>,
}

impl BotSlot {
    fn new(now: f64) -> Self {
        Self {
            brain: BotBrain::default(),
            next_cmd_time: now,
            next_full_think: now,
            prev_cmd_time: now,
            decision: BotDecision::default(),
            done_buying: false,
            buy_retries: 0,
            last_pos: Vec3::ZERO,
            stuck_ticks: 0,
            hostage_use_cooldown: 0.0,
            hostage_use_pulse: false,
            committed_site: None,
        }
    }
}

/// Server-wide bot manager held on [`GameState`].
pub(crate) struct BotManager {
    pub(crate) quota_enabled: bool,
    nav: Option<NavGraph>,
    next_quota_check: f64,
    slots: [Option<BotSlot>; MAX_PLAYERS],
}

impl Default for BotManager {
    fn default() -> Self {
        Self {
            quota_enabled: false,
            nav: None,
            next_quota_check: 0.0,
            slots: std::array::from_fn(|_| None),
        }
    }
}

impl BotManager {
    pub(crate) fn reset(&mut self) {
        let quota_enabled = self.quota_enabled;
        *self = Self::default();
        self.quota_enabled = quota_enabled;
    }
}

fn zone_floor_point(absmin: Vec3, absmax: Vec3) -> Vec3 {
    Vec3::new(
        (absmin.x + absmax.x) * 0.5,
        (absmin.y + absmax.y) * 0.5,
        absmin.z + ((absmax.z - absmin.z) * 0.5).min(36.0),
    )
}

fn ensure_nav(g: &mut GameState, eng: &EngineApi<'_>) {
    if g.bots.nav.is_some() {
        return;
    }
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
        while let Some(id) = eng.find_by_classname(cursor, class) {
            if let Some(v) = eng.ent(id) {
                seeds.push(v.origin);
            }
            cursor = Some(id);
        }
    }
    for class in [
        "func_bomb_target",
        "func_hostage_rescue",
        "func_vip_safetyzone",
        "func_escapezone",
    ] {
        let mut cursor = None;
        while let Some(id) = eng.find_by_classname(cursor, class) {
            if let Some(v) = eng.ent(id) {
                let c = zone_floor_point(v.absmin, v.absmax);
                let hx = ((v.absmax.x - v.absmin.x) * 0.5 - 16.0).clamp(0.0, 64.0);
                let hy = ((v.absmax.y - v.absmin.y) * 0.5 - 16.0).clamp(0.0, 64.0);
                for (ox, oy) in [(0.0, 0.0), (hx, 0.0), (-hx, 0.0), (0.0, hy), (0.0, -hy)] {
                    seeds.push(c + Vec3::new(ox, oy, 0.0));
                }
            }
            cursor = Some(id);
        }
    }
    let world = EngineBotWorld(eng);
    g.bots.nav = Some(NavGraph::build_with(&world, &seeds));
}

fn pick_bot_name(eng: &EngineApi<'_>, requested: Option<&str>) -> String {
    if let Some(name) = requested
        && !name.is_empty()
    {
        return name.to_string();
    }
    let max_clients = usize::from(eng.max_clients());
    for &candidate in CLASSIC_BOT_NAMES {
        let used = (0..max_clients)
            .filter_map(|s| eng.client_key(s as u8, "name"))
            .any(|n| n.eq_ignore_ascii_case(candidate));
        if !used {
            return candidate.to_string();
        }
    }
    "Bot".to_string()
}

/// Spawns one fake-client bot, joins a team + class, and queues it to spawn.
pub(crate) fn add_bot(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    team_pref: Option<Team>,
    name: Option<&str>,
) -> Option<u8> {
    let chosen_name = pick_bot_name(eng, name);
    let slot = eng.create_fake_client(&chosen_name)?;
    player::put_in_server(g, eng, slot);

    if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        p.joining = JoinState::PickingTeam;
    }

    let cvar_team = eng.cvar_str("bot_join_team").unwrap_or_default();
    let choice = match team_pref {
        Some(Team::Terrorist) => MENU_SLOT_TEAM_TERRORIST,
        Some(Team::Ct) => MENU_SLOT_TEAM_CT,
        _ if cvar_team.eq_ignore_ascii_case("T") => MENU_SLOT_TEAM_TERRORIST,
        _ if cvar_team.eq_ignore_ascii_case("CT") => MENU_SLOT_TEAM_CT,
        _ => MENU_SLOT_TEAM_RANDOM,
    };

    if !handle_menu_choose_team(g, eng, slot, choice) {
        eng.kick(slot, "Could not join team");
        return None;
    }

    let skin = g.random_range(1, 4);
    handle_menu_choose_appearance(g, eng, slot, skin);

    let now = eng.time();
    g.bots.slots[usize::from(slot)] = Some(BotSlot::new(now));
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

fn maintain_quota(g: &mut GameState, eng: &mut EngineApi<'_>) {
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
    let raw_quota = eng.cvar_f32("bot_quota").max(0.0) as usize;
    let desired = raw_quota.min(max_clients.saturating_sub(humans));

    if bots < desired {
        add_bot(g, eng, None, None);
    } else if bots > desired {
        for slot in (0..max_clients).rev() {
            if eng.is_fake_client(slot as u8) && g.bots.slots[slot].is_some() {
                eng.kick(slot as u8, "Bot quota reduced");
                break;
            }
        }
    }
}

fn maybe_buy(g: &mut GameState, eng: &mut EngineApi<'_>, slot: usize) {
    let Some(bot) = g.bots.slots[slot].as_mut() else {
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

    let slot_u8 = slot as u8;
    let team = p.team;
    let money = p.money;
    let has_primary = p.has_primary;
    let armor = p.armor;
    let has_defuser = p.has_defuser;
    let bomb_map = g.rules.map_has_bomb_target();

    if !has_primary {
        let weapon = match team {
            Team::Ct if money >= 3100 => Some("m4a1"),
            Team::Ct if money >= 2250 => Some("famas"),
            Team::Terrorist if money >= 2500 => Some("ak47"),
            Team::Terrorist if money >= 2000 => Some("galil"),
            Team::Ct | Team::Terrorist if money >= 1500 => Some("mp5"),
            _ => None,
        };
        if let Some(cmd) = weapon {
            player::client_command(g, eng, slot_u8, &[cmd.to_string()]);
        }
    }

    let now_primary = g.players[slot].as_ref().is_some_and(|p| p.has_primary);
    if now_primary {
        player::client_command(g, eng, slot_u8, &["primammo".to_string()]);
    }
    if armor < 100.0 {
        if money >= 1000 {
            player::client_command(g, eng, slot_u8, &["vesthelm".to_string()]);
        } else if money >= 650 {
            player::client_command(g, eng, slot_u8, &["vest".to_string()]);
        }
    }
    player::client_command(g, eng, slot_u8, &["secammo".to_string()]);
    if bomb_map && team == Team::Ct && !has_defuser && money >= 200 {
        player::client_command(g, eng, slot_u8, &["defuser".to_string()]);
    }

    if let Some(bot) = g.bots.slots[slot].as_mut() {
        bot.buy_retries += 1;
        if now_primary || bot.buy_retries >= 5 {
            bot.done_buying = true;
        }
    }
}

fn find_bomb_targets(eng: &EngineApi<'_>) -> [Option<Vec3>; 2] {
    let mut targets = [None, None];
    let mut count = 0usize;
    let mut cursor = None;
    while count < 2
        && let Some(id) = eng.find_by_classname(cursor, "func_bomb_target")
    {
        if let Some(v) = eng.ent(id) {
            targets[count] = Some(zone_floor_point(v.absmin, v.absmax));
            count += 1;
        }
        cursor = Some(id);
    }
    let mut cursor = None;
    while count < 2
        && let Some(id) = eng.find_by_classname(cursor, "info_bomb_target")
    {
        if let Some(v) = eng.ent(id) {
            targets[count] = Some(v.origin);
            count += 1;
        }
        cursor = Some(id);
    }
    targets
}

fn find_planted_c4(eng: &EngineApi<'_>) -> Option<Vec3> {
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "grenade") {
        if eng
            .private::<Box<dyn GameEntity>>(id)
            .is_some_and(|e| e.is_planted_live_c4())
            && let Some(v) = eng.ent(id)
        {
            let center = if v.size.length_squared() > 0.0 {
                v.absmin + v.size * 0.5
            } else {
                v.origin
            };
            return Some(center);
        }
        cursor = Some(id);
    }
    None
}

#[derive(Debug, Clone, Copy)]
struct HostageTarget {
    origin: Vec3,
    center: Vec3,
    follower: Option<EdictId>,
}

fn find_rescue_zones(eng: &EngineApi<'_>) -> Vec<Vec3> {
    let mut zones = Vec::new();
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "func_hostage_rescue") {
        if let Some(v) = eng.ent(id) {
            zones.push(zone_floor_point(v.absmin, v.absmax));
        }
        cursor = Some(id);
    }
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "info_hostage_rescue") {
        if let Some(v) = eng.ent(id) {
            zones.push(v.origin);
        }
        cursor = Some(id);
    }
    if zones.is_empty() && eng.find_by_classname(None, "hostage_entity").is_some() {
        let mut cursor = None;
        while let Some(id) = eng.find_by_classname(cursor, "info_player_start") {
            if let Some(v) = eng.ent(id) {
                zones.push(v.origin);
            }
            cursor = Some(id);
        }
    }
    zones
}

fn find_hostages(eng: &EngineApi<'_>) -> Vec<HostageTarget> {
    let mut out = Vec::new();
    let mut cursor = None;
    while let Some(id) = eng.find_by_classname(cursor, "hostage_entity") {
        if let Some(e) = eng.private::<Box<dyn GameEntity>>(id)
            && e.is_hostage()
            && e.is_alive(eng, id)
            && !e.is_hostage_rescued()
            && let Some(v) = eng.ent(id)
        {
            let center = if v.size.length_squared() > 0.0 {
                v.absmin + v.size * 0.5
            } else {
                v.origin + Vec3::new(0.0, 0.0, 31.0)
            };
            out.push(HostageTarget {
                origin: v.origin,
                center,
                follower: e.hostage_follower(),
            });
        }
        cursor = Some(id);
    }
    out
}

fn active_ammo(p: &player::Player) -> (u16, u16) {
    let Some(id) = p.active_item else {
        return (0, 0);
    };
    let Some((s, i)) = p.find_item(id) else {
        return (0, 0);
    };
    let w = &p.items[s][i].weapon;
    let clip = w.clip.max(0) as u16;
    let reserve = usize::try_from(w.primary_ammo_type)
        .ok()
        .and_then(|t| p.ammo.get(t).copied())
        .unwrap_or(0)
        .max(0) as u16;
    (clip, reserve)
}

fn maybe_select_weapon(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, want_c4: bool) {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    if want_c4 && p.has_c4 {
        if p.active_item != Some(WeaponId::C4) {
            player::client_command(g, eng, slot, &["weapon_c4".to_string()]);
        }
        return;
    }
    let active_is_knife_or_c4 = matches!(p.active_item, Some(WeaponId::Knife) | Some(WeaponId::C4))
        || p.active_item.is_none();
    if active_is_knife_or_c4 {
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

fn encode_buttons(dec: &BotDecision) -> u16 {
    let mut b = 0i32;
    if dec.forward > 0.1 {
        b |= IN_FORWARD;
    } else if dec.forward < -0.1 {
        b |= IN_BACK;
    }
    if dec.right > 0.1 {
        b |= IN_MOVERIGHT;
    } else if dec.right < -0.1 {
        b |= IN_MOVELEFT;
    }
    if dec.attack {
        b |= IN_ATTACK;
    }
    if dec.jump {
        b |= IN_JUMP;
    }
    if dec.duck {
        b |= IN_DUCK;
    }
    if dec.use_key {
        b |= IN_USE;
    }
    if dec.reload {
        b |= IN_RELOAD;
    }
    b as u16
}

fn gather_enemies(
    g: &GameState,
    eng: &EngineApi<'_>,
    my_slot: usize,
    my_team: Team,
    max_clients: usize,
) -> Vec<(u32, Vec3, Vec3)> {
    let mut enemies = Vec::new();
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
        {
            enemies.push((other as u32, ov.origin, ov.origin + ov.view_ofs));
        }
    }
    enemies
}

fn nearest_reachable_point(
    nav: Option<&NavGraph>,
    from: Vec3,
    candidates: &[Vec3],
) -> Option<Vec3> {
    if candidates.is_empty() {
        return None;
    }
    let Some(nav) = nav.filter(|n| !n.nodes.is_empty()) else {
        return candidates
            .iter()
            .copied()
            .min_by(|a, b| a.distance_squared(from).total_cmp(&b.distance_squared(from)));
    };
    let start = nav.nearest(from)?;
    candidates.iter().copied().min_by(|&a, &b| {
        let da = nav
            .nearest_reachable(start, a)
            .and_then(|i| nav.nodes.get(i))
            .map_or(f32::INFINITY, |n| n.pos.distance_squared(a));
        let db = nav
            .nearest_reachable(start, b)
            .and_then(|i| nav.nodes.get(i))
            .map_or(f32::INFINITY, |n| n.pos.distance_squared(b));
        da.total_cmp(&db)
    })
}

fn pick_committed_site(
    nav: Option<&NavGraph>,
    from: Vec3,
    bomb_targets: [Option<Vec3>; 2],
    pick_seed: usize,
) -> Option<Vec3> {
    let sites: Vec<Vec3> = bomb_targets.into_iter().flatten().collect();
    if sites.is_empty() {
        return None;
    }
    if sites.len() == 1 {
        return Some(sites[0]);
    }
    if let Some(nav) = nav.filter(|n| !n.nodes.is_empty())
        && let Some(start) = nav.nearest(from)
    {
        let reachable: Vec<Vec3> = sites
            .iter()
            .copied()
            .filter(|&s| {
                nav.nearest_reachable(start, s)
                    .and_then(|i| nav.nodes.get(i))
                    .is_some_and(|n| n.pos.distance(s) < 280.0)
            })
            .collect();
        if !reachable.is_empty() {
            return Some(reachable[pick_seed % reachable.len()]);
        }
    }
    nearest_reachable_point(nav, from, &sites)
}

fn hostage_goal_for_bot(
    nav: Option<&NavGraph>,
    bot_pos: Vec3,
    bot_edict: EdictId,
    my_team: Team,
    hostages: &[HostageTarget],
    rescue_zones: &[Vec3],
) -> (Option<Vec3>, Vec<HostageTarget>, Option<HostageTarget>, Option<Vec3>) {
    if my_team != Team::Ct || hostages.is_empty() {
        return (None, Vec::new(), None, None);
    }
    let following_me: Vec<HostageTarget> = hostages
        .iter()
        .copied()
        .filter(|h| h.follower == Some(bot_edict))
        .collect();
    if !following_me.is_empty() {
        let rescue_zone = nearest_reachable_point(nav, bot_pos, rescue_zones);
        let lagging = following_me
            .iter()
            .max_by(|a, b| {
                a.origin
                    .distance_squared(bot_pos)
                    .total_cmp(&b.origin.distance_squared(bot_pos))
            })
            .copied();
        let goal = if let Some(lag) = lagging
            && lag.origin.distance(bot_pos) > 340.0
        {
            Some(lag.origin)
        } else {
            rescue_zone
        };
        return (goal, following_me, None, rescue_zone);
    }

    let pool: Vec<Vec3> = {
        let free: Vec<Vec3> = hostages
            .iter()
            .filter(|h| h.follower.is_none())
            .map(|h| h.origin)
            .collect();
        if free.is_empty() {
            hostages.iter().map(|h| h.origin).collect()
        } else {
            free
        }
    };
    let chosen_pos = nearest_reachable_point(nav, bot_pos, &pool);
    let target_hostage = chosen_pos.and_then(|p| {
        hostages
            .iter()
            .copied()
            .min_by(|a, b| a.origin.distance_squared(p).total_cmp(&b.origin.distance_squared(p)))
    });
    (target_hostage.map(|h| h.origin), Vec::new(), target_hostage, None)
}

/// Drives bot quota maintenance, 30 Hz command generation, and 10 Hz AI updates every server
/// frame (`CBotManager::StartFrame`).
pub(crate) fn start_frame(g: &mut GameState, eng: &mut EngineApi<'_>) {
    ensure_nav(g, eng);
    let now = eng.time();
    if g.bots.quota_enabled && now >= g.bots.next_quota_check {
        g.bots.next_quota_check = now + 0.5;
        maintain_quota(g, eng);
    }

    let max_clients = usize::from(eng.max_clients()).min(MAX_PLAYERS);
    let bomb_targets = find_bomb_targets(eng);
    let planted_pos = find_planted_c4(eng);
    let hostages = find_hostages(eng);
    let rescue_zones = if hostages.is_empty() {
        Vec::new()
    } else {
        find_rescue_zones(eng)
    };

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

        let Some(p_state) = g.players[slot]
            .as_ref()
            .map(|p| (p.alive, p.joining, p.team))
        else {
            continue;
        };
        let (alive, joining, my_team) = p_state;

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
            let spawn_team = match my_team {
                Team::Terrorist => SpawnTeam::Terrorist,
                _ => SpawnTeam::CounterTerrorist,
            };
            let (clip, reserve) = active_ammo(p);

            let mut committed_site = g.bots.slots[slot]
                .as_ref()
                .and_then(|b| b.committed_site);
            if my_team == Team::Terrorist && p.has_c4 && planted_pos.is_none() {
                if committed_site.is_none() {
                    let pick_seed = (g.rng as usize).wrapping_add(slot);
                    committed_site =
                        pick_committed_site(g.bots.nav.as_ref(), hv.origin, bomb_targets, pick_seed);
                }
            } else {
                committed_site = None;
            }

            let (hostage_goal, following_me, target_hostage, rescue_zone) = if planted_pos.is_none()
            {
                hostage_goal_for_bot(
                    g.bots.nav.as_ref(),
                    hv.origin,
                    id,
                    my_team,
                    &hostages,
                    &rescue_zones,
                )
            } else {
                (None, Vec::new(), None, None)
            };

            let (eff_bomb_targets, priority_goal) =
                if my_team == Team::Terrorist && p.has_c4 && committed_site.is_some() {
                    ([committed_site, None], committed_site)
                } else {
                    (bomb_targets, hostage_goal)
                };

            let obs = BotObservation {
                self_id: slot as u32,
                team: spawn_team,
                pos: hv.origin,
                eye: hv.origin + hv.view_ofs,
                yaw_deg: hv.v_angle.y,
                pitch_deg: hv.v_angle.x,
                has_c4: p.has_c4,
                in_bomb_zone: p.signals.state() & SIGNAL_BOMB != 0,
                clip,
                reserve,
                flash_alpha: if (now as f32) < p.blind_until_time {
                    1.0
                } else {
                    0.0
                },
                bomb_planted_pos: planted_pos,
                bomb_targets: eff_bomb_targets,
                priority_goal,
                difficulty: 2,
            };
            let enemies = gather_enemies(g, eng, slot, my_team, max_clients);
            let seed = u64::from(g.rng) ^ ((now * 100.0) as u64).wrapping_add(slot as u64);
            let (mut brain, hostage_use_cooldown) = {
                let b = g.bots.slots[slot].as_ref().unwrap();
                (b.brain.clone(), b.hostage_use_cooldown)
            };
            let mut dec = g
                .bots
                .nav
                .as_ref()
                .map(|nav| {
                    think_bot_with(
                        &mut brain,
                        &obs,
                        &enemies,
                        nav,
                        &EngineBotWorld(eng),
                        &[],
                        seed,
                        0.1,
                    )
                })
                .unwrap_or_default();

            let world = EngineBotWorld(eng);
            let eye = hv.origin + hv.view_ofs;
            let enemy_visible = enemies.iter().any(|&(_, origin, enemy_eye)| {
                let aim = origin.lerp(enemy_eye, 0.72);
                eye.distance(aim) < 2600.0 && world.has_line_of_sight(eye, aim)
            });

            let mut waiting_for_hostage = false;
            let mut pulse_hostage_use = false;
            if !enemy_visible && my_team == Team::Ct && planted_pos.is_none() && !hostages.is_empty()
            {
                if !following_me.is_empty() {
                    let max_dist = following_me
                        .iter()
                        .map(|h| h.origin.distance(hv.origin))
                        .fold(0.0_f32, f32::max);
                    if (max_dist > 220.0 && max_dist <= 340.0)
                        || rescue_zone.is_some_and(|rz| hv.origin.distance(rz) < 64.0)
                    {
                        dec.forward = 0.0;
                        dec.right = 0.0;
                        waiting_for_hostage = true;
                    }
                } else if let Some(h) = target_hostage {
                    let dist = hv.origin.distance(h.origin);
                    if dist < 56.0 {
                        let d = h.center - eye;
                        let horiz = (d.x * d.x + d.y * d.y).sqrt().max(0.001);
                        dec.yaw_deg = d.y.atan2(d.x).to_degrees().rem_euclid(360.0);
                        dec.pitch_deg = (-d.z.atan2(horiz).to_degrees()).clamp(-80.0, 80.0);
                        dec.forward = 0.0;
                        dec.right = 0.0;
                        waiting_for_hostage = true;
                        if now >= hostage_use_cooldown {
                            dec.use_key = true;
                            pulse_hostage_use = true;
                        }
                    } else if dist < 110.0 && world.has_line_of_sight(eye, h.center) {
                        let d = h.center - eye;
                        dec.yaw_deg = d.y.atan2(d.x).to_degrees().rem_euclid(360.0);
                        dec.pitch_deg = 0.0;
                        dec.forward = 0.7;
                    }
                }
            }

            let mut detour_target = None;
            let mut want_stuck_recenter = false;
            if let Some(b) = g.bots.slots[slot].as_mut() {
                let moving_unblocked = (hv.flags & FL_FROZEN) == 0
                    && hv.max_speed > 5.0
                    && !waiting_for_hostage
                    && !dec.use_key
                    && !(dec.want_c4 && dec.attack)
                    && (dec.forward.abs() > 0.2 || dec.right.abs() > 0.2);

                if moving_unblocked {
                    let moved = hv.origin.truncate().distance(b.last_pos.truncate());
                    if moved < 4.0 {
                        b.stuck_ticks = b.stuck_ticks.saturating_add(1);
                    } else {
                        b.stuck_ticks = 0;
                    }
                    if b.stuck_ticks >= 4 {
                        dec.jump = b.stuck_ticks.is_multiple_of(4);
                        dec.right = if ((slot as u8) ^ (b.stuck_ticks / 6)) & 1 == 0 {
                            1.0
                        } else {
                            -1.0
                        };
                    }
                    if b.stuck_ticks >= 8 {
                        want_stuck_recenter = true;
                        detour_target = Some(b.stuck_ticks);
                    }
                    if b.stuck_ticks >= 14 {
                        brain.goal_node = None;
                        brain.repath_timer = 0.0;
                        if b.stuck_ticks >= 22 {
                            b.stuck_ticks = 0;
                        }
                    }
                } else {
                    b.stuck_ticks = 0;
                }

                b.last_pos = hv.origin;
                b.committed_site = committed_site;
                if pulse_hostage_use {
                    b.hostage_use_cooldown = now + 1.2;
                    b.hostage_use_pulse = true;
                }
                b.brain = brain;
                b.decision = dec;
                b.next_full_think = now + 0.1;
            }
            if want_stuck_recenter
                && let Some(nav) = g.bots.nav.as_ref()
                && let Some(curr) = nav.nearest(hv.origin)
            {
                let world = EngineBotWorld(eng);
                let recenter = nav.nodes[curr].pos;
                let pick = if hv.origin.truncate().distance(recenter.truncate()) > 20.0 {
                    Some(recenter)
                } else {
                    let clear: Vec<Vec3> = nav.nodes[curr]
                        .neighbors
                        .iter()
                        .filter_map(|&ni| nav.nodes.get(ni))
                        .map(|n| n.pos)
                        .filter(|&np| {
                            world
                                .trace_standing(
                                    hv.origin + Vec3::new(0.0, 0.0, 2.0),
                                    np + Vec3::new(0.0, 0.0, 2.0),
                                )
                                .fraction
                                >= 0.9
                        })
                        .collect();
                    let s_ticks = usize::from(detour_target.unwrap_or(0));
                    (!clear.is_empty()).then(|| clear[(slot + s_ticks / 4) % clear.len()])
                };
                if let Some(target_pt) = pick
                    && let Some(b) = g.bots.slots[slot].as_mut()
                {
                    let delta = target_pt - hv.origin;
                    b.decision.yaw_deg = delta.y.atan2(delta.x).to_degrees().rem_euclid(360.0);
                    b.decision.pitch_deg = 0.0;
                    b.decision.forward = 1.0;
                }
            }
        }

        let want_c4 = g.bots.slots[slot].as_ref().is_some_and(|b| b.decision.want_c4);
        maybe_select_weapon(g, eng, slot_u8, want_c4);

        let id = eng.client_edict(slot_u8);
        let Some(hv) = eng.ent(id) else {
            continue;
        };
        let frozen = hv.flags & FL_FROZEN != 0;
        let max_speed = if hv.max_speed > 1.0 { hv.max_speed } else { 250.0 };
        let Some(b) = g.bots.slots[slot].as_mut() else {
            continue;
        };
        let dec = b.decision;
        let msec = if frozen {
            0
        } else {
            ((now - b.prev_cmd_time) * 1000.0).clamp(1.0, 255.0) as u8
        };
        b.prev_cmd_time = now;

        let viewangles = Vec3::new(dec.pitch_deg, dec.yaw_deg, 0.0);
        let forwardmove = if frozen { 0.0 } else { dec.forward * max_speed };
        let sidemove = if frozen { 0.0 } else { dec.right * max_speed };
        let buttons = if frozen { 0 } else { encode_buttons(&dec) };

        if let Some(v) = eng.ent_mut(id) {
            v.v_angle = viewangles;
            v.angles = Vec3::new(-viewangles.x / 3.0, viewangles.y, 0.0);
        }
        eng.run_player_move(slot_u8, viewangles, forwardmove, sidemove, 0.0, buttons, 0, msec);
        if let Some(b) = g.bots.slots[slot].as_mut()
            && b.hostage_use_pulse
        {
            b.decision.use_key = false;
            b.hostage_use_pulse = false;
        }
    }
}

/// Resets per-round bot buy state (`RestartRound`).
pub(crate) fn on_round_restart(g: &mut GameState) {
    for b in g.bots.slots.iter_mut().flatten() {
        b.brain = BotBrain::default();
        b.done_buying = false;
        b.buy_retries = 0;
        b.decision = BotDecision::default();
        b.last_pos = Vec3::ZERO;
        b.stuck_ticks = 0;
        b.hostage_use_cooldown = 0.0;
        b.hostage_use_pulse = false;
        b.committed_site = None;
    }
}

/// Releases bot bookkeeping when a fake client leaves (`ClientDisconnect`).
pub(crate) fn on_disconnect(g: &mut GameState, slot: u8) {
    if let Some(entry) = g.bots.slots.get_mut(usize::from(slot)) {
        *entry = None;
    }
}

/// `GameDll::server_command` handler for `bot_add`, `bot_add_t`, `bot_add_ct`, and `bot_kick`.
pub(crate) fn server_command(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    argv: &[String],
) -> Option<String> {
    let cmd = argv.first()?.to_ascii_lowercase();
    match cmd.as_str() {
        "bot_add" => {
            let name = argv.get(1).map(String::as_str);
            if add_bot(g, eng, None, name).is_some() {
                Some(String::new())
            } else {
                Some("Failed to add bot\n".to_string())
            }
        }
        "bot_add_t" => {
            let name = argv.get(1).map(String::as_str);
            if add_bot(g, eng, Some(Team::Terrorist), name).is_some() {
                Some(String::new())
            } else {
                Some("Failed to add terrorist bot\n".to_string())
            }
        }
        "bot_add_ct" => {
            let name = argv.get(1).map(String::as_str);
            if add_bot(g, eng, Some(Team::Ct), name).is_some() {
                Some(String::new())
            } else {
                Some("Failed to add CT bot\n".to_string())
            }
        }
        "bot_kick" => {
            let target = argv.get(1).map(String::as_str);
            let n = kick_bots(g, eng, target);
            if g.bots.quota_enabled && target.is_none_or(|t| t.eq_ignore_ascii_case("all")) {
                eng.set_cvar("bot_quota", "0");
            }
            Some(format!("Kicked {n} bot(s)\n"))
        }
        _ => None,
    }
}
