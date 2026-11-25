//! Pure-Rust, zero-`unsafe`, `wasm32`-compatible, fuel-metered Python 3 plugin runtime for
//! `cs16-rs` (`mods/*.py` and `mods/<dir>/main.py`).
//!
//! Server owners can drop a `.py` file directly into `mods/`, edit it live, and hot-reload in
//! milliseconds while preserving `mod.storage` — with zero external Python installation, zero C
//! FFI, strict per-call instruction fuel caps (`OutOfFuel` with Python line tracebacks for
//! `ModFaultDump`), and full `cs16` ABI v2 bindings (`Player`, `vfx`, `Menu`, `CustomWeapon`,
//! `LangCatalog`, `AdminRegistry`, `LeaderboardStore`, `RpcBus`, and non-blocking webhooks).

#![forbid(unsafe_code)]

pub mod ast;
pub mod bindings;
pub mod error;
pub mod json_codec;
pub mod lexer;
pub mod parser;
pub mod reload;
pub mod runtime;
pub mod stdlib;
pub mod value;
pub mod vm;

pub use error::{CallFrame, PyError};
#[cfg(not(target_arch = "wasm32"))]
pub use reload::PyHotReloader;
pub use reload::{PyDiscoveredMod, discover_python_mods, load_python_mod_file};
pub use runtime::{
    PyModInstance, PyModRuntime, PySourceSyntaxCheck, PySyntaxReport, default_python_permissions,
    synthesize_manifest_from_py,
};
pub use value::PyPluginMeta;

/// Registers the built-in pure-Rust [`PyModRuntime`] on a [`cs_mod_host::ServerModHost`] so it can
/// load, execute, and hot-reload `.py` plugins alongside `.wasm` and `.amxx` plugins.
pub fn attach_python_runtime(host: &mut cs_mod_host::ServerModHost) {
    host.set_python_runtime(Box::new(PyModRuntime::new()));
}
