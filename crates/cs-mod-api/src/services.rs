//! Shared pure-Rust modding platform services used identically by `cs-mod-host` (`.wasm`/`.py`)
//! and `cs-amxx` (`.amxx`) so all plugin runtimes share one `MenuManager`, `LangCatalog`,
//! `AdminRegistry`, `NVaultStore`, $O(\log N)$ `LeaderboardStore`, and polyglot `RpcBus`.
//!
//! Clean-room GPL-3.0-or-later implementation citing AMX Mod X (`newmenus.cpp`, `CLang.cpp`,
//! `admin.sma`, `users.ini`, `nvault`, `csx`, read 2026-11-26). Zero `unsafe`, `wasm32`-clean.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::ops::{AdminFlags, LeaderboardEntry, MenuCreateSpec, MenuItemSpec, RpcValue};

// ===========================================================================
// 1. Host Paginated MenuManager (`newmenus.inc` + authentic CS 1.6 ShowMenu)
// ===========================================================================

/// Classic AMX Mod X `newmenus.inc` property constants (`MPROP_*`).
pub mod mprop {
    pub const PERPAGE: i32 = 1;
    pub const BACKNAME: i32 = 2;
    pub const NEXTNAME: i32 = 3;
    pub const EXITNAME: i32 = 4;
    pub const TITLE: i32 = 5;
    pub const EXIT: i32 = 6;
    pub const NOCOLORS: i32 = 7;
    pub const NUMBER_COLOR: i32 = 8;
    pub const SHOWPAGE: i32 = 9;
    pub const MEXIT_ALL: i32 = 1;
    pub const MEXIT_NEVER: i32 = -1;
}

/// Which runtime owns a managed menu or RPC callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginRuntimeKind {
    Wasm,
    Python,
    Amxx,
    Host,
}

/// Stored menu definition inside [`MenuManager`].
#[derive(Debug, Clone)]
pub struct ManagedMenu {
    pub id: u32,
    pub owner_id: String,
    pub owner_runtime: PluginRuntimeKind,
    pub owner_index: usize,
    pub title: String,
    pub items: Vec<MenuItemSpec>,
    pub callback_id: i32,
    pub callback_name: String,
    pub per_page: u8,
    pub back_label: String,
    pub next_label: String,
    pub exit_label: String,
    pub allow_exit: bool,
    pub no_colors: bool,
    pub show_page: bool,
    pub number_color: String,
}

/// Rendered classic GoldSrc `ShowMenu` packet payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedMenu {
    pub slot: u8,
    pub menu_id: u32,
    pub page: u16,
    pub keys_bitmask: u16,
    pub time_sec: i8,
    pub text: String,
}

#[derive(Debug, Clone)]
struct ActiveSlotMenu {
    menu_id: u32,
    page: u16,
    time_sec: i8,
    /// Mapping from pressed key `1..=9` (index `key - 1`) to absolute selectable item index.
    slot_item_map: [Option<usize>; 9],
    has_back: bool,
    has_next: bool,
    has_exit: bool,
}

/// Result of routing a client `menuselect <1..=10>` through [`MenuManager`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuSelectOutcome {
    /// Player has no host-managed plugin menu active; let vanilla `CsGame` handle `menuselect`.
    NotOwned,
    /// Player pressed `8. Back` or `9. More`; send the newly rendered page without firing callback.
    PageTurned(RenderedMenu),
    /// Player chose a selectable item (`item_index` is 0-based index into menu's selectable items).
    Selected {
        owner_id: String,
        owner_runtime: PluginRuntimeKind,
        owner_index: usize,
        menu_id: u32,
        callback_id: i32,
        callback_name: String,
        item_index: usize,
        info: String,
    },
    /// Player pressed `0. Exit` (`MENU_EXIT = -3` in AMXX).
    Exited {
        owner_id: String,
        owner_runtime: PluginRuntimeKind,
        owner_index: usize,
        menu_id: u32,
        callback_id: i32,
        callback_name: String,
    },
    /// Key was disabled or invalid on the current page; ignore without leaking to other menus.
    Ignored,
}

/// Host Paginated Menu Manager (`1..7`, `8. Back`, `9. More`, `0. Exit`, `\y`/`\w`/`\r`/`\d`/`\R`).
///
/// Tracks per-slot active `menu_id` so multiple `.wasm`, `.py`, and `.amxx` plugins never steal
/// each other's `menuselect` presses.
#[derive(Debug)]
pub struct MenuManager {
    next_id: u32,
    menus: BTreeMap<u32, ManagedMenu>,
    active: [Option<ActiveSlotMenu>; 32],
}

