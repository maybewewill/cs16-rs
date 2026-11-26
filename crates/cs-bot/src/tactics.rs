// SPDX-License-Identifier: GPL-3.0-or-later
//! Full ZBot tactical state machine, multi-sensory perception, team coordination, radio/chatter
//! scheduler, and `bot_*` cvar settings.
//!
//! Ported from ReGameDLL_CS (`bot/cs_bot.cpp`, `bot/cs_bot_listen.cpp`, `bot/cs_bot_radio.cpp`,
//! `bot/states/*.cpp`) as pure, `#![forbid(unsafe_code)]`, `wasm32`-clean Rust.

use glam::Vec3;

use crate::buy::WeaponRestrictions;
use crate::combat::{
    BotCombatState, CombatInput, CombatInventory, CombatOutput, CombatTarget, GrenadeThrowRequest,
    angle_diff, angles_to, step_combat,
};
use crate::profile::{BotDifficulty, BotProfile, BotTeam, BotWeapon, mix64};

/// Team join constraint (`bot_join_team`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BotJoinTeam {
    #[default]
    Any,
    CounterTerrorist,
    Terrorist,
}

/// Chatter verbosity setting (`bot_chatter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BotChatterMode {
    Off,
    Radio,
    Minimal,
    #[default]
    Normal,
}

/// Complete ZBot configuration holding all CS 1.6 `bot_*` console variables.
#[derive(Debug, Clone, PartialEq)]
pub struct BotSettings {
    pub bot_enable: bool,
    pub bot_quota: u32,
    pub bot_quota_match: f32,
    pub bot_difficulty: BotDifficulty,
    pub bot_prefix: String,
    pub bot_profile_db: String,
    pub bot_join_after_player: bool,
    pub bot_join_team: BotJoinTeam,
    pub bot_auto_vacate: bool,
    pub bot_chatter: BotChatterMode,
    pub bot_defer_to_human: bool,
    pub bot_walk: bool,
    pub bot_stop: bool,
    pub bot_zombie: bool,
    pub bot_debug: i32,
    pub bot_nav_edit: bool,
    pub bot_nav_zdraw: f32,
    pub bot_nav_autogen: bool,
    pub bot_quicksave: bool,
    pub bot_show_danger: bool,
    pub bot_show_nav: bool,
    pub bot_traceview: i32,
    pub weapon_restrictions: WeaponRestrictions,
}

impl Default for BotSettings {
    fn default() -> Self {
        Self {
            bot_enable: true,
            bot_quota: 0,
            bot_quota_match: 0.0,
            bot_difficulty: BotDifficulty::Normal,
            bot_prefix: String::new(),
            bot_profile_db: String::from("BotProfile.db"),
            bot_join_after_player: true,
            bot_join_team: BotJoinTeam::Any,
            bot_auto_vacate: true,
            bot_chatter: BotChatterMode::Normal,
            bot_defer_to_human: true,
            bot_walk: false,
            bot_stop: false,
            bot_zombie: false,
            bot_debug: 0,
            bot_nav_edit: false,
            bot_nav_zdraw: 4.0,
            bot_nav_autogen: true,
            bot_quicksave: false,
            bot_show_danger: false,
            bot_show_nav: false,
            bot_traceview: 0,
            weapon_restrictions: WeaponRestrictions::default(),
        }
    }
}

impl BotSettings {
    /// Applies a `bot_*` cvar or weapon preset command (`bot_knives_only`, etc.).
    /// Returns `true` if recognized.
    pub fn apply_cvar(&mut self, name: &str, value: &str) -> bool {
        let parse_bool = |s: &str| {
            matches!(s.trim(), "1" | "true" | "TRUE" | "yes" | "on")
                || s.trim().eq_ignore_ascii_case("true")
        };
        match name.to_ascii_lowercase().as_str() {
            "bot_enable" => self.bot_enable = parse_bool(value),
            "bot_quota" => {
                if let Ok(v) = value.trim().parse::<u32>() {
                    self.bot_quota = v;
                }
            }
            "bot_quota_match" => {
                if let Ok(v) = value.trim().parse::<f32>() {
                    self.bot_quota_match = v.max(0.0);
                }
            }
            "bot_difficulty" => {
                if let Some(d) = BotDifficulty::parse(value) {
                    self.bot_difficulty = d;
                }
            }
            "bot_prefix" => self.bot_prefix = value.to_string(),
            "bot_profile_db" => self.bot_profile_db = value.to_string(),
            "bot_join_after_player" => self.bot_join_after_player = parse_bool(value),
            "bot_join_team" => {
                let v = value.trim();
                self.bot_join_team = if v.eq_ignore_ascii_case("ct") {
                    BotJoinTeam::CounterTerrorist
                } else if v.eq_ignore_ascii_case("t") {
                    BotJoinTeam::Terrorist
                } else {
                    BotJoinTeam::Any
                };
            }
            "bot_auto_vacate" => self.bot_auto_vacate = parse_bool(value),
            "bot_chatter" => {
                let v = value.trim();
                self.bot_chatter = if v.eq_ignore_ascii_case("off") || v == "0" {
                    BotChatterMode::Off
                } else if v.eq_ignore_ascii_case("radio") {
                    BotChatterMode::Radio
                } else if v.eq_ignore_ascii_case("minimal") {
                    BotChatterMode::Minimal
                } else {
                    BotChatterMode::Normal
                };
            }
            "bot_defer_to_human" => self.bot_defer_to_human = parse_bool(value),
            "bot_walk" => self.bot_walk = parse_bool(value),
            "bot_stop" => self.bot_stop = parse_bool(value),
            "bot_zombie" => self.bot_zombie = parse_bool(value),
            "bot_debug" => {
                if let Ok(v) = value.trim().parse::<i32>() {
                    self.bot_debug = v;
                }
            }
            "bot_nav_edit" => self.bot_nav_edit = parse_bool(value),
            "bot_nav_zdraw" => {
                if let Ok(v) = value.trim().parse::<f32>() {
                    self.bot_nav_zdraw = v;
                }
            }
            "bot_nav_autogen" => self.bot_nav_autogen = parse_bool(value),
            "bot_quicksave" => self.bot_quicksave = parse_bool(value),
            "bot_show_danger" => self.bot_show_danger = parse_bool(value),
            "bot_show_nav" => self.bot_show_nav = parse_bool(value),
            "bot_traceview" => {
                if let Ok(v) = value.trim().parse::<i32>() {
                    self.bot_traceview = v;
                }
            }
            "bot_allow_pistols" => self.weapon_restrictions.allow_pistols = parse_bool(value),
            "bot_allow_shotguns" => self.weapon_restrictions.allow_shotguns = parse_bool(value),
            "bot_allow_sub_machine_guns" => {
                self.weapon_restrictions.allow_sub_machine_guns = parse_bool(value)
            }
            "bot_allow_rifles" => self.weapon_restrictions.allow_rifles = parse_bool(value),
            "bot_allow_snipers" => self.weapon_restrictions.allow_snipers = parse_bool(value),
            "bot_allow_machine_guns" => {
                self.weapon_restrictions.allow_machine_guns = parse_bool(value)
            }
            "bot_allow_grenades" => self.weapon_restrictions.allow_grenades = parse_bool(value),
            "bot_allow_shield" => self.weapon_restrictions.allow_shield = parse_bool(value),
            "bot_allow_rogues" => self.weapon_restrictions.allow_rogues = parse_bool(value),
            "bot_knives_only" => self.weapon_restrictions.knives_only(),
            "bot_pistols_only" => self.weapon_restrictions.pistols_only(),
            "bot_snipers_only" => self.weapon_restrictions.snipers_only(),
            "bot_all_weapons" => self.weapon_restrictions.all_weapons(),
            _ => return false,
        }
        true
    }
}

/// Full 18-state ZBot tactical FSM (`BotState` in ReGameDLL_CS + floor weapon scavenging).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BotTacticalState {
    #[default]
    Idle,
    Hunt,
    Attack,
    InvestigateNoise,
    MoveTo,
    FetchBomb,
    FetchGroundWeapon,
    PlantBomb,
    DefuseBomb,
    GuardBomb,
    Follow,
    EscapeFromBomb,
    RescueHostage,
    GuardHostages,
    EscortVip,
    AssassinateVip,
    Hide,
    AmbushCamp,
}

/// Active smoke cloud used for line-of-sight occlusion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmokeCloud {
    pub center: Vec3,
    pub radius: f32,
}

/// Acoustic event heard by bots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SoundKind {
    Footstep,
    Gunfire,
    Reload,
    BombBeep,
    BombPlanting,
    BombDefusing,
    GrenadeBounce,
    DoorOpen,
    Hostage,
}

impl SoundKind {
    /// Default CS 1.6 hearing radius (units).
    pub const fn default_radius(self) -> f32 {
        match self {
            Self::Footstep => 750.0,
            Self::Gunfire => 2400.0,
            Self::Reload => 680.0,
            Self::BombBeep => 1450.0,
            Self::BombPlanting | Self::BombDefusing => 950.0,
            Self::GrenadeBounce => 750.0,
            Self::DoorOpen => 800.0,
            Self::Hostage => 700.0,
        }
    }
}

