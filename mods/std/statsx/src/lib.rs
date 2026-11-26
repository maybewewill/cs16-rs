//! Standard StatsX & MiscStats (`std_statsx`) — Rust -> WASM plugin.
//!
//! Tracks persistent per-player kills, deaths, headshots, and damage in the host
//! $O(\log N)$ `LeaderboardStore` (`"statsx_rank"`), serving `/rank`, `/statsme`, `/me`, `/hp`,
//! authentic black-and-orange VGUI2 `MOTD.res` `/top15` HTML tables, killstreak announcements
//! (`First Blood`, `Double Kill`, `Multi-Kill`, `Monster-Kill`, `Godlike`, `Humiliation`), and
//! C4 planted/defused announcements.

use std::sync::{Mutex, OnceLock};

use cs_mod_sdk::{
    DhudMessageSpec, HookAction, HookId, HookPhase, HostBridge, Player, cs_mod_init,
};

cs_mod_sdk::cs16_wasm_host_bridge!();

const LANG_TOML: &str = include_str!("../lang.toml");
const TABLE_RANK: &str = "statsx_rank";

#[derive(Default, Clone)]
struct PlayerStats {
    name: String,
    kills: u32,
    deaths: u32,
    headshots: u32,
    round_dmg: u32,
    round_hits: u32,
    streak: u32,
    last_killer: Option<(String, String, i32, i32)>,
}

#[derive(Default)]
struct StatsState {
    slots: Vec<PlayerStats>,
    first_blood_taken: bool,
}

impl StatsState {
    fn ensure(&mut self, slot: u8) -> &mut PlayerStats {
        let idx = usize::from(slot);
        if self.slots.len() <= idx {
            self.slots.resize_with(idx + 1, PlayerStats::default);
        }
        &mut self.slots[idx]
    }
}

static STATE: OnceLock<Mutex<StatsState>> = OnceLock::new();

fn state() -> &'static Mutex<StatsState> {
    STATE.get_or_init(|| Mutex::new(StatsState::default()))
}

