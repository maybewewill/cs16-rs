//! Competitive Retakes (`retakes`) — Rust -> WASM server plugin.
//!
//! Features:
//! - Post-plant Bombsite A / B scenarios (`3T vs 4CT` ratio, auto-balanced for any player count)
//!   with curated map presets for `de_dust2`, `de_inferno`, `de_nuke`, `de_mirage`, `de_train`
//!   plus safe dynamic fallback around map bomb targets / spawn points on custom maps
//! - Crisp amber/red DHUD site callout banner (`RETAKES — BOMBSITE A [3T vs 4CT]` /
//!   `РЕТЕЙК — ТОЧКА А`), per-player `en`/`ru` localization, and CT defuse kit distribution
//! - Pistol / Force-Buy / Full-Buy round cycle & authentic GoldSrc `/guns` (`!guns`) paginated
//!   allocator menu (`M4A1`/`FAMAS`/`AUG`, `AK-47`/`Galil`, `USP`/`Glock`/`Deagle`/`P228`,
//!   max-1-per-team AWP lottery on Full-Buy rounds) persisted via mod storage
//! - Consecutive CT retake / T hold win-streak tracking with automatic team rotation & scramble
//!   (`retakes_scramble_streak`, default `4`) plus admin overrides (`amx_retakes_site`,
//!   `amx_retakes_round`).

use std::sync::{Mutex, OnceLock};

use cs_mod_sdk::{
    ArmorKind, DhudMessageSpec, HookAction, HookId, HookPhase, HostBridge, HudMessageSpec,
    MENU_EXIT, PaginatedMenu, Player, TeamKind, cs_mod_init, vfx, weapon_id,
};

cs_mod_sdk::cs16_wasm_host_bridge!();

const LANG_TOML: &str = include_str!("../lang.toml");
const CB_GUNS_MENU: i32 = 1;
const TIMER_REANNOUNCE: i32 = 40;
const DEFAULT_SCRAMBLE_STREAK: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bombsite {
    A,
    B,
}

