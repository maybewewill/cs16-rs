//! Immediate-mode renderer for sandboxed WASM mod HUD draw commands (`HudDrawCmd`) and
//! the opt-in Risk-R6 auto-download approval modal (`PendingModApproval`).
//!
//! Styled strictly in authentic Counter-Strike 1.6 `ClientScheme.res` (in-game HUD overlays:
//! sharp `0px` corners, translucent dark fills, `1px` `(188, 112, 0, 128)` amber borders) and
//! `TrackerScheme.res` (`#4c5844`/`#3e4637` olive + `1px` `#889180`/`#282e22` bevels for dialogs).

use cs_mod_api::HudDrawCmd;
use cs_mod_host::PendingModApproval;
use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Stroke, StrokeKind};

use crate::ui::vgui_theme::{VguiPalette, paint_vgui_bevel};

const HUD_CON_AMBER: Color32 = Color32::from_rgb(255, 180, 30);

/// Draws normalized `0..=1` HUD overlay primitives emitted by client WASM mods
/// with sharp `0px` corners and authentic `ClientScheme.res` borders.
pub fn draw_mod_hud(ctx: &egui::Context, cmds: &[HudDrawCmd]) {
    if cmds.is_empty() {
        return;
    }
    let rect = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::unique("cs16_mod_hud"),
    ));

    let norm_rect = |x: f32, y: f32, w: f32, h: f32| {
        let min = egui::pos2(
            (rect.left() + x * rect.width()).floor(),
            (rect.top() + y * rect.height()).floor(),
        );
        let size = egui::vec2((w * rect.width()).max(0.0).round(), (h * rect.height()).max(0.0).round());
        Rect::from_min_size(min, size)
    };
    let color = |rgba: [u8; 4]| Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]);
    let border = Color32::from_rgba_unmultiplied(188, 112, 0, 128);
    let shadow = Color32::from_black_alpha(210);

    for cmd in cmds {
        match cmd {
            HudDrawCmd::Rect { x, y, w, h, rgba } => {
                painter.rect_filled(norm_rect(*x, *y, *w, *h), CornerRadius::ZERO, color(*rgba));
            }
            HudDrawCmd::Text {
                x,
                y,
                size,
                rgba,
                text,
            } => {
                let pos = Pos2::new(
                    (rect.left() + x * rect.width()).floor(),
                    (rect.top() + y * rect.height()).floor(),
                );
                let font_size = (size * (rect.height() / 768.0).clamp(0.75, 1.5)).max(10.0);
                let font = egui::FontId::proportional(font_size);
                painter.text(pos + egui::vec2(1.0, 1.0), egui::Align2::LEFT_TOP, text, font.clone(), shadow);
                painter.text(pos, egui::Align2::LEFT_TOP, text, font, color(*rgba));
            }
            HudDrawCmd::ProgressBar {
                x,
                y,
                w,
                h,
                fraction,
                rgba,
            } => {
                let outer = norm_rect(*x, *y, *w, *h);
                painter.rect(
                    outer,
                    CornerRadius::ZERO,
                    Color32::from_rgba_unmultiplied(0, 0, 0, 165),
                    Stroke::new(1.0, border),
                    StrokeKind::Inside,
                );
                let frac = fraction.clamp(0.0, 1.0);
                let inner_w = (outer.width() - 2.0).max(0.0) * frac;
                let inner_h = (outer.height() - 2.0).max(0.0);
                if inner_w > 0.0 && inner_h > 0.0 {
                    let fill = Rect::from_min_size(
                        outer.min + egui::vec2(1.0, 1.0),
                        egui::vec2(inner_w, inner_h),
                    );
                    painter.rect_filled(fill, CornerRadius::ZERO, color(*rgba));
                }
            }
            HudDrawCmd::Panel {
                x,
                y,
                w,
                h,
                rgba,
                title,
            } => {
                let outer = norm_rect(*x, *y, *w, *h);
                painter.rect(
                    outer,
                    CornerRadius::ZERO,
                    color(*rgba),
                    Stroke::new(1.0, border),
                    StrokeKind::Inside,
                );
                if !title.is_empty() {
                    let pos = outer.min + egui::vec2(7.0, 5.0);
                    let font = egui::FontId::proportional(13.0);
                    painter.text(pos + egui::vec2(1.0, 1.0), egui::Align2::LEFT_TOP, title, font.clone(), shadow);
                    painter.text(pos, egui::Align2::LEFT_TOP, title, font, HUD_CON_AMBER);
                }
            }
        }
    }
}

