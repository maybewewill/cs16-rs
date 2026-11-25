//! 5-Second Legacy ReHLDS / AMX Mod X Server Migrator.
//!
//! Converts an existing Counter-Strike 1.6 `cstrike/` or `addons/amxmodx/` tree into native
//! `cs16-rs` configuration files (`configs/admins.toml`, `configs/plugins.toml`,
//! `configs/maps.toml`, `mapcycle.txt`, `configs/imported_amxx.cfg`, `configs/lang/amxx/*.txt`,
//! and `configs/lang/amxx_imported.toml`) while preserving custom third-party `.amxx` plugins
//! mounted under `[amxx]` for `cs-amxx` and honestly flagging unsupported native Metamod
//! `.dll`/`.so` modules with suggested `cs16-rs` alternatives.
//!
//! Clean-room GPL-3.0-or-later implementation citing AMX Mod X configuration semantics
//! (`users.ini`, `plugins.ini`, `plugins-*.ini`, `maps.ini`, `amxx.cfg`, `modules.ini`,
//! `data/lang/*.txt`), Metamod (`addons/metamod/plugins.ini`), and GoldSrc `mapcycle.txt`
//! (read 2026-11-26).
//!
//! Zero `unsafe` (`#![forbid(unsafe_code)]`). Core parsers, renderers, stock-plugin mapping,
//! path validators, and [`MigrationReport`] are 100% platform-neutral and compile cleanly on
//! `wasm32-unknown-unknown`; filesystem I/O helpers are gated under
//! `#[cfg(not(target_arch = "wasm32"))]`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::Ipv4Addr;
use std::path::PathBuf;

use cs_mod_api::AdminFlags;
use cs_mod_api::services::{AdminAuthKind, LangCatalog, normalize_lang_code};
use thiserror::Error;

/// Maximum size of any text configuration or language dictionary file read during migration
/// (`512 KiB`).
pub const MAX_CONFIG_FILE_BYTES: usize = 512 * 1024;
/// Maximum size of a single custom `.amxx` plugin binary copied during migration (`8 MiB`).
pub const MAX_AMXX_PLUGIN_BYTES: usize = 8 * 1024 * 1024;
/// Maximum number of `plugins-*.ini` files scanned in `configs/`.
pub const MAX_PLUGIN_LIST_FILES: usize = 64;
/// Maximum number of custom `.amxx` plugins mounted in one migration.
pub const MAX_CUSTOM_PLUGINS: usize = 256;
/// Maximum number of `data/lang/*.txt` dictionary files copied/indexed in one migration.
pub const MAX_LANG_FILES: usize = 256;
/// Maximum number of maps imported into the map rotation (`cs-game` `MapCycle::MAX_ITEMS`).
pub const MAX_MAPS: usize = 256;

/// Canonical order for native `cs16-rs` replacements auto-mapped from stock AMX Mod X plugins.
const NATIVE_MOD_ORDER: &[&str] = &[
    "std_admin",
    "std_mapmanager",
    "std_statsx",
    "std_restmenu",
    "antiflood_admanager.py",
];

/// Mapping entry from a stock AMX Mod X plugin to its native `cs16-rs` replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockPluginMapping {
    /// Lowercase stock AMXX plugin filename (e.g. `"admincmd.amxx"`).
    pub amxx_name: &'static str,
    /// Native `cs16-rs` plugin identifier (e.g. `"std_admin"`).
    pub native_mod: &'static str,
    /// Optional operator note surfaced in the migration report.
    pub note: Option<&'static str>,
}

/// Complete table of the 25 stock AMX Mod X base/cstrike plugins and their native `cs16-rs`
/// replacements.
pub const STOCK_AMXX_PLUGINS: &[StockPluginMapping] = &[
    StockPluginMapping {
        amxx_name: "admin.amxx",
        native_mod: "std_admin",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "admin_sql.amxx",
        native_mod: "std_admin",
        note: Some("SQL admin backend mapped to std_admin + configs/admins.toml"),
    },
    StockPluginMapping {
        amxx_name: "admincmd.amxx",
        native_mod: "std_admin",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "adminhelp.amxx",
        native_mod: "std_admin",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "adminslots.amxx",
        native_mod: "std_admin",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "multilingual.amxx",
        native_mod: "std_admin",
        note: Some("Per-player i18n is built into cs-mod-host LangCatalog (/lang)"),
    },
    StockPluginMapping {
        amxx_name: "menufront.amxx",
        native_mod: "std_admin",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "cmdmenu.amxx",
        native_mod: "std_admin",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "plmenu.amxx",
        native_mod: "std_admin",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "telemenu.amxx",
        native_mod: "std_admin",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "mapsmenu.amxx",
        native_mod: "std_admin",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "pluginmenu.amxx",
        native_mod: "std_admin",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "adminchat.amxx",
        native_mod: "std_admin",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "adminvote.amxx",
        native_mod: "std_admin",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "pausecfg.amxx",
        native_mod: "std_admin",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "antiflood.amxx",
        native_mod: "antiflood_admanager.py",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "scrollmsg.amxx",
        native_mod: "antiflood_admanager.py",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "imessage.amxx",
        native_mod: "antiflood_admanager.py",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "nextmap.amxx",
        native_mod: "std_mapmanager",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "mapchooser.amxx",
        native_mod: "std_mapmanager",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "timeleft.amxx",
        native_mod: "std_mapmanager",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "statscfg.amxx",
        native_mod: "std_statsx",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "statsx.amxx",
        native_mod: "std_statsx",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "miscstats.amxx",
        native_mod: "std_statsx",
        note: None,
    },
    StockPluginMapping {
        amxx_name: "restmenu.amxx",
        native_mod: "std_restmenu",
        note: None,
    },
];

/// Looks up a stock AMX Mod X plugin name (case-insensitively, accepting both `.amxx` and legacy
/// `.amx` extensions) and returns its native `cs16-rs` replacement mapping if recognized.
pub fn lookup_stock_amxx_plugin(name: &str) -> Option<StockPluginMapping> {
    let lower = name.trim().to_ascii_lowercase();
    let canonical = if let Some(stem) = lower.strip_suffix(".amx") {
        format!("{stem}.amxx")
    } else {
        lower
    };
    STOCK_AMXX_PLUGINS
        .iter()
        .copied()
        .find(|entry| entry.amxx_name == canonical)
}

/// Errors produced by AMX Mod X migration or path-hygiene checks.
#[derive(Debug, Error)]
pub enum ImportError {
    #[error(
        "AMX Mod X directory not found under `{0}` (expected `addons/amxmodx/` or `configs/`)"
    )]
    AmxxRootNotFound(PathBuf),
    #[error("path traversal rejected: `{0}`")]
    PathTraversal(String),
    #[error("symlink escape outside source directory rejected: `{0}`")]
    SymlinkEscape(String),
    #[error("file `{path}` exceeds size limit ({size} bytes > {limit} bytes)")]
    FileTooLarge {
        path: String,
        size: u64,
        limit: usize,
    },
    #[error("I/O error on `{path}`: {message}")]
    Io { path: String, message: String },
}

/// Configurable bounds and behavior for [`import_amxx_dir_with_options`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOptions {
    /// Maximum text config/dictionary size in bytes (default [`MAX_CONFIG_FILE_BYTES`]).
    pub max_config_file_bytes: usize,
    /// Maximum `.amxx` binary size in bytes (default [`MAX_AMXX_PLUGIN_BYTES`]).
    pub max_amxx_plugin_bytes: usize,
    /// Whether to copy found custom `.amxx` binaries into `<out_dir>/plugins/` (default `true`).
    pub copy_custom_binaries: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            max_config_file_bytes: MAX_CONFIG_FILE_BYTES,
            max_amxx_plugin_bytes: MAX_AMXX_PLUGIN_BYTES,
            copy_custom_binaries: true,
        }
    }
}

// ===========================================================================
// Strict Path Hygiene (Platform-Neutral)
// ===========================================================================

/// Validates a relative path string, rejecting empty paths, NUL/control bytes, absolute paths,
/// Windows drive prefixes, UNC paths, and any `..` traversal components.
///
/// Returns a normalized forward-slash relative path on success.
pub fn validate_safe_relative_path(raw: &str) -> Result<String, ImportError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ImportError::PathTraversal("empty path".to_string()));
    }
    if trimmed.bytes().any(|b| b == 0 || b < 0x20) {
        return Err(ImportError::PathTraversal(trimmed.to_string()));
    }
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err(ImportError::PathTraversal(trimmed.to_string()));
    }
    let normalized = trimmed.replace('\\', "/");
    let mut clean_parts = Vec::new();
    for part in normalized.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || (part.contains("..") && part.trim_matches('.').is_empty()) {
            return Err(ImportError::PathTraversal(trimmed.to_string()));
        }
        // Reject Windows drive-letter prefix (e.g. `C:foo`) or colon stream syntax (`foo:bar`).
        if part.contains(':') {
            return Err(ImportError::PathTraversal(trimmed.to_string()));
        }
        clean_parts.push(part);
    }
    if clean_parts.is_empty() {
        return Err(ImportError::PathTraversal(trimmed.to_string()));
    }
    Ok(clean_parts.join("/"))
}

/// Validates a single safe filename component (no directory separators, no `..`, no `:`, no
/// control bytes).
pub fn validate_safe_filename(raw: &str) -> Result<String, ImportError> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed == "."
        || trimmed == ".."
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains(':')
        || trimmed.bytes().any(|b| b == 0 || b < 0x20)
    {
        return Err(ImportError::PathTraversal(trimmed.to_string()));
    }
    Ok(trimmed.to_string())
}

// ===========================================================================
// 1. configs/users.ini -> configs/admins.toml
// ===========================================================================

/// One admin account parsed from AMX Mod X `configs/users.ini`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedAdmin {
    pub auth: String,
    pub password: String,
    pub access_flags: AdminFlags,
    pub auth_kind: AdminAuthKind,
    pub account_flags: String,
    /// `e` flag (or empty password): do not check `_pw`.
    pub skip_password: bool,
    /// `k` flag: case-sensitive name / clan tag match.
    pub case_sensitive: bool,
    /// `a` flag: disconnect player on invalid `_pw`.
    pub kick_on_bad_password: bool,
    pub source_line: usize,
}

