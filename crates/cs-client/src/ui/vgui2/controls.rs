//! Stateful VGUI2 control models and Steam 2003 `egui` painters for all 12 core controls:
//! `Panel`, `Label`, `Button`/`MouseOverPanelButton`, `ImagePanel`, `Frame`, `ListPanel`,
//! `RichText`, `ComboBox`, `Slider`, `CheckButton`, `TextEntry`, `PropertySheet` + `ProgressBar`.

use cs_assets::vgui_font::{MarlettGlyph, SchemeFontSet};
use cs_assets::vgui_res::TextAlignment;
use cs_vfs::Vfs;
use egui::{
    Align, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, RichText, ScrollArea, Sense,
    Stroke, TextureHandle, Vec2,
};

use super::runtime::{VguiRuntime, VguiUiSound};
use crate::ui::vgui_theme::{VguiPalette, paint_vgui_bevel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListSortDir {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListColumn {
    pub name: String,
    pub width: u32,
}

/// Sortable multi-column VGUI2 `ListPanel` state (used in ServerBrowser, PlayerList, ScoreBoard).
#[derive(Debug, Clone, Default)]
pub struct VguiListPanelState {
    pub columns: Vec<ListColumn>,
    pub rows: Vec<Vec<String>>,
    pub sort_col: usize,
    pub sort_dir: Option<ListSortDir>,
    pub selected_row: Option<usize>,
}

impl VguiListPanelState {
    pub fn sort_by(&mut self, col: usize) {
        if col >= self.columns.len() {
            return;
        }
        let next_dir = if self.sort_col == col && self.sort_dir == Some(ListSortDir::Ascending) {
            ListSortDir::Descending
        } else {
            ListSortDir::Ascending
        };
        self.sort_col = col;
        self.sort_dir = Some(next_dir);
        self.rows.sort_by(|a, b| {
            let va = a.get(col).map(String::as_str).unwrap_or("");
            let vb = b.get(col).map(String::as_str).unwrap_or("");
            let ord = match (va.parse::<f64>(), vb.parse::<f64>()) {
                (Ok(na), Ok(nb)) => na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal),
                _ => va.to_ascii_lowercase().cmp(&vb.to_ascii_lowercase()),
            };
            match next_dir {
                ListSortDir::Ascending => ord,
                ListSortDir::Descending => ord.reverse(),
            }
        });
    }
}

#[derive(Debug, Clone, Default)]
pub struct VguiRichTextState {
    pub lines: Vec<(String, [u8; 4])>,
}

impl VguiRichTextState {
    pub fn push(&mut self, text: impl Into<String>, color: [u8; 4]) {
        if self.lines.len() >= 512 {
            self.lines.remove(0);
        }
        self.lines.push((text.into(), color));
    }
}

#[derive(Debug, Clone, Default)]
pub struct VguiComboState {
    pub items: Vec<String>,
    pub selected: usize,
}

impl VguiComboState {
    pub fn selected_text(&self) -> &str {
        self.items.get(self.selected).map(String::as_str).unwrap_or("")
    }
}

#[derive(Debug, Clone)]
pub struct VguiSliderState {
    pub value: f32,
    pub min: f32,
    pub max: f32,
}

impl Default for VguiSliderState {
    fn default() -> Self {
        Self {
            value: 0.5,
            min: 0.0,
            max: 1.0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct VguiPropertySheetState {
    pub tabs: Vec<String>,
    pub active_tab: usize,
}

#[derive(Debug, Clone)]
pub enum VguiControlState {
    Text(String),
    Check(bool),
    Slider(VguiSliderState),
    Combo(VguiComboState),
    List(VguiListPanelState),
    RichText(VguiRichTextState),
    PropertySheet(VguiPropertySheetState),
    Progress(f32),
}

pub fn align_to_egui(a: TextAlignment) -> egui::Align2 {
    match a {
        TextAlignment::NorthWest => egui::Align2::LEFT_TOP,
        TextAlignment::North => egui::Align2::CENTER_TOP,
        TextAlignment::NorthEast => egui::Align2::RIGHT_TOP,
        TextAlignment::West => egui::Align2::LEFT_CENTER,
        TextAlignment::Center => egui::Align2::CENTER_CENTER,
        TextAlignment::East => egui::Align2::RIGHT_CENTER,
        TextAlignment::SouthWest => egui::Align2::LEFT_BOTTOM,
        TextAlignment::South => egui::Align2::CENTER_BOTTOM,
        TextAlignment::SouthEast => egui::Align2::RIGHT_BOTTOM,
    }
}

pub fn paint_marlett_glyph(painter: &egui::Painter, center: Pos2, size: f32, ch: char, col: Color32) {
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        ch.to_string(),
        FontId::new(size, FontFamily::Name("marlett".into())),
        col,
    );
}

pub fn draw_vgui_panel(painter: &egui::Painter, rect: Rect, fill: Color32, palette: VguiPalette, sunken: bool) {
    painter.rect_filled(rect, CornerRadius::ZERO, fill);
    paint_vgui_bevel(painter, rect, !sunken, palette);
}

pub fn draw_vgui_label(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    align: TextAlignment,
    font: FontId,
    color: Color32,
) {
    let anchor = align_to_egui(align);
    let pos = match align {
        TextAlignment::West | TextAlignment::NorthWest | TextAlignment::SouthWest => {
            Pos2::new(rect.left() + 4.0, rect.center().y)
        }
        TextAlignment::East | TextAlignment::NorthEast | TextAlignment::SouthEast => {
            Pos2::new(rect.right() - 4.0, rect.center().y)
        }
        _ => rect.center(),
    };
    painter.text(pos, anchor, text, font, color);
}

pub fn draw_vgui_button(
    ui: &mut egui::Ui,
    rect: Rect,
    text: &str,
    default_btn: bool,
    enabled: bool,
    palette: VguiPalette,
) -> egui::Response {
    let resp = ui.allocate_rect(rect, if enabled { Sense::click() } else { Sense::hover() });
    let down = resp.is_pointer_button_down_on();
    let bg = if !enabled {
        palette.control_bg
    } else if down {
        palette.window_bg
    } else if resp.hovered() {
        palette.control_dark_bg
    } else {
        palette.control_bg
    };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::ZERO, bg);
    paint_vgui_bevel(p, rect, !down, palette);
    if default_btn {
        p.rect_stroke(
            rect.shrink(2.0),
            CornerRadius::ZERO,
            Stroke::new(1.0, Color32::BLACK),
            egui::StrokeKind::Inside,
        );
    }
    let fg = if !enabled {
        palette.dim_text
    } else if resp.hovered() {
        palette.bright_control_text
    } else {
        palette.base_text
    };
    p.text(
        Pos2::new(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        FontId::proportional(13.0),
        fg,
    );
    resp
}

pub fn draw_vgui_image_panel(painter: &egui::Painter, rect: Rect, tex: Option<&TextureHandle>, tint: Color32) {
    if let Some(t) = tex {
        painter.image(
            t.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            tint,
        );
    } else {
        painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(60));
    }
}

pub fn draw_vgui_check_button(
    ui: &mut egui::Ui,
    id: egui::Id,
    checked: &mut bool,
    label: &str,
    palette: VguiPalette,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let (box_rect, resp) = ui.allocate_exact_size(Vec2::splat(15.0), Sense::click());
        let p = ui.painter();
        p.rect_filled(box_rect, CornerRadius::ZERO, palette.window_bg);
        paint_vgui_bevel(p, box_rect, false, palette);
        if resp.clicked() {
            *checked = !*checked;
            changed = true;
        }
        if *checked {
            let fg = palette.bright_control_text;
            let has_marlett = ui.ctx().fonts(|f| f.definitions().font_data.contains_key("marlett"));
            if has_marlett {
                paint_marlett_glyph(p, box_rect.center(), 13.0, MarlettGlyph::CHECK, fg);
            } else {
                p.line_segment(
                    [box_rect.left_top() + Vec2::new(3.0, 7.0), box_rect.center() + Vec2::new(-1.0, 3.0)],
                    Stroke::new(1.8, fg),
                );
                p.line_segment(
                    [box_rect.center() + Vec2::new(-1.0, 3.0), box_rect.right_top() + Vec2::new(-3.0, 3.0)],
                    Stroke::new(1.8, fg),
                );
            }
        }
        if ui.add(egui::Label::new(RichText::new(label).color(palette.base_text)).sense(Sense::click())).clicked() {
            *checked = !*checked;
            changed = true;
        }
    });
    let _ = id;
    changed
}

