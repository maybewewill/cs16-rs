//! Iconic CS 1.6 Main Menu (stitched `800_*_loading.tga` background + bottom-left menu items),
//! `New Game` VGUI2 dialog, and `Find Servers` VGUI2 dialog.

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::TextAlignment;
use cs_assets::vgui_scripts::{GameMenuEntry, SettingsScript};
use cs_cvar::{CVarRegistry, EngineAction};
use cs_vfs::Vfs;
use egui::{Color32, CornerRadius, Pos2, Rect, Sense, TextureHandle, Vec2};

use crate::ui::options::render_scr_options;
use crate::ui::screens::{ServerBrowserRenderCtx, ServerBrowserUi};
use crate::ui::vgui2::{
    TrackerCtx, VguiRuntime, VguiTextDrawOpts, draw_tracker_button_rect,
    draw_tracker_check_button_rect, draw_tracker_close_button, draw_tracker_combo_box_rect,
    paint_tracker_bevel_rect, paint_tracker_label_rect,
};
use crate::ui::vgui_theme::VguiPalette;

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
    new_game_drag_offset: Vec2,
    new_game_map_idx: usize,
    steam_networking: bool,
    pub selected_map: String,
    pub game_menu: Vec<GameMenuEntry>,
    pub settings_script: Option<SettingsScript>,
    pub loc: Option<LocalizationTable>,
    /// `resource/game_menu.tga` (the COUNTER-STRIKE logo under the menu), loaded on first draw.
    logo: Option<egui::TextureHandle>,
    logo_tried: bool,
    /// The armed (highlighted) menu row: the first row until the mouse arms another one.
    armed_item: usize,
}

