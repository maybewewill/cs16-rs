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

use cs_clientcore::{NET_TIMINGS, NetGraphTelemetry, PacketSampleStatus};
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

    /// Compute gap, bar length, and alpha matching `client.dll` `FUN_10043ff0` at GoldSrc 100 Hz (`dt = 0.01`).
    pub fn compute_geometry(
        &mut self,
        inputs: &CrosshairInputs,
        size_cvar: &str,
        screen_w: i32,
    ) -> CrosshairGeometry {
        self.compute_geometry_dt(inputs, size_cvar, screen_w, 0.01)
    }

    /// Compute gap, bar length, and alpha matching `client.dll` `FUN_10043ff0`, with frame-rate-
    /// independent exponential recovery anchored to GoldSrc 100 Hz (`dt = 0.01`).
    pub fn compute_geometry_dt(
        &mut self,
        inputs: &CrosshairInputs,
        size_cvar: &str,
        screen_w: i32,
        dt: f32,
    ) -> CrosshairGeometry {
        const ACC_CODE: [u8; 30] = [
            0, 8, 8, 1, 1, 1, 2, 3, 8, 8, 0, 2, 1, 3, 4, 5, 6, 8, 2, 3, 1, 7, 2, 8, 1, 0, 3, 3, 1,
            3,
        ];
        const SPD_CODE: [u8; 30] = [
            0, 3, 3, 3, 3, 3, 3, 1, 3, 3, 0, 3, 3, 1, 1, 0, 0, 3, 3, 1, 3, 1, 3, 3, 3, 0, 1, 1, 3,
            2,
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
            let safe_dt = if dt.is_finite() {
                dt.clamp(0.0, 0.1)
            } else {
                0.01
            };
            let steps = safe_dt * 100.0;
            if steps > 0.0 {
                let alpha_step = (2.0 * steps).round().max(1.0) as i32;
                self.alpha = (self.alpha + alpha_step).min(255);
                // Exact continuous form of `d_{n+1} = 0.987 * d_n - 0.1` per 10ms tick.
                let decay = 0.987_f32.powf(steps);
                self.crosshair_dist = self.crosshair_dist * decay - 0.1 * ((1.0 - decay) / 0.013);
            }
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

/// Draw the 1-to-1 `FUN_10043ff0` (`CHudAmmo::DrawCrosshair`) 4-bar dynamic crosshair,
/// plus optional Task 9 QoL extensions (`cl_crosshair_dot`, `cl_crosshair_outline`,
/// `cl_crosshair_t`, `cl_crosshair_gap`, all `"0"` by default).
pub fn draw_crosshair_vgui(
    ctx: &egui::Context,
    cvars: &CVarRegistry,
    state: &mut CrosshairState,
    inputs: &CrosshairInputs,
) {
    draw_crosshair_vgui_dt(ctx, cvars, state, inputs, 0.01);
}

/// Frame-rate-independent variant of [`draw_crosshair_vgui`] accepting live frame `dt` seconds.
pub fn draw_crosshair_vgui_dt(
    ctx: &egui::Context,
    cvars: &CVarRegistry,
    state: &mut CrosshairState,
    inputs: &CrosshairInputs,
    dt: f32,
) {
    if !cvars.get_bool("crosshair") {
        return;
    }
    let screen = ctx.content_rect();
    let sw = screen.width().round() as i32;
    let sh = screen.height().round() as i32;
    let geom = state.compute_geometry_dt(inputs, cvars.get_str("cl_crosshair_size"), sw, dt);
    let color = crosshair_color(cvars, inputs.night_vision, geom.alpha);
    let dot = cvars.get_bool("cl_crosshair_dot");
    let outline = cvars.get_bool("cl_crosshair_outline");
    let t_shape = cvars.get_bool("cl_crosshair_t");
    let extra_gap = {
        let v = cvars.get_f32("cl_crosshair_gap");
        if v.is_finite() { v } else { 0.0 }
    };

    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::unique("cs16_crosshair"),
    ));
    let cx = (screen.left() + (sw / 2) as f32).floor();
    let cy = (screen.top() + (sh / 2) as f32).floor();
    let g = ((geom.gap as f32) + extra_gap).max(0.0);
    let l = geom.bar_len as f32;
    let outline_color = Color32::from_rgba_unmultiplied(0, 0, 0, geom.alpha);

    let mut rects = vec![
        egui::Rect::from_min_size(Pos2::new(cx - g - l + 1.0, cy), egui::vec2(l, 1.0)),
        egui::Rect::from_min_size(Pos2::new(cx + g, cy), egui::vec2(l, 1.0)),
        egui::Rect::from_min_size(Pos2::new(cx, cy + g), egui::vec2(1.0, l)),
    ];
    if !t_shape {
        rects.push(egui::Rect::from_min_size(
            Pos2::new(cx, cy - g - l + 1.0),
            egui::vec2(1.0, l),
        ));
    }
    if dot {
        rects.push(egui::Rect::from_min_size(
            Pos2::new(cx, cy),
            egui::vec2(1.0, 1.0),
        ));
    }
    if outline {
        for r in &rects {
            painter.rect_filled(r.expand(1.0), 0.0, outline_color);
        }
    }
    for r in rects {
        painter.rect_filled(r, 0.0, color);
    }
}

