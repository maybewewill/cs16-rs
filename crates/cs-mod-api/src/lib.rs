//! Versioned modding ABI, manifest format, capability permissions, SHA-256 verification,
//! `.csmod` package archive codec, and ABI v2 ReAPI hook/op types for `cs16-rs`.
//!
//! Shared identically across native servers, native clients, `wasm32` web builds, and WASM
//! guest modules compiled against the `cs16_mod` host import namespace.

pub mod abi;
pub mod archive;
pub mod hash;
pub mod manifest;
pub mod ops;
#[cfg(feature = "services")]
pub mod services;

pub use abi::{
    BombEventCtx, CMD_MOD_REQUEST, CanBuyItemCtx, CanHavePlayerItemCtx, ClientModEntry,
    GrenadeThrowCtx, HookAction, HookId, HookPhase, HostageEventCtx, HudDrawCmd,
    MOD_CHUNK_MAX_DATA, MenuSelectCtx, ModChunkPacket, ModManifestPacket, ModSide,
    PlayerJumpCtx, PlayerKilledCtx, PlayerPostThinkCtx, PlayerPreThinkCtx, PlayerSayCtx,
    PlayerSpawnCtx, PlayerTakeDamageCtx, PlayersShouldCollideCtx, RoundEndCtx, RoundStartCtx,
    USERMSG_MOD_CHUNK, USERMSG_MOD_EVENT, USERMSG_MOD_MANIFEST, WeaponEventCtx,
};
pub use archive::{ArchiveError, CsModArchive};
pub use hash::{sha256_digest, sha256_hex};
pub use manifest::{
    DEFAULT_FUEL_PER_FRAME, DEFAULT_MEMORY_BYTES, MAX_FUEL_PER_FRAME, MAX_MEMORY_CAP,
    ManifestError, ModManifest, Permission, Permissions,
};
pub use ops::{
    AdminFlags, ArmorKind, BarTimeSpec, CustomWeaponSpec, DhudMessageSpec, HttpPostJsonSpec,
    HttpWebhookResult, HudMessageSpec, LangTrSpec, LeaderboardEntry, LeaderboardRankQuery,
    LeaderboardTopQuery, LeaderboardUpsertSpec, MenuCreateSpec, MenuDisplaySpec, MenuItemSpec,
    RpcCallSpec, RpcEmitEventSpec, RpcExportSpec, RpcValue, ScreenFadeSpec, ScreenShakeSpec,
    ShowMotdSpec, TeamKind, VfxOp,
};

/// Minimum supported `api_version` in `mod.toml` (v1 backwards compatibility).
pub const MIN_ABI_VERSION: u32 = 1;

/// Current binary/wire ABI version supported by the host (`api_version = 1` or `2`).
pub const ABI_VERSION: u32 = 2;

/// WebAssembly import module name provided by `cs-mod-host`.
pub const HOST_MODULE: &str = "cs16_mod";

/// Client userinfo key advertising support for extended WASM mod usermsgs and auto-download.
pub const MOD_CAPABILITY_KEY: &str = "_cs16rs_mods";