/// Result of parsing `configs/users.ini`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedUsersIni {
    pub admins: Vec<ImportedAdmin>,
    pub skipped_lines: usize,
    pub warnings: Vec<String>,
}

/// Parses AMX Mod X `configs/users.ini` lines (`"<auth>" "<password>" "<access>" "<account>"`),
/// ignoring blank lines and `;` / `//` comments (both full-line and trailing outside quotes),
/// preserving SteamID (`STEAM_*`/`VALVE_*`), IP/CIDR, Clan-tag, and Name+`_pw` accounts and all
/// `a..z` access flags.
pub fn parse_users_ini(src: &str) -> ParsedUsersIni {
    let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let mut out = ParsedUsersIni::default();

    for (idx, raw_line) in src.lines().enumerate() {
        let line_no = idx + 1;
        let stripped = strip_ini_comment_outside_quotes(raw_line).trim();
        if stripped.is_empty() {
            continue;
        }
        let tokens = split_quoted_tokens(stripped);
        if tokens.len() < 3 || tokens[0].trim().is_empty() {
            out.skipped_lines += 1;
            out.warnings.push(format!(
                "users.ini line {line_no}: malformed admin entry skipped"
            ));
            continue;
        }
        let auth = tokens[0].trim().to_string();
        let password = tokens[1].clone();
        let raw_access = tokens[2].trim();
        let access_flags = AdminFlags::parse_str(raw_access);
        if raw_access
            .chars()
            .any(|c| !c.is_ascii_alphabetic())
        {
            out.warnings.push(format!(
                "users.ini line {line_no}: ignored non-alphabetic characters in access flags `{raw_access}`"
            ));
        }
        let acc_raw = tokens.get(3).map(String::as_str).unwrap_or("").trim();
        let acc = acc_raw.to_ascii_lowercase();
        let auth_upper = auth.to_ascii_uppercase();

        let auth_kind = if acc.contains('c')
            || auth_upper.starts_with("STEAM_")
            || auth_upper.starts_with("VALVE_")
        {
            AdminAuthKind::SteamId
        } else if acc.contains('d') || is_ip_or_cidr(&auth) {
            AdminAuthKind::IpOrCidr
        } else if acc.contains('b') {
            AdminAuthKind::ClanTag
        } else {
            AdminAuthKind::Name
        };

        let skip_password = acc.contains('e') || password.is_empty();
        let case_sensitive = acc.contains('k');
        let kick_on_bad_password = acc.contains('a');

        out.admins.push(ImportedAdmin {
            auth,
            password,
            access_flags,
            auth_kind,
            account_flags: acc,
            skip_password,
            case_sensitive,
            kick_on_bad_password,
            source_line: line_no,
        });
    }
    out
}

/// Renders parsed [`ImportedAdmin`] accounts into `configs/admins.toml` compatible with
/// [`cs_mod_api::services::AdminRegistry::load_admins_toml_str`].
pub fn render_admins_toml(admins: &[ImportedAdmin]) -> String {
    let mut out = String::new();
    out.push_str("# Generated by cs16-rs AMX Mod X migrator from configs/users.ini\n");
    out.push_str("# Loaded automatically by cs-mod-host AdminRegistry (configs/admins.toml)\n");

    if admins.is_empty() {
        out.push_str("\n# No active admin accounts were present in users.ini.\n");
        return out;
    }

    for admin in admins {
        out.push_str("\n[[admin]]\n");
        let type_str = match admin.auth_kind {
            AdminAuthKind::SteamId => "steamid",
            AdminAuthKind::IpOrCidr => "ip",
            AdminAuthKind::ClanTag => "clantag",
            AdminAuthKind::Name => "name",
        };
        out.push_str(&format!("auth = \"{}\"\n", escape_toml_str(&admin.auth)));
        out.push_str(&format!("type = \"{type_str}\"\n"));
        out.push_str(&format!(
            "flags = \"{}\"\n",
            admin.access_flags.to_flag_string()
        ));
        if !admin.skip_password && !admin.password.is_empty() {
            out.push_str(&format!(
                "password = \"{}\"\n",
                escape_toml_str(&admin.password)
            ));
        } else if admin.skip_password && !admin.password.is_empty() {
            out.push_str("# password omitted because AMXX account flag 'e' disables _pw checks\n");
        }
        if admin.case_sensitive {
            out.push_str("case_sensitive = true\n");
        }
        if admin.kick_on_bad_password {
            out.push_str("kick_on_bad_password = true\n");
        }
        if !admin.account_flags.is_empty() {
            out.push_str(&format!(
                "# amxx account flags: \"{}\"\n",
                escape_toml_str(&admin.account_flags)
            ));
        }
    }
    out
}

// ===========================================================================
// 2. configs/plugins.ini & configs/plugins-*.ini -> configs/plugins.toml
// ===========================================================================

/// Record of one enabled stock AMX Mod X plugin upgraded to a native `cs16-rs` mod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockPluginUpgrade {
    pub amxx_name: String,
    pub native_mod: String,
    pub source_file: String,
    pub note: Option<String>,
}

/// Record of one enabled custom third-party `.amxx` plugin preserved for `cs-amxx`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomAmxxPlugin {
    /// Safe plugin filename or relative path inside `plugins/` (e.g. `"custom_vip.amxx"`).
    pub name: String,
    /// Logical mount path (`"plugins/<custom>.amxx"`).
    pub mount_path: String,
    pub source_file: String,
    pub debug: bool,
    /// Whether the `.amxx` binary was found in source `plugins/` and copied to output `plugins/`.
    pub binary_copied: bool,
}

/// Record of a disabled (`;` / `//` / `disabled`) plugin line encountered during migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisabledPluginEntry {
    pub name: String,
    pub source_file: String,
}

/// Parsed representation of one or more `plugins.ini` / `plugins-*.ini` files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedPluginsIni {
    pub upgraded_stock: Vec<StockPluginUpgrade>,
    pub native_mods_enabled: Vec<String>,
    pub custom_amxx: Vec<CustomAmxxPlugin>,
    pub disabled: Vec<DisabledPluginEntry>,
    pub warnings: Vec<String>,
}

/// Parses a single `plugins.ini` or `plugins-*.ini` file, validating plugin paths against `..`
/// traversal, mapping all recognized stock AMX Mod X plugins to their native `cs16-rs`
/// replacements, preserving custom `.amxx` plugins, and recording disabled entries cleanly.
pub fn parse_plugins_ini(
    src: &str,
    source_file: &str,
) -> Result<ParsedPluginsIni, ImportError> {
    let mut parsed = ParsedPluginsIni::default();
    parse_plugins_ini_into(src, source_file, &mut parsed)?;
    finalize_native_mod_order(&mut parsed);
    Ok(parsed)
}

/// Merges multiple `(source_filename, content)` plugin-list files (`plugins.ini` followed by
/// `plugins-*.ini`) in AMX Mod X order, honoring `disabled` overrides from later `plugins-*.ini`
/// files.
pub fn merge_parsed_plugins<'a>(
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<ParsedPluginsIni, ImportError> {
    let mut merged = ParsedPluginsIni::default();
    for (source_file, content) in files {
        parse_plugins_ini_into(content, source_file, &mut merged)?;
    }
    finalize_native_mod_order(&mut merged);
    Ok(merged)
}

fn parse_plugins_ini_into(
    src: &str,
    source_file: &str,
    out: &mut ParsedPluginsIni,
) -> Result<(), ImportError> {
    let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);

    for raw_line in src.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Disabled comment line (`;plugin.amxx` or `//plugin.amxx`)
        if trimmed.starts_with(';') || trimmed.starts_with("//") {
            let uncommented = trimmed
                .trim_start_matches(';')
                .trim_start_matches('/')
                .trim();
            if let Some(first_tok) = uncommented.split_whitespace().next() {
                let tok = first_tok.trim_matches('"');
                if is_amxx_filename(tok) {
                    push_disabled_unique(out, tok, source_file);
                }
            }
            continue;
        }

        // Active line: strip trailing `;` or `//` comment outside quotes
        let active = strip_ini_comment_outside_quotes(trimmed).trim();
        if active.is_empty() {
            continue;
        }
        let tokens = split_quoted_tokens(active);
        let Some(raw_plugin) = tokens.first() else {
            continue;
        };
        let plugin_str = raw_plugin.trim();
        if plugin_str.is_empty() {
            continue;
        }

        // Strict path hygiene: reject `..`, absolute paths, drive prefixes, control chars
        let safe_rel = validate_safe_relative_path(plugin_str)?;

        let mut is_debug = false;
        let mut is_explicitly_disabled = false;
        for flag in tokens.iter().skip(1) {
            if flag.eq_ignore_ascii_case("debug") {
                is_debug = true;
            } else if flag.eq_ignore_ascii_case("disabled") {
                is_explicitly_disabled = true;
            }
        }

        if is_explicitly_disabled {
            // AMXX 1.8.3+ `plugin.amxx disabled` in plugins-*.ini disables any earlier entry.
            out.upgraded_stock
                .retain(|u| !u.amxx_name.eq_ignore_ascii_case(&safe_rel));
            out.custom_amxx
                .retain(|c| !c.name.eq_ignore_ascii_case(&safe_rel));
            push_disabled_unique(out, &safe_rel, source_file);
            continue;
        }

        if !is_amxx_filename(&safe_rel) {
            out.warnings.push(format!(
                "{source_file}: skipped non-AMXX entry `{safe_rel}`"
            ));
            continue;
        }

        if let Some(mapping) = lookup_stock_amxx_plugin(&safe_rel) {
            if !out
                .upgraded_stock
                .iter()
                .any(|u| u.amxx_name.eq_ignore_ascii_case(mapping.amxx_name))
            {
                out.upgraded_stock.push(StockPluginUpgrade {
                    amxx_name: mapping.amxx_name.to_string(),
                    native_mod: mapping.native_mod.to_string(),
                    source_file: source_file.to_string(),
                    note: mapping.note.map(str::to_string),
                });
            }
        } else if !out
            .custom_amxx
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(&safe_rel))
        {
            if out.custom_amxx.len() >= MAX_CUSTOM_PLUGINS {
                out.warnings.push(format!(
                    "{source_file}: exceeded limit of {MAX_CUSTOM_PLUGINS} custom plugins; skipped `{safe_rel}`"
                ));
                continue;
            }
            out.custom_amxx.push(CustomAmxxPlugin {
                mount_path: format!("plugins/{safe_rel}"),
                name: safe_rel,
                source_file: source_file.to_string(),
                debug: is_debug,
                binary_copied: true,
            });
        }
    }

    Ok(())
}

