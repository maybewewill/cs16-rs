//! Authentic Counter-Strike 1.6 VGUI2 Developer Console (`~`).

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::TextAlignment;
use cs_cvar::{CVarRegistry, EngineAction, LogLevel};
use cs_vfs::Vfs;
use egui::{Color32, FontId, Key, Pos2, Rect, ScrollArea, Sense, Stroke, TextEdit, TextureHandle, Vec2};

use crate::ui::vgui2::VguiRuntime;
use crate::ui::vgui_theme::{VguiPalette, paint_vgui_bevel};

/// Render context for [`ConsoleUi`].
pub struct ConsoleRenderCtx<'a> {
    pub vfs: Option<&'a Vfs>,
    pub loc: Option<&'a LocalizationTable>,
    pub available_maps: &'a [String],
    pub palette: VguiPalette,
    pub steam_icon: Option<&'a TextureHandle>,
}

/// Default Steam 2003 `CConsoleDialog` bounds `(40, 40, 560, 400) * max(1.0, H / 720.0)`.
/// Measured across 800x600 `(40,40) 560x400`, 1280x720 `(40,40) 560x400`,
/// 1280x800 `(44,44) 622x444`, and 1280x1024 `(57,57) 796x569`.
pub fn console_default_rect(screen: Rect) -> Rect {
    let s = (screen.height() / 720.0).max(1.0);
    Rect::from_min_size(
        Pos2::new(
            screen.left() + (40.0 * s).round(),
            screen.top() + (40.0 * s).round(),
        ),
        Vec2::new((560.0 * s).round(), (400.0 * s).round()),
    )
}

#[derive(Default)]
pub struct ConsoleUi {
    pub open: bool,
    input: String,
    history_cursor: Option<usize>,
    request_focus: bool,
    scroll_to_bottom: bool,
    last_log_len: usize,
    drag_offset: Vec2,
    size_delta: Vec2,
    scroll_y: f32,
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

        let screen = ctx.content_rect();
        let screen_h = screen.height().round().max(480.0) as u32;
        let s = (screen.height() / 720.0).max(1.0);
        let base_rect = console_default_rect(screen);
        let w = (base_rect.width() + self.size_delta.x).clamp(360.0 * s, (screen.width() - 16.0).max(360.0));
        let h = (base_rect.height() + self.size_delta.y).clamp(220.0 * s, (screen.height() - 16.0).max(220.0));
        let min_x = (base_rect.left() + self.drag_offset.x).clamp(screen.left(), (screen.right() - w).max(screen.left()));
        let min_y = (base_rect.top() + self.drag_offset.y).clamp(screen.top(), (screen.bottom() - h).max(screen.top()));
        let win_rect = Rect::from_min_size(Pos2::new(min_x, min_y), Vec2::new(w, h));

        let palette = c.palette;
        let available_maps = c.available_maps;
        let title = c
            .loc
            .map(|l| l.resolve("#GameUI_Console"))
            .filter(|s| !s.is_empty() && !s.starts_with('#'))
            .unwrap_or("Console")
            .to_string();
        let tracker_fonts = runtime.tracker_scheme.fonts.clone();
        let mut close_clicked = false;

