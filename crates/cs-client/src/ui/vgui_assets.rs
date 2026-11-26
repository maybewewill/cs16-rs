//! The VGUI2 colours (`ClientScheme.res`), `icon_steam.tga` and the Steam 2003 window chrome.

use cs_assets::scheme::ClientVguiColors;
use cs_assets::vgui_res::TextAlignment;
use cs_vfs::Vfs;
use egui::{
    Color32, CornerRadius, Pos2, RichText, Sense, Stroke, TextureHandle, Vec2,
};

use crate::ui::vgui2::VguiRuntime;
use crate::ui::vgui2::font_atlas::VguiTextDrawOpts;
use crate::ui::vgui_theme::{VguiPalette, paint_vgui_bevel};

/// Exact in-game CS 1.6 `ClientScheme.res` color palette (translucent black + glowing amber).
#[derive(Debug, Clone, Copy)]
pub struct ClientPalette {
    pub base_text: Color32,
    pub title_text: Color32,
    pub border_bright: Color32,
    pub team_t: Color32,
    pub team_ct: Color32,
}

impl From<ClientVguiColors> for ClientPalette {
    fn from(c: ClientVguiColors) -> Self {
        let to_c = |a: [u8; 4]| Color32::from_rgba_unmultiplied(a[0], a[1], a[2], a[3]);
        Self {
            base_text: to_c(c.base_text),
            title_text: to_c(c.title_text),
            border_bright: to_c(c.border_bright),
            team_t: to_c(c.team_t),
            team_ct: to_c(c.team_ct),
        }
    }
}

/// Transparent 128x128 `cs16-rs` brand emblem (tintable for CS 1.6 VGUI2 titlebars and panels).
pub const BRAND_LOGO_UI_PNG: &[u8] = include_bytes!("../../assets/logo-ui.png");

/// Decode [`BRAND_LOGO_UI_PNG`] into `(width, height, rgba8)` for UI texture uploads or tests.
pub fn decode_brand_logo_ui_rgba() -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory_with_format(BRAND_LOGO_UI_PNG, image::ImageFormat::Png).ok()?;
    let rgba = img.into_rgba8();
    let (w, h) = rgba.dimensions();
    Some((w, h, rgba.into_raw()))
}

/// Upload the transparent `cs16-rs` brand emblem into an `egui` texture handle.
pub fn load_brand_logo_texture(ctx: &egui::Context, name: &str) -> Option<TextureHandle> {
    let (w, h, rgba) = decode_brand_logo_ui_rgba()?;
    let color_img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
    Some(ctx.load_texture(name, color_img, egui::TextureOptions::LINEAR))
}

/// The VGUI2 colours and the emblem icon the menus and dialogs draw with.
pub struct VguiAssetBank {
    pub client_palette: ClientPalette,
    pub steam_icon: Option<TextureHandle>,
}

impl VguiAssetBank {
    pub fn load(ctx: &egui::Context, vfs: Option<&Vfs>) -> Self {
        let client_palette = if let Some(vfs) = vfs {
            ClientPalette::from(ClientVguiColors::load(vfs))
        } else {
            ClientPalette::from(ClientVguiColors::default())
        };
        let steam_icon = load_brand_logo_texture(ctx, "vgui_cs16rs_icon");
        Self {
            client_palette,
            steam_icon,
        }
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

/// Configuration for [`show_tracker_window`] (Steam 2003 olive `TrackerScheme.res` window).
pub struct TrackerWindowCfg<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub default_pos: Pos2,
    pub default_size: Vec2,
    pub min_size: Vec2,
    pub resizable: bool,
    pub palette: VguiPalette,
    pub steam_icon: Option<&'a TextureHandle>,
    pub vfs: Option<&'a Vfs>,
}

/// Steam 2003 `TrackerScheme.res` window backed by [`VguiRuntime`], `VguiFontAtlas` (`DefaultBold`),
/// `icon_steam.tga`, 1px raised bevels, and interactive titlebar close sounds.
pub fn show_tracker_window(
    ctx: &egui::Context,
    runtime: &mut VguiRuntime,
    open: &mut bool,
    cfg: &TrackerWindowCfg<'_>,
    add_contents: impl FnOnce(&mut egui::Ui, &mut VguiRuntime),
) {
    if !*open {
        return;
    }
    let palette = cfg.palette;
    let screen_h = ctx.content_rect().height().round().max(480.0) as u32;
    let mut close_clicked = false;
    let resp = egui::Window::new(cfg.title)
        .id(egui::Id::unique(cfg.id))
        .title_bar(false)
        .default_pos(cfg.default_pos)
        .default_size(cfg.default_size)
        .min_size(cfg.min_size)
        .collapsible(false)
        .resizable(cfg.resizable)
        .frame(
            egui::Frame::NONE
                .fill(palette.control_bg)
                .inner_margin(egui::Margin::same(8))
                .stroke(Stroke::new(1.0, palette.border_bright)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if let Some(icon) = cfg.steam_icon {
                    ui.image(egui::load::SizedTexture::new(icon.id(), Vec2::new(15.0, 15.0)));
                }
                let tracker_fonts = runtime.tracker_scheme.fonts.clone();
                let title_w = (ui.available_width() - 26.0).max(80.0);
                let (title_rect, _) = ui.allocate_exact_size(Vec2::new(title_w, 18.0), Sense::hover());
                runtime.font_atlas.paint_text(
                    ui.painter(),
                    ctx,
                    cfg.vfs,
                    &tracker_fonts,
                    "tracker",
                    "DefaultBold",
                    screen_h,
                    title_rect,
                    cfg.title,
                    palette.bright_text,
                    VguiTextDrawOpts {
                        align: TextAlignment::West,
                        process_hotkey: false,
                        proportional_scale: false,
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (btn_rect, btn_resp) =
                        ui.allocate_exact_size(Vec2::new(17.0, 17.0), Sense::click());
                    runtime.track_interactive(&format!("{}_close", cfg.id), &btn_resp);
                    let down = btn_resp.is_pointer_button_down_on();
                    let bg = if down {
                        palette.window_bg
                    } else if btn_resp.hovered() {
                        palette.control_dark_bg
                    } else {
                        palette.control_bg
                    };
                    ui.painter().rect_filled(btn_rect, CornerRadius::ZERO, bg);
                    paint_vgui_bevel(ui.painter(), btn_rect, !down, palette);
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
            add_contents(ui, runtime);
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

#[cfg(test)]
mod tests {
    use super::decode_brand_logo_ui_rgba;

    #[test]
    fn brand_logo_ui_png_decodes_with_transparent_background() {
        let (w, h, rgba) = decode_brand_logo_ui_rgba().expect("decode logo-ui.png");
        assert_eq!((w, h), (128, 128));
        assert_eq!(rgba.len(), 128 * 128 * 4);
        assert_eq!(rgba[3], 0, "top-left corner must be transparent");
        assert!(
            rgba.as_chunks::<4>().0.iter().any(|px| px[3] == 255),
            "emblem body must contain opaque foreground pixels"
        );
    }
}
