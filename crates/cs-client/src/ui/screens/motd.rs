//! `.res`-driven CS 1.6 MOTD HTML viewer (`Resource/UI/MOTD.res`).

use cs_assets::scheme::LocalizationTable;
use cs_vfs::Vfs;
use egui::{CornerRadius, Pos2, RichText, ScrollArea, Stroke, TextureHandle, Vec2};

use crate::ui::vgui2::{HtmlDoc, VguiRuntime, parse_html_doc, paint_html_doc};
use crate::ui::vgui_assets::show_steam2003_window;
use crate::ui::vgui_theme::VguiPalette;

#[derive(Default)]
pub struct MotdVguiUi {
    pub open: bool,
    last_raw: String,
    parsed: HtmlDoc,
}

impl MotdVguiUi {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        vfs: Option<&Vfs>,
        loc: Option<&LocalizationTable>,
        hostname: &str,
        raw_motd: &str,
        palette: VguiPalette,
        steam_icon: Option<&TextureHandle>,
    ) -> bool {
        if !self.open {
            return false;
        }
        if let Some(v) = vfs {
            runtime.load_res(v, "resource/UI/MOTD.res");
        }
        if raw_motd != self.last_raw {
            self.last_raw = raw_motd.to_string();
            self.parsed = parse_html_doc(raw_motd);
        }

        let win_sz = runtime.doc_window_size("resource/UI/MOTD.res", [560.0, 448.0]);
        let ok_label = runtime
            .doc_label("resource/UI/MOTD.res", "ok", loc)
            .or_else(|| {
                loc.map(|l| l.resolve("#Cstrike_OK").to_string())
                    .filter(|s| !s.starts_with('#'))
            })
            .unwrap_or_else(|| "OK".to_string());
        let mut still_open = self.open;
        let mut clicked_ok = false;

        show_steam2003_window(
            ctx,
            "cs16_vgui_motd",
            if hostname.is_empty() { "Message of the Day" } else { hostname },
            &mut still_open,
            Pos2::new(110.0, 50.0),
            Vec2::new(win_sz[0].max(420.0), win_sz[1].max(320.0)),
            Vec2::new(420.0, 320.0),
            true,
            palette,
            steam_icon,
            |ui| {
                let area = ui.available_rect_before_wrap();
                let nodes = runtime.compute_layout(
                    "resource/UI/MOTD.res",
                    [area.left(), area.top(), area.width(), area.height()],
                    loc,
                );
                let res_title = nodes
                    .iter()
                    .find(|n| n.field_name.eq_ignore_ascii_case("ServerName") && !n.label.is_empty())
                    .map(|n| n.label.clone());
                ui.label(
                    RichText::new(if !hostname.is_empty() {
                        hostname
                    } else {
                        res_title.as_deref().unwrap_or("Counter-Strike 1.6 Server")
                    })
                    .size(17.0)
                    .strong()
                    .color(palette.bright_control_text),
                );
                ui.add_space(6.0);
                let fill = self
                    .parsed
                    .bg_color
                    .map_or(palette.window_bg, |[r, g, b, a]| {
                        egui::Color32::from_rgba_unmultiplied(r, g, b, a)
                    });
                let msg_h = runtime
                    .doc_control_rect(
                        "resource/UI/MOTD.res",
                        "message",
                        [area.left(), area.top(), area.width(), area.height()],
                    )
                    .map(|r| r[3].clamp(160.0, 420.0))
                    .unwrap_or_else(|| (ui.available_height() - 42.0).max(160.0));
                let body_h = msg_h.min((ui.available_height() - 42.0).max(160.0));
                egui::Frame::NONE
                    .fill(fill)
                    .inner_margin(egui::Margin::same(10))
                    .stroke(Stroke::new(1.0, palette.border_dark))
                    .show(ui, |ui| {
                        ui.set_min_size(Vec2::new(ui.available_width(), body_h));
                        ScrollArea::vertical()
                            .id_salt("cs16_motd_html_scroll")
                            .max_height(body_h)
                            .show(ui, |ui| {
                                paint_html_doc(ui, &self.parsed);
                            });
                    });
                ui.add_space(6.0);
                if ui
                    .add(
                        egui::Button::new(RichText::new(&ok_label).color(palette.bright_control_text))
                            .min_size(Vec2::new(128.0, 24.0))
                            .corner_radius(CornerRadius::ZERO),
                    )
                    .clicked()
                {
                    clicked_ok = true;
                }
            },
        );

        if !still_open || clicked_ok {
            self.open = false;
            return true;
        }
        false
    }
}
