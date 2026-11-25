//! Pure-Rust AMX Mod X (`.amxx`) container loader, Pawn 3.2 bytecode VM, and Counter-Strike
//! 1.6 plugin runtime.
//!
//! Zero `unsafe` (`#![forbid(unsafe_code)]`) and compatible with `wasm32-unknown-unknown`.
//! Ported from AMX Mod X GPL-3 sources (`amxxfile.h`, `amxx_loader.cpp`, `amx.h`, `amx.cpp`,
//! `CPlugin.cpp`).

pub mod amxxfile;
pub mod natives;
pub mod runtime;
pub mod vm;

pub use amxxfile::AmxError;
pub use natives::{
    AmxxCsHooks, AmxxExternalRpc, CsxPlayerStats, HAM_HANDLED, HAM_IGNORED, HAM_OVERRIDE,
    HAM_SUPERCEDE, HamFunction, MENU_EXIT, NoCsHooks, PLUGIN_CONTINUE, PLUGIN_HANDLED,
    reapi_member,
};
pub use runtime::{AmxxGame, AmxxPlugin, AmxxPluginResmon, AmxxRuntime, PluginStatus};
pub use vm::PawnVm;