fn push_disabled_unique(out: &mut ParsedPluginsIni, name: &str, source_file: &str) {
    if !out
        .disabled
        .iter()
        .any(|d| d.name.eq_ignore_ascii_case(name))
    {
        out.disabled.push(DisabledPluginEntry {
            name: name.to_string(),
            source_file: source_file.to_string(),
        });
    }
}

fn finalize_native_mod_order(parsed: &mut ParsedPluginsIni) {
    let present: BTreeSet<&str> = parsed
        .upgraded_stock
        .iter()
        .map(|u| u.native_mod.as_str())
        .collect();
    let mut ordered = Vec::new();
    for &canonical in NATIVE_MOD_ORDER {
        if present.contains(canonical) {
            ordered.push(canonical.to_string());
        }
    }
    for &other in &present {
        if !NATIVE_MOD_ORDER.contains(&other) {
            ordered.push(other.to_string());
        }
    }
    parsed.native_mods_enabled = ordered;
}

/// Renders `configs/plugins.toml` compatible with [`crate::ServerModHost::load_plugins_toml`].
///
/// - Stock AMX Mod X plugins auto-mapped to native `cs16-rs` modules are listed under `[native]`.
/// - Enabled custom `.amxx` plugins whose binaries are present (`binary_copied == true`) are
///   emitted under `[amxx]` as a single-line `amxx = ["../plugins/<custom>.amxx", ...]` array so
///   `ServerModHost::load_plugins_toml("<out>/configs/plugins.toml")` mounts them immediately.
/// - Custom `.amxx` plugins whose binary was missing on disk are recorded under `[amxx.missing]`
///   so they remain visible without failing server startup.
/// - Disabled entries are preserved under `[amxx.disabled]` for reference.
pub fn render_plugins_toml(parsed: &ParsedPluginsIni) -> String {
    let mut out = String::new();
    out.push_str("# Generated by cs16-rs AMX Mod X migrator\n");
    out.push_str("# Stock AMX Mod X plugins are auto-mapped to native cs16-rs equivalents.\n");
    out.push_str("# Custom third-party .amxx plugins remain mounted under [amxx] on cs-amxx.\n\n");

    if !parsed.native_mods_enabled.is_empty() {
        out.push_str("mods_dir = \"../mods\"\n\n");
    }

    out.push_str("[native]\n");
    if parsed.upgraded_stock.is_empty() {
        out.push_str("# No stock AMX Mod X plugins were enabled.\n");
    } else {
        out.push_str("# Upgraded from stock AMX Mod X plugins:\n");
        for u in &parsed.upgraded_stock {
            out.push_str(&format!("#   {} -> {}\n", u.amxx_name, u.native_mod));
        }
    }
    out.push_str(&format!(
        "enabled = {}\n\n",
        format_toml_string_array(&parsed.native_mods_enabled)
    ));

    let mounted: Vec<String> = parsed
        .custom_amxx
        .iter()
        .filter(|c| c.binary_copied)
        .map(|c| format!("../{}", c.mount_path))
        .collect();
    let missing: Vec<String> = parsed
        .custom_amxx
        .iter()
        .filter(|c| !c.binary_copied)
        .map(|c| format!("../{}", c.mount_path))
        .collect();

    out.push_str("[amxx]\n");
    out.push_str("# Custom third-party .amxx plugins mounted on cs-amxx (relative to configs/)\n");
    out.push_str(&format!("amxx = {}\n", format_toml_string_array(&mounted)));

    if !missing.is_empty() {
        out.push_str("\n[amxx.missing]\n");
        out.push_str(
            "# Referenced in plugins.ini but binary not found during import; copy into plugins/ and move to [amxx].amxx\n",
        );
        out.push_str(&format!(
            "plugins = {}\n",
            format_toml_string_array(&missing)
        ));
    }

    if !parsed.disabled.is_empty() {
        let disabled_names: Vec<String> =
            parsed.disabled.iter().map(|d| d.name.clone()).collect();
        out.push_str("\n[amxx.disabled]\n");
        out.push_str("# Disabled plugins preserved for reference (not loaded)\n");
        out.push_str(&format!(
            "plugins = {}\n",
            format_toml_string_array(&disabled_names)
        ));
    }

    out
}

// ===========================================================================
// 3. Maps (`configs/maps.ini` + `mapcycle.txt`) -> `configs/maps.toml` & `mapcycle.txt`
// ===========================================================================

/// One map rotation entry imported from `configs/maps.ini` or `mapcycle.txt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedMap {
    pub name: String,
    pub display_name: Option<String>,
    pub min_players: Option<u32>,
    pub max_players: Option<u32>,
}

/// Parses AMX Mod X `configs/maps.ini` (`<mapname> ["<Display Name>"]`, ignoring `;`/`//` comments).
pub fn parse_maps_ini(src: &str) -> Vec<ImportedMap> {
    let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let mut maps = Vec::new();
    let mut seen = BTreeSet::new();

    for raw_line in src.lines() {
        if maps.len() >= MAX_MAPS {
            break;
        }
        let line = strip_ini_comment_outside_quotes(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        let tokens = split_quoted_tokens(line);
        let Some(raw_name) = tokens.first() else {
            continue;
        };
        let name = raw_name.trim();
        if !is_valid_map_name(name) {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if !seen.insert(lower) {
            continue;
        }
        let display_name = tokens
            .get(1)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        maps.push(ImportedMap {
            name: name.to_string(),
            display_name,
            min_players: None,
            max_players: None,
        });
    }
    maps
}

/// Parses GoldSrc `mapcycle.txt` (`<mapname> [\minplayers\N\maxplayers\M]`, ignoring `//`/`;`
/// comments).
pub fn parse_mapcycle_txt(src: &str) -> Vec<ImportedMap> {
    let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let mut maps = Vec::new();
    let mut seen = BTreeSet::new();

    for raw_line in src.lines() {
        if maps.len() >= MAX_MAPS {
            break;
        }
        let line = strip_ini_comment_outside_quotes(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        let tokens = split_quoted_tokens(line);
        let Some(raw_name) = tokens.first() else {
            continue;
        };
        let name = raw_name.trim();
        if !is_valid_map_name(name) {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if !seen.insert(lower) {
            continue;
        }
        let rules = tokens.get(1).map(String::as_str).unwrap_or("");
        let min_players = parse_infokey_u32(rules, "minplayers").filter(|&n| n > 0);
        let max_players = parse_infokey_u32(rules, "maxplayers").filter(|&n| n > 0);
        maps.push(ImportedMap {
            name: name.to_string(),
            display_name: None,
            min_players,
            max_players,
        });
    }
    maps
}

/// Merges maps from `maps.ini` and `mapcycle.txt`, preserving order, deduplicating
/// case-insensitively, and combining display names with player-count bounds.
pub fn merge_maps(maps_ini: Vec<ImportedMap>, mapcycle: Vec<ImportedMap>) -> Vec<ImportedMap> {
    let mut merged: Vec<ImportedMap> = Vec::new();
    for m in maps_ini.into_iter().chain(mapcycle) {
        if let Some(existing) = merged
            .iter_mut()
            .find(|e| e.name.eq_ignore_ascii_case(&m.name))
        {
            if existing.display_name.is_none() {
                existing.display_name = m.display_name;
            }
            if existing.min_players.is_none() {
                existing.min_players = m.min_players;
            }
            if existing.max_players.is_none() {
                existing.max_players = m.max_players;
            }
        } else if merged.len() < MAX_MAPS {
            merged.push(m);
        }
    }
    merged
}

/// Renders `configs/maps.toml` for `std_mapmanager` map rotation and nomination menus.
pub fn render_maps_toml(maps: &[ImportedMap]) -> String {
    let mut out = String::new();
    out.push_str("# Generated by cs16-rs AMX Mod X migrator from configs/maps.ini & mapcycle.txt\n");
    let names: Vec<String> = maps.iter().map(|m| m.name.clone()).collect();
    out.push_str(&format!(
        "rotation = {}\n",
        format_toml_string_array(&names)
    ));
    for m in maps {
        out.push_str("\n[[map]]\n");
        out.push_str(&format!("name = \"{}\"\n", escape_toml_str(&m.name)));
        if let Some(ref disp) = m.display_name {
            out.push_str(&format!("display_name = \"{}\"\n", escape_toml_str(disp)));
        }
        if let Some(min_p) = m.min_players {
            out.push_str(&format!("min_players = {min_p}\n"));
        }
        if let Some(max_p) = m.max_players {
            out.push_str(&format!("max_players = {max_p}\n"));
        }
    }
    out
}

/// Renders standard GoldSrc `mapcycle.txt` compatible with [`cs_game::mapcycle::MapCycle::load`].
pub fn render_mapcycle_txt(maps: &[ImportedMap]) -> String {
    let mut out = String::new();
    out.push_str("// Generated by cs16-rs AMX Mod X migrator\n");
    for m in maps {
        let mut rules = String::new();
        if let Some(min_p) = m.min_players {
            rules.push_str(&format!("\\minplayers\\{min_p}"));
        }
        if let Some(max_p) = m.max_players {
            rules.push_str(&format!("\\maxplayers\\{max_p}"));
        }
        if rules.is_empty() {
            out.push_str(&format!("{}\n", m.name));
        } else {
            out.push_str(&format!("{} {rules}\n", m.name));
        }
    }
    out
}

// ===========================================================================
// 4. configs/amxx.cfg -> configs/imported_amxx.cfg
// ===========================================================================

/// One extracted CVar or server command from `configs/amxx.cfg`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedCvarEntry {
    pub name: String,
    pub value: String,
    pub raw_line: String,
    pub deprecated_warning: Option<String>,
}

/// Parsed result of `configs/amxx.cfg`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedAmxxCfg {
    pub entries: Vec<ImportedCvarEntry>,
    pub warnings: Vec<String>,
}

/// Parses server CVars and commands from AMX Mod X `configs/amxx.cfg`, stripping comments,
/// guarding against unsafe `exec` path traversal, and annotating SQL-only CVars.
pub fn parse_amxx_cfg(src: &str) -> ParsedAmxxCfg {
    let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let mut out = ParsedAmxxCfg::default();

    for (idx, raw_line) in src.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed_start = raw_line.trim_start();
        if trimmed_start.is_empty()
            || trimmed_start.starts_with("//")
            || trimmed_start.starts_with(';')
            || trimmed_start.starts_with('#')
        {
            continue;
        }
        let cleaned = strip_cfg_comment_outside_quotes(trimmed_start).trim();
        if cleaned.is_empty() {
            continue;
        }
        let mut parts = cleaned.splitn(2, char::is_whitespace);
        let Some(name_raw) = parts.next() else {
            continue;
        };
        let name = name_raw.trim();
        if name.is_empty() {
            continue;
        }
        let value = parts.next().map(str::trim).unwrap_or("").to_string();

        // Guard against `exec ../../...` traversal in legacy configs
        if name.eq_ignore_ascii_case("exec") {
            let target = value.trim_matches('"').trim();
            if validate_safe_relative_path(target).is_err() {
                let warn_msg = format!(
                    "amxx.cfg line {line_no}: skipped unsafe `exec` path `{target}`"
                );
                out.warnings.push(warn_msg);
                continue;
            }
        }

        let lower = name.to_ascii_lowercase();
        let deprecated_warning = if lower.starts_with("amx_sql_") {
            let msg = format!(
                "amxx.cfg: `{name}` is SQL-specific; cs16-rs uses configs/admins.toml & built-in nvault/LeaderboardStore"
            );
            out.warnings.push(msg.clone());
            Some(msg)
        } else {
            None
        };

        out.entries.push(ImportedCvarEntry {
            name: name.to_string(),
            value,
            raw_line: cleaned.to_string(),
            deprecated_warning,
        });
    }

    out
}

/// Renders extracted `amxx.cfg` commands into `configs/imported_amxx.cfg`.
pub fn render_imported_amxx_cfg(parsed: &ParsedAmxxCfg) -> String {
    let mut out = String::new();
    out.push_str("// Generated by cs16-rs AMX Mod X migrator from configs/amxx.cfg\n");
    out.push_str("// Execute via: exec configs/imported_amxx.cfg\n\n");
    for entry in &parsed.entries {
        if let Some(ref note) = entry.deprecated_warning {
            out.push_str(&format!("// NOTE: {note}\n"));
            out.push_str(&format!("// {}\n", entry.raw_line));
        } else {
            out.push_str(&format!("{}\n", entry.raw_line));
        }
    }
    out
}

// ===========================================================================
// 5. Language Dictionaries (`data/lang/*.txt` -> `configs/lang/amxx/` + TOML index)
// ===========================================================================

/// Metadata about one imported AMX Mod X `data/lang/*.txt` dictionary file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedLangFile {
    pub filename: String,
    pub entries_count: usize,
    pub languages: Vec<String>,
}

