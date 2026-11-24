//! `.res`-driven CS 1.6 Server Browser (`Servers/DialogServerBrowser.res` +
//! `InternetGamesPage.res` + `InternetGamesPage_Filters.res` + `DialogAddServer.res`):
//! all 6 tabs (Internet, Favorites, History, Spectate, LAN, Friends), sortable `ListPanel`,
//! server filters, and LAN UDP discovery.

#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use cs_proto48::query::discover_goldsrc_lan_servers;
use cs_vfs::Vfs;
use egui::{CornerRadius, Pos2, RichText, TextureHandle, Vec2};

use crate::ui::vgui2::controls::{ListColumn, VguiListPanelState, draw_vgui_list_panel};
use crate::ui::vgui2::VguiRuntime;
use crate::ui::vgui_assets::show_steam2003_window;
use crate::ui::vgui_theme::{VguiPalette, vgui_tab};

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
        vfs: Option<&Vfs>,
        palette: VguiPalette,
        steam_icon: Option<&TextureHandle>,
    ) -> Option<String> {
        if !self.open {
            return None;
        }
        if let Some(v) = vfs {
            runtime.load_res(v, "servers/DialogServerBrowser.res");
            runtime.load_res(v, "servers/InternetGamesPage.res");
        }
        self.sync_rows();

        let win_sz = runtime.doc_window_size("servers/DialogServerBrowser.res", [620.0, 410.0]);
        let mut still_open = self.open;
        let mut connect_to: Option<String> = None;

        show_steam2003_window(
            ctx,
            "cs16_vgui_server_browser",
            "Servers",
            &mut still_open,
            Pos2::new(100.0, 70.0),
            Vec2::new(win_sz[0].max(540.0), win_sz[1].max(320.0)),
            Vec2::new(540.0, 320.0),
            true,
            palette,
            steam_icon,
            |ui| {
                ui.horizontal(|ui| {
                    for (t, label) in ServerBrowserTab::ALL {
                        if vgui_tab(ui, label, self.tab == t, palette) {
                            self.tab = t;
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
                    palette,
                ) && let Some(srv) = entries.get(row_idx)
                {
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
                    if ui
                        .add(egui::Button::new("Change Filters").corner_radius(CornerRadius::ZERO))
                        .clicked()
                    {
                        self.show_filters = !self.show_filters;
                    }
                    if ui
                        .add(egui::Button::new("Add Server").corner_radius(CornerRadius::ZERO))
                        .clicked()
                    {
                        self.add_dialog_open = true;
                    }
                    if ui
                        .add(egui::Button::new("Refresh").corner_radius(CornerRadius::ZERO))
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
                        if ui
                            .add_enabled(
                                sel.is_some(),
                                egui::Button::new(RichText::new("Connect").color(palette.bright_control_text))
                                    .corner_radius(CornerRadius::ZERO),
                            )
                            .clicked()
                            && let Some(s) = sel
                        {
                            self.record_history(s.clone());
                            connect_to = Some(s.addr);
                        }
                    });
                });

                if self.add_dialog_open {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("IP / Host:");
                        ui.text_edit_singleline(&mut self.add_addr_input);
                        if ui.button("Add to Favorites").clicked() {
                            let addr = self.add_addr_input.trim().to_string();
                            if !addr.is_empty() {
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
                            }
                            self.add_dialog_open = false;
                        }
                        if ui.button("Connect Now").clicked() {
                            connect_to = Some(self.add_addr_input.trim().to_string());
                            self.add_dialog_open = false;
                        }
                    });
                }
            },
        );

        if !still_open {
            self.open = false;
        }
        connect_to
    }
}
