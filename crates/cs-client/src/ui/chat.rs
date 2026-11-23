//! The say prompt (`messagemode`, `Y`/`U`): a line of text that goes to the server as `say`.

use egui::{Color32, Key, RichText, Stroke};

use crate::ui::vgui_theme::VguiPalette;

#[derive(Default)]
pub struct ChatInputUi {
    pub open: bool,
    pub team_only: bool,
    buffer: String,
    request_focus: bool,
}

impl ChatInputUi {
    pub fn open_chat(&mut self, team_only: bool) {
        self.open = true;
        self.team_only = team_only;
        self.buffer.clear();
        self.request_focus = true;
    }

    pub fn show(&mut self, ctx: &egui::Context, _palette: VguiPalette) -> Option<(bool, String)> {
        if !self.open {
            return None;
        }
        let mut submitted = None;
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("cs16_chat_input"))
            .fixed_pos(egui::pos2(24.0, screen.bottom() - 108.0))
            .show(ctx, |ui| {
                // Authentic translucent black CS 1.6 say prompt
                egui::Frame::NONE
                    .fill(Color32::from_rgba_unmultiplied(0, 0, 0, 175))
                    .inner_margin(egui::Margin::symmetric(8, 5))
                    .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 112, 0, 160)))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let label = if self.team_only { "say_team :" } else { "say :" };
                            ui.label(RichText::new(label).color(Color32::from_rgb(255, 176, 0)));
                            let edit = egui::TextEdit::singleline(&mut self.buffer)
                                .desired_width(340.0)
                                .frame(egui::Frame::NONE)
                                .text_color(Color32::from_rgb(255, 220, 120))
                                .lock_focus(true);
                            let resp = ui.add(edit);
                            if self.request_focus {
                                resp.request_focus();
                                self.request_focus = false;
                            }
                            if ui.input(|i| i.key_pressed(Key::Enter)) {
                                let text = self.buffer.trim().to_string();
                                if !text.is_empty() {
                                    submitted = Some((self.team_only, text));
                                }
                                self.buffer.clear();
                                self.open = false;
                            } else if ui.input(|i| i.key_pressed(Key::Escape)) {
                                self.buffer.clear();
                                self.open = false;
                            }
                        });
                    });
            });
        submitted
    }
}
