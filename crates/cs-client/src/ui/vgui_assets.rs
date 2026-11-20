//! Real Valve VGUI2 asset bank (`ClientScheme.res`, `cstrike/gfx/vgui/*.tga`,
//! `cstrike/classes/*.res`, `cstrike_english.txt`, `maps/*.txt`, `icon_steam.tga`)
//! plus 1-to-1 CS 1.6 `BackgroundPanel.res` and Steam 2003 window chrome renderers.

use std::collections::HashMap;

use cs_assets::scheme::{
    ClassResPanel, ClientVguiColors, LocalizationTable, load_map_briefing, load_tga_image,
};
use cs_vfs::Vfs;
use egui::{
    Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, TextureHandle, Vec2,
};

use crate::ui::vgui_theme::{VguiPalette, paint_vgui_bevel};

/// Exact in-game CS 1.6 `ClientScheme.res` color palette (translucent black + glowing amber).
#[derive(Debug, Clone, Copy)]
pub struct ClientPalette {
    pub base_text: Color32,
    pub title_text: Color32,
    pub label_dim_text: Color32,
    pub disabled_text: Color32,
    pub window_bg: Color32,
    pub button_bg: Color32,
    pub selection_bg: Color32,
    pub border_bright: Color32,
    pub border_preset: Color32,
    pub team_t: Color32,
    pub team_ct: Color32,
}

impl From<ClientVguiColors> for ClientPalette {
    fn from(c: ClientVguiColors) -> Self {
        let to_c = |a: [u8; 4]| Color32::from_rgba_unmultiplied(a[0], a[1], a[2], a[3]);
        Self {
            base_text: to_c(c.base_text),
            title_text: to_c(c.title_text),
            label_dim_text: to_c(c.label_dim_text),
            disabled_text: to_c(c.disabled_text),
            window_bg: to_c(c.window_bg),
            button_bg: to_c(c.button_bg),
            selection_bg: to_c(c.selection_bg),
            border_bright: to_c(c.border_bright),
            border_preset: to_c(c.border_preset),
            team_t: to_c(c.team_t),
            team_ct: to_c(c.team_ct),
        }
    }
}

/// GPU texture & `.res` cache for all real Valve VGUI2 art & localization strings.
pub struct VguiAssetBank {
    pub client_palette: ClientPalette,
    pub loc: LocalizationTable,
    pub cs_logo: Option<TextureHandle>,
    pub steam_icon: Option<TextureHandle>,
    tga_cache: HashMap<String, Option<TextureHandle>>,
    class_res_cache: HashMap<String, Option<ClassResPanel>>,
    briefing_cache: HashMap<String, String>,
}

impl VguiAssetBank {
    pub fn load(ctx: &egui::Context, vfs: Option<&Vfs>) -> Self {
        let (client_palette, loc, cs_logo, steam_icon) = if let Some(vfs) = vfs {
            let client_palette = ClientPalette::from(ClientVguiColors::load(vfs));
            let loc = LocalizationTable::load(vfs);
            let cs_logo = load_tga_texture(ctx, vfs, "gfx/vgui/CS_logo.tga", "vgui_cs_logo");
            let steam_icon = load_tga_texture(ctx, vfs, "resource/icon_steam.tga", "vgui_steam_icon");
            (client_palette, loc, cs_logo, steam_icon)
        } else {
            (
                ClientPalette::from(ClientVguiColors::default()),
                LocalizationTable::default(),
                None,
                None,
            )
        };

        Self {
            client_palette,
            loc,
            cs_logo,
            steam_icon,
            tga_cache: HashMap::new(),
            class_res_cache: HashMap::new(),
            briefing_cache: HashMap::new(),
        }
    }

    pub fn get_tga(
        &mut self,
        ctx: &egui::Context,
        vfs: Option<&Vfs>,
        path: &str,
    ) -> Option<&TextureHandle> {
        let key = path.to_ascii_lowercase();
        if !self.tga_cache.contains_key(&key) {
            let loaded = vfs.and_then(|v| load_tga_texture(ctx, v, path, &format!("vgui_{key}")));
            self.tga_cache.insert(key.clone(), loaded);
        }
        self.tga_cache.get(&key).and_then(|o| o.as_ref())
    }