/// Renders an authentic Steam 2003 `TrackerScheme.res` approval dialog for pending server mod
/// downloads; returns `(mod_id, approved)` choices.
pub fn draw_mod_approval_prompt(
    ctx: &egui::Context,
    pending: &[PendingModApproval],
) -> Vec<(String, bool)> {
    let mut decisions = Vec::new();
    let Some(first) = pending.first() else {
        return decisions;
    };

    let palette = VguiPalette::from(cs_assets::scheme::VguiColors::default());
    let resp = egui::Window::new("Server Mod Download")
        .id(egui::Id::unique("cs16_mod_approval_dialog"))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::NONE
                .fill(palette.control_bg)
                .corner_radius(CornerRadius::ZERO)
                .inner_margin(egui::Margin::same(10))
                .stroke(Stroke::new(1.0, palette.border_bright)),
        )
        .show(ctx, |ui| {
            ui.set_min_width(340.0);
            ui.label(
                RichText::new("Server Mod Download")
                    .color(palette.bright_control_text)
                    .strong()
                    .size(14.0),
            );
            ui.add_space(4.0);
            egui::Frame::NONE
                .fill(palette.window_bg)
                .corner_radius(CornerRadius::ZERO)
                .inner_margin(egui::Margin::same(8))
                .stroke(Stroke::new(1.0, palette.border_dark))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!(
                            "Mod \"{}\" v{} ({} bytes)",
                            first.entry.id, first.entry.version, first.entry.byte_len
                        ))
                        .color(palette.bright_text),
                    );
                    ui.label(
                        RichText::new(format!(
                            "SHA-256: {}...",
                            &first.entry.sha256_hex[..first.entry.sha256_hex.len().min(16)]
                        ))
                        .monospace()
                        .color(palette.base_text),
                    );
                    let perms = if first.permission_names.is_empty() {
                        "none".to_string()
                    } else {
                        first.permission_names.join(", ")
                    };
                    ui.label(
                        RichText::new(format!("Permissions: {perms}")).color(palette.base_text),
                    );
                    if pending.len() > 1 {
                        ui.label(
                            RichText::new(format!("(+{} more queued)", pending.len() - 1))
                                .color(palette.dim_text),
                        );
                    }
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let allow_btn = egui::Button::new(
                    RichText::new("Allow").color(palette.bright_control_text),
                )
                .fill(palette.control_bg)
                .corner_radius(CornerRadius::ZERO)
                .stroke(Stroke::new(1.0, palette.border_bright));
                if ui.add(allow_btn).clicked() {
                    decisions.push((first.entry.id.clone(), true));
                }
                let deny_btn = egui::Button::new(RichText::new("Deny").color(palette.base_text))
                    .fill(palette.control_bg)
                    .corner_radius(CornerRadius::ZERO)
                    .stroke(Stroke::new(1.0, palette.border_bright));
                if ui.add(deny_btn).clicked() {
                    decisions.push((first.entry.id.clone(), false));
                }
            });
        });

    if let Some(inner) = resp {
        paint_vgui_bevel(
            &ctx.layer_painter(inner.response.layer_id),
            inner.response.rect,
            true,
            palette,
        );
    }
    decisions
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_mod_api::ClientModEntry;

    #[test]
    fn mod_hud_draws_commands_without_panicking() {
        let ctx = egui::Context::default();
        let cmds = vec![
            HudDrawCmd::Panel {
                x: 0.02,
                y: 0.02,
                w: 0.25,
                h: 0.1,
                rgba: [10, 20, 30, 180],
                title: "Mod Status".into(),
            },
            HudDrawCmd::Text {
                x: 0.03,
                y: 0.05,
                size: 14.0,
                rgba: [255, 255, 255, 255],
                text: "HP 100".into(),
            },
            HudDrawCmd::ProgressBar {
                x: 0.03,
                y: 0.08,
                w: 0.2,
                h: 0.02,
                fraction: 0.75,
                rgba: [0, 200, 100, 255],
            },
        ];
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            draw_mod_hud(ui.ctx(), &cmds);
        });
        out.textures_delta.clear();
    }

    #[test]
    fn mod_approval_prompt_renders_pending_entry() {
        let ctx = egui::Context::default();
        let pending = vec![PendingModApproval {
            mod_index: 0,
            entry: ClientModEntry {
                id: "custom_hud".into(),
                version: "1.0.0".into(),
                sha256_hex: "0123456789abcdef0123456789abcdef".into(),
                byte_len: 1024,
                permissions_mask: 0,
            },
            permission_names: vec!["hud_draw"],
        }];
        let mut decisions = Vec::new();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            decisions = draw_mod_approval_prompt(ui.ctx(), &pending);
        });
        out.textures_delta.clear();
        assert!(decisions.is_empty());
    }
}
