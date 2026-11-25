//! GoldSrc Protocol 48 HLTV (Half-Life TV) proxy, time-delay buffer,
//! auto-director, and optional `.dem` recorder.
//!
//! Architecture follows the GoldSrc HLTV model (`rehlds/common/hltv.h`,
//! `rehlds/engine/sv_main.cpp` signon & netchan framing) implemented purely in
//! safe Rust over [`cs_transport::PacketTransport`].

pub mod delay;
pub mod proxy;
