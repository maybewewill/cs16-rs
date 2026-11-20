//! Authentic Counter-Strike 1.6 VGUI2 Developer Console (`~`).

use cs_cvar::{CVarRegistry, EngineAction, LogLevel};
use egui::{Color32, CornerRadius, Key, RichText, ScrollArea, Stroke, TextEdit, TextureHandle};

use crate::ui::vgui_assets::show_steam2003_window;
use crate::ui::vgui_theme::{VguiPalette, paint_vgui_bevel};

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
        cvars: &mut CVarRegistry,
        available_maps: &[String],
        palette: VguiPalette,
        steam_icon: Option<&TextureHandle>,
    ) -> Vec<EngineAction> {
        let mut actions = Vec::new();
        if !self.open {
            return actions;
        }

        if cvars.log_lines().len() != self.last_log_len {
            self.last_log_len = cvars.log_lines().len();
            self.scroll_to_bottom = true;
        }

        let mut is_open = self.open;
        show_steam2003_window(
            ctx,
            "cs16_vgui_console",
            "Console",
            &mut is_open,
            egui::pos2(60.0, 50.0),
            egui::vec2(640.0, 400.0),
            egui::vec2(420.0, 240.0),
            true,
            palette,
            steam_icon,
            |ui| {
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
                            for line in cvars.log_lines() {
                                let color = match line.level {
                                    LogLevel::Info => palette.base_text,
                                    LogLevel::Command => palette.bright_control_text,
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
                        self.input.retain(|c| !matches!(c, '`' | '~' | 'ё' | 'Ё'));
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
                    let submit_clicked = ui
                        .add(
                            egui::Button::new("Submit")
                                .min_size(egui::vec2(68.0, 22.0))
                                .corner_radius(CornerRadius::ZERO),
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