impl Default for MenuManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MenuManager {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            menus: BTreeMap::new(),
            active: std::array::from_fn(|_| None),
        }
    }

    pub fn create_menu(
        &mut self,
        owner_id: impl Into<String>,
        owner_runtime: PluginRuntimeKind,
        owner_index: usize,
        spec: MenuCreateSpec,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let per_page = if spec.per_page == 0 {
            0
        } else {
            spec.per_page.clamp(1, 7)
        };
        let menu = ManagedMenu {
            id,
            owner_id: owner_id.into(),
            owner_runtime,
            owner_index,
            title: spec.title,
            items: spec.items,
            callback_id: spec.callback_id,
            callback_name: spec.callback_name,
            per_page,
            back_label: spec.back_label.unwrap_or_else(|| "Back".to_string()),
            next_label: spec.next_label.unwrap_or_else(|| "More".to_string()),
            exit_label: spec.exit_label.unwrap_or_else(|| "Exit".to_string()),
            allow_exit: spec.allow_exit,
            no_colors: false,
            show_page: true,
            number_color: if spec.number_color.is_empty() {
                "\\r".to_string()
            } else {
                spec.number_color
            },
        };
        self.menus.insert(id, menu);
        id
    }

    pub fn menu(&self, menu_id: u32) -> Option<&ManagedMenu> {
        self.menus.get(&menu_id)
    }

    pub fn menu_mut(&mut self, menu_id: u32) -> Option<&mut ManagedMenu> {
        self.menus.get_mut(&menu_id)
    }

    pub fn add_item(&mut self, menu_id: u32, item: MenuItemSpec) -> bool {
        let Some(m) = self.menus.get_mut(&menu_id) else {
            return false;
        };
        m.items.push(item);
        true
    }

    pub fn set_prop_int(&mut self, menu_id: u32, prop: i32, val: i32) -> bool {
        let Some(m) = self.menus.get_mut(&menu_id) else {
            return false;
        };
        match prop {
            mprop::PERPAGE => m.per_page = if val <= 0 { 0 } else { val.clamp(1, 7) as u8 },
            mprop::EXIT => m.allow_exit = val != mprop::MEXIT_NEVER,
            mprop::NOCOLORS => m.no_colors = val != 0,
            mprop::SHOWPAGE => m.show_page = val != 0,
            _ => return false,
        }
        true
    }

    pub fn set_prop_str(&mut self, menu_id: u32, prop: i32, val: impl Into<String>) -> bool {
        let Some(m) = self.menus.get_mut(&menu_id) else {
            return false;
        };
        let val = val.into();
        match prop {
            mprop::BACKNAME => m.back_label = val,
            mprop::NEXTNAME => m.next_label = val,
            mprop::EXITNAME => m.exit_label = val,
            mprop::TITLE => m.title = val,
            mprop::NUMBER_COLOR => m.number_color = val,
            _ => return false,
        }
        true
    }

    /// Returns `(selectable_item_ref)` for 0-based `item_index` (`menu_item_getinfo`).
    pub fn selectable_item(&self, menu_id: u32, item_index: usize) -> Option<&MenuItemSpec> {
        let m = self.menus.get(&menu_id)?;
        m.items
            .iter()
            .filter(|i| !i.is_blank && !i.is_text)
            .nth(item_index)
    }

    pub fn selectable_count(&self, menu_id: u32) -> usize {
        self.menus.get(&menu_id).map_or(0, |m| {
            m.items.iter().filter(|i| !i.is_blank && !i.is_text).count()
        })
    }

    pub fn destroy_menu(&mut self, menu_id: u32) -> bool {
        for slot in &mut self.active {
            if slot.as_ref().is_some_and(|a| a.menu_id == menu_id) {
                *slot = None;
            }
        }
        self.menus.remove(&menu_id).is_some()
    }

    pub fn clear_slot(&mut self, slot: u8) {
        if let Some(entry) = self.active.get_mut(usize::from(slot)) {
            *entry = None;
        }
    }

    pub fn clear_owner(&mut self, owner_id: &str) {
        let owned: Vec<u32> = self
            .menus
            .values()
            .filter(|m| m.owner_id == owner_id)
            .map(|m| m.id)
            .collect();
        for id in owned {
            self.destroy_menu(id);
        }
    }

    /// Returns `(active_menu_id, current_page)` if `slot` currently has a host menu open.
    pub fn player_menu_info(&self, slot: u8) -> Option<(u32, u16)> {
        self.active
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .map(|a| (a.menu_id, a.page))
    }

    /// Renders `menu_id` at `page` for `slot`, recording ownership so `menuselect` routes safely.
    pub fn display(&mut self, slot: u8, menu_id: u32, page: u16, time_sec: i8) -> Option<RenderedMenu> {
        if usize::from(slot) >= self.active.len() {
            return None;
        }
        let menu = self.menus.get(&menu_id)?;
        let selectable_indices: Vec<usize> = menu
            .items
            .iter()
            .enumerate()
            .filter_map(|(raw_idx, it)| (!it.is_blank && !it.is_text).then_some(raw_idx))
            .collect();
        let total_selectable = selectable_indices.len();
        let paginated = menu.per_page > 0;
        let per_page = if paginated {
            usize::from(menu.per_page)
        } else {
            9usize
        };
        let total_pages = if total_selectable == 0 {
            1usize
        } else {
            total_selectable.div_ceil(per_page)
        };
        let page_idx = usize::from(page).min(total_pages.saturating_sub(1));
        let start_sel = page_idx * per_page;
        let end_sel = (start_sel + per_page).min(total_selectable);

        // Determine raw item span for this page (including interleaved text/blank entries).
        let raw_start = if start_sel == 0 {
            0
        } else {
            selectable_indices[start_sel]
        };
        let raw_end = if end_sel < total_selectable {
            selectable_indices[end_sel]
        } else {
            menu.items.len()
        };

        let mut text = String::new();
        if !menu.no_colors {
            text.push_str("\\y");
        }
        text.push_str(&menu.title);
        if paginated && menu.show_page && total_pages > 1 {
            if menu.no_colors {
                text.push_str(&format!(" ({}/{})", page_idx + 1, total_pages));
            } else {
                text.push_str(&format!(" \\r{}/{}", page_idx + 1, total_pages));
            }
        }
        if !menu.no_colors {
            text.push_str("\\w");
        }
        text.push_str("\n\n");

        let mut keys_bitmask = 0u16;
        let mut slot_item_map = [None; 9];
        let mut local_key = 0usize; // 0-based (key 1 = 0)
        let mut sel_cursor = start_sel;

        for raw_idx in raw_start..raw_end {
            let item = &menu.items[raw_idx];
            if item.is_blank {
                text.push('\n');
                continue;
            }
            if item.is_text {
                text.push_str(&item.label);
                text.push('\n');
                continue;
            }
            if local_key >= per_page || local_key >= 9 {
                break;
            }
            let key_num = local_key + 1;
            if item.enabled {
                keys_bitmask |= 1u16 << local_key;
                slot_item_map[local_key] = Some(sel_cursor);
                if menu.no_colors {
                    text.push_str(&format!("{key_num}. {}\n", item.label));
                } else {
                    text.push_str(&format!("{}{key_num}.\\w {}\n", menu.number_color, item.label));
                }
            } else if menu.no_colors {
                text.push_str(&format!("{key_num}. {}\n", item.label));
            } else {
                text.push_str(&format!("\\d{key_num}. {}\\w\n", item.label));
            }
            local_key += 1;
            sel_cursor += 1;
        }

        let has_back = paginated && page_idx > 0;
        let has_next = paginated && (page_idx + 1) < total_pages;
        let has_exit = menu.allow_exit;

        if paginated && total_pages > 1 {
            text.push('\n');
            if has_back {
                keys_bitmask |= 1u16 << 7; // key 8
                if menu.no_colors {
                    text.push_str(&format!("8. {}\n", menu.back_label));
                } else {
                    text.push_str(&format!("{}8.\\w {}\n", menu.number_color, menu.back_label));
                }
            } else if menu.no_colors {
                text.push_str(&format!("8. {}\n", menu.back_label));
            } else {
                text.push_str(&format!("\\d8. {}\\w\n", menu.back_label));
            }

            if has_next {
                keys_bitmask |= 1u16 << 8; // key 9
                if menu.no_colors {
                    text.push_str(&format!("9. {}\n", menu.next_label));
                } else {
                    text.push_str(&format!("{}9.\\w {}\n", menu.number_color, menu.next_label));
                }
            } else if menu.no_colors {
                text.push_str(&format!("9. {}\n", menu.next_label));
            } else {
                text.push_str(&format!("\\d9. {}\\w\n", menu.next_label));
            }
        }

        if has_exit {
            keys_bitmask |= 1u16 << 9; // key 0 (menuselect 10)
            if !(paginated && total_pages > 1) {
                text.push('\n');
            }
            if menu.no_colors {
                text.push_str(&format!("0. {}\n", menu.exit_label));
            } else {
                text.push_str(&format!("{}0.\\w {}\n", menu.number_color, menu.exit_label));
            }
        }

        self.active[usize::from(slot)] = Some(ActiveSlotMenu {
            menu_id,
            page: page_idx as u16,
            time_sec,
            slot_item_map,
            has_back,
            has_next,
            has_exit,
        });

        Some(RenderedMenu {
            slot,
            menu_id,
            page: page_idx as u16,
            keys_bitmask,
            time_sec,
            text,
        })
    }

    /// Processes `menuselect <choice>` (`1..=10`, where `10` is key `0`).
    pub fn handle_menuselect(&mut self, slot: u8, choice: i32) -> MenuSelectOutcome {
        let Some(active) = self.active.get(usize::from(slot)).and_then(Option::as_ref).cloned()
        else {
            return MenuSelectOutcome::NotOwned;
        };
        let Some(menu) = self.menus.get(&active.menu_id).cloned() else {
            self.clear_slot(slot);
            return MenuSelectOutcome::NotOwned;
        };

        // Key 0 -> menuselect 10 (or 0 in some clients)
        if choice == 10 || choice == 0 {
            if !active.has_exit {
                return MenuSelectOutcome::Ignored;
            }
            self.clear_slot(slot);
            return MenuSelectOutcome::Exited {
                owner_id: menu.owner_id,
                owner_runtime: menu.owner_runtime,
                owner_index: menu.owner_index,
                menu_id: menu.id,
                callback_id: menu.callback_id,
                callback_name: menu.callback_name,
            };
        }

        // Key 8 = Back, Key 9 = More (when paginated)
        if choice == 8 && active.has_back {
            if let Some(rendered) =
                self.display(slot, active.menu_id, active.page.saturating_sub(1), active.time_sec)
            {
                return MenuSelectOutcome::PageTurned(rendered);
            }
            return MenuSelectOutcome::Ignored;
        }
        if choice == 9 && active.has_next {
            if let Some(rendered) =
                self.display(slot, active.menu_id, active.page.saturating_add(1), active.time_sec)
            {
                return MenuSelectOutcome::PageTurned(rendered);
            }
            return MenuSelectOutcome::Ignored;
        }

        if (1..=9).contains(&choice) {
            let key_idx = (choice - 1) as usize;
            if let Some(item_index) = active.slot_item_map[key_idx] {
                let info = self
                    .selectable_item(menu.id, item_index)
                    .map(|it| it.info.clone())
                    .unwrap_or_default();
                self.clear_slot(slot);
                return MenuSelectOutcome::Selected {
                    owner_id: menu.owner_id,
                    owner_runtime: menu.owner_runtime,
                    owner_index: menu.owner_index,
                    menu_id: menu.id,
                    callback_id: menu.callback_id,
                    callback_name: menu.callback_name,
                    item_index,
                    info,
                };
            }
        }
        MenuSelectOutcome::Ignored
    }
}

// ===========================================================================
// 2. Unified Multilingual LangCatalog (`configs/lang/*.toml` + AMXX `data/lang/*.txt`)
// ===========================================================================

/// Classic AMX Mod X language target constants (`lang.inc`).
pub const LANG_SERVER: i32 = 0;
pub const LANG_PLAYER: i32 = -1;

/// Unified per-player localization catalog shared across Rust/C++/Go/Python/`.amxx` plugins.
///
/// Loads both modern `configs/lang/*.toml` (`[en]`, `[ru]`, ...) and classic AMX Mod X
/// `data/lang/*.txt` (UTF-8 / UTF-8-BOM), resolves each player's language from `userinfo`
/// (`lang` / `cl_language`) or live `/lang <code>` persisted by SteamID/IP, and falls back to
/// `amx_language` (default `"en"`).
#[derive(Debug, Clone)]
pub struct LangCatalog {
    /// `lang_code` (lowercase ISO2, e.g. `"en"`, `"ru"`) -> (`KEY` -> template).
    tables: BTreeMap<String, BTreeMap<String, String>>,
    /// Server default language (`amx_language`, default `"en"`).
    server_lang: String,
    /// Active resolved language per player slot `0..32`.
    slot_langs: [Option<String>; 32],
    /// Stable player identity key per slot (SteamID or IP without port) for `/lang` persistence.
    slot_identities: [Option<String>; 32],
    /// Persisted `/lang` user overrides keyed by SteamID or IP.
    persisted_overrides: BTreeMap<String, String>,
    /// Loaded dictionary filenames (for AMXX `register_dictionary` idempotency).
    loaded_dictionaries: Vec<String>,
}

