//! `.res` / `.scr`-driven Valve VGUI2 screens & dialogs for CS 1.6.

pub mod buy_team_class;
pub mod command_menu;
pub mod dialogs;
pub mod motd;
pub mod server_browser;
pub mod spectator;

pub use buy_team_class::{BuyClassTeamMenuUi, BuyMenuRenderCtx, VguiMenuKind};
pub use command_menu::CommandMenuUi;
pub use dialogs::{
    DemoPlayerUi, DemoTransportState, DemoUiAction, DialogRenderCtx, DisconnectErrorDialog,
    LoadingDialogUi, PlayerListDialogUi, ServerPasswordUi,
};
pub use motd::{MotdRenderCtx, MotdVguiUi};
pub use server_browser::{
    ServerBrowserEntry, ServerBrowserRenderCtx, ServerBrowserTab, ServerBrowserUi, ServerFilter,
};
pub use spectator::{
    SpecSidebarPlayer, SpectatorMode, SpectatorRenderCtx, SpectatorVguiUi,
    collect_spec_sidebar_players,
};
