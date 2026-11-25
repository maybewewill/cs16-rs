//! `.res`-driven CS 1.6 Server Browser (`Servers/DialogServerBrowser.res` +
//! `InternetGamesPage.res` + `InternetGamesPage_Filters.res` + `DialogAddServer.res`):
//! all 6 tabs (Internet, Favorites, History, Spectate, LAN, Friends), sortable `ListPanel`,
//! server filters, and LAN UDP discovery.

use std::collections::{BTreeSet, HashMap};
#[cfg(not(target_arch = "wasm32"))]
use std::net::{SocketAddr, UdpSocket};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use cs_assets::scheme::LocalizationTable;
use cs_assets::vgui_res::TextAlignment;
#[cfg(not(target_arch = "wasm32"))]
use cs_proto48::bitbuf::MsgReader;
#[cfg(not(target_arch = "wasm32"))]
use cs_proto48::CONNECTIONLESS_HEADER;
#[cfg(not(target_arch = "wasm32"))]
use cs_proto48::query::{
    S2A_PLAYERS, S2C_CHALLENGE_QUERY, discover_goldsrc_lan_servers, encode_a2s_player_request,
    query_server_info,
};
use cs_vfs::Vfs;
use egui::{Color32, CornerRadius, Key, Pos2, Rect, Sense, Stroke, TextureHandle, Vec2};

use crate::ui::vgui2::controls::{ListColumn, VguiListPanelState};
use crate::ui::vgui2::{
    TrackerCtx, VguiRuntime, draw_tracker_button_rect, draw_tracker_check_button_rect,
    draw_tracker_close_button, draw_tracker_combo_box_rect, draw_tracker_text_entry_rect,
    paint_tracker_bevel_rect, paint_tracker_label_rect,
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

/// Anti-cheat filter mode matching `ServerBrowser.dll:FUN_10002e70` (`SecureFilter` ComboBox).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SecureFilter {
    #[default]
    All,
    SecureOnly,
    InsecureOnly,
}

/// Retail `PingFilter` thresholds (`ServerBrowser.dll` / `serverbrowser_english.txt`).
const PING_FILTER_OPTIONS: &[(&str, &str, u32)] = &[
    ("#ServerBrowser_All", "<All>", 0),
    ("#ServerBrowser_LessThan50", "< 50", 50),
    ("#ServerBrowser_LessThan100", "< 100", 100),
    ("#ServerBrowser_LessThan150", "< 150", 150),
    ("#ServerBrowser_LessThan250", "< 250", 250),
    ("#ServerBrowser_LessThan350", "< 350", 350),
    ("#ServerBrowser_LessThan600", "< 600", 600),
];

#[derive(Debug, Clone, Default)]
pub struct ServerFilter {
    pub game: String,
    pub map: String,
    pub max_ping: u32,
    pub not_full: bool,
    pub has_users: bool,
    pub no_password: bool,
    pub secure_only: bool,
    pub secure_filter: SecureFilter,
    pub valid_steam_account: bool,
}

impl ServerFilter {
    pub fn matches(&self, srv: &ServerBrowserEntry) -> bool {
        self.matches_with_steam(srv, srv.vac)
    }

