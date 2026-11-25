//! Zombie Plague Biohazard (`zombie_plague`) server mod — Rust -> WASM (`side = "both"`).
//!
//! Features:
//! - Outbreak countdown selecting Patient Zero with green `vfx::aura_light` & biohazard `GlowShell`
//! - 3 Zombie classes (`Classic`, `Raptor`, `Tank`) selectable via `/zclass`, claws-only (`CanHavePlayerItem`
//!   blocks gun pickups), NVG enabled, bullet knockback
//! - Human Ammo Packs earned per damage dealt + Extra-Items shop (`/shop`, `/zp`):
//!   - **Golden AK-47** via declarative `CustomWeaponBuilder` (`register_custom_weapon`)
//!   - **Frost Grenade** (`vfx::shockwave_ring` cyan freeze ring)
//!   - **LaserMine** (`vfx::laser_beam` tripwire)
//! - Auto-downloaded VGUI2 Biohazard client HUD (`zp_hud` custom usermsg).

use std::sync::{Mutex, OnceLock};

use cs_mod_sdk::{
    CustomWeaponBuilder, HookAction, HookId, HookPhase, HostBridge, MENU_EXIT, PaginatedMenu,
    Player, cs_mod_init, vfx, weapon_id,
};

cs_mod_sdk::cs16_wasm_host_bridge!();

const LANG_TOML: &str = include_str!("../lang.toml");
const CB_SHOP_MENU: i32 = 1;
const CB_CLASS_MENU: i32 = 2;
const TIMER_OUTBREAK: i32 = 30;
const TIMER_UNFREEZE: i32 = 31;
const TIMER_WELCOME: i32 = 32;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ZombieClass {
    Classic,
    Raptor,
    Tank,
}

impl ZombieClass {
    fn stats(self) -> (&'static str, f32, f32) {
        match self {
            Self::Classic => ("Classic Zombie", 2200.0, 260.0),
            Self::Raptor => ("Raptor Zombie", 1400.0, 310.0),
            Self::Tank => ("Tank Zombie", 4000.0, 225.0),
        }
    }
}

struct ZpState {
    is_zombie: [bool; 32],
    zclass: [ZombieClass; 32],
    ammo_packs: [u32; 32],
    dmg_accum: [f32; 32],
    outbreak_started: bool,
}

impl Default for ZpState {
    fn default() -> Self {
        Self {
            is_zombie: [false; 32],
            zclass: [ZombieClass::Classic; 32],
            ammo_packs: [25; 32],
            dmg_accum: [0.0; 32],
            outbreak_started: false,
        }
    }
}

static STATE: OnceLock<Mutex<ZpState>> = OnceLock::new();

fn state() -> &'static Mutex<ZpState> {
    STATE.get_or_init(|| Mutex::new(ZpState::default()))
}

fn send_zp_hud(host: &mut impl HostBridge, slot: u8, zombie: bool, ap: u32) {
    let clamped = ap.min(u32::from(u16::MAX)) as u16;
    let payload = [u8::from(zombie), (clamped & 0xFF) as u8, (clamped >> 8) as u8];
    host.send_custom_usermsg(Some(slot), "zp_hud", &payload);
}

fn infect_player(host: &mut impl HostBridge, slot: u8, attacker: Option<u8>) {
    let (zclass, ap) = {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        st.is_zombie[usize::from(slot)] = true;
        (st.zclass[usize::from(slot)], st.ammo_packs[usize::from(slot)])
    };
    let (_, hp, speed) = zclass.stats();
    let p = Player::new(slot);
    p.strip_weapons(host);
    p.give_item(host, "weapon_knife");
    p.set_max_health(host, hp);
    p.set_health(host, hp);
    p.set_maxspeed(host, speed);
    p.set_nvg(host, true);
    p.set_glow(host, [80, 255, 80], 18);
    if let Some(pos) = p.origin(host) {
        vfx::aura_light(host, pos, 28, [64, 255, 64], 15, 10, None);
    }
    send_zp_hud(host, slot, true, ap);
    if let Some(att) = attacker {
        let vname = p.name(host);
        let aname = Player::new(att).name(host);
        for s in 0..host.player_max_clients() {
            if host.player_is_active(s) {
                Player::new(s).say_tr(host, "ZP_INFECTED", &[("victim", &vname), ("attacker", &aname)]);
            }
        }
    }
}