pub fn draw_vgui_list_panel(
    ui: &mut egui::Ui,
    id: &str,
    state: &mut VguiListPanelState,
    height: f32,
    palette: VguiPalette,
) -> Option<usize> {
    let mut double_clicked = None;
    let frame_resp = egui::Frame::NONE
        .fill(palette.window_bg)
        .stroke(Stroke::new(1.0, palette.border_dark))
        .inner_margin(egui::Margin::same(2))
        .show(ui, |ui| {
            // Header row
            let mut sort_req = None;
            ui.horizontal(|ui| {
                for (col_idx, col) in state.columns.iter().enumerate() {
                    let marker = if state.sort_col == col_idx {
                        match state.sort_dir {
                            Some(ListSortDir::Ascending) => " ^",
                            Some(ListSortDir::Descending) => " v",
                            None => "",
                        }
                    } else {
                        ""
                    };
                    let btn = egui::Button::new(
                        RichText::new(format!("{}{marker}", col.name))
                            .size(12.0)
                            .color(palette.bright_text),
                    )
                    .min_size(Vec2::new(col.width as f32, 20.0))
                    .corner_radius(CornerRadius::ZERO);
                    if ui.add(btn).clicked() {
                        sort_req = Some(col_idx);
                    }
                }
            });
            if let Some(col_idx) = sort_req {
                state.sort_by(col_idx);
            }
            ui.separator();
            ScrollArea::vertical()
                .id_salt(id)
                .max_height((height - 28.0).max(60.0))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for idx in 0..state.rows.len() {
                        let is_sel = state.selected_row == Some(idx);
                        let (row_rect, row_resp) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width().max(200.0), 19.0),
                            Sense::click(),
                        );
                        if row_resp.clicked() {
                            state.selected_row = Some(idx);
                        }
                        if row_resp.double_clicked() {
                            state.selected_row = Some(idx);
                            double_clicked = Some(idx);
                        }
                        if is_sel {
                            ui.painter().rect_filled(row_rect, CornerRadius::ZERO, palette.selection_bg);
                        } else if row_resp.hovered() {
                            ui.painter().rect_filled(row_rect, CornerRadius::ZERO, palette.control_dark_bg);
                        }
                        let mut x = row_rect.left() + 4.0;
                        let fg = if is_sel { palette.bright_text } else { palette.base_text };
                        for (c_idx, col) in state.columns.iter().enumerate() {
                            let val = state.rows[idx].get(c_idx).map(String::as_str).unwrap_or("");
                            ui.painter().text(
                                Pos2::new(x, row_rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                val,
                                FontId::proportional(12.5),
                                fg,
                            );
                            x += col.width as f32 + 4.0;
                        }
                    }
                });
        });
    paint_vgui_bevel(ui.painter(), frame_resp.response.rect, false, palette);
    double_clicked
}

