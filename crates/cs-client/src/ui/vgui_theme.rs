//! Authentic Valve Steam 2003 / VGUI2 visual theme driven by `TrackerScheme.res`.

use cs_assets::scheme::VguiColors;
use egui::{Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Stroke, Style, TextStyle, Visuals};

#[derive(Debug, Clone, Copy)]
pub struct VguiPalette {
    pub control_bg: Color32,
    pub control_dark_bg: Color32,
    pub window_bg: Color32,
    pub selection_bg: Color32,
    pub border_bright: Color32,
    pub border_dark: Color32,
    pub base_text: Color32,
    pub bright_text: Color32,
    pub dim_text: Color32,
    pub bright_control_text: Color32,
}

impl From<VguiColors> for VguiPalette {
    fn from(c: VguiColors) -> Self {
        let to_c32 = |rgba: [u8; 4]| Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]);
        Self {
            control_bg: to_c32(c.control_bg),
            control_dark_bg: to_c32(c.control_dark_bg),
            window_bg: to_c32(c.window_bg),
            selection_bg: to_c32(c.selection_bg),
            border_bright: to_c32(c.border_bright),
            border_dark: to_c32(c.border_dark),
            base_text: to_c32(c.base_text),
            bright_text: to_c32(c.bright_text),
            dim_text: to_c32(c.dim_text),
            bright_control_text: to_c32(c.bright_control_text),
        }
    }
}

/// Apply the Steam 2003 olive-green VGUI2 theme and authentic Tahoma/Consolas fonts to an `egui::Context`.
pub fn apply_vgui2_theme(ctx: &egui::Context, palette: VguiPalette) {
    install_vgui_fonts(ctx);
    let mut visuals = Visuals::dark();
    visuals.override_text_color = Some(palette.base_text);
    visuals.panel_fill = palette.control_bg;
    visuals.window_fill = palette.control_bg;
    visuals.extreme_bg_color = palette.window_bg;
    visuals.faint_bg_color = palette.control_dark_bg;
    visuals.code_bg_color = palette.window_bg;

    visuals.window_corner_radius = CornerRadius::ZERO;
    visuals.menu_corner_radius = CornerRadius::ZERO;
    visuals.window_stroke = Stroke::new(1.0, palette.border_bright);
    visuals.window_shadow = egui::Shadow::NONE;
    visuals.popup_shadow = egui::Shadow::NONE;

    visuals.selection.bg_fill = palette.selection_bg;
    visuals.selection.stroke = Stroke::new(1.0, palette.bright_text);

    // Noninteractive (labels, separators)
    visuals.widgets.noninteractive.bg_fill = palette.control_bg;
    visuals.widgets.noninteractive.weak_bg_fill = palette.window_bg;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border_dark);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.base_text);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::ZERO;

    // Inactive buttons / checkboxes / inputs
    visuals.widgets.inactive.bg_fill = palette.control_bg;
    visuals.widgets.inactive.weak_bg_fill = palette.control_bg;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette.border_bright);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, palette.base_text);
    visuals.widgets.inactive.corner_radius = CornerRadius::ZERO;

    // Hovered controls
    visuals.widgets.hovered.bg_fill = palette.control_dark_bg;
    visuals.widgets.hovered.weak_bg_fill = palette.control_dark_bg;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.bright_control_text);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, palette.bright_control_text);
    visuals.widgets.hovered.corner_radius = CornerRadius::ZERO;
    visuals.widgets.hovered.expansion = 0.0;

    // Active / pressed controls
    visuals.widgets.active.bg_fill = palette.window_bg;
    visuals.widgets.active.weak_bg_fill = palette.window_bg;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, palette.border_dark);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, palette.bright_text);
    visuals.widgets.active.corner_radius = CornerRadius::ZERO;
    visuals.widgets.active.expansion = 0.0;

    // Open combo/menu
    visuals.widgets.open.bg_fill = palette.window_bg;
    visuals.widgets.open.weak_bg_fill = palette.window_bg;
    visuals.widgets.open.bg_stroke = Stroke::new(1.0, palette.bright_control_text);
    visuals.widgets.open.fg_stroke = Stroke::new(1.0, palette.bright_control_text);
    visuals.widgets.open.corner_radius = CornerRadius::ZERO;

    let mut style = Style {
        visuals,
        ..Style::default()
    };
    style.text_styles = [
        (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(15.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(6.0, 5.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.spacing.window_margin = egui::Margin::same(8);
    // Authentic VGUI2 windows snap open instantly with zero fade-in translucency
    style.animation_time = 0.0;

    ctx.set_theme(egui::Theme::Dark);
    ctx.set_style_of(egui::Theme::Dark, style);
}

fn install_vgui_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let proportional_candidates = [
        r"C:\Windows\Fonts\tahoma.ttf",
        r"C:\Windows\Fonts\verdana.ttf",
        r"C:\Windows\Fonts\segoeui.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/System/Library/Fonts/Supplemental/Tahoma.ttf",
    ];
    for path in proportional_candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "vgui_prop".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "vgui_prop".to_owned());
            break;
        }
    }

    let mono_candidates = [
        r"C:\Windows\Fonts\consola.ttf",
        r"C:\Windows\Fonts\lucon.ttf",
        r"C:\Windows\Fonts\cour.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/System/Library/Fonts/Menlo.ttc",
    ];
    for path in mono_candidates {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "vgui_mono".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .insert(0, "vgui_mono".to_owned());
            break;
        }
    }

    ctx.set_fonts(fonts);
}

/// Paint authentic 1px VGUI2 bevel lines (raised = bright top-left / dark bottom-right;
/// recessed = dark top-left / bright bottom-right).
pub fn paint_vgui_bevel(painter: &egui::Painter, rect: Rect, raised: bool, palette: VguiPalette) {
    let (tl, br) = if raised {
        (palette.border_bright, palette.border_dark)
    } else {
        (palette.border_dark, palette.border_bright)
    };
    let s_tl = Stroke::new(1.0, tl);
    let s_br = Stroke::new(1.0, br);

    let min = rect.min;
    let max = rect.max;
    painter.line_segment([Pos2::new(min.x, min.y), Pos2::new(max.x, min.y)], s_tl);
    painter.line_segment([Pos2::new(min.x, min.y), Pos2::new(min.x, max.y)], s_tl);
    painter.line_segment([Pos2::new(min.x, max.y), Pos2::new(max.x, max.y)], s_br);
    painter.line_segment([Pos2::new(max.x, min.y), Pos2::new(max.x, max.y)], s_br);
}

/// Draw a VGUI2 tab button with the signature golden active label.
pub fn vgui_tab(ui: &mut egui::Ui, label: &str, selected: bool, palette: VguiPalette) -> bool {
    let text_color = if selected {
        palette.bright_control_text
    } else {
        palette.base_text
    };
    let fill = if selected {
        palette.control_bg
    } else {
        palette.window_bg
    };
    let btn = egui::Button::new(egui::RichText::new(label).color(text_color))
        .fill(fill)
        .corner_radius(CornerRadius::ZERO)
        .stroke(Stroke::new(
            1.0,
            if selected {
                palette.border_bright
            } else {
                palette.border_dark
            },
        ));
    ui.add(btn).clicked()
}