/// Live movement/jump/duck key states for `cl_showkeys 1`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyDisplayState {
    pub forward: bool,
    pub back: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub duck: bool,
}

/// Draw `cl_showspeed 1` horizontal XY speed (`u/s`) centered above the bottom HUD bar
/// in authentic CS 1.6 `ClientScheme` HUD amber `(255, 160, 0)` with crisp 0px styling.
pub fn draw_showspeed(ctx: &egui::Context, cvars: &CVarRegistry, speed_xy: f32) {
    if cvars.get_f32("cl_showspeed") == 0.0 {
        return;
    }
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_cl_showspeed"),
    ));
    let font = FontId::monospace(14.0);
    let label = format!("{:.0} u/s", speed_xy.max(0.0));
    let pos = Pos2::new(screen.center().x.floor(), (screen.bottom() - 82.0).floor());
    painter.text(
        pos + egui::vec2(1.0, 1.0),
        egui::Align2::CENTER_BOTTOM,
        &label,
        font.clone(),
        Color32::from_black_alpha(230),
    );
    painter.text(
        pos,
        egui::Align2::CENTER_BOTTOM,
        &label,
        font,
        Color32::from_rgb(255, 160, 0),
    );
}

/// Draw `cl_showkeys 1` live `W A S D  JUMP  DUCK` indicator above the bottom HUD bar
/// using authentic `ClientScheme.res` sharp 0px dark boxes and `1px` amber borders.
pub fn draw_showkeys(ctx: &egui::Context, cvars: &CVarRegistry, keys: &KeyDisplayState) {
    if cvars.get_f32("cl_showkeys") == 0.0 {
        return;
    }
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_cl_showkeys"),
    ));
    let font = FontId::monospace(11.5);
    let items: [(&str, bool, f32); 6] = [
        ("W", keys.forward, 22.0),
        ("A", keys.left, 22.0),
        ("S", keys.back, 22.0),
        ("D", keys.right, 22.0),
        ("JUMP", keys.jump, 42.0),
        ("DUCK", keys.duck, 42.0),
    ];
    let gap = 3.0;
    let total_w: f32 =
        items.iter().map(|(_, _, w)| *w).sum::<f32>() + gap * (items.len() as f32 - 1.0);
    let box_h = 18.0;
    let mut x = (screen.center().x - total_w * 0.5).floor();
    let top_y = (screen.bottom() - 102.0 - box_h).floor();
    let bg_idle = Color32::from_rgba_unmultiplied(0, 0, 0, 153);
    let bg_active = Color32::from_rgba_unmultiplied(0, 0, 0, 195);
    let border_idle = Color32::from_rgba_unmultiplied(188, 112, 0, 128);
    let border_active = Color32::from_rgb(255, 160, 0);
    let text_idle = Color32::from_rgb(176, 176, 176);
    let text_active = Color32::from_rgb(255, 180, 30);

    for (lbl, on, w) in items {
        let r = egui::Rect::from_min_size(Pos2::new(x, top_y), egui::vec2(w, box_h));
        painter.rect(
            r,
            egui::CornerRadius::ZERO,
            if on { bg_active } else { bg_idle },
            egui::Stroke::new(1.0, if on { border_active } else { border_idle }),
            egui::StrokeKind::Inside,
        );
        let center = Pos2::new(r.center().x.floor(), r.center().y.floor());
        painter.text(
            center + egui::vec2(1.0, 1.0),
            egui::Align2::CENTER_CENTER,
            lbl,
            font.clone(),
            Color32::from_black_alpha(220),
        );
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            lbl,
            font.clone(),
            if on { text_active } else { text_idle },
        );
        x += w + gap;
    }
}