/// Parses one AMX Mod X `data/lang/*.txt` dictionary into `(lang_code -> (KEY -> value))` matching
/// [`LangCatalog::load_amxx_txt_str`] semantics so it can be indexed into
/// `configs/lang/amxx_imported.toml`.
pub fn parse_amxx_lang_txt(src: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let mut tables: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut current_lang = "en".to_string();

    for raw_line in src.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with("//") {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let sec = line[1..line.len() - 1].trim();
            if let Some(norm) = normalize_lang_code(sec) {
                current_lang = norm;
            }
            continue;
        }
        let Some((k_raw, v_raw)) = line.split_once('=') else {
            continue;
        };
        let key = k_raw.trim();
        if key.is_empty() {
            continue;
        }
        let val = unescape_amxx_lang_value(v_raw.trim());
        tables
            .entry(current_lang.clone())
            .or_default()
            .insert(key.to_string(), val);
    }

    tables
}

/// Renders merged AMX Mod X dictionary tables into `configs/lang/amxx_imported.toml` so
/// [`LangCatalog::load_toml_str`] and [`crate::ServerModHost::load_configs_dir`] load every
/// imported translation immediately on server boot.
pub fn render_lang_index_toml(
    tables: &BTreeMap<String, BTreeMap<String, String>>,
    source_files: &[ImportedLangFile],
) -> String {
    let mut out = String::new();
    out.push_str("# Generated by cs16-rs AMX Mod X migrator (index of configs/lang/amxx/*.txt)\n");
    if !source_files.is_empty() {
        let names: Vec<&str> = source_files.iter().map(|f| f.filename.as_str()).collect();
        out.push_str(&format!("# Sources: {}\n", names.join(", ")));
    }

    for (lang, entries) in tables {
        if entries.is_empty() {
            continue;
        }
        out.push_str(&format!("\n[{lang}]\n"));
        for (key, val) in entries {
            let formatted_key = if key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            {
                key.clone()
            } else {
                format!("\"{}\"", escape_toml_str(key))
            };
            out.push_str(&format!(
                "{formatted_key} = \"{}\"\n",
                escape_toml_str(val)
            ));
        }
    }
    out
}

/// Verifies how many entries `LangCatalog` loads from `src` (convenience wrapper).
pub fn count_amxx_lang_entries(src: &str) -> (usize, Vec<String>) {
    let mut cat = LangCatalog::new();
    let count = cat.load_amxx_txt_str(src);
    let langs = cat
        .available_languages()
        .into_iter()
        .map(str::to_string)
        .collect();
    (count, langs)
}

// ===========================================================================
// 6. Unsupported Metamod `.dll`/`.so` & AMXX Native Modules Detection
// ===========================================================================

/// Native `.dll`/`.so` Metamod plugin or unsupported AMXX C++ module flagged honestly during
/// migration with a suggested `cs16-rs` alternative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedNativeModule {
    pub name: String,
    pub raw_entry: String,
    pub source_file: String,
    pub platform: Option<String>,
    pub suggested_alternative: String,
}

/// Result of parsing AMX Mod X `configs/modules.ini`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedModulesIni {
    /// Modules listed in `modules.ini` that are natively provided by `cs-amxx` + `cs-mod-host`.
    pub native_supported: Vec<String>,
    /// Native `.dll`/`.so` or C++ extension modules that cannot run inside the sandbox.
    pub unsupported: Vec<UnsupportedNativeModule>,
}

/// Suggests a native `cs16-rs` alternative for a legacy Metamod `.dll`/`.so` or AMXX module name.
pub fn suggest_native_alternative(raw_name: &str) -> String {
    let lower = raw_name.to_ascii_lowercase();
    if lower.contains("amxmodx") {
        "Built-in unified cs-amxx + cs-mod-host runtime (no Metamod DLL needed)".to_string()
    } else if lower.contains("resemiclip") || lower.contains("semiclip") {
        "Native cs16-rs `semiclip` mod (PlayersShouldCollide hook in cs-mod-api v2)".to_string()
    } else if lower.contains("revoice") || lower.contains("voicetranscoder") || lower.contains("vtc")
    {
        "Built-in cs-server voice transcoder (`sv_voice_transcode 1`, `voice_steam_compat 1`)"
            .to_string()
    } else if lower.contains("reunion") || lower.contains("dproto") {
        "Built-in cs-server Protocol 48 authentication".to_string()
    } else if lower.contains("yapb")
        || lower.contains("podbot")
        || lower.contains("sypb")
        || lower.contains("zbot")
    {
        "Built-in `cs-bot` ZBot-compatible nav-mesh AI (`bot_quota`, `bot_nav_autogen 1`)"
            .to_string()
    } else if lower.contains("whblocker")
        || lower.contains("rechecker")
        || lower.contains("reaimdetector")
        || lower.contains("sxei")
    {
        "Built-in cs-server server-authoritative PVS culling & anti-cheat validation".to_string()
    } else if lower.contains("mysql") || lower.contains("sqlite") {
        "Built-in `nvault` + O(log N) `LeaderboardStore` + `ConfigStorage` persistence".to_string()
    } else if lower.contains("socket") || lower.contains("curl") || lower.contains("grip") {
        "Built-in capability-gated async HTTP webhooks (`http_post_json` with `permissions = [\"http:<host>\"]`)"
            .to_string()
    } else if lower.contains("orpheu") || lower.contains("okapi") {
        "Built-in ReAPI-grade `cs-mod-api` v2 Pre/Post hooks (no brittle memory signatures)"
            .to_string()
    } else if lower.contains("geoip") {
        "Port as a sandboxed Python/WASM mod or allowlisted HTTP webhook".to_string()
    } else {
        "Unsupported native binary module in zero-unsafe sandbox; port to a Rust/C++/Go WASM or Python mod using cs-mod-api v2"
            .to_string()
    }
}

