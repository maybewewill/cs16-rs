//! The weapons of the game (`weapons.cpp`, `wpn_shared/`, ReGameDLL_CS): the tables ([`info`]),
//! the shared random generator ([`random`]) that both the server and the client's prediction
//! use, the state of a weapon and its behaviour trait ([`weapon`]), the state machine that runs
//! them ([`core`]: `ItemPostFrame`, `DefaultDeploy`, `DefaultReload`, ...) and the weapons
//! themselves ([`pistols`], [`smgs`], [`rifles`], [`snipers`], [`shotguns`], [`knife`],
//! [`grenades`], [`c4`]).

pub mod c4;
pub mod core;
pub mod grenades;
pub mod info;
pub mod knife;
pub mod pistols;
pub mod random;
pub mod rifles;
pub mod shotguns;
pub mod smgs;
pub mod snipers;
pub mod weapon;

pub use info::*;
pub use random::{shared_random_float, shared_random_long};
pub use weapon::{
    MAX_ITEM_TYPES, Weapon, WeaponBehavior, WeaponEvents, WeaponSlotItem, new_weapon, precache_all,
};