fn trigger_frost_nova(host: &mut impl HostBridge, origin: [f32; 3], caster: u8) {
    vfx::shockwave_ring(host, origin, 240.0, [0, 220, 255, 240], 10, 14, None);
    let zombies: Vec<u8> = {
        let st = state().lock().unwrap_or_else(|e| e.into_inner());
        (0..host.player_max_clients())
            .filter(|&s| st.is_zombie[usize::from(s)] && host.player_is_active(s))
            .collect()
    };
    for z in zombies {
        let zp = Player::new(z);
        if let Some(zpos) = zp.origin(host) {
            let dx = zpos[0] - origin[0];
            let dy = zpos[1] - origin[1];
            let dz = zpos[2] - origin[2];
            if dx * dx + dy * dy + dz * dz <= 240.0 * 240.0 {
                zp.set_maxspeed(host, 1.0);
                zp.set_glow(host, [0, 210, 255], 24);
                zp.screen_fade(host, 0.4, 2.5, 0, [0, 180, 255, 120]);
                host.timer_after(3000, TIMER_UNFREEZE, i32::from(z));
            }
        }
    }
    Player::new(caster).say_tr(host, "ZP_FROST_CAST", &[]);
}

fn deploy_lasermine(host: &mut impl HostBridge, slot: u8) {
    let p = Player::new(slot);
    let start = p.origin(host).unwrap_or([0.0, 0.0, 0.0]);
    let end = p
        .aim_origin(host)
        .unwrap_or([start[0] + 192.0, start[1], start[2]]);
    vfx::laser_beam(host, start, end, [0, 255, 128, 255], 40, 4, 0, None);
    p.say_tr(host, "ZP_LASER_PLACED", &[]);
}

fn open_shop_menu(host: &mut impl HostBridge, slot: u8) {
    let p = Player::new(slot);
    let ap = state().lock().unwrap_or_else(|e| e.into_inner()).ammo_packs[usize::from(slot)];
    let title = format!("\\y{} \\r[{ap} AP]\\w", p.tr(host, "ZP_SHOP_TITLE", &[]));
    PaginatedMenu::new(title, CB_SHOP_MENU)
        .item("Golden AK-47 (1.5x DMG) \\R\\y15 AP\\w", "golden_ak47")
        .item("Frost Grenade (Freeze Ring) \\R\\y8 AP\\w", "frost_nade")
        .item("LaserMine Tripwire \\R\\y10 AP\\w", "lasermine")
        .display_to(host, slot, 0, 20);
}

fn open_class_menu(host: &mut impl HostBridge, slot: u8) {
    let p = Player::new(slot);
    let title = format!("\\y{}\\w", p.tr(host, "ZP_CLASS_TITLE", &[]));
    PaginatedMenu::new(title, CB_CLASS_MENU)
        .item("Classic Zombie \\R\\y2200 HP / Balanced\\w", "classic")
        .item("Raptor Zombie \\R\\y1400 HP / Fast\\w", "raptor")
        .item("Tank Zombie \\R\\y4000 HP / Heavy\\w", "tank")
        .display_to(host, slot, 0, 20);
}

#[cs_mod_init]
pub fn init_mod(host: &mut impl HostBridge) -> i32 {
    host.lang_load_toml(LANG_TOML);

    CustomWeaponBuilder::new("golden_ak47", weapon_id::AK47)
        .display_name("Golden AK-47")
        .v_model("models/v_golden_ak47.mdl")
        .p_model("models/p_golden_ak47.mdl")
        .damage_mult(1.5)
        .clip_size(35)
        .knockback(1.8)
        .cost(15)
        .register(host);

    host.hook_register(HookId::RoundStart, HookPhase::Post, 0);
    host.hook_register(HookId::ClientPutInServer, HookPhase::Post, 0);
    host.hook_register(HookId::PlayerTakeDamage, HookPhase::Pre, 0);
    host.hook_register(HookId::PlayerTakeDamage, HookPhase::Post, 0);
    host.hook_register(HookId::CanHavePlayerItem, HookPhase::Pre, 0);
    host.hook_register(HookId::GrenadeThrow, HookPhase::Post, 0);
    host.hook_register(HookId::PlayerSay, HookPhase::Pre, 0);
    host.hook_register(HookId::ClientCommand, HookPhase::Pre, 0);
    host.register_command("zp_shop");
    host.register_command("zp_infect");
    0
}

