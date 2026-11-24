//! Deterministic WebAssembly modding host (`wasmi`), pre/post `GameDll`/`EngineApi` hook
//! chains, sandboxed server/client host natives, `.csmod` package loader, and SHA-256
//! verified auto-download for `cs16-rs`.
//!
//! Zero `unsafe`: works identically on native server/client builds and `wasm32-unknown-unknown`.

pub mod client_host;
pub mod hookchain;
pub mod natives;
pub mod package;
pub mod runtime;
pub mod server_host;
pub mod transfer;

pub use client_host::{ClientModHost, ClientModStatus};
pub use package::{LoadedPackage, PackageError, discover_packages, load_package_csmod, load_package_dir};
pub use runtime::{ModCallOutcome, ModInstance, ModRuntime, RuntimeError, WasmiRuntime};
pub use server_host::{ModStatus, ModdedGame, ServerModHost};
pub use transfer::{ApprovalPolicy, ClientModTransfer, PendingModApproval, TransferError};

/// Compiles a WebAssembly Text (`.wat`) module into a binary `.wasm` payload.
pub fn compile_wat_to_wasm(wat_src: &str) -> Result<Vec<u8>, String> {
    wat::parse_str(wat_src).map_err(|e| format!("wat compile error: {e}"))
}
