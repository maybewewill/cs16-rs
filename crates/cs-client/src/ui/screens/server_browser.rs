//! `.res`-driven CS 1.6 Server Browser (`Servers/DialogServerBrowser.res` +
//! `InternetGamesPage.res` + `InternetGamesPage_Filters.res` + `DialogAddServer.res`):
//! all 6 tabs (Internet, Favorites, History, Spectate, LAN, Friends), sortable `ListPanel`,
//! server filters, and LAN UDP discovery.

#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::TextAlignment;
#[cfg(not(target_arch = "wasm32"))]
use cs_proto48::query::discover_goldsrc_lan_servers;
use cs_vfs::Vfs;
use egui::{Color32, CornerRadius, Key, Pos2, Rect, Sense, Stroke, TextureHandle, Vec2};

use crate::ui::vgui2::controls::{ListColumn, VguiListPanelState};
use crate::ui::vgui2::{
    TrackerCtx, VguiRuntime, draw_tracker_button_rect, draw_tracker_check_button_rect,
    draw_tracker_close_button, draw_tracker_text_entry_rect, paint_tracker_bevel_rect,
    paint_tracker_label_rect,
};
use crate::ui::vgui_theme::VguiPalette;

/// Render context for [`ServerBrowserUi`].
pub struct ServerBrowserRenderCtx<'a> {
    pub vfs: Option<&'a Vfs>,
    pub loc: Option<&'a LocalizationTable>,
    pub palette: VguiPalette,
    pub steam_icon: Option<&'a TextureHandle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServerBrowserTab {
    #[default]
    Internet,
    Favorites,
    History,
    Spectate,
    Lan,
    Friends,
}