pub fn draw_vgui_rich_text(
    ui: &mut egui::Ui,
    id: &str,
    state: &VguiRichTextState,
    height: f32,
    palette: VguiPalette,
) {
    let resp = egui::Frame::NONE
        .fill(palette.window_bg)
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ScrollArea::vertical()
                .id_salt(id)
                .max_height(height.max(48.0))
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (line, [r, g, b, a]) in &state.lines {
                        ui.label(
                            RichText::new(line)
                                .monospace()
                                .color(Color32::from_rgba_unmultiplied(*r, *g, *b, *a)),
                        );
                    }
                });
        });
    paint_vgui_bevel(ui.painter(), resp.response.rect, false, palette);
}

pub fn draw_vgui_progress_bar(painter: &egui::Painter, rect: Rect, fraction: f32, palette: VguiPalette) {
    painter.rect_filled(rect, CornerRadius::ZERO, palette.window_bg);
    paint_vgui_bevel(painter, rect, false, palette);
    let inner = rect.shrink(3.0);
    let frac = fraction.clamp(0.0, 1.0);
    let block_w: f32 = 10.0;
    let gap: f32 = 2.0;
    let fill_w = inner.width() * frac;
    let mut x = inner.left();
    while x + 2.0 < inner.left() + fill_w {
        let w = block_w.min((inner.left() + fill_w) - x);
        let b = Rect::from_min_size(Pos2::new(x, inner.top()), Vec2::new(w, inner.height()));
        painter.rect_filled(b, CornerRadius::ZERO, palette.bright_control_text);
        x += block_w + gap;
    }
}

pub fn draw_vgui_combo_box(
    ui: &mut egui::Ui,
    id: &str,
    state: &mut VguiComboState,
    width: f32,
    palette: VguiPalette,
) -> bool {
    let mut changed = false;
    let cur = state.selected_text().to_string();
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(RichText::new(cur).color(palette.base_text))
        .show_ui(ui, |ui| {
            for (i, item) in state.items.iter().enumerate() {
                if ui.selectable_label(state.selected == i, item).clicked() {
                    state.selected = i;
                    changed = true;
                }
            }
        });
    changed
}

pub fn draw_vgui_slider(ui: &mut egui::Ui, state: &mut VguiSliderState, label: &str, palette: VguiPalette) -> bool {
    ui.horizontal(|ui| {
        if !label.is_empty() {
            ui.label(RichText::new(label).color(palette.base_text));
        }
        ui.add(egui::Slider::new(&mut state.value, state.min..=state.max).show_value(true)).changed()
    })
    .inner
}

pub fn draw_vgui_text_entry(
    ui: &mut egui::Ui,
    text: &mut String,
    password: bool,
    numeric_only: bool,
    width: f32,
) -> bool {
    let te = egui::TextEdit::singleline(text)
        .password(password)
        .desired_width(width);
    let resp = ui.add(te);
    if resp.changed() && numeric_only {
        text.retain(|c| c.is_ascii_digit() || c == '.' || c == '-');
    }
    resp.changed()
}

pub fn draw_vgui_property_sheet(
    ui: &mut egui::Ui,
    sheet: &mut VguiPropertySheetState,
    palette: VguiPalette,
    body: impl FnOnce(&mut egui::Ui, usize),
) {
    ui.with_layout(egui::Layout::left_to_right(Align::Min), |ui| {
        for (idx, title) in sheet.tabs.iter().enumerate() {
            if crate::ui::vgui_theme::vgui_tab(ui, title, sheet.active_tab == idx, palette) {
                sheet.active_tab = idx;
            }
        }
    });
    let frame_resp = egui::Frame::NONE
        .fill(palette.control_bg)
        .inner_margin(egui::Margin::same(10))
        .stroke(Stroke::new(1.0, palette.border_dark))
        .show(ui, |ui| {
            body(ui, sheet.active_tab);
        });
    paint_vgui_bevel(ui.painter(), frame_resp.response.rect, false, palette);
}

/// Shared render context for pixel-exact `TrackerScheme.res` `.res`-driven controls.
#[derive(Clone, Copy)]
pub struct TrackerCtx<'a> {
    pub ctx: &'a egui::Context,
    pub vfs: Option<&'a Vfs>,
    pub palette: VguiPalette,
    pub screen_h: u32,
    pub scale: f32,
}

impl<'a> TrackerCtx<'a> {
    pub fn fonts<'r>(&self, rt: &'r VguiRuntime) -> &'r SchemeFontSet {
        &rt.tracker_scheme.fonts
    }
}

/// Paint a crisp 1px pixel-aligned VGUI2 bevel using `rect_filled` (avoids AA blur on edges).
pub fn paint_tracker_bevel_rect(
    painter: &egui::Painter,
    rect: Rect,
    raised: bool,
    palette: VguiPalette,
) {
    let r = Rect::from_min_max(
        Pos2::new(rect.left().round(), rect.top().round()),
        Pos2::new(rect.right().round(), rect.bottom().round()),
    );
    if r.width() < 2.0 || r.height() < 2.0 {
        return;
    }
    let (tl, br) = if raised {
        (palette.border_bright, palette.border_dark)
    } else {
        (palette.border_dark, palette.border_bright)
    };
    painter.rect_filled(Rect::from_min_max(r.min, Pos2::new(r.right(), r.top() + 1.0)), 0.0, tl);
    painter.rect_filled(Rect::from_min_max(r.min, Pos2::new(r.left() + 1.0, r.bottom())), 0.0, tl);
    painter.rect_filled(Rect::from_min_max(Pos2::new(r.left(), r.bottom() - 1.0), r.max), 0.0, br);
    painter.rect_filled(Rect::from_min_max(Pos2::new(r.right() - 1.0, r.top()), r.max), 0.0, br);
}

