//! `cs-clientcore`: the platform-free core of a Protocol 48 client.
//!
//! This part holds the connection (handshake, netchan, sign-on), the client cvars and console
//! commands that steer it, the resource list with its precache tables, file consistency and
//! downloads. Entity storage, interpolation, prediction and the HUD state come in later tasks
//! of phase 3. Nothing here reads a clock, a file or a socket: time is an argument, packets go
//! through `cs_transport::PacketTransport` and game files through [`ResourceFiles`], so the
//! crate builds for `wasm32-unknown-unknown`.

pub mod commands;
pub mod connection;
pub mod cvars;
pub mod entities;
pub mod frames;
pub mod interp;
pub mod precache;
pub mod resources;

pub use commands::{CMD_FILTERABLE, CMD_PRIVILEGED, CMD_STUFFTEXT_OK, CommandRegistry, ConsoleCommand};
pub use connection::{
    CONSISTENCY_REASON, ConnState, ConnStats, Connection, ConnectionConfig, ConsistencyPolicy,
    DatagramTap, MAX_CMDRATE, MESSAGE_LOG_CAPACITY, MIN_CMDRATE, NO_SERVERINFO_REASON,
};
pub use cvars::{ClientCvars, Cvar};
pub use entities::{ClientEntity, EntityStore, ModelKind};
pub use frames::{ClientClock, ClientFrames, DeltaRefusal, Frame, FrameOutcome, FrameRing};
pub use interp::{EmitInputs, EmitResult, EntityKind, InterpSettings, VisibleEntity};
pub use precache::{PrecacheEntry, PrecacheTables};
pub use resources::{AllFilesPresent, ResourceFiles, ResourceManager, res_type};
