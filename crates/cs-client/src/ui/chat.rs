//! The say prompt (`messagemode`, `Y`/`U`): a line of text that goes to the server as `say`.

use cs_assets::vgui_res::TextAlignment;
use cs_vfs::Vfs;
use egui::{Color32, Event, Key, Pos2, Rect, Vec2};

use crate::ui::vgui2::VguiRuntime;

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

        // Swallow initial hotkey press (`y`/`u`) on the frame messagemode opened.
        if self.request_focus {
            self.request_focus = false;
        } else {
            ctx.input(|i| {
                for ev in &i.events {
                    if let Event::Text(t) = ev {
                        for ch in t.chars() {
                            if !ch.is_control() && self.buffer.len() < 120 {
                                self.buffer.push(ch);
                            }
                        }
                    }
                }
                if i.key_pressed(Key::Backspace) {
                    self.buffer.pop();
                }
                if i.key_pressed(Key::Enter) {
                    let text = self.buffer.trim().to_string();
                    if !text.is_empty() {
                        submitted = Some((self.team_only, text));
                    }
                    self.buffer.clear();
                    self.open = false;
                } else if i.key_pressed(Key::Escape) {
                    self.buffer.clear();
                    self.open = false;
                }
            });
        }

        if !self.open {
            return submitted;
        }

        // Authentic CS 1.6 `messagemode` HUD overlay at top-left `(8, 4)` with NO box/frame.
        let prefix = if self.team_only { "say_team :" } else { "say :" };
        let line = if self.buffer.is_empty() {
            prefix.to_string()
        } else {
            format!("{prefix} {}", self.buffer)
        };
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::unique("cs16_chat_input"),
        ));
        let tracker_fonts = runtime.tracker_scheme.fonts.clone();
        let pos = Pos2::new(screen.left() + 8.0, screen.top() + 4.0);
        let rect = Rect::from_min_size(pos, Vec2::new((screen.width() - 16.0).max(200.0), 20.0));
        let shadow_rect = rect.translate(Vec2::new(1.0, 1.0));
        runtime.font_atlas.paint_text_with_scale(
            &painter,
            ctx,
            vfs,
            &tracker_fonts,
            "tracker",
            "EngineFont",
            screen_h,
            shadow_rect,
            &line,
            Color32::from_rgba_unmultiplied(0, 0, 0, 220),
            TextAlignment::NorthWest,
            false,
            1.0,
        );
        runtime.font_atlas.paint_text_with_scale(
            &painter,
            ctx,
            vfs,
            &tracker_fonts,
            "tracker",
            "EngineFont",
            screen_h,
            rect,
            &line,
            prompt_col,
            TextAlignment::NorthWest,
            false,
            1.0,
        );
        submitted
    }
}