    /// Evaluate filter predicates with explicit Steam-account association status
    /// (`ServerBrowser.dll:FUN_10002e70`, `InternetGamesPage_Filters.res`).
    pub fn matches_with_steam(&self, srv: &ServerBrowserEntry, has_steam_account: bool) -> bool {
        let g = self.game.trim();
        if !g.is_empty() && !g.eq_ignore_ascii_case("<All>") && !srv.game.eq_ignore_ascii_case(g) {
            return false;
        }
        if !self.map.trim().is_empty()
            && !srv
                .map
                .to_ascii_lowercase()
                .contains(&self.map.trim().to_ascii_lowercase())
        {
            return false;
        }
        if self.max_ping > 0 && srv.ping_ms >= self.max_ping {
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
        let eff_secure = if self.secure_filter != SecureFilter::All {
            self.secure_filter
        } else if self.secure_only {
            SecureFilter::SecureOnly
        } else {
            SecureFilter::All
        };
        match eff_secure {
            SecureFilter::All => {}
            SecureFilter::SecureOnly => {
                if !srv.vac {
                    return false;
                }
            }
            SecureFilter::InsecureOnly => {
                if srv.vac {
                    return false;
                }
            }
        }
        !(self.valid_steam_account && !has_steam_account)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoRetryMode {
    #[default]
    Alert,
    Join,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GameInfoPlayer {
    pub name: String,
    pub score: i32,
    pub duration_secs: f32,
}

/// Modal state for `Servers/DialogGameInfo.res` / `DialogGameInfo_AutoRetry.res`.
#[derive(Debug, Clone)]
pub struct GameInfoDialogState {
    pub entry: ServerBrowserEntry,
    pub players: Vec<GameInfoPlayer>,
    pub auto_retry: bool,
    pub retry_mode: AutoRetryMode,
    pub slot_alert_ready: bool,
    pub status_message: String,
    drag_offset: Vec2,
    last_retry_time: f64,
}

impl GameInfoDialogState {
    pub fn new(entry: ServerBrowserEntry) -> Self {
        let is_full = entry.max_players > 0 && entry.players >= entry.max_players;
        Self {
            entry,
            players: Vec::new(),
            auto_retry: is_full,
            retry_mode: AutoRetryMode::Alert,
            slot_alert_ready: false,
            status_message: String::new(),
            drag_offset: Vec2::ZERO,
            last_retry_time: 0.0,
        }
    }

    pub fn refresh_now(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Ok(sock_addr) = self.entry.addr.parse::<SocketAddr>() {
            if let Ok(q) = query_server_info(sock_addr, Duration::from_millis(250)) {
                self.entry.name = q.name;
                self.entry.game = if q.game.is_empty() {
                    "Counter-Strike".to_string()
                } else {
                    q.game
                };
                self.entry.map = q.map;
                self.entry.players = q.players;
                self.entry.max_players = q.max_players;
                self.entry.ping_ms = q.ping_ms;
                self.entry.password = q.password_protected;
                self.entry.vac = q.vac_secure;
                self.status_message.clear();
            } else {
                self.status_message = "Server is not responding.".to_string();
            }
            if let Ok(players) = query_server_players(sock_addr, Duration::from_millis(250)) {
                self.players = players;
            }
        }
    }

    /// Evaluate `AutoRetry` against the current player/max count (`ServerBrowser.dll` `CDialogGameInfo`).
    /// Returns `Some(addr)` when `AutoRetryJoin` should connect immediately.
    pub fn evaluate_auto_retry(&mut self) -> Option<String> {
        if !self.auto_retry || self.entry.max_players == 0 {
            return None;
        }
        if self.entry.players < self.entry.max_players {
            self.auto_retry = false;
            match self.retry_mode {
                AutoRetryMode::Join => return Some(self.entry.addr.clone()),
                AutoRetryMode::Alert => {
                    self.slot_alert_ready = true;
                    self.status_message = "Player slot available!".to_string();
                }
            }
        }
        None
    }
}

pub struct ServerBrowserUi {
    pub open: bool,
    pub tab: ServerBrowserTab,
    pub filter: ServerFilter,
    pub show_filters: bool,
    pub show_ip_column: bool,
    pub add_dialog_open: bool,
    pub add_addr_input: String,
    pub current_server: Option<ServerBrowserEntry>,
    pub game_info: Option<GameInfoDialogState>,
    pub internet: Vec<ServerBrowserEntry>,
    pub favorites: Vec<ServerBrowserEntry>,
    pub history: Vec<ServerBrowserEntry>,
    pub spectate: Vec<ServerBrowserEntry>,
    pub lan: Vec<ServerBrowserEntry>,
    pub friends: Vec<ServerBrowserEntry>,
    pub master_addr: String,
    pub list_state: VguiListPanelState,
    bots_by_addr: HashMap<String, u8>,
    steam_account_by_addr: HashMap<String, bool>,
    drag_offset: Vec2,
    add_drag_offset: Vec2,
    custom_size: Option<Vec2>,
    sort_col: Option<usize>,
    sort_asc: bool,
}

impl Default for ServerBrowserUi {
    fn default() -> Self {
        Self {
            open: false,
            tab: ServerBrowserTab::Internet,
            filter: ServerFilter::default(),
            show_filters: false,
            show_ip_column: false,
            add_dialog_open: false,
            add_addr_input: "127.0.0.1:27015".to_string(),
            current_server: None,
            game_info: None,
            internet: Vec::new(),
            favorites: Vec::new(),
            history: Vec::new(),
            spectate: Vec::new(),
            lan: Vec::new(),
            friends: Vec::new(),
            master_addr: "127.0.0.1:27010".to_string(),
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
            bots_by_addr: HashMap::new(),
            steam_account_by_addr: HashMap::new(),
            drag_offset: Vec2::ZERO,
            add_drag_offset: Vec2::ZERO,
            custom_size: None,
            sort_col: None,
            sort_asc: true,
        }
    }
}

impl ServerBrowserUi {
    pub fn set_server_bots(&mut self, addr: impl Into<String>, bots: u8) {
        self.bots_by_addr.insert(addr.into(), bots);
    }

    pub fn server_bots(&self, addr: &str) -> u8 {
        self.bots_by_addr.get(addr).copied().unwrap_or(0)
    }

    pub fn set_server_steam_account(&mut self, addr: impl Into<String>, valid: bool) {
        self.steam_account_by_addr.insert(addr.into(), valid);
    }

    pub fn server_has_steam_account(&self, srv: &ServerBrowserEntry) -> bool {
        self.steam_account_by_addr
            .get(&srv.addr)
            .copied()
            .unwrap_or(srv.vac)
    }

    pub fn set_current_server(&mut self, entry: Option<ServerBrowserEntry>) {
        self.current_server = entry;
    }

    pub fn open_game_info(&mut self, entry: ServerBrowserEntry) {
        let mut state = GameInfoDialogState::new(entry);
        state.refresh_now();
        self.game_info = Some(state);
    }

    /// Resolve dialog size matching `ServerBrowser.dll:FUN_100102d0` & `DialogServerBrowser.res`:
    /// defaults to `640x384` scaled by `s_chrome` (clamped to screen), preserving resized `custom_size`.
    pub fn resolve_dialog_size(&self, screen_size: Vec2, s_chrome: f32) -> Vec2 {
        let def_w = screen_size.x.min((640.0 * s_chrome).round());
        let def_h = screen_size.y.min((384.0 * s_chrome).round());
        let base_sz = self.custom_size.unwrap_or(Vec2::new(def_w, def_h));
        Vec2::new(
            base_sz.x.clamp(def_w, screen_size.x).round(),
            base_sz.y.clamp(def_h, screen_size.y).round(),
        )
    }

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

    fn raw_tab_entries(&self) -> &[ServerBrowserEntry] {
        match self.tab {
            ServerBrowserTab::Internet => &self.internet,
            ServerBrowserTab::Favorites => &self.favorites,
            ServerBrowserTab::History => &self.history,
            ServerBrowserTab::Spectate => &self.spectate,
            ServerBrowserTab::Lan => &self.lan,
            ServerBrowserTab::Friends => &self.friends,
        }
    }

    pub fn active_entries(&self) -> Vec<&ServerBrowserEntry> {
        self.raw_tab_entries()
            .iter()
            .filter(|s| self.filter.matches_with_steam(s, self.server_has_steam_account(s)))
            .collect()
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
            let discovered = discover_goldsrc_lan_servers(Duration::from_millis(300));
            let mut entries = Vec::with_capacity(discovered.len());
            for q in discovered {
                self.bots_by_addr.insert(q.addr.clone(), q.bots);
                self.steam_account_by_addr
                    .insert(q.addr.clone(), q.protocol >= 48 || q.vac_secure);
                entries.push(ServerBrowserEntry {
                    addr: q.addr,
                    name: q.name,
                    game: if q.game.is_empty() {
                        "Counter-Strike".to_string()
                    } else {
                        q.game
                    },
                    map: q.map,
                    players: q.players,
                    max_players: q.max_players,
                    ping_ms: q.ping_ms,
                    password: q.password_protected,
                    vac: q.vac_secure,
                    spectate_port: q.server_type == b'p',
                });
            }
            self.lan = entries;
        }
        self.sync_rows();
    }

    pub fn refresh_from_master(&mut self, spectate_only: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Ok(master) = self.master_addr.parse::<SocketAddr>() else {
                if spectate_only {
                    self.spectate.clear();
                } else {
                    self.internet.clear();
                }
                self.sync_rows();
                return;
            };
            let filter = if spectate_only {
                "\\gamedir\\cstrike\\proxy\\1"
            } else {
                "\\gamedir\\cstrike"
            };
            let mut entries = Vec::new();
            if let Ok(addrs) = cs_master::client::query_master_servers(
                master,
                0xFF,
                filter,
                Duration::from_millis(500),
            ) {
                for v4 in addrs.into_iter().take(64) {
                    let entry = match query_server_info(
                        SocketAddr::V4(v4),
                        Duration::from_millis(200),
                    ) {
                        Ok(q) => {
                            self.bots_by_addr.insert(q.addr.clone(), q.bots);
                            self.steam_account_by_addr
                                .insert(q.addr.clone(), q.protocol >= 48 || q.vac_secure);
                            ServerBrowserEntry {
                                addr: q.addr,
                                name: q.name,
                                game: if q.game.is_empty() {
                                    "Counter-Strike".to_string()
                                } else {
                                    q.game
                                },
                                map: q.map,
                                players: q.players,
                                max_players: q.max_players,
                                ping_ms: q.ping_ms,
                                password: q.password_protected,
                                vac: q.vac_secure,
                                spectate_port: spectate_only || q.server_type == b'p',
                            }
                        }
                        Err(_) => {
                            let addr_str = v4.to_string();
                            self.bots_by_addr.insert(addr_str.clone(), 0);
                            self.steam_account_by_addr.insert(addr_str.clone(), true);
                            ServerBrowserEntry {
                                addr: addr_str.clone(),
                                name: addr_str,
                                game: "Counter-Strike".to_string(),
                                map: String::new(),
                                players: 0,
                                max_players: 0,
                                ping_ms: 0,
                                password: false,
                                vac: false,
                                spectate_port: spectate_only,
                            }
                        }
                    };
                    entries.push(entry);
                }
            }
            if spectate_only {
                self.spectate = entries;
            } else {
                self.internet = entries;
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = spectate_only;
        self.sync_rows();
    }

    pub fn refresh_active_tab(&mut self) {
        match self.tab {
            ServerBrowserTab::Lan => self.refresh_lan(),
            ServerBrowserTab::Internet => self.refresh_from_master(false),
            ServerBrowserTab::Spectate => self.refresh_from_master(true),
            _ => self.sync_rows(),
        }
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
            runtime.load_res(v, "servers/DialogGameInfo_AutoRetry.res");
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

        // `ServerBrowser.dll:FUN_100102d0` & `DialogServerBrowser.res`: default first-run
        // `custom_size` is `640x384` scaled by `s_chrome`, clamped to screen.
        let min_w = screen.width().min((640.0 * s_chrome).round());
        let min_h = screen.height().min((384.0 * s_chrome).round());
        let resolved_sz = self.resolve_dialog_size(screen.size(), s_chrome);
        let dlg_w = resolved_sz.x;
        let dlg_h = resolved_sz.y;
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

        let mut close_now = ctx.input(|i| i.key_pressed(Key::Escape))
            && !self.add_dialog_open
            && self.game_info.is_none();
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
                    1 => self.server_bots(&a.addr).cmp(&self.server_bots(&b.addr)),
                    2 => a.vac.cmp(&b.vac),
                    3 => a.players.cmp(&b.players),
                    4 => a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()),
                    5 => a.game.to_ascii_lowercase().cmp(&b.game.to_ascii_lowercase()),
                    6 => a.map.to_ascii_lowercase().cmp(&b.map.to_ascii_lowercase()),
                    7 => a.ping_ms.cmp(&b.ping_ms),
                    _ => a.addr.cmp(&b.addr),
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
                    (104.0 * s_chrome).round()
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

                let list_act = draw_server_browser_list(
                    ui,
                    runtime,
                    &tc,
                    s.loc,
                    list_rect,
                    &entries,
                    &self.bots_by_addr,
                    self.show_ip_column,
                    &mut self.list_state.selected_row,
                );
                if let Some(col) = list_act.clicked_col {
                    if self.sort_col == Some(col) {
                        self.sort_asc = !self.sort_asc;
                    } else {
                        self.sort_col = Some(col);
                        self.sort_asc = true;
                    }
                }
                if list_act.toggle_ip_col {
                    self.show_ip_column = !self.show_ip_column;
                }
                if let Some(row_idx) = list_act.connect_row
                    && let Some(srv) = entries.get(row_idx)
                {
                    if srv.max_players > 0 && srv.players >= srv.max_players {
                        self.open_game_info(srv.clone());
                    } else {
                        self.record_history(srv.clone());
                        connect_to = Some(srv.addr.clone());
                    }
                }
                if let Some(row_idx) = list_act.info_row
                    && let Some(srv) = entries.get(row_idx)
                {
                    self.open_game_info(srv.clone());
                }
                if let Some(row_idx) = list_act.fav_row
                    && let Some(srv) = entries.get(row_idx)
                {
                    self.add_favorite(srv.clone());
                }

                if self.show_filters {
                    let fy0 = list_rect.bottom() + (6.0 * s_chrome).round();
                    let row_step = (26.0 * s_chrome).round();
                    let left_x = sheet_rect.left() + (8.0 * s_chrome).round();
                    let lbl_w1 = (44.0 * s_chrome).round();
                    let ctrl_w1 = (152.0 * s_chrome).round();
                    let ctrl_x1 = left_x + lbl_w1 + (4.0 * s_chrome).round();

                    // Col 1: `GameFilter` & `MapFilter` (`InternetGamesPage_Filters.res:171-210`)
                    paint_tracker_label_rect(
                        ui.painter(),
                        runtime,
                        &tc,
                        Rect::from_min_size(Pos2::new(left_x, fy0), Vec2::new(lbl_w1, btn_h)),
                        &loc_or(s.loc, "#ServerBrowser_Game", "Game"),
                        "Default",
                        TextAlignment::East,
                        tc.palette.base_text,
                        false,
                        true,
                    );
                    let mut games_set: BTreeSet<String> = BTreeSet::new();
                    games_set.insert("Counter-Strike".to_string());
                    for srv in self.raw_tab_entries() {
                        if !srv.game.trim().is_empty() {
                            games_set.insert(srv.game.clone());
                        }
                    }
                    let all_lbl = loc_or(s.loc, "#ServerBrowser_All", "<All>");
                    let mut game_items = vec![all_lbl];
                    game_items.extend(games_set);
                    let mut game_idx = if self.filter.game.trim().is_empty() {
                        0
                    } else {
                        game_items
                            .iter()
                            .position(|g| g.eq_ignore_ascii_case(self.filter.game.trim()))
                            .unwrap_or(0)
                    };
                    if draw_tracker_combo_box_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_filter_game",
                        Rect::from_min_size(Pos2::new(ctrl_x1, fy0), Vec2::new(ctrl_w1, btn_h)),
                        &game_items,
                        &mut game_idx,
                        true,
                    ) {
                        self.filter.game = if game_idx == 0 {
                            String::new()
                        } else {
                            game_items[game_idx].clone()
                        };
                    }

                    paint_tracker_label_rect(
                        ui.painter(),
                        runtime,
                        &tc,
                        Rect::from_min_size(Pos2::new(left_x, fy0 + row_step), Vec2::new(lbl_w1, btn_h)),
                        &loc_or(s.loc, "#ServerBrowser_Map", "Map"),
                        "Default",
                        TextAlignment::East,
                        tc.palette.base_text,
                        false,
                        true,
                    );
                    draw_tracker_text_entry_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_filter_map",
                        Rect::from_min_size(Pos2::new(ctrl_x1, fy0 + row_step), Vec2::new(ctrl_w1, btn_h)),
                        &mut self.filter.map,
                        false,
                        false,
                        true,
                    );

                    // Col 2: `PingFilter` & `SecureFilter` (`InternetGamesPage_Filters.res:211-250`)
                    let lbl_x2 = ctrl_x1 + ctrl_w1 + (8.0 * s_chrome).round();
                    let lbl_w2 = (70.0 * s_chrome).round();
                    let ctrl_x2 = lbl_x2 + lbl_w2 + (4.0 * s_chrome).round();
                    let ctrl_w2 = (108.0 * s_chrome).round();

                    paint_tracker_label_rect(
                        ui.painter(),
                        runtime,
                        &tc,
                        Rect::from_min_size(Pos2::new(lbl_x2, fy0), Vec2::new(lbl_w2, btn_h)),
                        &loc_or(s.loc, "#ServerBrowser_Latency", "Latency"),
                        "Default",
                        TextAlignment::East,
                        tc.palette.base_text,
                        false,
                        true,
                    );
                    let ping_items: Vec<String> = PING_FILTER_OPTIONS
                        .iter()
                        .map(|(tok, fb, _)| loc_or(s.loc, tok, fb))
                        .collect();
                    let mut ping_idx = PING_FILTER_OPTIONS
                        .iter()
                        .position(|(_, _, ms)| *ms == self.filter.max_ping)
                        .unwrap_or(0);
                    if draw_tracker_combo_box_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_filter_ping",
                        Rect::from_min_size(Pos2::new(ctrl_x2, fy0), Vec2::new(ctrl_w2, btn_h)),
                        &ping_items,
                        &mut ping_idx,
                        true,
                    ) {
                        self.filter.max_ping = PING_FILTER_OPTIONS[ping_idx].2;
                    }

                    paint_tracker_label_rect(
                        ui.painter(),
                        runtime,
                        &tc,
                        Rect::from_min_size(Pos2::new(lbl_x2, fy0 + row_step), Vec2::new(lbl_w2, btn_h)),
                        &loc_or(s.loc, "#ServerBrowser_AntiCheat", "Anti-cheat"),
                        "Default",
                        TextAlignment::East,
                        tc.palette.base_text,
                        false,
                        true,
                    );
                    let sec_items = vec![
                        loc_or(s.loc, "#ServerBrowser_All", "<All>"),
                        loc_or(s.loc, "#ServerBrowser_SecureOnly", "Secure"),
                        loc_or(s.loc, "#ServerBrowser_InsecureOnly", "Not secure"),
                    ];
                    let mut sec_idx = match self.filter.secure_filter {
                        SecureFilter::All => {
                            if self.filter.secure_only { 1 } else { 0 }
                        }
                        SecureFilter::SecureOnly => 1,
                        SecureFilter::InsecureOnly => 2,
                    };
                    if draw_tracker_combo_box_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_filter_secure",
                        Rect::from_min_size(Pos2::new(ctrl_x2, fy0 + row_step), Vec2::new(ctrl_w2, btn_h)),
                        &sec_items,
                        &mut sec_idx,
                        true,
                    ) {
                        self.filter.secure_filter = match sec_idx {
                            1 => SecureFilter::SecureOnly,
                            2 => SecureFilter::InsecureOnly,
                            _ => SecureFilter::All,
                        };
                        self.filter.secure_only = sec_idx == 1;
                    }

                    // Col 3: Checkboxes (`InternetGamesPage_Filters.res:251-326`)
                    let cx0 = ctrl_x2 + ctrl_w2 + (12.0 * s_chrome).round();
                    let cw = (sheet_rect.right() - cx0 - (6.0 * s_chrome).round()).max(120.0 * s_chrome);
                    let chk_h = (22.0 * s_chrome).round();
                    let chk_step = (23.0 * s_chrome).round();
                    draw_tracker_check_button_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_filt_full",
                        Rect::from_min_size(Pos2::new(cx0, fy0), Vec2::new(cw, chk_h)),
                        &mut self.filter.not_full,
                        &loc_or(s.loc, "#ServerBrowser_ServerNotFull", "Server not full"),
                        true,
                    );
                    draw_tracker_check_button_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_filt_users",
                        Rect::from_min_size(Pos2::new(cx0, fy0 + chk_step), Vec2::new(cw, chk_h)),
                        &mut self.filter.has_users,
                        &loc_or(s.loc, "#ServerBrowser_HasUsersPlaying", "Has users playing"),
                        true,
                    );
                    draw_tracker_check_button_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_filt_nopw",
                        Rect::from_min_size(Pos2::new(cx0, fy0 + chk_step * 2.0), Vec2::new(cw, chk_h)),
                        &mut self.filter.no_password,
                        &loc_or(s.loc, "#ServerBrowser_IsNotPasswordProtected", "Is not password protected"),
                        true,
                    );
                    draw_tracker_check_button_rect(
                        ui,
                        runtime,
                        &tc,
                        "sb_filt_steam_acct",
                        Rect::from_min_size(Pos2::new(cx0, fy0 + chk_step * 3.0), Vec2::new(cw, chk_h)),
                        &mut self.filter.valid_steam_account,
                        &loc_or(s.loc, "#ServerBrowser_ValidSteamAccount", "Has associated Steam account"),
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

                let mut summary_parts = vec![if self.filter.game.trim().is_empty() {
                    format!("{};", loc_or(s.loc, "#ServerBrowser_All", "<All>"))
                } else {
                    format!("{};", self.filter.game.trim())
                }];
                if !self.filter.map.trim().is_empty() {
                    summary_parts.push(format!("map: {};", self.filter.map.trim()));
                }
                if self.filter.max_ping > 0 {
                    summary_parts.push(format!("< {};", self.filter.max_ping));
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
                match self.filter.secure_filter {
                    SecureFilter::SecureOnly => summary_parts.push("secure;".into()),
                    SecureFilter::InsecureOnly => summary_parts.push("not secure;".into()),
                    SecureFilter::All if self.filter.secure_only => summary_parts.push("secure;".into()),
                    SecureFilter::All => {}
                }
                if self.filter.valid_steam_account {
                    summary_parts.push("steam account;".into());
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
                    self.refresh_active_tab();
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

                    if let Some(cur) = self.current_server.clone() {
                        let cur_w = (142.0 * s_chrome).round();
                        rx -= btn_gap + cur_w;
                        let cur_lbl = loc_or(
                            s.loc,
                            "#ServerBrowser_AddCurrentServer",
                            "Add Current Server",
                        );
                        if draw_tracker_button_rect(
                            ui,
                            runtime,
                            &tc,
                            "sb_add_current_btn",
                            Rect::from_min_size(Pos2::new(rx, btn_y), Vec2::new(cur_w, btn_h)),
                            &cur_lbl,
                            TextAlignment::West,
                            false,
                            true,
                        )
                        .clicked()
                        {
                            self.add_favorite(cur);
                        }
                    }
                } else {
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
                        self.refresh_active_tab();
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

        if let Some(mut info_state) = self.game_info.take() {
            let mut keep_open = true;
            if let Some(addr) = draw_game_info_dialog(ctx, runtime, &tc, s.loc, &mut info_state, &mut keep_open) {
                self.record_history(info_state.entry.clone());
                connect_to = Some(addr);
                keep_open = false;
            }
            if keep_open {
                self.game_info = Some(info_state);
            }
        }

        if close_now {
            self.open = false;
        }
        connect_to
    }
}

struct ServerListActions {
    clicked_col: Option<usize>,
    connect_row: Option<usize>,
    info_row: Option<usize>,
    fav_row: Option<usize>,
    toggle_ip_col: bool,
}

/// `Servers/DialogGameInfo.res` / `DialogGameInfo_AutoRetry.res` server info & auto-retry modal.
fn draw_game_info_dialog(
    ctx: &egui::Context,
    runtime: &mut VguiRuntime,
    tc: &TrackerCtx<'_>,
    loc: Option<&LocalizationTable>,
    state: &mut GameInfoDialogState,
    keep_open: &mut bool,
) -> Option<String> {
    let s = tc.scale;
    let screen = ctx.content_rect();
    let gw = screen.width().min((416.0 * s).round());
    let gh = screen.height().min((500.0 * s).round());
    let gx = screen.left() + ((screen.width() - gw) * 0.5).floor();
    let gy = screen.top() + ((screen.height() - gh) * 0.5).floor();
    let gmin = Pos2::new(
        (gx + state.drag_offset.x)
            .clamp(screen.left(), (screen.right() - gw).max(screen.left()))
            .round(),
        (gy + state.drag_offset.y)
            .clamp(screen.top(), (screen.bottom() - gh).max(screen.top()))
            .round(),
    );
    let grect = Rect::from_min_size(gmin, Vec2::new(gw, gh));
    let mut join_addr: Option<String> = None;

    let now = ctx.input(|i| i.time);
    if state.auto_retry && (now - state.last_retry_time) >= 2.0 {
        state.last_retry_time = now;
        state.refresh_now();
        if let Some(addr) = state.evaluate_auto_retry() {
            return Some(addr);
        }
    }
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        *keep_open = false;
        return None;
    }

    let title = format!(
        "{} - {}",
        loc_or(loc, "#ServerBrowser_GameInfoTitle", "Game Info"),
        state.entry.name
    );

    egui::Area::new(egui::Id::unique("cs16_vgui_game_info"))
        .order(egui::Order::Tooltip)
        .fixed_pos(gmin)
        .show(ctx, |ui| {
            ui.allocate_exact_size(grect.size(), Sense::hover());
            let p = ui.painter();
            p.rect_filled(grect, CornerRadius::ZERO, tc.palette.control_bg);
            paint_tracker_bevel_rect(p, grect, true, tc.palette);

            let drag = ui.interact(
                Rect::from_min_size(gmin, Vec2::new(gw - 28.0 * s, 28.0 * s)),
                ui.make_persistent_id("sb_gi_drag"),
                Sense::drag(),
            );
            if drag.dragged() {
                state.drag_offset += drag.drag_delta();
            }
            paint_tracker_label_rect(
                ui.painter(),
                runtime,
                tc,
                Rect::from_min_size(gmin + Vec2::new((10.0 * s).round(), (4.0 * s).round()), Vec2::new(gw - (38.0 * s).round(), (22.0 * s).round())),
                &title,
                "DefaultBold",
                TextAlignment::West,
                tc.palette.bright_text,
                false,
                true,
            );
            if draw_tracker_close_button(
                ui,
                runtime,
                tc,
                "sb_gi_x",
                Rect::from_min_size(
                    Pos2::new(grect.right() - (25.0 * s).round(), gmin.y + (8.0 * s).round()),
                    Vec2::splat((18.0 * s).round()),
                ),
            ) {
                *keep_open = false;
            }

            let vac_txt = if state.entry.vac {
                loc_or(loc, "#ServerBrowser_Secure", "Secure")
            } else {
                loc_or(loc, "#ServerBrowser_NotSecure", "Not secure")
            };
            let players_txt = format!("{} / {}", state.entry.players, state.entry.max_players);
            let ping_txt = state.entry.ping_ms.to_string();
            let rows = [
                (42.0, loc_or(loc, "#ServerBrowser_ServerName", "Name:"), state.entry.name.as_str()),
                (66.0, loc_or(loc, "#ServerBrowser_IPAddressLabel", "IP Address:"), state.entry.addr.as_str()),
                (90.0, loc_or(loc, "#ServerBrowser_GameLabel", "Game:"), state.entry.game.as_str()),
                (114.0, loc_or(loc, "#ServerBrowser_MapLabel", "Map:"), state.entry.map.as_str()),
                (138.0, loc_or(loc, "#ServerBrowser_PlayersLabel", "Players:"), players_txt.as_str()),
                (162.0, loc_or(loc, "#ServerBrowser_ValveAntiCheat", "Valve Anti-Cheat:"), vac_txt.as_str()),
                (186.0, loc_or(loc, "#ServerBrowser_LatencyLabel", "Latency:"), ping_txt.as_str()),
            ];
            for (y, lbl, val) in rows {
                paint_tracker_label_rect(
                    ui.painter(),
                    runtime,
                    tc,
                    Rect::from_min_size(gmin + Vec2::new((16.0 * s).round(), (y * s).round()), Vec2::new((108.0 * s).round(), (20.0 * s).round())),
                    &lbl,
                    "Default",
                    TextAlignment::East,
                    tc.palette.dim_text,
                    false,
                    true,
                );
                paint_tracker_label_rect(
                    ui.painter(),
                    runtime,
                    tc,
                    Rect::from_min_size(gmin + Vec2::new((128.0 * s).round(), (y * s).round()), Vec2::new(gw - (144.0 * s).round(), (20.0 * s).round())),
                    val,
                    "Default",
                    TextAlignment::West,
                    tc.palette.base_text,
                    false,
                    true,
                );
            }

            // Player list (`24, 228, 368, 140` scaled)
            let pl_rect = Rect::from_min_size(
                gmin + Vec2::new((24.0 * s).round(), (228.0 * s).round()),
                Vec2::new((gw - 48.0 * s).max(120.0), (140.0 * s).round()),
            );
            ui.painter().rect_filled(pl_rect, CornerRadius::ZERO, tc.palette.window_bg);
            paint_tracker_bevel_rect(ui.painter(), pl_rect, false, tc.palette);
            let pl_inner = pl_rect.shrink(1.0);
            let hdr_h = (18.0 * s).round();
            let col_w = [
                (pl_inner.width() * 0.56).round(),
                (pl_inner.width() * 0.20).round(),
                (pl_inner.width() * 0.24).round(),
            ];
            let hdr_labels = [
                loc_or(loc, "#ServerBrowser_PlayerName", "Player Name"),
                loc_or(loc, "#ServerBrowser_Score", "Score"),
                loc_or(loc, "#ServerBrowser_Time", "Time"),
            ];
            let mut hx = pl_inner.left();
            for (cw, lbl) in col_w.iter().zip(hdr_labels.iter()) {
                let cr = Rect::from_min_size(Pos2::new(hx, pl_inner.top()), Vec2::new(*cw, hdr_h));
                ui.painter().rect_filled(cr, CornerRadius::ZERO, tc.palette.control_bg);
                paint_tracker_bevel_rect(ui.painter(), cr, true, tc.palette);
                paint_tracker_label_rect(
                    ui.painter(),
                    runtime,
                    tc,
                    cr.shrink2(Vec2::new((4.0 * s).round(), 0.0)),
                    lbl,
                    "DefaultSmall",
                    TextAlignment::West,
                    tc.palette.bright_text,
                    false,
                    true,
                );
                hx += *cw;
            }
            let pl_body = Rect::from_min_max(Pos2::new(pl_inner.left(), pl_inner.top() + hdr_h), pl_inner.max);
            let pl_clip = ui.painter().with_clip_rect(pl_body);
            let row_h = (18.0 * s).round();
            for (idx, pl) in state.players.iter().enumerate() {
                let ry = pl_body.top() + idx as f32 * row_h;
                if ry + row_h > pl_body.bottom() {
                    break;
                }
                let mins = (pl.duration_secs.max(0.0) as u32) / 60;
                let secs = (pl.duration_secs.max(0.0) as u32) % 60;
                let vals = [
                    pl.name.clone(),
                    pl.score.to_string(),
                    format!("{mins}:{secs:02}"),
                ];
                let mut px = pl_body.left();
                for (cw, val) in col_w.iter().zip(vals.iter()) {
                    let cr = Rect::from_min_size(Pos2::new(px, ry), Vec2::new(*cw, row_h));
                    paint_tracker_label_rect(
                        &pl_clip,
                        runtime,
                        tc,
                        cr.shrink2(Vec2::new((4.0 * s).round(), 0.0)),
                        val,
                        "Default",
                        TextAlignment::West,
                        tc.palette.base_text,
                        false,
                        true,
                    );
                    px += *cw;
                }
            }

            let info_msg = if !state.status_message.is_empty() {
                state.status_message.clone()
            } else if state.auto_retry {
                match state.retry_mode {
                    AutoRetryMode::Alert => loc_or(
                        loc,
                        "#ServerBrowser_AlertWhenSlotIsFree",
                        "You will be alerted when a player slot is free on the server.",
                    ),
                    AutoRetryMode::Join => loc_or(
                        loc,
                        "#ServerBrowser_JoinWhenSlotIsFree",
                        "You will join the server as soon as a player slot is free.",
                    ),
                }
            } else {
                loc_or(
                    loc,
                    "#ServerBrowser_PressJoinToConnect",
                    "Press 'Join Game' to connect to the server.",
                )
            };
            paint_tracker_label_rect(
                ui.painter(),
                runtime,
                tc,
                Rect::from_min_size(gmin + Vec2::new((26.0 * s).round(), (371.0 * s).round()), Vec2::new(gw - (52.0 * s).round(), (24.0 * s).round())),
                &info_msg,
                "Default",
                TextAlignment::West,
                tc.palette.base_text,
                false,
                true,
            );

            let by = gmin.y + (400.0 * s).round();
            let bw = (80.0 * s).round();
            let bh = (24.0 * s).round();
            if draw_tracker_button_rect(
                ui,
                runtime,
                tc,
                "sb_gi_autoretry",
                Rect::from_min_size(Pos2::new(gmin.x + (28.0 * s).round(), by), Vec2::new(bw, bh)),
                &loc_or(loc, "#ServerBrowser_AutoRetry", "Auto-Retry"),
                TextAlignment::West,
                state.auto_retry,
                true,
            )
            .clicked()
            {
                state.auto_retry = !state.auto_retry;
                state.slot_alert_ready = false;
            }
            if draw_tracker_button_rect(
                ui,
                runtime,
                tc,
                "sb_gi_join",
                Rect::from_min_size(Pos2::new(gmin.x + (122.0 * s).round(), by), Vec2::new(bw, bh)),
                &loc_or(loc, "#ServerBrowser_JoinGame", "Join Game"),
                TextAlignment::West,
                true,
                true,
            )
            .clicked()
            {
                join_addr = Some(state.entry.addr.clone());
            }
            if draw_tracker_button_rect(
                ui,
                runtime,
                tc,
                "sb_gi_refresh",
                Rect::from_min_size(Pos2::new(gmin.x + (218.0 * s).round(), by), Vec2::new(bw, bh)),
                &loc_or(loc, "#ServerBrowser_Refresh", "Refresh"),
                TextAlignment::West,
                false,
                true,
            )
            .clicked()
            {
                state.refresh_now();
            }
            if draw_tracker_button_rect(
                ui,
                runtime,
                tc,
                "sb_gi_close",
                Rect::from_min_size(Pos2::new(gmin.x + (312.0 * s).round(), by), Vec2::new(bw, bh)),
                &loc_or(loc, "#ServerBrowser_Close", "Close"),
                TextAlignment::West,
                false,
                true,
            )
            .clicked()
            {
                *keep_open = false;
            }

            if state.auto_retry {
                let mut alert_sel = state.retry_mode == AutoRetryMode::Alert;
                if draw_tracker_check_button_rect(
                    ui,
                    runtime,
                    tc,
                    "sb_gi_alert_mode",
                    Rect::from_min_size(gmin + Vec2::new((27.0 * s).round(), (435.0 * s).round()), Vec2::new(gw - (48.0 * s).round(), (22.0 * s).round())),
                    &mut alert_sel,
                    &loc_or(
                        loc,
                        "#ServerBrowser_AlertMeWhenSlotOpens",
                        "Alert me when a player slot is available on the server.",
                    ),
                    true,
                ) {
                    state.retry_mode = AutoRetryMode::Alert;
                }
                let mut join_sel = state.retry_mode == AutoRetryMode::Join;
                if draw_tracker_check_button_rect(
                    ui,
                    runtime,
                    tc,
                    "sb_gi_join_mode",
                    Rect::from_min_size(gmin + Vec2::new((27.0 * s).round(), (458.0 * s).round()), Vec2::new(gw - (48.0 * s).round(), (22.0 * s).round())),
                    &mut join_sel,
                    &loc_or(
                        loc,
                        "#ServerBrowser_JoinWhenSlotOpens",
                        "Join the server as soon as a player slot is available.",
                    ),
                    true,
                ) {
                    state.retry_mode = AutoRetryMode::Join;
                }
            }
        });
    join_addr
}

