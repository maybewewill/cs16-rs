//! Iconic CS 1.6 Main Menu (stitched `800_*_loading.tga` background + bottom-left menu items),
//! `New Game` VGUI2 dialog, and `Find Servers` VGUI2 dialog.

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::TextAlignment;
use cs_assets::vgui_scripts::{GameMenuEntry, SettingsScript};
use cs_cvar::{CVarRegistry, EngineAction};
use cs_vfs::Vfs;
use egui::{Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, TextureHandle, Vec2};

use crate::ui::options::render_scr_options;
use crate::ui::screens::{ServerBrowserRenderCtx, ServerBrowserUi};
use crate::ui::vgui2::{TrackerButtonCfg, VguiRuntime, VguiTextDrawOpts};
use crate::ui::vgui_assets::{TrackerWindowCfg, show_tracker_window};
use crate::ui::vgui_theme::{VguiPalette, paint_vgui_bevel};

/// Render context for [`MainMenuUi`].
pub struct MainMenuRenderCtx<'a> {
    pub vfs: Option<&'a Vfs>,
    pub loc: Option<&'a LocalizationTable>,
    pub available_maps: &'a [String],
    pub in_game: bool,
    pub bg_texture: Option<&'a TextureHandle>,
    pub steam_icon: Option<&'a TextureHandle>,
    pub palette: VguiPalette,
}

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

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        cvars: &mut CVarRegistry,
        m: &MainMenuRenderCtx<'_>,
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

        if let Some(v) = m.vfs {
            runtime.load_res(v, "resource/CreateMultiplayerGameServerPage.res");
            runtime.load_res(v, "resource/CreateMultiplayerGameGameplayPage.res");
        }

        let palette = m.palette;
        let in_game = m.in_game;
        let available_maps = m.available_maps;

        // Ensure selected map exists in the list if possible
        if !available_maps.is_empty() && !available_maps.iter().any(|mp| mp == &self.selected_map) {
            if available_maps.iter().any(|mp| mp == "de_dust2") {
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
                    if let Some(tex) = m.bg_texture {
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

        // 2. Classic bottom-left transparent CS 1.6 menu column (`resource/GameMenu.res` with `MenuLarge`)
        let menu_x = (screen.width() * 0.042).clamp(24.0, 56.0);
        let menu_y = (screen.height() - if in_game { 295.0 } else { 210.0 }).max(90.0);
        let loc_ref = m.loc.or(self.loc.as_ref());
        egui::Area::new(egui::Id::unique("cs16_main_menu_items"))
            .order(egui::Order::Middle)
            .fixed_pos(Pos2::new(menu_x, menu_y))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;

                if !self.game_menu.is_empty() {
                    let entries = self.game_menu.clone();
                    for (idx, entry) in entries.into_iter().enumerate() {
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
                        if menu_link_vgui(ui, runtime, m.vfs, palette, &format!("gm_{idx}_{}", entry.command), label) {
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
                        if menu_link_vgui(ui, runtime, m.vfs, palette, "gm_resume", "Resume Game") {
                            out.resume_game = true;
                        }
                        if menu_link_vgui(ui, runtime, m.vfs, palette, "gm_disc", "Disconnect") {
                            out.actions.push(EngineAction::Disconnect);
                        }
                        if menu_link_vgui(ui, runtime, m.vfs, palette, "gm_plist", "Player List") {
                            out.open_player_list = true;
                        }
                        ui.add_space(6.0);
                    }

                    if menu_link_vgui(ui, runtime, m.vfs, palette, "gm_newgame", "New Game") {
                        self.new_game_open = true;
                    }
                    if menu_link_vgui(ui, runtime, m.vfs, palette, "gm_servers", "Find Servers") {
                        self.find_servers_open = true;
                        self.server_browser.open = true;
                    }
                    if menu_link_vgui(ui, runtime, m.vfs, palette, "gm_options", "Options") {
                        out.open_options = true;
                    }
                    if menu_link_vgui(ui, runtime, m.vfs, palette, "gm_demo", "Demo Player") {
                        out.open_demo_player = true;
                    }
                    ui.add_space(6.0);
                    if menu_link_vgui(ui, runtime, m.vfs, palette, "gm_quit", "Quit") {
                        out.actions.push(EngineAction::Quit);
                    }
                }
            });

        // 3. `New Game` Steam 2003 VGUI2 modal window (`CreateMultiplayerGame*.res`)
        if self.new_game_open {
            let mut open = self.new_game_open;
            let mut close_now = false;
            let title = loc_or(loc_ref, "#GameUI_NewGame", "New Game");
            let server_tab_lbl = loc_or(loc_ref, "#GameUI_Server", "Server");
            let game_tab_lbl = loc_or(loc_ref, "#GameUI_Game", "Game");
            let start_lbl = loc_or(loc_ref, "#GameUI_Start", "Start");
            let cancel_lbl = loc_or(loc_ref, "#GameUI_Cancel", "Cancel");

            show_tracker_window(
                ctx,
                runtime,
                &mut open,
                &TrackerWindowCfg {
                    id: "cs16_vgui_new_game",
                    title: &title,
                    default_pos: Pos2::new(190.0, 120.0),
                    default_size: Vec2::new(450.0, 355.0),
                    min_size: Vec2::new(420.0, 320.0),
                    resizable: false,
                    palette,
                    steam_icon: m.steam_icon,
                    vfs: m.vfs,
                },
                |ui, rt| {
                    ui.horizontal(|ui| {
                        if rt.draw_tracker_tab(
                            ui,
                            m.vfs,
                            palette,
                            "ng_tab_server",
                            &server_tab_lbl,
                            self.new_game_tab == NewGameTab::Server,
                        ) {
                            self.new_game_tab = NewGameTab::Server;
                        }
                        if rt.draw_tracker_tab(
                            ui,
                            m.vfs,
                            palette,
                            "ng_tab_game",
                            &game_tab_lbl,
                            self.new_game_tab == NewGameTab::Game,
                        ) {
                            self.new_game_tab = NewGameTab::Game;
                        }
                    });

                    let panel_resp = egui::Frame::NONE
                        .fill(palette.control_bg)
                        .inner_margin(egui::Margin::same(10))
                        .stroke(Stroke::new(1.0, palette.border_dark))
                        .show(ui, |ui| {
                            ui.set_min_size(egui::vec2(420.0, 245.0));
                            match self.new_game_tab {
                                NewGameTab::Server => {
                                    ui.horizontal(|ui| {
                                        let map_lbl = loc_or(loc_ref, "#Cstrike_Listen_MapName", "Map");
                                        ui.label(format!("{map_lbl}:"));
                                        egui::ComboBox::from_id_salt("ng_map_combo")
                                            .width(260.0)
                                            .selected_text(&self.selected_map)
                                            .show_ui(ui, |ui| {
                                                for mp in available_maps {
                                                    ui.selectable_value(&mut self.selected_map, mp.clone(), mp);
                                                }
                                            });
                                    });

                                    ui.add_space(6.0);
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
                                            if available_maps.iter().any(|mp| mp == classic) {
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

                                    ui.separator();
                                    let quota = cvars.get_i32("bot_quota").max(0);
                                    let mut bots_on = quota > 0;
                                    let inc_lbl = loc_or(
                                        loc_ref,
                                        "#Cstrike_Bot_IncludeBots",
                                        "Include CPU players (Bots) in this game",
                                    );
                                    if ui.checkbox(&mut bots_on, inc_lbl).changed() {
                                        cvars.set("bot_quota", if bots_on { "6" } else { "0" });
                                    }
                                    if bots_on {
                                        let num_lbl = loc_or(
                                            loc_ref,
                                            "#Cstrike_Bot_NumberOfBots",
                                            "Number of CPU players",
                                        );
                                        cvar_edit_row(ui, cvars, "bot_quota", &format!("{num_lbl}:"));
                                        let diff_lbl = loc_or(loc_ref, "#Cstrike_Bot_Difficulty", "Difficulty");
                                        let cur_diff = cvars.get_i32("bot_difficulty").clamp(0, 3);
                                        ui.horizontal(|ui| {
                                            ui.label(format!("{diff_lbl}:"));
                                            for (lvl, tok, fb) in [
                                                (0, "#Cstrike_Bot_Difficulty0", "Easy"),
                                                (1, "#Cstrike_Bot_Difficulty1", "Normal"),
                                                (2, "#Cstrike_Bot_Difficulty2", "Hard"),
                                                (3, "#Cstrike_Bot_Difficulty3", "Expert"),
                                            ] {
                                                let lbl = loc_or(loc_ref, tok, fb);
                                                if ui.radio(cur_diff == lvl, lbl).clicked() {
                                                    cvars.set("bot_difficulty", lvl.to_string());
                                                }
                                            }
                                        });
                                    }

                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new(format!("Discovered {} maps in VFS.", available_maps.len()))
                                            .small()
                                            .color(palette.dim_text),
                                    );
                                }
                                NewGameTab::Game => {
                                    egui::ScrollArea::vertical().max_height(245.0).show(ui, |ui| {
                                        if let Some(ref scr) = self.settings_script {
                                            render_scr_options(ui, cvars, scr, loc_ref);
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
                        if rt
                            .draw_tracker_button(
                                ui,
                                m.vfs,
                                palette,
                                &TrackerButtonCfg {
                                    id: "ng_cancel_btn",
                                    label: &cancel_lbl,
                                    min_size: Vec2::new(76.0, 24.0),
                                    default_btn: false,
                                    enabled: true,
                                },
                            )
                            .clicked()
                        {
                            close_now = true;
                        }
                        if rt
                            .draw_tracker_button(
                                ui,
                                m.vfs,
                                palette,
                                &TrackerButtonCfg {
                                    id: "ng_start_btn",
                                    label: &start_lbl,
                                    min_size: Vec2::new(76.0, 24.0),
                                    default_btn: true,
                                    enabled: true,
                                },
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
            let sb_ctx = ServerBrowserRenderCtx {
                vfs: m.vfs,
                loc: m.loc.or(self.loc.as_ref()),
                palette,
                steam_icon: m.steam_icon,
            };
            if let Some(addr) = self.server_browser.show(ctx, runtime, &sb_ctx) {
                out.actions.push(EngineAction::Connect(addr));
            }
            if !self.server_browser.open {
                self.find_servers_open = false;
            }
        }

        out
    }
}

/// Classic CS 1.6 borderless menu link in the lower-left corner (`TrackerScheme` `MenuLarge`).
fn menu_link_vgui(
    ui: &mut egui::Ui,
    runtime: &mut VguiRuntime,
    vfs: Option<&Vfs>,
    palette: VguiPalette,
    id_salt: &str,
    label: &str,
) -> bool {
    let ctx = ui.ctx().clone();
    let screen_h = ctx.content_rect().height().round().max(480.0) as u32;
    let tracker_fonts = runtime.tracker_scheme.fonts.clone();
    let measured = runtime.font_atlas.measure_text(
        &ctx,
        vfs,
        &tracker_fonts,
        "tracker",
        "MenuLarge",
        screen_h,
        label,
    );
    let desired = Vec2::new((measured.x + 12.0).max(90.0), (measured.y + 4.0).max(26.0));
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    runtime.track_interactive(id_salt, &response);

    let fg = if response.hovered() {
        palette.bright_control_text
    } else {
        palette.bright_text
    };
    let text_rect = Rect::from_min_size(
        Pos2::new(rect.min.x + 2.0, rect.min.y + 1.0),
        Vec2::new((rect.width() - 2.0).max(40.0), rect.height()),
    );
    runtime.font_atlas.paint_text(
        ui.painter(),
        &ctx,
        vfs,
        &tracker_fonts,
        "tracker",
        "MenuLarge",
        screen_h,
        text_rect,
        label,
        fg,
        VguiTextDrawOpts {
            align: TextAlignment::West,
            process_hotkey: false,
            proportional_scale: false,
        },
    );
    response.clicked()
}

fn loc_or(loc: Option<&LocalizationTable>, token: &str, fallback: &str) -> String {
    if let Some(l) = loc {
        let res = l.resolve(token);
        if !res.is_empty() && !res.starts_with('#') {
            return res.to_string();
        }
    }
    fallback.to_string()
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
