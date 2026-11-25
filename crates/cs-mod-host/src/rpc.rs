//! Polyglot Inter-Plugin RPC & Event Bus shared across WASM, Python, and AMXX plugins.

pub use cs_mod_api::services::{
    MultiForwardDesc, PluginRuntimeKind, RpcBus, RpcEventSubscriber, RpcExportEntry,
};
pub use cs_mod_api::{RpcCallSpec, RpcEmitEventSpec, RpcExportSpec, RpcValue};
