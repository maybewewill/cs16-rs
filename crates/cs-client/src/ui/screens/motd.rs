//! `.res`-driven CS 1.6 black-and-orange VGUI2 MOTD viewer
//! (`Resource/UI/BackgroundPanel.res` + `Resource/UI/MOTD.res` + `ClientScheme.res`).

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::{ControlKind, TextAlignment, VguiCoordMode};
use cs_vfs::Vfs;
use egui::{Color32, Key, Pos2, Rect, ScrollArea, Stroke, StrokeKind, Vec2};

use crate::ui::vgui2::font_atlas::VguiTextDrawOpts;
use crate::ui::vgui2::runtime::{modal_backdrop, scheme_color_or};
use crate::ui::vgui2::{HtmlDoc, VguiRuntime, VguiSchemeKind, paint_html_doc_vgui, parse_html_doc};

pub struct MotdRenderCtx<'a> {
    pub vfs: Option<&'a Vfs>,
    pub loc: Option<&'a LocalizationTable>,
    pub hostname: &'a str,
    pub raw_motd: &'a str,
}

#[derive(Default)]
pub struct MotdVguiUi {
    pub open: bool,
    last_raw: String,
    parsed: HtmlDoc,
    pub last_clicked_url: Option<String>,
}

impl MotdVguiUi {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        m: &MotdRenderCtx<'_>,
    ) -> bool {
        if !self.open {
            return false;
        }
        if let Some(v) = m.vfs {
            runtime.load_res(v, "resource/UI/MOTD.res");
            runtime.load_res(v, "resource/UI/BackgroundPanel.res");
        }
        if m.raw_motd != self.last_raw {
            self.last_raw = m.raw_motd.to_string();
            self.parsed = parse_html_doc(m.raw_motd);
        }

        modal_backdrop(ctx, "cs16_motd_backdrop");

        let screen = ctx.content_rect();
        let screen_h = screen.height().round().max(480.0) as u32;
        let mut clicked_ok = ctx.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Space));

        egui::Area::new(egui::Id::unique("cs16_vgui_motd_panel"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());
                let vp_rect = runtime.paint_background_panel(
                    ctx,
                    ui.painter(),
                    screen,
                    m.vfs,
                    "",
                );
                let vp = [vp_rect.left(), vp_rect.top(), vp_rect.width(), vp_rect.height()];
                let nodes = runtime.compute_layout_mode(
                    "resource/UI/MOTD.res",
                    vp,
                    VguiCoordMode::Proportional640,
                    m.loc,
                );

                let sx = (screen.width() / 640.0).max(0.5);
                let sy = (screen.height() / 480.0).max(0.5);
                let fallback_title = Rect::from_min_size(
                    Pos2::new(screen.left() + 76.0 * sx, screen.top() + 22.0 * sy),
                    Vec2::new(500.0 * sx, 48.0 * sy),
                );
                let fallback_msg = Rect::from_min_size(
                    Pos2::new(screen.left() + 76.0 * sx, screen.top() + 116.0 * sy),
                    Vec2::new(480.0 * sx, 240.0 * sy),
                );
                let fallback_ok = Rect::from_min_size(
                    Pos2::new(screen.left() + 76.0 * sx, screen.top() + 364.0 * sy),
                    Vec2::new(128.0 * sx, 20.0 * sy),
                );

                let mut title_rect = fallback_title;
                let mut res_title = None;
                let mut msg_rect = fallback_msg;
                let mut ok_rect = fallback_ok;
                let mut ok_label = m
                    .loc
                    .map(|l| l.resolve("#Cstrike_OK").to_string())
                    .filter(|s| !s.starts_with('#'))
                    .unwrap_or_else(|| "OK".to_string());

                for n in &nodes {
                    if !n.visible {
                        continue;
                    }
                    let r = Rect::from_min_size(Pos2::new(n.rect[0], n.rect[1]), Vec2::new(n.rect[2], n.rect[3]));
                    if n.field_name.eq_ignore_ascii_case("serverName") {
                        title_rect = r;
                        if !n.label.is_empty() {
                            res_title = Some(n.label.clone());
                        }
                    } else if n.field_name.eq_ignore_ascii_case("message") || matches!(n.kind, ControlKind::Html) {
                        msg_rect = r;
                    } else if n.field_name.eq_ignore_ascii_case("ok") || matches!(n.kind, ControlKind::Button) {
                        ok_rect = r;
                        if !n.label.is_empty() && !n.label.starts_with('#') {
                            ok_label = n.label.clone();
                        }
                    }
                }

                let title_text = if !m.hostname.is_empty() {
                    m.hostname.to_string()
                } else {
                    res_title.unwrap_or_else(|| "Counter-Strike 1.6 Server".to_string())
                };
                let title_col = scheme_color_or(
                    &runtime.client_scheme,
                    "TitleText",
                    Color32::from_rgb(255, 174, 0),
                );
                let client_fonts = runtime.client_scheme.fonts.clone();
                runtime.font_atlas.paint_text(
                    ui.painter(),
                    ctx,
                    m.vfs,
                    &client_fonts,
                    "client",
                    "Title",
                    screen_h,
                    title_rect,
                    &title_text,
                    title_col,
                    VguiTextDrawOpts {
                        align: TextAlignment::West,
                        process_hotkey: false,
                        proportional_scale: true,
                    },
                );

                let html_bg = self.parsed.bg_color.map_or_else(
                    || {
                        scheme_color_or(
                            &runtime.client_scheme,
                            "ControlDarkBG",
                            Color32::from_rgba_unmultiplied(0, 0, 0, 180),
                        )
                    },
                    |[r, g, b, a]| Color32::from_rgba_unmultiplied(r, g, b, a),
                );
                let border_col = scheme_color_or(
                    &runtime.client_scheme,
                    "BorderBright",
                    Color32::from_rgba_unmultiplied(188, 112, 0, 128),
                );
                ui.painter().rect_filled(msg_rect, 0.0, html_bg);
                ui.painter().rect_stroke(
                    msg_rect,
                    0.0,
                    Stroke::new(1.0, border_col),
                    StrokeKind::Inside,
                );

                let pad = self.parsed.margin.clamp(4.0, 20.0);
                let inner_msg = msg_rect.shrink(pad);
                let mut clicked_url = None;
                ui.scope_builder(egui::UiBuilder::new().max_rect(inner_msg), |ui| {
                    ScrollArea::vertical()
                        .id_salt("cs16_motd_html_scroll")
                        .max_height(inner_msg.height())
                        .show(ui, |ui| {
                            clicked_url = paint_html_doc_vgui(
                                ui,
                                &self.parsed,
                                runtime,
                                m.vfs,
                                VguiSchemeKind::Client,
                                screen_h,
                            );
                        });
                });
                if let Some(url) = clicked_url {
                    self.last_clicked_url = Some(url);
                }

                if runtime
                    .draw_client_button(
                        ui,
                        ctx,
                        m.vfs,
                        "cs16_motd_ok_btn",
                        ok_rect,
                        &ok_label,
                        true,
                        true,
                        false,
                        "Default",
                        TextAlignment::Center,
                        screen_h,
                    )
                    .clicked()
                {
                    clicked_ok = true;
                }
            });

        if clicked_ok {
            self.open = false;
            return true;
        }
        false
    }
}
