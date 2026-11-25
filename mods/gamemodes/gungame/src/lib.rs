//! GunGame 26-Level Progression (`gungame`) — Rust -> WASM plugin.
//!
//! Full 26-weapon ladder (`glock18 -> ... -> awp -> hegrenade -> knife`), knife level-steal,
//! instant respawn timer, leader `GlowShell`, cyan `vfx::shockwave_ring` on level-up, and
//! classic amber HUD level progress bar.

use std::sync::{Mutex, OnceLock};

use cs_mod_sdk::{
    ArmorKind, HookAction, HookId, HookPhase, HostBridge, HudMessageSpec, Player, cs_mod_init, vfx,
};

cs_mod_sdk::cs16_wasm_host_bridge!();

const LANG_TOML: &str = include_str!("../lang.toml");
const TIMER_RESPAWN: i32 = 20;

const LADDER: [&str; 26] = [
    "glock18",
    "usp",
    "p228",
    "deagle",
    "fiveseven",
    "elite",
    "m3",
    "xm1014",
    "tmp",
    "mac10",
    "mp5navy",
    "ump45",
    "p90",
    "galil",
    "famas",
    "ak47",
    "m4a1",
    "sg552",
    "aug",
    "scout",
    "sg550",
    "g3sg1",
    "m249",
    "awp",
    "hegrenade",
    "knife",
];

#[derive(Default)]
struct GgState {
    levels: [u8; 32],
    leader_slot: Option<u8>,
}

static STATE: OnceLock<Mutex<GgState>> = OnceLock::new();

fn state() -> &'static Mutex<GgState> {
    STATE.get_or_init(|| Mutex::new(GgState::default()))
}

fn broadcast_tr(host: &mut impl HostBridge, key: &str, args: &[(&str, &str)]) {
    for s in 0..host.player_max_clients() {
        if host.player_is_active(s) {
            Player::new(s).say_tr(host, key, args);
        }
    }
}

fn equip_level(host: &mut impl HostBridge, slot: u8, level: u8) {
    let idx = usize::from(level).min(LADDER.len() - 1);
    let wep = LADDER[idx];
    let p = Player::new(slot);
    p.strip_weapons(host);
    p.give_item(host, "weapon_knife");
    if wep != "knife" {
        p.give_weapon(host, wep);
    }
    p.set_armor(host, 100, ArmorKind::VestHelm);
    let filled = ((usize::from(level) + 1) * 10) / LADDER.len();
    let mut bar = String::with_capacity(12);
    bar.push('[');
    for i in 0..10 {
        bar.push(if i < filled { '#' } else { '-' });
    }
    bar.push(']');
    host.hud_message(&HudMessageSpec {
        slot: Some(slot),
        channel: 3,
        x: 0.02,
        y: 0.25,
        effect: 0,
        rgba1: [255, 160, 0, 255],
        rgba2: [255, 220, 120, 255],
        fadein: 0.05,
        hold: 5.0,
        fadeout: 0.2,
        fxtime: 0.0,
        text: format!("GG LVL {}/26 {} {}", level + 1, wep.to_ascii_uppercase(), bar),
    });
}

fn refresh_leader_glow(host: &mut impl HostBridge) {
    let (prev, next_leader) = {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        let prev = st.leader_slot;
        let mut best_slot = None;
        let mut best_lvl = 0u8;
        for s in 0..host.player_max_clients() {
            if host.player_is_active(s) {
                let lv = st.levels[usize::from(s)];
                if best_slot.is_none() || lv > best_lvl {
                    best_slot = Some(s);
                    best_lvl = lv;
                }
            }
        }
        st.leader_slot = best_slot;
        (prev, best_slot)
    };
    if prev != next_leader {
        if let Some(old) = prev {
            Player::new(old).clear_rendering(host);
        }
        if let Some(new_l) = next_leader {
            Player::new(new_l).set_glow(host, [255, 200, 0], 18);
        }
    }
}

