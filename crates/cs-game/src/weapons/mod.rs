//! The weapons of the game (`weapons.cpp`, `wpn_shared/`, ReGameDLL_CS). So far only the tables
//! ([`info`]) and the shared random generator ([`random`]) that both the server and the
//! client's prediction use; the behaviour of the weapons comes with the later tasks.

pub mod info;
pub mod random;

pub use info::*;
pub use random::{shared_random_float, shared_random_long};
