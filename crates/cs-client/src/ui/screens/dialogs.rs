//! VGUI2 modal dialogs:
//! - `LoadingDialog` (`Resource/LoadingDialog.res` + `VACBannedConnRefusedDialog.res`)
//! - `PlayerListDialog` (`Resource/PlayerListDialog.res`, Mute Voice -> `vban <mask>`)
//! - `DemoPlayerDialog` (`Resource/DemoPlayerDialog.res` / `DemoPlayerFileDialog.res`)
//! - `ServerPasswordUi` (`Servers/DialogServerPassword.res`)
//! - `DisconnectErrorDialog`

use std::collections::BTreeMap;

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::TextAlignment;
use cs_vfs::Vfs;
use egui::{Color32, CornerRadius, Key, Pos2, Rect, RichText, Sense, TextureHandle, Vec2};

use crate::ui::vgui2::controls::{
    ListColumn, VguiListPanelState, draw_vgui_progress_bar, draw_vgui_text_entry,
};
use crate::ui::vgui2::{
    TrackerButtonCfg, TrackerCtx, VguiRuntime, draw_tracker_button_rect,
    draw_tracker_close_button, draw_tracker_list_panel_rect, paint_tracker_bevel_rect,
    paint_tracker_label_rect,
};
use crate::ui::vgui_assets::{TrackerWindowCfg, show_tracker_window};
use crate::ui::vgui_theme::{VguiPalette, paint_vgui_bevel};

/// Render context for modal `TrackerScheme.res` GameUI dialogs.
pub struct DialogRenderCtx<'a> {
    pub vfs: Option<&'a Vfs>,
    pub loc: Option<&'a LocalizationTable>,
    pub palette: VguiPalette,
    pub steam_icon: Option<&'a TextureHandle>,
}

/// Connecting/map-loading progress bar + secondary download bar + Cancel button.
#[derive(Debug, Clone, Default)]
pub struct LoadingDialogUi {
    pub visible: bool,
    pub status_text: String,
    pub progress: f32,
    pub secondary_progress: Option<(String, f32)>,
    pub vac_secure: bool,
}

impl LoadingDialogUi {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        d: &DialogRenderCtx<'_>,
    ) -> bool {
        if !self.visible {
            return false;
        }
        if let Some(v) = d.vfs {
            runtime.load_res(v, "resource/LoadingDialog.res");
        }
        let win_sz = runtime.doc_window_size("resource/LoadingDialog.res", [380.0, 128.0]);
        let cancel_lbl = runtime
            .doc_label("resource/LoadingDialog.res", "CancelButton", d.loc)
            .unwrap_or_else(|| "Cancel".to_string());
        let mut cancel_clicked = false;
        let mut open = self.visible;
        let title = if self.vac_secure {
            "Connecting (VAC Secure)..."
        } else {
            "Loading..."
        };
        let status = d
            .loc
            .map_or(self.status_text.as_str(), |l| l.resolve(&self.status_text))
            .to_string();

        show_tracker_window(
            ctx,
            runtime,
            &mut open,
            &TrackerWindowCfg {
                id: "cs16_vgui_loading",
                title,
                default_pos: Pos2::new(260.0, 220.0),
                default_size: Vec2::new(win_sz[0].max(360.0), win_sz[1].max(118.0)),
                min_size: Vec2::new(340.0, 112.0),
                resizable: false,
                palette: d.palette,
                steam_icon: d.steam_icon,
                vfs: d.vfs,
            },
            |ui, rt| {
                ui.label(RichText::new(status).color(d.palette.base_text));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(260.0, 22.0), egui::Sense::hover());
                    draw_vgui_progress_bar(ui.painter(), bar_rect, self.progress, d.palette);
                    if rt
                        .draw_tracker_button(
                            ui,
                            d.vfs,
                            d.palette,
                            &TrackerButtonCfg {
                                id: "dlg_loading_cancel",
                                label: &cancel_lbl,
                                min_size: Vec2::new(72.0, 22.0),
                                default_btn: false,
                                enabled: true,
                            },
                        )
                        .clicked()
                    {
                        cancel_clicked = true;
                    }
                });
                if let Some((ref label, frac)) = self.secondary_progress {
                    ui.add_space(4.0);
                    ui.label(RichText::new(label).small().color(d.palette.dim_text));
                    let (b2, _) = ui.allocate_exact_size(Vec2::new(260.0, 16.0), egui::Sense::hover());
                    draw_vgui_progress_bar(ui.painter(), b2, frac, d.palette);
                }
            },
        );
        if !open || cancel_clicked {
            self.visible = false;
            return true;
        }
        false
    }
}