    pub fn get_class_res(&mut self, vfs: Option<&Vfs>, name: &str) -> Option<ClassResPanel> {
        let key = name.to_ascii_lowercase();
        if !self.class_res_cache.contains_key(&key) {
            let parsed = vfs.and_then(|v| ClassResPanel::load(v, name, &self.loc));
            self.class_res_cache.insert(key.clone(), parsed);
        }
        self.class_res_cache.get(&key).and_then(|o| o.clone())
    }

    pub fn get_map_briefing(&mut self, vfs: Option<&Vfs>, map_name: &str) -> String {
        let key = map_name.to_ascii_lowercase();
        if let Some(cached) = self.briefing_cache.get(&key) {
            return cached.clone();
        }
        let text = vfs
            .and_then(|v| load_map_briefing(v, map_name))
            .unwrap_or_else(|| {
                format!(
                    "{map_name}\n\nCounter-Terrorists: Prevent Terrorists from bombing the target areas. Defuse any planted C4.\n\nTerrorists: Destroy one of the target sites with C4."
                )
            });
        self.briefing_cache.insert(key, text.clone());
        text
    }
}

fn load_tga_texture(
    ctx: &egui::Context,
    vfs: &Vfs,
    path: &str,
    name: &str,
) -> Option<TextureHandle> {
    let img = load_tga_image(vfs, path).ok()?;
    let color_img = egui::ColorImage::from_rgba_unmultiplied(
        [img.width as usize, img.height as usize],
        &img.rgba,
    );
    Some(ctx.load_texture(name, color_img, egui::TextureOptions::LINEAR))
}

/// Geometry helper for the exact CS 1.6 `640 × 480` virtual coordinate system (`BackgroundPanel.res`).
#[derive(Debug, Clone, Copy)]
pub struct VirtualCanvas {
    pub origin: Pos2,
    pub scale: f32,
}

impl VirtualCanvas {
    pub fn from_screen(screen: Rect) -> Self {
        let scale = (screen.height() / 480.0).min(screen.width() / 640.0) * 0.96;
        let w = 640.0 * scale;
        let h = 480.0 * scale;
        let origin = Pos2::new(
            (screen.center().x - w * 0.5).round(),
            (screen.center().y - h * 0.5).round(),
        );
        Self { origin, scale }
    }

    pub fn pt(self, vx: f32, vy: f32) -> Pos2 {
        Pos2::new(
            (self.origin.x + vx * self.scale).round(),
            (self.origin.y + vy * self.scale).round(),
        )
    }

    pub fn rect(self, vx: f32, vy: f32, vw: f32, vh: f32) -> Rect {
        Rect::from_min_size(self.pt(vx, vy), Vec2::new(vw * self.scale, vh * self.scale))
    }
}

