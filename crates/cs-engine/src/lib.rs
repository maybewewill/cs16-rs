//! Server engine: the equivalent of the original game's dedicated server.

mod api_entities;
mod api_messages;
pub mod client;
pub mod collision;
mod connect;
pub mod console;
pub mod edict;
pub mod frame;
pub mod game;
mod lifecycle;
pub mod map;
pub mod mapents;
mod movestep;
pub mod oob;
pub mod precache;
pub mod server;
mod signon;
mod physics;
mod physics_move;
mod physics_water;
mod rng;
mod touch;
mod trace_ents;
mod unlag;
pub mod usercmd;
mod world;

pub use client::{ClientState, MAX_CLIENT_INCOMING_STREAM, MAX_REFUSED_STREAMS, SignonStage};
pub use api_messages::{MAX_MESSAGE_TEXT, MsgDest};
pub use collision::WorldCollision;
pub use console::{ConsoleOutcome, MAX_CONSOLE_LINE, valid_map_name};
pub use edict::{
    DEFAULT_MAX_EDICTS, EDICT_REUSE_DELAY, Edict, EdictId, EdictPool, EntVars, MAX_EDICTS_WIRE,
};
pub use frame::{
    ClientFrame, FramePlayer, FrameRing, MAX_DATAGRAM, MAX_PACKET_ENTITIES, UPDATE_BACKUP, clamp_rate,
    update_interval,
};
pub use game::{EngineApi, GameDll, entity_state_from_vars};
pub use lifecycle::{
    NAME_CHANGE_MIN_INTERVAL_SECS, PINGS_INTERVAL_SECS, SETINFO_BURST, SETINFO_RATE_PER_SEC,
};
pub use map::ServerMap;
pub use mapents::{MAX_ENTITY_KEYS, MAX_KEY_BYTES, MAX_VALUE_BYTES};
pub use movestep::{MOVE_NORMAL, WalkMoveMode};
pub use oob::{
    CHALLENGE_BUCKET_SECS, ChallengeIssuer, ConnectRequest, MAX_INFO_STRING, MAX_NAME_BYTES,
    OOB_GLOBAL_REPLIES_PER_SEC, OobContext, OobLimiter, OobOutcome, handle_oob,
    sanitize_name,
};
pub use precache::{MAX_GENERIC, MAX_MODELS, MAX_SOUNDS, PrecacheError, PrecacheTables, res_type};
pub use server::{
    MAX_DATAGRAMS_PER_FRAME, MAX_FRAMETIME, MAX_RELIABLE_BACKLOG, NetStats, SV_TIME_AT_MAP_START,
    Server, ServerConfig,
};
pub use signon::{
    INIT_BUFFER_BUDGET, MAX_SIGNON_BUFFER, SIGNON_BASELINE_RESERVE, SPAWN_TAIL_MAX,
};
pub use touch::MAX_TOUCHES_PER_FRAME;
pub use trace_ents::{EntTrace, MoveFilter};
pub use usercmd::{
    MAX_BANKED_MS, MAX_CONSECUTIVE_BAD_MOVES, MAX_DROPPED_REPLAY, MAX_USERCMD_STEP_MS, MsecBudget,
    STRINGCMD_BURST, STRINGCMD_RATE_PER_SEC, select_cmds_to_run, split_usercmd, usercmd_is_finite,
};
#[doc(hidden)]
pub use world::LinkSnapshot;
pub use world::{AREA_DEPTH, MAX_ENT_LEAFS, consts};