/// Draw `cl_ljstats 1` latest completed Kreedz jump summary above the speed/keys HUD
/// in an authentic `ClientScheme.res` translucent dark box with `1px` amber border and `con_color` text.
pub fn draw_ljstats(
    ctx: &egui::Context,
    cvars: &CVarRegistry,
    stats: Option<&cs_pm::ljstats::JumpStats>,
) {
    if cvars.get_f32("cl_ljstats") == 0.0 {
        return;
    }
    let Some(stats) = stats else {
        return;
    };
    let min_dist = cvars.get_f32("cl_ljstats_min");
    let min_threshold = if min_dist > 0.0 { min_dist } else { 215.0 };
    if stats.distance < min_threshold {
        return;
    }
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_cl_ljstats"),
    ));
    let font = FontId::monospace(12.5);
    let label = stats.format_summary();
    let galley =
        painter.layout_no_wrap(label.clone(), font.clone(), Color32::from_rgb(255, 180, 30));
    let pad_x = 8.0;
    let pad_y = 3.0;
    let box_w = (galley.size().x + pad_x * 2.0).ceil();
    let box_h = (galley.size().y + pad_y * 2.0).ceil();
    let rect = egui::Rect::from_min_size(
        Pos2::new(
            (screen.center().x - box_w * 0.5).floor(),
            (screen.bottom() - 126.0 - box_h).floor(),
        ),
        egui::vec2(box_w, box_h),
    );
    painter.rect(
        rect,
        egui::CornerRadius::ZERO,
        Color32::from_rgba_unmultiplied(0, 0, 0, 165),
        egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 112, 0, 145)),
        egui::StrokeKind::Inside,
    );
    let pos = Pos2::new(rect.center().x.floor(), rect.center().y.floor());
    painter.text(
        pos + egui::vec2(1.0, 1.0),
        egui::Align2::CENTER_CENTER,
        &label,
        font.clone(),
        Color32::from_black_alpha(230),
    );
    painter.text(
        pos,
        egui::Align2::CENTER_CENTER,
        &label,
        font,
        Color32::from_rgb(255, 180, 30),
    );
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

/// Draw `cl_showpos 1` local player position, view angles, and velocity (`SCR_DrawPos`,
/// Xash3D FWGS `engine/client/cl_scrn.c`, read 2026-11-25).
pub fn draw_showpos(ctx: &egui::Context, cvars: &CVarRegistry, t: &FrameTelemetry<'_>) {
    if cvars.get_f32("cl_showpos") == 0.0 {
        return;
    }
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_cl_showpos"),
    ));
    let font = FontId::monospace(12.0);
    let white = Color32::WHITE;
    let shadow = Color32::from_black_alpha(220);
    let lines = [
        format!("pos: {:.2} {:.2} {:.2}", t.pos[0], t.pos[1], t.pos[2]),
        format!("ang: {:.2} {:.2} 0.00", t.pitch_deg, t.yaw_deg),
        format!("vel: {:.2}", t.speed),
    ];
    let x = screen.right() - 12.0;
    for (idx, line) in lines.iter().enumerate() {
        let pos = Pos2::new(x, screen.top() + 8.0 + (idx as f32) * 15.0);
        painter.text(
            pos + egui::vec2(1.0, 1.0),
            egui::Align2::RIGHT_TOP,
            line,
            font.clone(),
            shadow,
        );
        painter.text(pos, egui::Align2::RIGHT_TOP, line, font.clone(), white);
    }
}