#[allow(clippy::too_many_arguments)]
pub fn paint_tracker_label_rect(
    painter: &egui::Painter,
    rt: &mut VguiRuntime,
    tc: &TrackerCtx<'_>,
    rect: Rect,
    text: &str,
    font_name: &str,
    align: TextAlignment,
    color: Color32,
    wrap: bool,
    enabled: bool,
) {
    if text.is_empty() {
        return;
    }
    let fonts = rt.tracker_scheme.fonts.clone();
    let fname = if font_name.trim().is_empty() { "Default" } else { font_name };
    if !enabled {
        let shadow_col = Color32::from_rgb(40, 46, 34);
        let dis_col = Color32::from_rgb(117, 128, 111);
        rt.font_atlas.paint_wrapped_text_with_scale(
            painter, tc.ctx, tc.vfs, &fonts, "tracker", fname, tc.screen_h,
            rect.translate(Vec2::new(1.0, 1.0)), text, shadow_col, align, wrap, tc.scale,
        );
        rt.font_atlas.paint_wrapped_text_with_scale(
            painter, tc.ctx, tc.vfs, &fonts, "tracker", fname, tc.screen_h,
            rect, text, dis_col, align, wrap, tc.scale,
        );
        return;
    }
    rt.font_atlas.paint_wrapped_text_with_scale(
        painter, tc.ctx, tc.vfs, &fonts, "tracker", fname, tc.screen_h,
        rect, text, color, align, wrap, tc.scale,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn draw_tracker_button_rect(
    ui: &mut egui::Ui,
    rt: &mut VguiRuntime,
    tc: &TrackerCtx<'_>,
    id_salt: &str,
    rect: Rect,
    label: &str,
    align: TextAlignment,
    default_btn: bool,
    enabled: bool,
) -> egui::Response {
    let r = Rect::from_min_max(
        Pos2::new(rect.left().round(), rect.top().round()),
        Pos2::new(rect.right().round(), rect.bottom().round()),
    );
    let id = ui.make_persistent_id(id_salt);
    let resp = ui.interact(r, id, if enabled { Sense::click() } else { Sense::hover() });
    if enabled {
        rt.track_interactive(id_salt, &resp);
    }
    let down = enabled && resp.is_pointer_button_down_on();
    let bg = if !enabled {
        tc.palette.control_bg
    } else if down {
        tc.palette.window_bg
    } else if resp.hovered() {
        tc.palette.control_dark_bg
    } else {
        tc.palette.control_bg
    };
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::ZERO, bg);
    if default_btn {
        // Original `BorderSelection` (0,0,0) outer 1px + inner bevel (measured on Advanced...).
        p.rect_filled(Rect::from_min_max(r.min, Pos2::new(r.right(), r.top() + 1.0)), 0.0, Color32::BLACK);
        p.rect_filled(Rect::from_min_max(r.min, Pos2::new(r.left() + 1.0, r.bottom())), 0.0, Color32::BLACK);
        p.rect_filled(Rect::from_min_max(Pos2::new(r.left(), r.bottom() - 1.0), r.max), 0.0, Color32::BLACK);
        p.rect_filled(Rect::from_min_max(Pos2::new(r.right() - 1.0, r.top()), r.max), 0.0, Color32::BLACK);
        paint_tracker_bevel_rect(p, r.shrink(1.0), !down, tc.palette);
    } else {
        paint_tracker_bevel_rect(p, r, !down, tc.palette);
    }
    let fg = if !enabled {
        Color32::from_rgb(117, 128, 111)
    } else if resp.hovered() || down {
        tc.palette.bright_control_text
    } else {
        tc.palette.bright_text
    };
    let inset = (6.0 * tc.scale).round();
    let text_rect = r.shrink2(Vec2::new(inset, 0.0));
    let fonts = rt.tracker_scheme.fonts.clone();
    rt.font_atlas.paint_text_with_scale(
        p, tc.ctx, tc.vfs, &fonts, "tracker", "Default", tc.screen_h,
        text_rect, label, fg, align, true, tc.scale,
    );
    resp
}

pub fn draw_tracker_close_button(
    ui: &mut egui::Ui,
    rt: &mut VguiRuntime,
    tc: &TrackerCtx<'_>,
    id_salt: &str,
    rect: Rect,
) -> bool {
    let r = Rect::from_min_max(
        Pos2::new(rect.left().round(), rect.top().round()),
        Pos2::new(rect.right().round(), rect.bottom().round()),
    );
    let resp = ui.interact(r, ui.make_persistent_id(id_salt), Sense::click());
    rt.track_interactive(id_salt, &resp);
    let down = resp.is_pointer_button_down_on();
    let bg = if down {
        tc.palette.window_bg
    } else if resp.hovered() {
        tc.palette.control_dark_bg
    } else {
        tc.palette.control_bg
    };
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::ZERO, bg);
    paint_tracker_bevel_rect(p, r, !down, tc.palette);
    let x_col = if resp.hovered() {
        tc.palette.bright_text
    } else {
        Color32::from_rgb(200, 206, 195)
    };
    let c = r.center();
    let half = (3.5 * tc.scale).round().max(3.0);
    let s = Stroke::new((1.5 * tc.scale).max(1.2), x_col);
    p.line_segment([c + Vec2::new(-half, -half), c + Vec2::new(half, half)], s);
    p.line_segment([c + Vec2::new(half, -half), c + Vec2::new(-half, half)], s);
    resp.clicked()
}

