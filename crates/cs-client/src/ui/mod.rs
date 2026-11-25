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
use egui::{Color32, FontId, Pos2};

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

/// Live player/weapon inputs driving the authentic `FUN_10043ff0` (`CHudAmmo::DrawCrosshair`) dynamic crosshair.
#[derive(Debug, Clone, Copy, Default)]
pub struct CrosshairInputs {
    pub weapon_id: u8,
    pub weapon_flags: u32,
    pub on_ground: bool,
    pub ducking: bool,
    pub speed_2d: f32,
    pub shots_fired: i32,
    pub night_vision: bool,
    pub dynamic: bool,
    pub hide_hud: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrosshairGeometry {
    pub gap: i32,
    pub bar_len: i32,
    pub alpha: u8,
}

/// Persistent per-client crosshair expansion/fade state matching GoldSrc `CHudAmmo::DrawCrosshair` (`FUN_10043ff0`).
#[derive(Debug, Clone, Copy)]
pub struct CrosshairState {
    pub crosshair_dist: f32,
    pub alpha: i32,
    pub last_shots_fired: i32,
}

impl Default for CrosshairState {
    fn default() -> Self {
        Self {
            crosshair_dist: 0.0,
            alpha: 255,
            last_shots_fired: 0,
        }
    }
}

impl CrosshairState {
    /// Per-weapon `(start_distance, delta_distance)` from `client.dll` `FUN_10043ff0` (`param_2` in `1..=30`).
    pub fn weapon_table(weapon_id: u8) -> (i32, i32) {
        if !(1..=30).contains(&weapon_id) {
            return (4, 3);
        }
        match weapon_id - 1 {
            0 | 3 | 8 | 10 | 15 | 16 | 17 | 24 | 25 => (8, 3),
            2 | 12 | 26 => (5, 3),
            4 => (9, 4),
            5 | 11 | 19 => (6, 3),
            6 => (9, 3),
            7 => (3, 3),
            18 => (6, 2),
            20 => (8, 6),
            22 | 28 | 29 => (7, 3),
            23 => (6, 4),
            27 => (4, 4),
            _ => (4, 3),
        }
    }