impl ServerBrowserTab {
    pub const ALL: [(Self, &'static str); 6] = [
        (Self::Internet, "Internet"),
        (Self::Favorites, "Favorites"),
        (Self::History, "History"),
        (Self::Spectate, "Spectate"),
        (Self::Lan, "LAN"),
        (Self::Friends, "Friends"),
    ];
}

#[derive(Debug, Clone, Default)]
pub struct ServerFilter {
    pub map: String,
    pub max_ping: u32,
    pub not_full: bool,
    pub has_users: bool,
    pub no_password: bool,
    pub secure_only: bool,
}

impl ServerFilter {
    pub fn matches(&self, srv: &ServerBrowserEntry) -> bool {
        if !self.map.trim().is_empty()
            && !srv
                .map
                .to_ascii_lowercase()
                .contains(&self.map.trim().to_ascii_lowercase())
        {
            return false;
        }
        if self.max_ping > 0 && srv.ping_ms > self.max_ping {
            return false;
        }
        if self.not_full && srv.players >= srv.max_players {
            return false;
        }
        if self.has_users && srv.players == 0 {
            return false;
        }
        if self.no_password && srv.password {
            return false;
        }
        if self.secure_only && !srv.vac {
            return false;
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerBrowserEntry {
    pub addr: String,
    pub name: String,
    pub game: String,
    pub map: String,
    pub players: u8,
    pub max_players: u8,
    pub ping_ms: u32,
    pub password: bool,
    pub vac: bool,
    pub spectate_port: bool,
}

pub struct ServerBrowserUi {
    pub open: bool,
    pub tab: ServerBrowserTab,
    pub filter: ServerFilter,
    pub show_filters: bool,
    pub add_dialog_open: bool,
    pub add_addr_input: String,
    pub internet: Vec<ServerBrowserEntry>,
    pub favorites: Vec<ServerBrowserEntry>,
    pub history: Vec<ServerBrowserEntry>,
    pub spectate: Vec<ServerBrowserEntry>,
    pub lan: Vec<ServerBrowserEntry>,
    pub friends: Vec<ServerBrowserEntry>,
    pub list_state: VguiListPanelState,
    drag_offset: Vec2,
    add_drag_offset: Vec2,
    custom_size: Option<Vec2>,
    sort_col: Option<usize>,
    sort_asc: bool,
    filter_game: String,
}

impl Default for ServerBrowserUi {
    fn default() -> Self {
        Self {
            open: false,
            tab: ServerBrowserTab::Internet,
            filter: ServerFilter::default(),
            show_filters: false,
            add_dialog_open: false,
            add_addr_input: "127.0.0.1:27015".to_string(),
            internet: Vec::new(),
            favorites: Vec::new(),
            history: Vec::new(),
            spectate: Vec::new(),
            lan: Vec::new(),
            friends: Vec::new(),
            list_state: VguiListPanelState {
                columns: vec![
                    ListColumn { name: "Servers".into(), width: 210 },
                    ListColumn { name: "Game".into(), width: 112 },
                    ListColumn { name: "Players".into(), width: 55 },
                    ListColumn { name: "Map".into(), width: 90 },
                    ListColumn { name: "Latency".into(), width: 55 },
                ],
                ..Default::default()
            },
            drag_offset: Vec2::ZERO,
            add_drag_offset: Vec2::ZERO,
            custom_size: None,
            sort_col: None,
            sort_asc: true,
            filter_game: "Counter-Strike".to_string(),
        }
    }
}

impl ServerBrowserUi {
    pub fn add_favorite(&mut self, entry: ServerBrowserEntry) {
        if !self.favorites.iter().any(|s| s.addr == entry.addr) {
            self.favorites.push(entry);
        }
    }

    pub fn record_history(&mut self, entry: ServerBrowserEntry) {
        self.history.retain(|s| s.addr != entry.addr);
        self.history.insert(0, entry);
        self.history.truncate(32);
    }

    pub fn active_entries(&self) -> Vec<&ServerBrowserEntry> {
        let src = match self.tab {
            ServerBrowserTab::Internet => &self.internet,
            ServerBrowserTab::Favorites => &self.favorites,
            ServerBrowserTab::History => &self.history,
            ServerBrowserTab::Spectate => &self.spectate,
            ServerBrowserTab::Lan => &self.lan,
            ServerBrowserTab::Friends => &self.friends,
        };
        src.iter().filter(|s| self.filter.matches(s)).collect()
    }

    pub fn sync_rows(&mut self) {
        let rows: Vec<Vec<String>> = self
            .active_entries()
            .into_iter()
            .map(|s| {
                vec![
                    s.name.clone(),
                    s.game.clone(),
                    format!("{}/{}", s.players, s.max_players),
                    s.map.clone(),
                    s.ping_ms.to_string(),
                ]
            })
            .collect();
        self.list_state.rows = rows;
    }

    pub fn refresh_lan(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.lan = discover_goldsrc_lan_servers(Duration::from_millis(300))
                .into_iter()
                .map(|q| ServerBrowserEntry {
                    addr: q.addr,
                    name: q.name,
                    game: "Counter-Strike".to_string(),
                    map: q.map,
                    players: q.players,
                    max_players: q.max_players,
                    ping_ms: q.ping_ms,
                    password: false,
                    vac: true,
                    spectate_port: false,
                })
                .collect();
        }
        self.sync_rows();
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        runtime: &mut VguiRuntime,
        s: &ServerBrowserRenderCtx<'_>,
    ) -> Option<String> {
        if !self.open {
            return None;
        }
        if let Some(v) = s.vfs {
            runtime.load_res(v, "servers/DialogServerBrowser.res");
            runtime.load_res(v, "servers/InternetGamesPage.res");
            runtime.load_res(v, "servers/InternetGamesPage_Filters.res");
            runtime.load_res(v, "servers/DialogAddServer.res");
        }
        self.sync_rows();

        let screen = ctx.content_rect();
        let s_chrome = (screen.height() / 720.0).max(1.0);
        let screen_h = screen.height().round().max(480.0) as u32;
        let tc = TrackerCtx {
            ctx,
            vfs: s.vfs,
            palette: s.palette,
            screen_h,
            scale: s_chrome,
        };

        // Original `CServerBrowserDialog` (`FUN_100102d0`) default/restored size capped to 960x594
        // (verified at 800x600 -> 800x594, 1280x{720,800,1024} -> 960x594).
        let min_w = screen.width().min((640.0 * s_chrome).round());
        let min_h = screen.height().min((384.0 * s_chrome).round());
        let base_sz = self.custom_size.unwrap_or_else(|| {
            Vec2::new(
                screen.width().min(960.0).round(),
                screen.height().min(594.0).round(),
            )
        });
        let dlg_w = base_sz.x.clamp(min_w, screen.width()).round();
        let dlg_h = base_sz.y.clamp(min_h, screen.height()).round();
        let default_x = screen.left() + ((screen.width() - dlg_w) * 0.5).floor();
        let default_y = screen.top() + ((screen.height() - dlg_h) * 0.5).floor();
        let max_x = (screen.right() - dlg_w).max(screen.left());
        let max_y = (screen.bottom() - dlg_h).max(screen.top());
        let dlg_min = Pos2::new(
            (default_x + self.drag_offset.x)
                .clamp(screen.left(), max_x)
                .round(),
            (default_y + self.drag_offset.y)
                .clamp(screen.top(), max_y)
                .round(),
        );
        let dlg_rect = Rect::from_min_size(dlg_min, Vec2::new(dlg_w, dlg_h));

        let mut close_now = ctx.input(|i| i.key_pressed(Key::Escape)) && !self.add_dialog_open;
        let mut connect_to: Option<String> = None;

        let title = loc_or(s.loc, "#ServerBrowser_ServerBrowserTitle", "Servers");
        let tab_labels = [
            (ServerBrowserTab::Internet, loc_or(s.loc, "#ServerBrowser_InternetTab", "Internet")),
            (ServerBrowserTab::Favorites, loc_or(s.loc, "#ServerBrowser_FavoritesTab", "Favorites")),
            (ServerBrowserTab::History, loc_or(s.loc, "#ServerBrowser_HistoryTab", "History")),
            (ServerBrowserTab::Spectate, loc_or(s.loc, "#ServerBrowser_SpectateTab", "Spectate")),
            (ServerBrowserTab::Lan, loc_or(s.loc, "#ServerBrowser_LanTab", "Lan")),
            (ServerBrowserTab::Friends, loc_or(s.loc, "#ServerBrowser_FriendsTab", "Friends")),
        ];

        let mut entries: Vec<ServerBrowserEntry> =
            self.active_entries().into_iter().cloned().collect();
        if let Some(col) = self.sort_col {
            entries.sort_by(|a, b| {
                let ord = match col {
                    0 => a.password.cmp(&b.password),
                    1 => false.cmp(&false),
                    2 => a.vac.cmp(&b.vac),
                    3 => a.players.cmp(&b.players),
                    4 => a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()),
                    5 => a.game.to_ascii_lowercase().cmp(&b.game.to_ascii_lowercase()),
                    6 => a.map.to_ascii_lowercase().cmp(&b.map.to_ascii_lowercase()),
                    _ => a.ping_ms.cmp(&b.ping_ms),
                };
                if self.sort_asc { ord } else { ord.reverse() }
            });
        }

        egui::Area::new(egui::Id::unique("cs16_vgui_server_browser"))
            .order(egui::Order::Foreground)
            .fixed_pos(dlg_rect.min)
            .show(ctx, |ui| {
                ui.allocate_exact_size(dlg_rect.size(), Sense::hover());
                let p = ui.painter();
                p.rect_filled(dlg_rect, CornerRadius::ZERO, tc.palette.control_bg);
                paint_tracker_bevel_rect(p, dlg_rect, true, tc.palette);

                // Titlebar (`0..29*s`)
                let title_drag = Rect::from_min_size(
                    dlg_min,
                    Vec2::new((dlg_w - 28.0 * s_chrome).max(40.0), (29.0 * s_chrome).round()),
                );
                let drag_resp = ui.interact(
                    title_drag,
                    ui.make_persistent_id("sb_title_drag"),
                    Sense::drag(),
                );
                if drag_resp.dragged() {
                    self.drag_offset += drag_resp.drag_delta();
                }
                if let Some(icon) = s.steam_icon {
                    let isz = (16.0 * s_chrome).round();
                    let ir = Rect::from_min_size(
                        dlg_min + Vec2::new((7.0 * s_chrome).round(), (8.0 * s_chrome).round()),
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
                        dlg_min + Vec2::new((28.0 * s_chrome).round(), (4.0 * s_chrome).round()),
                        Vec2::new((240.0 * s_chrome).round(), (22.0 * s_chrome).round()),
                    ),
                    &title,
                    "DefaultBold",
                    TextAlignment::West,
                    tc.palette.bright_text,
                    false,
                    true,
                );
                let close_r = Rect::from_min_size(
                    Pos2::new(
                        dlg_rect.right() - (25.0 * s_chrome).round(),
                        dlg_min.y + (8.0 * s_chrome).round(),
                    ),
                    Vec2::splat((18.0 * s_chrome).round()),
                );
                if draw_tracker_close_button(ui, runtime, &tc, "sb_close", close_r) {
                    close_now = true;
                }

                // `PropertySheet`: `(8, 72)..(dlg_w - 8, dlg_h - 34)`
                let margin_x = (8.0 * s_chrome).round();
                let sheet_rect = Rect::from_min_max(
                    Pos2::new(dlg_min.x + margin_x, dlg_min.y + (72.0 * s_chrome).round()),
                    Pos2::new(dlg_rect.right() - margin_x, dlg_rect.bottom() - (34.0 * s_chrome).round()),
                );
                ui.painter()
                    .rect_filled(sheet_rect, CornerRadius::ZERO, tc.palette.control_bg);
                paint_tracker_bevel_rect(ui.painter(), sheet_rect, true, tc.palette);

                // 6 tabs: Internet | Favorites | History | Spectate | LAN | Friends
                let mut tx = sheet_rect.left();
                let fonts = runtime.tracker_scheme.fonts.clone();
                let tab_pad_w = (20.0 * s_chrome).round();
                let min_tab_w = (72.0 * s_chrome).round();
                let tab_gap = (2.0 * s_chrome).round().max(1.0);
                for (t, label) in &tab_labels {
                    let tw = runtime
                        .font_atlas
                        .measure_text_with_scale(
                            ctx,
                            s.vfs,
                            &fonts,
                            "tracker",
                            "Default",
                            screen_h,
                            label,
                            s_chrome,
                        )
                        .x;
                    let tab_w = (tw + tab_pad_w).round().max(min_tab_w);
                    let active = self.tab == *t;
                    let top_y = dlg_min.y
                        + if active {
                            (46.0 * s_chrome).round()
                        } else {
                            (48.0 * s_chrome).round()
                        };
                    let tab_rect = Rect::from_min_max(
                        Pos2::new(tx, top_y),
                        Pos2::new(tx + tab_w, sheet_rect.top()),
                    );
                    let tresp = ui.interact(
                        tab_rect,
                        ui.make_persistent_id(("sb_tab", label)),
                        Sense::click(),
                    );
                    runtime.track_interactive(&format!("sb_tab_{label}"), &tresp);

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
                    let inset_x = (8.0 * s_chrome).round();
                    let lbl_rect = Rect::from_min_max(
                        Pos2::new(tab_rect.left() + inset_x, tab_rect.top() + (1.0 * s_chrome).round()),
                        Pos2::new(tab_rect.right() - (2.0 * s_chrome).round(), tab_rect.bottom()),
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
                        self.tab = *t;
                        self.list_state.selected_row = None;
                    }
                    tx += tab_w + tab_gap;
                }

                let btn_h = (24.0 * s_chrome).round();
                let btn_y = sheet_rect.bottom() - (33.0 * s_chrome).round();
                let filter_h = if self.show_filters {
                    (84.0 * s_chrome).round()
                } else {
                    0.0
                };
                let list_rect = Rect::from_min_max(
                    Pos2::new(sheet_rect.left() + 1.0, sheet_rect.top() + (9.0 * s_chrome).round()),
                    Pos2::new(
                        sheet_rect.right() - 1.0,
                        (btn_y - (10.0 * s_chrome).round() - filter_h).max(sheet_rect.top() + 60.0),
                    ),
                );

                let (clicked_col, dbl_row) = draw_server_browser_list(
                    ui,
                    runtime,
                    &tc,
                    s.loc,
                    list_rect,
                    &entries,
                    &mut self.list_state.selected_row,
                );
                if let Some(col) = clicked_col {
                    if self.sort_col == Some(col) {
                        self.sort_asc = !self.sort_asc;
                    } else {
                        self.sort_col = Some(col);
                        self.sort_asc = true;
                    }
                }
                if let Some(row_idx) = dbl_row
                    && let Some(srv) = entries.get(row_idx)
                {
                    self.record_history(srv.clone());
                    connect_to = Some(srv.addr.clone());
                }

                if self.show_filters {
                    let fy0 = list_rect.bottom() + (8.0 * s_chrome).round();
                    let pad = (12.0 * s_chrome).round();
                    let lbl_r = Rect::from_min_size(
                        Pos2::new(sheet_rect.left() + pad, fy0 + (2.0 * s_chrome).round()),
                        Vec2::new((42.0 * s_chrome).round(), btn_h),
                    );
                    paint_tracker_label_rect(
                        ui.painter(),
                        runtime,
                        &tc,
                        lbl_r,
                        "Map",
                        "Default",
                        TextAlignment::West,
                        tc.palette.base_text,
                        false,
                        true,
                    );
                    let map_r = Rect::from_min_size(
                        Pos2::new(lbl_r.right() + (4.0 * s_chrome).round(), fy0 + (2.0 * s_chrome).round()),
                        Vec2::new((156.0 * s_chrome).round(), btn_h),
                    );
                    draw_tracker_text_entry_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_filter_map",
                        map_r,
                        &mut self.filter.map,
                        false,
                        false,
                        true,
                    );
                    let cx0 = map_r.right() + (24.0 * s_chrome).round();
                    let cw = (180.0 * s_chrome).round();
                    draw_tracker_check_button_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_filt_full",
                        Rect::from_min_size(Pos2::new(cx0, fy0), Vec2::new(cw, btn_h)),
                        &mut self.filter.not_full,
                        &loc_or(s.loc, "#ServerBrowser_ServerNotFull", "Server not full"),
                        true,
                    );
                    draw_tracker_check_button_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_filt_users",
                        Rect::from_min_size(Pos2::new(cx0, fy0 + (26.0 * s_chrome).round()), Vec2::new(cw, btn_h)),
                        &mut self.filter.has_users,
                        &loc_or(s.loc, "#ServerBrowser_HasUsersPlaying", "Has users playing"),
                        true,
                    );
                    let cx1 = cx0 + cw + (12.0 * s_chrome).round();
                    draw_tracker_check_button_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_filt_nopw",
                        Rect::from_min_size(Pos2::new(cx1, fy0), Vec2::new(cw + 40.0 * s_chrome, btn_h)),
                        &mut self.filter.no_password,
                        &loc_or(s.loc, "#ServerBrowser_IsNotPasswordProtected", "Is not password protected"),
                        true,
                    );
                    draw_tracker_check_button_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_filt_vac",
                        Rect::from_min_size(Pos2::new(cx1, fy0 + (26.0 * s_chrome).round()), Vec2::new(cw + 40.0 * s_chrome, btn_h)),
                        &mut self.filter.secure_only,
                        "Anti-cheat (VAC)",
                        true,
                    );
                }