#[allow(clippy::too_many_arguments)]
pub fn draw_tracker_check_button_rect(
    ui: &mut egui::Ui,
    rt: &mut VguiRuntime,
    tc: &TrackerCtx<'_>,
    id_salt: &str,
    rect: Rect,
    checked: &mut bool,
    label: &str,
    enabled: bool,
) -> bool {
    let r = Rect::from_min_max(
        Pos2::new(rect.left().round(), rect.top().round()),
        Pos2::new(rect.right().round(), rect.bottom().round()),
    );
    let id = ui.make_persistent_id(id_salt);
    let resp = ui.interact(r, id, if enabled { Sense::click() } else { Sense::hover() });
    let mut changed = false;
    if enabled && resp.clicked() {
        *checked = !*checked;
        rt.push_ui_sound(VguiUiSound::Click);
        changed = true;
    }

    let box_sz = (14.0 * tc.scale).round().max(12.0);
    let box_x = (r.left() + 3.0 * tc.scale).round();
    let box_y = (r.center().y - box_sz * 0.5).round();
    let box_rect = Rect::from_min_size(Pos2::new(box_x, box_y), Vec2::splat(box_sz));

    let p = ui.painter();
    let box_fill = if *checked {
        Color32::from_rgb(98, 99, 92)
    } else {
        tc.palette.control_bg
    };
    p.rect_filled(box_rect, CornerRadius::ZERO, box_fill);
    let border_col = Color32::from_rgb(112, 104, 95);
    p.rect_filled(Rect::from_min_max(box_rect.min, Pos2::new(box_rect.right(), box_rect.top() + 1.0)), 0.0, border_col);
    p.rect_filled(Rect::from_min_max(box_rect.min, Pos2::new(box_rect.left() + 1.0, box_rect.bottom())), 0.0, border_col);
    p.rect_filled(Rect::from_min_max(Pos2::new(box_rect.left(), box_rect.bottom() - 1.0), box_rect.max), 0.0, border_col);
    p.rect_filled(Rect::from_min_max(Pos2::new(box_rect.right() - 1.0, box_rect.top()), box_rect.max), 0.0, border_col);

    if *checked {
        let chk_col = if enabled {
            tc.palette.bright_text
        } else {
            Color32::from_rgb(160, 170, 149)
        };
        let has_marlett = tc.ctx.fonts(|f| f.definitions().font_data.contains_key("marlett"));
        if has_marlett {
            paint_marlett_glyph(p, box_rect.center(), box_sz * 0.9, MarlettGlyph::CHECK, chk_col);
        } else {
            p.line_segment(
                [
                    box_rect.left_top() + Vec2::new(box_sz * 0.22, box_sz * 0.52),
                    box_rect.left_top() + Vec2::new(box_sz * 0.42, box_sz * 0.75),
                ],
                Stroke::new(2.0, chk_col),
            );
            p.line_segment(
                [
                    box_rect.left_top() + Vec2::new(box_sz * 0.42, box_sz * 0.75),
                    box_rect.left_top() + Vec2::new(box_sz * 0.80, box_sz * 0.24),
                ],
                Stroke::new(2.0, chk_col),
            );
        }
    }

    let fg = if !enabled {
        Color32::from_rgb(117, 128, 111)
    } else if *checked {
        tc.palette.bright_control_text
    } else {
        tc.palette.base_text
    };
    let text_x = (box_rect.right() + 7.0 * tc.scale).round();
    let text_rect = Rect::from_min_max(Pos2::new(text_x, r.top()), r.max);
    let fonts = rt.tracker_scheme.fonts.clone();
    rt.font_atlas.paint_text_with_scale(
        p, tc.ctx, tc.vfs, &fonts, "tracker", "Default", tc.screen_h,
        text_rect, label, fg, TextAlignment::West, true, tc.scale,
    );
    changed
}

