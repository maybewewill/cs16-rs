//! Server engine: the equivalent of the original game's dedicated server.

pub mod client;
pub mod edict;
pub mod game;
pub mod map;
pub mod oob;
pub mod precache;
pub mod server;
mod signon;

pub use client::{ClientState, SignonStage};
pub use edict::{DEFAULT_MAX_EDICTS, EDICT_REUSE_DELAY, Edict, EdictId, EdictPool, EntVars};
pub use game::{EngineApi, GameDll, entity_state_from_vars};
pub use map::ServerMap;
pub use oob::{
    CHALLENGE_BUCKET_SECS, ChallengeIssuer, ConnectRequest, MAX_INFO_STRING, MAX_NAME_BYTES,
    OobContext, OobLimiter, OobOutcome, handle_oob, sanitize_name,
};
pub use precache::{MAX_GENERIC, MAX_MODELS, MAX_SOUNDS, PrecacheError, PrecacheTables, res_type};
pub use server::{Server, ServerConfig};
