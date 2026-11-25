//! Engine-neutral GoldSrc player movement: `pm_shared` ([`pm`]), the BSP hull collision it
//! stands on ([`collision`]) and the physent traces of the engine's `pmovetst.cpp`
//! ([`physent`]). Server and client prediction run this one code; it knows nothing of edicts,
//! sockets or files, depends only on `cs-assets` and `glam`, and builds for wasm.

pub mod collision;
pub mod ljstats;
pub mod physent;
pub mod pm;
