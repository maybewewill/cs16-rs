//! Deterministic WebAssembly modding host (`wasmi`), pre/post `GameDll`/`EngineApi` & ReAPI
//! hook chains, sandboxed server/client host natives (ABI v1 & v2), WASI preview1 shims,
//! paginated `MenuManager`, multilingual `LangCatalog`, RBAC `AdminRegistry`, polyglot `RpcBus`,
//! `nvault` + $O(\log N)$ `LeaderboardStore`, capability-gated webhooks, `.csmod` package loader,
//! and SHA-256 verified auto-download for `cs16-rs`.
//!
//! Zero `unsafe`: works identically on native server/client builds and `wasm32-unknown-unknown`.

pub mod admins;
pub mod amxx_import;
pub mod client_host;
pub mod crash_dump;
pub mod hookchain;
pub mod lang;
pub mod menu;
pub mod natives;
pub mod package;
pub mod rpc;
pub mod runtime;
pub mod server_host;
pub mod storage;
pub mod transfer;
pub mod webhook;

pub use admins::{AdminFlags, AdminRegistry};
pub use client_host::{ClientModHost, ClientModStatus};
pub use lang::{LANG_PLAYER, LANG_SERVER, LangCatalog};
pub use menu::{MenuManager, MenuSelectOutcome, RenderedMenu};
pub use package::{
    LoadedPackage, PackageError, discover_packages, load_package_csmod, load_package_dir,
};
pub use rpc::RpcBus;
pub use runtime::{
    ModCallOutcome, ModInstance, ModRuntime, PythonModRuntime, RuntimeError, WasmiRuntime,
};
pub use server_host::{ModInfo, ModStatus, ModdedGame, PluginResmon, ServerModHost};
pub use storage::{LeaderboardEntry, LeaderboardStore, NVaultStore};
pub use transfer::{ApprovalPolicy, ClientModTransfer, PendingModApproval, TransferError};
pub use webhook::WebhookClient;

/// Compiles a WebAssembly Text (`.wat`) module into a binary `.wasm` payload.
pub fn compile_wat_to_wasm(wat_src: &str) -> Result<Vec<u8>, String> {
    wat::parse_str(wat_src).map_err(|e| format!("wat compile error: {e}"))
}
