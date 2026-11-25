//! Unified multi-language i18n catalog (`configs/lang/*.toml` + AMX Mod X `data/lang/*.txt`),
//! automatic `userinfo` language detection, live `/lang <code>` chat command, and persistent
//! per-player language preferences via [`ConfigStorage`].

use std::path::Path;

use cs_engine::{EngineApi, MsgDest};
use cs_platform::ConfigStorage;
use cs_proto48::usermsg::CsUserMsg;

pub use cs_mod_api::services::{LANG_PLAYER, LANG_SERVER, LangCatalog, normalize_lang_code};

/// Loads all `*.toml` and AMX Mod X `*.txt` translation files from `dir` into `catalog`.
///
/// Returns the total number of translation entries parsed.
pub fn load_lang_dir(catalog: &mut LangCatalog, dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok().map(|d| d.path()))
        .filter(|p| p.is_file())
        .collect();
    paths.sort();

    let mut total = 0usize;
    for path in paths {
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if ext.eq_ignore_ascii_case("toml") {
            total += catalog.load_toml_str(&content);
        } else if ext.eq_ignore_ascii_case("txt") {
            total += catalog.load_amxx_txt_str(&content);
        }
    }
    total
}

/// Derives a stable per-player persistence key (`SteamID` preferred, falling back to `IP` or `name`).
pub fn player_lang_identity(eng: &EngineApi<'_>, slot: u8) -> Option<String> {
    for key in ["*sid", "steamid", "ip", "name"] {
        if let Some(v) = eng.client_key(slot, key) {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                let sanitized: String = trimmed
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                if !sanitized.is_empty() {
                    return Some(sanitized);
                }
            }
        }
    }
    None
}

/// Synchronizes `slot`'s active language in `catalog` from persisted `/lang` storage,
/// `userinfo` (`lang` / `cl_language`), and the `amx_language` server CVar.
pub fn sync_player_lang_from_userinfo(
    catalog: &mut LangCatalog,
    eng: &EngineApi<'_>,
    slot: u8,
    storage: &dyn ConfigStorage,
) {
    if let Some(default_lang) = eng.cvar_str("amx_language")
        && !default_lang.trim().is_empty()
    {
        catalog.set_server_lang(&default_lang);
    }

    let identity = player_lang_identity(eng, slot);
    if let Some(ref id) = identity
        && let Some(saved) = storage.load(&format!("lang:{id}"))
        && !saved.trim().is_empty()
    {
        catalog.set_player_lang(slot, Some(id.as_str()), &saved);
        return;
    }

    let userinfo_lang = eng.client_key(slot, "lang");
    let cl_language = eng.client_key(slot, "cl_language");
    catalog.update_player_from_userinfo(slot, identity.as_deref(), userinfo_lang, cl_language);
}

/// Handles the built-in `/lang [code]` chat command, persisting the choice by SteamID/IP and
/// confirming in the player's newly chosen language. Returns `true` if handled (suppresses chat).
pub fn handle_lang_chat_command(
    catalog: &mut LangCatalog,
    eng: &mut EngineApi<'_>,
    slot: u8,
    text: &str,
    storage: &mut dyn ConfigStorage,
) -> bool {
    let trimmed = text.trim();
    if !(trimmed.eq_ignore_ascii_case("/lang")
        || trimmed.starts_with("/lang ")
        || trimmed.starts_with("/LANG "))
    {
        return false;
    }

    let arg = trimmed.split_whitespace().nth(1);
    let reply = if let Some(code_raw) = arg {
        let identity = player_lang_identity(eng, slot);
        if let Some(code) = catalog.set_player_lang(slot, identity.as_deref(), code_raw) {
            if let Some(ref id) = identity {
                let _ = storage.save(&format!("lang:{id}"), &code);
            }
            let custom = catalog.tr(Some(slot), "LANG_CHANGED", &[("lang", code.as_str())]);
            if custom == "LANG_CHANGED" {
                match code.as_str() {
                    "ru" => format!("[cs16-rs] Язык изменён на: {code}"),
                    _ => format!("[cs16-rs] Language set to: {code}"),
                }
            } else {
                custom
            }
        } else {
            format!("[cs16-rs] Unknown language code: {code_raw}")
        }
    } else {
        let current = catalog.player_lang(Some(slot)).to_string();
        let available = catalog.available_languages().join(", ");
        let list = if available.is_empty() {
            "en".to_string()
        } else {
            available
        };
        format!("[cs16-rs] Language: {current} (available: {list}). Usage: /lang <code>")
    };

    let _ = eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::SayText {
            sender_id: 0,
            text: reply,
            params: Vec::new(),
        },
    );
    true
}