/// Parses Metamod `addons/metamod/plugins.ini` (`win32 <path.dll>` / `linux <path.so>`), flagging
/// every enabled native binary entry honestly with a suggested `cs16-rs` alternative.
pub fn parse_metamod_plugins_ini(src: &str, source_file: &str) -> Vec<UnsupportedNativeModule> {
    let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();

    for raw_line in src.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with(';')
            || trimmed.starts_with("//")
            || trimmed.starts_with('#')
        {
            continue;
        }
        let cleaned = strip_ini_comment_outside_quotes(trimmed).trim();
        if cleaned.is_empty() {
            continue;
        }
        let tokens = split_quoted_tokens(cleaned);
        if tokens.is_empty() {
            continue;
        }
        let (platform, path_tok) = if tokens.len() >= 2
            && (tokens[0].eq_ignore_ascii_case("win32")
                || tokens[0].eq_ignore_ascii_case("linux")
                || tokens[0].eq_ignore_ascii_case("osx"))
        {
            (Some(tokens[0].to_ascii_lowercase()), tokens[1].as_str())
        } else if tokens[0].to_ascii_lowercase().ends_with(".dll")
            || tokens[0].to_ascii_lowercase().ends_with(".so")
            || tokens[0].to_ascii_lowercase().ends_with(".dylib")
        {
            (None, tokens[0].as_str())
        } else {
            continue;
        };

        let norm = path_tok.replace('\\', "/");
        let basename = norm.rsplit('/').next().unwrap_or(&norm).to_string();
        let dedup_key = format!(
            "{}:{}",
            platform.as_deref().unwrap_or(""),
            basename.to_ascii_lowercase()
        );
        if !seen.insert(dedup_key) {
            continue;
        }
        out.push(UnsupportedNativeModule {
            suggested_alternative: suggest_native_alternative(&norm),
            name: basename,
            raw_entry: norm,
            source_file: source_file.to_string(),
            platform,
        });
    }
    out
}

/// Built-in AMX Mod X modules already emulated natively by `cs-amxx` + `cs-mod-host`.
fn is_builtin_amxx_module(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "fun"
            | "engine"
            | "fakemeta"
            | "cstrike"
            | "csx"
            | "hamsandwich"
            | "nvault"
            | "regex"
            | "reapi"
    )
}

/// Parses AMX Mod X `configs/modules.ini`, separating modules natively supported by `cs-amxx`
/// from unsupported native C++ / `.dll` / `.so` extension modules.
pub fn parse_modules_ini(src: &str, source_file: &str) -> ParsedModulesIni {
    let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let mut out = ParsedModulesIni::default();
    let mut seen_supported = BTreeSet::new();
    let mut seen_unsupported = BTreeSet::new();

    for raw_line in src.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with(';')
            || trimmed.starts_with("//")
            || trimmed.starts_with('#')
        {
            continue;
        }
        let cleaned = strip_ini_comment_outside_quotes(trimmed).trim();
        let Some(first) = split_quoted_tokens(cleaned).into_iter().next() else {
            continue;
        };
        let norm = first.trim().replace('\\', "/");
        if norm.is_empty() {
            continue;
        }
        let lower = norm.to_ascii_lowercase();
        let has_binary_ext =
            lower.ends_with(".dll") || lower.ends_with(".so") || lower.ends_with(".dylib");
        if !has_binary_ext && is_builtin_amxx_module(&lower) {
            if seen_supported.insert(lower.clone()) {
                out.native_supported.push(lower);
            }
            continue;
        }
        let basename = norm.rsplit('/').next().unwrap_or(&norm).to_string();
        if seen_unsupported.insert(basename.to_ascii_lowercase()) {
            out.unsupported.push(UnsupportedNativeModule {
                suggested_alternative: suggest_native_alternative(&norm),
                name: basename,
                raw_entry: norm,
                source_file: source_file.to_string(),
                platform: None,
            });
        }
    }
    out
}

// ===========================================================================
// 7. MigrationReport & Display Summary
// ===========================================================================

/// Complete structured report emitted by the AMX Mod X server migrator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub source_dir: PathBuf,
    pub out_dir: PathBuf,
    pub admins_imported: usize,
    pub admins: Vec<ImportedAdmin>,
    pub upgraded_stock_plugins: Vec<StockPluginUpgrade>,
    pub native_mods_enabled: Vec<String>,
    pub custom_amxx_plugins: Vec<CustomAmxxPlugin>,
    pub disabled_plugins: Vec<DisabledPluginEntry>,
    pub maps_imported: Vec<ImportedMap>,
    pub cvars_imported: usize,
    pub cfg_entries: Vec<ImportedCvarEntry>,
    pub imported_lang_files: Vec<ImportedLangFile>,
    pub native_amxx_modules: Vec<String>,
    pub unsupported_native_modules: Vec<UnsupportedNativeModule>,
    pub written_files: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

impl MigrationReport {
    /// Returns a formatted multi-line migration summary suitable for CLI or server console output.
    pub fn summary(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for MigrationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== cs16-rs AMX Mod X Migration Report ===")?;
        writeln!(f, "Source: {}", self.source_dir.display())?;
        writeln!(f, "Output: {}", self.out_dir.display())?;
        writeln!(
            f,
            "  Admins imported:        {} (configs/admins.toml)",
            self.admins_imported
        )?;
        let native_list = if self.native_mods_enabled.is_empty() {
            "none".to_string()
        } else {
            self.native_mods_enabled.join(", ")
        };
        writeln!(
            f,
            "  Stock AMXX upgraded:    {} -> {} native cs16-rs mods ({})",
            self.upgraded_stock_plugins.len(),
            self.native_mods_enabled.len(),
            native_list
        )?;
        let copied_count = self
            .custom_amxx_plugins
            .iter()
            .filter(|c| c.binary_copied)
            .count();
        writeln!(
            f,
            "  Custom .amxx mounted:   {} (binaries copied: {}; configs/plugins.toml [amxx])",
            self.custom_amxx_plugins.len(),
            copied_count
        )?;
        writeln!(
            f,
            "  Disabled plugins:       {} (ignored & recorded)",
            self.disabled_plugins.len()
        )?;
        writeln!(
            f,
            "  Maps imported:          {} (configs/maps.toml, mapcycle.txt)",
            self.maps_imported.len()
        )?;
        writeln!(
            f,
            "  CVars/cmds extracted:   {} (configs/imported_amxx.cfg)",
            self.cvars_imported
        )?;
        let total_lang_entries: usize = self
            .imported_lang_files
            .iter()
            .map(|l| l.entries_count)
            .sum();
        writeln!(
            f,
            "  Language files indexed: {} ({} entries -> configs/lang/amxx/)",
            self.imported_lang_files.len(),
            total_lang_entries
        )?;
        if !self.native_amxx_modules.is_empty() {
            writeln!(
                f,
                "  AMXX modules built-in:  {}",
                self.native_amxx_modules.join(", ")
            )?;
        }
        if !self.unsupported_native_modules.is_empty() {
            writeln!(
                f,
                "Unsupported native modules ({}):",
                self.unsupported_native_modules.len()
            )?;
            for m in &self.unsupported_native_modules {
                match &m.platform {
                    Some(plat) => writeln!(
                        f,
                        "  - [{}] {} ({}) -> {}",
                        plat, m.name, m.source_file, m.suggested_alternative
                    )?,
                    None => writeln!(
                        f,
                        "  - {} ({}) -> {}",
                        m.name, m.source_file, m.suggested_alternative
                    )?,
                }
            }
        }
        if !self.warnings.is_empty() {
            writeln!(f, "Warnings ({}):", self.warnings.len())?;
            for w in &self.warnings {
                writeln!(f, "  - {w}")?;
            }
        }
        Ok(())
    }
}

// ===========================================================================
// 8. Native Filesystem Migrator (`#[cfg(not(target_arch = "wasm32"))]`)
// ===========================================================================

#[cfg(not(target_arch = "wasm32"))]
mod fs_import {
    use std::collections::BTreeMap;
    use std::fs::{self, File};
    use std::io::Read;
    use std::path::{Component, Path, PathBuf};

    use super::{
        ImportError, ImportOptions, ImportedLangFile, MAX_LANG_FILES, MAX_PLUGIN_LIST_FILES,
        MigrationReport, merge_maps, merge_parsed_plugins, parse_amxx_cfg, parse_amxx_lang_txt,
        parse_mapcycle_txt, parse_maps_ini, parse_metamod_plugins_ini, parse_modules_ini,
        parse_users_ini, render_admins_toml, render_imported_amxx_cfg, render_lang_index_toml,
        render_mapcycle_txt, render_maps_toml, render_plugins_toml, validate_safe_filename,
        validate_safe_relative_path,
    };

    /// Migrates a legacy ReHLDS / AMX Mod X `cstrike/` or `addons/amxmodx/` directory at
    /// `source_dir` into `out_dir` using default safety bounds ([`ImportOptions::default`]).
    pub fn import_amxx_dir(source_dir: &Path, out_dir: &Path) -> Result<MigrationReport, ImportError> {
        import_amxx_dir_with_options(source_dir, out_dir, &ImportOptions::default())
    }