fn player_key(host: &impl HostBridge, slot: u8) -> String {
    let p = Player::new(slot);
    let sid = p.steamid(host);
    if !sid.is_empty() {
        sid
    } else {
        let name = p.name(host);
        if name.is_empty() {
            format!("slot_{slot}")
        } else {
            name
        }
    }
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

fn parse_stats_payload(payload: &str) -> (String, u32, u32, u32) {
    let mut name = String::new();
    let mut kills = 0u32;
    let mut deaths = 0u32;
    let mut hs = 0u32;
    for part in payload.split('|') {
        if let Some((k, v)) = part.split_once('=') {
            match k {
                "n" => name = v.to_string(),
                "k" => kills = v.parse().unwrap_or(0),
                "d" => deaths = v.parse().unwrap_or(0),
                "h" => hs = v.parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    (name, kills, deaths, hs)
}

fn format_stats_payload(st: &PlayerStats) -> String {
    let clean_name = st.name.replace(['|', '=', '\n', '\r'], " ");
    format!("n={clean_name}|k={}|d={}|h={}", st.kills, st.deaths, st.headshots)
}

fn compute_skill_score(kills: u32, deaths: u32, hs: u32) -> f64 {
    (f64::from(kills) * 2.0 + f64::from(hs) * 0.5) - f64::from(deaths) * 0.5
}

fn upsert_player_leaderboard(host: &mut impl HostBridge, slot: u8, st: &PlayerStats) {
    let key = player_key(host, slot);
    let score = compute_skill_score(st.kills, st.deaths, st.headshots);
    let payload = format_stats_payload(st);
    host.leaderboard_upsert(TABLE_RANK, &key, score, &payload);
}

fn render_top15_motd_html(host: &impl HostBridge) -> String {
    let rows = host.leaderboard_top(TABLE_RANK, 0, 15);
    let mut html = String::from(
        "<html><head><style>\
         body{background:#000000;color:#FFB000;font-family:Tahoma,Verdana,sans-serif;font-size:12px;margin:8px;}\
         h2{margin:0 0 8px 0;color:#FFB000;font-size:15px;letter-spacing:1px;border-bottom:1px solid #FFB000;padding-bottom:4px;}\
         table{width:100%;border-collapse:collapse;background:#0A0A0A;}\
         th{background:#221A08;color:#FFB000;text-align:left;padding:4px 6px;border:1px solid #554418;font-weight:bold;}\
         td{padding:3px 6px;border:1px solid #2A220C;color:#E6D29C;}\
         tr:nth-child(even) td{background:#12100A;}\
         .num{text-align:right;color:#FFB000;}\
         </style></head><body>\
         <h2>COUNTER-STRIKE 1.6 &mdash; TOP 15 PLAYERS</h2>\
         <table><tr><th>#</th><th>Player</th><th class='num'>Kills</th><th class='num'>Deaths</th><th class='num'>HS</th><th class='num'>K/D</th></tr>",
    );
    if rows.is_empty() {
        html.push_str("<tr><td colspan='6'>No ranked players recorded yet.</td></tr>");
    } else {
        for row in rows {
            let (parsed_name, kills, deaths, hs) = parse_stats_payload(&row.payload_json);
            let display = if parsed_name.is_empty() { row.key } else { parsed_name };
            let kd = if deaths == 0 {
                f64::from(kills)
            } else {
                f64::from(kills) / f64::from(deaths)
            };
            html.push_str(&format!(
                "<tr><td class='num'>{}</td><td>{}</td><td class='num'>{}</td><td class='num'>{}</td><td class='num'>{}</td><td class='num'>{:.2}</td></tr>",
                row.rank,
                html_escape(&display),
                kills,
                deaths,
                hs,
                kd
            ));
        }
    }
    html.push_str("</table></body></html>");
    html
}

fn announce_dhud(host: &mut impl HostBridge, rgb: [u8; 3], key: &str, args: &[(&str, &str)]) {
    for s in 0..host.player_max_clients() {
        if host.player_is_active(s) {
            let text = Player::new(s).tr(host, key, args);
            host.dhud_message(&DhudMessageSpec {
                slot: Some(s),
                x: -1.0,
                y: 0.22,
                effect: 0,
                rgb,
                fadein: 0.1,
                hold: 2.5,
                fadeout: 0.3,
                fxtime: 0.0,
                text,
            });
        }
    }
}

fn send_rank(host: &mut impl HostBridge, slot: u8) {
    let p = Player::new(slot);
    let key = player_key(host, slot);
    let (kills, deaths, hs) = {
        let mut st_lock = state().lock().unwrap_or_else(|e| e.into_inner());
        let st = st_lock.ensure(slot);
        if st.name.is_empty() {
            st.name = p.name(host);
        }
        upsert_player_leaderboard(host, slot, st);
        (st.kills, st.deaths, st.headshots)
    };
    let (rank, total) = host
        .leaderboard_rank(TABLE_RANK, &key)
        .map_or((1, 1), |(e, t)| (e.rank, t));
    let r_s = rank.to_string();
    let t_s = total.to_string();
    let k_s = kills.to_string();
    let d_s = deaths.to_string();
    let h_s = hs.to_string();
    p.say_tr(
        host,
        "STATS_RANK",
        &[("rank", &r_s), ("total", &t_s), ("kills", &k_s), ("hs", &h_s), ("deaths", &d_s)],
    );
}

#[cs_mod_init]
pub fn init_mod(host: &mut impl HostBridge) -> i32 {
    host.lang_load_toml(LANG_TOML);
    host.hook_register(HookId::RoundStart, HookPhase::Post, 0);
    host.hook_register(HookId::PlayerTakeDamage, HookPhase::Post, 0);
    host.hook_register(HookId::PlayerKilled, HookPhase::Post, 0);
    host.hook_register(HookId::PlayerSay, HookPhase::Pre, 0);
    host.hook_register(HookId::ClientCommand, HookPhase::Pre, 0);
    host.hook_register(HookId::BombPlanted, HookPhase::Post, 0);
    host.hook_register(HookId::BombDefused, HookPhase::Post, 0);
    host.register_command("rank");
    host.register_command("top15");
    0
}

pub fn dispatch_hook(host: &mut impl HostBridge, hook: HookId, phase: HookPhase) -> HookAction {
    match (hook, phase) {
        (HookId::RoundStart, HookPhase::Post) => {
            let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
            st.first_blood_taken = false;
            for p in &mut st.slots {
                p.round_dmg = 0;
                p.round_hits = 0;
                p.last_killer = None;
            }
            HookAction::Continue
        }
        (HookId::PlayerTakeDamage, HookPhase::Post) => {
            let victim = host.hook_arg_i32(0);
            let attacker = host.hook_arg_i32(1);
            let dmg = host.hook_arg_f32(0).max(0.0).round() as u32;
            if attacker >= 0 && attacker != victim && dmg > 0 {
                let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                let entry = st.ensure(attacker.clamp(0, 31) as u8);
                entry.round_dmg = entry.round_dmg.saturating_add(dmg);
                entry.round_hits = entry.round_hits.saturating_add(1);
            }
            HookAction::Continue
        }
        (HookId::PlayerKilled, HookPhase::Post) => {
            let victim_slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let killer_raw = host.hook_arg_i32(1);
            let headshot = host.hook_arg_i32(4) != 0;
            let team_kill = host.hook_arg_i32(5) != 0;
            let weapon = host.hook_arg_str(0).unwrap_or_else(|| "weapon".to_string());
            let victim_name = Player::new(victim_slot).name(host);

            let (first_blood, streak_after, killer_name_opt, is_knife) = {
                let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                let v_entry = st.ensure(victim_slot);
                v_entry.name = victim_name.clone();
                v_entry.deaths = v_entry.deaths.saturating_add(1);
                v_entry.streak = 0;

                if killer_raw >= 0 && killer_raw != i32::from(victim_slot) && !team_kill {
                    let kslot = killer_raw.clamp(0, 31) as u8;
                    let kp = Player::new(kslot);
                    let kname = kp.name(host);
                    let khp = kp.health(host).round() as i32;
                    let kap = kp.armor(host);
                    st.ensure(victim_slot).last_killer =
                        Some((kname.clone(), weapon.clone(), khp.max(0), kap.max(0)));
                    let v_snap = st.ensure(victim_slot).clone();
                    upsert_player_leaderboard(host, victim_slot, &v_snap);

                    let k_entry = st.ensure(kslot);
                    k_entry.name = kname.clone();
                    k_entry.kills = k_entry.kills.saturating_add(1);
                    if headshot {
                        k_entry.headshots = k_entry.headshots.saturating_add(1);
                    }
                    k_entry.streak = k_entry.streak.saturating_add(1);
                    let k_snap = k_entry.clone();
                    let streak = k_entry.streak;
                    upsert_player_leaderboard(host, kslot, &k_snap);

                    let fb = !st.first_blood_taken;
                    st.first_blood_taken = true;
                    (fb, streak, Some(kname), weapon.eq_ignore_ascii_case("knife"))
                } else {
                    let v_snap = st.ensure(victim_slot).clone();
                    upsert_player_leaderboard(host, victim_slot, &v_snap);
                    (false, 0, None, false)
                }
            };

            if let Some(kname) = killer_name_opt {
                if is_knife {
                    announce_dhud(
                        host,
                        [255, 80, 80],
                        "STATS_HUMILIATION",
                        &[("killer", &kname), ("victim", &victim_name)],
                    );
                } else if first_blood {
                    announce_dhud(host, [255, 176, 0], "STATS_FIRST_BLOOD", &[("player", &kname)]);
                } else {
                    match streak_after {
                        2 => announce_dhud(host, [255, 176, 0], "STATS_DOUBLE_KILL", &[("player", &kname)]),
                        3 => announce_dhud(host, [255, 140, 0], "STATS_MULTI_KILL", &[("player", &kname)]),
                        4 => announce_dhud(host, [255, 96, 48], "STATS_MONSTER_KILL", &[("player", &kname)]),
                        s if s >= 5 => announce_dhud(host, [255, 64, 64], "STATS_GODLIKE", &[("player", &kname)]),
                        _ => {}
                    }
                }
            }
            HookAction::Continue
        }
        (HookId::BombPlanted, HookPhase::Post) => {
            let c4 = host.cvar_get_f32("mp_c4timer");
            let secs = if c4 > 1.0 { c4.round() as i32 } else { 35 };
            let secs_s = secs.to_string();
            announce_dhud(host, [255, 64, 64], "STATS_C4_PLANTED", &[("secs", &secs_s)]);
            HookAction::Continue
        }
        (HookId::BombDefused, HookPhase::Post) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let name = Player::new(slot).name(host);
            announce_dhud(host, [116, 212, 116], "STATS_C4_DEFUSED", &[("player", &name)]);
            HookAction::Continue
        }
        (HookId::ClientCommand, HookPhase::Pre) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let cmd = host.hook_arg_str(0).unwrap_or_default().to_ascii_lowercase();
            match cmd.as_str() {
                "rank" => {
                    send_rank(host, slot);
                    HookAction::Handled
                }
                "top15" => {
                    let p = Player::new(slot);
                    let title = p.tr(host, "STATS_TOP15_TITLE", &[]);
                    let html = render_top15_motd_html(host);
                    p.show_motd(host, &title, &html);
                    HookAction::Handled
                }
                _ => HookAction::Continue,
            }
        }
        (HookId::PlayerSay, HookPhase::Pre) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let msg = host.hook_arg_str(0).unwrap_or_default();
            let tok = msg.trim().split_whitespace().next().unwrap_or("").to_ascii_lowercase();
            let p = Player::new(slot);
            match tok.as_str() {
                "/rank" | "rank" | "/statsme" => {
                    send_rank(host, slot);
                    HookAction::Handled
                }
                "/top15" | "top15" => {
                    let title = p.tr(host, "STATS_TOP15_TITLE", &[]);
                    let html = render_top15_motd_html(host);
                    p.show_motd(host, &title, &html);
                    HookAction::Supercede
                }
                "/me" => {
                    let (dmg, hits) = {
                        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                        let e = st.ensure(slot);
                        (e.round_dmg, e.round_hits)
                    };
                    let d_s = dmg.to_string();
                    let h_s = hits.to_string();
                    p.say_tr(host, "STATS_ME", &[("dmg", &d_s), ("hits", &h_s)]);
                    HookAction::Handled
                }
                "/hp" => {
                    let last = {
                        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                        st.ensure(slot).last_killer.clone()
                    };
                    if let Some((killer, weapon, hp, ap)) = last {
                        let hp_s = hp.to_string();
                        let ap_s = ap.to_string();
                        p.say_tr(
                            host,
                            "STATS_HP",
                            &[("killer", &killer), ("weapon", &weapon), ("hp", &hp_s), ("ap", &ap_s)],
                        );
                    } else {
                        p.say_tr(host, "STATS_HP_ALIVE", &[]);
                    }
                    HookAction::Handled
                }
                _ => HookAction::Continue,
            }
        }
        _ => HookAction::Continue,
    }
}

cs_mod_sdk::cs16_wasm_exports! {
    init: init_mod,
    hook: dispatch_hook,
}
