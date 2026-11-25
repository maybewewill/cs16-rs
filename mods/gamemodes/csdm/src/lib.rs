//! Counter-Strike Deathmatch (`csdm`) server mod — Rust -> WASM (`side = "both"`).
//!
//! Features:
//! - Instant respawn (`1.5 s` timer) at spawn points with `2.0 s` `GlowShell` spawn protection
//! - Paginated `/guns` / `guns` weapon menu with `"Remember selection"` persisted per player
//! - Floor weapon cleanup on death and `csdm_hud` killstreak packets for the VGUI2 client HUD.

use std::sync::{Mutex, OnceLock};

use cs_mod_sdk::{
    ArmorKind, HookAction, HookId, HookPhase, HostBridge, MENU_EXIT, PaginatedMenu, Player,
    TeamKind, cs_mod_init,
};

cs_mod_sdk::cs16_wasm_host_bridge!();

const LANG_TOML: &str = include_str!("../lang.toml");
const CB_GUNS_MENU: i32 = 1;
const TIMER_RESPAWN: i32 = 10;
const TIMER_UNPROTECT: i32 = 11;
const TIMER_WELCOME: i32 = 12;

const PRIMARY_GUNS: [(&str, &str); 6] = [
    ("AK-47 Kalashnikov", "weapon_ak47"),
    ("M4A1 Carbine", "weapon_m4a1"),
    ("AWP Magnum Sniper", "weapon_awp"),
    ("FAMAS Clarion", "weapon_famas"),
    ("Galil Defender", "weapon_galil"),
    ("MP5 Navy", "weapon_mp5navy"),
];

#[derive(Default)]
struct CsdmState {
    protected_until_ms: [i64; 32],
    remembered_weapon: [Option<String>; 32],
    remember_enabled: [bool; 32],
    streaks: [u8; 32],
}

static STATE: OnceLock<Mutex<CsdmState>> = OnceLock::new();

fn state() -> &'static Mutex<CsdmState> {
    STATE.get_or_init(|| Mutex::new(CsdmState::default()))
}

fn equip_loadout(host: &mut impl HostBridge, slot: u8, primary: &str) {
    let p = Player::new(slot);
    p.strip_weapons(host);
    p.give_item(host, "weapon_knife");
    p.give_item(host, primary);
    p.give_item(host, "weapon_deagle");
    p.set_armor(host, 100, ArmorKind::VestHelm);
    let label = primary.strip_prefix("weapon_").unwrap_or(primary).to_ascii_uppercase();
    p.say_tr(host, "CSDM_EQUIPPED", &[("weapon", &label)]);
}

fn open_guns_menu(host: &mut impl HostBridge, slot: u8) {
    let p = Player::new(slot);
    let title = format!("\\y{}\\w", p.tr(host, "CSDM_GUNS_TITLE", &[]));
    let rem_lbl = p.tr(host, "CSDM_REMEMBER", &[]);
    let remember = state().lock().unwrap_or_else(|e| e.into_inner()).remember_enabled[usize::from(slot)];
    let mut menu = PaginatedMenu::new(title, CB_GUNS_MENU).per_page(7);
    for (label, item) in PRIMARY_GUNS {
        menu = menu.item(label, item);
    }
    let rem_tag = if remember { "\\y[ON]\\w" } else { "\\d[OFF]\\w" };
    menu = menu.item(format!("{rem_lbl} \\R{rem_tag}"), "__toggle_remember__");
    menu.display_to(host, slot, 0, 20);
}

fn apply_spawn_protection(host: &mut impl HostBridge, slot: u8) {
    let now = host.time_ms();
    let p = Player::new(slot);
    {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        st.protected_until_ms[usize::from(slot)] = now + 2000;
    }
    let rgb = match p.team(host) {
        TeamKind::Terrorist => [255, 72, 72],
        _ => [96, 176, 255],
    };
    p.set_glow(host, rgb, 20);
    host.timer_after(2000, TIMER_UNPROTECT, i32::from(slot));
    let msg = p.tr(host, "CSDM_RESPAWN", &[]);
    p.center_print(host, &msg);
}

fn send_hud_state(host: &mut impl HostBridge, slot: u8, streak: u8, protected: bool) {
    let payload = [streak, u8::from(protected)];
    host.send_custom_usermsg(Some(slot), "csdm_hud", &payload);
}

#[cs_mod_init]
pub fn init_mod(host: &mut impl HostBridge) -> i32 {
    host.lang_load_toml(LANG_TOML);
    host.hook_register(HookId::ClientPutInServer, HookPhase::Post, 0);
    host.hook_register(HookId::PlayerSpawn, HookPhase::Post, 0);
    host.hook_register(HookId::PlayerTakeDamage, HookPhase::Pre, 0);
    host.hook_register(HookId::PlayerKilled, HookPhase::Post, 0);
    host.hook_register(HookId::PlayerSay, HookPhase::Pre, 0);
    host.hook_register(HookId::ClientCommand, HookPhase::Pre, 0);
    host.register_command("guns");
    host.register_command("dm_respawn");
    0
}