    /// Migrates a legacy ReHLDS / AMX Mod X `cstrike/` or `addons/amxmodx/` directory at
    /// `source_dir` into `out_dir` with explicit [`ImportOptions`].
    pub fn import_amxx_dir_with_options(
        source_dir: &Path,
        out_dir: &Path,
        opts: &ImportOptions,
    ) -> Result<MigrationReport, ImportError> {
        let sandbox_root = fs::canonicalize(source_dir).map_err(|e| ImportError::Io {
            path: source_dir.display().to_string(),
            message: e.to_string(),
        })?;
        if !sandbox_root.is_dir() {
            return Err(ImportError::AmxxRootNotFound(source_dir.to_path_buf()));
        }

        // Locate AMXX root without ever escaping `sandbox_root`.
        let nested_amxx = source_dir.join("addons").join("amxmodx");
        let (amxx_root, has_cstrike_root) = if nested_amxx.exists() {
            let verified = verify_dir_under_root(&sandbox_root, &nested_amxx)?;
            (verified, true)
        } else if source_dir.join("configs").exists() {
            let _ = verify_dir_under_root(&sandbox_root, &source_dir.join("configs"))?;
            (sandbox_root.clone(), false)
        } else {
            return Err(ImportError::AmxxRootNotFound(source_dir.to_path_buf()));
        };

        let configs_src = amxx_root.join("configs");
        let configs_src_verified = if configs_src.exists() {
            Some(verify_dir_under_root(&sandbox_root, &configs_src)?)
        } else {
            None
        };

        let out_configs = out_dir.join("configs");
        create_dir_all_checked(&out_configs)?;

        let mut report = MigrationReport {
            source_dir: source_dir.to_path_buf(),
            out_dir: out_dir.to_path_buf(),
            ..MigrationReport::default()
        };

        // 1. configs/users.ini -> configs/admins.toml
        if let Some(ref cfg_dir) = configs_src_verified {
            let users_path = cfg_dir.join("users.ini");
            if path_exists_or_symlink(&users_path) {
                let raw = read_bounded_utf8(
                    &sandbox_root,
                    &users_path,
                    opts.max_config_file_bytes,
                )?;
                let parsed_users = parse_users_ini(&raw);
                report.admins_imported = parsed_users.admins.len();
                report.admins = parsed_users.admins;
                report.warnings.extend(parsed_users.warnings);
                let admins_toml = render_admins_toml(&report.admins);
                let dest = out_configs.join("admins.toml");
                write_file_checked(&dest, admins_toml.as_bytes())?;
                report.written_files.push(dest);
            }
        }

        // 2. configs/plugins.ini & configs/plugins-*.ini -> configs/plugins.toml
        let mut plugin_list_inputs: Vec<(String, String)> = Vec::new();
        if let Some(ref cfg_dir) = configs_src_verified {
            let main_plugins = cfg_dir.join("plugins.ini");
            if path_exists_or_symlink(&main_plugins) {
                let raw = read_bounded_utf8(
                    &sandbox_root,
                    &main_plugins,
                    opts.max_config_file_bytes,
                )?;
                plugin_list_inputs.push(("configs/plugins.ini".to_string(), raw));
            }

            let mut extra_ini: Vec<PathBuf> = Vec::new();
            for entry in read_dir_sorted(cfg_dir)? {
                let Some(fname) = entry.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let lower = fname.to_ascii_lowercase();
                if lower.starts_with("plugins-") && lower.ends_with(".ini") {
                    extra_ini.push(entry);
                }
            }
            if extra_ini.len() > MAX_PLUGIN_LIST_FILES {
                report.warnings.push(format!(
                    "configs/: found {} plugins-*.ini files; capped at {MAX_PLUGIN_LIST_FILES}",
                    extra_ini.len()
                ));
                extra_ini.truncate(MAX_PLUGIN_LIST_FILES);
            }
            for extra_path in extra_ini {
                let fname = extra_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("plugins-extra.ini")
                    .to_string();
                let raw = read_bounded_utf8(
                    &sandbox_root,
                    &extra_path,
                    opts.max_config_file_bytes,
                )?;
                plugin_list_inputs.push((format!("configs/{fname}"), raw));
            }
        }

        let plugin_refs: Vec<(&str, &str)> = plugin_list_inputs
            .iter()
            .map(|(n, c)| (n.as_str(), c.as_str()))
            .collect();
        let mut parsed_plugins = merge_parsed_plugins(plugin_refs)?;
        report.warnings.extend(std::mem::take(&mut parsed_plugins.warnings));

        // Copy enabled custom .amxx binaries from <amxx_root>/plugins/ -> <out_dir>/plugins/
        let plugins_src_dir = amxx_root.join("plugins");
        let plugins_src_verified = if plugins_src_dir.exists() {
            Some(verify_dir_under_root(&sandbox_root, &plugins_src_dir)?)
        } else {
            None
        };
        let out_plugins_dir = out_dir.join("plugins");

        for custom in &mut parsed_plugins.custom_amxx {
            let safe_rel = validate_safe_relative_path(&custom.name)?;
            if !opts.copy_custom_binaries {
                custom.binary_copied = false;
                continue;
            }
            let Some(ref src_plugins) = plugins_src_verified else {
                custom.binary_copied = false;
                report.warnings.push(format!(
                    "Custom AMXX plugin `{}` referenced in {} was not found (plugins/ directory missing)",
                    custom.name, custom.source_file
                ));
                continue;
            };
            let candidate = src_plugins.join(&safe_rel);
            if !path_exists_or_symlink(&candidate) {
                custom.binary_copied = false;
                report.warnings.push(format!(
                    "Custom AMXX plugin `{}` referenced in {} was not found in plugins/; copy it to plugins/{} and enable in configs/plugins.toml",
                    custom.name, custom.source_file, safe_rel
                ));
                continue;
            }
            let bytes =
                read_bounded_bytes(&sandbox_root, &candidate, opts.max_amxx_plugin_bytes)?;
            let dest = out_plugins_dir.join(&safe_rel);
            if let Some(parent) = dest.parent() {
                create_dir_all_checked(parent)?;
            }
            write_file_checked(&dest, &bytes)?;
            custom.binary_copied = true;
            report.written_files.push(dest);
        }

        for u in &parsed_plugins.upgraded_stock {
            if let Some(ref note) = u.note {
                report.warnings.push(format!("{}: {note}", u.amxx_name));
            }
        }

        let plugins_toml = render_plugins_toml(&parsed_plugins);
        let plugins_toml_path = out_configs.join("plugins.toml");
        write_file_checked(&plugins_toml_path, plugins_toml.as_bytes())?;
        report.written_files.push(plugins_toml_path);
        report.upgraded_stock_plugins = parsed_plugins.upgraded_stock;
        report.native_mods_enabled = parsed_plugins.native_mods_enabled;
        report.custom_amxx_plugins = parsed_plugins.custom_amxx;
        report.disabled_plugins = parsed_plugins.disabled;

        // 3. configs/maps.ini + mapcycle.txt -> configs/maps.toml & mapcycle.txt
        let maps_from_ini = if let Some(ref cfg_dir) = configs_src_verified {
            let p = cfg_dir.join("maps.ini");
            if path_exists_or_symlink(&p) {
                let raw = read_bounded_utf8(&sandbox_root, &p, opts.max_config_file_bytes)?;
                parse_maps_ini(&raw)
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };
        let mapcycle_candidates = if has_cstrike_root {
            vec![
                sandbox_root.join("mapcycle.txt"),
                amxx_root.join("mapcycle.txt"),
            ]
        } else {
            vec![amxx_root.join("mapcycle.txt")]
        };
        let mut maps_from_cycle = Vec::new();
        for cand in mapcycle_candidates {
            if path_exists_or_symlink(&cand) {
                let raw = read_bounded_utf8(&sandbox_root, &cand, opts.max_config_file_bytes)?;
                maps_from_cycle = merge_maps(maps_from_cycle, parse_mapcycle_txt(&raw));
            }
        }
        let merged_maps = merge_maps(maps_from_ini, maps_from_cycle);
        if !merged_maps.is_empty() {
            let maps_toml = render_maps_toml(&merged_maps);
            let maps_toml_path = out_configs.join("maps.toml");
            write_file_checked(&maps_toml_path, maps_toml.as_bytes())?;
            report.written_files.push(maps_toml_path);

            let mapcycle_text = render_mapcycle_txt(&merged_maps);
            let mapcycle_out = out_dir.join("mapcycle.txt");
            write_file_checked(&mapcycle_out, mapcycle_text.as_bytes())?;
            report.written_files.push(mapcycle_out);
            report.maps_imported = merged_maps;
        }

        // 4. configs/amxx.cfg -> configs/imported_amxx.cfg
        if let Some(ref cfg_dir) = configs_src_verified {
            let cfg_path = cfg_dir.join("amxx.cfg");
            if path_exists_or_symlink(&cfg_path) {
                let raw =
                    read_bounded_utf8(&sandbox_root, &cfg_path, opts.max_config_file_bytes)?;
                let parsed_cfg = parse_amxx_cfg(&raw);
                report.cvars_imported = parsed_cfg.entries.len();
                report.warnings.extend(parsed_cfg.warnings.clone());
                let rendered_cfg = render_imported_amxx_cfg(&parsed_cfg);
                let dest = out_configs.join("imported_amxx.cfg");
                write_file_checked(&dest, rendered_cfg.as_bytes())?;
                report.written_files.push(dest);
                report.cfg_entries = parsed_cfg.entries;
            }
        }

        // 5. data/lang/*.txt -> configs/lang/amxx/*.txt + configs/lang/amxx_imported.toml
        let lang_src = amxx_root.join("data").join("lang");
        if path_exists_or_symlink(&lang_src) {
            let verified_lang = verify_dir_under_root(&sandbox_root, &lang_src)?;
            let mut lang_files: Vec<PathBuf> = read_dir_sorted(&verified_lang)?
                .into_iter()
                .filter(|p| {
                    p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("txt"))
                })
                .collect();
            if lang_files.len() > MAX_LANG_FILES {
                report.warnings.push(format!(
                    "data/lang/: found {} files; capped at {MAX_LANG_FILES}",
                    lang_files.len()
                ));
                lang_files.truncate(MAX_LANG_FILES);
            }

            let out_lang_amxx = out_configs.join("lang").join("amxx");
            let mut merged_tables: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();

            for src_file in lang_files {
                let fname_raw = src_file
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                let fname = validate_safe_filename(fname_raw)?;
                let raw =
                    read_bounded_utf8(&sandbox_root, &src_file, opts.max_config_file_bytes)?;
                let parsed_tables = parse_amxx_lang_txt(&raw);
                let mut entry_count = 0usize;
                let mut langs = Vec::new();
                for (lang_code, kv) in parsed_tables {
                    if !kv.is_empty() {
                        langs.push(lang_code.clone());
                        entry_count += kv.len();
                        let dst_table = merged_tables.entry(lang_code).or_default();
                        for (k, v) in kv {
                            dst_table.insert(k, v);
                        }
                    }
                }
                create_dir_all_checked(&out_lang_amxx)?;
                let dest_txt = out_lang_amxx.join(&fname);
                write_file_checked(&dest_txt, raw.as_bytes())?;
                report.written_files.push(dest_txt);
                report.imported_lang_files.push(ImportedLangFile {
                    filename: fname,
                    entries_count: entry_count,
                    languages: langs,
                });
            }

            if !merged_tables.is_empty() {
                let index_toml =
                    render_lang_index_toml(&merged_tables, &report.imported_lang_files);
                let index_path = out_configs.join("lang").join("amxx_imported.toml");
                write_file_checked(&index_path, index_toml.as_bytes())?;
                report.written_files.push(index_path);
            }
        }

        // 6. Unsupported Metamod .dll/.so & AMXX modules.ini detection
        if let Some(ref cfg_dir) = configs_src_verified {
            let modules_ini = cfg_dir.join("modules.ini");
            if path_exists_or_symlink(&modules_ini) {
                let raw =
                    read_bounded_utf8(&sandbox_root, &modules_ini, opts.max_config_file_bytes)?;
                let parsed_mods = parse_modules_ini(&raw, "configs/modules.ini");
                report.native_amxx_modules = parsed_mods.native_supported;
                report
                    .unsupported_native_modules
                    .extend(parsed_mods.unsupported);
            }
        }

        let metamod_candidates = if has_cstrike_root {
            vec![
                (
                    "addons/metamod/plugins.ini",
                    sandbox_root
                        .join("addons")
                        .join("metamod")
                        .join("plugins.ini"),
                ),
                (
                    "metamod/plugins.ini",
                    sandbox_root.join("metamod").join("plugins.ini"),
                ),
            ]
        } else {
            vec![(
                "metamod/plugins.ini",
                sandbox_root.join("metamod").join("plugins.ini"),
            )]
        };
        for (label, mm_path) in metamod_candidates {
            if path_exists_or_symlink(&mm_path) {
                let raw =
                    read_bounded_utf8(&sandbox_root, &mm_path, opts.max_config_file_bytes)?;
                let detected = parse_metamod_plugins_ini(&raw, label);
                report.unsupported_native_modules.extend(detected);
            }
        }

        Ok(report)
    }

