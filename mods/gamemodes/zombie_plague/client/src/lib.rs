//! Zombie Plague Client Biohazard HUD (`client.wasm`) — Authentic CS 1.6 VGUI2 0px-corner overlay.

use std::sync::atomic::{AtomicU16, AtomicU8, Ordering};

use cs_mod_sdk::{HookAction, HostBridge, HudCanvas, cs_mod_hud, vgui};

cs_mod_sdk::cs16_wasm_host_bridge!();

static IS_ZOMBIE: AtomicU8 = AtomicU8::new(0);
static AMMO_PACKS: AtomicU16 = AtomicU16::new(25);

pub fn render_zp_hud(host: &mut impl HostBridge, health: i32) {
    let zombie = IS_ZOMBIE.load(Ordering::Relaxed) != 0;
    let ap = AMMO_PACKS.load(Ordering::Relaxed);
    let mut canvas = HudCanvas::new();
    canvas.vgui_box(0.015, 0.18, 0.22, 0.105, "BIOHAZARD ZP");
    let role = if zombie { "ZOMBIE" } else { "HUMAN" };
    let line = format!("{role} | AP: {ap} | /shop");
    let col = if zombie { vgui::HUD_GREEN } else { vgui::HUD_AMBER };
    canvas.text(0.025, 0.218, 13.0, col, &line);
    let max_hp = if zombie { 2200.0 } else { 100.0 };
    let frac = ((health.max(0) as f32) / max_hp).clamp(0.0, 1.0);
    canvas.vgui_progress_bar(0.025, 0.252, 0.20, 0.016, frac, col);
    canvas.flush(host);
}

#[cs_mod_hud]
pub fn on_zp_hud(host: &mut impl HostBridge, _dt_ms: i32, health: i32, _armor: i32, _money: i32) {
    render_zp_hud(host, health);
}

pub fn on_zp_usermsg(host: &mut impl HostBridge) -> HookAction {
    let payload = host.hook_arg_bytes();
    if payload.len() >= 3 {
        IS_ZOMBIE.store(payload[0], Ordering::Relaxed);
        let ap = u16::from_le_bytes([payload[1], payload[2]]);
        AMMO_PACKS.store(ap, Ordering::Relaxed);
    }
    HookAction::Handled
}

cs_mod_sdk::cs16_wasm_exports! {
    client_hud: on_zp_hud,
    client_usermsg: on_zp_usermsg,
}