/// Sound event emitted into the world for multi-sensory perception.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundEvent {
    pub source_id: Option<u32>,
    pub team: Option<BotTeam>,
    pub pos: Vec3,
    pub kind: SoundKind,
    pub radius: f32,
    pub time: f32,
}

/// Potential visible enemy supplied to perception (before FOV/smoke/flash filtering).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerceivedEnemyCandidate {
    pub id: u32,
    pub pos: Vec3,
    pub eye: Vec3,
    pub vel: Vec3,
    pub has_los: bool,
    pub has_primary: bool,
    pub is_vip: bool,
}

/// CS 1.6 radio message identifiers (`client.cpp::RADIO_MESSAGES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RadioMessage {
    CoverMe,
    YouTakeThePoint,
    HoldThisPosition,
    RegroupTeam,
    FollowMe,
    TakingFire,
    GoGoGo,
    TeamFallBack,
    StickTogether,
    GetInPosition,
    StormTheFront,
    ReportIn,
    Affirmative,
    EnemySpotted,
    NeedBackup,
    SectorClear,
    InPosition,
    ReportingIn,
    GetOutOfThere,
    Negative,
    EnemyDown,
}

impl RadioMessage {
    /// Canonical CS 1.6 radio console command alias (`crates/cs-game/src/hud_misc.rs`).
    pub const fn alias(self) -> &'static str {
        match self {
            Self::CoverMe => "coverme",
            Self::YouTakeThePoint => "takepoint",
            Self::HoldThisPosition => "holdpos",
            Self::RegroupTeam => "regroup",
            Self::FollowMe => "followme",
            Self::TakingFire => "takingfire",
            Self::GoGoGo => "go",
            Self::TeamFallBack => "fallback",
            Self::StickTogether => "sticktog",
            Self::GetInPosition => "getinpos",
            Self::StormTheFront => "stormfront",
            Self::ReportIn => "report",
            Self::Affirmative => "roger",
            Self::EnemySpotted => "enemyspot",
            Self::NeedBackup => "needbackup",
            Self::SectorClear => "sectorclear",
            Self::InPosition => "inposition",
            Self::ReportingIn => "reportingin",
            Self::GetOutOfThere => "getout",
            Self::Negative => "negative",
            Self::EnemyDown => "enemydown",
        }
    }

    /// Parses a radio alias or HUD label.
    pub fn from_alias(raw: &str) -> Option<Self> {
        let s = raw.trim().to_ascii_lowercase();
        Some(match s.as_str() {
            "coverme" => Self::CoverMe,
            "takepoint" => Self::YouTakeThePoint,
            "holdpos" => Self::HoldThisPosition,
            "regroup" => Self::RegroupTeam,
            "followme" => Self::FollowMe,
            "takingfire" => Self::TakingFire,
            "go" => Self::GoGoGo,
            "fallback" => Self::TeamFallBack,
            "sticktog" => Self::StickTogether,
            "getinpos" => Self::GetInPosition,
            "stormfront" => Self::StormTheFront,
            "report" => Self::ReportIn,
            "roger" | "affirmative" => Self::Affirmative,
            "enemyspot" => Self::EnemySpotted,
            "needbackup" => Self::NeedBackup,
            "sectorclear" => Self::SectorClear,
            "inposition" => Self::InPosition,
            "reportingin" => Self::ReportingIn,
            "getout" => Self::GetOutOfThere,
            "negative" => Self::Negative,
            "enemydown" => Self::EnemyDown,
            _ => return None,
        })
    }
}

/// High-level ZBot voice chatter event (`BotChatter.db`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BotChatterEvent {
    SpotEnemy,
    BombPlanted,
    BombDefusing,
    GuardingBomb,
    HostageTaken,
    VipSpotted,
    HeardNoise,
    PinnedDown,
    Blind,
    KilledEnemy,
    Clear,
    ReportingIn,
}

/// Radio broadcast sent by a teammate or human leader.
#[derive(Debug, Clone, PartialEq)]
pub struct IncomingRadio {
    pub sender_id: u32,
    pub sender_team: BotTeam,
    pub sender_pos: Vec3,
    pub sender_place: Option<String>,
    pub message: RadioMessage,
    pub time: f32,
    pub is_human: bool,
}

/// Outgoing radio/chatter command emitted by the bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingRadio {
    pub message: RadioMessage,
    pub place_name: Option<String>,
    pub chatter_event: Option<BotChatterEvent>,
    pub formatted_callout: String,
}

/// Hostage info visible to tactical planner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TacticalHostage {
    pub id: u32,
    pub pos: Vec3,
    pub follower_id: Option<u32>,
    pub rescued: bool,
}

/// Tactical cover/sniper/ambush spot hint (populated from `.nav` hiding spots).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TacticalSpot {
    pub pos: Vec3,
    pub look_at: Vec3,
    pub is_sniper_spot: bool,
    pub is_cover: bool,
}

/// Kind of floor pickup (`armoury_entity` or dropped `weaponbox`) available on the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroundPickupKind {
    Weapon(BotWeapon),
    Kevlar,
    AssaultSuit,
}

impl GroundPickupKind {
    /// Relative tactical priority for floor weapon/equipment scavenging.
    pub fn base_priority(self, profile: &BotProfile) -> u8 {
        match self {
            Self::Weapon(w) => {
                let base: u8 = match w {
                    BotWeapon::Ak47
                    | BotWeapon::M4a1
                    | BotWeapon::Aug
                    | BotWeapon::Sg552
                    | BotWeapon::Awp => 100,
                    BotWeapon::Famas
                    | BotWeapon::Galil
                    | BotWeapon::Scout
                    | BotWeapon::Sg550
                    | BotWeapon::G3sg1
                    | BotWeapon::M249 => 90,
                    BotWeapon::Mp5n
                    | BotWeapon::P90
                    | BotWeapon::Ump45
                    | BotWeapon::Mac10
                    | BotWeapon::Tmp
                    | BotWeapon::Xm1014
                    | BotWeapon::M3 => 75,
                    BotWeapon::Deagle
                    | BotWeapon::Elite
                    | BotWeapon::FiveSeven
                    | BotWeapon::P228
                    | BotWeapon::Usp
                    | BotWeapon::Glock18 => 45,
                    BotWeapon::HeGrenade => 25,
                    BotWeapon::Flashbang => 20,
                    BotWeapon::SmokeGrenade => 18,
                    BotWeapon::ShieldGun | BotWeapon::Knife | BotWeapon::C4 => 0,
                };
                if base >= 75 && profile.weapon_preferences.contains(&w) {
                    base.saturating_add(15)
                } else {
                    base
                }
            }
            Self::AssaultSuit => 35,
            Self::Kevlar => 30,
        }
    }
}

/// Reachable floor weapon or equipment pickup visible to tactical planning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundPickup {
    pub pos: Vec3,
    pub kind: GroundPickupKind,
}

/// World & self snapshot passed into [`BotTacticalBrain::step`].
#[derive(Debug, Clone)]
pub struct TacticalContext<'a> {
    pub self_id: u32,
    pub team: BotTeam,
    pub now: f32,
    pub dt: f32,
    pub pos: Vec3,
    pub eye: Vec3,
    pub vel: Vec3,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub punch_angle_deg: Vec3,
    pub fov_deg: u8,
    pub on_ground: bool,
    pub is_crouching: bool,
    pub health: f32,
    pub flash_alpha: f32,
    pub place_name: Option<&'a str>,
    pub inventory: CombatInventory,
    pub profile: &'a BotProfile,
    pub settings: &'a BotSettings,
    pub enemy_candidates: &'a [PerceivedEnemyCandidate],
    pub smokes: &'a [SmokeCloud],
    pub sounds: &'a [SoundEvent],
    pub incoming_radio: &'a [IncomingRadio],
    pub human_teammate_alive: bool,
    pub human_near_objective: bool,
    // Mode & objective inputs
    pub has_c4: bool,
    pub in_bomb_zone: bool,
    pub bomb_sites: &'a [Vec3],
    pub dropped_bomb_pos: Option<Vec3>,
    pub planted_bomb_pos: Option<Vec3>,
    pub bomb_time_remaining: Option<f32>,
    pub hostages: &'a [TacticalHostage],
    pub rescue_zones: &'a [Vec3],
    pub is_vip: bool,
    pub vip_pos: Option<Vec3>,
    pub vip_safety_zones: &'a [Vec3],
    // Navigation / cover hints
    pub cover_spots: &'a [TacticalSpot],
    pub patrol_waypoints: &'a [Vec3],
    // Arena / floor scavenging & mode traits
    pub ground_pickups: &'a [GroundPickup],
    pub has_buyzones: bool,
    pub can_buy: bool,
    pub knife_only_arena: bool,
    pub has_armor: bool,
    pub has_helmet: bool,
    pub rng_seed: u64,
}

/// Combined tactical + combat command output for one AI think step.
#[derive(Debug, Clone, PartialEq)]
pub struct TacticalDecision {
    pub state: BotTacticalState,
    pub move_goal: Option<Vec3>,
    pub walk: bool,
    pub duck: bool,
    pub use_key: bool,
    pub want_c4: bool,
    pub combat: CombatOutput,
    pub outgoing_radio: Option<OutgoingRadio>,
    pub heard_noise: Option<Vec3>,
    pub visible_enemy_count: usize,
}

