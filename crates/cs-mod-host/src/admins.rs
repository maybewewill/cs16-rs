//! Shared RBAC [`AdminRegistry`] loader (`configs/admins.toml` + AMX Mod X `users.ini`)
//! and live slot flag synchronization against `EngineApi` `userinfo`.

use std::path::Path;

use cs_engine::EngineApi;

pub use cs_mod_api::AdminFlags;
pub use cs_mod_api::services::{AdminAuthKind, AdminEntry, AdminRegistry};

/// Loads admin entries from `<base_dir>/configs/admins.toml`, `<base_dir>/configs/users.ini`,
/// and `<base_dir>/addons/amxmodx/configs/users.ini` when present.
pub fn load_admins_from_dir(registry: &mut AdminRegistry, base_dir: &Path) -> usize {
    let mut loaded = 0usize;
    let toml_candidates = [
        base_dir.join("configs").join("admins.toml"),
        base_dir.join("admins.toml"),
    ];
    for path in toml_candidates {
        if let Ok(content) = std::fs::read_to_string(&path) {
            loaded += registry.load_admins_toml_str(&content);
        }
    }
    let ini_candidates = [
        base_dir.join("configs").join("users.ini"),
        base_dir.join("users.ini"),
        base_dir
            .join("addons")
            .join("amxmodx")
            .join("configs")
            .join("users.ini"),
    ];
    for path in ini_candidates {
        if let Ok(content) = std::fs::read_to_string(&path) {
            loaded += registry.load_users_ini_str(&content);
        }
    }
    loaded
}

/// Evaluates `slot`'s `userinfo` (`*sid`/`steamid`, `ip`, `name`, `_pw`) against `registry`
/// and caches the resulting [`AdminFlags`] bitmask for the slot.
///
/// Returns `Err(reason)` if a matching admin entry requires kicking on an invalid `_pw`.
pub fn sync_player_admin(
    registry: &mut AdminRegistry,
    eng: &EngineApi<'_>,
    slot: u8,
) -> Result<AdminFlags, String> {
    let steamid = eng
        .client_key(slot, "*sid")
        .or_else(|| eng.client_key(slot, "steamid"))
        .unwrap_or("");
    let ip = eng.client_key(slot, "ip").unwrap_or("");
    let name = eng.client_key(slot, "name").unwrap_or("");
    let password = eng.client_key(slot, "_pw").unwrap_or("");
    registry.update_slot(slot, steamid, ip, name, password)
}