    /// Compute gap, bar length, and alpha matching `client.dll` `FUN_10043ff0`.
    pub fn compute_geometry(
        &mut self,
        inputs: &CrosshairInputs,
        size_cvar: &str,
        screen_w: i32,
    ) -> CrosshairGeometry {
        const ACC_CODE: [u8; 30] = [
            0, 8, 8, 1, 1, 1, 2, 3,
            8, 8, 0, 2, 1, 3, 4, 5,
            6, 8, 2, 3, 1, 7, 2, 8,
            1, 0, 3, 3, 1, 3,
        ];
        const SPD_CODE: [u8; 30] = [
            0, 3, 3, 3, 3, 3, 3, 1,
            3, 3, 0, 3, 3, 1, 1, 0,
            0, 3, 3, 1, 3, 1, 3, 3,
            3, 0, 1, 1, 3, 2,
        ];
        const SPD_THRESH: [i32; 4] = [0, 140, 170, 0];

        let (mut distance, delta_distance) = Self::weapon_table(inputs.weapon_id);
        let idx = inputs.weapon_id.saturating_sub(1) as usize;

        if inputs.dynamic && (inputs.hide_hud & 1) == 0 && idx < 30 {
            let code = ACC_CODE[idx];
            if code != 1 && code != 8 {
                let wf = inputs.weapon_flags;
                let flags = match code {
                    0 => 0x07,
                    2 => 0x01,
                    3 => 0x03,
                    4 => (wf & 0x10) | 0x03,
                    5 => ((wf & 0x01) << 3) | 0x07,
                    6 => ((wf & 0x02) << 3) | 0x07,
                    7 => ((wf & 0x04) << 1) | 0x03,
                    _ => 0,
                };
                let speed_thresh = SPD_THRESH[SPD_CODE[idx] as usize];
                if !inputs.on_ground && (flags & 0x01) != 0 {
                    distance = (distance as f32 * 2.0) as i32;
                } else if inputs.ducking && (flags & 0x04) != 0 {
                    distance = (distance as f32 * 0.5) as i32;
                } else if (inputs.speed_2d as i32) > speed_thresh && (flags & 0x02) != 0 {
                    distance = (distance as f32 * 1.5) as i32;
                }
                if (flags & 0x08) != 0 {
                    distance = (distance as f32 * 1.4) as i32;
                }
                if (flags & 0x10) != 0 {
                    distance = (distance as f32 * 1.4) as i32;
                }
            }
        }

        if self.last_shots_fired < inputs.shots_fired {
            self.alpha = (self.alpha - 40).max(120);
            self.crosshair_dist = (self.crosshair_dist + delta_distance as f32).min(15.0);
            self.last_shots_fired = inputs.shots_fired;
        } else {
            if inputs.shots_fired < self.last_shots_fired {
                self.last_shots_fired = inputs.shots_fired;
            }
            self.alpha = (self.alpha + 2).min(255);
            self.crosshair_dist -= self.crosshair_dist * 0.013 + 0.1;
        }

        let base_dist = distance as f32;
        if self.crosshair_dist < base_dist {
            self.crosshair_dist = base_dist;
        }

        let mut bar_len = 5 - (((self.crosshair_dist - base_dist) * -0.5) as i32);
        let sw = screen_w.max(320);
        let size_base = match size_cvar.trim().to_ascii_lowercase().as_str() {
            "small" | "1" => 1024,
            "medium" | "2" => 800,
            "large" | "3" => 640,
            _ => {
                if sw <= 640 {
                    1024
                } else if sw < 1024 {
                    800
                } else {
                    640
                }
            }
        };

        let gap_f = if sw != size_base {
            bar_len = (bar_len * sw) / size_base;
            (sw as f32 * self.crosshair_dist) / (size_base as f32)
        } else {
            self.crosshair_dist
        };

        CrosshairGeometry {
            gap: (gap_f as i32).max(1),
            bar_len: bar_len.max(1),
            alpha: self.alpha.clamp(0, 255) as u8,
        }
    }
}

/// Resolve crosshair RGBA color matching `client.dll` `FUN_10043ff0` (`0x10044530..10044560`):
/// NVG overrides RGB to `(250, 50, 50)` while still honoring `cl_crosshair_translucent` and `alpha` (`120..=255`).
pub fn crosshair_color(cvars: &CVarRegistry, night_vision: bool, alpha: u8) -> Color32 {
    let (r, g, b) = if night_vision {
        (250, 50, 50)
    } else {
        parse_rgb(cvars.get_str("cl_crosshair_color"))
    };
    if cvars.get_bool("cl_crosshair_translucent") {
        crate::ui::hud::hud_additive_color(Color32::from_rgb(r, g, b), alpha)
    } else {
        Color32::from_rgba_unmultiplied(r, g, b, alpha)
    }
}

/// Returns `true` when `CHudAmmo::Draw` (`client.dll` `FUN_10043a60`) enables the dynamic
/// 4-bar crosshair (`m_iFOV > 54` and weapon is not Scout `3`, SG550 `13`, AWP `18`, or G3SG1 `24`).
pub fn should_draw_dynamic_crosshair(fov: f32, weapon_id: u8) -> bool {
    fov.is_finite() && fov > 54.0 && !matches!(weapon_id, 3 | 13 | 18 | 24)
}

/// Draw the 1-to-1 `FUN_10043ff0` (`CHudAmmo::DrawCrosshair`) 4-bar dynamic crosshair.
pub fn draw_crosshair_vgui(
    ctx: &egui::Context,
    cvars: &CVarRegistry,
    state: &mut CrosshairState,
    inputs: &CrosshairInputs,
) {
    if !cvars.get_bool("crosshair") {
        return;
    }
    let screen = ctx.content_rect();
    let sw = screen.width().round() as i32;
    let sh = screen.height().round() as i32;
    let geom = state.compute_geometry(inputs, cvars.get_str("cl_crosshair_size"), sw);
    let color = crosshair_color(cvars, inputs.night_vision, geom.alpha);

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::unique("cs16_crosshair"),
    ));
    let cx = (screen.left() + (sw / 2) as f32).floor();
    let cy = (screen.top() + (sh / 2) as f32).floor();
    let g = geom.gap as f32;
    let l = geom.bar_len as f32;

    // 4 1px-thick filled rectangles matching `FUN_10043ff0`:
    // Left:   (cx - gap - bar_len + 1, cy, bar_len, 1)
    // Right:  (cx + gap,               cy, bar_len, 1)
    // Top:    (cx, cy - gap - bar_len + 1, 1, bar_len)
    // Bottom: (cx, cy + gap,               1, bar_len)
    painter.rect_filled(egui::Rect::from_min_size(Pos2::new(cx - g - l + 1.0, cy), egui::vec2(l, 1.0)), 0.0, color);
    painter.rect_filled(egui::Rect::from_min_size(Pos2::new(cx + g, cy), egui::vec2(l, 1.0)), 0.0, color);
    painter.rect_filled(egui::Rect::from_min_size(Pos2::new(cx, cy - g - l + 1.0), egui::vec2(1.0, l)), 0.0, color);
    painter.rect_filled(egui::Rect::from_min_size(Pos2::new(cx, cy + g), egui::vec2(1.0, l)), 0.0, color);
}