/// Player list mute dialog (`PlayerListDialog.res` / `CPlayerListDialog`, `FUN_1003a8d0`).
#[derive(Debug, Clone)]
pub struct PlayerListDialogUi {
    pub open: bool,
    pub muted_mask: u32,
    pub list: VguiListPanelState,
    slot_order: Vec<u8>,
    drag_offset: Vec2,
}

impl Default for PlayerListDialogUi {
    fn default() -> Self {
        Self {
            open: false,
            muted_mask: 0,
            list: VguiListPanelState {
                columns: vec![
                    ListColumn { name: "Player Name".into(), width: 240 },
                    ListColumn { name: "Properties".into(), width: 150 },
                ],
                ..Default::default()
            },
            slot_order: Vec::new(),
            drag_offset: Vec2::ZERO,
        }
    }
}

impl PlayerListDialogUi {
    pub fn sync_players(&mut self, names: &BTreeMap<u8, String>, local_slot: Option<u8>) {
        self.slot_order.clear();
        self.list.rows.clear();
        for (&slot, name) in names {
            if Some(slot) == local_slot {
                continue;
            }
            let muted = (slot < 32) && (self.muted_mask & (1u32 << slot)) != 0;
            self.slot_order.push(slot);
            self.list.rows.push(vec![
                name.clone(),
                if muted { "Muted".into() } else { "Active".into() },
            ]);
        }
    }

    pub fn toggle_mute_selected(&mut self) -> Option<String> {
        let row = self.list.selected_row?;
        let &slot = self.slot_order.get(row)?;
        if slot < 32 {
            self.muted_mask ^= 1u32 << slot;
            Some(self.vban_command())
        } else {
            None
        }
    }