#[allow(clippy::too_many_arguments)]
pub fn draw_tracker_combo_box_rect(
    ui: &mut egui::Ui,
    rt: &mut VguiRuntime,
    tc: &TrackerCtx<'_>,
    id_salt: &str,
    rect: Rect,
    items: &[String],
    selected: &mut usize,
    enabled: bool,
) -> bool {
    let r = Rect::from_min_max(
        Pos2::new(rect.left().round(), rect.top().round()),
        Pos2::new(rect.right().round(), rect.bottom().round()),
    );
    let id = ui.make_persistent_id(id_salt);
    let open_id = id.with("combo_open");
    let mut is_open = tc.ctx.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    let resp = ui.interact(r, id, if enabled { Sense::click() } else { Sense::hover() });
    if enabled && resp.clicked() {
        is_open = !is_open;
        rt.push_ui_sound(VguiUiSound::Click);
    }

    let p = ui.painter();
    p.rect_filled(r, CornerRadius::ZERO, tc.palette.window_bg);
    paint_tracker_bevel_rect(p, r, false, tc.palette);

    let sel_text = items.get(*selected).map(String::as_str).unwrap_or("");
    let inset = (6.0 * tc.scale).round();
    let arrow_space = (18.0 * tc.scale).round();
    let text_rect = Rect::from_min_max(
        Pos2::new(r.left() + inset, r.top()),
        Pos2::new((r.right() - arrow_space).max(r.left() + inset), r.bottom()),
    );
    let fg = if enabled {
        tc.palette.base_text
    } else {
        Color32::from_rgb(117, 128, 111)
    };
    let fonts = rt.tracker_scheme.fonts.clone();
    rt.font_atlas.paint_text_with_scale(
        p, tc.ctx, tc.vfs, &fonts, "tracker", "Default", tc.screen_h,
        text_rect, sel_text, fg, TextAlignment::West, false, tc.scale,
    );

    // Small downward triangle near right edge (as measured in original GameUI ComboBox).
    let ax = (r.right() - 10.0 * tc.scale).round();
    let ay = r.center().y.round();
    let ah = (2.5 * tc.scale).round().max(2.0);
    let aw = (3.5 * tc.scale).round().max(3.0);
    p.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(ax - aw, ay - ah * 0.5),
            Pos2::new(ax + aw, ay - ah * 0.5),
            Pos2::new(ax, ay + ah),
        ],
        fg,
        Stroke::NONE,
    ));

    let mut changed = false;
    if is_open && enabled && !items.is_empty() {
        let row_h = (19.0 * tc.scale).round();
        let list_h = (items.len().min(8) as f32 * row_h + 4.0).round();
        let popup_rect = Rect::from_min_size(
            Pos2::new(r.left(), r.bottom()),
            Vec2::new(r.width(), list_h),
        );
        let area_resp = egui::Area::new(open_id.with("popup"))
            .order(egui::Order::Tooltip)
            .fixed_pos(popup_rect.min)
            .show(tc.ctx, |ui| {
                egui::Frame::NONE
                    .fill(tc.palette.control_bg)
                    .stroke(Stroke::new(1.0, tc.palette.border_dark))
                    .inner_margin(egui::Margin::same(1))
                    .show(ui, |ui| {
                        ui.set_min_size(Vec2::new(r.width() - 2.0, list_h - 2.0));
                        ScrollArea::vertical()
                            .id_salt(open_id.with("scroll"))
                            .max_height(list_h - 4.0)
                            .show(ui, |ui| {
                                for (idx, item) in items.iter().enumerate() {
                                    let (row_r, row_resp) = ui.allocate_exact_size(
                                        Vec2::new(r.width() - 4.0, row_h),
                                        Sense::click(),
                                    );
                                    let hi = row_resp.hovered() || *selected == idx;
                                    if hi {
                                        ui.painter().rect_filled(row_r, 0.0, tc.palette.selection_bg);
                                    }
                                    let text_c = if hi {
                                        tc.palette.bright_text
                                    } else {
                                        tc.palette.base_text
                                    };
                                    rt.font_atlas.paint_text_with_scale(
                                        ui.painter(), tc.ctx, tc.vfs, &fonts, "tracker", "Default",
                                        tc.screen_h, row_r.shrink2(Vec2::new(inset, 0.0)), item,
                                        text_c, TextAlignment::West, false, tc.scale,
                                    );
                                    if row_resp.clicked() {
                                        *selected = idx;
                                        changed = true;
                                        is_open = false;
                                        rt.push_ui_sound(VguiUiSound::Click);
                                    }
                                }
                            });
                    });
            });
        if tc.ctx.input(|i| i.pointer.any_pressed())
            && !resp.hovered()
            && !area_resp.response.contains_pointer()
        {
            is_open = false;
        }
    }
    tc.ctx.data_mut(|d| d.insert_temp(open_id, is_open));
    changed
}

#[allow(clippy::too_many_arguments)]
pub fn draw_tracker_text_entry_rect(
    ui: &mut egui::Ui,
    rt: &mut VguiRuntime,
    tc: &TrackerCtx<'_>,
    id_salt: &str,
    rect: Rect,
    text: &mut String,
    password: bool,
    numeric_only: bool,
    enabled: bool,
) -> bool {
    let r = Rect::from_min_max(
        Pos2::new(rect.left().round(), rect.top().round()),
        Pos2::new(rect.right().round(), rect.bottom().round()),
    );
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::ZERO, tc.palette.window_bg);
    paint_tracker_bevel_rect(p, r, false, tc.palette);

    let edit_id = ui.make_persistent_id(id_salt).with("te");
    let has_focus = ui.memory(|m| m.has_focus(edit_id));
    if !enabled {
        let fonts = rt.tracker_scheme.fonts.clone();
        let shown = if password {
            "*".repeat(text.chars().count())
        } else {
            text.clone()
        };
        rt.font_atlas.paint_text_with_scale(
            p, tc.ctx, tc.vfs, &fonts, "tracker", "Default", tc.screen_h,
            r.shrink2(Vec2::new((5.0 * tc.scale).round(), 0.0)),
            &shown, Color32::from_rgb(117, 128, 111), TextAlignment::West, false, tc.scale,
        );
        return false;
    }

    if !has_focus {
        let resp = ui.interact(r, edit_id.with("hit"), Sense::click());
        if resp.clicked() {
            ui.memory_mut(|m| m.request_focus(edit_id));
        }
        let shown = if password {
            "*".repeat(text.chars().count())
        } else {
            text.clone()
        };
        let fonts = rt.tracker_scheme.fonts.clone();
        rt.font_atlas.paint_text_with_scale(
            ui.painter(), tc.ctx, tc.vfs, &fonts, "tracker", "Default", tc.screen_h,
            r.shrink2(Vec2::new((5.0 * tc.scale).round(), 0.0)),
            &shown, tc.palette.base_text, TextAlignment::West, false, tc.scale,
        );
        return false;
    }

    let inner = r.shrink2(Vec2::new((4.0 * tc.scale).round(), (2.0 * tc.scale).round()));
    let mut changed = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
        let te = egui::TextEdit::singleline(text)
            .id(edit_id)
            .frame(egui::Frame::NONE)
            .password(password)
            .text_color(tc.palette.base_text)
            .font(FontId::proportional((13.0 * tc.scale).round()))
            .desired_width(inner.width());
        let resp = ui.add(te);
        if resp.changed() {
            if numeric_only {
                text.retain(|c| c.is_ascii_digit() || c == '.' || c == '-');
            }
            changed = true;
        }
    });
    changed
}

