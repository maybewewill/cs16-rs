//! Authentic Counter-Strike 1.6 VGUI2 Developer Console (`~`).

use cs_assets::scheme::LocalizationTable;
use cs_cvar::{CVarRegistry, EngineAction, LogLevel};
use cs_vfs::Vfs;
use egui::{Color32, Key, RichText, ScrollArea, Stroke, TextEdit, TextureHandle, Vec2};

use crate::ui::parse_rgb;
use crate::ui::vgui2::{TrackerButtonCfg, VguiRuntime, paint_vgui_border};
use crate::ui::vgui_assets::{TrackerWindowCfg, show_tracker_window};
use crate::ui::vgui_theme::{VguiPalette, paint_vgui_bevel};

/// Render context for [`ConsoleUi`].
pub struct ConsoleRenderCtx<'a> {
    pub vfs: Option<&'a Vfs>,
    pub loc: Option<&'a LocalizationTable>,
    pub available_maps: &'a [String],
    pub palette: VguiPalette,
    pub steam_icon: Option<&'a TextureHandle>,
}

#[derive(Default)]
pub struct ConsoleUi {
    pub open: bool,
    input: String,
    history_cursor: Option<usize>,
    request_focus: bool,
    scroll_to_bottom: bool,
    last_log_len: usize,
}

impl ConsoleUi {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.request_focus = true;
            self.scroll_to_bottom = true;
        }
    }

    pub fn open(&mut self) {
        self.open = true;
        self.request_focus = true;
        self.scroll_to_bottom = true;
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        cvars: &mut CVarRegistry,
        c: &ConsoleRenderCtx<'_>,
    ) -> Vec<EngineAction> {
        let mut actions = Vec::new();
        if !self.open {
            return actions;
        }

        if cvars.log_lines().len() != self.last_log_len {
            self.last_log_len = cvars.log_lines().len();
            self.scroll_to_bottom = true;
        }

        let palette = c.palette;
        let available_maps = c.available_maps;
        let title = c
            .loc
            .map(|l| l.resolve("#GameUI_Console"))
            .filter(|s| !s.is_empty() && !s.starts_with('#'))
            .unwrap_or("Console")
            .to_string();
        let mut is_open = self.open;

        show_tracker_window(
            ctx,
            runtime,
            &mut is_open,
            &TrackerWindowCfg {
                id: "cs16_vgui_console",
                title: &title,
                default_pos: egui::pos2(60.0, 50.0),
                default_size: egui::vec2(640.0, 400.0),
                min_size: egui::vec2(420.0, 240.0),
                resizable: true,
                palette,
                steam_icon: c.steam_icon,
                vfs: c.vfs,
            },
            |ui, rt| {
                // Recessed log viewport
                let avail_h = (ui.available_height() - 46.0).max(120.0);
                let log_frame = egui::Frame::NONE
                    .fill(palette.window_bg)
                    .inner_margin(egui::Margin::same(6))
                    .stroke(Stroke::new(1.0, palette.border_dark));

                let frame_resp = log_frame.show(ui, |ui| {
                    ui.set_min_size(egui::vec2(ui.available_width(), avail_h));
                    ScrollArea::vertical()
                        .id_salt("cs16_console_scroll")
                        .max_height(avail_h)
                        .auto_shrink([false, false])
                        .stick_to_bottom(self.scroll_to_bottom)
                        .show(ui, |ui| {
                            let (cr, cg, cb) = parse_rgb(cvars.get_str("con_color"));
                            let con_col = Color32::from_rgb(cr, cg, cb);
                            for line in cvars.log_lines() {
                                let color = match line.level {
                                    LogLevel::Info => con_col,
                                    LogLevel::Command => palette.bright_text,
                                    LogLevel::Warning => Color32::from_rgb(235, 190, 95),
                                    LogLevel::Error => Color32::from_rgb(235, 105, 95),
                                };
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&line.text).monospace().color(color),
                                    )
                                    .wrap(),
                                );
                            }
                        });
                });
                paint_vgui_bevel(ui.painter(), frame_resp.response.rect, false, palette);
                paint_vgui_border(ui.painter(), frame_resp.response.rect, "InsetBorder", &rt.tracker_scheme);
                self.scroll_to_bottom = false;

                ui.add_space(6.0);

                // Input bar + Submit button
                ui.horizontal(|ui| {
                    let input_w = (ui.available_width() - 78.0).max(120.0);
                    let edit = TextEdit::singleline(&mut self.input)
                        .id_salt("cs16_console_input")
                        .font(egui::TextStyle::Monospace)
                        .desired_width(input_w)
                        .lock_focus(true);
                    let edit_resp = ui.add(edit);

                    if self.request_focus {
                        edit_resp.request_focus();
                        self.request_focus = false;
                    }

                    // Strip accidental ` or ~ typed while opening/closing the console
                    if self.input.contains('`') || self.input.contains('~') || self.input.contains('ё') || self.input.contains('Ё') {
                        self.input.retain(|ch| !matches!(ch, '`' | '~' | 'ё' | 'Ё'));
                    }

                    // Handle Tab autocomplete and Up/Down history when input is focused
                    if edit_resp.has_focus() {
                        if ui.input(|i| i.key_pressed(Key::Tab)) {
                            let (completed, matches) = cvars.complete(&self.input, available_maps);
                            if let Some(comp) = completed {
                                self.input = if matches.len() == 1 && !comp.ends_with(' ') {
                                    format!("{comp} ")
                                } else {
                                    comp
                                };
                            }
                            if matches.len() > 1 {
                                let preview = matches
                                    .iter()
                                    .take(24)
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join("  ");
                                cvars.info(format!("Matches ({}): {preview}", matches.len()));
                                self.scroll_to_bottom = true;
                            }
                        }

                        if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
                            let len = cvars.history().len();
                            if len > 0 {
                                let next_idx = match self.history_cursor {
                                    None => len - 1,
                                    Some(0) => 0,
                                    Some(idx) => idx - 1,
                                };
                                self.history_cursor = Some(next_idx);
                                if let Some(h) = cvars.history().get(next_idx) {
                                    self.input = h.clone();
                                }
                            }
                        }

                        if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
                            let len = cvars.history().len();
                            if let Some(idx) = self.history_cursor {
                                if idx + 1 < len {
                                    self.history_cursor = Some(idx + 1);
                                    if let Some(h) = cvars.history().get(idx + 1) {
                                        self.input = h.clone();
                                    }
                                } else {
                                    self.history_cursor = None;
                                    self.input.clear();
                                }
                            }
                        }
                    }

                    let enter_pressed = edit_resp.lost_focus()
                        && ui.input(|i| i.key_pressed(Key::Enter));
                    let submit_clicked = rt
                        .draw_tracker_button(
                            ui,
                            c.vfs,
                            palette,
                            &TrackerButtonCfg {
                                id: "console_submit_btn",
                                label: "Submit",
                                min_size: Vec2::new(68.0, 22.0),
                                default_btn: false,
                                enabled: true,
                            },
                        )
                        .clicked();

                    if enter_pressed || submit_clicked {
                        let cmd = std::mem::take(&mut self.input);
                        self.history_cursor = None;
                        actions.extend(cvars.execute_console_line(&cmd));
                        self.scroll_to_bottom = true;
                        edit_resp.request_focus();
                    }
                });
            },
        );

        self.open = is_open;
        actions
    }
}