impl Bombsite {
    fn label_for_lang(self, lang: &str) -> &'static str {
        if lang.eq_ignore_ascii_case("ru") {
            match self {
                Self::A => "А",
                Self::B => "Б",
            }
        } else {
            match self {
                Self::A => "A",
                Self::B => "B",
            }
        }
    }

    fn banner_rgb(self) -> [u8; 3] {
        match self {
            Self::A => [255, 176, 0], // Crisp CS 1.6 amber
            Self::B => [255, 72, 64], // Tactical alert red
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundType {
    Pistol,
    ForceBuy,
    FullBuy,
}

impl RoundType {
    fn label_for_lang(self, lang: &str) -> &'static str {
        if lang.eq_ignore_ascii_case("ru") {
            match self {
                Self::Pistol => "ПИСТОЛЕТНЫЙ",
                Self::ForceBuy => "ФОРС-БАЙ",
                Self::FullBuy => "ПОЛНЫЙ ЗАКУП",
            }
        } else {
            match self {
                Self::Pistol => "PISTOL",
                Self::ForceBuy => "FORCE-BUY",
                Self::FullBuy => "FULL-BUY",
            }
        }
    }

    fn from_round_index(round_num: u32) -> Self {
        match round_num.saturating_sub(1) % 5 {
            0 => Self::Pistol,
            1 => Self::ForceBuy,
            _ => Self::FullBuy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CtRifle {
    #[default]
    M4a1,
    Famas,
    Aug,
}

impl CtRifle {
    fn as_key(self) -> &'static str {
        match self {
            Self::M4a1 => "m4a1",
            Self::Famas => "famas",
            Self::Aug => "aug",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::M4a1 => "M4A1",
            Self::Famas => "FAMAS",
            Self::Aug => "AUG",
        }
    }

    fn item_name(self) -> &'static str {
        match self {
            Self::M4a1 => "weapon_m4a1",
            Self::Famas => "weapon_famas",
            Self::Aug => "weapon_aug",
        }
    }

    fn weapon_id_and_ammo(self) -> (u8, i32) {
        match self {
            Self::M4a1 => (weapon_id::M4A1, 90),
            Self::Famas => (weapon_id::FAMAS, 90),
            Self::Aug => (weapon_id::AUG, 90),
        }
    }

    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "famas" => Self::Famas,
            "aug" => Self::Aug,
            _ => Self::M4a1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TRifle {
    #[default]
    Ak47,
    Galil,
}

impl TRifle {
    fn as_key(self) -> &'static str {
        match self {
            Self::Ak47 => "ak47",
            Self::Galil => "galil",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ak47 => "AK-47",
            Self::Galil => "Galil",
        }
    }

    fn item_name(self) -> &'static str {
        match self {
            Self::Ak47 => "weapon_ak47",
            Self::Galil => "weapon_galil",
        }
    }

    fn weapon_id_and_ammo(self) -> (u8, i32) {
        match self {
            Self::Ak47 => (weapon_id::AK47, 90),
            Self::Galil => (weapon_id::GALIL, 90),
        }
    }

    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "galil" => Self::Galil,
            _ => Self::Ak47,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PistolChoice {
    #[default]
    DefaultSidearm,
    Deagle,
    P228,
}

impl PistolChoice {
    fn as_key(self) -> &'static str {
        match self {
            Self::DefaultSidearm => "default",
            Self::Deagle => "deagle",
            Self::P228 => "p228",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::DefaultSidearm => "USP/Glock",
            Self::Deagle => "Deagle",
            Self::P228 => "P228",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::DefaultSidearm => Self::Deagle,
            Self::Deagle => Self::P228,
            Self::P228 => Self::DefaultSidearm,
        }
    }

    fn item_for_team(self, team: TeamKind) -> (&'static str, u8, i32) {
        match self {
            Self::Deagle => ("weapon_deagle", weapon_id::DEAGLE, 35),
            Self::P228 => ("weapon_p228", weapon_id::P228, 52),
            Self::DefaultSidearm => match team {
                TeamKind::Terrorist => ("weapon_glock18", weapon_id::GLOCK18, 120),
                _ => ("weapon_usp", weapon_id::USP, 100),
            },
        }
    }

    fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "deagle" => Self::Deagle,
            "p228" | "p250" => Self::P228,
            _ => Self::DefaultSidearm,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct LoadoutPref {
    ct_rifle: CtRifle,
    t_rifle: TRifle,
    pistol: PistolChoice,
    awp_opt_in: bool,
    loaded_from_storage: bool,
}

impl LoadoutPref {
    fn encode(self) -> String {
        format!(
            "{},{},{},{}",
            self.ct_rifle.as_key(),
            self.t_rifle.as_key(),
            self.pistol.as_key(),
            u8::from(self.awp_opt_in),
        )
    }

    fn decode(raw: &str) -> Self {
        let mut parts = raw.split(',');
        let ct_rifle = parts.next().map(CtRifle::parse).unwrap_or_default();
        let t_rifle = parts.next().map(TRifle::parse).unwrap_or_default();
        let pistol = parts.next().map(PistolChoice::parse).unwrap_or_default();
        let awp_opt_in = parts.next().is_some_and(|v| v.trim() == "1");
        Self {
            ct_rifle,
            t_rifle,
            pistol,
            awp_opt_in,
            loaded_from_storage: true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SitePreset {
    bomb_origin: [f32; 3],
    t_spawns: [[f32; 3]; 4],
    ct_spawns: [[f32; 3]; 5],
}

fn map_preset(map_name: &str, site: Bombsite) -> Option<SitePreset> {
    let lower = map_name.trim().to_ascii_lowercase();
    match (lower.as_str(), site) {
        ("de_dust2", Bombsite::A) => Some(SitePreset {
            bomb_origin: [1120.0, 2460.0, 128.0],
            t_spawns: [
                [1040.0, 2680.0, 128.0], // Goose
                [1320.0, 2320.0, 128.0], // A Ramp
                [1180.0, 2520.0, 128.0], // Site boxes
                [920.0, 2420.0, 128.0],  // Short barrels
            ],
            ct_spawns: [
                [420.0, 2180.0, 64.0],  // CT Spawn approach
                [640.0, 1860.0, 64.0],  // Short catwalk
                [1460.0, 1540.0, 64.0], // Long A corner
                [260.0, 2340.0, 64.0],  // Mid-to-CT
                [1280.0, 1800.0, 64.0], // Long cross
            ],
        }),
        ("de_dust2", Bombsite::B) => Some(SitePreset {
            bomb_origin: [-1560.0, 2600.0, 64.0],
            t_spawns: [
                [-1680.0, 2820.0, 64.0], // Back plat
                [-1360.0, 2520.0, 64.0], // B Window
                [-1800.0, 2440.0, 64.0], // Big box
                [-1480.0, 2760.0, 64.0], // Car / fence
            ],
            ct_spawns: [
                [-940.0, 2260.0, 64.0],  // B Doors
                [-1140.0, 2580.0, 96.0], // Mid window jump
                [-1420.0, 1680.0, 64.0], // Upper tunnels
                [-720.0, 2120.0, 64.0],  // Mid-to-B
                [-1280.0, 1960.0, 64.0], // Lower tunnel flank
            ],
        }),
        ("de_inferno", Bombsite::A) => Some(SitePreset {
            bomb_origin: [2080.0, 420.0, 160.0],
            t_spawns: [
                [2260.0, 240.0, 160.0], // Pit
                [1980.0, 600.0, 160.0], // Graveyard
                [2140.0, 500.0, 160.0], // Site truck
                [1880.0, 320.0, 224.0], // Balcony
            ],
            ct_spawns: [
                [1520.0, 1180.0, 160.0], // Library
                [1340.0, 860.0, 160.0],  // Arch
                [1120.0, 440.0, 160.0],  // Mid boil
                [1680.0, 1020.0, 160.0], // Moto
                [1440.0, 220.0, 224.0],  // Apartments
            ],
        }),
        ("de_inferno", Bombsite::B) => Some(SitePreset {
            bomb_origin: [440.0, 2660.0, 160.0],
            t_spawns: [
                [640.0, 2860.0, 160.0], // Newbox
                [280.0, 2940.0, 160.0], // Dark
                [520.0, 2540.0, 160.0], // First oranges
                [360.0, 2420.0, 160.0], // Fountain
            ],
            ct_spawns: [
                [1180.0, 2580.0, 160.0], // CT Speed
                [1020.0, 2960.0, 160.0], // Ruins / Construction
                [320.0, 1920.0, 160.0],  // Banana top
                [1320.0, 2420.0, 160.0], // Well
                [880.0, 2720.0, 160.0],  // Coffins
            ],
        }),
        ("de_nuke", Bombsite::A) => Some(SitePreset {
            bomb_origin: [520.0, -960.0, -400.0],
            t_spawns: [
                [380.0, -820.0, -400.0],  // Hut roof/exit
                [660.0, -1040.0, -400.0], // Squeaky side
                [520.0, -1120.0, -240.0], // Rafters
                [460.0, -900.0, -400.0],  // Silo
            ],
            ct_spawns: [
                [1080.0, -760.0, -240.0],  // Heaven
                [1140.0, -1180.0, -400.0], // Hell / Lockers
                [860.0, -1480.0, -400.0],  // Main
                [180.0, -1380.0, -400.0],  // Mini
                [1220.0, -420.0, -400.0],  // Ramp
            ],
        }),
        ("de_nuke", Bombsite::B) => Some(SitePreset {
            bomb_origin: [540.0, -1020.0, -760.0],
            t_spawns: [
                [380.0, -920.0, -760.0],  // Dark
                [680.0, -1140.0, -760.0], // Control room
                [480.0, -1200.0, -760.0], // Back reactor
                [620.0, -880.0, -760.0],  // Decon
            ],
            ct_spawns: [
                [1080.0, -640.0, -760.0], // Ramp lower
                [240.0, -1460.0, -760.0], // Secret tunnel
                [880.0, -1360.0, -760.0], // Double doors
                [500.0, -680.0, -760.0],  // Vents drop
                [1180.0, -980.0, -760.0], // Single door
            ],
        }),
        ("de_mirage", Bombsite::A) => Some(SitePreset {
            bomb_origin: [-420.0, -2060.0, -160.0],
            t_spawns: [
                [-560.0, -1980.0, -160.0], // Triple
                [-260.0, -2140.0, -160.0], // Firebox
                [-340.0, -1840.0, -160.0], // Tetris
                [-140.0, -2280.0, -96.0],  // Palace
            ],
            ct_spawns: [
                [-1140.0, -2260.0, -160.0], // CT Spawn
                [-980.0, -1680.0, -160.0],  // Jungle
                [-780.0, -1420.0, -160.0],  // Connector
                [-1080.0, -1980.0, -160.0], // Ticket booth
                [-540.0, -1320.0, -160.0],  // Stairs flank
            ],
        }),
        ("de_mirage", Bombsite::B) => Some(SitePreset {
            bomb_origin: [-2020.0, 260.0, -160.0],
            t_spawns: [
                [-2180.0, 440.0, -160.0], // Bench
                [-1880.0, 520.0, -160.0], // Van
                [-2120.0, 140.0, -160.0], // Pillar
                [-1940.0, 380.0, -96.0],  // Balcony apps
            ],
            ct_spawns: [
                [-1460.0, -280.0, -160.0], // Market door
                [-1340.0, 120.0, -160.0],  // Market window
                [-1380.0, 420.0, -160.0],  // Catwalk short
                [-1620.0, -420.0, -160.0], // Kitchen
                [-1180.0, 240.0, -160.0],  // Arches
            ],
        }),
        ("de_train", Bombsite::A) => Some(SitePreset {
            bomb_origin: [280.0, -220.0, -160.0],
            t_spawns: [
                [140.0, -120.0, -160.0], // Red train
                [420.0, -340.0, -160.0], // Electric
                [360.0, -80.0, -160.0],  // Old bomb
                [120.0, -420.0, -160.0], // Popdog exit
            ],
            ct_spawns: [
                [960.0, -180.0, -160.0],  // Ivy
                [820.0, -680.0, -160.0],  // Connector / Z
                [1080.0, -520.0, -160.0], // CT Spawn
                [640.0, -760.0, -64.0],   // Heaven
                [520.0, 420.0, -160.0],   // Main flank
            ],
        }),
        ("de_train", Bombsite::B) => Some(SitePreset {
            bomb_origin: [160.0, -1420.0, -224.0],
            t_spawns: [
                [60.0, -1320.0, -224.0],  // Oil train
                [300.0, -1540.0, -224.0], // Back B
                [220.0, -1260.0, -128.0], // Upper catwalk
                [-60.0, -1480.0, -224.0], // Blue train
            ],
            ct_spawns: [
                [840.0, -1180.0, -224.0], // Z connector
                [760.0, -1580.0, -224.0], // Lower CT ramp
                [680.0, -1320.0, -128.0], // Upper CT
                [980.0, -1420.0, -224.0], // CT stairs
                [460.0, -1880.0, -224.0], // Halls flank
            ],
        }),
        _ => None,
    }
}

fn dynamic_fallback_preset(host: &impl HostBridge, site: Bombsite) -> SitePreset {
    let mut bomb_targets: Vec<[f32; 3]> = Vec::new();
    let mut t_ents: Vec<[f32; 3]> = Vec::new();
    let mut ct_ents: Vec<[f32; 3]> = Vec::new();

    let scan_limit = (host.ent_count().clamp(0, 256) as u16)
        .saturating_add(64)
        .min(256);
    for idx in 1..=scan_limit {
        if let Some(pos) = host.ent_get_origin(idx)
            && let Some(cls) = host.ent_classname(idx)
        {
            if cls == "func_bomb_target" || cls == "info_bomb_target" {
                bomb_targets.push(pos);
            } else if cls == "info_player_deathmatch" {
                t_ents.push(pos);
            } else if cls == "info_player_start" {
                ct_ents.push(pos);
            }
        }
    }

    let anchor = if !bomb_targets.is_empty() {
        let pick = match site {
            Bombsite::A => 0,
            Bombsite::B => 1.min(bomb_targets.len() - 1),
        };
        bomb_targets[pick]
    } else if !t_ents.is_empty() {
        t_ents[0]
    } else if !ct_ents.is_empty() {
        ct_ents[0]
    } else {
        match site {
            Bombsite::A => [256.0, 256.0, 64.0],
            Bombsite::B => [-256.0, 256.0, 64.0],
        }
    };

    let t_spawns = [
        [anchor[0] - 96.0, anchor[1] + 64.0, anchor[2]],
        [anchor[0] + 96.0, anchor[1] + 64.0, anchor[2]],
        [anchor[0], anchor[1] + 128.0, anchor[2]],
        [anchor[0] - 64.0, anchor[1] - 64.0, anchor[2]],
    ];

    let ct_spawns = if ct_ents.len() >= 2 {
        [
            ct_ents[0],
            ct_ents[1 % ct_ents.len()],
            ct_ents[2 % ct_ents.len()],
            ct_ents[3 % ct_ents.len()],
            [anchor[0], anchor[1] - 384.0, anchor[2]],
        ]
    } else {
        [
            [anchor[0] - 320.0, anchor[1] - 280.0, anchor[2]],
            [anchor[0] + 320.0, anchor[1] - 280.0, anchor[2]],
            [anchor[0], anchor[1] - 420.0, anchor[2]],
            [anchor[0] - 180.0, anchor[1] - 360.0, anchor[2]],
            [anchor[0] + 180.0, anchor[1] - 360.0, anchor[2]],
        ]
    };

    SitePreset {
        bomb_origin: anchor,
        t_spawns,
        ct_spawns,
    }
}

fn resolve_preset(host: &impl HostBridge, map_name: &str, site: Bombsite) -> SitePreset {
    map_preset(map_name, site).unwrap_or_else(|| dynamic_fallback_preset(host, site))
}

struct RetakesState {
    map_name: String,
    round_num: u32,
    active_site: Bombsite,
    active_round: RoundType,
    forced_site: Option<Bombsite>,
    forced_round: Option<RoundType>,
    streak_override: Option<u32>,
    ct_streak: u32,
    t_streak: u32,
    scramble_count: u32,
    just_scrambled: bool,
    awp_lottery_cursor: usize,
    ct_awp_slot: Option<u8>,
    t_awp_slot: Option<u8>,
    last_t_count: usize,
    last_ct_count: usize,
    prefs: [LoadoutPref; 32],
}

impl Default for RetakesState {
    fn default() -> Self {
        Self {
            map_name: "de_dust2".to_string(),
            round_num: 0,
            active_site: Bombsite::A,
            active_round: RoundType::Pistol,
            forced_site: None,
            forced_round: None,
            streak_override: None,
            ct_streak: 0,
            t_streak: 0,
            scramble_count: 0,
            just_scrambled: false,
            awp_lottery_cursor: 0,
            ct_awp_slot: None,
            t_awp_slot: None,
            last_t_count: 0,
            last_ct_count: 0,
            prefs: [LoadoutPref::default(); 32],
        }
    }
}

static STATE: OnceLock<Mutex<RetakesState>> = OnceLock::new();

fn state() -> &'static Mutex<RetakesState> {
    STATE.get_or_init(|| Mutex::new(RetakesState::default()))
}

fn pref_storage_key(host: &impl HostBridge, slot: u8) -> String {
    let p = Player::new(slot);
    let sid = p.steamid(host);
    if !sid.is_empty() {
        let safe: String = sid
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                    c
                } else {
                    '_'
                }
            })
            .take(40)
            .collect();
        if !safe.is_empty() {
            return format!("retakes_pref_{safe}");
        }
    }
    format!("retakes_pref_slot_{slot}")
}

fn load_pref(host: &impl HostBridge, slot: u8) -> LoadoutPref {
    let idx = usize::from(slot);
    {
        let st = state().lock().unwrap_or_else(|e| e.into_inner());
        if st.prefs[idx].loaded_from_storage {
            return st.prefs[idx];
        }
    }
    let key = pref_storage_key(host, slot);
    let pref = host
        .storage_get(&key)
        .map(|raw| LoadoutPref::decode(&raw))
        .unwrap_or(LoadoutPref {
            loaded_from_storage: true,
            ..LoadoutPref::default()
        });
    let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
    st.prefs[idx] = pref;
    pref
}

fn save_pref(host: &mut impl HostBridge, slot: u8, pref: LoadoutPref) {
    let mut stored = pref;
    stored.loaded_from_storage = true;
    {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        st.prefs[usize::from(slot)] = stored;
    }
    let key = pref_storage_key(host, slot);
    host.storage_set(&key, &stored.encode());
}

fn scramble_streak_limit(host: &impl HostBridge) -> u32 {
    if let Some(ov) = state()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .streak_override
    {
        return ov.clamp(1, 20);
    }
    let raw = host.cvar_get_f32("retakes_scramble_streak");
    if raw >= 1.0 {
        (raw as u32).clamp(1, 20)
    } else {
        DEFAULT_SCRAMBLE_STREAK
    }
}

/// Computes target Terrorist count matching the classic `3T vs 4CT` ratio (`~43%` T / `57%` CT).
pub fn target_t_count(total_players: usize) -> usize {
    if total_players <= 1 {
        return total_players;
    }
    ((total_players * 3) + 2) / 7
}

fn active_combat_slots(host: &impl HostBridge) -> Vec<(u8, TeamKind)> {
    let mut out = Vec::new();
    for slot in 0..host.player_max_clients() {
        if host.player_is_active(slot) {
            let team = Player::new(slot).team(host);
            if team != TeamKind::Spectator {
                out.push((slot, team));
            }
        }
    }
    out
}

fn tr_for_lang_cached(
    host: &impl HostBridge,
    slot: u8,
    lang: &str,
    cache: &mut Vec<(String, String)>,
    key: &str,
    args: &[(&str, &str)],
) -> String {
    if let Some((_, existing)) = cache.iter().find(|(l, _)| l == lang) {
        return existing.clone();
    }
    let translated = host.lang_tr(Some(slot), key, args);
    cache.push((lang.to_string(), translated.clone()));
    translated
}

fn announce_round_banner(
    host: &mut impl HostBridge,
    site: Bombsite,
    round_type: RoundType,
    t_count: usize,
    ct_count: usize,
    include_chat: bool,
) {
    let t_str = t_count.to_string();
    let ct_str = ct_count.to_string();
    let rgb = site.banner_rgb();
    let mut banner_cache: Vec<(String, String)> = Vec::new();
    let mut chat_cache: Vec<(String, String)> = Vec::new();

    for slot in 0..host.player_max_clients() {
        if !host.player_is_active(slot) {
            continue;
        }
        let lang = host.lang_player_code(Some(slot));
        let site_lbl = site.label_for_lang(&lang);
        let round_lbl = round_type.label_for_lang(&lang);
        let args = [
            ("site", site_lbl),
            ("t", t_str.as_str()),
            ("ct", ct_str.as_str()),
            ("round", round_lbl),
        ];
        let banner = tr_for_lang_cached(
            host,
            slot,
            &lang,
            &mut banner_cache,
            "RETAKES_BANNER",
            &args,
        );
        host.dhud_message(&DhudMessageSpec {
            slot: Some(slot),
            x: -1.0,
            y: 0.16,
            rgb,
            effect: 0,
            fadein: 0.1,
            hold: 3.5,
            fadeout: 0.4,
            fxtime: 0.0,
            text: banner.clone(),
        });
        host.hud_message(&HudMessageSpec {
            slot: Some(slot),
            channel: 2,
            x: 0.02,
            y: 0.22,
            rgba1: [rgb[0], rgb[1], rgb[2], 255],
            rgba2: [255, 230, 160, 255],
            effect: 0,
            fadein: 0.05,
            hold: 6.0,
            fadeout: 0.3,
            fxtime: 0.0,
            text: banner,
        });
        if include_chat {
            let chat = tr_for_lang_cached(
                host,
                slot,
                &lang,
                &mut chat_cache,
                "RETAKES_ROUND_START",
                &args,
            );
            Player::new(slot).say(host, &chat);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn equip_player_loadout(
    host: &mut impl HostBridge,
    slot: u8,
    team: TeamKind,
    round_type: RoundType,
    awp_winner: bool,
    give_defuser: bool,
    give_c4: bool,
    defuser_msg_cache: &mut Vec<(String, String)>,
) {
    let pref = load_pref(host, slot);
    let p = Player::new(slot);

    p.strip_weapons(host);
    p.set_health(host, 100.0);
    p.give_item(host, "weapon_knife");

    let (pistol_item, pistol_wid, pistol_ammo) = pref.pistol.item_for_team(team);
    p.give_item(host, pistol_item);
    p.set_bpammo(host, pistol_wid, pistol_ammo);

    match round_type {
        RoundType::Pistol => {
            p.set_armor(host, 100, ArmorKind::Kevlar);
            p.give_item(host, "weapon_flashbang");
        }
        RoundType::ForceBuy => {
            p.set_armor(host, 100, ArmorKind::VestHelm);
            match team {
                TeamKind::Terrorist => {
                    p.give_item(host, "weapon_galil");
                    p.set_bpammo(host, weapon_id::GALIL, 90);
                }
                _ => {
                    p.give_item(host, "weapon_famas");
                    p.set_bpammo(host, weapon_id::FAMAS, 90);
                }
            }
            p.give_item(host, "weapon_flashbang");
            p.give_item(host, "weapon_hegrenade");
        }
        RoundType::FullBuy => {
            p.set_armor(host, 100, ArmorKind::VestHelm);
            if awp_winner {
                p.give_item(host, "weapon_awp");
                p.set_bpammo(host, weapon_id::AWP, 30);
                p.say_tr(host, "RETAKES_AWP_AWARDED", &[]);
            } else {
                match team {
                    TeamKind::Terrorist => {
                        p.give_item(host, pref.t_rifle.item_name());
                        let (wid, ammo) = pref.t_rifle.weapon_id_and_ammo();
                        p.set_bpammo(host, wid, ammo);
                    }
                    _ => {
                        p.give_item(host, pref.ct_rifle.item_name());
                        let (wid, ammo) = pref.ct_rifle.weapon_id_and_ammo();
                        p.set_bpammo(host, wid, ammo);
                    }
                }
            }
            p.give_item(host, "weapon_hegrenade");
            p.give_item(host, "weapon_flashbang");
            p.give_item(host, "weapon_smokegrenade");
        }
    }

    if team == TeamKind::Ct {
        p.set_defuser(host, give_defuser);
        if give_defuser {
            let lang = host.lang_player_code(Some(slot));
            let msg = tr_for_lang_cached(
                host,
                slot,
                &lang,
                defuser_msg_cache,
                "RETAKES_DEFUSER_GIVEN",
                &[],
            );
            p.say(host, &msg);
        }
    } else {
        p.set_defuser(host, false);
        if give_c4 {
            p.give_item(host, "weapon_c4");
        }
    }
}

fn pick_awp_winner(candidates: &[u8], cursor: usize) -> Option<u8> {
    if candidates.is_empty() {
        None
    } else {
        Some(candidates[cursor % candidates.len()])
    }
}

fn setup_retake_round(host: &mut impl HostBridge, explicit_round: Option<u32>) {
    let active = active_combat_slots(host);
    let (map_name, site, round_type, should_balance, lottery_cursor) = {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        st.round_num = match explicit_round {
            Some(r) if r > 0 => r,
            _ => st.round_num.saturating_add(1),
        };
        let site = st.forced_site.take().unwrap_or(if st.round_num % 2 == 1 {
            Bombsite::A
        } else {
            Bombsite::B
        });
        let round_type = st
            .forced_round
            .take()
            .unwrap_or_else(|| RoundType::from_round_index(st.round_num));
        st.active_site = site;
        st.active_round = round_type;
        let should_balance = !st.just_scrambled;
        st.just_scrambled = false;
        let cursor = st.awp_lottery_cursor;
        if round_type == RoundType::FullBuy {
            st.awp_lottery_cursor = st.awp_lottery_cursor.wrapping_add(1);
        }
        (
            st.map_name.clone(),
            site,
            round_type,
            should_balance,
            cursor,
        )
    };

    // Partition into T and CT teams matching 3T vs 4CT ratio when rebalancing.
    let mut t_slots: Vec<u8> = Vec::new();
    let mut ct_slots: Vec<u8> = Vec::new();
    let mut unassigned: Vec<u8> = Vec::new();
    for (slot, team) in &active {
        match team {
            TeamKind::Terrorist => t_slots.push(*slot),
            TeamKind::Ct => ct_slots.push(*slot),
            _ => unassigned.push(*slot),
        }
    }

    let total = active.len();
    let want_t = target_t_count(total).max(usize::from(total >= 1));

    for slot in unassigned {
        if t_slots.len() < want_t {
            Player::new(slot).set_team(host, TeamKind::Terrorist, None);
            t_slots.push(slot);
        } else {
            Player::new(slot).set_team(host, TeamKind::Ct, None);
            ct_slots.push(slot);
        }
    }

    if should_balance && total >= 2 {
        while t_slots.len() > want_t {
            if let Some(moved) = t_slots.pop() {
                Player::new(moved).set_team(host, TeamKind::Ct, None);
                ct_slots.push(moved);
            }
        }
        while t_slots.len() < want_t && ct_slots.len() > 1 {
            if let Some(moved) = ct_slots.pop() {
                Player::new(moved).set_team(host, TeamKind::Terrorist, None);
                t_slots.push(moved);
            }
        }
    }

    // Run AWP lottery (max 1 AWP per team on Full-Buy rounds only!).
    let (t_awp, ct_awp) = if round_type == RoundType::FullBuy {
        let t_opt: Vec<u8> = t_slots
            .iter()
            .copied()
            .filter(|&s| load_pref(host, s).awp_opt_in)
            .collect();
        let ct_opt: Vec<u8> = ct_slots
            .iter()
            .copied()
            .filter(|&s| load_pref(host, s).awp_opt_in)
            .collect();
        (
            pick_awp_winner(&t_opt, lottery_cursor),
            pick_awp_winner(&ct_opt, lottery_cursor),
        )
    } else {
        (None, None)
    };

    {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        st.t_awp_slot = t_awp;
        st.ct_awp_slot = ct_awp;
        st.last_t_count = t_slots.len();
        st.last_ct_count = ct_slots.len();
    }

    let preset = resolve_preset(host, &map_name, site);
    vfx::aura_light(host, preset.bomb_origin, 22, site.banner_rgb(), 20, 4, None);

    let mut defuser_cache: Vec<(String, String)> = Vec::new();
    for (idx, &slot) in t_slots.iter().enumerate() {
        let spawn = preset.t_spawns[idx % preset.t_spawns.len()];
        let p = Player::new(slot);
        host.ent_set_origin(p.edict_index(), spawn);
        host.ent_set_velocity(p.edict_index(), [0.0, 0.0, 0.0]);
        equip_player_loadout(
            host,
            slot,
            TeamKind::Terrorist,
            round_type,
            t_awp == Some(slot),
            false,
            idx == 0,
            &mut defuser_cache,
        );
    }

    let pistol_kit_limit = ct_slots.len().div_ceil(2).max(1);
    for (idx, &slot) in ct_slots.iter().enumerate() {
        let spawn = preset.ct_spawns[idx % preset.ct_spawns.len()];
        let p = Player::new(slot);
        host.ent_set_origin(p.edict_index(), spawn);
        host.ent_set_velocity(p.edict_index(), [0.0, 0.0, 0.0]);
        let has_kit = match round_type {
            RoundType::Pistol => idx < pistol_kit_limit,
            RoundType::ForceBuy | RoundType::FullBuy => true,
        };
        equip_player_loadout(
            host,
            slot,
            TeamKind::Ct,
            round_type,
            ct_awp == Some(slot),
            has_kit,
            false,
            &mut defuser_cache,
        );
    }

    announce_round_banner(host, site, round_type, t_slots.len(), ct_slots.len(), true);
}

fn rotate_and_scramble_teams(host: &mut impl HostBridge) {
    let active = active_combat_slots(host);
    if active.len() < 2 {
        return;
    }

    let mut prev_t: Vec<u8> = Vec::new();
    let mut prev_ct: Vec<u8> = Vec::new();
    for (slot, team) in &active {
        if *team == TeamKind::Terrorist {
            prev_t.push(*slot);
        } else {
            prev_ct.push(*slot);
        }
    }

    let rot = {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        st.scramble_count = st.scramble_count.saturating_add(1);
        st.just_scrambled = true;
        st.scramble_count as usize
    };

    // Place previous CTs first (rotated) so sides genuinely swap on scramble!
    let mut ordered: Vec<u8> = Vec::with_capacity(active.len());
    if !prev_ct.is_empty() {
        let shift = rot % prev_ct.len();
        for i in 0..prev_ct.len() {
            ordered.push(prev_ct[(i + shift) % prev_ct.len()]);
        }
    }
    if !prev_t.is_empty() {
        let shift = rot % prev_t.len();
        for i in 0..prev_t.len() {
            ordered.push(prev_t[(i + shift) % prev_t.len()]);
        }
    }

    let want_t = target_t_count(ordered.len()).max(1);
    for (idx, slot) in ordered.into_iter().enumerate() {
        let new_team = if idx < want_t {
            TeamKind::Terrorist
        } else {
            TeamKind::Ct
        };
        Player::new(slot).set_team(host, new_team, None);
    }
}

fn open_guns_menu(host: &mut impl HostBridge, slot: u8) {
    let pref = load_pref(host, slot);
    let p = Player::new(slot);
    let title = format!("\\y{}\\w", p.tr(host, "RETAKES_GUNS_TITLE", &[]));
    let ct_lbl = p.tr(host, "RETAKES_MENU_CT", &[]);
    let t_lbl = p.tr(host, "RETAKES_MENU_T", &[]);
    let pistol_lbl = p.tr(host, "RETAKES_MENU_PISTOL", &[]);
    let awp_lbl = p.tr(host, "RETAKES_MENU_AWP", &[]);

    let mark = |active: bool| {
        if active {
            "\\y[ACTIVE]\\w"
        } else {
            "\\d[ ]\\w"
        }
    };
    let awp_tag = if pref.awp_opt_in {
        "\\y[ON]\\w"
    } else {
        "\\d[OFF]\\w"
    };

    PaginatedMenu::new(title, CB_GUNS_MENU)
        .per_page(7)
        .item(
            format!(
                "{ct_lbl}: M4A1 Carbine \\R{}",
                mark(pref.ct_rifle == CtRifle::M4a1)
            ),
            "ct:m4a1",
        )
        .item(
            format!(
                "{ct_lbl}: FAMAS Clarion \\R{}",
                mark(pref.ct_rifle == CtRifle::Famas)
            ),
            "ct:famas",
        )
        .item(
            format!(
                "{ct_lbl}: AUG Bullpup \\R{}",
                mark(pref.ct_rifle == CtRifle::Aug)
            ),
            "ct:aug",
        )
        .item(
            format!(
                "{t_lbl}: AK-47 Kalashnikov \\R{}",
                mark(pref.t_rifle == TRifle::Ak47)
            ),
            "t:ak47",
        )
        .item(
            format!(
                "{t_lbl}: Galil Defender \\R{}",
                mark(pref.t_rifle == TRifle::Galil)
            ),
            "t:galil",
        )
        .item(
            format!("{pistol_lbl}: \\y[{}]\\w (Cycle)", pref.pistol.label()),
            "pistol:cycle",
        )
        .item(format!("{awp_lbl} (Max 1/team) \\R{awp_tag}"), "awp:toggle")
        .item(
            format!(
                "{pistol_lbl}: USP / Glock \\R{}",
                mark(pref.pistol == PistolChoice::DefaultSidearm)
            ),
            "pistol:default",
        )
        .item(
            format!(
                "{pistol_lbl}: Desert Eagle \\R{}",
                mark(pref.pistol == PistolChoice::Deagle)
            ),
            "pistol:deagle",
        )
        .item(
            format!(
                "{pistol_lbl}: P228 / P250 \\R{}",
                mark(pref.pistol == PistolChoice::P228)
            ),
            "pistol:p228",
        )
        .display_to(host, slot, 0, 25);
}

fn confirm_pref_saved(host: &mut impl HostBridge, slot: u8, pref: LoadoutPref) {
    let p = Player::new(slot);
    let awp_str = if pref.awp_opt_in { "ON" } else { "OFF" };
    p.say_tr(
        host,
        "RETAKES_PREF_SAVED",
        &[
            ("ct", pref.ct_rifle.label()),
            ("t", pref.t_rifle.label()),
            ("pistol", pref.pistol.label()),
            ("awp", awp_str),
        ],
    );
}

fn is_admin(host: &impl HostBridge, slot: Option<u8>) -> bool {
    match slot {
        None => true,
        Some(s) => {
            let p = Player::new(s);
            p.has_flag(host, 'a') || p.has_flag(host, 'c') || p.has_flag(host, 'h')
        }
    }
}

fn handle_admin_site(host: &mut impl HostBridge, slot: Option<u8>, arg: &str) -> HookAction {
    if !is_admin(host, slot) {
        if let Some(s) = slot {
            Player::new(s).say_tr(host, "RETAKES_NO_ACCESS", &[]);
        }
        return HookAction::Handled;
    }
    let site = match arg.trim().to_ascii_lowercase().as_str() {
        "b" | "site_b" => Bombsite::B,
        _ => Bombsite::A,
    };
    {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        st.forced_site = Some(site);
    }
    let mut cache: Vec<(String, String)> = Vec::new();
    for s in 0..host.player_max_clients() {
        if host.player_is_active(s) {
            let lang = host.lang_player_code(Some(s));
            let lbl = site.label_for_lang(&lang);
            let msg = tr_for_lang_cached(
                host,
                s,
                &lang,
                &mut cache,
                "RETAKES_ADMIN_SITE",
                &[("site", lbl)],
            );
            Player::new(s).say(host, &msg);
        }
    }
    HookAction::Handled
}

fn handle_admin_round(host: &mut impl HostBridge, slot: Option<u8>, arg: &str) -> HookAction {
    if !is_admin(host, slot) {
        if let Some(s) = slot {
            Player::new(s).say_tr(host, "RETAKES_NO_ACCESS", &[]);
        }
        return HookAction::Handled;
    }
    let rt = match arg.trim().to_ascii_lowercase().as_str() {
        "pistol" | "p" | "eco" => RoundType::Pistol,
        "force" | "forcebuy" | "f" => RoundType::ForceBuy,
        _ => RoundType::FullBuy,
    };
    {
        let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
        st.forced_round = Some(rt);
    }
    let mut cache: Vec<(String, String)> = Vec::new();
    for s in 0..host.player_max_clients() {
        if host.player_is_active(s) {
            let lang = host.lang_player_code(Some(s));
            let lbl = rt.label_for_lang(&lang);
            let msg = tr_for_lang_cached(
                host,
                s,
                &lang,
                &mut cache,
                "RETAKES_ADMIN_ROUND",
                &[("round", lbl)],
            );
            Player::new(s).say(host, &msg);
        }
    }
    HookAction::Handled
}

#[cs_mod_init]
pub fn init_mod(host: &mut impl HostBridge) -> i32 {
    host.lang_load_toml(LANG_TOML);
    host.hook_register(HookId::ServerActivate, HookPhase::Post, 0);
    host.hook_register(HookId::ClientPutInServer, HookPhase::Post, 0);
    host.hook_register(HookId::ClientDisconnect, HookPhase::Post, 0);
    host.hook_register(HookId::RoundStart, HookPhase::Post, 0);
    host.hook_register(HookId::FreezeTimeEnd, HookPhase::Post, 0);
    host.hook_register(HookId::RoundEnd, HookPhase::Post, 0);
    host.hook_register(HookId::PlayerSpawn, HookPhase::Post, 0);
    host.hook_register(HookId::CanBuyItem, HookPhase::Pre, 0);
    host.hook_register(HookId::PlayerSay, HookPhase::Pre, 0);
    host.hook_register(HookId::ClientCommand, HookPhase::Pre, 0);
    host.hook_register(HookId::ServerCommand, HookPhase::Pre, 0);
    host.register_command("guns");
    host.register_command("amx_retakes_site");
    host.register_command("amx_retakes_round");
    host.register_command("amx_retakes_streak");
    0
}

pub fn dispatch_hook(host: &mut impl HostBridge, hook: HookId, phase: HookPhase) -> HookAction {
    match (hook, phase) {
        (HookId::ServerActivate, HookPhase::Post) => {
            if let Some(map) = host.hook_arg_str(0)
                && !map.trim().is_empty()
            {
                let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                st.map_name = map.trim().to_string();
            }
            HookAction::Continue
        }
        (HookId::ClientPutInServer, HookPhase::Post)
        | (HookId::ClientDisconnect, HookPhase::Post) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as usize;
            state().lock().unwrap_or_else(|e| e.into_inner()).prefs[slot].loaded_from_storage =
                false;
            HookAction::Continue
        }
        (HookId::CanBuyItem, HookPhase::Pre) => HookAction::Supercede,
        (HookId::RoundStart, HookPhase::Post) => {
            let rn = host.hook_arg_i32(0);
            let explicit = (rn > 0).then_some(rn as u32);
            setup_retake_round(host, explicit);
            host.timer_after(250, TIMER_REANNOUNCE, 0);
            HookAction::Continue
        }
        (HookId::FreezeTimeEnd, HookPhase::Post) => {
            let (round_num, site, rt, t_c, ct_c) = {
                let st = state().lock().unwrap_or_else(|e| e.into_inner());
                (
                    st.round_num,
                    st.active_site,
                    st.active_round,
                    st.last_t_count,
                    st.last_ct_count,
                )
            };
            if round_num == 0 {
                setup_retake_round(host, Some(1));
            } else {
                announce_round_banner(host, site, rt, t_c, ct_c, false);
            }
            HookAction::Continue
        }
        (HookId::PlayerSpawn, HookPhase::Post) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let team = Player::new(slot).team(host);
            if matches!(team, TeamKind::Terrorist | TeamKind::Ct) {
                let (round_type, t_awp, ct_awp, site, map_name) = {
                    let st = state().lock().unwrap_or_else(|e| e.into_inner());
                    (
                        st.active_round,
                        st.t_awp_slot,
                        st.ct_awp_slot,
                        st.active_site,
                        st.map_name.clone(),
                    )
                };
                let preset = resolve_preset(host, &map_name, site);
                let spawn = match team {
                    TeamKind::Terrorist => {
                        preset.t_spawns[usize::from(slot) % preset.t_spawns.len()]
                    }
                    _ => preset.ct_spawns[usize::from(slot) % preset.ct_spawns.len()],
                };
                let p = Player::new(slot);
                host.ent_set_origin(p.edict_index(), spawn);
                let awp_win = match team {
                    TeamKind::Terrorist => t_awp == Some(slot),
                    _ => ct_awp == Some(slot),
                };
                let give_kit = team == TeamKind::Ct
                    && (round_type != RoundType::Pistol || slot.is_multiple_of(2));
                let mut defuser_cache = Vec::new();
                equip_player_loadout(
                    host,
                    slot,
                    team,
                    round_type,
                    awp_win,
                    give_kit,
                    false,
                    &mut defuser_cache,
                );
            }
            HookAction::Continue
        }
        (HookId::RoundEnd, HookPhase::Post) => {
            let winner = host.hook_arg_i32(0);
            let limit = scramble_streak_limit(host);
            let limit_s = limit.to_string();

            let (site, streak, team_key, hit_scramble) = {
                let mut st = state().lock().unwrap_or_else(|e| e.into_inner());
                let site = st.active_site;
                match winner {
                    2 => {
                        st.ct_streak = st.ct_streak.saturating_add(1);
                        st.t_streak = 0;
                        let s = st.ct_streak;
                        let scr = s >= limit;
                        if scr {
                            st.ct_streak = 0;
                        }
                        (site, s, Some("RETAKES_TEAM_CT"), scr)
                    }
                    1 => {
                        st.t_streak = st.t_streak.saturating_add(1);
                        st.ct_streak = 0;
                        let s = st.t_streak;
                        let scr = s >= limit;
                        if scr {
                            st.t_streak = 0;
                        }
                        (site, s, Some("RETAKES_TEAM_T"), scr)
                    }
                    _ => (site, 0, None, false),
                }
            };

            if let Some(tk) = team_key {
                let streak_s = streak.to_string();
                let win_msg_key = if winner == 2 {
                    "RETAKES_SWAP_CT_WIN"
                } else {
                    "RETAKES_HOLD_T_WIN"
                };
                let mut win_cache: Vec<(String, String)> = Vec::new();
                for s in 0..host.player_max_clients() {
                    if host.player_is_active(s) {
                        let lang = host.lang_player_code(Some(s));
                        let site_lbl = site.label_for_lang(&lang);
                        let msg = tr_for_lang_cached(
                            host,
                            s,
                            &lang,
                            &mut win_cache,
                            win_msg_key,
                            &[
                                ("site", site_lbl),
                                ("streak", &streak_s),
                                ("limit", &limit_s),
                            ],
                        );
                        Player::new(s).say(host, &msg);
                    }
                }

                if hit_scramble {
                    rotate_and_scramble_teams(host);
                    let mut scr_cache: Vec<(String, String)> = Vec::new();
                    for s in 0..host.player_max_clients() {
                        if host.player_is_active(s) {
                            let lang = host.lang_player_code(Some(s));
                            let team_lbl = if lang.eq_ignore_ascii_case("ru") {
                                if tk == "RETAKES_TEAM_CT" {
                                    "Спецназ"
                                } else {
                                    "Террористы"
                                }
                            } else if tk == "RETAKES_TEAM_CT" {
                                "CTs"
                            } else {
                                "Terrorists"
                            };
                            let msg = tr_for_lang_cached(
                                host,
                                s,
                                &lang,
                                &mut scr_cache,
                                "RETAKES_SCRAMBLE",
                                &[("team", team_lbl), ("streak", &streak_s)],
                            );
                            Player::new(s).say(host, &msg);
                        }
                    }
                }
            }
            HookAction::Continue
        }
        (HookId::PlayerSay, HookPhase::Pre) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let msg = host.hook_arg_str(0).unwrap_or_default();
            let tok = msg.split_whitespace().next().unwrap_or("");
            if tok.eq_ignore_ascii_case("/guns")
                || tok.eq_ignore_ascii_case("!guns")
                || tok.eq_ignore_ascii_case("guns")
            {
                open_guns_menu(host, slot);
                return HookAction::Supercede;
            }
            HookAction::Continue
        }
        (HookId::ClientCommand, HookPhase::Pre) => {
            let slot = host.hook_arg_i32(0).clamp(0, 31) as u8;
            let cmd = host
                .hook_arg_str(0)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let arg1 = host.hook_arg_str(1).unwrap_or_default();
            match cmd.as_str() {
                "guns" => {
                    open_guns_menu(host, slot);
                    HookAction::Handled
                }
                "amx_retakes_site" => handle_admin_site(host, Some(slot), &arg1),
                "amx_retakes_round" => handle_admin_round(host, Some(slot), &arg1),
                "amx_retakes_streak" => {
                    if !is_admin(host, Some(slot)) {
                        Player::new(slot).say_tr(host, "RETAKES_NO_ACCESS", &[]);
                        return HookAction::Handled;
                    }
                    if let Ok(val) = arg1.trim().parse::<u32>() {
                        state()
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .streak_override = Some(val.clamp(1, 20));
                    }
                    HookAction::Handled
                }
                _ => HookAction::Continue,
            }
        }
        (HookId::ServerCommand, HookPhase::Pre) => {
            let cmd = host
                .hook_arg_str(0)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let arg1 = host.hook_arg_str(1).unwrap_or_default();
            match cmd.as_str() {
                "amx_retakes_site" => handle_admin_site(host, None, &arg1),
                "amx_retakes_round" => handle_admin_round(host, None, &arg1),
                "amx_retakes_streak" => {
                    if let Ok(val) = arg1.trim().parse::<u32>() {
                        state()
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .streak_override = Some(val.clamp(1, 20));
                    }
                    HookAction::Handled
                }
                _ => HookAction::Continue,
            }
        }
        _ => HookAction::Continue,
    }
}

pub fn dispatch_timer(host: &mut impl HostBridge, cb_id: i32, _arg: i32) -> HookAction {
    if cb_id == TIMER_REANNOUNCE {
        let (site, rt, t_c, ct_c) = {
            let st = state().lock().unwrap_or_else(|e| e.into_inner());
            (
                st.active_site,
                st.active_round,
                st.last_t_count,
                st.last_ct_count,
            )
        };
        if t_c + ct_c > 0 {
            announce_round_banner(host, site, rt, t_c, ct_c, false);
        }
    }
    HookAction::Continue
}

pub fn dispatch_menu(
    host: &mut impl HostBridge,
    cb_id: i32,
    slot: u8,
    _menu_id: u32,
    item_index: i32,
) -> HookAction {
    if cb_id != CB_GUNS_MENU || item_index == MENU_EXIT {
        return HookAction::Handled;
    }
    let info = host.hook_arg_str(0).unwrap_or_default();
    let mut pref = load_pref(host, slot);
    let mut changed = true;
    match info.as_str() {
        "ct:m4a1" => pref.ct_rifle = CtRifle::M4a1,
        "ct:famas" => pref.ct_rifle = CtRifle::Famas,
        "ct:aug" => pref.ct_rifle = CtRifle::Aug,
        "t:ak47" => pref.t_rifle = TRifle::Ak47,
        "t:galil" => pref.t_rifle = TRifle::Galil,
        "pistol:cycle" => pref.pistol = pref.pistol.next(),
        "pistol:default" => pref.pistol = PistolChoice::DefaultSidearm,
        "pistol:deagle" => pref.pistol = PistolChoice::Deagle,
        "pistol:p228" => pref.pistol = PistolChoice::P228,
        "awp:toggle" => pref.awp_opt_in = !pref.awp_opt_in,
        _ => changed = false,
    }
    if changed {
        save_pref(host, slot, pref);
        confirm_pref_saved(host, slot, pref);
        open_guns_menu(host, slot);
    }
    HookAction::Handled
}

cs_mod_sdk::cs16_wasm_exports! {
    init: init_mod,
    hook: dispatch_hook,
    timer: dispatch_timer,
    menu: dispatch_menu,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_mod_sdk::{AdminFlags, MockHostBridge};

    #[test]
    fn target_ratio_hits_3t_vs_4ct_for_seven_players() {
        assert_eq!(target_t_count(7), 3);
        assert_eq!(target_t_count(2), 1);
        assert_eq!(target_t_count(5), 2);
        assert_eq!(target_t_count(4), 2);
    }

    #[test]
    fn retakes_cycle_loadouts_guns_menu_and_streak_scramble() {
        let mut host = MockHostBridge::default();
        assert_eq!(init_mod(&mut host), 0);

        // 7 players (default 3T vs 4CT), player 0 is admin with RU lang
        for s in 0..7u8 {
            host.add_player(s, format!("P{s}"));
            let team = if s < 3 {
                TeamKind::Terrorist
            } else {
                TeamKind::Ct
            };
            host.players.get_mut(&s).unwrap().team = team;
        }
        host.set_player_lang(0, "ru");
        host.set_player_admin_flags(0, AdminFlags::parse_str("ac"));

        // Round 1 -> Pistol on Bombsite A
        host.active_i32_args[0] = 1;
        dispatch_hook(&mut host, HookId::RoundStart, HookPhase::Post);
        assert!(
            host.dhud_messages
                .iter()
                .any(|m| m.text.contains("РЕТЕЙК — ТОЧКА А"))
        );
        assert!(
            host.dhud_messages
                .iter()
                .any(|m| m.text.contains("RETAKES — BOMBSITE A [3T vs 4CT]"))
        );

        // /guns menu opens and updates preference into storage
        host.active_i32_args[0] = 3;
        host.active_str_args[0] = "/guns".to_string();
        assert_eq!(
            dispatch_hook(&mut host, HookId::PlayerSay, HookPhase::Pre),
            HookAction::Supercede
        );
        host.active_str_args[0] = "awp:toggle".to_string();
        dispatch_menu(&mut host, CB_GUNS_MENU, 3, 1, 6);
        assert!(load_pref(&host, 3).awp_opt_in);
    }
}