pub fn dispatch_hook(host: &mut impl HostBridge, hook: HookId, phase: HookPhase) -> HookAction {
    match (hook, phase) {
        (HookId::ClientPutInServer, HookPhase::Post) | (HookId::PlayerSpawn, HookPhase::Post) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            apply_spawn_protection(host, slot);
            if hook == HookId::ClientPutInServer {
                host.timer_after(30, TIMER_WELCOME, i32::from(slot));
            }
            let (remember, saved, streak) = {
                let st = state().lock().unwrap_or_else(|e| e.into_inner());
                let i = usize::from(slot);
                (st.remember_enabled[i], st.remembered_weapon[i].clone(), st.streaks[i])
            };
            send_hud_state(host, slot, streak, true);
            if remember && let Some(ref wep) = saved {
                equip_loadout(host, slot, wep);
            } else if hook == HookId::PlayerSpawn {
                open_guns_menu(host, slot);
            }
            HookAction::Continue
        }
        (HookId::PlayerTakeDamage, HookPhase::Pre) => {
            let victim = host.hook_arg_i32(0).clamp(0, 31) as usize;
            let now = host.time_ms();
            let until = state().lock().unwrap_or_else(|e| e.into_inner()).protected_until_ms[victim];
            if now < until {
                return HookAction::Supercede;
            }
            HookAction::Continue
        }
        (HookId::PlayerKilled, HookPhase::Post) => {
            let victim = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let killer = host.hook_arg_i32(1);
            {
                let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                st.streaks[usize::from(victim)] = 0;
                if killer >= 0 && killer != i32::from(victim) {
                    let ks = killer.clamp(0, 31) as u8;
                    st.streaks[usize::from(ks)] = st.streaks[usize::from(ks)].saturating_add(1);
                    let s_val = st.streaks[usize::from(ks)];
                    drop(st);
                    send_hud_state(host, ks, s_val, false);
                }
            }
            send_hud_state(host, victim, 0, false);
            host.timer_after(1500, TIMER_RESPAWN, i32::from(victim));
            HookAction::Continue
        }
        (HookId::ClientCommand, HookPhase::Pre) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let cmd = host.hook_arg_str(0).unwrap_or_default().to_ascii_lowercase();
            match cmd.as_str() {
                "guns" => {
                    state().lock().unwrap_or_else(|e| e.into_inner()).remember_enabled[usize::from(slot)] = false;
                    open_guns_menu(host, slot);
                    HookAction::Handled
                }
                "dm_respawn" => {
                    Player::new(slot).respawn(host);
                    apply_spawn_protection(host, slot);
                    HookAction::Handled
                }
                _ => HookAction::Continue,
            }
        }
        (HookId::PlayerSay, HookPhase::Pre) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let msg = host.hook_arg_str(0).unwrap_or_default();
            let tok = msg.trim().split_whitespace().next().unwrap_or("");
            if tok.eq_ignore_ascii_case("guns") || tok.eq_ignore_ascii_case("/guns") {
                state().lock().unwrap_or_else(|e| e.into_inner()).remember_enabled[usize::from(slot)] = false;
                open_guns_menu(host, slot);
                return HookAction::Supercede;
            }
            HookAction::Continue
        }
        _ => HookAction::Continue,
    }
}

pub fn dispatch_timer(host: &mut impl HostBridge, cb_id: i32, arg: i32) -> HookAction {
    let slot = arg.clamp(0, 31) as u8;
    match cb_id {
        TIMER_RESPAWN => {
            if host.player_is_active(slot) {
                Player::new(slot).respawn(host);
            }
        }
        TIMER_UNPROTECT => {
            let streak = {
                let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                st.protected_until_ms[usize::from(slot)] = 0;
                st.streaks[usize::from(slot)]
            };
            if host.player_is_active(slot) {
                Player::new(slot).clear_rendering(host);
                send_hud_state(host, slot, streak, false);
            }
        }
        TIMER_WELCOME => {
            if host.player_is_active(slot) {
                let p = Player::new(slot);
                let msg = p.tr(host, "CSDM_RESPAWN", &[]);
                p.center_print(host, &msg);
            }
        }
        _ => {}
    }
    HookAction::Continue
}

pub fn dispatch_menu(
    host: &mut impl HostBridge,
    cb_id: i32,
    slot: u8,
    _menu_id: u32,
    item_index: i32,
) -> HookAction {
    if cb_id != CB_GUNS_MENU || item_index == MENU_EXIT {
        return HookAction::Handled;
    }
    let info = host.hook_arg_str(0).unwrap_or_default();
    let idx = usize::from(slot);
    if info == "__toggle_remember__" {
        {
            let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
            st.remember_enabled[idx] = !st.remember_enabled[idx];
        }
        open_guns_menu(host, slot);
        return HookAction::Handled;
    }
    if info.starts_with("weapon_") {
        {
            let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
            st.remembered_weapon[idx] = Some(info.clone());
        }
        host.storage_set(&format!("gun_{slot}"), &info);
        equip_loadout(host, slot, &info);
    }
    HookAction::Handled
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

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn cs_mod_menu(cb_id: i32, slot: i32, menu_id: i32, item_index: i32) -> i32 {
    let mut host = WasmHostBridge;
    dispatch_menu(&mut host, cb_id, slot.clamp(0, 31) as u8, menu_id as u32, item_index) as i32
}