#[allow(clippy::too_many_arguments)]
pub fn draw_tracker_slider_rect(
    ui: &mut egui::Ui,
    rt: &mut VguiRuntime,
    tc: &TrackerCtx<'_>,
    id_salt: &str,
    rect: Rect,
    value: &mut f32,
    min: f32,
    max: f32,
    left_text: &str,
    right_text: &str,
) -> bool {
    let r = Rect::from_min_max(
        Pos2::new(rect.left().round(), rect.top().round()),
        Pos2::new(rect.right().round(), rect.bottom().round()),
    );
    let s = tc.scale;
    let thumb_w = (7.0 * s).round().max(6.0);
    let thumb_h = (15.0 * s).round().max(12.0);
    let pad_x = (thumb_w * 0.5).ceil();
    let track_x0 = (r.left() + pad_x).round();
    let track_x1 = (r.right() - pad_x).max(track_x0 + 8.0).round();
    let track_y = (r.top() + 8.0 * s).round();
    let track_h = (3.0 * s).round().max(3.0);
    let track_rect = Rect::from_min_max(
        Pos2::new(track_x0, track_y),
        Pos2::new(track_x1, track_y + track_h),
    );

    let hit_rect = Rect::from_min_max(
        Pos2::new(r.left(), r.top()),
        Pos2::new(r.right(), (track_y + thumb_h).min(r.bottom())),
    );
    let id = ui.make_persistent_id(id_salt);
    let resp = ui.interact(hit_rect, id, Sense::click_and_drag());
    let mut changed = false;
    let span = (max - min).max(0.0001);
    if (resp.clicked() || resp.dragged())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let frac = ((pos.x - track_x0) / (track_x1 - track_x0).max(1.0)).clamp(0.0, 1.0);
        let next = min + frac * span;
        if (next - *value).abs() > 1e-5 {
            *value = next;
            changed = true;
        }
    }

    let p = ui.painter();
    // Track: dark outer/inner + SliderTrackColor (31,31,31)
    p.rect_filled(track_rect, 0.0, Color32::from_rgb(31, 31, 31));
    paint_tracker_bevel_rect(p, track_rect, false, tc.palette);

    // 9 vertical tick marks (`SliderTickColor` 127,140,127) matching `OptionsSub*.res` captures.
    let tick_col = Color32::from_rgb(127, 140, 127);
    let tick_y0 = track_rect.bottom() + (3.0 * s).round();
    let tick_h = (3.0 * s).round().max(2.0);
    for i in 0..9 {
        let frac = i as f32 / 8.0;
        let tx = (track_x0 + frac * (track_x1 - track_x0)).round();
        p.rect_filled(
            Rect::from_min_size(Pos2::new(tx, tick_y0), Vec2::new(1.0, tick_h)),
            0.0,
            tick_col,
        );
    }

    // Thumb (`ControlBG` + 1px raised bevel).
    let frac = ((*value - min) / span).clamp(0.0, 1.0);
    let thumb_cx = (track_x0 + frac * (track_x1 - track_x0)).round();
    let thumb_rect = Rect::from_center_size(
        Pos2::new(thumb_cx, track_rect.center().y),
        Vec2::new(thumb_w, thumb_h),
    );
    p.rect_filled(thumb_rect, 0.0, tc.palette.control_bg);
    paint_tracker_bevel_rect(p, thumb_rect, true, tc.palette);

    if !left_text.is_empty() || !right_text.is_empty() {
        let lbl_y = tick_y0 + tick_h + (2.0 * s).round();
        let lbl_rect = Rect::from_min_max(Pos2::new(r.left(), lbl_y), r.max);
        let fonts = rt.tracker_scheme.fonts.clone();
        if !left_text.is_empty() {
            rt.font_atlas.paint_text_with_scale(
                p, tc.ctx, tc.vfs, &fonts, "tracker", "DefaultSmall", tc.screen_h,
                lbl_rect, left_text, tc.palette.dim_text, TextAlignment::NorthWest, false, s,
            );
        }
        if !right_text.is_empty() {
            rt.font_atlas.paint_text_with_scale(
                p, tc.ctx, tc.vfs, &fonts, "tracker", "DefaultSmall", tc.screen_h,
                lbl_rect, right_text, tc.palette.dim_text, TextAlignment::NorthEast, false, s,
            );
        }
    }
    changed
}