        egui::Area::new(egui::Id::unique("cs16_vgui_console"))
            .order(egui::Order::Foreground)
            .fixed_pos(win_rect.min)
            .show(ctx, |ui| {
                ui.set_min_size(win_rect.size());
                let painter = ui.painter();

                painter.rect_filled(win_rect, 0.0, palette.control_bg);
                paint_vgui_bevel(painter, win_rect, true, palette);

                // Titlebar strip (0..35*s): Steam icon, "Console" (DefaultBold), [X] close button.
                let title_h = (35.0 * s).round();
                let icon_sz = (16.0 * s).round();
                let icon_rect = Rect::from_min_size(
                    Pos2::new(win_rect.left() + (8.0 * s).round(), win_rect.top() + (8.0 * s).round()),
                    Vec2::splat(icon_sz),
                );
                if let Some(icon) = c.steam_icon {
                    painter.image(
                        icon.id(),
                        icon_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }

                let title_rect = Rect::from_min_size(
                    Pos2::new(win_rect.left() + (28.0 * s).round(), win_rect.top() + (8.0 * s).round()),
                    Vec2::new((w - 64.0 * s).max(80.0), (18.0 * s).round()),
                );
                runtime.font_atlas.paint_text_with_scale(
                    painter,
                    ctx,
                    c.vfs,
                    &tracker_fonts,
                    "tracker",
                    "DefaultBold",
                    screen_h,
                    title_rect,
                    &title,
                    palette.bright_text,
                    TextAlignment::West,
                    false,
                    s,
                );

                let close_sz = (18.0 * s).round();
                let close_rect = Rect::from_min_size(
                    Pos2::new(win_rect.right() - (25.0 * s).round(), win_rect.top() + (8.0 * s).round()),
                    Vec2::splat(close_sz),
                );
                let drag_rect = Rect::from_min_max(
                    win_rect.min,
                    Pos2::new(close_rect.left() - 4.0, win_rect.top() + title_h),
                );
                let drag_resp = ui.interact(drag_rect, ui.make_persistent_id("cs16_con_drag"), Sense::drag());
                if drag_resp.dragged() {
                    self.drag_offset += drag_resp.drag_delta();
                }

                let close_resp = ui.interact(close_rect, ui.make_persistent_id("cs16_con_close"), Sense::click());
                runtime.track_interactive("cs16_vgui_console_close", &close_resp);
                let close_down = close_resp.is_pointer_button_down_on();
                let close_bg = if close_down {
                    palette.window_bg
                } else if close_resp.hovered() {
                    palette.control_dark_bg
                } else {
                    palette.control_bg
                };
                ui.painter().rect_filled(close_rect, 0.0, close_bg);
                paint_vgui_bevel(ui.painter(), close_rect, !close_down, palette);
                let x_col = if close_resp.hovered() { palette.bright_text } else { palette.base_text };
                let cc = close_rect.center();
                let x_half = (3.5 * s).round().max(3.0);
                let xs = Stroke::new((1.5 * s).clamp(1.2, 2.5), x_col);
                ui.painter().line_segment([cc + Vec2::new(-x_half, -x_half), cc + Vec2::new(x_half, x_half)], xs);
                ui.painter().line_segment([cc + Vec2::new(x_half, -x_half), cc + Vec2::new(-x_half, x_half)], xs);
                if close_resp.clicked() {
                    close_clicked = true;
                }

                // Recessed log viewport + right VGUI2 scrollbar (`7*s .. w - 7*s`, `35*s .. h - 38*s`)
                let margin_x = (7.0 * s).round();
                let bot_area_h = (38.0 * s).round();
                let log_rect = Rect::from_min_max(
                    Pos2::new(win_rect.left() + margin_x, win_rect.top() + title_h),
                    Pos2::new(win_rect.right() - margin_x, win_rect.bottom() - bot_area_h),
                );
                ui.painter().rect_filled(log_rect, 0.0, palette.window_bg);
                paint_vgui_bevel(ui.painter(), log_rect, false, palette);

                let sb_w = (18.0 * s).round().max(14.0);
                let sb_rect = Rect::from_min_max(
                    Pos2::new(log_rect.right() - sb_w - 1.0, log_rect.top() + 1.0),
                    Pos2::new(log_rect.right() - 1.0, log_rect.bottom() - 1.0),
                );
                ui.painter().rect_filled(sb_rect, 0.0, Color32::from_rgb(88, 102, 78));
                let up_btn = Rect::from_min_size(sb_rect.min, Vec2::splat(sb_w));
                let dn_btn = Rect::from_min_size(
                    Pos2::new(sb_rect.left(), sb_rect.bottom() - sb_w),
                    Vec2::splat(sb_w),
                );
                if draw_tracker_sb_arrow(ui, up_btn, palette, true) {
                    self.scroll_y = (self.scroll_y - 32.0 * s).max(0.0);
                    self.scroll_to_bottom = false;
                }
                if draw_tracker_sb_arrow(ui, dn_btn, palette, false) {
                    self.scroll_y += 32.0 * s;
                    self.scroll_to_bottom = false;
                }

                let inner_log = Rect::from_min_max(
                    Pos2::new(log_rect.left() + (6.0 * s).round(), log_rect.top() + (4.0 * s).round()),
                    Pos2::new(sb_rect.left() - (4.0 * s).round(), log_rect.bottom() - (4.0 * s).round()),
                );
                let line_h = runtime
                    .font_atlas
                    .measure_text_with_scale(
                        ctx, c.vfs, &tracker_fonts, "tracker", "Default", screen_h, "Ay", s,
                    )
                    .y
                    .max((13.0 * s).round());
                let scroll_out = ui.scope_builder(egui::UiBuilder::new().max_rect(inner_log), |ui| {
                    let mut sa = ScrollArea::vertical()
                        .id_salt("cs16_console_scroll")
                        .max_height(inner_log.height())
                        .auto_shrink([false, false])
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
                    sa = if self.scroll_to_bottom {
                        sa.stick_to_bottom(true)
                    } else {
                        sa.vertical_scroll_offset(self.scroll_y)
                    };
                    sa.show(ui, |ui| {
                        let lines = cvars.log_lines();
                        let total_h = (lines.len() as f32 * line_h).max(inner_log.height());
                        let (alloc, _) = ui.allocate_exact_size(
                            Vec2::new(inner_log.width(), total_h),
                            Sense::hover(),
                        );
                        let clipped = ui.painter().with_clip_rect(inner_log);
                        for (idx, line) in lines.iter().enumerate() {
                            let y = (alloc.top() + idx as f32 * line_h).round();
                            if y + line_h < inner_log.top() || y > inner_log.bottom() {
                                continue;
                            }
                            let color = match line.level {
                                LogLevel::Info => palette.base_text,
                                LogLevel::Command => palette.bright_text,
                                LogLevel::Warning => Color32::from_rgb(235, 190, 95),
                                LogLevel::Error => Color32::from_rgb(235, 105, 95),
                            };
                            let lr = Rect::from_min_size(
                                Pos2::new(alloc.left(), y),
                                Vec2::new(inner_log.width(), line_h + 2.0),
                            );
                            runtime.font_atlas.paint_text_with_scale(
                                &clipped,
                                ctx,
                                c.vfs,
                                &tracker_fonts,
                                "tracker",
                                "Default",
                                screen_h,
                                lr,
                                &line.text,
                                color,
                                TextAlignment::NorthWest,
                                false,
                                s,
                            );
                        }
                    })
                });
                self.scroll_y = scroll_out.inner.state.clamped_offset().y;
                self.scroll_to_bottom = false;

                // Bottom TextEntry (`7*s .. w - 85*s`, `h - 30*s .. h - 8*s`) + Submit button
                let ctrl_top = win_rect.bottom() - (30.0 * s).round();
                let ctrl_h = (22.0 * s).round();
                let submit_w = (71.0 * s).round();
                let submit_rect = Rect::from_min_size(
                    Pos2::new(win_rect.right() - margin_x - submit_w, ctrl_top),
                    Vec2::new(submit_w, ctrl_h),
                );
                let entry_rect = Rect::from_min_max(
                    Pos2::new(win_rect.left() + margin_x, ctrl_top),
                    Pos2::new(submit_rect.left() - (7.0 * s).round(), ctrl_top + ctrl_h),
                );

                ui.painter().rect_filled(entry_rect, 0.0, palette.window_bg);
                paint_vgui_bevel(ui.painter(), entry_rect, false, palette);

                let inner_entry = entry_rect.shrink2(Vec2::new((5.0 * s).round(), (2.0 * s).round()));
                let edit_resp = ui
                    .scope_builder(egui::UiBuilder::new().max_rect(inner_entry), |ui| {
                        let edit = TextEdit::singleline(&mut self.input)
                            .id_salt("cs16_console_input")
                            .frame(egui::Frame::NONE)
                            .font(FontId::proportional((12.0 * s).round()))
                            .text_color(palette.base_text)
                            .desired_width(inner_entry.width())
                            .lock_focus(true);
                        ui.add(edit)
                    })
                    .inner;

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

                let enter_pressed = edit_resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let submit_resp = ui.interact(
                    submit_rect,
                    ui.make_persistent_id("console_submit_btn"),
                    Sense::click(),
                );
                runtime.track_interactive("console_submit_btn", &submit_resp);
                let sub_down = submit_resp.is_pointer_button_down_on();
                let sub_bg = if sub_down {
                    palette.window_bg
                } else if submit_resp.hovered() {
                    palette.control_dark_bg
                } else {
                    palette.control_bg
                };
                ui.painter().rect_filled(submit_rect, 0.0, sub_bg);
                paint_vgui_bevel(ui.painter(), submit_rect, !sub_down, palette);
                runtime.font_atlas.paint_text_with_scale(
                    ui.painter(),
                    ctx,
                    c.vfs,
                    &tracker_fonts,
                    "tracker",
                    "Default",
                    screen_h,
                    submit_rect.shrink2(Vec2::new((6.0 * s).round(), 0.0)),
                    "Submit",
                    if submit_resp.hovered() { palette.bright_control_text } else { palette.base_text },
                    TextAlignment::West,
                    false,
                    s,
                );

                if enter_pressed || submit_resp.clicked() {
                    let cmd = std::mem::take(&mut self.input);
                    self.history_cursor = None;
                    actions.extend(cvars.execute_console_line(&cmd));
                    self.scroll_to_bottom = true;
                    edit_resp.request_focus();
                }

                // Bottom-right 3x3 dotted resize grip (`w - 9*s, h - 9*s`)
                let grip_sz = (12.0 * s).round();
                let grip_rect = Rect::from_min_size(
                    Pos2::new(win_rect.right() - grip_sz, win_rect.bottom() - grip_sz),
                    Vec2::splat(grip_sz),
                );
                let grip_resp = ui.interact(grip_rect, ui.make_persistent_id("cs16_con_resize"), Sense::drag());
                if grip_resp.dragged() {
                    self.size_delta += grip_resp.drag_delta();
                }
                let gx0 = win_rect.right() - (9.0 * s).round();
                let gy0 = win_rect.bottom() - (9.0 * s).round();
                let step = (3.0 * s).round().max(2.0);
                for r in 0..3 {
                    for c_i in (2 - r)..3 {
                        let px = gx0 + c_i as f32 * step;
                        let py = gy0 + r as f32 * step;
                        ui.painter().rect_filled(
                            Rect::from_min_size(Pos2::new(px, py), Vec2::splat(1.0)),
                            0.0,
                            palette.border_bright,
                        );
                    }
                }
            });

        if close_clicked {
            self.open = false;
        }
        actions
    }
}