pub fn dispatch_hook(host: &mut impl HostBridge, hook: HookId, phase: HookPhase) -> HookAction {
    match (hook, phase) {
        (HookId::RoundStart, HookPhase::Post) => {
            {
                let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                st.is_zombie = [false; 32];
                st.outbreak_started = false;
            }
            for s in 0..host.player_max_clients() {
                if host.player_is_active(s) {
                    let p = Player::new(s);
                    p.clear_rendering(host);
                    let ap = state().lock().unwrap_or_else(|e| e.into_inner()).ammo_packs[usize::from(s)];
                    send_zp_hud(host, s, false, ap);
                    let msg = p.tr(host, "ZP_COUNTDOWN", &[("secs", "5")]);
                    p.center_print(host, &msg);
                }
            }
            host.timer_after(5000, TIMER_OUTBREAK, 0);
            HookAction::Continue
        }
        (HookId::ClientPutInServer, HookPhase::Post) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let ap = state().lock().unwrap_or_else(|e| e.into_inner()).ammo_packs[usize::from(slot)];
            send_zp_hud(host, slot, false, ap);
            host.timer_after(30, TIMER_WELCOME, i32::from(slot));
            HookAction::Continue
        }
        (HookId::CanHavePlayerItem, HookPhase::Pre) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as usize;
            let wid = host.hook_arg_i32(1) as u8;
            let is_z = state().lock().unwrap_or_else(|e| e.into_inner()).is_zombie[slot];
            if is_z && wid != weapon_id::KNIFE {
                return HookAction::Supercede;
            }
            HookAction::Continue
        }
        (HookId::PlayerTakeDamage, HookPhase::Pre) => {
            let victim = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let attacker = host.hook_arg_i32(1);
            if attacker >= 0 && attacker != i32::from(victim) {
                let att = attacker.clamp(0, 31) as u8;
                let (att_z, vic_z) = {
                    let st = state().lock().unwrap_or_else(|e| e.into_inner());
                    (st.is_zombie[usize::from(att)], st.is_zombie[usize::from(victim)])
                };
                if att_z && !vic_z {
                    infect_player(host, victim, Some(att));
                    return HookAction::Supercede;
                }
            }
            HookAction::Continue
        }
        (HookId::PlayerTakeDamage, HookPhase::Post) => {
            let victim = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let attacker = host.hook_arg_i32(1);
            let dmg = host.hook_arg_f32(0).max(0.0);
            if attacker >= 0 && attacker != i32::from(victim) && dmg > 0.0 {
                let att = attacker.clamp(0, 31) as u8;
                let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                if !st.is_zombie[usize::from(att)] && st.is_zombie[usize::from(victim)] {
                    st.dmg_accum[usize::from(att)] += dmg;
                    while st.dmg_accum[usize::from(att)] >= 200.0 {
                        st.dmg_accum[usize::from(att)] -= 200.0;
                        st.ammo_packs[usize::from(att)] = st.ammo_packs[usize::from(att)].saturating_add(1);
                    }
                    let ap = st.ammo_packs[usize::from(att)];
                    drop(st);
                    send_zp_hud(host, att, false, ap);
                }
            }
            HookAction::Continue
        }
        (HookId::GrenadeThrow, HookPhase::Post) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let wid = host.hook_arg_i32(1) as u8;
            if wid == weapon_id::FLASHBANG
                && let Some(pos) = Player::new(slot).origin(host)
            {
                trigger_frost_nova(host, pos, slot);
            }
            HookAction::Continue
        }
        (HookId::ClientCommand, HookPhase::Pre) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let cmd = host.hook_arg_str(0).unwrap_or_default().to_ascii_lowercase();
            match cmd.as_str() {
                "zp_shop" => {
                    open_shop_menu(host, slot);
                    HookAction::Handled
                }
                "zp_infect" => {
                    infect_player(host, slot, None);
                    HookAction::Handled
                }
                _ => HookAction::Continue,
            }
        }
        (HookId::PlayerSay, HookPhase::Pre) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let msg = host.hook_arg_str(0).unwrap_or_default();
            let tok = msg.trim().split_whitespace().next().unwrap_or("").to_ascii_lowercase();
            match tok.as_str() {
                "/shop" | "/zp" => {
                    open_shop_menu(host, slot);
                    HookAction::Supercede
                }
                "/zclass" => {
                    open_class_menu(host, slot);
                    HookAction::Supercede
                }
                "/frost" => {
                    let pos = Player::new(slot).origin(host).unwrap_or([0.0; 3]);
                    trigger_frost_nova(host, pos, slot);
                    HookAction::Supercede
                }
                "/lm" => {
                    deploy_lasermine(host, slot);
                    HookAction::Supercede
                }
                _ => HookAction::Continue,
            }
        }
        _ => HookAction::Continue,
    }
}

