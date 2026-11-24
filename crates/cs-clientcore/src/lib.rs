//! `cs-clientcore`: the platform-free core of a Protocol 48 client.
//!
//! This crate holds the connection (handshake, netchan, sign-on), client cvars and console
//! commands, the resource list with its precache tables, file consistency and downloads, entity
//! storage and interpolation, movement prediction, and client-side effects / HUD state. Nothing
//! here reads a clock, a file or a socket: time is an argument, packets go through
//! `cs_transport::PacketTransport` and game files through [`ResourceFiles`], so the crate
//! builds for `wasm32-unknown-unknown`.

pub mod commands;
pub mod connection;
pub mod cvars;
pub mod entities;
pub mod frames;
pub mod interp;
pub mod events;
pub mod hud;
pub mod precache;
pub mod predict;
pub mod resources;
pub mod sound;
pub mod tempents;
pub mod weapons_predict;

pub use commands::{CMD_FILTERABLE, CMD_PRIVILEGED, CMD_STUFFTEXT_OK, CommandRegistry, ConsoleCommand};
pub use connection::{
    CONSISTENCY_REASON, ConnState, ConnStats, Connection, ConnectionConfig, ConsistencyPolicy,
    DatagramTap, MAX_CMDRATE, MESSAGE_LOG_CAPACITY, MIN_CMDRATE, NO_SERVERINFO_REASON,
    ViewAngleUpdate,
};
pub use cvars::{ClientCvars, Cvar};
pub use entities::{ClientEntity, EntityStore, ModelKind};
pub use frames::{ClientClock, ClientFrames, DeltaRefusal, Frame, FrameOutcome, FrameRing};
pub use interp::{EmitInputs, EmitResult, EntityKind, InterpSettings, VisibleEntity};
pub use precache::{PrecacheEntry, PrecacheTables};
pub use predict::{
    LocalState, PlayerPose, PredictInputs, PredictSettings, PredictedSound, Prediction,
    PredictionCheck, Predictor, WeaponState, split_usercmd,
};
pub use resources::{AllFilesPresent, ResourceFiles, ResourceManager, res_type};
pub use events::{EventFx, EventQueue, FiredEvent, FxEnv};
pub use hud::{ClientEffects, HudState};
pub use sound::{SoundCommand, SoundQueue, SoundStart};
pub use tempents::{FxRandom, FxWorld, NoWorld, RenderEffect, TempEnt, TempEnts};
pub use weapons_predict::PredictedWeaponEvent;
