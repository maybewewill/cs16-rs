//! `#![forbid(unsafe_code)]` implementation of Valve's **GoldSrc Protocol 48**
//! (Counter-Strike 1.6 / Half-Life 1 network wire protocol), ported deterministically from
//! `ReHLDS` and `ReGameDLL_CS` (licensing: `docs/LICENSING.md`).

pub mod bitbuf;
pub mod compress;
pub mod crypto;
pub mod delta;
pub mod delta_offsets;
pub mod handshake;
pub mod info;
pub mod messages;
pub mod netchan;
pub mod query;
pub mod usermsg;

/// GoldSrc network protocol version (`PROTOCOL_VERSION` in `ReHLDS/engine/net.h`).
pub const PROTOCOL_VERSION: u32 = 48;

/// Connectionless packet 4-byte prefix (`0xFFFF_FFFF`).
pub const CONNECTIONLESS_HEADER: i32 = -1;

/// Split UDP packet 4-byte prefix (`0xFFFF_FFFE`).
pub const SPLIT_PACKET_HEADER: i32 = -2;