#[cfg(not(target_arch = "wasm32"))]
fn query_server_players(addr: SocketAddr, timeout: Duration) -> Result<Vec<GameInfoPlayer>, String> {
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    sock.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    let mut challenge = -1i32;
    let mut buf = [0u8; 4096];
    for _ in 0..2 {
        let req = encode_a2s_player_request(challenge);
        sock.send_to(&req, addr).map_err(|e| e.to_string())?;
        let (len, _) = sock.recv_from(&mut buf).map_err(|e| e.to_string())?;
        let mut r = MsgReader::new(&buf[..len]);
        let hdr = r.read_i32().map_err(|e| e.to_string())?;
        if hdr != CONNECTIONLESS_HEADER {
            return Err("bad connectionless header".into());
        }
        let kind = r.read_u8().map_err(|e| e.to_string())?;
        if kind == S2C_CHALLENGE_QUERY {
            challenge = r.read_i32().map_err(|e| e.to_string())?;
            continue;
        }
        if kind == S2A_PLAYERS {
            let count = r.read_u8().map_err(|e| e.to_string())?;
            let mut players = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                let _idx = r.read_u8().map_err(|e| e.to_string())?;
                let name = r.read_str().map_err(|e| e.to_string())?;
                let score = r.read_i32().map_err(|e| e.to_string())?;
                let duration_secs = r.read_f32().map_err(|e| e.to_string())?;
                players.push(GameInfoPlayer {
                    name,
                    score,
                    duration_secs,
                });
            }
            return Ok(players);
        }
        return Err(format!("unexpected opcode {kind}"));
    }
    Err("player query challenge failed".into())
}

