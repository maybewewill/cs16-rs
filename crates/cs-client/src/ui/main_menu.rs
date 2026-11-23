//! Iconic CS 1.6 Main Menu (stitched `800_*_loading.tga` background + bottom-left menu items),
//! `New Game` VGUI2 dialog, and `Find Servers` VGUI2 dialog.

use std::time::Duration;

use cs_cvar::{CVarRegistry, EngineAction};
use cs_proto48::query::{ServerQueryInfo, discover_goldsrc_lan_servers};
use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Stroke, TextureHandle};

use crate::ui::vgui_assets::show_steam2003_window;
use crate::ui::vgui_theme::{VguiPalette, paint_vgui_bevel, vgui_tab};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum NewGameTab {
    #[default]
    Server,
    Game,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ServerBrowserTab {
    #[default]
    Internet,
    Favorites,
    History,
    Lan,
}

pub struct MainMenuUi {
    pub visible: bool,
    pub new_game_open: bool,
    pub find_servers_open: bool,
    new_game_tab: NewGameTab,
    server_tab: ServerBrowserTab,
    pub selected_map: String,
    connect_addr: String,
    lan_servers: Vec<ServerQueryInfo>,
}

impl Default for MainMenuUi {
    fn default() -> Self {
        Self {
            visible: true,
            new_game_open: false,
            find_servers_open: false,
            new_game_tab: NewGameTab::Server,
            server_tab: ServerBrowserTab::Lan,
            selected_map: "de_dust2".to_string(),
            connect_addr: "127.0.0.1:27015".to_string(),
            lan_servers: Vec::new(),
        }
    }
}

pub struct MenuCommands {
    pub actions: Vec<EngineAction>,
    pub open_options: bool,
    pub resume_game: bool,
}

impl MainMenuUi {
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        cvars: &mut CVarRegistry,
        available_maps: &[String],
        in_game: bool,
        bg_texture: Option<&TextureHandle>,
        steam_icon: Option<&TextureHandle>,
        palette: VguiPalette,
    ) -> MenuCommands {
        let mut out = MenuCommands {
            actions: Vec::new(),
            open_options: false,
            resume_game: false,
        };

        if !self.visible {
            return out;
        }

        // Ensure selected map exists in the list if possible
        if !available_maps.is_empty() && !available_maps.iter().any(|m| m == &self.selected_map) {
            if available_maps.iter().any(|m| m == "de_dust2") {
                self.selected_map = "de_dust2".to_string();
            } else {
                self.selected_map = available_maps[0].clone();
            }
        }

        let screen = ctx.content_rect();

        // 1. Background (full iconic CS 1.6 artwork when on main menu, subtle dim tint when paused over map)
        egui::Area::new(egui::Id::unique("cs16_menu_bg"))
            .order(egui::Order::Background)
            .fixed_pos(screen.min)
            .interactable(false)
            .show(ctx, |ui| {
                let painter = ui.painter();
                if !in_game {
                    if let Some(tex) = bg_texture {
                        // Scale-to-cover while keeping authentic 4:3 composition centered
                        let tex_aspect = 800.0 / 600.0;
                        let win_aspect = screen.width() / screen.height().max(1.0);
                        let uv = if win_aspect > tex_aspect {
                            let v_span = tex_aspect / win_aspect;
                            let v_pad = (1.0 - v_span) * 0.5;
                            Rect::from_min_max(Pos2::new(0.0, v_pad), Pos2::new(1.0, 1.0 - v_pad))
                        } else {
                            let u_span = win_aspect / tex_aspect;
                            let u_pad = (1.0 - u_span) * 0.5;
                            Rect::from_min_max(Pos2::new(u_pad, 0.0), Pos2::new(1.0 - u_pad, 1.0))
                        };
                        painter.image(tex.id(), screen, uv, Color32::WHITE);
                    } else {
                        painter.rect_filled(screen, 0.0, Color32::from_rgb(28, 32, 24));
                    }
                } else {
                    painter.rect_filled(screen, 0.0, Color32::from_black_alpha(95));
                }
            });

        // 2. Classic bottom-left transparent CS 1.6 menu column (`GameMenu.res` 1:1)
        let menu_x = (screen.width() * 0.042).clamp(24.0, 56.0);
        let menu_y = (screen.height() - if in_game { 255.0 } else { 195.0 }).max(100.0);
        egui::Area::new(egui::Id::unique("cs16_main_menu_items"))
            .order(egui::Order::Middle)
            .fixed_pos(Pos2::new(menu_x, menu_y))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;

                if in_game {
                    if menu_link(ui, "Resume Game", palette) {
                        out.resume_game = true;
                    }
                    if menu_link(ui, "Disconnect", palette) {
                        out.actions.push(EngineAction::Disconnect);
                    }
                    ui.add_space(6.0);
                }

                if menu_link(ui, "New Game", palette) {
                    self.new_game_open = true;
                }
                if menu_link(ui, "Find Servers", palette) {
                    self.find_servers_open = true;
                }
                if menu_link(ui, "Options", palette) {
                    out.open_options = true;
                }
                ui.add_space(6.0);
                if menu_link(ui, "Quit", palette) {
                    out.actions.push(EngineAction::Quit);
                }
            });

        // 3. `New Game` Steam 2003 VGUI2 modal window
        if self.new_game_open {
            let mut open = self.new_game_open;
            let mut close_now = false;
            show_steam2003_window(
                ctx,
                "cs16_vgui_new_game",
                "New Game",
                &mut open,
                Pos2::new(190.0, 120.0),
                egui::vec2(440.0, 320.0),
                egui::vec2(420.0, 300.0),
                false,
                palette,
                steam_icon,
                |ui| {
                    ui.horizontal(|ui| {
                        if vgui_tab(ui, "Server", self.new_game_tab == NewGameTab::Server, palette) {
                            self.new_game_tab = NewGameTab::Server;
                        }
                        if vgui_tab(ui, "Game", self.new_game_tab == NewGameTab::Game, palette) {
                            self.new_game_tab = NewGameTab::Game;
                        }
                    });

                    let panel_resp = egui::Frame::NONE
                        .fill(palette.control_bg)
                        .inner_margin(egui::Margin::same(10))
                        .stroke(Stroke::new(1.0, palette.border_dark))
                        .show(ui, |ui| {
                            ui.set_min_size(egui::vec2(410.0, 215.0));
                            match self.new_game_tab {
                                NewGameTab::Server => {
                                    ui.horizontal(|ui| {
                                        ui.label("Map:");
                                        egui::ComboBox::from_id_salt("ng_map_combo")
                                            .width(260.0)
                                            .selected_text(&self.selected_map)
                                            .show_ui(ui, |ui| {
                                                for m in available_maps {
                                                    ui.selectable_value(&mut self.selected_map, m.clone(), m);
                                                }
                                            });
                                    });

                                    ui.add_space(10.0);
                                    ui.label(RichText::new("Quick pick classic maps:").color(palette.dim_text));
                                    ui.horizontal_wrapped(|ui| {
                                        for classic in [
                                            "de_dust2",
                                            "de_inferno",
                                            "de_nuke",
                                            "de_aztec",
                                            "de_train",
                                            "cs_assault",
                                            "cs_office",
                                            "cs_italy",
                                        ] {
                                            if available_maps.iter().any(|m| m == classic) {
                                                let active = self.selected_map == classic;
                                                let txt = RichText::new(classic).color(if active {
                                                    palette.bright_control_text
                                                } else {
                                                    palette.base_text
                                                });
                                                if ui
                                                    .add(egui::Button::new(txt).corner_radius(CornerRadius::ZERO))
                                                    .clicked()
                                                {
                                                    self.selected_map = classic.to_string();
                                                }
                                            }
                                        }
                                    });

                                    ui.add_space(10.0);
                                    ui.label(
                                        RichText::new(format!("Discovered {} maps in VFS.", available_maps.len()))
                                            .small()
                                            .color(palette.dim_text),
                                    );
                                }
                                NewGameTab::Game => {
                                    cvar_edit_row(ui, cvars, "hostname", "Server name:");
                                    cvar_edit_row(ui, cvars, "bot_quota", "Bots count (0..15):");
                                    cvar_edit_row(ui, cvars, "bot_difficulty", "Bot difficulty (0-3):");
                                    cvar_edit_row(ui, cvars, "mp_roundtime", "Time per round (min):");
                                    cvar_edit_row(ui, cvars, "mp_freezetime", "Freeze time (sec):");
                                    cvar_edit_row(ui, cvars, "mp_startmoney", "Starting money ($):");
                                    cvar_edit_row(ui, cvars, "sv_gravity", "Gravity (sv_gravity):");
                                    cvar_edit_row(ui, cvars, "sv_airaccelerate", "Air accelerate:");
                                }
                            }
                        });
                    paint_vgui_bevel(ui.painter(), panel_resp.response.rect, false, palette);

                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new("Cancel").min_size(egui::vec2(76.0, 24.0)).corner_radius(CornerRadius::ZERO))
                            .clicked()
                        {
                            close_now = true;
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Start").color(palette.bright_control_text),
                                )
                                .min_size(egui::vec2(76.0, 24.0))
                                .corner_radius(CornerRadius::ZERO),
                            )
                            .clicked()
                        {
                            out.actions
                                .push(EngineAction::LoadMap(self.selected_map.clone()));
                            close_now = true;
                        }
                    });
                },
            );
            if !open || close_now {
                self.new_game_open = false;
            }
        }

        // 4. `Find Servers` Steam 2003 VGUI2 window
        if self.find_servers_open {
            let mut open = self.find_servers_open;
            show_steam2003_window(
                ctx,
                "cs16_vgui_servers",
                "Servers",
                &mut open,
                Pos2::new(150.0, 110.0),
                egui::vec2(520.0, 320.0),
                egui::vec2(460.0, 280.0),
                true,
                palette,
                steam_icon,
                |ui| {
                    ui.horizontal(|ui| {
                        for (t, name) in [
                            (ServerBrowserTab::Internet, "Internet"),
                            (ServerBrowserTab::Favorites, "Favorites"),
                            (ServerBrowserTab::History, "History"),
                            (ServerBrowserTab::Lan, "LAN"),
                        ] {
                            if vgui_tab(ui, name, self.server_tab == t, palette) {
                                self.server_tab = t;
                            }
                        }
                    });

                    let list_resp = egui::Frame::NONE
                        .fill(palette.window_bg)
                        .inner_margin(egui::Margin::same(8))
                        .stroke(Stroke::new(1.0, palette.border_dark))
                        .show(ui, |ui| {
                            ui.set_min_size(egui::vec2(490.0, 200.0));
                            if self.lan_servers.is_empty() {
                                ui.label(
                                    RichText::new("No LAN servers discovered yet. Click \"Refresh LAN\" or start one:")
                                        .color(palette.bright_control_text),
                                );
                                ui.label(
                                    RichText::new("  cs-server --map de_dust2 --port 27015 --bots 9")
                                        .monospace()
                                        .color(palette.base_text),
                                );
                            } else {
                                egui::Grid::new("cs16_lan_grid")
                                    .num_columns(5)
                                    .min_col_width(64.0)
                                    .spacing([12.0, 4.0])
                                    .show(ui, |ui| {
                                        ui.label(RichText::new("Server").color(palette.bright_control_text));
                                        ui.label(RichText::new("Map").color(palette.bright_control_text));
                                        ui.label(RichText::new("Players").color(palette.bright_control_text));
                                        ui.label(RichText::new("Latency").color(palette.bright_control_text));
                                        ui.label("");
                                        ui.end_row();
                                        for srv in &self.lan_servers {
                                            ui.label(&srv.name);
                                            ui.label(&srv.map);
                                            ui.label(format!("{}/{}", srv.players, srv.max_players));
                                            ui.label(format!("{} ms", srv.ping_ms));
                                            if ui
                                                .add(egui::Button::new("Join").corner_radius(CornerRadius::ZERO))
                                                .clicked()
                                            {
                                                out.actions.push(EngineAction::Connect(srv.addr.clone()));
                                            }
                                            ui.end_row();
                                        }
                                    });
                            }
                        });
                    paint_vgui_bevel(ui.painter(), list_resp.response.rect, false, palette);

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label("Direct IP:");
                        ui.text_edit_singleline(&mut self.connect_addr);
                        if ui
                            .add(egui::Button::new("Connect").corner_radius(CornerRadius::ZERO))
                            .clicked()
                        {
                            out.actions
                                .push(EngineAction::Connect(self.connect_addr.clone()));
                        }
                        if ui
                            .add(egui::Button::new("Refresh LAN").corner_radius(CornerRadius::ZERO))
                            .clicked()
                        {
                            self.lan_servers = discover_goldsrc_lan_servers(Duration::from_millis(300));
                        }
                    });
                },
            );
            if !open {
                self.find_servers_open = false;
            }
        }

        out
    }
}

/// Classic CS 1.6 borderless menu link in the lower-left corner.
fn menu_link(ui: &mut egui::Ui, label: &str, palette: VguiPalette) -> bool {
    let font = FontId::proportional(20.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), palette.base_text);
    let desired = egui::vec2(galley.size().x + 12.0, galley.size().y + 4.0);
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());

    let color = if response.hovered() {
        palette.bright_control_text
    } else {
        palette.bright_text
    };

    // Crisp 1px drop shadow like original GoldSrc bitmap menu text
    let pos = Pos2::new(rect.min.x + 2.0, rect.min.y + 2.0);
    ui.painter().text(
        pos + egui::vec2(1.0, 1.0),
        egui::Align2::LEFT_TOP,
        label,
        font.clone(),
        Color32::from_black_alpha(200),
    );
    ui.painter()
        .text(pos, egui::Align2::LEFT_TOP, label, font, color);

    response.clicked()
}

fn cvar_edit_row(ui: &mut egui::Ui, cvars: &mut CVarRegistry, key: &str, label: &str) {
    let mut val = cvars.get_str(key).to_string();
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.text_edit_singleline(&mut val).changed() {
            cvars.set(key, val);
        }
    });
}
