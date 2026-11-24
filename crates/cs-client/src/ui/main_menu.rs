//! Iconic CS 1.6 Main Menu (stitched `800_*_loading.tga` background + bottom-left menu items),
//! `New Game` VGUI2 dialog, and `Find Servers` VGUI2 dialog.

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_scripts::{GameMenuEntry, SettingsScript};
use cs_cvar::{CVarRegistry, EngineAction};
use cs_vfs::Vfs;
use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Stroke, TextureHandle};

use crate::ui::options::render_scr_options;
use crate::ui::screens::ServerBrowserUi;
use crate::ui::vgui2::VguiRuntime;
use crate::ui::vgui_assets::show_steam2003_window;
use crate::ui::vgui_theme::{VguiPalette, paint_vgui_bevel, vgui_tab};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum NewGameTab {
    #[default]
    Server,
    Game,
}

pub struct MainMenuUi {
    pub visible: bool,
    pub new_game_open: bool,
    pub find_servers_open: bool,
    pub server_browser: ServerBrowserUi,
    new_game_tab: NewGameTab,
    pub selected_map: String,
    pub game_menu: Vec<GameMenuEntry>,
    pub settings_script: Option<SettingsScript>,
    pub loc: Option<LocalizationTable>,
}

impl Default for MainMenuUi {
    fn default() -> Self {
        Self {
            visible: true,
            new_game_open: false,
            find_servers_open: false,
            server_browser: ServerBrowserUi::default(),
            new_game_tab: NewGameTab::Server,
            selected_map: "de_dust2".to_string(),
            game_menu: Vec::new(),
            settings_script: None,
            loc: None,
        }
    }
}

pub struct MenuCommands {
    pub actions: Vec<EngineAction>,
    pub open_options: bool,
    pub open_player_list: bool,
    pub open_demo_player: bool,
    pub resume_game: bool,
}

impl MainMenuUi {
    pub fn load_resources(&mut self, vfs: &Vfs, lang: &str) {
        self.game_menu = GameMenuEntry::load(vfs);
        self.settings_script = SettingsScript::load(vfs, "settings.scr");
        self.loc = Some(LocalizationTable::load_language(vfs, lang));
    }

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
            open_player_list: false,
            open_demo_player: false,
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

        // 2. Classic bottom-left transparent CS 1.6 menu column (`resource/GameMenu.res`)
        let menu_x = (screen.width() * 0.042).clamp(24.0, 56.0);
        let menu_y = (screen.height() - if in_game { 295.0 } else { 210.0 }).max(90.0);
        let loc_ref = self.loc.as_ref();
        egui::Area::new(egui::Id::unique("cs16_main_menu_items"))
            .order(egui::Order::Middle)
            .fixed_pos(Pos2::new(menu_x, menu_y))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 7.0;

                if !self.game_menu.is_empty() {
                    let entries = self.game_menu.clone();
                    for entry in entries {
                        if entry.only_in_game && !in_game {
                            continue;
                        }
                        if entry.not_multi && in_game {
                            continue;
                        }
                        let raw_label = loc_ref.map_or(entry.label.as_str(), |l| l.resolve(&entry.label));
                        let label = raw_label.trim();
                        if label.is_empty() {
                            continue;
                        }
                        if menu_link(ui, label, palette) {
                            match entry.command.as_str() {
                                "ResumeGame" => out.resume_game = true,
                                "Disconnect" => out.actions.push(EngineAction::Disconnect),
                                "OpenPlayerListDialog" => out.open_player_list = true,
                                "OpenCreateMultiplayerGameDialog" => self.new_game_open = true,
                                "OpenServerBrowser" => {
                                    self.find_servers_open = true;
                                    self.server_browser.open = true;
                                }
                                "OpenOptionsDialog" => out.open_options = true,
                                "Quit" => out.actions.push(EngineAction::Quit),
                                cmd => {
                                    if let Some(rest) = cmd.strip_prefix("engine ") {
                                        out.actions.extend(cvars.execute_script(rest));
                                    }
                                }
                            }
                        }
                    }
                } else {
                    if in_game {
                        if menu_link(ui, "Resume Game", palette) {
                            out.resume_game = true;
                        }
                        if menu_link(ui, "Disconnect", palette) {
                            out.actions.push(EngineAction::Disconnect);
                        }
                        if menu_link(ui, "Player List", palette) {
                            out.open_player_list = true;
                        }
                        ui.add_space(6.0);
                    }

                    if menu_link(ui, "New Game", palette) {
                        self.new_game_open = true;
                    }
                    if menu_link(ui, "Find Servers", palette) {
                        self.find_servers_open = true;
                        self.server_browser.open = true;
                    }
                    if menu_link(ui, "Options", palette) {
                        out.open_options = true;
                    }
                    if menu_link(ui, "Demo Player", palette) {
                        out.open_demo_player = true;
                    }
                    ui.add_space(6.0);
                    if menu_link(ui, "Quit", palette) {
                        out.actions.push(EngineAction::Quit);
                    }
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
                                    egui::ScrollArea::vertical().max_height(215.0).show(ui, |ui| {
                                        if let Some(ref scr) = self.settings_script {
                                            render_scr_options(ui, cvars, scr, self.loc.as_ref());
                                        }
                                        ui.separator();
                                        cvar_edit_row(ui, cvars, "bot_quota", "Bots count (0..15):");
                                        cvar_edit_row(ui, cvars, "bot_difficulty", "Bot difficulty (0-3):");
                                    });
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

        // 4. `Find Servers` full 6-tab VGUI2 Server Browser window (`DialogServerBrowser.res`)
        if self.find_servers_open {
            self.server_browser.open = true;
            let mut tmp_rt = VguiRuntime::new();
            if let Some(addr) = self
                .server_browser
                .show(ctx, &mut tmp_rt, None, palette, steam_icon)
            {
                out.actions.push(EngineAction::Connect(addr));
            }
            if !self.server_browser.open {
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
