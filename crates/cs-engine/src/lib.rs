//! Server engine: the equivalent of the original game's dedicated server.

mod api_entities;
mod api_messages;
pub mod bans;
pub use bans::BanList;
pub mod client;
pub use cs_pm::collision;
mod connect;
pub mod console;
pub mod downloads;
pub use downloads::{ConsistencyRule, CustomizationStore, ServerFileProvider};
pub mod edict;
pub mod logging;
pub use logging::ServerLogger;
pub mod rcon;
pub use rcon::{RconPolicy, RconState};
pub mod events;
pub mod frame;
pub mod game;
mod lifecycle;
pub mod map;
pub mod mapents;
pub mod master;
pub use master::{HeartbeatSnapshot, MasterRegistration};
mod movestep;
pub mod oob;
pub mod pm;
pub mod precache;
pub mod server;
mod signon;
mod physics;
mod physics_move;
mod physics_water;
mod rng;
mod studio;
mod texture;
mod touch;
mod trace_ents;
mod unlag;
pub mod usercmd;
pub mod voice_bridge;
mod world;

pub use client::{ClientState, MAX_CLIENT_INCOMING_STREAM, MAX_REFUSED_STREAMS, SignonStage};
pub use api_messages::{MAX_MESSAGE_TEXT, MsgDest};
pub use collision::WorldCollision;
pub use console::{ConsoleOutcome, MAX_CONSOLE_LINE, valid_map_name};
pub use edict::{
    DEFAULT_MAX_EDICTS, EDICT_REUSE_DELAY, Edict, EdictId, EdictPool, EntVars, MAX_EDICTS_WIRE,
};
pub use events::{MAX_EVENT_QUEUE, event_flags};
pub use frame::{
    ClientFrame, FramePlayer, FrameRing, MAX_DATAGRAM, MAX_PACKET_ENTITIES, UPDATE_BACKUP, clamp_rate,
    update_interval,
};
pub use game::{
    EngineApi, FullPackCtx, GameDll, GameStatEvent, baseline_state_from_vars,
    entity_state_from_vars,
};
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
pub use pm::{
    CONTENTS_LADDER, MAX_MOVEENTS, MAX_PHYSENTS, PM_GLASS_IGNORE, PM_NORMAL, PM_PLAYER_MAXS,
    PM_PLAYER_MINS, PM_STUDIO_BOX, PM_STUDIO_IGNORE, PM_WORLD_ONLY, PhysEntView, PmPhysEnts,
    PmTraceResult,
};
pub use precache::{
    MAX_DECALS, MAX_GENERIC, MAX_MODELS, MAX_SOUNDS, ModelLoader, PrecacheError, PrecacheTables,
    decal_names_from_wads, res_type, wad_lump_names,
};
pub use server::{
    MAX_DATAGRAMS_PER_FRAME, MAX_FRAMETIME, MAX_RELIABLE_BACKLOG, NetStats, SV_TIME_AT_MAP_START,
    Server, ServerClientInfo, ServerConfig,
};
pub use signon::{
    INIT_BUFFER_BUDGET, MAX_SIGNON_BUFFER, SIGNON_BASELINE_RESERVE, SPAWN_TAIL_MAX,
};
pub use touch::MAX_TOUCHES_PER_FRAME;
pub use trace_ents::{EntTrace, MoveFilter};
pub use usercmd::{
    MAX_BANKED_MS, MAX_CONSECUTIVE_BAD_MOVES, MAX_DROPPED_REPLAY, MAX_USERCMD_STEP_MS, MsecBudget,
    STRINGCMD_BURST, STRINGCMD_RATE_PER_SEC, select_cmds_to_run, select_cmds_with_seeds, split_usercmd, usercmd_is_finite,
};
#[doc(hidden)]
pub use world::LinkSnapshot;
pub use world::{AREA_DEPTH, MAX_ENT_LEAFS, consts};