fn draw_tracker_sb_arrow(ui: &mut egui::Ui, rect: Rect, palette: VguiPalette, up: bool) -> bool {
    let resp = ui.interact(
        rect,
        ui.make_persistent_id(if up { "cs16_con_sb_up" } else { "cs16_con_sb_dn" }),
        Sense::click(),
    );
    let down = resp.is_pointer_button_down_on();
    ui.painter().rect_filled(
        rect,
        0.0,
        if down {
            palette.window_bg
        } else if resp.hovered() {
            palette.control_dark_bg
        } else {
            palette.control_bg
        },
    );
    paint_vgui_bevel(ui.painter(), rect, !down, palette);
    let hw = (rect.width() * 0.22).round().max(3.0);
    let hh = (rect.height() * 0.14).round().max(2.0);
    let cx = rect.center().x.round();
    let cy = rect.center().y.round();
    let pts = if up {
        vec![Pos2::new(cx, cy - hh), Pos2::new(cx - hw, cy + hh), Pos2::new(cx + hw, cy + hh)]
    } else {
        vec![Pos2::new(cx - hw, cy - hh), Pos2::new(cx + hw, cy - hh), Pos2::new(cx, cy + hh)]
    };
    ui.painter().add(egui::Shape::convex_polygon(pts, palette.bright_text, Stroke::NONE));
    resp.clicked()
}
