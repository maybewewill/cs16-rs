//! The VGUI2 colours (`ClientScheme.res`), `icon_steam.tga` and the Steam 2003 window chrome.

use cs_assets::scheme::{ClientVguiColors, load_tga_image};
use cs_vfs::Vfs;
use egui::{
    Color32, CornerRadius, Pos2, RichText, Sense, Stroke, TextureHandle, Vec2,
};

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

/// The VGUI2 colours and the icon the menus draw with.
pub struct VguiAssetBank {
    pub client_palette: ClientPalette,
    pub steam_icon: Option<TextureHandle>,
}

impl VguiAssetBank {
    pub fn load(ctx: &egui::Context, vfs: Option<&Vfs>) -> Self {
        let (client_palette, steam_icon) = if let Some(vfs) = vfs {
            (
                ClientPalette::from(ClientVguiColors::load(vfs)),
                load_tga_texture(ctx, vfs, "resource/icon_steam.tga", "vgui_steam_icon"),
            )
        } else {
            (ClientPalette::from(ClientVguiColors::default()), None)
        };
        Self {
            client_palette,
            steam_icon,
        }
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