                // Bottom bar inside sheet (`InternetGamesPage.res`)
                let filt_lbl = loc_or(s.loc, "#ServerBrowser_ChangeFilters", "Change filters");
                let filt_r = Rect::from_min_size(
                    Pos2::new(sheet_rect.left() + margin_x, btn_y),
                    Vec2::new((108.0 * s_chrome).round(), btn_h),
                );
                if draw_tracker_button_rect(
                    ui,
                    runtime,
                    &tc,
                    "sb_filters_btn",
                    filt_r,
                    &filt_lbl,
                    TextAlignment::West,
                    false,
                    true,
                )
                .clicked()
                {
                    self.show_filters = !self.show_filters;
                }

                let mut summary_parts = vec![format!("{};", self.filter_game)];
                if !self.filter.map.trim().is_empty() {
                    summary_parts.push(format!("map: {};", self.filter.map.trim()));
                }
                if self.filter.not_full {
                    summary_parts.push("not full;".into());
                }
                if self.filter.has_users {
                    summary_parts.push("has players;".into());
                }
                if self.filter.no_password {
                    summary_parts.push("no password;".into());
                }
                if self.filter.secure_only {
                    summary_parts.push("VAC;".into());
                }
                let summary_txt = summary_parts.join(" ");

                let btn_gap = (8.0 * s_chrome).round();
                let mut rx = sheet_rect.right() - margin_x;