/// Persistent ZBot tactical brain state.
#[derive(Debug, Clone)]
pub struct BotTacticalBrain {
    pub state: BotTacticalState,
    pub combat: BotCombatState,
    pub committed_site: Option<Vec3>,
    pub move_goal: Option<Vec3>,
    pub follow_leader_id: Option<u32>,
    pub follow_pos: Option<Vec3>,
    pub hold_spot: Option<TacticalSpot>,
    pub noise_pos: Option<Vec3>,
    pub noise_time: f32,
    pub last_enemy_pos: Option<Vec3>,
    pub last_enemy_time: f32,
    pub is_rogue: bool,
    pub next_radio_time: f32,
    pub next_spot_radio_time: f32,
    pub patrol_index: usize,
    pub pending_radio_reply: Option<OutgoingRadio>,
    pub state_timer: f32,
    pub bomb_planted_acked: bool,
}

impl Default for BotTacticalBrain {
    fn default() -> Self {
        Self {
            state: BotTacticalState::Idle,
            combat: BotCombatState::default(),
            committed_site: None,
            move_goal: None,
            follow_leader_id: None,
            follow_pos: None,
            hold_spot: None,
            noise_pos: None,
            noise_time: -100.0,
            last_enemy_pos: None,
            last_enemy_time: -100.0,
            is_rogue: false,
            next_radio_time: 0.0,
            next_spot_radio_time: 0.0,
            patrol_index: 0,
            pending_radio_reply: None,
            state_timer: 0.0,
            bomb_planted_acked: false,
        }
    }
}

impl BotTacticalBrain {
    /// Resets per-round state while keeping personality identity.
    pub fn on_round_reset(&mut self, profile: &BotProfile, settings: &BotSettings, seed: u64) {
        *self = Self::default();
        self.is_rogue = settings.weapon_restrictions.allow_rogues
            && profile.teamwork < 0.45
            && mix64(seed ^ 0x5061).is_multiple_of(4);
    }

    /// Queues an `EnemyDown` chatter/radio call when this bot eliminates an enemy.
    pub fn on_enemy_killed(&mut self, place_name: Option<&str>, now: f32, mode: BotChatterMode) {
        self.queue_radio(
            RadioMessage::EnemyDown,
            place_name,
            Some(BotChatterEvent::KilledEnemy),
            now,
            mode,
        );
    }

    /// Processes a human or teammate radio call immediately (`CCSBot::OnRadioMessage`).
    #[allow(clippy::too_many_arguments)]
    pub fn on_radio(
        &mut self,
        radio: &IncomingRadio,
        self_team: BotTeam,
        self_pos: Vec3,
        self_place: Option<&str>,
        has_c4: bool,
        profile: &BotProfile,
        settings: &BotSettings,
    ) {
        if radio.sender_team != self_team {
            return;
        }
        let allow_orders = !self.is_rogue && profile.teamwork >= 0.25;

        match radio.message {
            RadioMessage::FollowMe | RadioMessage::StickTogether | RadioMessage::RegroupTeam => {
                if !allow_orders || has_c4 || self.state == BotTacticalState::DefuseBomb {
                    self.queue_radio(
                        RadioMessage::Negative,
                        self_place,
                        None,
                        radio.time,
                        settings.bot_chatter,
                    );
                    return;
                }
                self.follow_leader_id = Some(radio.sender_id);
                self.follow_pos = Some(radio.sender_pos);
                self.hold_spot = None;
                self.state = BotTacticalState::Follow;
                self.move_goal = Some(radio.sender_pos);
                self.queue_radio(
                    RadioMessage::Affirmative,
                    self_place,
                    None,
                    radio.time,
                    settings.bot_chatter,
                );
            }
            RadioMessage::HoldThisPosition | RadioMessage::GetInPosition => {
                if !allow_orders || has_c4 {
                    self.queue_radio(
                        RadioMessage::Negative,
                        self_place,
                        None,
                        radio.time,
                        settings.bot_chatter,
                    );
                    return;
                }
                self.follow_leader_id = None;
                self.hold_spot = Some(TacticalSpot {
                    pos: self_pos,
                    look_at: radio.sender_pos,
                    is_sniper_spot: false,
                    is_cover: true,
                });
                self.state = BotTacticalState::AmbushCamp;
                self.move_goal = Some(self_pos);
                self.queue_radio(
                    RadioMessage::Affirmative,
                    self_place,
                    None,
                    radio.time,
                    settings.bot_chatter,
                );
            }
            RadioMessage::CoverMe => {
                if !allow_orders || has_c4 {
                    return;
                }
                self.follow_leader_id = Some(radio.sender_id);
                self.follow_pos = Some(radio.sender_pos);
                self.state = BotTacticalState::Follow;
                self.move_goal = Some(radio.sender_pos);
                self.queue_radio(
                    RadioMessage::Affirmative,
                    self_place,
                    None,
                    radio.time,
                    settings.bot_chatter,
                );
            }
            RadioMessage::TakingFire | RadioMessage::NeedBackup => {
                if self.state != BotTacticalState::Attack
                    && self.state != BotTacticalState::DefuseBomb
                    && self.state != BotTacticalState::PlantBomb
                    && self_pos.distance(radio.sender_pos) < 1800.0
                {
                    self.state = BotTacticalState::MoveTo;
                    self.move_goal = Some(radio.sender_pos);
                    self.queue_radio(
                        RadioMessage::Affirmative,
                        self_place,
                        None,
                        radio.time,
                        settings.bot_chatter,
                    );
                }
            }
            RadioMessage::EnemySpotted => {
                if self.last_enemy_pos.is_none() || radio.time - self.last_enemy_time > 2.5 {
                    self.last_enemy_pos = Some(radio.sender_pos);
                    self.last_enemy_time = radio.time;
                    if matches!(self.state, BotTacticalState::Idle | BotTacticalState::Hunt) {
                        self.move_goal = Some(radio.sender_pos);
                    }
                }
            }
            RadioMessage::ReportIn => {
                let (msg, chatter) =
                    if self.last_enemy_pos.is_some() && radio.time - self.last_enemy_time < 3.0 {
                        (RadioMessage::EnemySpotted, BotChatterEvent::SpotEnemy)
                    } else if matches!(
                        self.state,
                        BotTacticalState::Idle | BotTacticalState::AmbushCamp
                    ) {
                        (RadioMessage::SectorClear, BotChatterEvent::Clear)
                    } else {
                        (RadioMessage::ReportingIn, BotChatterEvent::ReportingIn)
                    };
                self.queue_radio(
                    msg,
                    self_place,
                    Some(chatter),
                    radio.time,
                    settings.bot_chatter,
                );
            }
            RadioMessage::GetOutOfThere => {
                self.state = BotTacticalState::EscapeFromBomb;
            }
            RadioMessage::GoGoGo | RadioMessage::StormTheFront => {
                self.hold_spot = None;
                if self.state == BotTacticalState::AmbushCamp
                    || self.state == BotTacticalState::Hide
                {
                    self.state = BotTacticalState::Hunt;
                }
                self.queue_radio(
                    RadioMessage::Affirmative,
                    self_place,
                    None,
                    radio.time,
                    settings.bot_chatter,
                );
            }
            _ => {}
        }
    }