/// Draw `r_speeds 1` renderer batch/triangle statistics (`SCR_RSpeeds`,
/// Xash3D FWGS `engine/client/cl_scrn.c`, read 2026-11-25).
pub fn draw_r_speeds(ctx: &egui::Context, cvars: &CVarRegistry, t: &FrameTelemetry<'_>) {
    if cvars.get_f32("r_speeds") == 0.0 {
        return;
    }
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_r_speeds"),
    ));
    let font = FontId::monospace(12.0);
    let white = Color32::WHITE;
    let shadow = Color32::from_black_alpha(220);
    let line = format!(
        "map: {}  batches: {}  tris: {}  ({:.2} ms)",
        t.map_name, t.draw_calls, t.triangles, t.frame_ms
    );
    let pos = Pos2::new(screen.right() - 12.0, screen.top() + 64.0);
    painter.text(
        pos + egui::vec2(1.0, 1.0),
        egui::Align2::RIGHT_TOP,
        &line,
        font.clone(),
        shadow,
    );
    painter.text(pos, egui::Align2::RIGHT_TOP, &line, font, white);
}

/// Draw classic CS 1.6 `net_graph 1/2/3` without live NetChan ring data.
pub fn draw_net_graph(
    ctx: &egui::Context,
    cvars: &CVarRegistry,
    t: &FrameTelemetry<'_>,
    palette: VguiPalette,
) {
    draw_net_graph_with_telemetry(ctx, cvars, t, None, palette);
}