impl Default for LangCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl LangCatalog {
    pub fn new() -> Self {
        Self {
            tables: BTreeMap::new(),
            server_lang: "en".to_string(),
            slot_langs: std::array::from_fn(|_| None),
            slot_identities: std::array::from_fn(|_| None),
            persisted_overrides: BTreeMap::new(),
            loaded_dictionaries: Vec::new(),
        }
    }

    pub fn server_lang(&self) -> &str {
        &self.server_lang
    }

    pub fn available_languages(&self) -> Vec<&str> {
        self.tables.keys().map(String::as_str).collect()
    }

    pub fn set_server_lang(&mut self, code: &str) {
        if let Some(norm) = normalize_lang_code(code) {
            self.server_lang = norm;
        }
    }

    pub fn loaded_dictionaries(&self) -> &[String] {
        &self.loaded_dictionaries
    }

    pub fn mark_dictionary_loaded(&mut self, name: &str) {
        let lower = name.to_ascii_lowercase();
        if !self.loaded_dictionaries.iter().any(|d| d == &lower) {
            self.loaded_dictionaries.push(lower);
        }
    }

    pub fn insert(&mut self, lang: &str, key: impl Into<String>, value: impl Into<String>) {
        let code = normalize_lang_code(lang).unwrap_or_else(|| lang.trim().to_ascii_lowercase());
        if code.is_empty() {
            return;
        }
        self.tables
            .entry(code)
            .or_default()
            .insert(key.into(), value.into());
    }

    /// Parses modern `configs/lang/<bundle>.toml` (`[en]`, `[ru]`, ...).
    pub fn load_toml_str(&mut self, src: &str) -> usize {
        let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
        let mut current_lang = "en".to_string();
        let mut count = 0usize;
        for raw_line in src.lines() {
            let line = strip_toml_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                let sec = line[1..line.len() - 1].trim().trim_matches('"');
                if let Some(norm) = normalize_lang_code(sec) {
                    current_lang = norm;
                }
                continue;
            }
            let Some((k_raw, v_raw)) = line.split_once('=') else {
                continue;
            };
            let key = k_raw.trim().trim_matches('"');
            if key.is_empty() {
                continue;
            }
            let val = parse_toml_string_val(v_raw.trim());
            self.insert(&current_lang, key, val);
            count += 1;
        }
        count
    }

    /// Parses classic AMX Mod X `data/lang/*.txt` (UTF-8 or UTF-8-BOM, `[en]` / `[ru]`, `KEY = value`).
    ///
    /// References AMX Mod X `CLangMngr::MergeDefinitionFile` (`amxmodx/CLang.cpp`, read 2026-11-26).
    pub fn load_amxx_txt_str(&mut self, src: &str) -> usize {
        let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
        let mut current_lang = "en".to_string();
        let mut count = 0usize;
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
            self.insert(&current_lang, key, val);
            count += 1;
        }
        count
    }

    /// Updates a player's language from `userinfo` (`lang` / `cl_language`), preserving any
    /// explicit `/lang` override saved for `identity` (SteamID or IP).
    pub fn update_player_from_userinfo(
        &mut self,
        slot: u8,
        identity: Option<&str>,
        userinfo_lang: Option<&str>,
        cl_language: Option<&str>,
    ) -> String {
        let idx = usize::from(slot);
        if idx >= self.slot_langs.len() {
            return self.server_lang.clone();
        }
        if let Some(id) = identity.filter(|s| !s.is_empty()) {
            self.slot_identities[idx] = Some(id.to_string());
            if let Some(saved) = self.persisted_overrides.get(id).cloned() {
                self.slot_langs[idx] = Some(saved.clone());
                return saved;
            }
        }
        let resolved = userinfo_lang
            .and_then(normalize_lang_code)
            .or_else(|| cl_language.and_then(normalize_lang_code));
        if let Some(code) = resolved {
            self.slot_langs[idx] = Some(code.clone());
            code
        } else {
            self.slot_langs[idx].clone().unwrap_or_else(|| self.server_lang.clone())
        }
    }

    /// Handles live `/lang <code|name>`, persisting the preference by SteamID/IP when known.
    pub fn set_player_lang(
        &mut self,
        slot: u8,
        identity: Option<&str>,
        raw_code: &str,
    ) -> Option<String> {
        let code = normalize_lang_code(raw_code)?;
        let idx = usize::from(slot);
        if idx < self.slot_langs.len() {
            self.slot_langs[idx] = Some(code.clone());
            let id_owned = identity
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or_else(|| self.slot_identities[idx].clone());
            if let Some(id) = id_owned {
                self.slot_identities[idx] = Some(id.clone());
                self.persisted_overrides.insert(id, code.clone());
            }
        }
        Some(code)
    }

    pub fn clear_slot(&mut self, slot: u8) {
        let idx = usize::from(slot);
        if idx < self.slot_langs.len() {
            self.slot_langs[idx] = None;
            self.slot_identities[idx] = None;
        }
    }

    pub fn player_lang(&self, slot: Option<u8>) -> &str {
        slot.and_then(|s| self.slot_langs.get(usize::from(s)))
            .and_then(Option::as_deref)
            .unwrap_or(&self.server_lang)
    }

    pub fn persisted_overrides(&self) -> &BTreeMap<String, String> {
        &self.persisted_overrides
    }

    pub fn load_persisted_overrides(&mut self, entries: impl IntoIterator<Item = (String, String)>) {
        for (id, lang) in entries {
            if let Some(code) = normalize_lang_code(&lang) {
                self.persisted_overrides.insert(id, code);
            }
        }
    }

    /// Raw template lookup for `lang` with fallback to `server_lang` -> `"en"`.
    pub fn lookup_key(&self, lang: &str, key: &str) -> Option<&str> {
        let norm = normalize_lang_code(lang);
        if let Some(ref code) = norm
            && let Some(val) = self.tables.get(code).and_then(|m| m.get(key))
        {
            return Some(val.as_str());
        }
        if let Some(val) = self.tables.get(&self.server_lang).and_then(|m| m.get(key)) {
            return Some(val.as_str());
        }
        self.tables.get("en").and_then(|m| m.get(key)).map(String::as_str)
    }

    /// Translates `key` for `slot` (`None` = server default `amx_language`) with named and/or
    /// positional arguments (`{name}`, `{0}`, `%s`, `%d`, `%i`, `%f`).
    pub fn tr(&self, slot: Option<u8>, key: &str, args: &[(&str, &str)]) -> String {
        let lang = self.player_lang(slot);
        let template = self.lookup_key(lang, key).unwrap_or(key);
        format_lang_template(template, args)
    }

    /// Resolves an AMX Mod X `%L` placeholder (`target_id`: `LANG_SERVER = 0`, `LANG_PLAYER = -1`,
    /// or 1-based player index `1..=32`) using `fallback_slot` when `target_id == LANG_PLAYER`.
    pub fn format_amxx_lang_key(
        &self,
        target_id: i32,
        fallback_slot: Option<u8>,
        key: &str,
        positional_args: &[String],
    ) -> String {
        let slot = if target_id == LANG_SERVER {
            None
        } else if target_id == LANG_PLAYER {
            fallback_slot
        } else if (1..=32).contains(&target_id) {
            Some((target_id - 1) as u8)
        } else {
            None
        };
        let pairs: Vec<(String, &str)> = positional_args
            .iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), v.as_str()))
            .collect();
        let borrowed: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        self.tr(slot, key, &borrowed)
    }
}