impl Default for MainMenuUi {
    fn default() -> Self {
        Self {
            visible: true,
            new_game_open: false,
            find_servers_open: false,
            server_browser: ServerBrowserUi::default(),
            new_game_tab: NewGameTab::Server,
            new_game_drag_offset: Vec2::ZERO,
            new_game_map_idx: 0,
            steam_networking: true,
            selected_map: "de_dust2".to_string(),
            game_menu: Vec::new(),
            settings_script: None,
            loc: None,
            logo: None,
            logo_tried: false,
            armed_item: 0,
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
                        // The 800x600 `BackgroundLayout.txt` composite is scaled uniformly by
                        // max(W/800, H/600) and cropped centred (measured against the original:
                        // 1280x720 and 1600x1080 crop top and bottom, 1280x1024 crops the sides).
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

        // 2. The game menu column (`resource/GameMenu.res`, `TrackerScheme` `MenuLarge`).
        // Layout reversed from GameUI.dll (`CBasePanel::ApplySchemeSettings`, `FUN_1003e590`)
        // and measured at 800x600..1600x1080: `s = max(1, height / 720)`, item pitch is
        // `MenuItemHeight` (48) * s, last baseline is 63 * s above bottom, logo 16 * s above
        // bottom, and `GameMenuInset` (115 * s) is scaled by 0.7 when `screen_w <= 1024`.
        let s_menu = (screen.height() / 720.0).max(1.0);
        let base_x: f32 = if screen.width() > 1024.0 { 115.0 } else { (115.0f32 * 0.7).round() };
        let menu_x = screen.min.x + base_x * s_menu;
        let item_pitch = 48.0 * s_menu;
        let loc_ref = m.loc.or(self.loc.as_ref());
        let source_entries = if self.game_menu.is_empty() {
            fallback_game_menu()
        } else {
            self.game_menu.clone()
        };
        let visible_entries: Vec<GameMenuEntry> = source_entries
            .iter()
            .filter(|e| !(e.only_in_game && !in_game) && !(e.not_multi && in_game))
            .cloned()
            .collect();
        let rows = visible_entries.len().max(1) as f32;
        let last_baseline = screen.max.y - 63.0 * s_menu;
        let menu_top = last_baseline - (rows - 1.0) * item_pitch - item_pitch * 0.75;

        // The logo (`game_menu.tga`) below the menu, drawn at its own size * s.
        if !self.logo_tried {
            self.logo_tried = true;
            self.logo = m.vfs.and_then(|v| cs_assets::scheme::load_tga_image(v, "resource/game_menu.tga").ok()).map(|img| {
                ctx.load_texture(
                    "cs16_menu_logo",
                    egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.rgba),
                    egui::TextureOptions::LINEAR,
                )
            });
        }
        if !in_game && let Some(logo) = &self.logo {
            let size = logo.size_vec2() * s_menu;
            // `game_menu.tga` (207x32, content from (1, 1) to (206, 25)): the content's left
            // edge is 4 px left of the menu text and its last row 16 px above the bottom (* s).
            let min = Pos2::new(menu_x - 5.0 * s_menu, screen.max.y - (16.0 + 24.0) * s_menu);
            egui::Area::new(egui::Id::unique("cs16_menu_logo"))
                .order(egui::Order::Middle)
                .fixed_pos(min)
                .interactable(false)
                .show(ctx, |ui| {
                    ui.painter().image(
                        logo.id(),
                        Rect::from_min_size(min, size),
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                });
        }

        egui::Area::new(egui::Id::unique("cs16_main_menu_items"))
            .order(egui::Order::Middle)
            .fixed_pos(Pos2::new(menu_x, menu_top))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut armed = self.armed_item;
                for (row, entry) in visible_entries.iter().enumerate() {
                    let raw_label = loc_ref.map_or(entry.label.as_str(), |l| l.resolve(&entry.label));
                    let label = raw_label.trim();
                    if label.is_empty() {
                        // A blank `GameMenu.res` entry is a spacer of one item height.
                        ui.allocate_exact_size(Vec2::new(10.0, item_pitch), Sense::hover());
                        continue;
                    }
                    let (clicked, hovered) = menu_item_vgui(
                        ui,
                        runtime,
                        m.vfs,
                        &format!("gm_{row}_{}", entry.command),
                        label,
                        s_menu,
                        item_pitch,
                        armed == row,
                    );
                    if hovered {
                        armed = row;
                    }
                    if clicked {
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
                self.armed_item = armed;
            });

        // 3. `New Game` Steam 2003 VGUI2 modal window (`CreateMultiplayerGameDialog`, `FUN_1001e9b0`)
        if self.new_game_open {
            if let Some(v) = cvars.get("sv_use_steam_networking") {
                self.steam_networking = v.as_bool();
            }
            let mut close_now = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            let s = (screen.height() / 720.0).max(1.0);
            let screen_h = screen.height().round().max(480.0) as u32;
            let tc = TrackerCtx {
                ctx,
                vfs: m.vfs,
                palette,
                screen_h,
                scale: s,
            };

            let dlg_w = (448.0 * s).floor();
            let dlg_h = (460.0 * s).floor();
            let default_x = screen.left() + ((screen.width() - dlg_w) * 0.5).floor();
            let default_y = screen.top() + ((screen.height() - dlg_h) * 0.5).floor();
            let max_x = (screen.right() - dlg_w).max(screen.left());
            let max_y = (screen.bottom() - dlg_h).max(screen.top());
            let dlg_min = Pos2::new(
                (default_x + self.new_game_drag_offset.x)
                    .clamp(screen.left(), max_x)
                    .round(),
                (default_y + self.new_game_drag_offset.y)
                    .clamp(screen.top(), max_y)
                    .round(),
            );
            let dlg_rect = Rect::from_min_size(dlg_min, Vec2::new(dlg_w, dlg_h));

            let title = loc_or(loc_ref, "#GameUI_CreateServer", "Create Server");
            let server_tab_lbl = loc_or(loc_ref, "#GameUI_Server", "Server");
            let game_tab_lbl = loc_or(loc_ref, "#GameUI_Game", "Game");
            let start_lbl = loc_or(loc_ref, "#GameUI_Start", "Start");
            let cancel_lbl = loc_or(loc_ref, "#GameUI_Cancel", "Cancel");

            egui::Area::new(egui::Id::unique("cs16_vgui_new_game"))
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
                        ui.make_persistent_id("ng_title_drag"),
                        Sense::drag(),
                    );
                    if drag_resp.dragged() {
                        self.new_game_drag_offset += drag_resp.drag_delta();
                    }
                    if let Some(icon) = m.steam_icon {
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
                    let title_rect = Rect::from_min_size(
                        dlg_min + Vec2::new((28.0 * s).round(), (4.0 * s).round()),
                        Vec2::new((240.0 * s).round(), (22.0 * s).round()),
                    );
                    paint_tracker_label_rect(
                        ui.painter(),
                        runtime,
                        &tc,
                        title_rect,
                        &title,
                        "DefaultBold",
                        TextAlignment::West,
                        tc.palette.bright_text,
                        false,
                        true,
                    );
                    let close_rect = Rect::from_min_size(
                        dlg_min + Vec2::new(((448.0 - 25.0) * s).round(), (8.0 * s).round()),
                        Vec2::splat((18.0 * s).round()),
                    );
                    if draw_tracker_close_button(ui, runtime, &tc, "ng_close", close_rect) {
                        close_now = true;
                    }

                    // PropertySheet (`(5, 55, 438, 368) * s`)
                    let sheet_rect = Rect::from_min_size(
                        dlg_min + Vec2::new((5.0 * s).round(), (55.0 * s).round()),
                        Vec2::new((438.0 * s).round(), (368.0 * s).round()),
                    );
                    ui.painter()
                        .rect_filled(sheet_rect, CornerRadius::ZERO, tc.palette.control_bg);
                    paint_tracker_bevel_rect(ui.painter(), sheet_rect, true, tc.palette);

                    let tabs = [
                        (NewGameTab::Server, server_tab_lbl),
                        (NewGameTab::Game, game_tab_lbl),
                    ];
                    let mut tx = dlg_min.x + (5.0 * s).round();
                    let fonts = runtime.tracker_scheme.fonts.clone();
                    let tab_pad_w = (16.0 * s).round();
                    let min_tab_w = (64.0 * s).round();
                    let tab_gap = (1.0 * s).round().max(1.0);
                    for (tab_val, label) in &tabs {
                        let tw = runtime
                            .font_atlas
                            .measure_text_with_scale(
                                ctx,
                                m.vfs,
                                &fonts,
                                "tracker",
                                "Default",
                                screen_h,
                                label,
                                s,
                            )
                            .x;
                        let tab_w = (tw + tab_pad_w).round().max(min_tab_w);
                        let active = self.new_game_tab == *tab_val;
                        let top_y =
                            dlg_min.y + if active { (29.0 * s).round() } else { (31.0 * s).round() };
                        let tab_rect = Rect::from_min_max(
                            Pos2::new(tx, top_y),
                            Pos2::new(tx + tab_w, sheet_rect.top()),
                        );
                        let tresp = ui.interact(
                            tab_rect,
                            ui.make_persistent_id(("ng_tab", label)),
                            Sense::click(),
                        );
                        runtime.track_interactive(&format!("ng_tab_{label}"), &tresp);

                        let tp = ui.painter();
                        tp.rect_filled(tab_rect, CornerRadius::ZERO, tc.palette.control_bg);
                        tp.rect_filled(
                            Rect::from_min_max(tab_rect.min, Pos2::new(tab_rect.right(), tab_rect.top() + 1.0)),
                            0.0,
                            tc.palette.border_bright,
                        );
                        tp.rect_filled(
                            Rect::from_min_max(tab_rect.min, Pos2::new(tab_rect.left() + 1.0, tab_rect.bottom())),
                            0.0,
                            tc.palette.border_bright,
                        );
                        tp.rect_filled(
                            Rect::from_min_max(Pos2::new(tab_rect.right() - 1.0, tab_rect.top()), tab_rect.max),
                            0.0,
                            tc.palette.border_dark,
                        );
                        if active {
                            tp.rect_filled(
                                Rect::from_min_max(
                                    Pos2::new(tab_rect.left() + 1.0, sheet_rect.top()),
                                    Pos2::new(tab_rect.right() - 1.0, sheet_rect.top() + 1.0),
                                ),
                                0.0,
                                tc.palette.control_bg,
                            );
                        }
                        let fg = if active {
                            tc.palette.bright_control_text
                        } else {
                            tc.palette.bright_text
                        };
                        let inset_x = (8.0 * s).round();
                        let lbl_rect = Rect::from_min_max(
                            Pos2::new(tab_rect.left() + inset_x, tab_rect.top() + (1.0 * s).round()),
                            Pos2::new(tab_rect.right() - (2.0 * s).round(), tab_rect.bottom()),
                        );
                        paint_tracker_label_rect(
                            tp,
                            runtime,
                            &tc,
                            lbl_rect,
                            label,
                            "Default",
                            TextAlignment::West,
                            fg,
                            false,
                            true,
                        );
                        if tresp.clicked() {
                            self.new_game_tab = *tab_val;
                        }
                        tx += tab_w + tab_gap;
                    }

                    let page_origin = sheet_rect.min;
                    let page_sub = |x: f32, y: f32, w: f32, h: f32| -> Rect {
                        Rect::from_min_size(
                            page_origin + Vec2::new((x * s).round(), (y * s).round()),
                            Vec2::new((w * s).round(), (h * s).round()),
                        )
                    };

                    match self.new_game_tab {
                        NewGameTab::Server => {
                            let map_lbl = loc_or(loc_ref, "#Cstrike_Listen_MapName", "Map");
                            paint_tracker_label_rect(
                                ui.painter(),
                                runtime,
                                &tc,
                                page_sub(28.0, 28.0, 38.0, 24.0),
                                &map_lbl,
                                "Default",
                                TextAlignment::West,
                                tc.palette.base_text,
                                false,
                                true,
                            );

                            let mut combo_items = Vec::with_capacity(available_maps.len() + 1);
                            combo_items.push(loc_or(loc_ref, "#GameUI_RandomMap", "< Random Map >"));
                            combo_items.extend(available_maps.iter().cloned());
                            if self.new_game_map_idx >= combo_items.len() {
                                self.new_game_map_idx = 0;
                            }
                            if draw_tracker_combo_box_rect(
                                ui,
                                runtime,
                                &tc,
                                "ng_map_combo",
                                page_sub(66.0, 28.0, 174.0, 24.0),
                                &combo_items,
                                &mut self.new_game_map_idx,
                                true,
                            ) && self.new_game_map_idx > 0
                                && let Some(mp) = available_maps.get(self.new_game_map_idx - 1)
                            {
                                self.selected_map = mp.clone();
                            }

                            let div_r = page_sub(28.0, 70.0, 280.0, 2.0);
                            ui.painter().rect_filled(
                                Rect::from_min_size(div_r.min, Vec2::new(div_r.width(), 1.0)),
                                0.0,
                                tc.palette.border_dark,
                            );
                            ui.painter().rect_filled(
                                Rect::from_min_size(
                                    Pos2::new(div_r.left(), div_r.top() + 1.0),
                                    Vec2::new(div_r.width(), 1.0),
                                ),
                                0.0,
                                tc.palette.border_bright,
                            );

                            let net_lbl = loc_or(
                                loc_ref,
                                "#Valve_Enable_SteamNetworking",
                                "Enable Steam Networking",
                            );
                            if draw_tracker_check_button_rect(
                                ui,
                                runtime,
                                &tc,
                                "ng_steam_net",
                                page_sub(21.0, 83.0, 320.0, 24.0),
                                &mut self.steam_networking,
                                &net_lbl,
                                true,
                            ) {
                                cvars.set(
                                    "sv_use_steam_networking",
                                    if self.steam_networking { "1" } else { "0" },
                                );
                            }
                        }
                        NewGameTab::Game => {
                            let list_r = page_sub(10.0, 10.0, 418.0, 346.0);
                            ui.painter()
                                .rect_filled(list_r, CornerRadius::ZERO, tc.palette.window_bg);
                            paint_tracker_bevel_rect(ui.painter(), list_r, false, tc.palette);
                            let inner = list_r.shrink((6.0 * s).round());
                            ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                                egui::ScrollArea::vertical()
                                    .max_height(inner.height())
                                    .show(ui, |ui| {
                                        if let Some(ref scr) = self.settings_script {
                                            render_scr_options(ui, cvars, scr, loc_ref);
                                        }
                                        ui.separator();
                                        cvar_edit_row(ui, cvars, "bot_quota", "Bots count (0..15):");
                                        cvar_edit_row(ui, cvars, "bot_difficulty", "Bot difficulty (0-3):");
                                    });
                            });
                        }
                    }

                    // Bottom buttons: Start (`283, 427, 72, 24 * s`) & Cancel (`363, 427, 72, 24 * s`)
                    let start_r = Rect::from_min_size(
                        dlg_min + Vec2::new((283.0 * s).round(), (427.0 * s).round()),
                        Vec2::new((72.0 * s).round(), (24.0 * s).round()),
                    );
                    let cancel_r = Rect::from_min_size(
                        dlg_min + Vec2::new((363.0 * s).round(), (427.0 * s).round()),
                        Vec2::new((72.0 * s).round(), (24.0 * s).round()),
                    );
                    if draw_tracker_button_rect(
                        ui,
                        runtime,
                        &tc,
                        "ng_start_btn",
                        start_r,
                        &start_lbl,
                        TextAlignment::West,
                        true,
                        true,
                    )
                    .clicked()
                    {
                        let chosen = if self.new_game_map_idx == 0 {
                            available_maps
                                .iter()
                                .find(|mp| *mp == "de_dust2")
                                .or_else(|| available_maps.first())
                                .cloned()
                                .unwrap_or_else(|| self.selected_map.clone())
                        } else {
                            available_maps
                                .get(self.new_game_map_idx - 1)
                                .cloned()
                                .unwrap_or_else(|| self.selected_map.clone())
                        };
                        self.selected_map = chosen.clone();
                        out.actions.push(EngineAction::LoadMap(chosen));
                        close_now = true;
                    }
                    if draw_tracker_button_rect(
                        ui,
                        runtime,
                        &tc,
                        "ng_cancel_btn",
                        cancel_r,
                        &cancel_lbl,
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

/// One game-menu row (`TrackerScheme`: `MenuLarge` text in `MenuColor`, the armed row in
/// `ArmedMenuColor` over an additive `MenuBlurLarge` glow in `BlurMenuColor`). The row is
/// `pitch` tall and the text baseline sits three quarters down it; fonts scale with `s`.
/// Returns `(clicked, hovered)`.
#[allow(clippy::too_many_arguments)]
fn menu_item_vgui(
    ui: &mut egui::Ui,
    runtime: &mut VguiRuntime,
    vfs: Option<&Vfs>,
    id_salt: &str,
    label: &str,
    s: f32,
    pitch: f32,
    armed: bool,
) -> (bool, bool) {
    let ctx = ui.ctx().clone();
    // `MenuLarge`/`MenuBlurLarge` drawn at `tall * MENU_FONT_FACTOR * s` pixels. The factor is
    // measured: the original's item widths are 0.775 of a `tall`-28 rasterization here
    // ("Find servers" 117 vs 149 px, "Options" 72 vs 94, "Quit" 40 vs 52 at 1280x800).
    const MENU_FONT_FACTOR: f32 = 0.775;
    let screen_h = 480;
    let scale_fonts = false;
    let mut tracker_fonts = runtime.tracker_scheme.fonts.clone();
    for name in ["menularge", "menublurlarge"] {
        if let Some(variants) = tracker_fonts.fonts.get_mut(name) {
            for v in variants.iter_mut() {
                v.tall = (v.tall * MENU_FONT_FACTOR * s).round();
            }
        }
    }
    let scheme_color = |runtime: &VguiRuntime, name: &str, fallback: [u8; 4]| {
        let c = runtime.tracker_scheme.resolve_color(name).unwrap_or(fallback);
        Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
    };
    let menu_color = scheme_color(runtime, "MenuColor", [238, 174, 0, 255]);
    let armed_color = scheme_color(runtime, "ArmedMenuColor", [255, 186, 0, 255]);
    let blur_color = scheme_color(runtime, "BlurMenuColor", [255, 178, 67, 255]);
    let measured = runtime.font_atlas.measure_text_scaled(
        &ctx,
        vfs,
        &tracker_fonts,
        "tracker",
        "MenuLarge",
        screen_h,
        label,
        scale_fonts,
    );
    let desired = Vec2::new(measured.x.max(40.0) + 4.0, pitch);
    let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
    runtime.track_interactive(id_salt, &response);
    let hovered = response.hovered();
    let lit = armed || hovered;
    // Text box: its bottom is the baseline (three quarters down the row) plus the descent.
    let baseline = rect.min.y + pitch * 0.75;
    let text_rect = Rect::from_min_max(
        Pos2::new(rect.min.x, baseline - measured.y),
        Pos2::new(rect.max.x, baseline + measured.y * 0.22),
    );
    let opts = VguiTextDrawOpts {
        align: TextAlignment::SouthWest,
        process_hotkey: false,
        proportional_scale: scale_fonts,
    };
    if lit {
        // Additive glow (`MenuBlurLarge`: blur 5, additive): a premultiplied colour with zero
        // alpha adds onto what is under it. Gaussian taps (sigma = radius / 2) whose weights sum
        // to 1.6, so the glyph cores saturate to the bright yellow the original shows on the
        // armed row (measured 255 255 28 over `ArmedMenuColor` text).
        let radius = (5.0 * s).round().max(1.0) as i32;
        let sigma = radius as f32 * 0.5;
        let mut taps = Vec::new();
        let mut total = 0.0f32;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let w = (-((dx * dx + dy * dy) as f32) / (2.0 * sigma * sigma)).exp();
                taps.push((dx, dy, w));
                total += w;
            }
        }
        for (dx, dy, w) in taps {
            let k = 1.6 * w / total;
            if k < 0.004 {
                continue;
            }
            let glow = Color32::from_rgba_premultiplied(
                (f32::from(blur_color.r()) * k).round() as u8,
                (f32::from(blur_color.g()) * k).round() as u8,
                (f32::from(blur_color.b()) * k).round() as u8,
                0,
            );
            runtime.font_atlas.paint_text(
                ui.painter(),
                &ctx,
                vfs,
                &tracker_fonts,
                "tracker",
                "MenuBlurLarge",
                screen_h,
                text_rect.translate(Vec2::new(dx as f32, dy as f32)),
                label,
                glow,
                opts,
            );
        }
    }
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
        if lit { armed_color } else { menu_color },
        opts,
    );
    (response.clicked(), hovered)
}

/// The menu when `resource/GameMenu.res` is missing (stock entries, same order).
fn fallback_game_menu() -> Vec<GameMenuEntry> {
    let e = |label: &str, command: &str, only_in_game: bool| GameMenuEntry {
        label: label.to_string(),
        command: command.to_string(),
        only_in_game,
        not_single: false,
        not_multi: false,
    };
    vec![
        e("#GameUI_GameMenu_ResumeGame", "ResumeGame", true),
        e("#GameUI_GameMenu_Disconnect", "Disconnect", true),
        e("#GameUI_GameMenu_PlayerList", "OpenPlayerListDialog", true),
        e("", "", true),
        e("#GameUI_GameMenu_NewGame", "OpenCreateMultiplayerGameDialog", false),
        e("#GameUI_GameMenu_FindServers", "OpenServerBrowser", false),
        e("#GameUI_GameMenu_Options", "OpenOptionsDialog", false),
        e("#GameUI_GameMenu_Quit", "Quit", false),
    ]
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