pub fn dispatch_timer(host: &mut impl HostBridge, cb_id: i32, arg: i32) -> HookAction {
    match cb_id {
        TIMER_OUTBREAK => {
            let first_active = (0..host.player_max_clients()).find(|&s| host.player_is_active(s));
            if let Some(pz) = first_active {
                {
                    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                    if st.outbreak_started {
                        return HookAction::Continue;
                    }
                    st.outbreak_started = true;
                }
                infect_player(host, pz, None);
                let pname = Player::new(pz).name(host);
                for s in 0..host.player_max_clients() {
                    if host.player_is_active(s) {
                        let msg = Player::new(s).tr(host, "ZP_OUTBREAK", &[("player", &pname)]);
                        Player::new(s).center_print(host, &msg);
                    }
                }
            }
        }
        TIMER_UNFREEZE => {
            let slot = arg.clamp(0, 31) as u8;
            let zclass = state().lock().unwrap_or_else(|e| e.into_inner()).zclass[usize::from(slot)];
            let (_, _, speed) = zclass.stats();
            let zp = Player::new(slot);
            zp.set_maxspeed(host, speed);
            zp.set_glow(host, [80, 255, 80], 18);
        }
        TIMER_WELCOME => {
            let slot = arg.clamp(0, 31) as u8;
            if host.player_is_active(slot) {
                Player::new(slot).center_print(host, "ZOMBIE OUTBREAK!");
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
    if item_index == MENU_EXIT {
        return HookAction::Handled;
    }
    let info = host.hook_arg_str(0).unwrap_or_default();
    let p = Player::new(slot);
    match cb_id {
        CB_SHOP_MENU => {
            let cost: u32 = match info.as_str() {
                "golden_ak47" => 15,
                "frost_nade" => 8,
                "lasermine" => 10,
                _ => return HookAction::Handled,
            };
            let (ok, have) = {
                let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                let cur = st.ammo_packs[usize::from(slot)];
                if cur >= cost {
                    st.ammo_packs[usize::from(slot)] = cur - cost;
                    (true, cur - cost)
                } else {
                    (false, cur)
                }
            };
            if !ok {
                let c_s = cost.to_string();
                let h_s = have.to_string();
                p.say_tr(host, "ZP_NO_AP", &[("cost", &c_s), ("have", &h_s)]);
                return HookAction::Handled;
            }
            send_zp_hud(host, slot, false, have);
            match info.as_str() {
                "golden_ak47" => {
                    p.give_custom_weapon(host, "golden_ak47");
                    p.say_tr(host, "ZP_BOUGHT_GOLD_AK", &[("cost", "15")]);
                }
                "frost_nade" => {
                    p.give_item(host, "weapon_flashbang");
                    let pos = p.origin(host).unwrap_or([0.0; 3]);
                    trigger_frost_nova(host, pos, slot);
                }
                "lasermine" => deploy_lasermine(host, slot),
                _ => {}
            }
        }
        CB_CLASS_MENU => {
            let chosen = match info.as_str() {
                "raptor" => ZombieClass::Raptor,
                "tank" => ZombieClass::Tank,
                _ => ZombieClass::Classic,
            };
            state().lock().unwrap_or_else(|e| e.into_inner()).zclass[usize::from(slot)] = chosen;
            let (label, _, _) = chosen.stats();
            p.say_tr(host, "ZP_CLASS_PICKED", &[("class", label)]);
        }
        _ => {}
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