/// Normalizes a language name or ISO-639 code (e.g. `"russian"` -> `"ru"`, `"english"` -> `"en"`).
pub fn normalize_lang_code(raw: &str) -> Option<String> {
    let clean = raw.trim().to_ascii_lowercase();
    if clean.is_empty() {
        return None;
    }
    let mapped = match clean.as_str() {
        "en" | "eng" | "english" | "en_us" | "en-us" | "en_gb" | "en-gb" => "en",
        "ru" | "rus" | "russian" | "ru_ru" | "ru-ru" => "ru",
        "de" | "deu" | "ger" | "german" | "de_de" => "de",
        "es" | "spa" | "spanish" | "es_es" => "es",
        "fr" | "fra" | "fre" | "french" | "fr_fr" => "fr",
        "pl" | "pol" | "polish" | "pl_pl" => "pl",
        "uk" | "ukr" | "ukrainian" | "uk_ua" => "uk",
        "pt" | "por" | "portuguese" | "pt_br" | "brazilian" => "pt",
        "tr" | "tur" | "turkish" | "tr_tr" => "tr",
        "it" | "ita" | "italian" => "it",
        "nl" | "nld" | "dut" | "dutch" => "nl",
        "sv" | "swe" | "swedish" => "sv",
        "ro" | "ron" | "rum" | "romanian" => "ro",
        "bg" | "bul" | "bulgarian" => "bg",
        "cs" | "ces" | "cze" | "czech" => "cs",
        "zh" | "zho" | "chi" | "chinese" | "schinese" | "tchinese" => "zh",
        "ko" | "kor" | "korean" => "ko",
        "ja" | "jpn" | "japanese" => "ja",
        other if other.len() == 2 && other.bytes().all(|b| b.is_ascii_alphabetic()) => {
            return Some(other.to_string());
        }
        _ => return None,
    };
    Some(mapped.to_string())
}

fn format_lang_template(template: &str, args: &[(&str, &str)]) -> String {
    if args.is_empty() {
        return template.to_string();
    }
    let mut out = template.to_string();
    // 1. Named `{name}` and positional `{0}` placeholders
    for (idx, &(k, v)) in args.iter().enumerate() {
        if !k.is_empty() {
            let needle = format!("{{{k}}}");
            if out.contains(&needle) {
                out = out.replace(&needle, v);
            }
        }
        let pos_needle = format!("{{{idx}}}");
        if out.contains(&pos_needle) {
            out = out.replace(&pos_needle, v);
        }
    }
    // 2. Classic printf `%s` / `%d` / `%i` / `%u` / `%f` / `%.Nf` placeholders in order
    if out.contains('%') {
        let mut rendered = String::with_capacity(out.len());
        let bytes = out.as_bytes();
        let mut i = 0usize;
        let mut arg_idx = 0usize;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 1 < bytes.len() {
                if bytes[i + 1] == b'%' {
                    rendered.push('%');
                    i += 2;
                    continue;
                }
                let mut j = i + 1;
                while j < bytes.len()
                    && (bytes[j].is_ascii_digit() || matches!(bytes[j], b'.' | b'-' | b'+'))
                {
                    j += 1;
                }
                if j < bytes.len()
                    && matches!(bytes[j], b's' | b'd' | b'i' | b'u' | b'f' | b'c')
                    && let Some(&(_, val)) = args.get(arg_idx)
                {
                    rendered.push_str(val);
                    arg_idx += 1;
                    i = j + 1;
                    continue;
                }
            }
            let ch = out[i..].chars().next().unwrap_or('\0');
            rendered.push(ch);
            i += ch.len_utf8();
        }
        out = rendered;
    }
    out
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_quotes = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn parse_toml_string_val(raw: &str) -> String {
    let trimmed = raw.trim();
    let inner = if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        &trimmed[1..trimmed.len() - 1]
    } else if trimmed.len() >= 2 && trimmed.starts_with('\'') && trimmed.ends_with('\'') {
        return trimmed[1..trimmed.len() - 1].to_string();
    } else {
        trimmed
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('x') => {
                    let h1 = chars.next();
                    let h2 = chars.next();
                    if let (Some(a), Some(b)) = (h1, h2) {
                        let hex = format!("{a}{b}");
                        if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                            out.push(byte as char);
                        }
                    }
                }
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
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

// ===========================================================================
// 3. Shared Admin RBAC (`configs/admins.toml` + AMXX `users.ini`)
// ===========================================================================

/// How an [`AdminEntry`] matches a connecting player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminAuthKind {
    SteamId,
    IpOrCidr,
    Name,
    ClanTag,
}

/// One parsed admin account entry from `configs/admins.toml` or `users.ini`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminEntry {
    pub auth: String,
    pub password: String,
    pub access_flags: AdminFlags,
    pub auth_kind: AdminAuthKind,
    /// `e` flag in `users.ini`: do not check password.
    pub skip_password: bool,
    /// `k` flag in `users.ini`: case-sensitive name/tag match.
    pub case_sensitive: bool,
    /// `a` flag in `users.ini`: disconnect player on invalid password.
    pub kick_on_bad_password: bool,
}

/// Shared Admin RBAC registry matching AMX Mod X `users.ini` + `configs/admins.toml`.
#[derive(Debug, Clone)]
pub struct AdminRegistry {
    entries: Vec<AdminEntry>,
    default_flags: AdminFlags,
    slot_flags: [AdminFlags; 32],
}