                let conn_w = (75.0 * s_chrome).round();
                rx -= conn_w;
                let sel = self
                    .list_state
                    .selected_row
                    .and_then(|i| entries.get(i))
                    .cloned();
                let conn_lbl = loc_or(s.loc, "#ServerBrowser_Connect", "Connect");
                if draw_tracker_button_rect(
                    ui,
                    runtime,
                    &tc,
                    "sb_connect_btn",
                    Rect::from_min_size(Pos2::new(rx, btn_y), Vec2::new(conn_w, btn_h)),
                    &conn_lbl,
                    TextAlignment::West,
                    true,
                    sel.is_some(),
                )
                .clicked()
                    && let Some(srv) = sel
                {
                    self.record_history(srv.clone());
                    connect_to = Some(srv.addr);
                }

                let ref_w = (95.0 * s_chrome).round();
                rx -= btn_gap + ref_w;
                let ref_lbl = loc_or(s.loc, "#ServerBrowser_RefreshAll", "Refresh all");
                if draw_tracker_button_rect(
                    ui,
                    runtime,
                    &tc,
                    "sb_refresh_btn",
                    Rect::from_min_size(Pos2::new(rx, btn_y), Vec2::new(ref_w, btn_h)),
                    &ref_lbl,
                    TextAlignment::West,
                    false,
                    true,
                )
                .clicked()
                {
                    self.refresh_lan();
                }

