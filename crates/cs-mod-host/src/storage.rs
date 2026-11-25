//! Shared `nvault` key-value store and $O(\log N)$ sorted [`LeaderboardStore`] with
//! optional [`ConfigStorage`] persistence helpers.

use cs_platform::ConfigStorage;

pub use cs_mod_api::LeaderboardEntry;
pub use cs_mod_api::services::{LeaderboardStore, NVaultEntry, NVaultStore};

/// Persists all dirty entries of `vault` into `storage` under `nvault:<vault>:<key>`.
pub fn save_nvault_entry(
    storage: &mut dyn ConfigStorage,
    vault: &str,
    key: &str,
    value: &str,
    timestamp: i64,
) {
    let full_key = format!("nvault:{vault}:{key}");
    let encoded = format!("{timestamp}\t{value}");
    let _ = storage.save(&full_key, &encoded);
}

/// Removes one persisted `nvault` entry from `storage`.
pub fn remove_nvault_entry(storage: &mut dyn ConfigStorage, vault: &str, key: &str) {
    let full_key = format!("nvault:{vault}:{key}");
    let _ = storage.remove(&full_key);
}