impl Default for AdminRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AdminRegistry {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            default_flags: AdminFlags::USER,
            slot_flags: [AdminFlags::USER; 32],
        }
    }

    pub fn entries(&self) -> &[AdminEntry] {
        &self.entries
    }

    pub fn add_entry(&mut self, entry: AdminEntry) {
        self.entries.push(entry);
    }

    pub fn clear_entries(&mut self) {
        self.entries.clear();
    }

    pub fn set_default_flags(&mut self, flags: AdminFlags) {
        self.default_flags = flags;
    }

    /// Parses classic AMX Mod X `users.ini` (`"<auth>" "<password>" "<access>" "<account>"`).
    ///
    /// References AMX Mod X `plugins/admin.sma` (`loadSettings`, read 2026-11-26).
    pub fn load_users_ini_str(&mut self, src: &str) -> usize {
        let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
        let mut loaded = 0usize;
        for raw_line in src.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with("//") {
                continue;
            }
            let tokens = split_quoted_tokens(line);
            if tokens.len() < 3 {
                continue;
            }
            let auth = tokens[0].clone();
            let password = tokens[1].clone();
            let access_flags = AdminFlags::parse_str(&tokens[2]);
            let acc = tokens.get(3).map(String::as_str).unwrap_or("");
            let auth_kind = if acc.contains('c') || auth.starts_with("STEAM_") || auth.starts_with("VALVE_") {
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
            self.entries.push(AdminEntry {
                auth,
                password,
                access_flags,
                auth_kind,
                skip_password,
                case_sensitive,
                kick_on_bad_password,
            });
            loaded += 1;
        }
        loaded
    }

    /// Parses modern `configs/admins.toml` (`[[admin]]` sections with `auth`, `flags`, `password`, `type`).
    pub fn load_admins_toml_str(&mut self, src: &str) -> usize {
        let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
        let mut loaded = 0usize;
        let mut cur_auth: Option<String> = None;
        let mut cur_pw = String::new();
        let mut cur_flags = String::new();
        let mut cur_type = String::new();
        let mut cur_case_sensitive = false;
        let mut cur_kick_bad_pw = false;

        let flush = |entries: &mut Vec<AdminEntry>,
                     loaded: &mut usize,
                     auth: &mut Option<String>,
                     pw: &mut String,
                     flags: &mut String,
                     ty: &mut String,
                     cs: &mut bool,
                     kick: &mut bool| {
            if let Some(a) = auth.take()
                && !a.is_empty()
            {
                let kind = match ty.trim().to_ascii_lowercase().as_str() {
                    "steamid" | "steam" | "authid" => AdminAuthKind::SteamId,
                    "ip" | "cidr" => AdminAuthKind::IpOrCidr,
                    "clantag" | "tag" => AdminAuthKind::ClanTag,
                    "name" => AdminAuthKind::Name,
                    _ => {
                        if a.starts_with("STEAM_") || a.starts_with("VALVE_") {
                            AdminAuthKind::SteamId
                        } else if is_ip_or_cidr(&a) {
                            AdminAuthKind::IpOrCidr
                        } else {
                            AdminAuthKind::Name
                        }
                    }
                };
                let password = std::mem::take(pw);
                let skip_password = password.is_empty();
                entries.push(AdminEntry {
                    auth: a,
                    password,
                    access_flags: AdminFlags::parse_str(flags),
                    auth_kind: kind,
                    skip_password,
                    case_sensitive: *cs,
                    kick_on_bad_password: *kick,
                });
                *loaded += 1;
            }
            pw.clear();
            flags.clear();
            ty.clear();
            *cs = false;
            *kick = false;
        };

        for raw_line in src.lines() {
            let line = strip_toml_comment(raw_line).trim();
            if line.is_empty() {
                continue;
            }
            if line == "[[admin]]" || line == "[[admins]]" {
                flush(
                    &mut self.entries,
                    &mut loaded,
                    &mut cur_auth,
                    &mut cur_pw,
                    &mut cur_flags,
                    &mut cur_type,
                    &mut cur_case_sensitive,
                    &mut cur_kick_bad_pw,
                );
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                continue;
            }
            let Some((k_raw, v_raw)) = line.split_once('=') else {
                continue;
            };
            let k = k_raw.trim();
            let v = v_raw.trim();
            match k {
                "auth" | "steamid" | "ip" | "name" => {
                    if k != "auth" && cur_type.is_empty() {
                        cur_type = k.to_string();
                    }
                    cur_auth = Some(parse_toml_string_val(v));
                }
                "password" | "pw" => cur_pw = parse_toml_string_val(v),
                "flags" | "access" => cur_flags = parse_toml_string_val(v),
                "type" | "auth_type" => cur_type = parse_toml_string_val(v),
                "case_sensitive" => cur_case_sensitive = v.eq_ignore_ascii_case("true"),
                "kick_on_bad_password" => cur_kick_bad_pw = v.eq_ignore_ascii_case("true"),
                _ => {}
            }
        }
        flush(
            &mut self.entries,
            &mut loaded,
            &mut cur_auth,
            &mut cur_pw,
            &mut cur_flags,
            &mut cur_type,
            &mut cur_case_sensitive,
            &mut cur_kick_bad_pw,
        );
        loaded
    }

    /// Evaluates matching entries for `(steamid, ip, name, _pw)` and returns the resolved flags
    /// (`Ok(flags)` or `Err(reason)` if a matching entry requires kicking on bad `_pw`).
    pub fn authenticate(
        &self,
        steamid: &str,
        ip: &str,
        name: &str,
        pw: &str,
    ) -> Result<AdminFlags, String> {
        let ip_no_port = ip.split(':').next().unwrap_or(ip);
        let mut combined = 0u32;
        let mut matched_any = false;
        for entry in &self.entries {
            let id_match = match entry.auth_kind {
                AdminAuthKind::SteamId => {
                    !steamid.is_empty() && entry.auth.eq_ignore_ascii_case(steamid)
                }
                AdminAuthKind::IpOrCidr => ip_matches_pattern(ip_no_port, &entry.auth),
                AdminAuthKind::Name => {
                    if entry.case_sensitive {
                        name == entry.auth
                    } else {
                        name.eq_ignore_ascii_case(&entry.auth)
                    }
                }
                AdminAuthKind::ClanTag => {
                    if entry.case_sensitive {
                        name.contains(&entry.auth)
                    } else {
                        name.to_ascii_lowercase()
                            .contains(&entry.auth.to_ascii_lowercase())
                    }
                }
            };
            if !id_match {
                continue;
            }
            if !entry.skip_password && pw != entry.password {
                if entry.kick_on_bad_password {
                    return Err("Invalid admin password (_pw)".to_string());
                }
                continue;
            }
            matched_any = true;
            combined |= entry.access_flags.bits();
        }
        if matched_any {
            Ok(AdminFlags::from_bits(combined))
        } else {
            Ok(self.default_flags)
        }
    }

    pub fn resolve_flags(&self, steamid: &str, ip: &str, name: &str, pw: &str) -> AdminFlags {
        self.authenticate(steamid, ip, name, pw)
            .unwrap_or(self.default_flags)
    }

    pub fn update_slot(
        &mut self,
        slot: u8,
        steamid: &str,
        ip: &str,
        name: &str,
        pw: &str,
    ) -> Result<AdminFlags, String> {
        let flags = self.authenticate(steamid, ip, name, pw)?;
        if let Some(cell) = self.slot_flags.get_mut(usize::from(slot)) {
            *cell = flags;
        }
        Ok(flags)
    }

    pub fn set_slot_flags(&mut self, slot: u8, flags: AdminFlags) {
        if let Some(cell) = self.slot_flags.get_mut(usize::from(slot)) {
            *cell = flags;
        }
    }

    pub fn clear_slot(&mut self, slot: u8) {
        if let Some(cell) = self.slot_flags.get_mut(usize::from(slot)) {
            *cell = self.default_flags;
        }
    }

    pub fn player_flags(&self, slot: u8) -> AdminFlags {
        self.slot_flags
            .get(usize::from(slot))
            .copied()
            .unwrap_or(self.default_flags)
    }

    pub fn player_has_flag(&self, slot: u8, flag: char) -> bool {
        self.player_flags(slot).has_char(flag)
    }
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

fn is_ip_or_cidr(s: &str) -> bool {
    if let Some((ip_part, prefix_part)) = s.split_once('/') {
        ip_part.parse::<Ipv4Addr>().is_ok()
            && prefix_part.parse::<u8>().is_ok_and(|p| p <= 32)
    } else {
        s.parse::<Ipv4Addr>().is_ok()
    }
}

fn ip_matches_pattern(ip_str: &str, pattern: &str) -> bool {
    let Ok(ip) = ip_str.parse::<Ipv4Addr>() else {
        return false;
    };
    if let Some((net_str, prefix_str)) = pattern.split_once('/') {
        let (Ok(net), Ok(prefix)) = (net_str.parse::<Ipv4Addr>(), prefix_str.parse::<u8>()) else {
            return false;
        };
        if prefix > 32 {
            return false;
        }
        let mask = if prefix == 0 {
            0u32
        } else {
            !0u32 << (32 - prefix)
        };
        (u32::from(ip) & mask) == (u32::from(net) & mask)
    } else if let Ok(exact) = pattern.parse::<Ipv4Addr>() {
        ip == exact
    } else {
        false
    }
}

// ===========================================================================
// 4. NVaultStore (`nvault.inc`) + O(log N) Order-Statistic AVL LeaderboardStore
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NVaultEntry {
    pub value: String,
    /// Unix-like epoch seconds (`0` = permanent via `nvault_pset`, never pruned).
    pub timestamp: i64,
}

/// Pure-Rust AMX Mod X `nvault` key-value vault store.
///
/// References AMX Mod X `modules/nvault/amxxapi.cpp` (read 2026-11-26).
#[derive(Debug, Default)]
pub struct NVaultStore {
    next_handle: i32,
    by_name: BTreeMap<String, i32>,
    vaults: BTreeMap<i32, (String, BTreeMap<String, NVaultEntry>)>,
}

impl NVaultStore {
    pub fn new() -> Self {
        Self {
            next_handle: 1,
            by_name: BTreeMap::new(),
            vaults: BTreeMap::new(),
        }
    }

    pub fn open(&mut self, name: &str) -> i32 {
        let key = name.trim().to_ascii_lowercase();
        if key.is_empty() {
            return -1;
        }
        if let Some(&handle) = self.by_name.get(&key) {
            return handle;
        }
        let handle = self.next_handle.max(1);
        self.next_handle = handle.wrapping_add(1).max(1);
        self.by_name.insert(key.clone(), handle);
        self.vaults.insert(handle, (key, BTreeMap::new()));
        handle
    }

    pub fn get(&self, handle: i32, key: &str) -> Option<&str> {
        self.vaults
            .get(&handle)
            .and_then(|(_, map)| map.get(key))
            .map(|e| e.value.as_str())
    }

    pub fn lookup(&self, handle: i32, key: &str) -> Option<(&str, i64)> {
        self.vaults
            .get(&handle)
            .and_then(|(_, map)| map.get(key))
            .map(|e| (e.value.as_str(), e.timestamp))
    }

    pub fn set(&mut self, handle: i32, key: impl Into<String>, value: impl Into<String>, now_ts: i64) -> bool {
        let Some((_, map)) = self.vaults.get_mut(&handle) else {
            return false;
        };
        map.insert(
            key.into(),
            NVaultEntry {
                value: value.into(),
                timestamp: now_ts.max(1),
            },
        );
        true
    }

    pub fn pset(&mut self, handle: i32, key: impl Into<String>, value: impl Into<String>) -> bool {
        let Some((_, map)) = self.vaults.get_mut(&handle) else {
            return false;
        };
        map.insert(
            key.into(),
            NVaultEntry {
                value: value.into(),
                timestamp: 0,
            },
        );
        true
    }

    pub fn remove(&mut self, handle: i32, key: &str) -> bool {
        self.vaults
            .get_mut(&handle)
            .is_some_and(|(_, map)| map.remove(key).is_some())
    }

    /// Removes non-permanent entries (`timestamp > 0`) with `start <= timestamp <= end` (if `end == 0`,
    /// removes entries with `timestamp >= start`; if `start == 0`, removes entries with `timestamp <= end`).
    pub fn prune(&mut self, handle: i32, start: i64, end: i64) -> usize {
        let Some((_, map)) = self.vaults.get_mut(&handle) else {
            return 0;
        };
        let before = map.len();
        map.retain(|_, e| {
            if e.timestamp == 0 {
                return true; // Permanent entry
            }
            let after_start = start <= 0 || e.timestamp >= start;
            let before_end = end <= 0 || e.timestamp <= end;
            !(after_start && before_end)
        });
        before - map.len()
    }