                let qref_w = (105.0 * s_chrome).round();
                rx -= btn_gap + qref_w;
                let qref_lbl = loc_or(s.loc, "#ServerBrowser_RefreshQuick", "Quick refresh");
                if draw_tracker_button_rect(
                    ui,
                    runtime,
                    &tc,
                    "sb_qrefresh_btn",
                    Rect::from_min_size(Pos2::new(rx, btn_y), Vec2::new(qref_w, btn_h)),
                    &qref_lbl,
                    TextAlignment::West,
                    false,
                    !entries.is_empty(),
                )
                .clicked()
                {
                    self.refresh_lan();
                }

                if self.tab == ServerBrowserTab::Favorites {
                    let add_w = (98.0 * s_chrome).round();
                    rx -= btn_gap + add_w;
                    let add_lbl = loc_or(s.loc, "#ServerBrowser_AddServer", "Add a server");
                    if draw_tracker_button_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_add_btn",
                        Rect::from_min_size(Pos2::new(rx, btn_y), Vec2::new(add_w, btn_h)),
                        &add_lbl,
                        TextAlignment::West,
                        false,
                        true,
                    )
                    .clicked()
                    {
                        self.add_dialog_open = true;
                    }
                }

                let sum_x0 = filt_r.right() + (8.0 * s_chrome).round();
                let sum_w = (rx - sum_x0 - (8.0 * s_chrome).round()).max(0.0);
                if sum_w > 12.0 {
                    paint_tracker_label_rect(
                        ui.painter(),
                        runtime,
                        &tc,
                        Rect::from_min_size(Pos2::new(sum_x0, btn_y), Vec2::new(sum_w, btn_h)),
                        &summary_txt,
                        "Default",
                        TextAlignment::West,
                        tc.palette.dim_text,
                        false,
                        true,
                    );
                }

                // Bottom status bar below sheet + bottom-right 3x3 resize grip
                let status = if entries.is_empty() {
                    match self.tab {
                        ServerBrowserTab::Lan => "There are no servers running on your local network.",
                        _ => "Refreshing server list...",
                    }
                    .to_string()
                } else {
                    format!("{} server(s) listed.", entries.len())
                };
                paint_tracker_label_rect(
                    ui.painter(),
                    runtime,
                    &tc,
                    Rect::from_min_max(
                        Pos2::new(sheet_rect.left(), sheet_rect.bottom() + (3.0 * s_chrome).round()),
                        Pos2::new(dlg_rect.right() - (22.0 * s_chrome).round(), dlg_rect.bottom() - 2.0),
                    ),
                    &status,
                    "Default",
                    TextAlignment::West,
                    tc.palette.dim_text,
                    false,
                    true,
                );

                let grip_sz = (14.0 * s_chrome).round();
                let grip_rect = Rect::from_min_size(
                    Pos2::new(dlg_rect.right() - grip_sz, dlg_rect.bottom() - grip_sz),
                    Vec2::splat(grip_sz),
                );
                let grip_resp = ui.interact(
                    grip_rect,
                    ui.make_persistent_id("sb_resize_grip"),
                    Sense::drag(),
                );
                if grip_resp.dragged() {
                    let cur = self.custom_size.unwrap_or(Vec2::new(dlg_w, dlg_h));
                    let next = cur + grip_resp.drag_delta();
                    self.custom_size = Some(Vec2::new(
                        next.x.clamp(min_w, screen.width()),
                        next.y.clamp(min_h, screen.height()),
                    ));
                }
                let gp = ui.painter();
                let dot_sz = (2.0 * s_chrome).round().max(1.5);
                let step = (3.5 * s_chrome).round().max(3.0);
                for (dx, dy) in [(1, 3), (2, 2), (2, 3), (3, 1), (3, 2), (3, 3)] {
                    gp.rect_filled(
                        Rect::from_min_size(
                            Pos2::new(
                                grip_rect.left() + dx as f32 * step,
                                grip_rect.top() + dy as f32 * step,
                            ),
                            Vec2::splat(dot_sz),
                        ),
                        0.0,
                        tc.palette.border_bright,
                    );
                }
            });

        if self.add_dialog_open {
            let mut close_add = ctx.input(|i| i.key_pressed(Key::Escape));
            let aw = (410.0 * s_chrome).round();
            let ah = (144.0 * s_chrome).round();
            let adx = screen.left() + ((screen.width() - aw) * 0.5).floor();
            let ady = screen.top() + ((screen.height() - ah) * 0.5).floor();
            let amin = Pos2::new(
                (adx + self.add_drag_offset.x).clamp(screen.left(), (screen.right() - aw).max(screen.left())).round(),
                (ady + self.add_drag_offset.y).clamp(screen.top(), (screen.bottom() - ah).max(screen.top())).round(),
            );
            let arect = Rect::from_min_size(amin, Vec2::new(aw, ah));
            let add_title = loc_or(s.loc, "#ServerBrowser_AddServersTitle", "Add Server");
            egui::Area::new(egui::Id::unique("cs16_vgui_add_server"))
                .order(egui::Order::Tooltip)
                .fixed_pos(arect.min)
                .show(ctx, |ui| {
                    ui.allocate_exact_size(arect.size(), Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(arect, CornerRadius::ZERO, tc.palette.control_bg);
                    paint_tracker_bevel_rect(p, arect, true, tc.palette);
                    let drag = ui.interact(
                        Rect::from_min_size(amin, Vec2::new(aw - 28.0 * s_chrome, 28.0 * s_chrome)),
                        ui.make_persistent_id("sb_add_drag"),
                        Sense::drag(),
                    );
                    if drag.dragged() {
                        self.add_drag_offset += drag.drag_delta();
                    }
                    paint_tracker_label_rect(
                        ui.painter(),
                        runtime,
                        &tc,
                        Rect::from_min_size(
                            amin + Vec2::new((12.0 * s_chrome).round(), (4.0 * s_chrome).round()),
                            Vec2::new((240.0 * s_chrome).round(), (22.0 * s_chrome).round()),
                        ),
                        &add_title,
                        "DefaultBold",
                        TextAlignment::West,
                        tc.palette.bright_text,
                        false,
                        true,
                    );
                    if draw_tracker_close_button(
                        ui,
                        runtime,
                        &tc,
                        "sb_add_x",
                        Rect::from_min_size(
                            Pos2::new(arect.right() - (25.0 * s_chrome).round(), amin.y + (8.0 * s_chrome).round()),
                            Vec2::splat((18.0 * s_chrome).round()),
                        ),
                    ) {
                        close_add = true;
                    }
                    paint_tracker_label_rect(
                        ui.painter(),
                        runtime,
                        &tc,
                        Rect::from_min_size(
                            amin + Vec2::new((16.0 * s_chrome).round(), (34.0 * s_chrome).round()),
                            Vec2::new(aw - (32.0 * s_chrome).round(), (22.0 * s_chrome).round()),
                        ),
                        "Enter the IP address or hostname of the server you wish to add:",
                        "Default",
                        TextAlignment::West,
                        tc.palette.base_text,
                        false,
                        true,
                    );
                    draw_tracker_text_entry_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_add_ip_te",
                        Rect::from_min_size(
                            amin + Vec2::new((16.0 * s_chrome).round(), (62.0 * s_chrome).round()),
                            Vec2::new(aw - (32.0 * s_chrome).round(), (24.0 * s_chrome).round()),
                        ),
                        &mut self.add_addr_input,
                        false,
                        false,
                        true,
                    );
                    let enter = ui.input(|i| i.key_pressed(Key::Enter));
                    let has_addr = !self.add_addr_input.trim().is_empty();
                    let by = amin.y + ah - (34.0 * s_chrome).round();
                    let bh = (24.0 * s_chrome).round();
                    if draw_tracker_button_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_add_fav",
                        Rect::from_min_size(Pos2::new(amin.x + (16.0 * s_chrome).round(), by), Vec2::new((128.0 * s_chrome).round(), bh)),
                        "Add to Favorites",
                        TextAlignment::West,
                        true,
                        has_addr,
                    )
                    .clicked()
                        || (enter && has_addr)
                    {
                        let addr = self.add_addr_input.trim().to_string();
                        self.add_favorite(ServerBrowserEntry {
                            addr: addr.clone(),
                            name: addr,
                            game: "Counter-Strike".into(),
                            map: "-".into(),
                            players: 0,
                            max_players: 32,
                            ping_ms: 0,
                            password: false,
                            vac: true,
                            spectate_port: false,
                        });
                        close_add = true;
                    }
                    if draw_tracker_button_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_add_conn",
                        Rect::from_min_size(Pos2::new(amin.x + (152.0 * s_chrome).round(), by), Vec2::new((100.0 * s_chrome).round(), bh)),
                        "Connect Now",
                        TextAlignment::West,
                        false,
                        has_addr,
                    )
                    .clicked()
                        && has_addr
                    {
                        connect_to = Some(self.add_addr_input.trim().to_string());
                        close_add = true;
                    }
                    if draw_tracker_button_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_add_cancel",
                        Rect::from_min_size(Pos2::new(arect.right() - (88.0 * s_chrome).round(), by), Vec2::new((72.0 * s_chrome).round(), bh)),
                        "Cancel",
                        TextAlignment::West,
                        false,
                        true,
                    )
                    .clicked()
                    {
                        close_add = true;
                    }
                });
            if close_add {
                self.add_dialog_open = false;
            }
        }

        if close_now {
            self.open = false;
        }
        connect_to
    }
}

