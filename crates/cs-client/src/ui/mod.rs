//! VGUI2 UI orchestrator plus the in-game HUD, the scoreboard, the server menus, the crosshair and `net_graph`.

pub mod chat;
pub mod console;
pub mod hud;
pub mod hud_extras;
pub mod main_menu;
pub mod mod_hud;
pub mod options;
pub mod scoreboard;
pub mod screens;
pub mod server_menu;
pub mod vgui2;
pub mod vgui_assets;
pub mod vgui_theme;

use cs_cvar::CVarRegistry;
use egui::{Color32, FontId, Pos2, Stroke};

use crate::ui::vgui_theme::VguiPalette;

pub struct FrameTelemetry<'a> {
    pub fps: f32,
    pub frame_ms: f32,
    pub map_name: &'a str,
    pub pos: [f32; 3],
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub speed: f32,
    pub draw_calls: usize,
    pub triangles: usize,
}

/// Draw the classic 4-bar Counter-Strike 1.6 crosshair in the center of the screen.
pub fn draw_crosshair(ctx: &egui::Context, cvars: &CVarRegistry, dynamic_extra_gap: f32) {
    let screen = ctx.content_rect();
    let center = screen.center();

    let (r, g, b) = parse_rgb(cvars.get_str("cl_crosshair_color"));
    let alpha = if cvars.get_bool("cl_crosshair_translucent") {
        195
    } else {
        255
    };
    let color = Color32::from_rgba_unmultiplied(r, g, b, alpha);

    let (len, base_gap) = match cvars.get_str("cl_crosshair_size").to_ascii_lowercase().as_str() {
        "small" => (6.0f32, 4.0f32),
        "large" => (12.0f32, 6.0f32),
        _ => (9.0f32, 5.0f32), // medium / auto
    };
    let gap = base_gap + dynamic_extra_gap.clamp(0.0, 22.0);

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::unique("cs16_crosshair"),
    ));
    let stroke = Stroke::new(1.5, color);
    let cx = center.x.round();
    let cy = center.y.round();

    // Top, Bottom, Left, Right segments
    painter.line_segment([Pos2::new(cx, cy - gap - len), Pos2::new(cx, cy - gap)], stroke);
    painter.line_segment([Pos2::new(cx, cy + gap), Pos2::new(cx, cy + gap + len)], stroke);
    painter.line_segment([Pos2::new(cx - gap - len, cy), Pos2::new(cx - gap, cy)], stroke);
    painter.line_segment([Pos2::new(cx + gap, cy), Pos2::new(cx + gap + len, cy)], stroke);
}

/// Draw classic CS 1.6 `net_graph 1` / `net_graph 3` telemetry in the bottom-right corner.
pub fn draw_net_graph(ctx: &egui::Context, cvars: &CVarRegistry, t: &FrameTelemetry<'_>, palette: VguiPalette) {
    let mode = cvars.get_i32("net_graph");
    if mode <= 0 {
        return;
    }

    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_net_graph"),
    ));

    let gold = Color32::from_rgb(235, 210, 120);
    let font = FontId::monospace(12.0);
    let pos_mode = cvars.get_i32("net_graphpos");
    // Position cleanly above the bottom HUD bar so ammo/money/HP/timer are never overlapped.
    let (x, y, align) = match pos_mode {
        2 => (screen.center().x, screen.bottom() - 62.0, egui::Align2::CENTER_BOTTOM),
        3 => (16.0, screen.bottom() - 78.0, egui::Align2::LEFT_BOTTOM),
        _ => (screen.right() - 16.0, screen.bottom() - 122.0, egui::Align2::RIGHT_BOTTOM),
    };

    let lines = if mode == 3 {
        vec![format!(
            "{:.0} fps  {:.2} ms   map: {}   spd: {:.0} u/s",
            t.fps, t.frame_ms, t.map_name, t.speed
        )]
    } else {
        vec![
            format!("{:.1} fps  ({:.2} ms)  batches: {}  tris: {}", t.fps, t.frame_ms, t.draw_calls, t.triangles),
            format!(
                "map: {}  pos: {:.0} {:.0} {:.0}  ang: {:.0}/{:.0}  spd: {:.0} u/s",
                t.map_name, t.pos[0], t.pos[1], t.pos[2], t.yaw_deg, t.pitch_deg, t.speed
            ),
        ]
    };

    for (idx, line) in lines.iter().rev().enumerate() {
        let pos = Pos2::new(x, y - (idx as f32) * 15.0);
        painter.text(
            pos + egui::vec2(1.0, 1.0),
            align,
            line,
            font.clone(),
            Color32::from_black_alpha(220),
        );
        painter.text(pos, align, line, font.clone(), gold);
    }
    let _ = palette;
}

pub fn parse_rgb(s: &str) -> (u8, u8, u8) {
    let mut parts = s.split_whitespace().filter_map(|p| p.parse::<u8>().ok());
    let r = parts.next().unwrap_or(50);
    let g = parts.next().unwrap_or(250);
    let b = parts.next().unwrap_or(50);
    (r, g, b)
}