    fn path_exists_or_symlink(path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok()
    }

    fn reject_parent_components(path: &Path) -> Result<(), ImportError> {
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(ImportError::PathTraversal(path.display().to_string()));
        }
        Ok(())
    }

    fn verify_path_under_root(
        sandbox_root: &Path,
        candidate: &Path,
    ) -> Result<PathBuf, ImportError> {
        reject_parent_components(candidate)?;
        let meta = fs::symlink_metadata(candidate).map_err(|e| ImportError::Io {
            path: candidate.display().to_string(),
            message: e.to_string(),
        })?;
        let canonical = fs::canonicalize(candidate).map_err(|e| {
            if meta.file_type().is_symlink() {
                ImportError::SymlinkEscape(candidate.display().to_string())
            } else {
                ImportError::Io {
                    path: candidate.display().to_string(),
                    message: e.to_string(),
                }
            }
        })?;
        if !canonical.starts_with(sandbox_root) {
            return Err(ImportError::SymlinkEscape(candidate.display().to_string()));
        }
        Ok(canonical)
    }

    fn verify_dir_under_root(
        sandbox_root: &Path,
        candidate: &Path,
    ) -> Result<PathBuf, ImportError> {
        let canonical = verify_path_under_root(sandbox_root, candidate)?;
        if !canonical.is_dir() {
            return Err(ImportError::Io {
                path: candidate.display().to_string(),
                message: "expected directory".to_string(),
            });
        }
        Ok(canonical)
    }

    fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>, ImportError> {
        let mut paths = Vec::new();
        let entries = fs::read_dir(dir).map_err(|e| ImportError::Io {
            path: dir.display().to_string(),
            message: e.to_string(),
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| ImportError::Io {
                path: dir.display().to_string(),
                message: e.to_string(),
            })?;
            paths.push(entry.path());
        }
        paths.sort();
        Ok(paths)
    }

    fn read_bounded_bytes(
        sandbox_root: &Path,
        path: &Path,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ImportError> {
        let canonical = verify_path_under_root(sandbox_root, path)?;
        let meta = fs::metadata(&canonical).map_err(|e| ImportError::Io {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        if !meta.is_file() {
            return Err(ImportError::Io {
                path: path.display().to_string(),
                message: "expected regular file".to_string(),
            });
        }
        if meta.len() > max_bytes as u64 {
            return Err(ImportError::FileTooLarge {
                path: path.display().to_string(),
                size: meta.len(),
                limit: max_bytes,
            });
        }
        let file = File::open(&canonical).map_err(|e| ImportError::Io {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        let mut buf = Vec::with_capacity((meta.len() as usize).min(max_bytes));
        let mut limited = file.take((max_bytes as u64).saturating_add(1));
        limited.read_to_end(&mut buf).map_err(|e| ImportError::Io {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        if buf.len() > max_bytes {
            return Err(ImportError::FileTooLarge {
                path: path.display().to_string(),
                size: buf.len() as u64,
                limit: max_bytes,
            });
        }
        Ok(buf)
    }

    fn read_bounded_utf8(
        sandbox_root: &Path,
        path: &Path,
        max_bytes: usize,
    ) -> Result<String, ImportError> {
        let bytes = read_bounded_bytes(sandbox_root, path, max_bytes)?;
        Ok(match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
        })
    }

    fn create_dir_all_checked(dir: &Path) -> Result<(), ImportError> {
        fs::create_dir_all(dir).map_err(|e| ImportError::Io {
            path: dir.display().to_string(),
            message: e.to_string(),
        })
    }

    fn write_file_checked(path: &Path, bytes: &[u8]) -> Result<(), ImportError> {
        fs::write(path, bytes).map_err(|e| ImportError::Io {
            path: path.display().to_string(),
            message: e.to_string(),
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use fs_import::{import_amxx_dir, import_amxx_dir_with_options};

// ===========================================================================
// Internal Helpers (Platform-Neutral)
// ===========================================================================

fn is_amxx_filename(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    (lower.ends_with(".amxx") && lower.len() > 5) || (lower.ends_with(".amx") && lower.len() > 4)
}

fn is_valid_map_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.contains("..")
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')
        })
        && !name.starts_with('.')
        && !name.ends_with('.')
}

fn is_ip_or_cidr(s: &str) -> bool {
    if let Some((ip_part, prefix_part)) = s.split_once('/') {
        ip_part.parse::<Ipv4Addr>().is_ok()
            && prefix_part.parse::<u8>().is_ok_and(|p| p <= 32)
    } else {
        s.parse::<Ipv4Addr>().is_ok()
    }
}

fn strip_ini_comment_outside_quotes(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_quotes = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            in_quotes = !in_quotes;
        } else if !in_quotes {
            if b == b';' {
                return &line[..i];
            }
            if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                return &line[..i];
            }
        }
        i += 1;
    }
    line
}

fn strip_cfg_comment_outside_quotes(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_quotes = false;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            in_quotes = !in_quotes;
        } else if !in_quotes && b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            return &line[..i];
        }
        i += 1;
    }
    line
}

fn split_quoted_tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() {
            chars.next();
            continue;
        }
        if ch == '"' {
            chars.next();
            let mut tok = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                tok.push(c);
            }
            out.push(tok);
        } else {
            let mut tok = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                tok.push(c);
                chars.next();
            }
            out.push(tok);
        }
    }
    out
}

fn parse_infokey_u32(info: &str, key: &str) -> Option<u32> {
    let mut parts = info.split('\\').skip(1);
    while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
        if k.eq_ignore_ascii_case(key) {
            return v.trim().parse::<u32>().ok();
        }
    }
    None
}

fn unescape_amxx_lang_value(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '^' {
            match chars.peek().copied() {
                Some('n') => {
                    chars.next();
                    out.push('\n');
                }
                Some('t') => {
                    chars.next();
                    out.push('\t');
                }
                Some('1') => {
                    chars.next();
                    out.push('\x01');
                }
                Some('3') => {
                    chars.next();
                    out.push('\x03');
                }
                Some('4') => {
                    chars.next();
                    out.push('\x04');
                }
                _ => out.push(ch),
            }
        } else if ch == '\\' && chars.peek() == Some(&'n') {
            chars.next();
            out.push('\n');
        } else {
            out.push(ch);
        }
    }
    out
}

fn escape_toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn format_toml_string_array(items: &[String]) -> String {
    let mut out = String::from("[");
    for (idx, item) in items.iter().enumerate() {
        if idx > 0 {
            out.push_str(", ");
        }
        out.push('"');
        out.push_str(&escape_toml_str(item));
        out.push('"');
    }
    out.push(']');
    out
}

#[cfg(test)]
mod tests {
    use cs_mod_api::services::{AdminRegistry, LangCatalog};

    use super::*;

    #[test]
    fn recognizes_all_25_stock_amxx_plugins() {
        let expected = [
            ("admin.amxx", "std_admin"),
            ("admin_sql.amxx", "std_admin"),
            ("admincmd.amxx", "std_admin"),
            ("adminhelp.amxx", "std_admin"),
            ("adminslots.amxx", "std_admin"),
            ("multilingual.amxx", "std_admin"),
            ("menufront.amxx", "std_admin"),
            ("cmdmenu.amxx", "std_admin"),
            ("plmenu.amxx", "std_admin"),
            ("telemenu.amxx", "std_admin"),
            ("mapsmenu.amxx", "std_admin"),
            ("pluginmenu.amxx", "std_admin"),
            ("adminchat.amxx", "std_admin"),
            ("antiflood.amxx", "antiflood_admanager.py"),
            ("scrollmsg.amxx", "antiflood_admanager.py"),
            ("imessage.amxx", "antiflood_admanager.py"),
            ("adminvote.amxx", "std_admin"),
            ("nextmap.amxx", "std_mapmanager"),
            ("mapchooser.amxx", "std_mapmanager"),
            ("timeleft.amxx", "std_mapmanager"),
            ("pausecfg.amxx", "std_admin"),
            ("statscfg.amxx", "std_statsx"),
            ("restmenu.amxx", "std_restmenu"),
            ("statsx.amxx", "std_statsx"),
            ("miscstats.amxx", "std_statsx"),
        ];
        assert_eq!(expected.len(), 25);
        assert_eq!(STOCK_AMXX_PLUGINS.len(), 25);
        for (amxx, native) in expected {
            let m = lookup_stock_amxx_plugin(amxx)
                .unwrap_or_else(|| panic!("stock plugin `{amxx}` must be recognized"));
            assert_eq!(m.native_mod, native);
            // Case-insensitive and legacy `.amx` support:
            let upper = amxx.to_ascii_uppercase();
            assert_eq!(lookup_stock_amxx_plugin(&upper).unwrap().native_mod, native);
            let legacy_amx = amxx.replace(".amxx", ".amx");
            assert_eq!(
                lookup_stock_amxx_plugin(&legacy_amx).unwrap().native_mod,
                native
            );
        }
        assert!(lookup_stock_amxx_plugin("custom_vip.amxx").is_none());
    }

