//! Server engine: the equivalent of the original game's dedicated server.

pub mod edict;
pub mod map;
pub mod precache;

pub use edict::{DEFAULT_MAX_EDICTS, EDICT_REUSE_DELAY, Edict, EdictId, EdictPool, EntVars};
pub use map::ServerMap;
pub use precache::{MAX_GENERIC, MAX_MODELS, MAX_SOUNDS, PrecacheError, PrecacheTables, res_type};