/// 8-column `CGameListPanel` (`gamelist`) + right VGUI2 scrollbar matching `ServerBrowser.dll`
/// (`FUN_10001080`): Password | Bots | VAC | Players | Servers (N) | Game | Map | Latency.
#[allow(clippy::too_many_arguments)]
fn draw_server_browser_list(
    ui: &mut egui::Ui,
    rt: &mut VguiRuntime,
    tc: &TrackerCtx<'_>,
    loc: Option<&LocalizationTable>,
    rect: Rect,
    entries: &[ServerBrowserEntry],
    selected_row: &mut Option<usize>,
) -> (Option<usize>, Option<usize>) {
    let r = Rect::from_min_max(
        Pos2::new(rect.left().round(), rect.top().round()),
        Pos2::new(rect.right().round(), rect.bottom().round()),
    );
    let s = tc.scale;
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::ZERO, tc.palette.window_bg);
    paint_tracker_bevel_rect(p, r, false, tc.palette);

    let inner = r.shrink(1.0);
    let sb_w = (18.0 * s).round();
    let hdr_h = (18.0 * s).round();
    let row_h = (18.0 * s).round();
    let body_w = (inner.width() - sb_w).max(80.0);

    // Column widths from `CBaseGamesPage` (`FUN_10001080`) & `orig_servers_1280x720.png`:
    // 16 | 20 | 16 | 55 | flex Servers (N) | 112 | 90 | 55
    let fixed_sum = ((16.0 + 20.0 + 16.0 + 55.0 + 112.0 + 90.0 + 55.0) * s).round();
    let servers_w = (body_w - fixed_sum).max((50.0 * s).round());
    let col_widths = [
        (16.0 * s).round(),
        (20.0 * s).round(),
        (16.0 * s).round(),
        (55.0 * s).round(),
        servers_w,
        (112.0 * s).round(),
        (90.0 * s).round(),
        (body_w - ((16.0 + 20.0 + 16.0 + 55.0 + 112.0 + 90.0) * s).round() - servers_w)
            .max((45.0 * s).round()),
    ];
    let col_labels = [
        "".to_string(),
        "".to_string(),
        "".to_string(),
        loc_or(loc, "#ServerBrowser_Players", "Players"),
        format!("{} ({})", loc_or(loc, "#ServerBrowser_Servers", "Servers"), entries.len()),
        loc_or(loc, "#ServerBrowser_Game", "Game"),
        loc_or(loc, "#ServerBrowser_Map", "Map"),
        loc_or(loc, "#ServerBrowser_Latency", "Latency"),
    ];

    let hdr_rect = Rect::from_min_size(inner.min, Vec2::new(body_w, hdr_h));
    let fonts = rt.tracker_scheme.fonts.clone();
    let mut clicked_col = None;
    let mut cx = hdr_rect.left();
    for (idx, (&cw, label)) in col_widths.iter().zip(col_labels.iter()).enumerate() {
        let cr = Rect::from_min_size(Pos2::new(cx, hdr_rect.top()), Vec2::new(cw, hdr_h));
        let resp = ui.interact(cr, ui.make_persistent_id(("sb_hdr", idx)), Sense::click());
        if resp.clicked() {
            clicked_col = Some(idx);
        }
        p.rect_filled(cr, CornerRadius::ZERO, tc.palette.control_bg);
        paint_tracker_bevel_rect(p, cr, true, tc.palette);
        let clipped = p.with_clip_rect(cr);
        if idx == 0 {
            // Miniature lock icon (`icon_password` column)
            let c = cr.center();
            let bw = (7.0 * s).round();
            let bh = (5.0 * s).round();
            let body = Rect::from_center_size(Pos2::new(c.x, c.y + 1.5 * s), Vec2::new(bw, bh));
            clipped.rect_filled(body, 0.0, tc.palette.base_text);
            let sw = (4.0 * s).round();
            clipped.rect_stroke(
                Rect::from_center_size(Pos2::new(c.x, body.top() - 1.5 * s), Vec2::new(sw, 3.0 * s)),
                1.0,
                Stroke::new(1.0, tc.palette.base_text),
                egui::StrokeKind::Inside,
            );
        } else if idx == 1 {
            // Miniature bot 'B' badge (`ServerBrowser.dll` `CGameListPanel` col 1)
            let c = cr.center();
            let badge = Rect::from_center_size(c, Vec2::splat((10.0 * s).round()));
            clipped.rect_filled(badge, 0.0, tc.palette.base_text);
            paint_tracker_label_rect(
                &clipped,
                rt,
                tc,
                badge,
                "B",
                "DefaultSmall",
                TextAlignment::Center,
                tc.palette.window_bg,
                false,
                true,
            );
        } else if idx == 2 {
            // Miniature VAC shield icon (`ServerBrowser.dll` `CGameListPanel` col 2)
            let c = cr.center();
            let sw = (3.5 * s).round();
            let sh = (4.5 * s).round();
            clipped.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(c.x - sw, c.y - sh * 0.7),
                    Pos2::new(c.x + sw, c.y - sh * 0.7),
                    Pos2::new(c.x + sw * 0.7, c.y + sh * 0.3),
                    Pos2::new(c.x, c.y + sh),
                    Pos2::new(c.x - sw * 0.7, c.y + sh * 0.3),
                ],
                tc.palette.base_text,
                Stroke::NONE,
            ));
        } else if !label.is_empty() {
            let tr = cr.shrink2(Vec2::new((4.0 * s).round(), 0.0));
            rt.font_atlas.paint_text_with_scale(
                &clipped,
                tc.ctx,
                tc.vfs,
                &fonts,
                "tracker",
                "DefaultSmall",
                tc.screen_h,
                tr,
                label,
                tc.palette.bright_text,
                TextAlignment::West,
                false,
                s,
            );
        }
        cx += cw;
    }

    let body_rect = Rect::from_min_max(
        Pos2::new(inner.left(), hdr_rect.bottom()),
        Pos2::new(inner.left() + body_w, inner.bottom()),
    );
    let mut double_clicked = None;
    let list_id = ui.make_persistent_id("cs16_sb_gamelist");
    let scroll_id = list_id.with("scroll_off");
    let visible_rows = ((body_rect.height() / row_h).floor() as usize).max(1);
    let max_off = entries.len().saturating_sub(visible_rows);
    let mut scroll_off = tc
        .ctx
        .data(|d| d.get_temp::<usize>(scroll_id))
        .unwrap_or(0)
        .min(max_off);

    let list_hit = ui.interact(body_rect, list_id.with("body"), Sense::click());
    if list_hit.hovered() {
        let wheel = tc.ctx.input(|i| i.smooth_scroll_delta.y);
        if wheel.abs() > 2.0 {
            if wheel < 0.0 {
                scroll_off = (scroll_off + 2).min(max_off);
            } else {
                scroll_off = scroll_off.saturating_sub(2);
            }
        }
    }

    let body_clip = p.with_clip_rect(body_rect);
    for vis_idx in 0..=visible_rows {
        let r_idx = scroll_off + vis_idx;
        let Some(srv) = entries.get(r_idx) else {
            break;
        };
        let ry = body_rect.top() + vis_idx as f32 * row_h;
        let row_r = Rect::from_min_size(Pos2::new(body_rect.left(), ry), Vec2::new(body_w, row_h));
        if list_hit.clicked()
            && let Some(pos) = list_hit.interact_pointer_pos()
            && row_r.contains(pos)
        {
            *selected_row = Some(r_idx);
        }
        if list_hit.double_clicked()
            && let Some(pos) = list_hit.interact_pointer_pos()
            && row_r.contains(pos)
        {
            *selected_row = Some(r_idx);
            double_clicked = Some(r_idx);
        }
        let is_sel = *selected_row == Some(r_idx);
        if is_sel {
            body_clip.rect_filled(row_r, 0.0, tc.palette.selection_bg);
        }
        let fg = if is_sel {
            tc.palette.bright_text
        } else {
            tc.palette.base_text
        };
        let cells = [
            if srv.password { "P".to_string() } else { String::new() },
            String::new(),
            if srv.vac { "V".to_string() } else { String::new() },
            format!("{}/{}", srv.players, srv.max_players),
            srv.name.clone(),
            srv.game.clone(),
            srv.map.clone(),
            srv.ping_ms.to_string(),
        ];
        let mut x = row_r.left();
        for (c_idx, (&cw, val)) in col_widths.iter().zip(cells.iter()).enumerate() {
            if !val.is_empty() {
                let cr = Rect::from_min_size(Pos2::new(x, ry), Vec2::new(cw, row_h));
                let align = if c_idx == 7 {
                    TextAlignment::East
                } else if c_idx < 3 {
                    TextAlignment::Center
                } else {
                    TextAlignment::West
                };
                rt.font_atlas.paint_text_with_scale(
                    &body_clip,
                    tc.ctx,
                    tc.vfs,
                    &fonts,
                    "tracker",
                    "Default",
                    tc.screen_h,
                    cr.shrink2(Vec2::new((4.0 * s).round(), 0.0)),
                    val,
                    fg,
                    align,
                    false,
                    s,
                );
            }
            x += cw;
        }
    }

    // VGUI2 vertical scrollbar on right (`18 * s` wide, `control_dark_bg` track + arrow buttons)
    let sb_rect = Rect::from_min_max(
        Pos2::new(inner.right() - sb_w, inner.top()),
        inner.max,
    );
    p.rect_filled(sb_rect, 0.0, tc.palette.control_dark_bg);
    let up_r = Rect::from_min_size(sb_rect.min, Vec2::splat(sb_w));
    let dn_r = Rect::from_min_size(Pos2::new(sb_rect.left(), sb_rect.bottom() - sb_w), Vec2::splat(sb_w));
    p.rect_filled(up_r, 0.0, tc.palette.control_bg);
    paint_tracker_bevel_rect(p, up_r, true, tc.palette);
    p.rect_filled(dn_r, 0.0, tc.palette.control_bg);
    paint_tracker_bevel_rect(p, dn_r, true, tc.palette);
    let tri_col = tc.palette.base_text;
    let tw = (3.5 * s).round().max(2.5);
    let th = (2.5 * s).round().max(2.0);
    let uc = up_r.center();
    p.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(uc.x, uc.y - th * 0.6),
            Pos2::new(uc.x + tw, uc.y + th * 0.6),
            Pos2::new(uc.x - tw, uc.y + th * 0.6),
        ],
        tri_col,
        Stroke::NONE,
    ));
    let dc = dn_r.center();
    p.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(dc.x - tw, dc.y - th * 0.6),
            Pos2::new(dc.x + tw, dc.y - th * 0.6),
            Pos2::new(dc.x, dc.y + th * 0.6),
        ],
        tri_col,
        Stroke::NONE,
    ));
    if ui.interact(up_r, list_id.with("up"), Sense::click()).clicked() {
        scroll_off = scroll_off.saturating_sub(2);
    }
    if ui.interact(dn_r, list_id.with("dn"), Sense::click()).clicked() {
        scroll_off = (scroll_off + 2).min(max_off);
    }
    let track_h = (dn_r.top() - up_r.bottom()).max(8.0);
    if entries.len() > visible_rows {
        let thumb_h = (track_h * (visible_rows as f32 / entries.len() as f32))
            .clamp((16.0 * s).round(), track_h);
        let frac = if max_off > 0 { scroll_off as f32 / max_off as f32 } else { 0.0 };
        let ty = up_r.bottom() + frac * (track_h - thumb_h);
        let thumb_r = Rect::from_min_size(Pos2::new(sb_rect.left(), ty.round()), Vec2::new(sb_w, thumb_h.round()));
        p.rect_filled(thumb_r, 0.0, tc.palette.control_bg);
        paint_tracker_bevel_rect(p, thumb_r, true, tc.palette);
    }
    tc.ctx.data_mut(|d| d.insert_temp(scroll_id, scroll_off));

    (clicked_col, double_clicked)
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