    pub fn close(&mut self, handle: i32) -> bool {
        if let Some((name, _)) = self.vaults.remove(&handle) {
            self.by_name.remove(&name);
            true
        } else {
            false
        }
    }

    pub fn vault_entries(&self, handle: i32) -> Option<&BTreeMap<String, NVaultEntry>> {
        self.vaults.get(&handle).map(|(_, map)| map)
    }
}

/// Ordered leaderboard key: highest `score` first, then lexicographical `key` ascending.
#[derive(Debug, Clone)]
struct RankKey {
    score: f64,
    key: String,
}

impl PartialEq for RankKey {
    fn eq(&self, other: &Self) -> bool {
        self.score.to_bits() == other.score.to_bits() && self.key == other.key
    }
}
impl Eq for RankKey {}

impl Ord for RankKey {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then_with(|| self.key.cmp(&other.key))
    }
}

impl PartialOrd for RankKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Pure-Rust size-augmented AVL order-statistic tree guaranteeing $O(\log N)$ insert, remove,
/// `order_of_key` (`/rank`), and `select_kth` (`/top15`) across 50,000+ players.
#[derive(Debug, Clone)]
struct AvlNode {
    key: RankKey,
    payload: String,
    height: u32,
    size: usize,
    left: Option<Box<AvlNode>>,
    right: Option<Box<AvlNode>>,
}

impl AvlNode {
    fn new(key: RankKey, payload: String) -> Box<Self> {
        Box::new(Self {
            key,
            payload,
            height: 1,
            size: 1,
            left: None,
            right: None,
        })
    }

    fn h(n: &Option<Box<Self>>) -> u32 {
        n.as_ref().map_or(0, |x| x.height)
    }

    fn sz(n: &Option<Box<Self>>) -> usize {
        n.as_ref().map_or(0, |x| x.size)
    }

    fn update(&mut self) {
        self.height = 1 + Self::h(&self.left).max(Self::h(&self.right));
        self.size = 1 + Self::sz(&self.left) + Self::sz(&self.right);
    }

    fn balance_factor(&self) -> i32 {
        Self::h(&self.left) as i32 - Self::h(&self.right) as i32
    }

    fn rotate_right(mut y: Box<Self>) -> Box<Self> {
        let mut x = y.left.take().expect("left child exists");
        y.left = x.right.take();
        y.update();
        x.right = Some(y);
        x.update();
        x
    }

    fn rotate_left(mut x: Box<Self>) -> Box<Self> {
        let mut y = x.right.take().expect("right child exists");
        x.right = y.left.take();
        x.update();
        y.left = Some(x);
        y.update();
        y
    }

    fn rebalance(mut node: Box<Self>) -> Box<Self> {
        node.update();
        let bf = node.balance_factor();
        if bf > 1 {
            if node.left.as_ref().is_some_and(|l| l.balance_factor() < 0) {
                let left = node.left.take().unwrap();
                node.left = Some(Self::rotate_left(left));
            }
            return Self::rotate_right(node);
        }
        if bf < -1 {
            if node.right.as_ref().is_some_and(|r| r.balance_factor() > 0) {
                let right = node.right.take().unwrap();
                node.right = Some(Self::rotate_right(right));
            }
            return Self::rotate_left(node);
        }
        node
    }

    fn insert(root: Option<Box<Self>>, key: RankKey, payload: String) -> Box<Self> {
        let Some(mut node) = root else {
            return Self::new(key, payload);
        };
        match key.cmp(&node.key) {
            Ordering::Less => node.left = Some(Self::insert(node.left.take(), key, payload)),
            Ordering::Greater => node.right = Some(Self::insert(node.right.take(), key, payload)),
            Ordering::Equal => {
                node.payload = payload;
                return node;
            }
        }
        Self::rebalance(node)
    }

    fn take_min(mut node: Box<Self>) -> (Option<Box<Self>>, RankKey, String) {
        if let Some(left) = node.left.take() {
            let (new_left, min_k, min_p) = Self::take_min(left);
            node.left = new_left;
            (Some(Self::rebalance(node)), min_k, min_p)
        } else {
            (node.right.take(), node.key, node.payload)
        }
    }

    fn remove(root: Option<Box<Self>>, key: &RankKey) -> Option<Box<Self>> {
        let mut node = root?;
        match key.cmp(&node.key) {
            Ordering::Less => node.left = Self::remove(node.left.take(), key),
            Ordering::Greater => node.right = Self::remove(node.right.take(), key),
            Ordering::Equal => {
                return match (node.left.take(), node.right.take()) {
                    (None, right) => right,
                    (left, None) => left,
                    (Some(left), Some(right)) => {
                        let (new_right, min_k, min_p) = Self::take_min(right);
                        node.key = min_k;
                        node.payload = min_p;
                        node.left = Some(left);
                        node.right = new_right;
                        Some(Self::rebalance(node))
                    }
                };
            }
        }
        Some(Self::rebalance(node))
    }

    /// Returns 0-based rank (number of strictly smaller `RankKey`s, i.e. higher scores) in $O(\log N)$.
    fn order_of_key(mut cur: Option<&Self>, key: &RankKey) -> Option<usize> {
        let mut rank = 0usize;
        while let Some(node) = cur {
            match key.cmp(&node.key) {
                Ordering::Less => cur = node.left.as_deref(),
                Ordering::Greater => {
                    rank += Self::sz(&node.left) + 1;
                    cur = node.right.as_deref();
                }
                Ordering::Equal => return Some(rank + Self::sz(&node.left)),
            }
        }
        None
    }

    /// Selects the 0-based `k`-th ranked element in $O(\log N)$.
    fn select_kth(mut cur: Option<&Self>, mut k: usize) -> Option<(&RankKey, &str)> {
        while let Some(node) = cur {
            let left_sz = Self::sz(&node.left);
            if k < left_sz {
                cur = node.left.as_deref();
            } else if k == left_sz {
                return Some((&node.key, node.payload.as_str()));
            } else {
                k -= left_sz + 1;
                cur = node.right.as_deref();
            }
        }
        None
    }

    /// Collects `[start..end)` 0-based ranks in-order in $O(\log N + (\text{end}-\text{start}))$.
    fn collect_range(
        node: Option<&Self>,
        base_rank: usize,
        start: usize,
        end: usize,
        out: &mut Vec<LeaderboardEntry>,
    ) {
        let Some(n) = node else {
            return;
        };
        if start >= end || base_rank >= end || base_rank + n.size <= start {
            return;
        }
        let left_sz = Self::sz(&n.left);
        let my_rank = base_rank + left_sz;
        if start < my_rank {
            Self::collect_range(n.left.as_deref(), base_rank, start, end, out);
        }
        if my_rank >= start && my_rank < end {
            out.push(LeaderboardEntry {
                rank: my_rank + 1,
                key: n.key.key.clone(),
                score: n.key.score,
                payload_json: n.payload.clone(),
            });
        }
        if my_rank + 1 < end {
            Self::collect_range(n.right.as_deref(), my_rank + 1, start, end, out);
        }
    }
}

#[derive(Debug, Default)]
struct LeaderboardTable {
    root: Option<Box<AvlNode>>,
    by_key: HashMap<String, (f64, String)>,
}

impl LeaderboardTable {
    fn upsert(&mut self, key: String, score: f64, payload_json: String) {
        let score = if score.is_finite() { score } else { 0.0 };
        if let Some((old_score, _)) = self.by_key.get(&key) {
            let old_rk = RankKey {
                score: *old_score,
                key: key.clone(),
            };
            self.root = AvlNode::remove(self.root.take(), &old_rk);
        }
        self.by_key.insert(key.clone(), (score, payload_json.clone()));
        self.root = Some(AvlNode::insert(
            self.root.take(),
            RankKey { score, key },
            payload_json,
        ));
    }

    fn remove(&mut self, key: &str) -> bool {
        let Some((old_score, _)) = self.by_key.remove(key) else {
            return false;
        };
        let rk = RankKey {
            score: old_score,
            key: key.to_string(),
        };
        self.root = AvlNode::remove(self.root.take(), &rk);
        true
    }

    fn len(&self) -> usize {
        self.by_key.len()
    }

    fn rank(&self, key: &str) -> Option<LeaderboardEntry> {
        let (score, payload) = self.by_key.get(key)?;
        let rk = RankKey {
            score: *score,
            key: key.to_string(),
        };
        let zero_based = AvlNode::order_of_key(self.root.as_deref(), &rk)?;
        Some(LeaderboardEntry {
            rank: zero_based + 1,
            key: key.to_string(),
            score: *score,
            payload_json: payload.clone(),
        })
    }

