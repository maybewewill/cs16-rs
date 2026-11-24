//! VGUI2 modal dialogs:
//! - `LoadingDialog` (`Resource/LoadingDialog.res` + `VACBannedConnRefusedDialog.res`)
//! - `PlayerListDialog` (`Resource/PlayerListDialog.res`, Mute Voice -> `vban <mask>`)
//! - `DemoPlayerDialog` (`Resource/DemoPlayerDialog.res` / `DemoPlayerFileDialog.res`)
//! - `ServerPasswordUi` (`Servers/DialogServerPassword.res`)
//! - `DisconnectErrorDialog`

use std::collections::BTreeMap;

use cs_assets::scheme::LocalizationTable;
use cs_vfs::Vfs;
use egui::{CornerRadius, Pos2, RichText, TextureHandle, Vec2};

use crate::ui::vgui2::controls::{
    ListColumn, VguiListPanelState, draw_vgui_list_panel, draw_vgui_progress_bar,
    draw_vgui_text_entry,
};
use crate::ui::vgui2::VguiRuntime;
use crate::ui::vgui_assets::show_steam2003_window;
use crate::ui::vgui_theme::VguiPalette;

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
        vfs: Option<&Vfs>,
        loc: Option<&LocalizationTable>,
        palette: VguiPalette,
        steam_icon: Option<&TextureHandle>,
    ) -> bool {
        if !self.visible {
            return false;
        }
        if let Some(v) = vfs {
            runtime.load_res(v, "resource/LoadingDialog.res");
        }
        let mut cancel_clicked = false;
        let mut open = self.visible;
        let title = if self.vac_secure {
            "Connecting (VAC Secure)..."
        } else {
            "Loading..."
        };
        let status = loc
            .map_or(self.status_text.as_str(), |l| l.resolve(&self.status_text))
            .to_string();

        show_steam2003_window(
            ctx,
            "cs16_vgui_loading",
            title,
            &mut open,
            Pos2::new(260.0, 220.0),
            Vec2::new(380.0, 128.0),
            Vec2::new(340.0, 112.0),
            false,
            palette,
            steam_icon,
            |ui| {
                ui.label(RichText::new(status).color(palette.base_text));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(260.0, 22.0), egui::Sense::hover());
                    draw_vgui_progress_bar(ui.painter(), bar_rect, self.progress, palette);
                    if ui
                        .add(
                            egui::Button::new("Cancel")
                                .min_size(Vec2::new(72.0, 22.0))
                                .corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                    {
                        cancel_clicked = true;
                    }
                });
                if let Some((ref label, frac)) = self.secondary_progress {
                    ui.add_space(4.0);
                    ui.label(RichText::new(label).small().color(palette.dim_text));
                    let (b2, _) = ui.allocate_exact_size(Vec2::new(260.0, 16.0), egui::Sense::hover());
                    draw_vgui_progress_bar(ui.painter(), b2, frac, palette);
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

/// Player list mute dialog (`PlayerListDialog.res`): generates server `vban <hex>` voice mask.
#[derive(Debug, Clone)]
pub struct PlayerListDialogUi {
    pub open: bool,
    pub muted_mask: u32,
    pub list: VguiListPanelState,
    slot_order: Vec<u8>,
}

impl Default for PlayerListDialogUi {
    fn default() -> Self {
        Self {
            open: false,
            muted_mask: 0,
            list: VguiListPanelState {
                columns: vec![
                    ListColumn { name: "Player".into(), width: 220 },
                    ListColumn { name: "Status".into(), width: 120 },
                ],
                ..Default::default()
            },
            slot_order: Vec::new(),
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
        vfs: Option<&Vfs>,
        names: &BTreeMap<u8, String>,
        local_slot: Option<u8>,
        palette: VguiPalette,
        steam_icon: Option<&TextureHandle>,
    ) -> Option<String> {
        if !self.open {
            return None;
        }
        if let Some(v) = vfs {
            runtime.load_res(v, "resource/PlayerListDialog.res");
        }
        self.sync_players(names, local_slot);

        let mut still_open = self.open;
        let mut close_now = false;
        let mut out_vban = None;

        show_steam2003_window(
            ctx,
            "cs16_vgui_playerlist",
            "Player List",
            &mut still_open,
            Pos2::new(168.0, 75.0),
            Vec2::new(450.0, 360.0),
            Vec2::new(380.0, 280.0),
            false,
            palette,
            steam_icon,
            |ui| {
                let _ = draw_vgui_list_panel(ui, "cs16_playerlist", &mut self.list, 250.0, palette);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            self.list.selected_row.is_some(),
                            egui::Button::new("Mute in-game voice").corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                    {
                        out_vban = self.toggle_mute_selected();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new("Close").corner_radius(CornerRadius::ZERO))
                            .clicked()
                        {
                            close_now = true;
                        }
                    });
                });
            },
        );

        if !still_open || close_now {
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
        vfs: Option<&Vfs>,
        palette: VguiPalette,
        steam_icon: Option<&TextureHandle>,
    ) {
        if !self.open {
            return;
        }
        if let Some(v) = vfs {
            runtime.load_res(v, "resource/DemoPlayerDialog.res");
            runtime.load_res(v, "resource/DemoPlayerFileDialog.res");
        }
        let mut still_open = self.open;
        show_steam2003_window(
            ctx,
            "cs16_vgui_demoplayer",
            "Demo Player",
            &mut still_open,
            Pos2::new(200.0, 340.0),
            Vec2::new(380.0, 150.0),
            Vec2::new(340.0, 140.0),
            false,
            palette,
            steam_icon,
            |ui| {
                ui.horizontal(|ui| {
                    ui.label("Demo:");
                    ui.text_edit_singleline(&mut self.selected_demo);
                });
                ui.add(egui::Slider::new(&mut self.tick, 0..=self.total_ticks.max(1)).text("Tick"));
                ui.horizontal(|ui| {
                    for (lbl, act) in [
                        ("|<", "start"),
                        ("<<", "slower"),
                        ("<", "stepb"),
                        ("||", "pause"),
                        (">", "play"),
                        (">", "stepf"),
                        (">>", "faster"),
                        (">|", "end"),
                        ("[]", "stop"),
                    ] {
                        if ui
                            .add(egui::Button::new(lbl).corner_radius(CornerRadius::ZERO))
                            .clicked()
                        {
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
        palette: VguiPalette,
        steam_icon: Option<&TextureHandle>,
    ) -> Option<(String, String)> {
        if !self.open {
            return None;
        }
        let mut open = self.open;
        let mut submit = None;
        let mut close_now = false;
        show_steam2003_window(
            ctx,
            "cs16_vgui_server_password",
            "Server Password",
            &mut open,
            Pos2::new(260.0, 210.0),
            Vec2::new(320.0, 140.0),
            Vec2::new(300.0, 130.0),
            false,
            palette,
            steam_icon,
            |ui| {
                ui.label(format!("Enter password for {}:", self.target_addr));
                draw_vgui_text_entry(ui, &mut self.password, true, false, 260.0);
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Connect").clicked() {
                        submit = Some((self.target_addr.clone(), self.password.clone()));
                        close_now = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close_now = true;
                    }
                });
            },
        );
        if !open || close_now {
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
        palette: VguiPalette,
        steam_icon: Option<&TextureHandle>,
    ) {
        if !self.open {
            return;
        }
        let mut still_open = self.open;
        let mut close = false;
        show_steam2003_window(
            ctx,
            "cs16_vgui_disconnect_err",
            if self.title.is_empty() { "Disconnected" } else { &self.title },
            &mut still_open,
            Pos2::new(250.0, 210.0),
            Vec2::new(360.0, 140.0),
            Vec2::new(300.0, 120.0),
            false,
            palette,
            steam_icon,
            |ui| {
                ui.label(RichText::new(&self.message).color(palette.bright_control_text));
                ui.add_space(10.0);
                if ui.button("OK").clicked() {
                    close = true;
                }
            },
        );
        if !still_open || close {
            self.open = false;
        }
    }
}
