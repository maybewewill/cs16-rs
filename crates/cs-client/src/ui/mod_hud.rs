//! Immediate-mode renderer for sandboxed WASM mod HUD draw commands (`HudDrawCmd`) and
//! the opt-in Risk-R6 auto-download approval modal (`PendingModApproval`).

use cs_mod_api::HudDrawCmd;
use cs_mod_host::PendingModApproval;

/// Draws normalized `0..=1` HUD overlay primitives emitted by client WASM mods.
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
        let min = egui::pos2(rect.left() + x * rect.width(), rect.top() + y * rect.height());
        let size = egui::vec2((w * rect.width()).max(0.0), (h * rect.height()).max(0.0));
        egui::Rect::from_min_size(min, size)
    };
    let color = |rgba: [u8; 4]| egui::Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]);

    for cmd in cmds {
        match cmd {
            HudDrawCmd::Rect { x, y, w, h, rgba } => {
                painter.rect_filled(norm_rect(*x, *y, *w, *h), 2.0, color(*rgba));
            }
            HudDrawCmd::Text {
                x,
                y,
                size,
                rgba,
                text,
            } => {
                let pos = egui::pos2(rect.left() + x * rect.width(), rect.top() + y * rect.height());
                let font_size = (size * (rect.height() / 768.0).clamp(0.75, 1.5)).max(10.0);
                painter.text(
                    pos,
                    egui::Align2::LEFT_TOP,
                    text,
                    egui::FontId::proportional(font_size),
                    color(*rgba),
                );
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
                painter.rect_filled(outer, 2.0, egui::Color32::from_black_alpha(150));
                let frac = fraction.clamp(0.0, 1.0);
                let fill = egui::Rect::from_min_size(
                    outer.min,
                    egui::vec2(outer.width() * frac, outer.height()),
                );
                painter.rect_filled(fill, 2.0, color(*rgba));
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
                painter.rect_filled(outer, 4.0, color(*rgba));
                if !title.is_empty() {
                    painter.text(
                        outer.min + egui::vec2(8.0, 6.0),
                        egui::Align2::LEFT_TOP,
                        title,
                        egui::FontId::proportional(14.0),
                        egui::Color32::WHITE,
                    );
                }
            }
        }
    }
}

/// Renders an approval modal for pending server mod downloads; returns `(mod_id, approved)` choices.
pub fn draw_mod_approval_prompt(
    ctx: &egui::Context,
    pending: &[PendingModApproval],
) -> Vec<(String, bool)> {
    let mut decisions = Vec::new();
    let Some(first) = pending.first() else {
        return decisions;
    };

    egui::Window::new("Server Mod Download")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!(
                "Mod \"{}\" v{} ({} bytes)",
                first.entry.id, first.entry.version, first.entry.byte_len
            ));
            ui.label(format!(
                "SHA-256: {}...",
                &first.entry.sha256_hex[..first.entry.sha256_hex.len().min(16)]
            ));
            let perms = if first.permission_names.is_empty() {
                "none".to_string()
            } else {
                first.permission_names.join(", ")
            };
            ui.label(format!("Permissions: {perms}"));
            if pending.len() > 1 {
                ui.label(format!("(+{} more queued)", pending.len() - 1));
            }
            ui.horizontal(|ui| {
                if ui.button("Allow").clicked() {
                    decisions.push((first.entry.id.clone(), true));
                }
                if ui.button("Deny").clicked() {
                    decisions.push((first.entry.id.clone(), false));
                }
            });
        });
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
