//! CSDM Client HUD (`client.wasm`) — Authentic CS 1.6 VGUI2 0px-corner respawn/streak overlay.

use std::sync::atomic::{AtomicU8, Ordering};

use cs_mod_sdk::{HostBridge, HudCanvas, vgui};

cs_mod_sdk::cs16_wasm_host_bridge!();

static STREAK: AtomicU8 = AtomicU8::new(0);
static PROTECTED: AtomicU8 = AtomicU8::new(0);

pub fn render_csdm_hud(host: &mut impl HostBridge, health: i32) {
    let streak = STREAK.load(Ordering::Relaxed);
    let prot = PROTECTED.load(Ordering::Relaxed) != 0;
    let mut canvas = HudCanvas::new();
    canvas.vgui_box(0.015, 0.18, 0.19, 0.095, "CSDM");
    let status = if prot {
        format!("PROTECTED | STREAK {streak}")
    } else {
        format!("STREAK {streak} | /guns")
    };
    canvas.amber_text(0.025, 0.215, 13.0, &status);
    let frac = (health.clamp(0, 100) as f32) / 100.0;
    let bar_col = if prot { vgui::HUD_CT_BLUE } else { vgui::HUD_AMBER };
    canvas.vgui_progress_bar(0.025, 0.248, 0.17, 0.016, frac, bar_col);
    canvas.flush(host);
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn cs_mod_client_hud(_dt_ms: i32, health: i32, _armor: i32, _money: i32) {
    let mut host = WasmHostBridge;
    render_csdm_hud(&mut host, health);
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn cs_mod_client_usermsg() -> i32 {
    let host = WasmHostBridge;
    let payload = host.hook_arg_bytes();
    if payload.len() >= 2 {
        STREAK.store(payload[0], Ordering::Relaxed);
        PROTECTED.store(payload[1], Ordering::Relaxed);
    }
    1
}
