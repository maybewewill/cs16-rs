//! `.res` / `.scr`-driven Valve VGUI2 screens & dialogs for CS 1.6.

pub mod buy_team_class;
pub mod command_menu;
pub mod dialogs;
pub mod motd;
pub mod server_browser;
pub mod spectator;

pub use buy_team_class::{BuyClassTeamMenuUi, VguiMenuKind};
pub use command_menu::CommandMenuUi;
pub use dialogs::{
    DemoPlayerUi, DisconnectErrorDialog, LoadingDialogUi, PlayerListDialogUi, ServerPasswordUi,
};
pub use motd::MotdVguiUi;
pub use server_browser::{ServerBrowserEntry, ServerBrowserTab, ServerBrowserUi, ServerFilter};
pub use spectator::{SpectatorMode, SpectatorVguiUi};