fn advance_player(host: &mut impl HostBridge, slot: u8) {
    let p = Player::new(slot);
    let name = p.name(host);
    let new_level = {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        let lv = &mut st.levels[usize::from(slot)];
        *lv = lv.saturating_add(1);
        *lv
    };

    if let Some(pos) = p.origin(host) {
        vfx::shockwave_ring(host, pos, 180.0, [255, 180, 0, 220], 8, 12, None);
    }

    if usize::from(new_level) >= LADDER.len() {
        broadcast_tr(host, "GG_VICTORY", &[("player", &name)]);
        host.center_print(None, &format!("GUNGAME VICTORY: {name}"));
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        st.levels = [0; 32];
        drop(st);
        host.server_exec("sv_restart 3");
        return;
    }

    equip_level(host, slot, new_level);
    let lv_s = (new_level + 1).to_string();
    let wep = LADDER[usize::from(new_level)].to_ascii_uppercase();
    broadcast_tr(host, "GG_LEVEL_UP", &[("player", &name), ("level", &lv_s), ("weapon", &wep)]);
    refresh_leader_glow(host);
}

#[cs_mod_init]
pub fn init_mod(host: &mut impl HostBridge) -> i32 {
    host.lang_load_toml(LANG_TOML);
    host.hook_register(HookId::PlayerSpawn, HookPhase::Post, 0);
    host.hook_register(HookId::PlayerKilled, HookPhase::Post, 0);
    host.hook_register(HookId::CanBuyItem, HookPhase::Pre, 0);
    host.hook_register(HookId::ClientCommand, HookPhase::Pre, 0);
    host.register_command("gg_kill");
    0
}

pub fn dispatch_hook(host: &mut impl HostBridge, hook: HookId, phase: HookPhase) -> HookAction {
    match (hook, phase) {
        (HookId::CanBuyItem, HookPhase::Pre) => HookAction::Supercede,
        (HookId::PlayerSpawn, HookPhase::Post) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let lv = state().lock().unwrap_or_else(|e| e.into_inner()).levels[usize::from(slot)];
            equip_level(host, slot, lv);
            HookAction::Continue
        }
        (HookId::PlayerKilled, HookPhase::Post) => {
            let victim = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let killer = host.hook_arg_i32(1);
            let team_kill = host.hook_arg_i32(5) != 0;
            let weapon = host.hook_arg_str(0).unwrap_or_default();
            if killer >= 0 && killer != i32::from(victim) && !team_kill {
                let kslot = killer.clamp(0, 31) as u8;
                if weapon.eq_ignore_ascii_case("knife") {
                    let v_name = Player::new(victim).name(host);
                    let k_name = Player::new(kslot).name(host);
                    {
                        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                        let v_lv = &mut st.levels[usize::from(victim)];
                        *v_lv = v_lv.saturating_sub(1);
                    }
                    broadcast_tr(host, "GG_STEAL", &[("killer", &k_name), ("victim", &v_name)]);
                }
                advance_player(host, kslot);
            }
            host.timer_after(2000, TIMER_RESPAWN, i32::from(victim));
            HookAction::Continue
        }
        (HookId::ClientCommand, HookPhase::Pre) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let cmd = host.hook_arg_str(0).unwrap_or_default();
            if cmd.eq_ignore_ascii_case("gg_kill") {
                advance_player(host, slot);
                return HookAction::Handled;
            }
            HookAction::Continue
        }
        _ => HookAction::Continue,
    }
}

pub fn dispatch_timer(host: &mut impl HostBridge, cb_id: i32, arg: i32) -> HookAction {
    if cb_id == TIMER_RESPAWN {
        let slot = arg.clamp(0, 31) as u8;
        if host.player_is_active(slot) {
            Player::new(slot).respawn(host);
        }
    }
    HookAction::Continue
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn cs_mod_init() -> i32 {
    let mut host = WasmHostBridge;
    init_mod(&mut host)
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn cs_mod_hook(hook_id: i32, phase: i32) -> i32 {
    let (Some(hook), Some(ph)) = (HookId::from_i32(hook_id), HookPhase::from_i32(phase)) else {
        return HookAction::Continue as i32;
    };
    let mut host = WasmHostBridge;
    dispatch_hook(&mut host, hook, ph) as i32
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn cs_mod_timer(cb_id: i32, arg: i32) -> i32 {
    let mut host = WasmHostBridge;
    dispatch_timer(&mut host, cb_id, arg) as i32
}