    /// Runs one full ZBot perception -> tactical FSM -> combat step.
    pub fn step(&mut self, ctx: &TacticalContext<'_>) -> TacticalDecision {
        self.state_timer += ctx.dt;

        if ctx.settings.bot_stop || ctx.settings.bot_zombie {
            self.state = BotTacticalState::Idle;
            let combat = CombatOutput {
                yaw_deg: ctx.yaw_deg,
                pitch_deg: ctx.pitch_deg,
                desired_weapon: ctx.inventory.active_weapon,
                primary_attack: false,
                secondary_attack: false,
                reload: false,
                want_crouch: false,
                want_stop_to_shoot: true,
                strafe_bias: 0.0,
                needs_reload_in_cover: false,
                threw_grenade: None,
                engaged_target: None,
            };
            return TacticalDecision {
                state: BotTacticalState::Idle,
                move_goal: None,
                walk: false,
                duck: false,
                use_key: false,
                want_c4: false,
                combat,
                outgoing_radio: None,
                heard_noise: None,
                visible_enemy_count: 0,
            };
        }

        // 1. Handle incoming radio calls first.
        for radio in ctx.incoming_radio {
            if radio.sender_id != ctx.self_id {
                self.on_radio(
                    radio,
                    ctx.team,
                    ctx.pos,
                    ctx.place_name,
                    ctx.has_c4,
                    ctx.profile,
                    ctx.settings,
                );
            }
        }

        // 2. Multi-sensory perception: vision (FOV + LOS + smoke + flash) & sound events.
        let visible = perceive_visible_enemies(ctx);
        if let Some(first_vis) = visible.first() {
            let had_recent = ctx.now - self.last_enemy_time < 4.0;
            self.last_enemy_pos = Some(first_vis.pos);
            self.last_enemy_time = ctx.now;
            if !had_recent && ctx.now >= self.next_spot_radio_time {
                self.next_spot_radio_time = ctx.now + 7.0;
                self.queue_radio(
                    RadioMessage::EnemySpotted,
                    ctx.place_name,
                    Some(BotChatterEvent::SpotEnemy),
                    ctx.now,
                    ctx.settings.bot_chatter,
                );
            }
        }

        let heard = perceive_most_relevant_sound(ctx);
        if let Some((sound_pos, kind)) = heard {
            self.noise_pos = Some(sound_pos);
            self.noise_time = ctx.now;
            if matches!(kind, SoundKind::BombBeep | SoundKind::BombPlanting)
                && ctx.team == BotTeam::CounterTerrorist
            {
                self.last_enemy_pos = Some(sound_pos);
                self.last_enemy_time = ctx.now;
            }
        }

        // 3. Grenade request when enemy last seen behind corner or clustered.
        let grenade_req =
            pick_tactical_grenade(ctx, &visible, self.last_enemy_pos, self.last_enemy_time);

        // 4. Step combat controller first so `needs_reload_in_cover` informs tactical hiding.
        let look_override = self.compute_look_override(ctx, heard.map(|h| h.0));
        let mut combat_out = step_combat(
            &mut self.combat,
            &CombatInput {
                dt: ctx.dt,
                self_pos: ctx.pos,
                self_eye: ctx.eye,
                self_vel: ctx.vel,
                current_yaw_deg: ctx.yaw_deg,
                current_pitch_deg: ctx.pitch_deg,
                punch_angle_deg: ctx.punch_angle_deg,
                current_fov: ctx.fov_deg,
                on_ground: ctx.on_ground,
                is_crouching: ctx.is_crouching,
                health: ctx.health,
                flash_alpha: ctx.flash_alpha,
                inventory: ctx.inventory,
                visible_enemies: &visible,
                grenade_request: grenade_req,
                look_at_override: look_override,
                profile: ctx.profile,
                difficulty: ctx.settings.bot_difficulty,
                restrictions: &ctx.settings.weapon_restrictions,
                rng_seed: ctx.rng_seed,
            },
        );

        // 5. Select tactical state & objective goal.
        let mut duck = combat_out.want_crouch;
        let mut use_key = false;
        let mut want_c4 = false;

        self.update_tactical_state(
            ctx,
            &visible,
            heard,
            &combat_out,
            &mut duck,
            &mut use_key,
            &mut want_c4,
        );

        let walk = !ctx.knife_only_arena
            && (ctx.settings.bot_walk
                || ctx.profile.walk
                    && visible.is_empty()
                    && !matches!(
                        self.state,
                        BotTacticalState::EscapeFromBomb | BotTacticalState::FetchGroundWeapon
                    )
                || (self.state == BotTacticalState::InvestigateNoise && ctx.profile.skill >= 0.5))
            || ctx.settings.bot_walk;

        // 6. Authentic CS 1.6 knife-out spawn rollout (`250 u/s`): equip knife while traversing
        // long safe distances far from enemies, and switch back to primary rifle well before
        // entering enemy LoS or contact zones (`~1150u` pre-contact margin).
        if should_use_knife_rollout(
            ctx,
            self.state,
            self.move_goal,
            &visible,
            heard,
            walk,
            want_c4,
            use_key,
            &combat_out,
            self.last_enemy_time,
            self.noise_time,
        ) {
            combat_out.desired_weapon = Some(BotWeapon::Knife);
        }

        let outgoing_radio = self.pending_radio_reply.take();

        TacticalDecision {
            state: self.state,
            move_goal: self.move_goal,
            walk,
            duck,
            use_key,
            want_c4,
            combat: combat_out,
            outgoing_radio,
            heard_noise: heard.map(|h| h.0),
            visible_enemy_count: visible.len(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn update_tactical_state(
        &mut self,
        ctx: &TacticalContext<'_>,
        visible: &[CombatTarget],
        heard: Option<(Vec3, SoundKind)>,
        combat: &CombatOutput,
        duck: &mut bool,
        use_key: &mut bool,
        want_c4: &mut bool,
    ) {
        // Priority 1: Escape imminent C4 blast.
        if let Some(bomb_pos) = ctx.planted_bomb_pos
            && ctx.bomb_time_remaining.is_some_and(|t| t <= 5.5)
            && ctx.pos.distance(bomb_pos) < 1600.0
        {
            self.state = BotTacticalState::EscapeFromBomb;
            self.move_goal = Some(escape_point_away_from(ctx.pos, bomb_pos, ctx.cover_spots));
            return;
        }

        // Priority 2: Reload in cover or retreat when heavily flashed/critical health under threat.
        let low_hp_cover = !ctx.knife_only_arena
            && ctx.health < 25.0
            && !visible.is_empty()
            && ctx.profile.aggression < 0.7;
        if (combat.needs_reload_in_cover || low_hp_cover || ctx.flash_alpha >= 0.75)
            && !ctx.knife_only_arena
            && let Some(spot) = nearest_cover_spot(ctx.pos, ctx.cover_spots)
        {
            self.state = BotTacticalState::Hide;
            self.move_goal = Some(spot.pos);
            *duck = spot.is_cover;
            return;
        }

        // Priority 2.5: Scavenge a floor weapon on buyzone-less / arena maps or when broke/post-buy.
        let needs_ground_weapon = !ctx.knife_only_arena
            && ctx.inventory.primary.is_none()
            && (!ctx.has_buyzones || !ctx.can_buy);
        if needs_ground_weapon && let Some(pickup) = select_best_ground_pickup(ctx) {
            let best_is_primary =
                matches!(pickup.kind, GroundPickupKind::Weapon(w) if w.is_primary());
            let closest_vis_dist = visible
                .first()
                .map_or(f32::INFINITY, |e| ctx.pos.distance(e.pos));
            if visible.is_empty() || (best_is_primary && closest_vis_dist > 150.0) {
                self.state = BotTacticalState::FetchGroundWeapon;
                self.move_goal = Some(pickup.pos);
                return;
            }
        }

        // Priority 3: Active Attack when enemy is visible (unless defusing/planting under smoke cover or VIP fleeing).
        if !visible.is_empty() {
            if ctx.is_vip {
                // VIP prioritizes escaping toward safety zone while defending self.
                if let Some(safe) = nearest_point(ctx.pos, ctx.vip_safety_zones) {
                    self.state = BotTacticalState::EscortVip;
                    self.move_goal = Some(safe);
                    return;
                }
            }
            self.state = BotTacticalState::Attack;
            self.move_goal = visible.first().map(|e| e.pos);
            if ctx.health < 45.0 && ctx.now >= self.next_radio_time {
                self.queue_radio(
                    RadioMessage::TakingFire,
                    ctx.place_name,
                    Some(BotChatterEvent::PinnedDown),
                    ctx.now,
                    ctx.settings.bot_chatter,
                );
            }
            return;
        }

        // Priority 4: Planted C4 duties.
        if let Some(bomb_pos) = ctx.planted_bomb_pos {
            match ctx.team {
                BotTeam::CounterTerrorist => {
                    if !self.bomb_planted_acked {
                        self.bomb_planted_acked = true;
                        self.queue_radio(
                            RadioMessage::GoGoGo,
                            ctx.place_name,
                            Some(BotChatterEvent::BombPlanted),
                            ctx.now,
                            ctx.settings.bot_chatter,
                        );
                    }
                    self.state = BotTacticalState::DefuseBomb;
                    self.move_goal = Some(bomb_pos);
                    if ctx.pos.distance(bomb_pos) < 58.0 {
                        *use_key = true;
                        *duck = true;
                        self.queue_radio(
                            RadioMessage::CoverMe,
                            ctx.place_name,
                            Some(BotChatterEvent::BombDefusing),
                            ctx.now,
                            ctx.settings.bot_chatter,
                        );
                    }
                    return;
                }
                BotTeam::Terrorist => {
                    if self.state != BotTacticalState::GuardBomb {
                        self.queue_radio(
                            RadioMessage::InPosition,
                            ctx.place_name,
                            Some(BotChatterEvent::GuardingBomb),
                            ctx.now,
                            ctx.settings.bot_chatter,
                        );
                    }
                    self.state = BotTacticalState::GuardBomb;
                    let guard_pt = best_guard_spot(bomb_pos, ctx.cover_spots).unwrap_or(bomb_pos);
                    self.move_goal = Some(guard_pt);
                    return;
                }
            }
        }

        // Priority 5: Terrorist carrying C4 or fetching dropped C4.
        if ctx.team == BotTeam::Terrorist {
            if ctx.has_c4 {
                if ctx.in_bomb_zone {
                    self.state = BotTacticalState::PlantBomb;
                    self.move_goal = Some(ctx.pos);
                    *want_c4 = true;
                    *duck = true;
                    return;
                }
                if self.committed_site.is_none() {
                    self.committed_site = choose_bomb_site(ctx.pos, ctx.bomb_sites, ctx.rng_seed);
                }
                if let Some(site) = self.committed_site {
                    self.state = BotTacticalState::MoveTo;
                    self.move_goal = Some(site);
                    return;
                }
            } else if let Some(drop_pos) = ctx.dropped_bomb_pos
                && !(ctx.settings.bot_defer_to_human && ctx.human_near_objective)
            {
                self.state = BotTacticalState::FetchBomb;
                self.move_goal = Some(drop_pos);
                return;
            }
        }

        // Priority 6: VIP mode (as_* maps).
        if !ctx.vip_safety_zones.is_empty() {
            match ctx.team {
                BotTeam::CounterTerrorist => {
                    if ctx.is_vip {
                        self.state = BotTacticalState::EscortVip;
                        self.move_goal = nearest_point(ctx.pos, ctx.vip_safety_zones);
                        return;
                    } else if let Some(vip_pos) = ctx.vip_pos {
                        self.state = BotTacticalState::EscortVip;
                        self.move_goal = Some(vip_pos);
                        return;
                    }
                }
                BotTeam::Terrorist => {
                    self.state = BotTacticalState::AssassinateVip;
                    self.move_goal = ctx
                        .vip_pos
                        .or_else(|| nearest_point(ctx.pos, ctx.vip_safety_zones));
                    return;
                }
            }
        }

        // Priority 7: Hostage mode (cs_* maps).
        if !ctx.hostages.is_empty() {
            let unrescued: Vec<TacticalHostage> = ctx
                .hostages
                .iter()
                .copied()
                .filter(|h| !h.rescued)
                .collect();
            if !unrescued.is_empty() {
                match ctx.team {
                    BotTeam::CounterTerrorist => {
                        let following_me: Vec<TacticalHostage> = unrescued
                            .iter()
                            .copied()
                            .filter(|h| h.follower_id == Some(ctx.self_id))
                            .collect();
                        self.state = BotTacticalState::RescueHostage;
                        if !following_me.is_empty() {
                            self.move_goal = nearest_point(ctx.pos, ctx.rescue_zones);
                            return;
                        }
                        if !(ctx.settings.bot_defer_to_human && ctx.human_near_objective)
                            && let Some(h) = unrescued
                                .iter()
                                .filter(|h| h.follower_id.is_none())
                                .min_by(|a, b| {
                                    a.pos
                                        .distance_squared(ctx.pos)
                                        .total_cmp(&b.pos.distance_squared(ctx.pos))
                                })
                        {
                            self.move_goal = Some(h.pos);
                            if ctx.pos.distance(h.pos) < 60.0 {
                                *use_key = true;
                            }
                            return;
                        }
                    }
                    BotTeam::Terrorist => {
                        // Guard hostages unless chasing a recent noise.
                        if heard.is_none() && (ctx.now - self.noise_time > 5.0) {
                            self.state = BotTacticalState::GuardHostages;
                            let target_h = unrescued
                                .iter()
                                .min_by(|a, b| {
                                    a.pos
                                        .distance_squared(ctx.pos)
                                        .total_cmp(&b.pos.distance_squared(ctx.pos))
                                })
                                .unwrap();
                            self.move_goal = best_guard_spot(target_h.pos, ctx.cover_spots)
                                .or(Some(target_h.pos));
                            return;
                        }
                    }
                }
            }
        }

        // Priority 8: Following human/radio leader or holding ordered position.
        if self.state == BotTacticalState::Follow
            && let Some(goal) = self.follow_pos
        {
            self.move_goal = Some(goal);
            return;
        }
        if self.state == BotTacticalState::AmbushCamp
            && let Some(spot) = self.hold_spot
        {
            self.move_goal = Some(spot.pos);
            *duck = spot.is_cover;
            return;
        }

        // Priority 9: Investigate heard enemy noise.
        if let Some((sound_pos, _kind)) = heard {
            self.state = BotTacticalState::InvestigateNoise;
            self.move_goal = Some(sound_pos);
            if ctx.now >= self.next_radio_time {
                self.queue_radio(
                    RadioMessage::NeedBackup,
                    ctx.place_name,
                    Some(BotChatterEvent::HeardNoise),
                    ctx.now,
                    ctx.settings.bot_chatter,
                );
            }
            return;
        }
        if self.state == BotTacticalState::InvestigateNoise
            && ctx.now - self.noise_time < 4.5
            && let Some(np) = self.noise_pos
        {
            if ctx.pos.distance(np) > 96.0 {
                self.move_goal = Some(np);
                return;
            }
            self.queue_radio(
                RadioMessage::SectorClear,
                ctx.place_name,
                Some(BotChatterEvent::Clear),
                ctx.now,
                ctx.settings.bot_chatter,
            );
        }

        // Priority 10: Defensive ambush/sniper camp or active Hunt across bomb sites / waypoints.
        let prefers_camp = !ctx.knife_only_arena
            && ((!ctx.bomb_sites.is_empty()
                && ctx.team == BotTeam::CounterTerrorist
                && (ctx.inventory.primary.is_some_and(|w| w.is_sniper())
                    || ctx.profile.aggression < 0.45))
                || (!ctx.has_buyzones
                    && ctx.inventory.primary.is_some_and(|w| w.is_sniper())
                    && ctx.profile.aggression < 0.4));
        if prefers_camp
            && let Some(spot) = ctx
                .cover_spots
                .iter()
                .find(|s| s.is_sniper_spot || s.is_cover)
        {
            self.state = BotTacticalState::AmbushCamp;
            self.hold_spot = Some(*spot);
            self.move_goal = Some(spot.pos);
            *duck = ctx.profile.crouch;
            return;
        }

        if let Some(last_pos) = self.last_enemy_pos
            && ctx.now - self.last_enemy_time < 8.0
            && ctx.pos.distance(last_pos) > 120.0
        {
            self.state = BotTacticalState::Hunt;
            self.move_goal = Some(last_pos);
            return;
        }

        if !ctx.patrol_waypoints.is_empty() {
            self.state = BotTacticalState::Hunt;
            let current = ctx.patrol_waypoints[self.patrol_index % ctx.patrol_waypoints.len()];
            if ctx.pos.distance(current) < 96.0 {
                self.patrol_index = (self.patrol_index + 1) % ctx.patrol_waypoints.len();
            }
            self.move_goal =
                Some(ctx.patrol_waypoints[self.patrol_index % ctx.patrol_waypoints.len()]);
            return;
        }

        if let Some(site) = choose_bomb_site(ctx.pos, ctx.bomb_sites, ctx.rng_seed) {
            self.state = BotTacticalState::Hunt;
            self.move_goal = Some(site);
            return;
        }

        self.state = BotTacticalState::Idle;
        self.move_goal = None;
    }

    fn compute_look_override(
        &self,
        ctx: &TacticalContext<'_>,
        heard_pos: Option<Vec3>,
    ) -> Option<Vec3> {
        if let Some(pos) = heard_pos {
            return Some(pos + Vec3::new(0.0, 0.0, 48.0));
        }
        if self.state == BotTacticalState::AmbushCamp
            && let Some(spot) = self.hold_spot
            && ctx.pos.distance(spot.pos) < 64.0
        {
            return Some(spot.look_at);
        }
        self.move_goal.map(|g| g + Vec3::new(0.0, 0.0, 48.0))
    }

    fn queue_radio(
        &mut self,
        message: RadioMessage,
        place_name: Option<&str>,
        chatter_event: Option<BotChatterEvent>,
        now: f32,
        mode: BotChatterMode,
    ) {
        if mode == BotChatterMode::Off || now < self.next_radio_time {
            return;
        }
        if mode == BotChatterMode::Minimal
            && !matches!(
                message,
                RadioMessage::EnemySpotted
                    | RadioMessage::TakingFire
                    | RadioMessage::Affirmative
                    | RadioMessage::Negative
                    | RadioMessage::GetOutOfThere
            )
        {
            return;
        }
        self.next_radio_time = now + 2.5;
        let place = place_name.map(ToString::to_string);
        let formatted_callout = match &place {
            Some(p) => format!("[{p}] {}", message.alias()),
            None => message.alias().to_string(),
        };
        self.pending_radio_reply = Some(OutgoingRadio {
            message,
            place_name: place,
            chatter_event: if mode == BotChatterMode::Radio {
                None
            } else {
                chatter_event
            },
            formatted_callout,
        });
    }
}

/// True when it is safe and beneficial to run with the Knife (`250 u/s`) during spawn rollout or
/// long rotations, switching back to the primary rifle well before entering enemy LoS or contact.
#[allow(clippy::too_many_arguments)]
fn should_use_knife_rollout(
    ctx: &TacticalContext<'_>,
    state: BotTacticalState,
    move_goal: Option<Vec3>,
    visible: &[CombatTarget],
    heard: Option<(Vec3, SoundKind)>,
    walk: bool,
    want_c4: bool,
    use_key: bool,
    combat_out: &CombatOutput,
    last_enemy_time: f32,
    noise_time: f32,
) -> bool {
    if !ctx.inventory.has_knife
        || ctx.knife_only_arena
        || !visible.is_empty()
        || heard.is_some()
        || walk
        || want_c4
        || use_key
        || combat_out.reload
        || combat_out.primary_attack
        || combat_out.secondary_attack
        || ctx.now - last_enemy_time <= 3.5
        || ctx.now - noise_time <= 2.5
    {
        return false;
    }

    let rolling_state = matches!(
        state,
        BotTacticalState::MoveTo
            | BotTacticalState::Hunt
            | BotTacticalState::FetchBomb
            | BotTacticalState::DefuseBomb
            | BotTacticalState::EscapeFromBomb
            | BotTacticalState::RescueHostage
            | BotTacticalState::EscortVip
    );
    if !rolling_state {
        return false;
    }

    let Some(goal) = move_goal else {
        return false;
    };
    let dist_to_goal = ctx.pos.distance(goal);
    if dist_to_goal <= 220.0 {
        return false;
    }

    // Never run with knife if any nearby enemy already has line of sight (<1900u) or is within
    // pre-contact range (<1150u, even behind walls).
    if ctx
        .enemy_candidates
        .iter()
        .any(|e| e.has_los && ctx.pos.distance(e.pos) < 1900.0)
    {
        return false;
    }
    let nearest_enemy_dist = ctx
        .enemy_candidates
        .iter()
        .map(|e| ctx.pos.distance(e.pos))
        .fold(f32::INFINITY, f32::min);
    if ctx.enemy_candidates.is_empty() {
        if dist_to_goal <= 450.0 {
            return false;
        }
    } else if nearest_enemy_dist <= 1150.0 {
        return false;
    }

    // Switch back to rifle before stepping into an active bombsite (unless fleeing C4 blast).
    if state != BotTacticalState::EscapeFromBomb {
        if ctx.bomb_sites.iter().any(|s| ctx.pos.distance(*s) <= 320.0) {
            return false;
        }
        if ctx
            .planted_bomb_pos
            .is_some_and(|b| ctx.pos.distance(b) <= 550.0)
        {
            return false;
        }
    }

    true
}

/// Filters enemy candidates through flashblindness, FOV cone, LOS, and smoke occlusion.
pub fn perceive_visible_enemies(ctx: &TacticalContext<'_>) -> Vec<CombatTarget> {
    if ctx.flash_alpha >= 0.55 {
        return Vec::new();
    }

    let (fwd_x, fwd_y) = {
        let r = ctx.yaw_deg.to_radians();
        (r.cos(), r.sin())
    };
    let cos_half_fov = if ctx.fov_deg < 60 {
        32.0_f32.to_radians().cos()
    } else {
        56.0_f32.to_radians().cos()
    };

    let mut out = Vec::new();
    for &c in ctx.enemy_candidates {
        if !c.has_los {
            continue;
        }
        let aim_pt = c.pos.lerp(c.eye, 0.72);
        let delta = aim_pt - ctx.eye;
        let dist = delta.length();
        if dist > 2800.0 || dist < 0.001 {
            continue;
        }

        // Within peripheral awareness (<140u) bots sense enemies even outside narrow FOV.
        if dist > 140.0 {
            let horiz = Vec3::new(delta.x, delta.y, 0.0).normalize_or_zero();
            let dot = horiz.x * fwd_x + horiz.y * fwd_y;
            if dot < cos_half_fov {
                continue;
            }
        }

        if segment_blocked_by_smoke(ctx.eye, aim_pt, ctx.smokes) {
            continue;
        }

        let threat_score = if c.is_vip {
            1.4
        } else if c.has_primary {
            1.0
        } else {
            0.5
        };

        out.push(CombatTarget {
            id: c.id,
            pos: c.pos,
            eye: c.eye,
            vel: c.vel,
            threat_score,
            is_moving: c.vel.length_squared() > 25.0,
            has_primary: c.has_primary,
        });
    }
    out
}

/// Selects the strongest audible sound event relevant to the bot.
pub fn perceive_most_relevant_sound(ctx: &TacticalContext<'_>) -> Option<(Vec3, SoundKind)> {
    let mut best: Option<(f32, Vec3, SoundKind)> = None;
    for s in ctx.sounds {
        if s.source_id == Some(ctx.self_id) {
            continue;
        }
        if ctx.now - s.time > 2.0 {
            continue;
        }
        // Ignore friendly footsteps/reloads unless it's a bomb beep/defuse/plant or gunfire.
        let is_friendly = s.team == Some(ctx.team);
        if is_friendly
            && matches!(
                s.kind,
                SoundKind::Footstep | SoundKind::Reload | SoundKind::DoorOpen
            )
        {
            continue;
        }
        let radius = if s.radius > 0.0 {
            s.radius
        } else {
            s.kind.default_radius()
        };
        let dist = ctx.pos.distance(s.pos);
        if dist > radius {
            continue;
        }
        let priority = match s.kind {
            SoundKind::BombBeep | SoundKind::BombDefusing | SoundKind::BombPlanting => 3.0,
            SoundKind::Gunfire => 2.2,
            SoundKind::Reload | SoundKind::Footstep => 1.5,
            _ => 1.0,
        };
        let score = priority * (1.0 - dist / radius);
        if best.is_none_or(|(b_score, _, _)| score > b_score) {
            best = Some((score, s.pos, s.kind));
        }
    }
    best.map(|(_, pos, kind)| (pos, kind))
}

/// Segment-sphere intersection test against active CS 1.6 smoke clouds.
pub fn segment_blocked_by_smoke(a: Vec3, b: Vec3, smokes: &[SmokeCloud]) -> bool {
    let ab = b - a;
    let ab_len_sq = ab.length_squared();
    for s in smokes {
        let r_sq = s.radius * s.radius;
        if ab_len_sq < 1e-4 {
            if a.distance_squared(s.center) <= r_sq {
                return true;
            }
            continue;
        }
        let t = ((s.center - a).dot(ab) / ab_len_sq).clamp(0.0, 1.0);
        let closest = a + ab * t;
        if closest.distance_squared(s.center) <= r_sq {
            return true;
        }
    }
    false
}

fn pick_tactical_grenade(
    ctx: &TacticalContext<'_>,
    visible: &[CombatTarget],
    last_enemy_pos: Option<Vec3>,
    last_enemy_time: f32,
) -> Option<GrenadeThrowRequest> {
    if !ctx.settings.weapon_restrictions.allow_grenades {
        return None;
    }
    if !visible.is_empty() {
        // Throw HE when multiple enemies are visible at mid range.
        if visible.len() >= 2 && ctx.inventory.he_count > 0 {
            let d = ctx.pos.distance(visible[0].pos);
            if (300.0..=950.0).contains(&d) {
                return Some(GrenadeThrowRequest {
                    grenade: BotWeapon::HeGrenade,
                    target: visible[0].pos,
                });
            }
        }
        return None;
    }
    // Flash or HE toward recently spotted enemy behind cover.
    if let Some(epos) = last_enemy_pos
        && (ctx.now - last_enemy_time) < 2.2
    {
        let d = ctx.pos.distance(epos);
        if (260.0..=900.0).contains(&d) {
            if ctx.inventory.he_count > 0 && ctx.profile.aggression >= 0.45 {
                return Some(GrenadeThrowRequest {
                    grenade: BotWeapon::HeGrenade,
                    target: epos,
                });
            }
            if ctx.inventory.flash_count > 0 && ctx.profile.teamwork >= 0.5 {
                return Some(GrenadeThrowRequest {
                    grenade: BotWeapon::Flashbang,
                    target: epos,
                });
            }
        }
    }
    None
}

fn is_pickup_eligible(ctx: &TacticalContext<'_>, kind: GroundPickupKind) -> bool {
    match kind {
        GroundPickupKind::Weapon(w) => {
            if !ctx.settings.weapon_restrictions.allows_weapon(w) {
                return false;
            }
            if w.is_primary() {
                ctx.inventory.primary.is_none()
            } else if w.is_pistol() {
                ctx.inventory.secondary.is_none()
            } else {
                match w {
                    BotWeapon::HeGrenade => ctx.inventory.he_count < 1,
                    BotWeapon::Flashbang => ctx.inventory.flash_count < 2,
                    BotWeapon::SmokeGrenade => ctx.inventory.smoke_count < 1,
                    _ => false,
                }
            }
        }
        GroundPickupKind::Kevlar => !ctx.has_armor,
        GroundPickupKind::AssaultSuit => !ctx.has_armor || !ctx.has_helmet,
    }
}

/// Selects the highest-priority eligible floor pickup adjusted for travel distance.
pub fn select_best_ground_pickup(ctx: &TacticalContext<'_>) -> Option<GroundPickup> {
    ctx.ground_pickups
        .iter()
        .copied()
        .filter(|p| is_pickup_eligible(ctx, p.kind))
        .max_by(|a, b| {
            let score_a =
                f32::from(a.kind.base_priority(ctx.profile)) * 20.0 - ctx.pos.distance(a.pos);
            let score_b =
                f32::from(b.kind.base_priority(ctx.profile)) * 20.0 - ctx.pos.distance(b.pos);
            score_a.total_cmp(&score_b)
        })
}

fn choose_bomb_site(pos: Vec3, sites: &[Vec3], seed: u64) -> Option<Vec3> {
    if sites.is_empty() {
        return None;
    }
    if sites.len() == 1 {
        return Some(sites[0]);
    }
    let pick = (mix64(seed) as usize) % sites.len();
    Some(sites[pick]).or_else(|| nearest_point(pos, sites))
}

fn nearest_point(pos: Vec3, pts: &[Vec3]) -> Option<Vec3> {
    pts.iter()
        .copied()
        .min_by(|a, b| a.distance_squared(pos).total_cmp(&b.distance_squared(pos)))
}

fn nearest_cover_spot(pos: Vec3, spots: &[TacticalSpot]) -> Option<TacticalSpot> {
    spots.iter().copied().filter(|s| s.is_cover).min_by(|a, b| {
        a.pos
            .distance_squared(pos)
            .total_cmp(&b.pos.distance_squared(pos))
    })
}

fn best_guard_spot(objective: Vec3, spots: &[TacticalSpot]) -> Option<Vec3> {
    spots
        .iter()
        .filter(|s| {
            let d = s.pos.distance(objective);
            (180.0..=750.0).contains(&d)
        })
        .min_by(|a, b| {
            a.pos
                .distance_squared(objective)
                .total_cmp(&b.pos.distance_squared(objective))
        })
        .map(|s| s.pos)
}

fn escape_point_away_from(pos: Vec3, bomb_pos: Vec3, spots: &[TacticalSpot]) -> Vec3 {
    if let Some(far_spot) = spots
        .iter()
        .filter(|s| s.pos.distance(bomb_pos) > 1200.0)
        .min_by(|a, b| {
            a.pos
                .distance_squared(pos)
                .total_cmp(&b.pos.distance_squared(pos))
        })
    {
        return far_spot.pos;
    }
    let dir = (pos - bomb_pos).truncate().normalize_or_zero();
    let dir = if dir.length_squared() < 0.01 {
        glam::Vec2::new(1.0, 0.0)
    } else {
        dir
    };
    pos + Vec3::new(dir.x * 1600.0, dir.y * 1600.0, 0.0)
}

/// Helper for aiming towards a target point (useful for external tests/integration).
pub fn yaw_pitch_to(from: Vec3, to: Vec3) -> (f32, f32) {
    angles_to(from, to)
}

/// Shortest signed angular delta in degrees (`[-180, 180]`).
pub fn shortest_angle_delta(from: f32, to: f32) -> f32 {
    angle_diff(from, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_ctx<'a>(
        profile: &'a BotProfile,
        settings: &'a BotSettings,
        team: BotTeam,
    ) -> TacticalContext<'a> {
        TacticalContext {
            self_id: 1,
            team,
            now: 10.0,
            dt: 0.1,
            pos: Vec3::ZERO,
            eye: Vec3::new(0.0, 0.0, 64.0),
            vel: Vec3::ZERO,
            yaw_deg: 0.0,
            pitch_deg: 0.0,
            punch_angle_deg: Vec3::ZERO,
            fov_deg: 90,
            on_ground: true,
            is_crouching: false,
            health: 100.0,
            flash_alpha: 0.0,
            place_name: Some("Bombsite A"),
            inventory: CombatInventory {
                primary: Some(BotWeapon::M4a1),
                primary_clip: 30,
                primary_reserve: 90,
                active_weapon: Some(BotWeapon::M4a1),
                ..Default::default()
            },
            profile,
            settings,
            enemy_candidates: &[],
            smokes: &[],
            sounds: &[],
            incoming_radio: &[],
            human_teammate_alive: false,
            human_near_objective: false,
            has_c4: false,
            in_bomb_zone: false,
            bomb_sites: &[],
            dropped_bomb_pos: None,
            planted_bomb_pos: None,
            bomb_time_remaining: None,
            hostages: &[],
            rescue_zones: &[],
            is_vip: false,
            vip_pos: None,
            vip_safety_zones: &[],
            cover_spots: &[],
            patrol_waypoints: &[],
            ground_pickups: &[],
            has_buyzones: true,
            can_buy: true,
            knife_only_arena: false,
            has_armor: false,
            has_helmet: false,
            rng_seed: 42,
        }
    }

    #[test]
    fn bomb_fetch_plant_guard_escape_and_ct_defuse() {
        let prof = BotProfile::fallback("Minh", BotDifficulty::Expert);
        let settings = BotSettings::default();
        let sites = [Vec3::new(600.0, 0.0, 0.0)];
        let spots = [TacticalSpot {
            pos: Vec3::new(350.0, 120.0, 0.0),
            look_at: sites[0],
            is_sniper_spot: false,
            is_cover: true,
        }];

        // T fetches dropped C4
        let mut t_brain = BotTacticalBrain::default();
        let mut t_ctx = base_ctx(&prof, &settings, BotTeam::Terrorist);
        t_ctx.bomb_sites = &sites;
        t_ctx.cover_spots = &spots;
        t_ctx.dropped_bomb_pos = Some(Vec3::new(150.0, 0.0, 0.0));
        let d_fetch = t_brain.step(&t_ctx);
        assert_eq!(d_fetch.state, BotTacticalState::FetchBomb);
        assert_eq!(d_fetch.move_goal, Some(Vec3::new(150.0, 0.0, 0.0)));

        // T with C4 in bomb zone plants it
        t_ctx.dropped_bomb_pos = None;
        t_ctx.has_c4 = true;
        t_ctx.in_bomb_zone = true;
        t_ctx.pos = sites[0];
        let d_plant = t_brain.step(&t_ctx);
        assert_eq!(d_plant.state, BotTacticalState::PlantBomb);
        assert!(d_plant.want_c4);
        assert!(d_plant.duck);

        // Once planted, T guards bomb from cover spot
        t_ctx.has_c4 = false;
        t_ctx.in_bomb_zone = false;
        t_ctx.planted_bomb_pos = Some(sites[0]);
        t_ctx.bomb_time_remaining = Some(30.0);
        let d_guard = t_brain.step(&t_ctx);
        assert_eq!(d_guard.state, BotTacticalState::GuardBomb);
        assert_eq!(d_guard.move_goal, Some(spots[0].pos));

        // When fuse <= 5.5s, T escapes from bomb
        t_ctx.bomb_time_remaining = Some(4.0);
        let d_escape = t_brain.step(&t_ctx);
        assert_eq!(d_escape.state, BotTacticalState::EscapeFromBomb);

        // CT approaches and defuses planted bomb
        let mut ct_brain = BotTacticalBrain::default();
        let mut ct_ctx = base_ctx(&prof, &settings, BotTeam::CounterTerrorist);
        ct_ctx.planted_bomb_pos = Some(sites[0]);
        ct_ctx.bomb_time_remaining = Some(20.0);
        ct_ctx.pos = sites[0] + Vec3::new(25.0, 0.0, 0.0);
        let d_defuse = ct_brain.step(&ct_ctx);
        assert_eq!(d_defuse.state, BotTacticalState::DefuseBomb);
        assert!(d_defuse.use_key);
        assert!(d_defuse.duck);
    }

    #[test]
    fn hostage_guard_and_rescue_states() {
        let prof = BotProfile::fallback("Cliffe", BotDifficulty::Hard);
        let settings = BotSettings::default();
        let hostages = [TacticalHostage {
            id: 10,
            pos: Vec3::new(400.0, 0.0, 0.0),
            follower_id: None,
            rescued: false,
        }];
        let rescue_zones = [Vec3::new(-400.0, 0.0, 0.0)];

        let mut t_brain = BotTacticalBrain::default();
        let mut t_ctx = base_ctx(&prof, &settings, BotTeam::Terrorist);
        t_ctx.hostages = &hostages;
        let d_t = t_brain.step(&t_ctx);
        assert_eq!(d_t.state, BotTacticalState::GuardHostages);

        let mut ct_brain = BotTacticalBrain::default();
        let mut ct_ctx = base_ctx(&prof, &settings, BotTeam::CounterTerrorist);
        ct_ctx.hostages = &hostages;
        ct_ctx.rescue_zones = &rescue_zones;
        ct_ctx.pos = Vec3::new(370.0, 0.0, 0.0);
        let d_ct_use = ct_brain.step(&ct_ctx);
        assert_eq!(d_ct_use.state, BotTacticalState::RescueHostage);
        assert!(d_ct_use.use_key);

        // Once hostage follows CT, goal shifts to rescue zone
        let following = [TacticalHostage {
            id: 10,
            pos: Vec3::new(380.0, 0.0, 0.0),
            follower_id: Some(1),
            rescued: false,
        }];
        ct_ctx.hostages = &following;
        let d_ct_escort = ct_brain.step(&ct_ctx);
        assert_eq!(d_ct_escort.state, BotTacticalState::RescueHostage);
        assert_eq!(d_ct_escort.move_goal, Some(rescue_zones[0]));
    }

    #[test]
    fn vip_escort_and_assassination_states() {
        let prof = BotProfile::fallback("Stone", BotDifficulty::Hard);
        let settings = BotSettings::default();
        let zones = [Vec3::new(900.0, 0.0, 0.0)];

        let mut vip_brain = BotTacticalBrain::default();
        let mut vip_ctx = base_ctx(&prof, &settings, BotTeam::CounterTerrorist);
        vip_ctx.is_vip = true;
        vip_ctx.vip_safety_zones = &zones;
        let d_vip = vip_brain.step(&vip_ctx);
        assert_eq!(d_vip.state, BotTacticalState::EscortVip);
        assert_eq!(d_vip.move_goal, Some(zones[0]));

        let mut t_brain = BotTacticalBrain::default();
        let mut t_ctx = base_ctx(&prof, &settings, BotTeam::Terrorist);
        t_ctx.vip_safety_zones = &zones;
        t_ctx.vip_pos = Some(Vec3::new(300.0, 50.0, 0.0));
        let d_assassinate = t_brain.step(&t_ctx);
        assert_eq!(d_assassinate.state, BotTacticalState::AssassinateVip);
        assert_eq!(d_assassinate.move_goal, Some(Vec3::new(300.0, 50.0, 0.0)));
    }

    #[test]
    fn perception_fov_smoke_flash_and_sound_events() {
        let prof = BotProfile::fallback("Ghost", BotDifficulty::Expert);
        let settings = BotSettings::default();
        let mut brain = BotTacticalBrain::default();

        // Enemy behind bot (> 140u and outside FOV cone) is NOT perceived visually.
        let behind = [PerceivedEnemyCandidate {
            id: 2,
            pos: Vec3::new(-400.0, 0.0, 0.0),
            eye: Vec3::new(-400.0, 0.0, 64.0),
            vel: Vec3::ZERO,
            has_los: true,
            has_primary: true,
            is_vip: false,
        }];
        let mut ctx = base_ctx(&prof, &settings, BotTeam::CounterTerrorist);
        ctx.enemy_candidates = &behind;
        assert_eq!(perceive_visible_enemies(&ctx).len(), 0);

        // Enemy in front, occluded by smoke, is NOT perceived.
        let front = [PerceivedEnemyCandidate {
            id: 2,
            pos: Vec3::new(500.0, 0.0, 0.0),
            eye: Vec3::new(500.0, 0.0, 64.0),
            vel: Vec3::ZERO,
            has_los: true,
            has_primary: true,
            is_vip: false,
        }];
        let smoke = [SmokeCloud {
            center: Vec3::new(250.0, 0.0, 40.0),
            radius: 120.0,
        }];
        ctx.enemy_candidates = &front;
        ctx.smokes = &smoke;
        assert_eq!(perceive_visible_enemies(&ctx).len(), 0);

        // Enemy footstep sound causes InvestigateNoise.
        let sounds = [SoundEvent {
            source_id: Some(2),
            team: Some(BotTeam::Terrorist),
            pos: Vec3::new(320.0, 80.0, 0.0),
            kind: SoundKind::Footstep,
            radius: 750.0,
            time: 9.8,
        }];
        ctx.sounds = &sounds;
        let d_noise = brain.step(&ctx);
        assert_eq!(d_noise.state, BotTacticalState::InvestigateNoise);
        assert_eq!(d_noise.move_goal, Some(Vec3::new(320.0, 80.0, 0.0)));

        // Removing smoke makes front enemy visible -> Attack; high flashalpha blinds bot.
        ctx.smokes = &[];
        ctx.sounds = &[];
        let d_atk = brain.step(&ctx);
        assert_eq!(d_atk.state, BotTacticalState::Attack);
        assert_eq!(d_atk.visible_enemy_count, 1);

        ctx.flash_alpha = 0.9;
        assert_eq!(perceive_visible_enemies(&ctx).len(), 0);
    }

    #[test]
    fn cover_hide_ambush_and_radio_coordination() {
        let mut prof = BotProfile::fallback("Wyatt", BotDifficulty::Hard);
        prof.crouch = true;
        let mut settings = BotSettings::default();
        assert!(settings.apply_cvar("bot_chatter", "normal"));
        assert!(settings.apply_cvar("bot_allow_snipers", "1"));

        let spots = [TacticalSpot {
            pos: Vec3::new(200.0, 0.0, 0.0),
            look_at: Vec3::new(700.0, 0.0, 0.0),
            is_sniper_spot: true,
            is_cover: true,
        }];
        let sites = [Vec3::new(700.0, 0.0, 0.0)];

        let mut brain = BotTacticalBrain::default();
        let mut ctx = base_ctx(&prof, &settings, BotTeam::CounterTerrorist);
        ctx.inventory.primary = Some(BotWeapon::Awp);
        ctx.inventory.active_weapon = Some(BotWeapon::Awp);
        ctx.cover_spots = &spots;
        ctx.bomb_sites = &sites;
        ctx.place_name = Some("Long A");

        // Sniper CT enters AmbushCamp.
        let d_camp = brain.step(&ctx);
        assert_eq!(d_camp.state, BotTacticalState::AmbushCamp);

        // Human radio FollowMe -> bot switches to Follow and replies Affirmative.
        let follow_radio = [IncomingRadio {
            sender_id: 9,
            sender_team: BotTeam::CounterTerrorist,
            sender_pos: Vec3::new(50.0, 50.0, 0.0),
            sender_place: Some(String::from("CT Spawn")),
            message: RadioMessage::FollowMe,
            time: 10.1,
            is_human: true,
        }];
        ctx.now = 10.1;
        ctx.incoming_radio = &follow_radio;
        let d_follow = brain.step(&ctx);
        assert_eq!(d_follow.state, BotTacticalState::Follow);
        assert_eq!(
            d_follow.outgoing_radio.as_ref().map(|r| r.message),
            Some(RadioMessage::Affirmative)
        );

        // Human radio ReportIn -> location-aware ReportingIn with place name.
        let report_radio = [IncomingRadio {
            sender_id: 9,
            sender_team: BotTeam::CounterTerrorist,
            sender_pos: Vec3::new(50.0, 50.0, 0.0),
            sender_place: None,
            message: RadioMessage::ReportIn,
            time: 13.0,
            is_human: true,
        }];
        ctx.now = 13.0;
        ctx.incoming_radio = &report_radio;
        let d_report = brain.step(&ctx);
        let out_rad = d_report.outgoing_radio.expect("expected ReportingIn reply");
        assert_eq!(out_rad.message, RadioMessage::ReportingIn);
        assert_eq!(out_rad.place_name.as_deref(), Some("Long A"));
        assert!(out_rad.formatted_callout.contains("Long A"));

        // Empty clip triggers Hide at nearest cover spot.
        ctx.now = 16.0;
        ctx.incoming_radio = &[];
        ctx.inventory.primary_clip = 0;
        ctx.inventory.primary_reserve = 30;
        let d_hide = brain.step(&ctx);
        assert_eq!(d_hide.state, BotTacticalState::Hide);
        assert_eq!(d_hide.move_goal, Some(spots[0].pos));
    }

    #[test]
    fn floor_weapon_scavenging_and_knife_arena_tactics() {
        let mut prof = BotProfile::fallback("PoolRunner", BotDifficulty::Hard);
        prof.walk = true;
        prof.aggression = 0.2;
        let settings = BotSettings::default();
        let pickups = [
            GroundPickup {
                pos: Vec3::new(80.0, 0.0, 0.0),
                kind: GroundPickupKind::Weapon(BotWeapon::HeGrenade),
            },
            GroundPickup {
                pos: Vec3::new(220.0, 40.0, 0.0),
                kind: GroundPickupKind::Weapon(BotWeapon::Ak47),
            },
            GroundPickup {
                pos: Vec3::new(140.0, -40.0, 0.0),
                kind: GroundPickupKind::Weapon(BotWeapon::Mp5n),
            },
        ];

        // fy_pool_day: lacks primary & no buyzones -> FetchGroundWeapon prioritizes AK47 over HE/MP5.
        let mut brain = BotTacticalBrain::default();
        let mut ctx = base_ctx(&prof, &settings, BotTeam::Terrorist);
        ctx.inventory.primary = None;
        ctx.inventory.primary_clip = 0;
        ctx.inventory.primary_reserve = 0;
        ctx.inventory.active_weapon = Some(BotWeapon::Glock18);
        ctx.has_buyzones = false;
        ctx.can_buy = false;
        ctx.ground_pickups = &pickups;
        let d_fetch = brain.step(&ctx);
        assert_eq!(d_fetch.state, BotTacticalState::FetchGroundWeapon);
        assert_eq!(d_fetch.move_goal, Some(Vec3::new(220.0, 40.0, 0.0)));
        assert!(!d_fetch.walk);

        // 35hp knife-only arena: even at 15 HP with cover nearby, bot charges aggressively (no Hide, no walk).
        let spots = [TacticalSpot {
            pos: Vec3::new(-100.0, 0.0, 0.0),
            look_at: Vec3::new(100.0, 0.0, 0.0),
            is_sniper_spot: false,
            is_cover: true,
        }];
        let enemy = [PerceivedEnemyCandidate {
            id: 7,
            pos: Vec3::new(250.0, 0.0, 0.0),
            eye: Vec3::new(250.0, 0.0, 64.0),
            vel: Vec3::ZERO,
            has_los: true,
            has_primary: false,
            is_vip: false,
        }];
        let mut knife_ctx = base_ctx(&prof, &settings, BotTeam::CounterTerrorist);
        knife_ctx.inventory = CombatInventory {
            primary: None,
            primary_clip: 0,
            primary_reserve: 0,
            secondary: None,
            secondary_clip: 0,
            secondary_reserve: 0,
            active_weapon: Some(BotWeapon::Knife),
            has_knife: true,
            ..Default::default()
        };
        knife_ctx.health = 15.0;
        knife_ctx.knife_only_arena = true;
        knife_ctx.has_buyzones = false;
        knife_ctx.can_buy = false;
        knife_ctx.cover_spots = &spots;
        knife_ctx.enemy_candidates = &enemy;
        let d_knife = brain.step(&knife_ctx);
        assert_eq!(d_knife.state, BotTacticalState::Attack);
        assert_eq!(d_knife.move_goal, Some(Vec3::new(250.0, 0.0, 0.0)));
        assert!(!d_knife.walk);
    }
}
