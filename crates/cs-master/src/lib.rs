//! Pure-Rust GoldSrc Master Server implementation (`cs-master`).
//!
//! Implements the GoldSrc Master Server Query (`0x31`) and Heartbeat (`q` / `0` / `b`)
//! protocols, server registry with TTL expiration and filtering, a UDP + optional HTTP/1.1
//! server, and a multi-page query client.
//!
//! Protocol and registry modules are `wasm32-unknown-unknown` compatible; native socket
//! I/O lives in [`server`] and [`client`].

pub mod protocol;
pub mod registry;

#[cfg(not(target_arch = "wasm32"))]
pub mod client;
#[cfg(not(target_arch = "wasm32"))]
pub mod server;