    #[test]
    fn path_hygiene_rejects_traversal_and_absolute_paths() {
        assert!(validate_safe_relative_path("../evil.amxx").is_err());
        assert!(validate_safe_relative_path("sub/../../evil.amxx").is_err());
        assert!(validate_safe_relative_path("..\\evil.amxx").is_err());
        assert!(validate_safe_relative_path("/etc/passwd").is_err());
        assert!(validate_safe_relative_path("C:\\Windows\\evil.amxx").is_err());
        assert!(validate_safe_relative_path("foo\0bar.amxx").is_err());
        assert!(validate_safe_relative_path("").is_err());

        assert_eq!(
            validate_safe_relative_path("custom_vip.amxx").unwrap(),
            "custom_vip.amxx"
        );
        assert_eq!(
            validate_safe_relative_path("extra\\surf_timer.amxx").unwrap(),
            "extra/surf_timer.amxx"
        );

        assert!(validate_safe_filename("../a.txt").is_err());
        assert!(validate_safe_filename("dir/a.txt").is_err());
        assert!(validate_safe_filename("a:b.txt").is_err());
        assert_eq!(validate_safe_filename("custom.txt").unwrap(), "custom.txt");
    }

    #[test]
    fn users_ini_parses_all_auth_kinds_and_roundtrips_into_admin_registry() {
        let ini = "\u{FEFF}; Users configuration file\n\
// Comment line\n\
\"STEAM_0:1:12345\" \"\" \"abcdefghijklmnopqrstu\" \"ce\" ; owner\n\
\"VALVE_0:0:99999\" \"ignored_pw\" \"bt\" \"ce\"\n\
\"192.168.1.50\" \"\" \"cde\" \"de\"\n\
\"10.0.0.0/24\" \"\" \"b\" \"de\"\n\
\"[PRO]\" \"clan_pass\" \"mnot\" \"bk\" // case-sensitive clan tag\n\
\"SuperAdmin\" \"s3cr3t\" \"abcdefghijklmnopqrstuvwxyz\" \"a\"\n";

        let parsed = parse_users_ini(ini);
        assert_eq!(parsed.admins.len(), 6);
        assert_eq!(parsed.skipped_lines, 0);
        assert!(parsed.warnings.is_empty());

        let toml = render_admins_toml(&parsed.admins);
        let mut reg = AdminRegistry::new();
        assert_eq!(reg.load_admins_toml_str(&toml), 6);

        // SteamID STEAM_*
        let f_owner = reg.authenticate("STEAM_0:1:12345", "", "Any", "").unwrap();
        assert!(f_owner.contains(AdminFlags::IMMUNITY));
        assert!(f_owner.contains(AdminFlags::RCON));
        assert!(f_owner.contains(AdminFlags::MENU));

        // VALVE_* with `e` flag ignores password even though users.ini had "ignored_pw"
        let f_valve = reg.authenticate("VALVE_0:0:99999", "", "Any", "").unwrap();
        assert!(f_valve.contains(AdminFlags::RESERVATION));
        assert!(f_valve.contains(AdminFlags::LEVEL_H));

        // IP and CIDR
        let f_ip = reg.authenticate("", "192.168.1.50:27005", "Any", "").unwrap();
        assert!(f_ip.contains(AdminFlags::BAN));
        let f_cidr = reg.authenticate("", "10.0.0.77:27005", "Any", "").unwrap();
        assert!(f_cidr.contains(AdminFlags::RESERVATION));

        // Case-sensitive clan tag + password
        let f_clan_ok = reg
            .authenticate("", "", "[PRO] Sniper", "clan_pass")
            .unwrap();
        assert!(f_clan_ok.has_char('m') && f_clan_ok.has_char('t'));
        let f_clan_wrong_case = reg
            .authenticate("", "", "[pro] Sniper", "clan_pass")
            .unwrap();
        assert_eq!(f_clan_wrong_case, AdminFlags::USER);

        // Name + `_pw` with `a` flag (kick on bad password) + ALL 26 flags `a..z`
        assert!(reg.authenticate("", "", "SuperAdmin", "wrong").is_err());
        let f_all = reg.authenticate("", "", "SuperAdmin", "s3cr3t").unwrap();
        assert_eq!(f_all.to_flag_string(), "abcdefghijklmnopqrstuvwxyz");
    }

    #[test]
    fn plugins_ini_upgrades_stock_mounts_custom_and_tracks_disabled() {
        let main_ini = "\
; AMX Mod X plugins\n\
admin.amxx ; admin base\n\
;admin_sql.amxx ; SQL disabled\n\
admincmd.amxx\n\
antiflood.amxx\n\
mapchooser.amxx\n\
statsx.amxx\n\
restmenu.amxx\n\
custom_vip.amxx debug ; third party\n\
//old_disabled.amxx\n";
        let extra_ini = "\
my_surf.amxx\n\
restmenu.amxx disabled\n";

        let merged = merge_parsed_plugins([
            ("configs/plugins.ini", main_ini),
            ("configs/plugins-extra.ini", extra_ini),
        ])
        .unwrap();

        assert_eq!(
            merged.native_mods_enabled,
            vec![
                "std_admin",
                "std_mapmanager",
                "std_statsx",
                "antiflood_admanager.py",
            ]
        );
        assert_eq!(merged.custom_amxx.len(), 2);
        assert_eq!(merged.custom_amxx[0].name, "custom_vip.amxx");
        assert!(merged.custom_amxx[0].debug);
        assert_eq!(merged.custom_amxx[0].mount_path, "plugins/custom_vip.amxx");
        assert_eq!(merged.custom_amxx[1].name, "my_surf.amxx");
        assert_eq!(merged.disabled.len(), 3); // admin_sql, old_disabled, restmenu

        let toml = render_plugins_toml(&merged);
        assert!(toml.contains("[native]"));
        assert!(toml.contains("[amxx]"));
        assert!(toml.contains("../plugins/custom_vip.amxx"));
        assert!(toml.contains("../plugins/my_surf.amxx"));
        assert!(toml.contains("[amxx.disabled]"));
    }

    #[test]
    fn plugins_ini_rejects_path_traversal_entry() {
        let bad_ini = "admin.amxx\n../../escape.amxx\n";
        let err = parse_plugins_ini(bad_ini, "configs/plugins.ini").unwrap_err();
        assert!(matches!(err, ImportError::PathTraversal(_)));
    }

    #[test]
    fn metamod_and_modules_detection_flags_native_binaries_with_alternatives() {
        let mm = "\
; Metamod plugins.ini\n\
win32 addons\\amxmodx\\dlls\\amxmodx_mm.dll\n\
linux addons/resemiclip/resemiclip_mm_i386.so\n\
linux addons/revoice/revoice_mm_i386.so\n\
win32 addons/yapb/bin/yapb.dll\n\
linux addons/mystery/custom_mm.so\n";
        let detected = parse_metamod_plugins_ini(mm, "addons/metamod/plugins.ini");
        assert_eq!(detected.len(), 5);
        assert!(detected[0].suggested_alternative.contains("cs-amxx"));
        assert!(detected[1].suggested_alternative.contains("semiclip"));
        assert!(detected[2].suggested_alternative.contains("sv_voice_transcode"));
        assert!(detected[3].suggested_alternative.contains("cs-bot"));
        assert!(detected[4].suggested_alternative.contains("cs-mod-api v2"));

        let mods_ini = "fun\ncstrike\nhamsandwich\nreapi\nmysql\norpheu\ncustom_ext.so\n";
        let parsed_mods = parse_modules_ini(mods_ini, "configs/modules.ini");
        assert_eq!(
            parsed_mods.native_supported,
            vec!["fun", "cstrike", "hamsandwich", "reapi"]
        );
        assert_eq!(parsed_mods.unsupported.len(), 3);
        assert!(parsed_mods.unsupported[0].suggested_alternative.contains("nvault"));
        assert!(parsed_mods.unsupported[1].suggested_alternative.contains("cs-mod-api"));
    }

    #[test]
    fn lang_index_toml_roundtrips_multilingual_entries() {
        let txt = "\u{FEFF}[en]\nWELCOME = ^4Welcome %s!^1\n[ru]\nWELCOME = ^4Добро пожаловать, %s!^1\n";
        let tables = parse_amxx_lang_txt(txt);
        let meta = vec![ImportedLangFile {
            filename: "custom.txt".to_string(),
            entries_count: 2,
            languages: vec!["en".to_string(), "ru".to_string()],
        }];
        let toml = render_lang_index_toml(&tables, &meta);

        let mut catalog = LangCatalog::new();
        assert_eq!(catalog.load_toml_str(&toml), 2);
        catalog.set_player_lang(0, None, "en");
        catalog.set_player_lang(1, None, "ru");
        assert_eq!(
            catalog.tr(Some(0), "WELCOME", &[("0", "Alex")]),
            "\x04Welcome Alex!\x01"
        );
        assert_eq!(
            catalog.tr(Some(1), "WELCOME", &[("0", "Alex")]),
            "\x04Добро пожаловать, Alex!\x01"
        );
    }
}