    fn get_by_rank(&self, rank_1based: usize) -> Option<LeaderboardEntry> {
        let idx = rank_1based.checked_sub(1)?;
        let (rk, payload) = AvlNode::select_kth(self.root.as_deref(), idx)?;
        Some(LeaderboardEntry {
            rank: rank_1based,
            key: rk.key.clone(),
            score: rk.score,
            payload_json: payload.to_string(),
        })
    }

    fn top(&self, offset: usize, limit: usize) -> Vec<LeaderboardEntry> {
        let mut out = Vec::with_capacity(limit.min(self.len()));
        let end = offset.saturating_add(limit).min(self.len());
        AvlNode::collect_range(self.root.as_deref(), 0, offset, end, &mut out);
        out
    }
}

/// Multi-table $O(\log N)$ sorted leaderboard store supporting `/rank`, `/top15`, and `csx.inc`.
#[derive(Debug, Default)]
pub struct LeaderboardStore {
    tables: BTreeMap<String, LeaderboardTable>,
}

impl LeaderboardStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(
        &mut self,
        table: &str,
        key: impl Into<String>,
        score: f64,
        payload_json: impl Into<String>,
    ) {
        self.tables
            .entry(table.to_string())
            .or_default()
            .upsert(key.into(), score, payload_json.into());
    }

    pub fn remove(&mut self, table: &str, key: &str) -> bool {
        self.tables
            .get_mut(table)
            .is_some_and(|t| t.remove(key))
    }

    pub fn count(&self, table: &str) -> usize {
        self.tables.get(table).map_or(0, LeaderboardTable::len)
    }

    /// Returns `(LeaderboardEntry, total_in_table)` in $O(\log N)$.
    pub fn rank(&self, table: &str, key: &str) -> Option<(LeaderboardEntry, usize)> {
        let t = self.tables.get(table)?;
        let entry = t.rank(key)?;
        Some((entry, t.len()))
    }

    /// Returns `limit` entries starting at 0-based `offset` in $O(\log N + \text{limit})$.
    pub fn top(&self, table: &str, offset: usize, limit: usize) -> Vec<LeaderboardEntry> {
        self.tables
            .get(table)
            .map_or_else(Vec::new, |t| t.top(offset, limit))
    }

    /// Selects one entry by 1-based rank in $O(\log N)$.
    pub fn get_by_rank(&self, table: &str, rank_1based: usize) -> Option<LeaderboardEntry> {
        self.tables.get(table)?.get_by_rank(rank_1based)
    }
}

// ===========================================================================
// 5. Polyglot Inter-Plugin RPC & Event / MultiForward Bus
// ===========================================================================

/// Registered cross-plugin callable function (`export_fn` / AMXX `register_native`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcExportEntry {
    pub fn_name: String,
    pub owner_id: String,
    pub runtime: PluginRuntimeKind,
    pub owner_index: usize,
    pub callback_id: i32,
    pub callback_name: String,
}

/// Subscribed listener for a named event or `CreateMultiForward`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcEventSubscriber {
    pub owner_id: String,
    pub runtime: PluginRuntimeKind,
    pub owner_index: usize,
    pub callback_id: i32,
    pub callback_name: String,
}

/// Handle returned by `CreateMultiForward` (`amxmodx.inc`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiForwardDesc {
    pub handle: i32,
    pub name: String,
    pub exec_type: i32,
    pub param_types: Vec<i32>,
}

/// Host-side Rust RPC callback type for in-process plugins/tests.
pub type HostRpcCallback = Arc<dyn Fn(&[RpcValue]) -> RpcValue + Send + Sync>;

/// Polyglot Inter-Plugin Bus bridging `.wasm`, `.py`, `.amxx`, and host callbacks.
#[derive(Default)]
pub struct RpcBus {
    exports: BTreeMap<String, RpcExportEntry>,
    host_fns: BTreeMap<String, HostRpcCallback>,
    event_subs: BTreeMap<String, Vec<RpcEventSubscriber>>,
    forwards: BTreeMap<i32, MultiForwardDesc>,
    next_forward_handle: i32,
    /// Active synchronous native call argument frame (for AMXX `get_param` / `get_string` / `set_string`).
    call_frames: Vec<Vec<RpcValue>>,
    /// Recorded emitted events log (useful for diagnostics & tests).
    emitted_events: Vec<(String, String, Vec<RpcValue>)>,
}

impl std::fmt::Debug for RpcBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcBus")
            .field("exports", &self.exports)
            .field("host_fns_count", &self.host_fns.len())
            .field("event_subs", &self.event_subs)
            .field("forwards", &self.forwards)
            .finish()
    }
}

impl RpcBus {
    pub fn new() -> Self {
        Self {
            next_forward_handle: 1,
            ..Self::default()
        }
    }

    pub fn export_fn(&mut self, entry: RpcExportEntry) {
        self.exports.insert(entry.fn_name.clone(), entry);
    }

    pub fn register_host_fn(
        &mut self,
        fn_name: impl Into<String>,
        cb: impl Fn(&[RpcValue]) -> RpcValue + Send + Sync + 'static,
    ) {
        let name = fn_name.into();
        self.exports.insert(
            name.clone(),
            RpcExportEntry {
                fn_name: name.clone(),
                owner_id: "host".to_string(),
                runtime: PluginRuntimeKind::Host,
                owner_index: 0,
                callback_id: 0,
                callback_name: name.clone(),
            },
        );
        self.host_fns.insert(name, Arc::new(cb));
    }

    pub fn resolve_fn(&self, fn_name: &str) -> Option<&RpcExportEntry> {
        self.exports.get(fn_name)
    }

    pub fn call_host_fn(&self, fn_name: &str, args: &[RpcValue]) -> Option<RpcValue> {
        self.host_fns.get(fn_name).map(|cb| cb(args))
    }

    pub fn subscribe_event(&mut self, event_name: impl Into<String>, sub: RpcEventSubscriber) {
        let list = self.event_subs.entry(event_name.into()).or_default();
        if !list.iter().any(|s| {
            s.owner_id == sub.owner_id
                && s.callback_id == sub.callback_id
                && s.callback_name == sub.callback_name
        }) {
            list.push(sub);
        }
    }

    pub fn subscribers(&self, event_name: &str) -> Vec<RpcEventSubscriber> {
        self.event_subs.get(event_name).cloned().unwrap_or_default()
    }

    pub fn record_event(&mut self, event_name: String, payload_json: String, args: Vec<RpcValue>) {
        if self.emitted_events.len() >= 256 {
            self.emitted_events.remove(0);
        }
        self.emitted_events.push((event_name, payload_json, args));
    }

    pub fn emitted_events(&self) -> &[(String, String, Vec<RpcValue>)] {
        &self.emitted_events
    }

    pub fn create_multi_forward(
        &mut self,
        name: impl Into<String>,
        exec_type: i32,
        param_types: Vec<i32>,
    ) -> i32 {
        let handle = self.next_forward_handle.max(1);
        self.next_forward_handle = handle.wrapping_add(1).max(1);
        self.forwards.insert(
            handle,
            MultiForwardDesc {
                handle,
                name: name.into(),
                exec_type,
                param_types,
            },
        );
        handle
    }

    pub fn multi_forward(&self, handle: i32) -> Option<&MultiForwardDesc> {
        self.forwards.get(&handle)
    }

    pub fn destroy_forward(&mut self, handle: i32) -> bool {
        self.forwards.remove(&handle).is_some()
    }

    pub fn push_call_frame(&mut self, args: Vec<RpcValue>) {
        self.call_frames.push(args);
    }

    pub fn pop_call_frame(&mut self) -> Option<Vec<RpcValue>> {
        self.call_frames.pop()
    }

    pub fn current_call_frame(&self) -> Option<&[RpcValue]> {
        self.call_frames.last().map(Vec::as_slice)
    }

    pub fn current_call_frame_mut(&mut self) -> Option<&mut Vec<RpcValue>> {
        self.call_frames.last_mut()
    }

    pub fn unregister_owner(&mut self, owner_id: &str) {
        self.exports.retain(|_, e| e.owner_id != owner_id);
        for subs in self.event_subs.values_mut() {
            subs.retain(|s| s.owner_id != owner_id);
        }
    }
}

// ===========================================================================
// 6. SharedPlatformServices Handle (shared between ServerModHost & AmxxRuntime)
// ===========================================================================

/// Unified interior state shared across all loaded plugin runtimes (`.wasm`, `.py`, `.amxx`).
#[derive(Debug, Default)]
pub struct PlatformServicesInner {
    pub menus: MenuManager,
    pub lang: LangCatalog,
    pub admins: AdminRegistry,
    pub nvault: NVaultStore,
    pub leaderboards: LeaderboardStore,
    pub rpc: RpcBus,
}