    pub fn vban_command(&self) -> String {
        format!("vban {:x}", self.muted_mask)
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        names: &BTreeMap<u8, String>,
        local_slot: Option<u8>,
        d: &DialogRenderCtx<'_>,
    ) -> Option<String> {
        if !self.open {
            return None;
        }
        if let Some(v) = d.vfs {
            runtime.load_res(v, "resource/PlayerListDialog.res");
        }
        self.sync_players(names, local_slot);

        let loc_or = |tok: &str, fb: &str| -> String {
            if let Some(l) = d.loc {
                let r = l.resolve(tok);
                if !r.is_empty() && !r.starts_with('#') {
                    return r.to_string();
                }
            }
            fb.to_string()
        };

        let screen = ctx.content_rect();
        let s = (screen.height() / 720.0).max(1.0);
        let screen_h = screen.height().round().max(480.0) as u32;
        let tc = TrackerCtx {
            ctx,
            vfs: d.vfs,
            palette: d.palette,
            screen_h,
            scale: s,
        };

        let dlg_w = (467.0 * s).round();
        let dlg_h = (388.0 * s).round();
        let default_x = screen.left() + ((screen.width() - dlg_w) * 0.5).floor();
        let default_y = screen.top() + ((screen.height() - dlg_h) * 0.5).floor();
        let max_x = (screen.right() - dlg_w).max(screen.left());
        let max_y = (screen.bottom() - dlg_h).max(screen.top());
        let dlg_min = Pos2::new(
            (default_x + self.drag_offset.x).clamp(screen.left(), max_x).round(),
            (default_y + self.drag_offset.y).clamp(screen.top(), max_y).round(),
        );
        let dlg_rect = Rect::from_min_size(dlg_min, Vec2::new(dlg_w, dlg_h));

        let mut close_now = ctx.input(|i| i.key_pressed(Key::Escape));
        let mut out_vban = None;
        let title = loc_or("#GameUI_CurrentPlayers", "Current Players");
        let col_name = loc_or("#GameUI_PlayerName", "Player Name");
        let col_props = loc_or("#GameUI_Properties", "Properties");
        let empty_lbl = loc_or("#GameUI_NoOtherPlayersInGame", "There are no other players in the game.");
        let mute_lbl = loc_or("#GameUI_MuteIngameVoice", "Mute in-game voice");
        let close_lbl = loc_or("#GameUI_Close", "Close");

        egui::Area::new(egui::Id::unique("cs16_vgui_playerlist"))
            .order(egui::Order::Foreground)
            .fixed_pos(dlg_rect.min)
            .show(ctx, |ui| {
                ui.allocate_exact_size(dlg_rect.size(), Sense::hover());
                let p = ui.painter();
                p.rect_filled(dlg_rect, CornerRadius::ZERO, tc.palette.control_bg);
                paint_tracker_bevel_rect(p, dlg_rect, true, tc.palette);

                let title_drag = Rect::from_min_size(
                    dlg_min,
                    Vec2::new((dlg_w - 28.0 * s).max(40.0), (29.0 * s).round()),
                );
                let drag_resp = ui.interact(
                    title_drag,
                    ui.make_persistent_id("plist_title_drag"),
                    Sense::drag(),
                );
                if drag_resp.dragged() {
                    self.drag_offset += drag_resp.drag_delta();
                }
                if let Some(icon) = d.steam_icon {
                    let isz = (16.0 * s).round();
                    let ir = Rect::from_min_size(
                        dlg_min + Vec2::new((7.0 * s).round(), (8.0 * s).round()),
                        Vec2::splat(isz),
                    );
                    ui.painter().image(
                        icon.id(),
                        ir,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                paint_tracker_label_rect(
                    ui.painter(),
                    runtime,
                    &tc,
                    Rect::from_min_size(
                        dlg_min + Vec2::new((28.0 * s).round(), (4.0 * s).round()),
                        Vec2::new((260.0 * s).round(), (22.0 * s).round()),
                    ),
                    &title,
                    "DefaultBold",
                    TextAlignment::West,
                    tc.palette.bright_text,
                    false,
                    true,
                );
                let close_r = Rect::from_min_size(
                    dlg_min + Vec2::new(((467.0 - 25.0) * s).round(), (8.0 * s).round()),
                    Vec2::splat((18.0 * s).round()),
                );
                if draw_tracker_close_button(ui, runtime, &tc, "plist_close", close_r) {
                    close_now = true;
                }

                let list_r = Rect::from_min_size(
                    dlg_min + Vec2::new((28.0 * s).round(), (38.0 * s).round()),
                    Vec2::new((410.0 * s).round(), (279.0 * s).round()),
                );
                let cols = [(col_name.as_str(), 240.0), (col_props.as_str(), 150.0)];
                let _ = draw_tracker_list_panel_rect(
                    ui,
                    runtime,
                    &tc,
                    "cs16_playerlist",
                    list_r,
                    &cols,
                    &self.list.rows,
                    &mut self.list.selected_row,
                );
                if self.list.rows.is_empty() {
                    let empty_r = Rect::from_min_size(
                        list_r.min + Vec2::new((10.0 * s).round(), (28.0 * s).round()),
                        Vec2::new((list_r.width() - 36.0 * s).max(40.0), (22.0 * s).round()),
                    );
                    paint_tracker_label_rect(
                        ui.painter(),
                        runtime,
                        &tc,
                        empty_r,
                        &empty_lbl,
                        "Default",
                        TextAlignment::West,
                        tc.palette.dim_text,
                        false,
                        true,
                    );
                }

                let mute_r = Rect::from_min_size(
                    dlg_min + Vec2::new((173.0 * s).round(), (326.0 * s).round()),
                    Vec2::new((144.0 * s).round(), (24.0 * s).round()),
                );
                if draw_tracker_button_rect(
                    ui,
                    runtime,
                    &tc,
                    "dlg_plist_mute",
                    mute_r,
                    &mute_lbl,
                    TextAlignment::West,
                    true,
                    self.list.selected_row.is_some(),
                )
                .clicked()
                {
                    out_vban = self.toggle_mute_selected();
                }

                let close_btn_r = Rect::from_min_size(
                    dlg_min + Vec2::new((352.0 * s).round(), (352.0 * s).round()),
                    Vec2::new((86.0 * s).round(), (24.0 * s).round()),
                );
                if draw_tracker_button_rect(
                    ui,
                    runtime,
                    &tc,
                    "dlg_plist_close",
                    close_btn_r,
                    &close_lbl,
                    TextAlignment::West,
                    false,
                    true,
                )
                .clicked()
                {
                    close_now = true;
                }
            });
        if close_now {
            self.open = false;
        }
        out_vban
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DemoTransportState {
    #[default]
    Stopped,
    Playing,
    Paused,
}

/// VGUI2 Demo Player dialog (`DemoPlayerDialog.res` / `DemoPlayerFileDialog.res`).
#[derive(Debug, Clone)]
pub struct DemoPlayerUi {
    pub open: bool,
    pub selected_demo: String,
    pub state: DemoTransportState,
    pub speed: f32,
    pub tick: u32,
    pub total_ticks: u32,
    pub available_demos: Vec<String>,
}

impl Default for DemoPlayerUi {
    fn default() -> Self {
        Self {
            open: false,
            selected_demo: String::new(),
            state: DemoTransportState::Stopped,
            speed: 1.0,
            tick: 0,
            total_ticks: 1000,
            available_demos: Vec::new(),
        }
    }
}

impl DemoPlayerUi {
    pub fn load_demo(&mut self, path: &str) {
        self.selected_demo = path.to_string();
        self.tick = 0;
        self.state = DemoTransportState::Playing;
    }

    pub fn step_command(&mut self, action: &str) {
        match action {
            "start" => self.tick = 0,
            "slower" => self.speed = (self.speed * 0.5).max(0.25),
            "faster" => self.speed = (self.speed * 2.0).min(8.0),
            "stepb" => self.tick = self.tick.saturating_sub(50),
            "stepf" => self.tick = (self.tick + 50).min(self.total_ticks),
            "pause" => self.state = DemoTransportState::Paused,
            "play" => self.state = DemoTransportState::Playing,
            "end" => {
                self.tick = self.total_ticks;
                self.state = DemoTransportState::Paused;
            }
            "stop" => {
                self.tick = 0;
                self.state = DemoTransportState::Stopped;
            }
            _ => {}
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        d: &DialogRenderCtx<'_>,
    ) {
        if !self.open {
            return;
        }
        if let Some(v) = d.vfs {
            runtime.load_res(v, "resource/DemoPlayerDialog.res");
            runtime.load_res(v, "resource/DemoPlayerFileDialog.res");
        }
        let win_sz = runtime.doc_window_size("resource/DemoPlayerDialog.res", [380.0, 150.0]);
        let mut still_open = self.open;
        show_tracker_window(
            ctx,
            runtime,
            &mut still_open,
            &TrackerWindowCfg {
                id: "cs16_vgui_demoplayer",
                title: "Demo Player",
                default_pos: Pos2::new(200.0, 340.0),
                default_size: Vec2::new(win_sz[0].max(360.0), win_sz[1].max(145.0)),
                min_size: Vec2::new(340.0, 140.0),
                resizable: false,
                palette: d.palette,
                steam_icon: d.steam_icon,
                vfs: d.vfs,
            },
            |ui, rt| {
                ui.horizontal(|ui| {
                    ui.label("Demo:");
                    ui.text_edit_singleline(&mut self.selected_demo);
                });
                ui.add(egui::Slider::new(&mut self.tick, 0..=self.total_ticks.max(1)).text("Tick"));
                ui.horizontal(|ui| {
                    for (lbl, act, icon_path) in [
                        ("|<", "start", "resource/icon_start.tga"),
                        ("<<", "slower", "resource/icon_slower.tga"),
                        ("<", "stepb", "resource/icon_stepb.tga"),
                        ("||", "pause", "resource/icon_pause.tga"),
                        (">", "play", "resource/icon_play.tga"),
                        (">", "stepf", "resource/icon_stepf.tga"),
                        (">>", "faster", "resource/icon_faster.tga"),
                        (">|", "end", "resource/icon_end.tga"),
                        ("[]", "stop", "resource/icon_stop.tga"),
                    ] {
                        let tex = rt.ensure_texture(ctx, d.vfs, icon_path);
                        let (btn_r, resp) =
                            ui.allocate_exact_size(Vec2::new(24.0, 22.0), Sense::click());
                        rt.track_interactive(&format!("demo_{act}"), &resp);
                        let down = resp.is_pointer_button_down_on();
                        let bg = if down {
                            d.palette.window_bg
                        } else if resp.hovered() {
                            d.palette.control_dark_bg
                        } else {
                            d.palette.control_bg
                        };
                        ui.painter().rect_filled(btn_r, CornerRadius::ZERO, bg);
                        paint_vgui_bevel(ui.painter(), btn_r, !down, d.palette);
                        if let Some(t) = tex {
                            ui.painter().image(
                                t.id(),
                                btn_r.shrink(3.0),
                                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        } else {
                            ui.painter().text(
                                btn_r.center(),
                                egui::Align2::CENTER_CENTER,
                                lbl,
                                egui::FontId::proportional(11.0),
                                d.palette.base_text,
                            );
                        }
                        if resp.clicked() {
                            self.step_command(act);
                        }
                    }
                    ui.label(format!("{:.2}x", self.speed));
                });
            },
        );
        if !still_open {
            self.open = false;
        }
    }
}

/// `DialogServerPassword.res` prompt.
#[derive(Debug, Clone, Default)]
pub struct ServerPasswordUi {
    pub open: bool,
    pub target_addr: String,
    pub password: String,
}

impl ServerPasswordUi {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        d: &DialogRenderCtx<'_>,
    ) -> Option<(String, String)> {
        if !self.open {
            return None;
        }
        if let Some(v) = d.vfs {
            runtime.load_res(v, "servers/DialogServerPassword.res");
        }
        let mut open = self.open;
        let mut submit = None;
        let mut close_now = false;
        let enter_pressed = ctx.input(|i| i.key_pressed(Key::Enter));
        let esc_pressed = ctx.input(|i| i.key_pressed(Key::Escape));

        show_tracker_window(
            ctx,
            runtime,
            &mut open,
            &TrackerWindowCfg {
                id: "cs16_vgui_server_password",
                title: "Server Password",
                default_pos: Pos2::new(260.0, 210.0),
                default_size: Vec2::new(330.0, 145.0),
                min_size: Vec2::new(300.0, 130.0),
                resizable: false,
                palette: d.palette,
                steam_icon: d.steam_icon,
                vfs: d.vfs,
            },
            |ui, rt| {
                ui.label(format!("Enter password for {}:", self.target_addr));
                draw_vgui_text_entry(ui, &mut self.password, true, false, 280.0);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if rt
                        .draw_tracker_button(
                            ui,
                            d.vfs,
                            d.palette,
                            &TrackerButtonCfg {
                                id: "dlg_pw_connect",
                                label: "Connect",
                                min_size: Vec2::new(78.0, 24.0),
                                default_btn: true,
                                enabled: true,
                            },
                        )
                        .clicked()
                        || enter_pressed
                    {
                        submit = Some((self.target_addr.clone(), self.password.clone()));
                        close_now = true;
                    }
                    if rt
                        .draw_tracker_button(
                            ui,
                            d.vfs,
                            d.palette,
                            &TrackerButtonCfg {
                                id: "dlg_pw_cancel",
                                label: "Cancel",
                                min_size: Vec2::new(72.0, 24.0),
                                default_btn: false,
                                enabled: true,
                            },
                        )
                        .clicked()
                    {
                        close_now = true;
                    }
                });
            },
        );
        if !open || close_now || esc_pressed {
            self.open = false;
        }
        submit
    }
}

/// Modal disconnect / VAC / connection error box.
#[derive(Debug, Clone, Default)]
pub struct DisconnectErrorDialog {
    pub open: bool,
    pub title: String,
    pub message: String,
}

impl DisconnectErrorDialog {
    pub fn trigger(&mut self, title: impl Into<String>, message: impl Into<String>) {
        self.title = title.into();
        self.message = message.into();
        self.open = true;
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        d: &DialogRenderCtx<'_>,
    ) {
        if !self.open {
            return;
        }
        if let Some(v) = d.vfs {
            runtime.load_res(v, "resource/LoadingDialogError.res");
        }
        let mut still_open = self.open;
        let mut close = ctx.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape));
        show_tracker_window(
            ctx,
            runtime,
            &mut still_open,
            &TrackerWindowCfg {
                id: "cs16_vgui_disconnect_err",
                title: if self.title.is_empty() { "Disconnected" } else { &self.title },
                default_pos: Pos2::new(250.0, 210.0),
                default_size: Vec2::new(370.0, 145.0),
                min_size: Vec2::new(300.0, 120.0),
                resizable: false,
                palette: d.palette,
                steam_icon: d.steam_icon,
                vfs: d.vfs,
            },
            |ui, rt| {
                ui.label(RichText::new(&self.message).color(d.palette.bright_control_text));
                ui.add_space(10.0);
                if rt
                    .draw_tracker_button(
                        ui,
                        d.vfs,
                        d.palette,
                        &TrackerButtonCfg {
                            id: "dlg_err_ok",
                            label: "OK",
                            min_size: Vec2::new(72.0, 24.0),
                            default_btn: true,
                            enabled: true,
                        },
                    )
                    .clicked()
                {
                    close = true;
                }
            },
        );
        if !still_open || close {
            self.open = false;
        }
    }
}
