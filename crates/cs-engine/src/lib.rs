//! Server engine: the equivalent of the original game's dedicated server.

pub mod edict;

pub use edict::{DEFAULT_MAX_EDICTS, EDICT_REUSE_DELAY, Edict, EdictId, EdictPool, EntVars};