#[allow(clippy::too_many_arguments)]
pub fn draw_tracker_list_panel_rect(
    ui: &mut egui::Ui,
    rt: &mut VguiRuntime,
    tc: &TrackerCtx<'_>,
    id_salt: &str,
    rect: Rect,
    columns: &[(&str, f32)],
    rows: &[Vec<String>],
    selected_row: &mut Option<usize>,
) -> Option<usize> {
    let r = Rect::from_min_max(
        Pos2::new(rect.left().round(), rect.top().round()),
        Pos2::new(rect.right().round(), rect.bottom().round()),
    );
    let s = tc.scale;
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::ZERO, tc.palette.window_bg);
    paint_tracker_bevel_rect(p, r, false, tc.palette);

    let inner = r.shrink(1.0);
    let hdr_h = (22.0 * s).round();
    let row_h = (18.0 * s).round();
    let sb_w = (16.0 * s).round();

    let hdr_rect = Rect::from_min_size(inner.min, Vec2::new((inner.width() - sb_w).max(20.0), hdr_h));
    let fonts = rt.tracker_scheme.fonts.clone();
    let mut col_x = hdr_rect.left() + (6.0 * s).round();
    for (title, w_unscaled) in columns {
        let cw = (*w_unscaled * s).round();
        let crect = Rect::from_min_size(Pos2::new(col_x, hdr_rect.top() - 1.0), Vec2::new(cw, hdr_h + 2.0));
        rt.font_atlas.paint_text_with_scale(
            p, tc.ctx, tc.vfs, &fonts, "tracker", "DefaultSmall", tc.screen_h,
            crect, title, tc.palette.bright_control_text, TextAlignment::West, false, s,
        );
        col_x += cw;
    }

    let body_rect = Rect::from_min_max(
        Pos2::new(inner.left(), inner.top() + hdr_h),
        inner.max,
    );
    let mut double_clicked = None;
    let list_id = ui.make_persistent_id(id_salt);
    let scroll_id = list_id.with("scroll_off");
    let visible_rows = ((body_rect.height() / row_h).floor() as usize).max(1);
    let max_off = rows.len().saturating_sub(visible_rows);
    let mut scroll_off = tc
        .ctx
        .data(|d| d.get_temp::<usize>(scroll_id))
        .unwrap_or(0)
        .min(max_off);

    let list_hit = ui.interact(
        Rect::from_min_max(body_rect.min, Pos2::new(body_rect.right() - sb_w, body_rect.bottom())),
        list_id.with("body"),
        Sense::click(),
    );
    if list_hit.hovered() {
        let wheel = tc.ctx.input(|i| i.smooth_scroll_delta.y);
        if wheel.abs() > 2.0 {
            if wheel < 0.0 {
                scroll_off = (scroll_off + 2).min(max_off);
            } else {
                scroll_off = scroll_off.saturating_sub(2);
            }
        }
    }

    let clipped = p.with_clip_rect(Rect::from_min_max(
        body_rect.min,
        Pos2::new(body_rect.right() - sb_w, body_rect.bottom()),
    ));
    for vis_idx in 0..=visible_rows {
        let r_idx = scroll_off + vis_idx;
        if r_idx >= rows.len() {
            break;
        }
        let ry = body_rect.top() + vis_idx as f32 * row_h;
        let row_r = Rect::from_min_size(
            Pos2::new(body_rect.left(), ry),
            Vec2::new((body_rect.width() - sb_w).max(10.0), row_h),
        );
        if list_hit.clicked()
            && let Some(pos) = list_hit.interact_pointer_pos()
            && row_r.contains(pos)
        {
            *selected_row = Some(r_idx);
        }
        if list_hit.double_clicked()
            && let Some(pos) = list_hit.interact_pointer_pos()
            && row_r.contains(pos)
        {
            *selected_row = Some(r_idx);
            double_clicked = Some(r_idx);
        }
        let is_sel = *selected_row == Some(r_idx);
        if is_sel {
            clipped.rect_filled(row_r, 0.0, tc.palette.selection_bg);
        }
        let mut x = row_r.left() + (6.0 * s).round();
        let fg = if is_sel { tc.palette.bright_text } else { tc.palette.base_text };
        for (c_idx, (_, w_unscaled)) in columns.iter().enumerate() {
            let cw = (*w_unscaled * s).round();
            let val = rows[r_idx].get(c_idx).map(String::as_str).unwrap_or("");
            if !val.is_empty() {
                let crect = Rect::from_min_size(Pos2::new(x, ry), Vec2::new(cw, row_h));
                rt.font_atlas.paint_text_with_scale(
                    &clipped, tc.ctx, tc.vfs, &fonts, "tracker", "Default", tc.screen_h,
                    crect, val, fg, TextAlignment::West, false, s,
                );
            }
            x += cw;
        }
    }

    // Scrollbar on right (`ControlDarkBG` track + beveled `ControlBG` buttons/thumb).
    let sb_rect = Rect::from_min_max(
        Pos2::new(inner.right() - sb_w, inner.top()),
        inner.max,
    );
    p.rect_filled(sb_rect, 0.0, tc.palette.control_dark_bg);
    let btn_h = sb_w;
    let up_r = Rect::from_min_size(sb_rect.min, Vec2::new(sb_w, btn_h));
    let dn_r = Rect::from_min_size(Pos2::new(sb_rect.left(), sb_rect.bottom() - btn_h), Vec2::new(sb_w, btn_h));
    p.rect_filled(up_r, 0.0, tc.palette.control_bg);
    paint_tracker_bevel_rect(p, up_r, true, tc.palette);
    p.rect_filled(dn_r, 0.0, tc.palette.control_bg);
    paint_tracker_bevel_rect(p, dn_r, true, tc.palette);
    let tri_col = tc.palette.base_text;
    let tw = (3.5 * s).round().max(2.5);
    let th = (2.5 * s).round().max(2.0);
    let uc = up_r.center();
    p.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(uc.x, uc.y - th * 0.6),
            Pos2::new(uc.x + tw, uc.y + th * 0.6),
            Pos2::new(uc.x - tw, uc.y + th * 0.6),
        ],
        tri_col,
        Stroke::NONE,
    ));
    let dc = dn_r.center();
    p.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(dc.x - tw, dc.y - th * 0.6),
            Pos2::new(dc.x + tw, dc.y - th * 0.6),
            Pos2::new(dc.x, dc.y + th * 0.6),
        ],
        tri_col,
        Stroke::NONE,
    ));
    let up_hit = ui.interact(up_r, list_id.with("up"), Sense::click());
    let dn_hit = ui.interact(dn_r, list_id.with("dn"), Sense::click());
    if up_hit.clicked() {
        scroll_off = scroll_off.saturating_sub(2);
    }
    if dn_hit.clicked() {
        scroll_off = (scroll_off + 2).min(max_off);
    }
    let track_h = (dn_r.top() - up_r.bottom()).max(8.0);
    if rows.len() > visible_rows {
        let thumb_h = (track_h * (visible_rows as f32 / rows.len() as f32))
            .clamp((16.0 * s).round(), track_h);
        let frac = if max_off > 0 { scroll_off as f32 / max_off as f32 } else { 0.0 };
        let ty = up_r.bottom() + frac * (track_h - thumb_h);
        let thumb_r = Rect::from_min_size(Pos2::new(sb_rect.left(), ty.round()), Vec2::new(sb_w, thumb_h.round()));
        p.rect_filled(thumb_r, 0.0, tc.palette.control_bg);
        paint_tracker_bevel_rect(p, thumb_r, true, tc.palette);
    }
    tc.ctx.data_mut(|d| d.insert_temp(scroll_id, scroll_off));
    double_clicked
}
