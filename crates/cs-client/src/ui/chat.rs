//! The say prompt (`messagemode`, `Y`/`U`): a line of text that goes to the server as `say`.

use cs_assets::vgui_res::TextAlignment;
use cs_vfs::Vfs;
use egui::{Color32, Key, Sense, Stroke, Vec2};

use crate::ui::vgui2::{VguiRuntime, VguiTextDrawOpts};

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

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        vfs: Option<&Vfs>,
        con_color: [u8; 3],
    ) -> Option<(bool, String)> {
        if !self.open {
            return None;
        }
        let mut submitted = None;
        let screen = ctx.content_rect();
        let screen_h = screen.height().round().max(480.0) as u32;
        let prompt_col = Color32::from_rgb(con_color[0], con_color[1], con_color[2]);

        egui::Area::new(egui::Id::unique("cs16_chat_input"))
            .fixed_pos(egui::pos2(24.0, screen.bottom() - 108.0))
            .show(ctx, |ui| {
                // Authentic translucent black CS 1.6 say prompt (`EngineFont`)
                egui::Frame::NONE
                    .fill(Color32::from_rgba_unmultiplied(0, 0, 0, 175))
                    .inner_margin(egui::Margin::symmetric(8, 5))
                    .stroke(Stroke::new(1.0, Color32::from_rgba_unmultiplied(188, 112, 0, 160)))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let label = if self.team_only { "say_team :" } else { "say :" };
                            let tracker_fonts = runtime.tracker_scheme.fonts.clone();
                            let measured = runtime.font_atlas.measure_text(
                                ctx,
                                vfs,
                                &tracker_fonts,
                                "tracker",
                                "EngineFont",
                                screen_h,
                                label,
                            );
                            let (lbl_rect, _) = ui.allocate_exact_size(
                                Vec2::new((measured.x + 6.0).max(48.0), measured.y.max(18.0)),
                                Sense::hover(),
                            );
                            runtime.font_atlas.paint_text(
                                ui.painter(),
                                ctx,
                                vfs,
                                &tracker_fonts,
                                "tracker",
                                "EngineFont",
                                screen_h,
                                lbl_rect,
                                label,
                                prompt_col,
                                VguiTextDrawOpts {
                                    align: TextAlignment::West,
                                    process_hotkey: false,
                                    proportional_scale: false,
                                },
                            );
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