/// 8-column (+ toggleable col 8 `IPAddr`, width 64) `CGameListPanel` (`gamelist`) + right VGUI2
/// scrollbar matching `ServerBrowser.dll:FUN_10001080`.
#[allow(clippy::too_many_arguments)]
fn draw_server_browser_list(
    ui: &mut egui::Ui,
    rt: &mut VguiRuntime,
    tc: &TrackerCtx<'_>,
    loc: Option<&LocalizationTable>,
    rect: Rect,
    entries: &[ServerBrowserEntry],
    bots_by_addr: &HashMap<String, u8>,
    show_ip_column: bool,
    selected_row: &mut Option<usize>,
) -> ServerListActions {
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

    // Column widths from `CBaseGamesPage` (`FUN_10001080`):
    // 16 | 20 | 16 | 55 | flex Servers (N) | 112 | 90 | 55 [| 64 IPAddr]
    let ip_w = if show_ip_column { 64.0 } else { 0.0 };
    let fixed_sum = ((16.0 + 20.0 + 16.0 + 55.0 + 112.0 + 90.0 + 55.0 + ip_w) * s).round();
    let servers_w = (body_w - fixed_sum).max((50.0 * s).round());
    let latency_w = (body_w
        - ((16.0 + 20.0 + 16.0 + 55.0 + 112.0 + 90.0 + ip_w) * s).round()
        - servers_w)
        .max((45.0 * s).round());
    let mut col_widths = vec![
        (16.0 * s).round(),
        (20.0 * s).round(),
        (16.0 * s).round(),
        (55.0 * s).round(),
        servers_w,
        (112.0 * s).round(),
        (90.0 * s).round(),
        latency_w,
    ];
    let mut col_labels = vec![
        "".to_string(),
        "".to_string(),
        "".to_string(),
        loc_or(loc, "#ServerBrowser_Players", "Players"),
        format!("{} ({})", loc_or(loc, "#ServerBrowser_Servers", "Servers"), entries.len()),
        loc_or(loc, "#ServerBrowser_Game", "Game"),
        loc_or(loc, "#ServerBrowser_Map", "Map"),
        loc_or(loc, "#ServerBrowser_Latency", "Latency"),
    ];
    if show_ip_column {
        col_widths.push((64.0 * s).round());
        col_labels.push(loc_or(loc, "#ServerBrowser_IPAddress", "IP Address"));
    }

    let hdr_rect = Rect::from_min_size(inner.min, Vec2::new(body_w, hdr_h));
    let fonts = rt.tracker_scheme.fonts.clone();
    let mut clicked_col = None;
    let mut header_right_clicked = false;
    let mut cx = hdr_rect.left();
    for (idx, (&cw, label)) in col_widths.iter().zip(col_labels.iter()).enumerate() {
        let cr = Rect::from_min_size(Pos2::new(cx, hdr_rect.top()), Vec2::new(cw, hdr_h));
        let resp = ui.interact(cr, ui.make_persistent_id(("sb_hdr", idx)), Sense::click());
        if resp.clicked() {
            clicked_col = Some(idx);
        }
        if resp.secondary_clicked() {
            header_right_clicked = true;
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

    let mut toggle_ip_col = false;
    let hdr_menu_id = ui.make_persistent_id("sb_hdr_col_menu");
    let hdr_menu_pos_id = hdr_menu_id.with("pos");
    if header_right_clicked
        && let Some(pos) = tc.ctx.input(|i| i.pointer.interact_pos())
    {
        tc.ctx.data_mut(|d| d.insert_temp(hdr_menu_pos_id, pos));
    }
    if let Some(menu_pos) = tc.ctx.data(|d| d.get_temp::<Pos2>(hdr_menu_pos_id)) {
        let mut close_menu = false;
        let area_resp = egui::Area::new(hdr_menu_id.with("area"))
            .order(egui::Order::Tooltip)
            .fixed_pos(menu_pos)
            .show(tc.ctx, |ui| {
                let w = (128.0 * s).round();
                let h = (24.0 * s).round();
                let (item_r, item_resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
                ui.painter().rect_filled(item_r, CornerRadius::ZERO, tc.palette.control_bg);
                paint_tracker_bevel_rect(ui.painter(), item_r, true, tc.palette);
                let mut chk = show_ip_column;
                if draw_tracker_check_button_rect(
                    ui,
                    rt,
                    tc,
                    "sb_hdr_toggle_ip",
                    item_r,
                    &mut chk,
                    &loc_or(loc, "#ServerBrowser_IPAddress", "IP Address"),
                    true,
                ) || item_resp.clicked()
                {
                    toggle_ip_col = true;
                    close_menu = true;
                }
            });
        if close_menu
            || (tc.ctx.input(|i| i.pointer.any_pressed())
                && !area_resp.response.contains_pointer())
        {
            tc.ctx.data_mut(|d| d.remove::<Pos2>(hdr_menu_pos_id));
        }
    }

    let body_rect = Rect::from_min_max(
        Pos2::new(inner.left(), hdr_rect.bottom()),
        Pos2::new(inner.left() + body_w, inner.bottom()),
    );
    let mut double_clicked = None;
    let mut info_row = None;
    let mut fav_row = None;
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

    let row_menu_id = list_id.with("row_ctx_menu");
    let row_menu_data_id = row_menu_id.with("data");
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
        if list_hit.secondary_clicked()
            && let Some(pos) = list_hit.interact_pointer_pos()
            && row_r.contains(pos)
        {
            *selected_row = Some(r_idx);
            tc.ctx
                .data_mut(|d| d.insert_temp(row_menu_data_id, (r_idx, pos)));
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
        let bots = bots_by_addr.get(&srv.addr).copied().unwrap_or(0);
        let mut cells = vec![
            if srv.password { "P".to_string() } else { String::new() },
            if bots > 0 { bots.to_string() } else { String::new() },
            if srv.vac { "V".to_string() } else { String::new() },
            format!("{}/{}", srv.players, srv.max_players),
            srv.name.clone(),
            srv.game.clone(),
            srv.map.clone(),
            srv.ping_ms.to_string(),
        ];
        if show_ip_column {
            cells.push(srv.addr.clone());
        }
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

    if let Some((menu_row, menu_pos)) = tc.ctx.data(|d| d.get_temp::<(usize, Pos2)>(row_menu_data_id))
        && menu_row < entries.len()
    {
        let mut close_menu = false;
        let area_resp = egui::Area::new(row_menu_id.with("area"))
            .order(egui::Order::Tooltip)
            .fixed_pos(menu_pos)
            .show(tc.ctx, |ui| {
                let w = (168.0 * s).round();
                let item_h = (22.0 * s).round();
                let items = [
                    loc_or(loc, "#ServerBrowser_ConnectToServer", "Connect to server"),
                    loc_or(loc, "#ServerBrowser_ViewServerInfo", "View server info"),
                    loc_or(loc, "#ServerBrowser_AddServerToFavorites", "Add server to favorites"),
                ];
                let box_r = Rect::from_min_size(menu_pos, Vec2::new(w, item_h * items.len() as f32));
                ui.painter().rect_filled(box_r, CornerRadius::ZERO, tc.palette.control_bg);
                paint_tracker_bevel_rect(ui.painter(), box_r, true, tc.palette);
                for (idx, lbl) in items.iter().enumerate() {
                    let (ir, iresp) = ui.allocate_exact_size(Vec2::new(w, item_h), Sense::click());
                    if iresp.hovered() {
                        ui.painter().rect_filled(ir, 0.0, tc.palette.selection_bg);
                    }
                    paint_tracker_label_rect(
                        ui.painter(),
                        rt,
                        tc,
                        ir.shrink2(Vec2::new((6.0 * s).round(), 0.0)),
                        lbl,
                        "Default",
                        TextAlignment::West,
                        if iresp.hovered() { tc.palette.bright_text } else { tc.palette.base_text },
                        false,
                        true,
                    );
                    if iresp.clicked() {
                        match idx {
                            0 => double_clicked = Some(menu_row),
                            1 => info_row = Some(menu_row),
                            _ => fav_row = Some(menu_row),
                        }
                        close_menu = true;
                    }
                }
            });
        if close_menu
            || (tc.ctx.input(|i| i.pointer.any_pressed())
                && !area_resp.response.contains_pointer())
        {
            tc.ctx.data_mut(|d| d.remove::<(usize, Pos2)>(row_menu_data_id));
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

    ServerListActions {
        clicked_col,
        connect_row: double_clicked,
        info_row,
        fav_row,
        toggle_ip_col,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry(addr: &str, game: &str, map: &str, ping_ms: u32, vac: bool) -> ServerBrowserEntry {
        ServerBrowserEntry {
            addr: addr.to_string(),
            name: format!("Server {addr}"),
            game: game.to_string(),
            map: map.to_string(),
            players: 10,
            max_players: 16,
            ping_ms,
            password: false,
            vac,
            spectate_port: false,
        }
    }

    #[test]
    fn default_size_is_640x384_scaled_and_clamped_and_persists_custom() {
        let mut sb = ServerBrowserUi::default();
        assert_eq!(sb.resolve_dialog_size(Vec2::new(1280.0, 720.0), 1.0), Vec2::new(640.0, 384.0));
        assert_eq!(sb.resolve_dialog_size(Vec2::new(1920.0, 1080.0), 1.5), Vec2::new(960.0, 576.0));
        assert_eq!(sb.resolve_dialog_size(Vec2::new(500.0, 300.0), 1.0), Vec2::new(500.0, 300.0));

        sb.custom_size = Some(Vec2::new(820.0, 500.0));
        assert_eq!(sb.resolve_dialog_size(Vec2::new(1280.0, 720.0), 1.0), Vec2::new(820.0, 500.0));
        assert!(!sb.show_ip_column, "col 8 IPAddr must be hidden by default");
    }

    #[test]
    fn filters_handle_game_ping_3_state_secure_and_steam_account() {
        let mut sb = ServerBrowserUi::default();
        let s_cs_vac_low = sample_entry("10.0.0.1:27015", "Counter-Strike", "de_dust2", 42, true);
        let s_cs_insec_mid = sample_entry("10.0.0.2:27015", "Counter-Strike", "de_inferno", 95, false);
        let s_hl_vac_hi = sample_entry("10.0.0.3:27015", "Half-Life", "crossfire", 180, true);
        sb.internet = vec![s_cs_vac_low.clone(), s_cs_insec_mid.clone(), s_hl_vac_hi.clone()];
        sb.set_server_steam_account("10.0.0.1:27015", true);
        sb.set_server_steam_account("10.0.0.2:27015", false);
        sb.set_server_steam_account("10.0.0.3:27015", true);

        sb.filter.game = "Counter-Strike".into();
        assert_eq!(sb.active_entries().len(), 2);

        sb.filter.max_ping = 50;
        assert_eq!(sb.active_entries().len(), 1);
        assert_eq!(sb.active_entries()[0].addr, "10.0.0.1:27015");

        sb.filter.max_ping = 0;
        sb.filter.secure_filter = SecureFilter::InsecureOnly;
        assert_eq!(sb.active_entries().len(), 1);
        assert_eq!(sb.active_entries()[0].addr, "10.0.0.2:27015");

        sb.filter.secure_filter = SecureFilter::SecureOnly;
        assert_eq!(sb.active_entries().len(), 1);
        assert_eq!(sb.active_entries()[0].addr, "10.0.0.1:27015");

        sb.filter.secure_filter = SecureFilter::All;
        sb.filter.valid_steam_account = true;
        assert_eq!(sb.active_entries().len(), 1);
        assert_eq!(sb.active_entries()[0].addr, "10.0.0.1:27015");
    }

    #[test]
    fn bots_column_toggleable_ip_col_favorites_and_game_info_auto_retry_render() {
        let mut sb = ServerBrowserUi {
            open: true,
            tab: ServerBrowserTab::Favorites,
            ..Default::default()
        };
        let mut full_srv = sample_entry("192.168.1.50:27015", "Counter-Strike", "de_nuke", 18, true);
        full_srv.players = 16;
        full_srv.max_players = 16;
        sb.set_server_bots(&full_srv.addr, 4);
        assert_eq!(sb.server_bots(&full_srv.addr), 4);

        sb.set_current_server(Some(full_srv.clone()));
        sb.add_favorite(full_srv.clone());
        sb.show_filters = true;
        sb.show_ip_column = true;
        sb.open_game_info(full_srv);
        assert!(sb.game_info.as_ref().unwrap().auto_retry);

        // Auto-retry transitions cleanly when a slot opens.
        if let Some(ref mut gi) = sb.game_info {
            gi.retry_mode = AutoRetryMode::Join;
            gi.entry.players = 15;
            assert_eq!(gi.evaluate_auto_retry().as_deref(), Some("192.168.1.50:27015"));
        }

        let mut rt = VguiRuntime::new();
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0))),
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            sb.show(
                ui.ctx(),
                &mut rt,
                &ServerBrowserRenderCtx {
                    vfs: None,
                    loc: None,
                    palette: VguiPalette::from(cs_assets::scheme::VguiColors::default()),
                    steam_icon: None,
                },
            );
        });
        out.textures_delta.clear();
        assert!(!out.shapes.is_empty());
    }
}
