//! Versioned modding ABI, manifest format, capability permissions, SHA-256 verification,
//! and `.csmod` package archive codec for `cs16-rs`.
//!
//! Shared identically across native servers, native clients, `wasm32` web builds, and WASM
//! guest modules compiled against the `cs16_mod` host import namespace.

pub mod abi;
pub mod archive;
pub mod hash;
pub mod manifest;

pub use abi::{
    CMD_MOD_REQUEST, ClientModEntry, HookAction, HookId, HookPhase, HudDrawCmd,
    MOD_CHUNK_MAX_DATA, ModChunkPacket, ModManifestPacket, ModSide, USERMSG_MOD_CHUNK,
    USERMSG_MOD_EVENT, USERMSG_MOD_MANIFEST,
};
pub use archive::{ArchiveError, CsModArchive};
pub use hash::{sha256_digest, sha256_hex};
pub use manifest::{
    DEFAULT_FUEL_PER_FRAME, DEFAULT_MEMORY_BYTES, MAX_FUEL_PER_FRAME, MAX_MEMORY_CAP,
    ManifestError, ModManifest, Permission, Permissions,
};

/// Current binary/wire ABI version required in `mod.toml` (`api_version`).
pub const ABI_VERSION: u32 = 1;

/// WebAssembly import module name provided by `cs-mod-host`.
pub const HOST_MODULE: &str = "cs16_mod";

/// Client userinfo key advertising support for extended WASM mod usermsgs and auto-download.
pub const MOD_CAPABILITY_KEY: &str = "_cs16rs_mods";