/// Paint the exact Counter-Strike 1.6 `Resource/UI/BackgroundPanel.res` rounded translucent black frame:
/// - Rounded top title bar (`20..620`, `20..72`) with `gfx/vgui/CS_logo.tga` tinted `TitleIcon` and bold amber title
/// - Iconic 2px transparent horizontal slit (`y = 72..74`) showing the 3D map underneath
/// - Rounded bottom body panel (`20..620`, `74..460`)
pub fn paint_cs16_background_panel(
    painter: &egui::Painter,
    vc: VirtualCanvas,
    title: &str,
    bank: &VguiAssetBank,
) {
    let cp = bank.client_palette;
    let r = (10.0 * vc.scale).round().clamp(4.0, 24.0) as u8;
    let fill = cp.window_bg; // 0 0 0 200

    let top_rect = vc.rect(20.0, 20.0, 600.0, 52.0);
    painter.rect_filled(
        top_rect,
        CornerRadius {
            nw: r,
            ne: r,
            sw: 0,
            se: 0,
        },
        fill,
    );

    // 2px transparent GapPanel at y = 72..74!
    let body_rect = vc.rect(20.0, 74.0, 600.0, 386.0);
    painter.rect_filled(
        body_rect,
        CornerRadius {
            nw: 0,
            ne: 0,
            sw: r,
            se: r,
        },
        fill,
    );

    // Real CS_logo.tga at (26, 26, 40, 40) tinted amber TitleIcon (255, 174, 0)
    let logo_rect = vc.rect(26.0, 26.0, 40.0, 40.0);
    if let Some(logo) = &bank.cs_logo {
        painter.image(
            logo.id(),
            logo_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            cp.title_text,
        );
    } else {
        painter.text(
            logo_rect.center(),
            egui::Align2::CENTER_CENTER,
            "CS",
            FontId::proportional(20.0 * vc.scale),
            cp.title_text,
        );
    }

    // Exact Title label at (76, 22, 500, 48)
    let title_rect = vc.rect(76.0, 22.0, 520.0, 48.0);
    painter.text(
        Pos2::new(title_rect.left(), title_rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        FontId::proportional(22.0 * vc.scale),
        cp.title_text,
    );
}

/// Exact CS 1.6 `MouseOverPanelButton` styled from `ClientScheme.res`:
/// dark translucent fill (`0 0 0 64`), 1px thin amber-brown border (`188 112 0 140`),
/// warm golden hover (`255 176 0 100`), left-aligned amber text (`255 176 0`).
#[allow(clippy::too_many_arguments)]
pub fn cs16_vgui_button(
    ui: &mut egui::Ui,
    rect: Rect,
    id_salt: &str,
    hotkey: Option<&str>,
    label: &str,
    enabled: bool,
    selected_highlight: bool,
    scale: f32,
    cp: ClientPalette,
) -> (bool, bool) {
    let resp = ui.interact(rect, egui::Id::unique(id_salt), Sense::click());
    let hovered = resp.hovered();
    let active_bg = hovered || selected_highlight;

    let fill = if active_bg && enabled {
        cp.selection_bg
    } else {
        cp.button_bg
    };
    let border_col = if active_bg && enabled {
        cp.border_preset
    } else {
        Color32::from_rgba_unmultiplied(
            cp.border_bright.r(),
            cp.border_bright.g(),
            cp.border_bright.b(),
            165,
        )
    };
    let text_col = if !enabled {
        cp.disabled_text
    } else if active_bg {
        Color32::from_rgb(255, 210, 90)
    } else {
        cp.base_text
    };

    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::ZERO,
        fill,
        Stroke::new(1.0, border_col),
        egui::StrokeKind::Inside,
    );

    let font = FontId::proportional((10.5 * scale).clamp(11.0, 18.0));
    let mut text_x = rect.left() + 6.0 * scale;
    if let Some(k) = hotkey {
        painter.text(
            Pos2::new(text_x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            k,
            font.clone(),
            text_col,
        );
        text_x += 14.0 * scale;
    }
    painter.text(
        Pos2::new(text_x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        font,
        text_col,
    );

    (enabled && resp.clicked(), hovered)
}

/// Render ANY real `cstrike/classes/<name>.res` preview panel (weapon image + 9-row technical specs,
/// equipment description, or player class 256×196 portrait + bio) at `(vx, vy)` on the `640×480` virtual grid!
#[allow(clippy::too_many_arguments)]
pub fn render_class_res_panel(
    ctx: &egui::Context,
    painter: &egui::Painter,
    vc: VirtualCanvas,
    vx: f32,
    vy: f32,
    res_name: &str,
    vfs: Option<&Vfs>,
    bank: &mut VguiAssetBank,
) {
    let cp = bank.client_palette;
    let Some(panel) = bank.get_class_res(vfs, res_name) else {
        return;
    };

    // 1. Dark framed image preview box (`imageBG` + `imageBorder`)
    let [bx, by, bw, bh] = panel.image_box;
    let box_rect = vc.rect(vx + bx, vy + by, bw, bh);
    painter.rect(
        box_rect,
        CornerRadius::ZERO,
        Color32::from_rgba_unmultiplied(0, 0, 0, 170),
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 112, 0, 150)),
        egui::StrokeKind::Inside,
    );

    // 2. Real Valve TGA weapon silhouette or class portrait (`classimage`)
    if let Some(ref img_path) = panel.image_path
        && let Some(tex) = bank.get_tga(ctx, vfs, img_path)
    {
        let [ix, iy, iw, ih] = panel.image_rect;
        // Center horizontally nicely inside imageBG while preserving Valve aspect ratio
        let pad_x = ((bw - iw) * 0.5).max(0.0);
        let img_rect = vc.rect(vx + ix + pad_x, vy + iy, iw, ih);
        painter.image(
            tex.id(),
            img_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    // 3. Exact positioned labels resolved from `cstrike_english.txt`
    for lbl in &panel.labels {
        let pos = vc.pt(vx + lbl.x, vy + lbl.y);
        let max_w = (lbl.w * vc.scale).max(120.0);
        let font_px = if lbl.small_font {
            (8.8 * vc.scale).clamp(10.0, 15.0)
        } else {
            (9.8 * vc.scale).clamp(11.0, 16.5)
        };
        let col = if lbl.bright {
            cp.title_text
        } else {
            Color32::from_rgba_unmultiplied(
                cp.base_text.r(),
                cp.base_text.g(),
                cp.base_text.b(),
                220,
            )
        };
        let galley = painter.layout(
            lbl.text.clone(),
            FontId::proportional(font_px),
            col,
            max_w,
        );
        painter.galley(pos, galley, col);
    }
}

/// Authentic Steam 2003 VGUI2 window (`TrackerScheme.res`):
/// Olive frame (`76 88 68`), raised 1px bevel, left-aligned `icon_steam.tga` + white bold title,
/// and classic square beveled `[×]` close button in the top-right corner!
#[allow(clippy::too_many_arguments)]
pub fn show_steam2003_window(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    open: &mut bool,
    default_pos: Pos2,
    default_size: Vec2,
    min_size: Vec2,
    resizable: bool,
    palette: VguiPalette,
    steam_icon: Option<&TextureHandle>,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    if !*open {
        return;
    }

    let mut close_clicked = false;
    let resp = egui::Window::new(title)
        .id(egui::Id::unique(id))
        .title_bar(false)
        .default_pos(default_pos)
        .default_size(default_size)
        .min_size(min_size)
        .collapsible(false)
        .resizable(resizable)
        .frame(
            egui::Frame::NONE
                .fill(palette.control_bg)
                .inner_margin(egui::Margin::same(8))
                .stroke(Stroke::new(1.0, palette.border_bright)),
        )
        .show(ctx, |ui| {
            // Custom Steam 2003 titlebar: [Steam Icon] Title ....... [×]
            ui.horizontal(|ui| {
                if let Some(icon) = steam_icon {
                    ui.image(egui::load::SizedTexture::new(icon.id(), Vec2::new(15.0, 15.0)));
                }
                ui.label(
                    RichText::new(title)
                        .color(palette.bright_text)
                        .strong()
                        .size(13.5),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (btn_rect, btn_resp) =
                        ui.allocate_exact_size(Vec2::new(17.0, 17.0), Sense::click());
                    let bg = if btn_resp.hovered() {
                        palette.control_dark_bg
                    } else {
                        palette.control_bg
                    };
                    ui.painter().rect_filled(btn_rect, CornerRadius::ZERO, bg);
                    paint_vgui_bevel(ui.painter(), btn_rect, !btn_resp.is_pointer_button_down_on(), palette);
                    let x_col = if btn_resp.hovered() {
                        palette.bright_text
                    } else {
                        palette.base_text
                    };
                    let c = btn_rect.center();
                    let s = Stroke::new(1.5, x_col);
                    ui.painter().line_segment([c + Vec2::new(-3.5, -3.5), c + Vec2::new(3.5, 3.5)], s);
                    ui.painter().line_segment([c + Vec2::new(3.5, -3.5), c + Vec2::new(-3.5, 3.5)], s);
                    if btn_resp.clicked() {
                        close_clicked = true;
                    }
                });
            });
            ui.add_space(4.0);
            add_contents(ui);
        });

    if let Some(inner) = resp {
        paint_vgui_bevel(
            &ctx.layer_painter(inner.response.layer_id),
            inner.response.rect,
            true,
            palette,
        );
    }

    if close_clicked {
        *open = false;
    }
}
