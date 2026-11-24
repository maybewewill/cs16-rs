//! Stateful VGUI2 control models and Steam 2003 `egui` painters for all 12 core controls:
//! `Panel`, `Label`, `Button`/`MouseOverPanelButton`, `ImagePanel`, `Frame`, `ListPanel`,
//! `RichText`, `ComboBox`, `Slider`, `CheckButton`, `TextEntry`, `PropertySheet` + `ProgressBar`.

use cs_assets::vgui_font::MarlettGlyph;
use cs_assets::vgui_res::TextAlignment;
use egui::{
    Align, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, RichText, ScrollArea, Sense,
    Stroke, TextureHandle, Vec2,
};

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