/// Thread-safe cloneable handle to shared modding platform services.
#[derive(Debug, Clone, Default)]
pub struct SharedPlatformServices {
    inner: Arc<Mutex<PlatformServicesInner>>,
}

impl SharedPlatformServices {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PlatformServicesInner::default())),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, PlatformServicesInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_manager_paginates_and_isolates_per_slot_ownership() {
        let mut mgr = MenuManager::new();
        let items: Vec<MenuItemSpec> = (1..=10)
            .map(|i| MenuItemSpec::selectable(format!("Weapon {i}"), format!("wpn_{i}"), i != 2))
            .collect();
        let menu_a = mgr.create_menu(
            "vip_mod",
            PluginRuntimeKind::Wasm,
            0,
            MenuCreateSpec {
                title: "VIP Arsenal".to_string(),
                items,
                callback_id: 77,
                ..Default::default()
            },
        );
        let menu_b = mgr.create_menu(
            "admin.amxx",
            PluginRuntimeKind::Amxx,
            1,
            MenuCreateSpec {
                title: "Admin Menu".to_string(),
                items: vec![MenuItemSpec::selectable("Kick Player", "kick", true)],
                callback_id: 0,
                callback_name: "admin_cb".to_string(),
                ..Default::default()
            },
        );

        let p0 = mgr.display(0, menu_a, 0, -1).unwrap();
        let p1 = mgr.display(1, menu_b, 0, -1).unwrap();
        assert!(p0.text.contains("\\yVIP Arsenal \\r1/2\\w"));
        assert!(p0.text.contains("\\d2. Weapon 2\\w"));
        assert_eq!(p0.keys_bitmask & (1 << 1), 0, "disabled item 2 must not accept key 2");
        assert_ne!(p0.keys_bitmask & (1 << 8), 0, "key 9 More enabled on page 1");
        assert_eq!(p0.keys_bitmask & (1 << 7), 0, "key 8 Back disabled on page 1");
        assert!(p1.text.contains("\\yAdmin Menu\\w"));

        // Slot 0 turns to page 2 via key 9
        let turn = mgr.handle_menuselect(0, 9);
        let MenuSelectOutcome::PageTurned(page2) = turn else {
            panic!("expected PageTurned, got {turn:?}");
        };
        assert_eq!(page2.page, 1);
        assert!(page2.text.contains("1.\\w Weapon 8"));
        assert_ne!(page2.keys_bitmask & (1 << 7), 0, "key 8 Back enabled on page 2");

        // Slot 1 selects item 1 on its OWN menu_b — never collides with slot 0's menu_a
        let sel_b = mgr.handle_menuselect(1, 1);
        assert!(matches!(
            sel_b,
            MenuSelectOutcome::Selected { ref owner_id, menu_id, item_index: 0, ref info, .. }
            if owner_id == "admin.amxx" && menu_id == menu_b && info == "kick"
        ));

        // Slot 0 selects key 2 on page 2 -> Weapon 9 (index 8)
        let sel_a = mgr.handle_menuselect(0, 2);
        assert!(matches!(
            sel_a,
            MenuSelectOutcome::Selected { ref owner_id, menu_id, item_index: 8, ref info, .. }
            if owner_id == "vip_mod" && menu_id == menu_a && info == "wpn_9"
        ));
    }

    #[test]
    fn lang_catalog_simultaneous_ru_and_en_and_amxx_txt() {
        let mut lang = LangCatalog::new();
        lang.load_toml_str(
            r#"
            [en]
            VIP_BOUGHT = "You received {weapon}!"
            [ru]
            VIP_BOUGHT = "Вы получили {weapon}!"
            "#,
        );
        // AMXX UTF-8-BOM data/lang/*.txt
        lang.load_amxx_txt_str(
            "\u{FEFF}[en]\nWELCOME = ^4Welcome ^1%s to CS 1.6!\n[ru]\nWELCOME = ^4Добро пожаловать, ^1%s!\n",
        );

        lang.update_player_from_userinfo(0, Some("STEAM_0:0:1"), None, Some("russian"));
        lang.update_player_from_userinfo(1, Some("STEAM_0:0:2"), Some("en"), None);

        assert_eq!(
            lang.tr(Some(0), "VIP_BOUGHT", &[("weapon", "AK-47")]),
            "Вы получили AK-47!"
        );
        assert_eq!(
            lang.tr(Some(1), "VIP_BOUGHT", &[("weapon", "AK-47")]),
            "You received AK-47!"
        );
        assert_eq!(
            lang.format_amxx_lang_key(LANG_PLAYER, Some(0), "WELCOME", &["Ivan".to_string()]),
            "\x04Добро пожаловать, \x01Ivan!"
        );
        assert_eq!(
            lang.format_amxx_lang_key(LANG_SERVER, Some(0), "WELCOME", &["Server".to_string()]),
            "\x04Welcome \x01Server to CS 1.6!"
        );

        // Live `/lang ru` persists by SteamID across reconnect
        assert_eq!(lang.set_player_lang(1, Some("STEAM_0:0:2"), "russian").as_deref(), Some("ru"));
        lang.clear_slot(1);
        lang.update_player_from_userinfo(1, Some("STEAM_0:0:2"), Some("english"), None);
        assert_eq!(lang.player_lang(Some(1)), "ru");
    }

    #[test]
    fn admin_registry_loads_toml_and_users_ini_with_cidr_and_pw() {
        let mut reg = AdminRegistry::new();
        reg.load_users_ini_str(
            r#"
            ; SteamID admin
            "STEAM_0:1:42" "" "abcdefu" "ce"
            // CIDR subnet VIP
            "192.168.10.0/24" "" "bt" "de"
            "Boss" "secret123" "abcdefghijklmnopqrstu" "a"
            "#,
        );
        reg.load_admins_toml_str(
            r#"
            [[admin]]
            auth = "STEAM_0:0:99"
            flags = "t"
            type = "steamid"
            "#,
        );

        let f_steam = reg.resolve_flags("STEAM_0:1:42", "1.2.3.4:27005", "Player", "");
        assert!(f_steam.has_char('a') && f_steam.has_char('c') && f_steam.has_char('u'));

        let f_cidr = reg.resolve_flags("STEAM_ID_LAN", "192.168.10.55:27005", "LanVip", "");
        assert!(f_cidr.has_char('t') && f_cidr.has_char('b'));

        assert!(reg.authenticate("STEAM_ID_LAN", "8.8.8.8", "Boss", "wrong").is_err());
        let f_boss = reg.authenticate("STEAM_ID_LAN", "8.8.8.8", "Boss", "secret123").unwrap();
        assert!(f_boss.has_char('l') && f_boss.has_char('t'));

        let f_vip = reg.resolve_flags("STEAM_0:0:99", "8.8.8.8", "VipGuy", "");
        assert!(f_vip.has_char('t') && !f_vip.has_char('d'));

        let f_default = reg.resolve_flags("STEAM_0:0:12345", "8.8.8.8", "Nobody", "");
        assert_eq!(f_default, AdminFlags::USER);
    }

    #[test]
    fn leaderboard_store_handles_large_dataset_in_log_n_and_nvault_prunes() {
        let mut lb = LeaderboardStore::new();
        for i in 0..5_000usize {
            lb.upsert("statsx", format!("STEAM_0:0:{i}"), (i * 10) as f64, format!("{{\"k\":{i}}}"));
        }
        // Update an existing entry to #1
        lb.upsert("statsx", "STEAM_0:0:42", 999_999.0, "{\"k\":999}");
        assert_eq!(lb.count("statsx"), 5_000);
        let top3 = lb.top("statsx", 0, 3);
        assert_eq!(top3.len(), 3);
        assert_eq!(top3[0].key, "STEAM_0:0:42");
        assert_eq!(top3[0].rank, 1);
        assert_eq!(top3[1].key, "STEAM_0:0:4999");
        assert_eq!(top3[1].rank, 2);

        let (entry, total) = lb.rank("statsx", "STEAM_0:0:42").unwrap();
        assert_eq!(entry.rank, 1);
        assert_eq!(total, 5_000);
        assert_eq!(lb.get_by_rank("statsx", 2).unwrap().key, "STEAM_0:0:4999");

        let mut nv = NVaultStore::new();
        let h = nv.open("vip_vault");
        assert!(nv.set(h, "temp", "10", 100));
        assert!(nv.pset(h, "perm", "99"));
        assert_eq!(nv.lookup(h, "temp"), Some(("10", 100)));
        assert_eq!(nv.prune(h, 0, 150), 1);
        assert_eq!(nv.get(h, "temp"), None);
        assert_eq!(nv.get(h, "perm"), Some("99"));
    }
}