/// Draw the classic 4-bar Counter-Strike 1.6 crosshair in the center of the screen.
pub fn draw_crosshair(ctx: &egui::Context, cvars: &CVarRegistry, dynamic_extra_gap: f32) {
    let mut state = CrosshairState {
        crosshair_dist: 4.0 + dynamic_extra_gap.clamp(0.0, 22.0),
        alpha: 255,
        last_shots_fired: 0,
    };
    let inputs = CrosshairInputs {
        weapon_id: 16,
        dynamic: dynamic_extra_gap > 0.0,
        speed_2d: dynamic_extra_gap * 45.0,
        on_ground: true,
        ..CrosshairInputs::default()
    };
    draw_crosshair_vgui(ctx, cvars, &mut state, &inputs);
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

/// Draw classic top-left `cl_showfps 1` framerate readout and `con_notifytime` recent lines.
pub fn draw_top_left_console_overlays(
    ctx: &egui::Context,
    cvars: &CVarRegistry,
    fps: f32,
    notify_lines: &[(String, f64)],
    now_sec: f64,
    console_open: bool,
) {
    let show_fps = cvars.get_i32("cl_showfps") != 0;
    let notify_time = f64::from(cvars.get_f32("con_notifytime").max(0.0));
    let show_notify = !console_open && notify_time > 0.0;
    if !show_fps && !show_notify {
        return;
    }
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_top_left_overlays"),
    ));
    let font = FontId::monospace(13.0);
    let mut y = screen.top() + 8.0;
    if show_fps {
        let fps_val = if fps.is_finite() { fps.round().max(0.0) } else { 0.0 };
        let col = if fps_val >= 60.0 {
            Color32::from_rgb(80, 255, 80)
        } else if fps_val >= 30.0 {
            Color32::from_rgb(255, 220, 60)
        } else {
            Color32::from_rgb(255, 80, 80)
        };
        let label = format!("{fps_val:.0} fps");
        let pos = Pos2::new(screen.left() + 8.0, y);
        painter.text(pos + egui::vec2(1.0, 1.0), egui::Align2::LEFT_TOP, &label, font.clone(), Color32::BLACK);
        painter.text(pos, egui::Align2::LEFT_TOP, &label, font.clone(), col);
        y += 18.0;
    }
    if show_notify {
        let con_col = crate::ui::hud::HudExtrasContext::default().con_color;
        let _ = con_col;
        for (line, _ts) in notify_lines
            .iter()
            .filter(|(_, ts)| now_sec - *ts <= notify_time)
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            let pos = Pos2::new(screen.left() + 8.0, y);
            painter.text(pos + egui::vec2(1.0, 1.0), egui::Align2::LEFT_TOP, line, font.clone(), Color32::BLACK);
            painter.text(pos, egui::Align2::LEFT_TOP, line, font.clone(), Color32::from_rgb(255, 195, 65));
            y += 15.0;
        }
    }
}

pub fn parse_rgb(s: &str) -> (u8, u8, u8) {
    let mut parts = s.split_whitespace().filter_map(|p| p.parse::<u8>().ok());
    let r = parts.next().unwrap_or(50);
    let g = parts.next().unwrap_or(250);
    let b = parts.next().unwrap_or(50);
    (r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvg_crosshair_honors_cl_crosshair_translucent_and_alpha() {
        let mut cvars = CVarRegistry::new_with_defaults();
        cvars.set("cl_crosshair_translucent", "0");
        assert_eq!(
            crosshair_color(&cvars, true, 160),
            Color32::from_rgba_unmultiplied(250, 50, 50, 160)
        );
        cvars.set("cl_crosshair_translucent", "1");
        assert_eq!(
            crosshair_color(&cvars, true, 120),
            crate::ui::hud::hud_additive_color(Color32::from_rgb(250, 50, 50), 120)
        );
    }

    #[test]
    fn sniper_weapons_and_zoomed_fov_suppress_dynamic_crosshair() {
        assert!(!should_draw_dynamic_crosshair(90.0, 3));  // Scout
        assert!(!should_draw_dynamic_crosshair(90.0, 13)); // SG550
        assert!(!should_draw_dynamic_crosshair(90.0, 18)); // AWP
        assert!(!should_draw_dynamic_crosshair(90.0, 24)); // G3SG1
        assert!(!should_draw_dynamic_crosshair(54.0, 28)); // FOV <= 54
        assert!(should_draw_dynamic_crosshair(55.0, 8));   // AUG zoomed (55 deg)
        assert!(should_draw_dynamic_crosshair(90.0, 28));  // AK47 unzoomed
    }
}
