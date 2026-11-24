//! `.res`-driven CS 1.6 Server Browser (`Servers/DialogServerBrowser.res` +
//! `InternetGamesPage.res` + `InternetGamesPage_Filters.res` + `DialogAddServer.res`):
//! all 6 tabs (Internet, Favorites, History, Spectate, LAN, Friends), sortable `ListPanel`,
//! server filters, and LAN UDP discovery.

#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use cs_assets::scheme::LocalizationTable;
#[cfg(not(target_arch = "wasm32"))]
use cs_proto48::query::discover_goldsrc_lan_servers;
use cs_vfs::Vfs;
use egui::{Key, Pos2, TextureHandle, Vec2};

use crate::ui::vgui2::controls::{ListColumn, VguiListPanelState, draw_vgui_list_panel};
use crate::ui::vgui2::{TrackerButtonCfg, VguiRuntime};
use crate::ui::vgui_assets::{TrackerWindowCfg, show_tracker_window};
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
    Internet,
    Favorites,
    History,
    Spectate,
    #[default]
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
}

impl Default for ServerBrowserUi {
    fn default() -> Self {
        Self {
            open: false,
            tab: ServerBrowserTab::Lan,
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
                    ListColumn { name: "Game".into(), width: 85 },
                    ListColumn { name: "Players".into(), width: 64 },
                    ListColumn { name: "Map".into(), width: 95 },
                    ListColumn { name: "Latency".into(), width: 58 },
                ],
                ..Default::default()
            },
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

        let win_sz = runtime.doc_window_size("servers/DialogServerBrowser.res", [604.0, 384.0]);
        let mut still_open = self.open;
        let mut connect_to: Option<String> = None;

        let tab_labels = [
            (ServerBrowserTab::Internet, loc_or(s.loc, "#ServerBrowser_InternetTab", "Internet")),
            (ServerBrowserTab::Favorites, loc_or(s.loc, "#ServerBrowser_FavoritesTab", "Favorites")),
            (ServerBrowserTab::History, loc_or(s.loc, "#ServerBrowser_HistoryTab", "History")),
            (ServerBrowserTab::Spectate, loc_or(s.loc, "#ServerBrowser_SpectateTab", "Spectate")),
            (ServerBrowserTab::Lan, loc_or(s.loc, "#ServerBrowser_LanTab", "Lan")),
            (ServerBrowserTab::Friends, loc_or(s.loc, "#ServerBrowser_FriendsTab", "Friends")),
        ];

