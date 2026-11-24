//! Hardened WebSocket-to-UDP Protocol 48 relay allowing browser clients to connect only
//! to explicitly allowlisted GoldSrc servers within per-IP session and packet rate limits.

pub mod policy;
#[cfg(not(target_arch = "wasm32"))]
pub mod server;
pub mod validate;

pub use policy::{
    RelayPolicy, RelayReject, SessionGuard, SessionTracker, TargetAllowlist, TokenBucket,
    parse_relay_target,
};
#[cfg(not(target_arch = "wasm32"))]
pub use server::{RelayHandle, RelayServer};
pub use validate::{is_valid_c2s_protocol48, is_valid_s2c_protocol48};