/// Draw authentic GoldSrc `net_graph 1/2/3` ported from Xash3D FWGS
/// `engine/client/cl_netgraph.c` (`SCR_DrawNetGraph`, `NetGraph_DrawTimes`,
/// `NetGraph_DrawDataUsage`, `NetGraph_DrawHatches`, `NetGraph_DrawTextFields`, read 2026-11-25).
pub fn draw_net_graph_with_telemetry(
    ctx: &egui::Context,
    cvars: &CVarRegistry,
    t: &FrameTelemetry<'_>,
    net: Option<&NetGraphTelemetry>,
    palette: VguiPalette,
) {
    let _ = palette;
    let mode = cvars.get_i32("net_graph");
    if mode <= 0 {
        return;
    }

    const LERP_HEIGHT: i32 = 24;
    const EXTRAP_POINT: i32 = LERP_HEIGHT / 3; // 8

    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_net_graph"),
    ));

    let screen_w = screen.width().round().max(32.0) as i32;
    let req_w = cvars.get_i32("net_graphwidth").max(16);
    let w = req_w.min(NET_TIMINGS as i32).min((screen_w - 10).max(16));
    let h = cvars.get_i32("net_graphheight").clamp(32, 512);
    let solid = cvars.get_f32("net_graphsolid") != 0.0;
    let net_scale = cvars.get_f32("net_scale").max(0.1);
    let pos_mode = cvars.get_i32("net_graphpos");

    // `NetGraph_GetScreenPos`: 1 = right, 2 = center, 3 = left.
    let x = match pos_mode {
        2 => screen.left() + (screen.width() - w as f32) * 0.5,
        3 => screen.left() + 5.0,
        _ => screen.right() - 5.0 - w as f32,
    };

    let text_color = Color32::from_rgb(230, 230, 179); // `cl_netgraph.c:366` (0.9, 0.9, 0.7)
    let shadow = Color32::from_black_alpha(220);
    let font = FontId::monospace(12.0);
    let line_h = 14.0;
    let text_top = screen.bottom() - 8.0 - 4.0 * line_h;

    let fps_val = if t.fps.is_finite() {
        t.fps.max(0.0)
    } else {
        0.0
    };
    let ping_ms = net.map_or(0.0, NetGraphTelemetry::ping_ms).max(0.0);
    let last_in = net.map_or(0, NetGraphTelemetry::last_in_bytes);
    let in_kbs = net.map_or(0.0, NetGraphTelemetry::in_kbs);
    let in_pps = net.map_or(0.0, NetGraphTelemetry::in_packets_per_sec);
    let last_out = net.map_or(0, NetGraphTelemetry::last_out_bytes);
    let out_kbs = net.map_or(0.0, NetGraphTelemetry::out_kbs);
    let out_pps = net.map_or(0.0, NetGraphTelemetry::out_packets_per_sec);
    let loss = ((net.map_or(0.0, NetGraphTelemetry::loss_percent) + 0.5) - 0.01).max(0.0) as i32;
    let choke = ((net.map_or(0.0, NetGraphTelemetry::choke_percent) + 0.5) - 0.01).max(0.0) as i32;

    let rows = [
        format!("{fps_val:.1} fps  {} ms", ping_ms.round() as i32),
        format!("in :  {last_in}  {in_kbs:.2} k/s  {in_pps:.0}/s"),
        format!("out:  {last_out}  {out_kbs:.2} k/s  {out_pps:.0}/s"),
        format!("loss: {loss}  choke: {choke}"),
    ];
    for (idx, line) in rows.iter().enumerate() {
        let pos = Pos2::new(x, text_top + (idx as f32) * line_h);
        painter.text(
            pos + egui::vec2(1.0, 1.0),
            egui::Align2::LEFT_TOP,
            line,
            font.clone(),
            shadow,
        );
        painter.text(pos, egui::Align2::LEFT_TOP, line, font.clone(), text_color);
    }

    if mode >= 3 {
        return;
    }

    // Graph timelane directly above the 4 text rows (`net_graph 1` and `2`).
    let graph_bottom = text_top - 4.0;
    let ping_bottom = graph_bottom - (LERP_HEIGHT as f32) - 2.0;
    let ping_height = (h - LERP_HEIGHT - 2).max(16);

    let at_edge = |a: i32| a <= 3 || a >= w - 4;

    // 1. `NetGraph_DrawTimes`: pale-blue/orange `ex_interp` strip + blue `cl_cmdrate` tick dots.
    let latest_out = net.map_or(0, NetGraphTelemetry::latest_outgoing_seq);
    for a in 0..w {
        let col_x = x + (w - a - 1) as f32;
        if let Some(ng) = net {
            let seq = latest_out.wrapping_sub(a as u32);
            let sample = ng.outgoing_sample(seq);
            if sample.sequence == seq && seq != 0 {
                let lerp_h = ((sample.cmd_lerp / 3.0) * LERP_HEIGHT as f32)
                    .min((h as f32) * 0.7)
                    .round() as i32;
                let base_y = graph_bottom - 4.0;
                if lerp_h >= EXTRAP_POINT {
                    let span = (lerp_h - EXTRAP_POINT).max(1);
                    let start = if solid { 0 } else { span - 1 };
                    for j in start..span {
                        let frac = (j as f32) / ((LERP_HEIGHT - EXTRAP_POINT).max(1) as f32);
                        let col = Color32::from_rgb(
                            (255.0 - 5.0 * frac).round() as u8,
                            (127.0 * (1.0 - frac)).round() as u8,
                            0,
                        );
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                Pos2::new(col_x, base_y - (EXTRAP_POINT + j) as f32),
                                egui::vec2(1.0, 1.0),
                            ),
                            0.0,
                            col,
                        );
                    }
                } else {
                    let old_h = lerp_h.max(0);
                    let span = (EXTRAP_POINT - old_h).max(1);
                    let len = if solid { span } else { 1 };
                    for j in 0..len {
                        let frac = ((old_h + j) as f32) / (EXTRAP_POINT as f32);
                        let col = Color32::from_rgb(
                            (63.0 * (1.0 - frac)).round() as u8,
                            (96.0 + 90.0 * frac).round() as u8,
                            255,
                        );
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                Pos2::new(col_x, base_y - (old_h + j) as f32),
                                egui::vec2(1.0, 1.0),
                            ),
                            0.0,
                            col,
                        );
                    }
                }

                let dot_col = if sample.sent {
                    Color32::from_rgb(70, 150, 255) // blue `cl_cmdrate` tick dot
                } else {
                    Color32::from_rgba_unmultiplied(255, 0, 0, 200)
                };
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        Pos2::new(col_x, graph_bottom - 3.0),
                        egui::vec2(1.0, 2.0),
                    ),
                    0.0,
                    dot_col,
                );
            }
        }
        if at_edge(a) {
            painter.rect_filled(
                egui::Rect::from_min_size(
                    Pos2::new(col_x, graph_bottom - 4.0 - EXTRAP_POINT as f32),
                    egui::vec2(1.0, 1.0),
                ),
                0.0,
                Color32::WHITE,
            );
        }
    }

    // 2. `NetGraph_DrawDataUsage`: latency/packet height bars, red drops, yellow chokes, blue invalid deltas.
    let latest_in = net.map_or(0, NetGraphTelemetry::latest_incoming_seq);
    let mut last_valid_h = 4.0f32;
    for a in 0..w {
        let col_x = x + (w - a - 1) as f32;
        if let Some(ng) = net {
            let seq = latest_in.wrapping_sub(a as u32);
            let sample = ng.incoming_sample(seq);
            if sample.sequence == seq && seq != 0 {
                let (bar_h, col, is_ping) = match sample.status {
                    PacketSampleStatus::Dropped => (
                        last_valid_h.max(8.0).min(ping_height as f32),
                        Color32::from_rgb(255, 0, 0),
                        false,
                    ),
                    PacketSampleStatus::InvalidDelta => (
                        last_valid_h.max(6.0).min(ping_height as f32),
                        Color32::from_rgb(0, 0, 255),
                        false,
                    ),
                    PacketSampleStatus::Skipped => (
                        last_valid_h.max(6.0).min(ping_height as f32),
                        Color32::from_rgb(240, 127, 63),
                        false,
                    ),
                    PacketSampleStatus::Valid => {
                        let lat = sample.latency_sec.clamp(0.0, 1.0);
                        let h_px = (((lat + 0.1) / 1.1) * (ping_height as f32))
                            .round()
                            .clamp(1.0, ping_height as f32);
                        last_valid_h = h_px;
                        let c = if sample.choked {
                            Color32::from_rgb(255, 255, 0)
                        } else if lat >= 0.25 {
                            Color32::from_rgb(255, 80, 60)
                        } else if lat >= 0.12 {
                            Color32::from_rgb(255, 220, 60)
                        } else {
                            Color32::from_rgba_unmultiplied(63, 255, 63, 180)
                        };
                        (h_px, c, true)
                    }
                    PacketSampleStatus::Empty => (0.0, Color32::TRANSPARENT, false),
                };

                if bar_h > 0.0 {
                    let top_y = ping_bottom - bar_h;
                    if is_ping && !solid {
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                Pos2::new(col_x, top_y),
                                egui::vec2(1.0, 1.0),
                            ),
                            0.0,
                            col,
                        );
                    } else if is_ping {
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                Pos2::new(col_x, top_y),
                                egui::vec2(1.0, bar_h),
                            ),
                            0.0,
                            col,
                        );
                    } else {
                        // Drop / invalid / skipped spike (`cl_netgraph.c:520-533`).
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                Pos2::new(col_x, top_y),
                                egui::vec2(1.0, bar_h.min(2.0)),
                            ),
                            0.0,
                            col,
                        );
                        if bar_h > 3.0 {
                            painter.rect_filled(
                                egui::Rect::from_min_size(
                                    Pos2::new(col_x, ping_bottom - 2.0),
                                    egui::vec2(1.0, 2.0),
                                ),
                                0.0,
                                col,
                            );
                        }
                    }
                }
            }
        }

        if at_edge(a) {
            painter.rect_filled(
                egui::Rect::from_min_size(Pos2::new(col_x, ping_bottom), egui::vec2(1.0, 1.0)),
                0.0,
                Color32::from_rgba_unmultiplied(0, 255, 0, 160),
            );
        }

        // 3. `net_graph 2`: per-packet byte height & scale hatches above latency lane.
        if mode >= 2 {
            let data_base_y = ping_bottom - (ping_height as f32) - 2.0;
            if at_edge(a) {
                painter.rect_filled(
                    egui::Rect::from_min_size(Pos2::new(col_x, data_base_y), egui::vec2(1.0, 1.0)),
                    0.0,
                    Color32::WHITE,
                );
            }
            if let Some(ng) = net {
                let seq = latest_in.wrapping_sub(a as u32);
                let sample = ng.incoming_sample(seq);
                if sample.sequence == seq
                    && sample.status == PacketSampleStatus::Valid
                    && sample.bytes > 0
                {
                    let seg_h = ((sample.bytes as f32) / net_scale).clamp(1.0, 48.0);
                    let top_y = (data_base_y - seg_h).max(1.0);
                    let draw_h = if solid {
                        (data_base_y - top_y).max(1.0)
                    } else {
                        1.0
                    };
                    painter.rect_filled(
                        egui::Rect::from_min_size(Pos2::new(col_x, top_y), egui::vec2(1.0, draw_h)),
                        0.0,
                        Color32::from_rgba_unmultiplied(240, 240, 240, 160),
                    );
                }
            }
        }
    }

    if mode >= 2 {
        // `NetGraph_DrawHatches` (`cl_netgraph.c:333-354`).
        let data_base_y = ping_bottom - (ping_height as f32) - 2.0;
        let ystep = ((10.0 / net_scale) as i32).max(1);
        let max_bytes = net.map_or(200, NetGraphTelemetry::max_msg_bytes) + 50;
        let mut hy = data_base_y;
        while hy > 1.0 && ((data_base_y - hy) * net_scale) < max_bytes as f32 {
            let val = ((data_base_y - hy) * net_scale).round() as i32;
            if val % 50 == 0 {
                painter.rect_filled(
                    egui::Rect::from_min_size(Pos2::new(x, hy), egui::vec2(4.0, 1.0)),
                    0.0,
                    Color32::from_rgb(0, 200, 0),
                );
            } else if ystep > 5 {
                painter.rect_filled(
                    egui::Rect::from_min_size(Pos2::new(x, hy), egui::vec2(4.0, 1.0)),
                    0.0,
                    Color32::from_rgba_unmultiplied(0, 63, 63, 200),
                );
            }
            hy -= ystep as f32;
        }
    }

    // Right-side rate labels matching `NetGraph_DrawTextFields` (`cl_netgraph.c:422-425`).
    let cmdrate = cvars.get_f32("cl_cmdrate").clamp(10.0, 100.0);
    let updaterate = cvars.get_f32("cl_updaterate").clamp(10.0, 102.0);
    let rate_x = x + (w as f32) - 26.0;
    let upd_label = format!("{updaterate:.0}/s");
    let cmd_label = format!("{cmdrate:.0}/s");
    let upd_pos = Pos2::new(rate_x, ping_bottom - (ping_height as f32));
    let cmd_pos = Pos2::new(rate_x, graph_bottom - (LERP_HEIGHT as f32));
    painter.text(
        upd_pos + egui::vec2(1.0, 1.0),
        egui::Align2::LEFT_TOP,
        &upd_label,
        font.clone(),
        shadow,
    );
    painter.text(
        upd_pos,
        egui::Align2::LEFT_TOP,
        &upd_label,
        font.clone(),
        text_color,
    );
    painter.text(
        cmd_pos + egui::vec2(1.0, 1.0),
        egui::Align2::LEFT_TOP,
        &cmd_label,
        font.clone(),
        shadow,
    );
    painter.text(
        cmd_pos,
        egui::Align2::LEFT_TOP,
        &cmd_label,
        font,
        text_color,
    );
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
        let fps_val = if fps.is_finite() {
            fps.round().max(0.0)
        } else {
            0.0
        };
        let col = if fps_val >= 60.0 {
            Color32::from_rgb(80, 255, 80)
        } else if fps_val >= 30.0 {
            Color32::from_rgb(255, 220, 60)
        } else {
            Color32::from_rgb(255, 80, 80)
        };
        let label = format!("{fps_val:.0} fps");
        let pos = Pos2::new(screen.left() + 8.0, y);
        painter.text(
            pos + egui::vec2(1.0, 1.0),
            egui::Align2::LEFT_TOP,
            &label,
            font.clone(),
            Color32::BLACK,
        );
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
            let clean: String = line
                .chars()
                .map(|c| if c == '\t' { ' ' } else { c })
                .filter(|c| !c.is_control())
                .collect();
            let clean = clean.trim();
            if clean.is_empty() {
                continue;
            }
            let pos = Pos2::new(screen.left() + 8.0, y);
            painter.text(
                pos + egui::vec2(1.0, 1.0),
                egui::Align2::LEFT_TOP,
                clean,
                font.clone(),
                Color32::BLACK,
            );
            painter.text(
                pos,
                egui::Align2::LEFT_TOP,
                clean,
                font.clone(),
                Color32::from_rgb(255, 195, 65),
            );
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
        assert!(!should_draw_dynamic_crosshair(90.0, 3)); // Scout
        assert!(!should_draw_dynamic_crosshair(90.0, 13)); // SG550
        assert!(!should_draw_dynamic_crosshair(90.0, 18)); // AWP
        assert!(!should_draw_dynamic_crosshair(90.0, 24)); // G3SG1
        assert!(!should_draw_dynamic_crosshair(54.0, 28)); // FOV <= 54
        assert!(should_draw_dynamic_crosshair(55.0, 8)); // AUG zoomed (55 deg)
        assert!(should_draw_dynamic_crosshair(90.0, 28)); // AK47 unzoomed
    }

    #[test]
    fn net_graph_cl_showpos_and_r_speeds_gate_on_cvars() {
        let mut cvars = CVarRegistry::new_with_defaults();
        let t = FrameTelemetry {
            fps: 100.0,
            frame_ms: 10.0,
            map_name: "de_dust2",
            pos: [10.0, 20.0, 30.0],
            yaw_deg: 90.0,
            pitch_deg: -5.0,
            speed: 250.0,
            draw_calls: 12,
            triangles: 3456,
        };
        let ctx = egui::Context::default();
        let run = |cv: &CVarRegistry| -> usize {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(1024.0, 768.0),
                )),
                ..Default::default()
            };
            ctx.begin_pass(input);
            draw_showpos(&ctx, cv, &t);
            draw_r_speeds(&ctx, cv, &t);
            draw_net_graph(
                &ctx,
                cv,
                &t,
                VguiPalette::from(cs_assets::scheme::VguiColors::default()),
            );
            let mut out = ctx.end_pass();
            out.textures_delta.clear();
            out.shapes.len()
        };
        assert_eq!(run(&cvars), 0);
        cvars.set("cl_showpos", "1");
        assert!(run(&cvars) > 0);
        cvars.set("cl_showpos", "0");
        cvars.set("r_speeds", "1");
        assert!(run(&cvars) > 0);
    }
}