        show_tracker_window(
            ctx,
            runtime,
            &mut still_open,
            &TrackerWindowCfg {
                id: "cs16_vgui_server_browser",
                title: "Servers",
                default_pos: Pos2::new(100.0, 70.0),
                default_size: Vec2::new(win_sz[0].max(560.0), win_sz[1].max(340.0)),
                min_size: Vec2::new(540.0, 320.0),
                resizable: true,
                palette: s.palette,
                steam_icon: s.steam_icon,
                vfs: s.vfs,
            },
            |ui, rt| {
                ui.horizontal(|ui| {
                    for (t, label) in &tab_labels {
                        if rt.draw_tracker_tab(
                            ui,
                            s.vfs,
                            s.palette,
                            &format!("sb_tab_{label}"),
                            label,
                            self.tab == *t,
                        ) {
                            self.tab = *t;
                            self.list_state.selected_row = None;
                        }
                    }
                });

                let entries: Vec<ServerBrowserEntry> =
                    self.active_entries().into_iter().cloned().collect();
                if let Some(row_idx) = draw_vgui_list_panel(
                    ui,
                    "cs16_sb_list",
                    &mut self.list_state,
                    220.0,
                    s.palette,
                ) && let Some(srv) = entries.get(row_idx)
                {
                    self.record_history(srv.clone());
                    connect_to = Some(srv.addr.clone());
                }

                if self.show_filters {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Map:");
                        ui.add(egui::TextEdit::singleline(&mut self.filter.map).desired_width(90.0));
                        ui.checkbox(&mut self.filter.not_full, "Not full");
                        ui.checkbox(&mut self.filter.has_users, "Has players");
                        ui.checkbox(&mut self.filter.no_password, "No password");
                        ui.checkbox(&mut self.filter.secure_only, "VAC secure");
                    });
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let filt_lbl = loc_or(s.loc, "#ServerBrowser_ChangeFilters", "Change filters");
                    if rt
                        .draw_tracker_button(
                            ui,
                            s.vfs,
                            s.palette,
                            &TrackerButtonCfg {
                                id: "sb_filters_btn",
                                label: &filt_lbl,
                                min_size: Vec2::new(108.0, 24.0),
                                default_btn: false,
                                enabled: true,
                            },
                        )
                        .clicked()
                    {
                        self.show_filters = !self.show_filters;
                    }
                    let add_lbl = loc_or(s.loc, "#ServerBrowser_AddServer", "Add Server");
                    if rt
                        .draw_tracker_button(
                            ui,
                            s.vfs,
                            s.palette,
                            &TrackerButtonCfg {
                                id: "sb_add_btn",
                                label: &add_lbl,
                                min_size: Vec2::new(96.0, 24.0),
                                default_btn: false,
                                enabled: true,
                            },
                        )
                        .clicked()
                    {
                        self.add_dialog_open = true;
                    }
                    let ref_lbl = loc_or(s.loc, "#ServerBrowser_Refresh", "Refresh");
                    if rt
                        .draw_tracker_button(
                            ui,
                            s.vfs,
                            s.palette,
                            &TrackerButtonCfg {
                                id: "sb_refresh_btn",
                                label: &ref_lbl,
                                min_size: Vec2::new(76.0, 24.0),
                                default_btn: false,
                                enabled: true,
                            },
                        )
                        .clicked()
                    {
                        self.refresh_lan();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let sel = self
                            .list_state
                            .selected_row
                            .and_then(|i| entries.get(i))
                            .cloned();
                        let conn_lbl = loc_or(s.loc, "#ServerBrowser_Connect", "Connect");
                        if rt
                            .draw_tracker_button(
                                ui,
                                s.vfs,
                                s.palette,
                                &TrackerButtonCfg {
                                    id: "sb_connect_btn",
                                    label: &conn_lbl,
                                    min_size: Vec2::new(76.0, 24.0),
                                    default_btn: true,
                                    enabled: sel.is_some(),
                                },
                            )
                            .clicked()
                            && let Some(srv) = sel
                        {
                            self.record_history(srv.clone());
                            connect_to = Some(srv.addr);
                        }
                    });
                });

                ui.add_space(4.0);
                let status = if entries.is_empty() {
                    "No servers responded on local network query.".to_string()
                } else {
                    format!("{} server(s) listed.", entries.len())
                };
                ui.label(egui::RichText::new(status).small().color(s.palette.dim_text));
            },
        );

        if self.add_dialog_open {
            let mut add_open = self.add_dialog_open;
            let mut close_add = false;
            show_tracker_window(
                ctx,
                runtime,
                &mut add_open,
                &TrackerWindowCfg {
                    id: "cs16_vgui_add_server",
                    title: &loc_or(s.loc, "#ServerBrowser_AddServersTitle", "Add Server"),
                    default_pos: Pos2::new(220.0, 170.0),
                    default_size: Vec2::new(410.0, 140.0),
                    min_size: Vec2::new(360.0, 125.0),
                    resizable: false,
                    palette: s.palette,
                    steam_icon: s.steam_icon,
                    vfs: s.vfs,
                },
                |ui, rt| {
                    ui.label("Enter the IP address or hostname of the server you wish to add:");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.add_addr_input)
                            .desired_width(370.0),
                    );
                    let enter = resp.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if rt
                            .draw_tracker_button(
                                ui,
                                s.vfs,
                                s.palette,
                                &TrackerButtonCfg {
                                    id: "sb_add_cancel",
                                    label: "Cancel",
                                    min_size: Vec2::new(72.0, 24.0),
                                    default_btn: false,
                                    enabled: true,
                                },
                            )
                            .clicked()
                        {
                            close_add = true;
                        }
                        if rt
                            .draw_tracker_button(
                                ui,
                                s.vfs,
                                s.palette,
                                &TrackerButtonCfg {
                                    id: "sb_add_connect",
                                    label: "Connect Now",
                                    min_size: Vec2::new(98.0, 24.0),
                                    default_btn: false,
                                    enabled: !self.add_addr_input.trim().is_empty(),
                                },
                            )
                            .clicked()
                        {
                            connect_to = Some(self.add_addr_input.trim().to_string());
                            close_add = true;
                        }
                        if (rt
                            .draw_tracker_button(
                                ui,
                                s.vfs,
                                s.palette,
                                &TrackerButtonCfg {
                                    id: "sb_add_fav",
                                    label: "Add to Favorites",
                                    min_size: Vec2::new(124.0, 24.0),
                                    default_btn: true,
                                    enabled: !self.add_addr_input.trim().is_empty(),
                                },
                            )
                            .clicked()
                            || enter)
                            && !self.add_addr_input.trim().is_empty()
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
                    });
                },
            );
            if !add_open || close_add || ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.add_dialog_open = false;
            }
        }

        if !still_open {
            self.open = false;
        }
        connect_to
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
