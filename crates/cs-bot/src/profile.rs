// SPDX-License-Identifier: GPL-3.0-or-later
//! Valve `BotProfile.db` parser, template inheritance resolver, and built-in CS 1.6 ZBot roster.
//!
//! Ported from ReGameDLL_CS (`game_shared/bot/bot_profile.cpp`, `bot_profile.h`) as pure,
//! `#![forbid(unsafe_code)]`, `wasm32`-clean Rust.

use std::collections::{HashMap, HashSet};
use std::fmt;

/// Bot team identifier used across AI modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BotTeam {
    Terrorist,
    CounterTerrorist,
}

/// ZBot difficulty tier (`BotDifficultyType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(u8)]
pub enum BotDifficulty {
    Easy = 0,
    #[default]
    Normal = 1,
    Hard = 2,
    Expert = 3,
}

impl BotDifficulty {
    /// Parses difficulty from integer `0..=3` or case-insensitive name.
    pub fn parse(raw: &str) -> Option<Self> {
        let s = raw.trim();
        match s {
            "0" => return Some(Self::Easy),
            "1" => return Some(Self::Normal),
            "2" => return Some(Self::Hard),
            "3" => return Some(Self::Expert),
            _ => {}
        }
        if s.eq_ignore_ascii_case("easy") {
            Some(Self::Easy)
        } else if s.eq_ignore_ascii_case("normal") {
            Some(Self::Normal)
        } else if s.eq_ignore_ascii_case("hard") {
            Some(Self::Hard)
        } else if s.eq_ignore_ascii_case("expert") {
            Some(Self::Expert)
        } else {
            None
        }
    }

    /// Single-bit mask corresponding to this difficulty.
    pub const fn bit(self) -> u8 {
        1u8 << (self as u8)
    }

    /// Canonical uppercase label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Easy => "EASY",
            Self::Normal => "NORMAL",
            Self::Hard => "HARD",
            Self::Expert => "EXPERT",
        }
    }
}

/// Bitmask of allowed [`BotDifficulty`] levels for a `BotProfile`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BotDifficultyMask(pub u8);

impl BotDifficultyMask {
    pub const EASY: Self = Self(1 << 0);
    pub const NORMAL: Self = Self(1 << 1);
    pub const HARD: Self = Self(1 << 2);
    pub const EXPERT: Self = Self(1 << 3);
    pub const ANY: Self = Self(0x0F);

    /// Returns true if `difficulty` is enabled in this mask.
    pub const fn contains(self, difficulty: BotDifficulty) -> bool {
        (self.0 & difficulty.bit()) != 0
    }

    /// Parses `+` or whitespace-delimited difficulty tokens such as `EASY+NORMAL` or `HARD`.
    pub fn parse(raw: &str) -> Option<Self> {
        let mut mask = 0u8;
        let mut found = false;
        for part in raw.split(['+', '|', ',', ' ', '\t']) {
            let p = part.trim();
            if p.is_empty() {
                continue;
            }
            if p.eq_ignore_ascii_case("any") || p.eq_ignore_ascii_case("all") {
                mask |= Self::ANY.0;
                found = true;
            } else {
                let d = BotDifficulty::parse(p)?;
                mask |= d.bit();
                found = true;
            }
        }
        found.then_some(Self(mask))
    }
}

impl Default for BotDifficultyMask {
    fn default() -> Self {
        Self::ANY
    }
}

/// Broad weapon category for restriction checks and combat heuristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeaponCategory {
    Knife,
    Pistol,
    Shotgun,
    Smg,
    Rifle,
    Sniper,
    MachineGun,
    Grenade,
    Shield,
    Equipment,
}

/// CS 1.6 weapons that bots can prefer, buy, carry, or select in combat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BotWeapon {
    // Pistols
    Glock18,
    Usp,
    P228,
    Deagle,
    FiveSeven,
    Elite,
    // Shotguns
    M3,
    Xm1014,
    // SMGs
    Mac10,
    Tmp,
    Mp5n,
    Ump45,
    P90,
    // Rifles
    Galil,
    Famas,
    Ak47,
    M4a1,
    Aug,
    Sg552,
    // Snipers
    Scout,
    Awp,
    Sg550,
    G3sg1,
    // Machine gun
    M249,
    // Shield
    ShieldGun,
    // Grenades
    HeGrenade,
    Flashbang,
    SmokeGrenade,
    // Melee / Bomb
    Knife,
    C4,
}

impl BotWeapon {
    /// Parses a weapon preference token from `BotProfile.db` or CS buy aliases.
    pub fn from_alias(raw: &str) -> Option<Self> {
        let s = raw.trim().to_ascii_lowercase();
        let s = s.strip_prefix("weapon_").unwrap_or(&s);
        Some(match s {
            "glock" | "glock18" | "9x19mm" => Self::Glock18,
            "usp" | "km45" => Self::Usp,
            "p228" | "228compact" => Self::P228,
            "deagle" | "nighthaw" | "nighthawk" => Self::Deagle,
            "fn57" | "fiveseven" => Self::FiveSeven,
            "elites" | "elite" => Self::Elite,
            "m3" | "12gauge" => Self::M3,
            "xm1014" | "autoshotgun" => Self::Xm1014,
            "mac10" => Self::Mac10,
            "tmp" | "mp" => Self::Tmp,
            "mp5" | "mp5navy" | "smg" => Self::Mp5n,
            "ump45" => Self::Ump45,
            "p90" | "c90" => Self::P90,
            "galil" | "defender" => Self::Galil,
            "famas" | "clarion" => Self::Famas,
            "ak47" | "cv47" => Self::Ak47,
            "m4a1" => Self::M4a1,
            "aug" | "bullpup" => Self::Aug,
            "sg552" | "krieg552" => Self::Sg552,
            "scout" => Self::Scout,
            "awp" | "magnum" => Self::Awp,
            "sg550" | "krieg550" => Self::Sg550,
            "g3sg1" | "d3au1" => Self::G3sg1,
            "m249" => Self::M249,
            "shield" => Self::ShieldGun,
            "hegren" | "hegrenade" | "grenade" => Self::HeGrenade,
            "flash" | "flashbang" => Self::Flashbang,
            "sgren" | "smokegrenade" => Self::SmokeGrenade,
            "knife" => Self::Knife,
            "c4" => Self::C4,
            _ => return None,
        })
    }

    /// Canonical CS 1.6 buy alias (matches `crates/cs-game/src/buy.rs::WEAPON_ALIASES`).
    pub const fn buy_alias(self) -> &'static str {
        match self {
            Self::Glock18 => "glock",
            Self::Usp => "usp",
            Self::P228 => "p228",
            Self::Deagle => "deagle",
            Self::FiveSeven => "fn57",
            Self::Elite => "elites",
            Self::M3 => "m3",
            Self::Xm1014 => "xm1014",
            Self::Mac10 => "mac10",
            Self::Tmp => "tmp",
            Self::Mp5n => "mp5",
            Self::Ump45 => "ump45",
            Self::P90 => "p90",
            Self::Galil => "galil",
            Self::Famas => "famas",
            Self::Ak47 => "ak47",
            Self::M4a1 => "m4a1",
            Self::Aug => "aug",
            Self::Sg552 => "sg552",
            Self::Scout => "scout",
            Self::Awp => "awp",
            Self::Sg550 => "sg550",
            Self::G3sg1 => "g3sg1",
            Self::M249 => "m249",
            Self::ShieldGun => "shield",
            Self::HeGrenade => "hegren",
            Self::Flashbang => "flash",
            Self::SmokeGrenade => "sgren",
            Self::Knife => "knife",
            Self::C4 => "c4",
        }
    }

    /// Weapon category used for restriction checks and combat heuristics.
    pub const fn category(self) -> WeaponCategory {
        match self {
            Self::Glock18 | Self::Usp | Self::P228 | Self::Deagle | Self::FiveSeven | Self::Elite => {
                WeaponCategory::Pistol
            }
            Self::M3 | Self::Xm1014 => WeaponCategory::Shotgun,
            Self::Mac10 | Self::Tmp | Self::Mp5n | Self::Ump45 | Self::P90 => WeaponCategory::Smg,
            Self::Galil | Self::Famas | Self::Ak47 | Self::M4a1 | Self::Aug | Self::Sg552 => {
                WeaponCategory::Rifle
            }
            Self::Scout | Self::Awp | Self::Sg550 | Self::G3sg1 => WeaponCategory::Sniper,
            Self::M249 => WeaponCategory::MachineGun,
            Self::ShieldGun => WeaponCategory::Shield,
            Self::HeGrenade | Self::Flashbang | Self::SmokeGrenade => WeaponCategory::Grenade,
            Self::Knife => WeaponCategory::Knife,
            Self::C4 => WeaponCategory::Equipment,
        }
    }

    /// True if this weapon occupies the primary slot.
    pub const fn is_primary(self) -> bool {
        matches!(
            self.category(),
            WeaponCategory::Shotgun
                | WeaponCategory::Smg
                | WeaponCategory::Rifle
                | WeaponCategory::Sniper
                | WeaponCategory::MachineGun
        )
    }

    /// True if this weapon is a pistol.
    pub const fn is_pistol(self) -> bool {
        matches!(self.category(), WeaponCategory::Pistol)
    }

    /// True if this weapon is a sniper rifle with scope zoom.
    pub const fn is_sniper(self) -> bool {
        matches!(self.category(), WeaponCategory::Sniper)
    }

    /// True if this weapon has a secondary scope zoom (AUG/SG552 or sniper).
    pub const fn has_zoom(self) -> bool {
        self.is_sniper() || matches!(self, Self::Aug | Self::Sg552)
    }
}

/// Spring-damper angular look parameters (`LookAngle*` in `BotProfile.db`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LookSpringParams {
    pub max_accel_normal: f32,
    pub stiffness_normal: f32,
    pub damping_normal: f32,
    pub max_accel_attacking: f32,
    pub stiffness_attacking: f32,
    pub damping_attacking: f32,
}

impl Default for LookSpringParams {
    fn default() -> Self {
        Self {
            max_accel_normal: 2000.0,
            stiffness_normal: 100.0,
            damping_normal: 25.0,
            max_accel_attacking: 3000.0,
            stiffness_attacking: 150.0,
            damping_attacking: 30.0,
        }
    }
}

/// Parsed personality and skill profile for a ZBot (`BotProfile` in ReGameDLL_CS).
#[derive(Debug, Clone, PartialEq)]
pub struct BotProfile {
    pub name: String,
    /// Normalized skill in `0.0..=1.0` (`Skill = 0..100` in `BotProfile.db`).
    pub skill: f32,
    /// Normalized aggression in `0.0..=1.0` (`Aggression = 0..100`).
    pub aggression: f32,
    /// Delay before firing when an enemy is acquired (seconds).
    pub reaction_time: f32,
    /// Extra delay before opening fire after reaction completes (seconds).
    pub attack_delay: f32,
    /// Normalized teamwork tendency in `0.0..=1.0` (`Teamwork = 0..100`).
    pub teamwork: f32,
    /// Initial angular aim focus scatter (degrees).
    pub aim_focus_initial: f32,
    /// Exponential aim focus error decay factor per second (`0.0..=1.0`).
    pub aim_focus_decay: f32,
    /// Offset multiplier applied to aim focus jitter.
    pub aim_focus_offset_scale: f32,
    /// Interval between aim focus re-samples (seconds).
    pub aim_focus_interval: f32,
    /// Ordered weapon preferences (`WeaponPreference` entries).
    pub weapon_preferences: Vec<BotWeapon>,
    /// Profile relative cost (`1..=5`, `Cost`).
    pub cost: u8,
    /// Difficulty bitmask (`Difficulty`).
    pub difficulty: BotDifficultyMask,
    /// Voice pitch percent (`VoicePitch`, default `100`).
    pub voice_pitch: u16,
    /// Player model skin index (`Skin`, `0` = random).
    pub skin: u8,
    /// Whether the bot prefers walking silently (`Walk`).
    pub walk: bool,
    /// Whether the bot prefers crouching in firefights (`Crouch`).
    pub crouch: bool,
    /// Optional custom voice bank path/name (`VoiceBank`).
    pub voice_bank: Option<String>,
    /// Optional preferred team (`Team = T | CT | ANY`).
    pub team: Option<BotTeam>,
    /// Spring-damper parameters for look angle tracking.
    pub look_spring: LookSpringParams,
}

impl Default for BotProfile {
    fn default() -> Self {
        Self {
            name: String::from("Default"),
            skill: 0.5,
            aggression: 0.5,
            reaction_time: 0.3,
            attack_delay: 0.0,
            teamwork: 0.75,
            aim_focus_initial: 20.0,
            aim_focus_decay: 0.7,
            aim_focus_offset_scale: 0.3,
            aim_focus_interval: 0.8,
            weapon_preferences: Vec::new(),
            cost: 0,
            difficulty: BotDifficultyMask::ANY,
            voice_pitch: 100,
            skin: 0,
            walk: false,
            crouch: false,
            voice_bank: None,
            team: None,
            look_spring: LookSpringParams::default(),
        }
    }
}

impl BotProfile {
    /// Returns true if this profile can spawn at `difficulty`.
    pub fn matches_difficulty(&self, difficulty: BotDifficulty) -> bool {
        self.difficulty.contains(difficulty)
    }

    /// Synthesizes a fallback profile for `difficulty` when no DB entry matches.
    pub fn fallback(name: impl Into<String>, difficulty: BotDifficulty) -> Self {
        match difficulty {
            BotDifficulty::Easy => Self {
                name: name.into(),
                skill: 0.25,
                aggression: 0.30,
                reaction_time: 0.45,
                attack_delay: 0.35,
                teamwork: 0.40,
                aim_focus_initial: 26.0,
                aim_focus_decay: 0.82,
                aim_focus_offset_scale: 0.45,
                aim_focus_interval: 1.0,
                difficulty: BotDifficultyMask::EASY,
                cost: 1,
                ..Self::default()
            },
            BotDifficulty::Normal => Self {
                name: name.into(),
                skill: 0.50,
                aggression: 0.50,
                reaction_time: 0.30,
                attack_delay: 0.15,
                teamwork: 0.65,
                aim_focus_initial: 18.0,
                aim_focus_decay: 0.70,
                aim_focus_offset_scale: 0.30,
                aim_focus_interval: 0.8,
                difficulty: BotDifficultyMask::NORMAL,
                cost: 2,
                ..Self::default()
            },
            BotDifficulty::Hard => Self {
                name: name.into(),
                skill: 0.75,
                aggression: 0.65,
                reaction_time: 0.20,
                attack_delay: 0.05,
                teamwork: 0.80,
                aim_focus_initial: 12.0,
                aim_focus_decay: 0.58,
                aim_focus_offset_scale: 0.20,
                aim_focus_interval: 0.6,
                difficulty: BotDifficultyMask::HARD,
                cost: 3,
                ..Self::default()
            },
            BotDifficulty::Expert => Self {
                name: name.into(),
                skill: 0.95,
                aggression: 0.75,
                reaction_time: 0.12,
                attack_delay: 0.0,
                teamwork: 0.90,
                aim_focus_initial: 6.0,
                aim_focus_decay: 0.45,
                aim_focus_offset_scale: 0.12,
                aim_focus_interval: 0.45,
                difficulty: BotDifficultyMask::EXPERT,
                cost: 4,
                ..Self::default()
            },
        }
    }
}

/// Errors produced when parsing a `BotProfile.db` document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileParseError {
    UnexpectedEnd { line: usize },
    UnterminatedBlock { name: String, line: usize },
    UnknownTemplate { name: String, line: usize },
    CyclicInheritance { template: String },
    InvalidValue { key: String, value: String, line: usize },
    InvalidHeader { line: usize, text: String },
}

impl fmt::Display for ProfileParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEnd { line } => {
                write!(f, "unexpected `End` outside profile block at line {line}")
            }
            Self::UnterminatedBlock { name, line } => {
                write!(f, "unterminated profile/template block `{name}` starting at line {line}")
            }
            Self::UnknownTemplate { name, line } => {
                write!(f, "unknown BotProfile template `{name}` at line {line}")
            }
            Self::CyclicInheritance { template } => {
                write!(f, "cyclic template inheritance involving `{template}`")
            }
            Self::InvalidValue { key, value, line } => {
                write!(f, "invalid value `{value}` for `{key}` at line {line}")
            }
            Self::InvalidHeader { line, text } => {
                write!(f, "invalid profile header `{text}` at line {line}")
            }
        }
    }
}

impl std::error::Error for ProfileParseError {}

#[derive(Debug, Clone)]
enum BlockPatch {
    Skill(f32),
    Aggression(f32),
    ReactionTime(f32),
    AttackDelay(f32),
    Teamwork(f32),
    AimFocusInitial(f32),
    AimFocusDecay(f32),
    AimFocusOffsetScale(f32),
    AimFocusInterval(f32),
    ClearWeaponPreferences,
    AddWeaponPreference(BotWeapon),
    Cost(u8),
    Difficulty(BotDifficultyMask),
    VoicePitch(u16),
    Skin(u8),
    Walk(bool),
    Crouch(bool),
    VoiceBank(Option<String>),
    Team(Option<BotTeam>),
    LookMaxAccelNormal(f32),
    LookStiffnessNormal(f32),
    LookDampingNormal(f32),
    LookMaxAccelAttacking(f32),
    LookStiffnessAttacking(f32),
    LookDampingAttacking(f32),
}

#[derive(Debug, Clone)]
struct TemplateDef {
    name: String,
    parents: Vec<String>,
    patches: Vec<BlockPatch>,
    line: usize,
}

/// Parsed `BotProfile.db` containing defaults, named templates, and bot profiles.
#[derive(Debug, Clone, Default)]
pub struct BotProfileDb {
    pub default_profile: BotProfile,
    pub templates: HashMap<String, BotProfile>,
    pub profiles: Vec<BotProfile>,
}

impl BotProfileDb {
    /// Parses the built-in CS 1.6 default bot roster covering Easy/Normal/Hard/Expert.
    pub fn builtin() -> Self {
        Self::parse(BUILTIN_BOT_PROFILE_DB)
            .expect("built-in BotProfile.db must always parse cleanly")
    }

    /// Parses a Valve `BotProfile.db` text file.
    pub fn parse(input: &str) -> Result<Self, ProfileParseError> {
        let mut default_profile = BotProfile::default();
        let mut raw_templates: HashMap<String, TemplateDef> = HashMap::new();
        let mut template_order: Vec<String> = Vec::new();
        let mut profiles: Vec<BotProfile> = Vec::new();

        enum ActiveBlock {
            Default {
                patches: Vec<BlockPatch>,
            },
            Template {
                name: String,
                parents: Vec<String>,
                patches: Vec<BlockPatch>,
                line: usize,
            },
            Profile {
                name: String,
                parents: Vec<String>,
                patches: Vec<BlockPatch>,
                line: usize,
            },
        }

        let mut current: Option<ActiveBlock> = None;

        for (idx, raw_line) in input.lines().enumerate() {
            let line_no = idx + 1;
            let stripped = strip_line_comment(raw_line).trim();
            if stripped.is_empty() {
                continue;
            }

            if stripped.eq_ignore_ascii_case("End") {
                match current.take() {
                    Some(ActiveBlock::Default { patches }) => {
                        apply_patches(&mut default_profile, &patches);
                    }
                    Some(ActiveBlock::Template {
                        name,
                        parents,
                        patches,
                        line,
                    }) => {
                        let key = name.to_ascii_lowercase();
                        raw_templates.insert(
                            key.clone(),
                            TemplateDef {
                                name,
                                parents,
                                patches,
                                line,
                            },
                        );
                        template_order.push(key);
                    }
                    Some(ActiveBlock::Profile {
                        name,
                        parents,
                        patches,
                        line,
                    }) => {
                        let mut resolved = default_profile.clone();
                        let mut visiting = HashSet::new();
                        for p in &parents {
                            apply_template_patches(
                                p,
                                &mut resolved,
                                &raw_templates,
                                &mut visiting,
                                line,
                            )?;
                        }
                        resolved.name = name;
                        apply_patches(&mut resolved, &patches);
                        profiles.push(resolved);
                    }
                    None => return Err(ProfileParseError::UnexpectedEnd { line: line_no }),
                }
                continue;
            }

            if let Some(block) = current.as_mut() {
                let patch = parse_attribute_line(stripped, line_no)?;
                match block {
                    ActiveBlock::Default { patches }
                    | ActiveBlock::Template { patches, .. }
                    | ActiveBlock::Profile { patches, .. } => {
                        if let Some(p) = patch {
                            patches.push(p);
                        }
                    }
                }
                continue;
            }

            // Start of a new block header.
            let tokens = tokenize_header(stripped);
            if tokens.is_empty() {
                continue;
            }
            if tokens.len() == 1 && tokens[0].eq_ignore_ascii_case("Default") {
                current = Some(ActiveBlock::Default {
                    patches: Vec::new(),
                });
            } else if tokens[0].eq_ignore_ascii_case("Template") {
                let (name, parents) = parse_template_header(&tokens[1..], line_no, stripped)?;
                current = Some(ActiveBlock::Template {
                    name,
                    parents,
                    patches: Vec::new(),
                    line: line_no,
                });
            } else {
                let (name, parents) = parse_profile_header(&tokens, line_no, stripped)?;
                current = Some(ActiveBlock::Profile {
                    name,
                    parents,
                    patches: Vec::new(),
                    line: line_no,
                });
            }
        }

        if let Some(open) = current {
            let (name, line) = match open {
                ActiveBlock::Default { .. } => (String::from("Default"), 1),
                ActiveBlock::Template { name, line, .. }
                | ActiveBlock::Profile { name, line, .. } => (name, line),
            };
            return Err(ProfileParseError::UnterminatedBlock { name, line });
        }

        let mut templates = HashMap::with_capacity(raw_templates.len());
        for key in template_order {
            if let Some(def) = raw_templates.get(&key) {
                let mut visiting = HashSet::new();
                let mut resolved = default_profile.clone();
                apply_template_patches(
                    &def.name,
                    &mut resolved,
                    &raw_templates,
                    &mut visiting,
                    def.line,
                )?;
                resolved.name = def.name.clone();
                templates.insert(def.name.clone(), resolved);
            }
        }

        Ok(Self {
            default_profile,
            templates,
            profiles,
        })
    }

    /// Looks up a profile by exact or case-insensitive name.
    pub fn find_by_name(&self, name: &str) -> Option<&BotProfile> {
        self.profiles
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// Returns all profiles matching `difficulty`.
    pub fn profiles_for_difficulty(&self, difficulty: BotDifficulty) -> Vec<&BotProfile> {
        self.profiles
            .iter()
            .filter(|p| p.matches_difficulty(difficulty))
            .collect()
    }

    /// Selects an unused profile matching `difficulty`, falling back to any profile or a
    /// synthesized fallback if every named profile is already in use.
    pub fn select_profile(
        &self,
        difficulty: BotDifficulty,
        used_names: &[&str],
        rng_seed: u64,
    ) -> BotProfile {
        let is_used = |name: &str| used_names.iter().any(|u| u.eq_ignore_ascii_case(name));

        let matching_unused: Vec<&BotProfile> = self
            .profiles
            .iter()
            .filter(|p| p.matches_difficulty(difficulty) && !is_used(&p.name))
            .collect();
        if !matching_unused.is_empty() {
            let idx = (mix64(rng_seed) as usize) % matching_unused.len();
            return matching_unused[idx].clone();
        }

        let any_unused: Vec<&BotProfile> = self
            .profiles
            .iter()
            .filter(|p| !is_used(&p.name))
            .collect();
        if !any_unused.is_empty() {
            let idx = (mix64(rng_seed) as usize) % any_unused.len();
            return any_unused[idx].clone();
        }

        let matching_all = self.profiles_for_difficulty(difficulty);
        if !matching_all.is_empty() {
            let idx = (mix64(rng_seed) as usize) % matching_all.len();
            return matching_all[idx].clone();
        }

        BotProfile::fallback("Bot", difficulty)
    }
}

fn apply_template_patches(
    name: &str,
    target: &mut BotProfile,
    raw_templates: &HashMap<String, TemplateDef>,
    visiting: &mut HashSet<String>,
    ref_line: usize,
) -> Result<(), ProfileParseError> {
    let key = name.to_ascii_lowercase();
    let Some(def) = raw_templates.get(&key) else {
        return Err(ProfileParseError::UnknownTemplate {
            name: name.to_string(),
            line: ref_line,
        });
    };
    if !visiting.insert(key.clone()) {
        return Err(ProfileParseError::CyclicInheritance {
            template: def.name.clone(),
        });
    }

    for parent in &def.parents {
        apply_template_patches(parent, target, raw_templates, visiting, def.line)?;
    }
    apply_patches(target, &def.patches);
    visiting.remove(&key);
    Ok(())
}

fn apply_patches(target: &mut BotProfile, patches: &[BlockPatch]) {
    for patch in patches {
        match patch {
            BlockPatch::Skill(v) => target.skill = *v,
            BlockPatch::Aggression(v) => target.aggression = *v,
            BlockPatch::ReactionTime(v) => target.reaction_time = *v,
            BlockPatch::AttackDelay(v) => target.attack_delay = *v,
            BlockPatch::Teamwork(v) => target.teamwork = *v,
            BlockPatch::AimFocusInitial(v) => target.aim_focus_initial = *v,
            BlockPatch::AimFocusDecay(v) => target.aim_focus_decay = *v,
            BlockPatch::AimFocusOffsetScale(v) => target.aim_focus_offset_scale = *v,
            BlockPatch::AimFocusInterval(v) => target.aim_focus_interval = *v,
            BlockPatch::ClearWeaponPreferences => target.weapon_preferences.clear(),
            BlockPatch::AddWeaponPreference(w) => target.weapon_preferences.push(*w),
            BlockPatch::Cost(v) => target.cost = *v,
            BlockPatch::Difficulty(m) => target.difficulty = *m,
            BlockPatch::VoicePitch(v) => target.voice_pitch = *v,
            BlockPatch::Skin(v) => target.skin = *v,
            BlockPatch::Walk(v) => target.walk = *v,
            BlockPatch::Crouch(v) => target.crouch = *v,
            BlockPatch::VoiceBank(v) => target.voice_bank = v.clone(),
            BlockPatch::Team(t) => target.team = *t,
            BlockPatch::LookMaxAccelNormal(v) => target.look_spring.max_accel_normal = *v,
            BlockPatch::LookStiffnessNormal(v) => target.look_spring.stiffness_normal = *v,
            BlockPatch::LookDampingNormal(v) => target.look_spring.damping_normal = *v,
            BlockPatch::LookMaxAccelAttacking(v) => target.look_spring.max_accel_attacking = *v,
            BlockPatch::LookStiffnessAttacking(v) => target.look_spring.stiffness_attacking = *v,
            BlockPatch::LookDampingAttacking(v) => target.look_spring.damping_attacking = *v,
        }
    }
}

fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_quotes = false;
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'"' {
            in_quotes = !in_quotes;
        } else if !in_quotes && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            return &line[..i];
        }
        i += 1;
    }
    line
}

fn tokenize_header(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in s.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                if !in_quotes && !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

fn split_plus_parents(spec: &str) -> Vec<String> {
    spec.split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_template_header(
    rest: &[String],
    line: usize,
    raw: &str,
) -> Result<(String, Vec<String>), ProfileParseError> {
    if rest.is_empty() {
        return Err(ProfileParseError::InvalidHeader {
            line,
            text: raw.to_string(),
        });
    }
    // Supports:
    //   Template Name
    //   Template Parent1+Parent2 Name
    //   Template Name : Parent1+Parent2
    if let Some(colon_pos) = rest.iter().position(|t| t == ":") {
        if colon_pos == 0 || colon_pos + 1 >= rest.len() {
            return Err(ProfileParseError::InvalidHeader {
                line,
                text: raw.to_string(),
            });
        }
        let name = rest[..colon_pos].join(" ");
        let mut parents = Vec::new();
        for tok in &rest[colon_pos + 1..] {
            parents.extend(split_plus_parents(tok.trim_end_matches(',')));
        }
        return Ok((name, parents));
    }
    if rest.len() == 1 {
        let joined = &rest[0];
        if let Some((name, parent_str)) = joined.split_once(':') {
            return Ok((name.trim().to_string(), split_plus_parents(parent_str)));
        }
        return Ok((joined.clone(), Vec::new()));
    }
    let name = rest.last().cloned().unwrap_or_default();
    let mut parents = Vec::new();
    for tok in &rest[..rest.len() - 1] {
        parents.extend(split_plus_parents(tok));
    }
    Ok((name, parents))
}

fn parse_profile_header(
    tokens: &[String],
    line: usize,
    raw: &str,
) -> Result<(String, Vec<String>), ProfileParseError> {
    if tokens.is_empty() {
        return Err(ProfileParseError::InvalidHeader {
            line,
            text: raw.to_string(),
        });
    }
    if tokens.len() == 1 {
        return Ok((tokens[0].clone(), Vec::new()));
    }
    // `Template1+Template2 BotName` (or `Template1 Template2 "Bot Name"`)
    let name = tokens.last().cloned().unwrap_or_default();
    let mut parents = Vec::new();
    for tok in &tokens[..tokens.len() - 1] {
        parents.extend(split_plus_parents(tok));
    }
    Ok((name, parents))
}

fn parse_attribute_line(line: &str, line_no: usize) -> Result<Option<BlockPatch>, ProfileParseError> {
    let (key, val) = if let Some((k, v)) = line.split_once('=') {
        (k.trim(), unquote(v.trim()))
    } else {
        let mut parts = line.splitn(2, char::is_whitespace);
        let k = parts.next().unwrap_or("").trim();
        let v = unquote(parts.next().unwrap_or("").trim());
        (k, v)
    };

    if key.is_empty() {
        return Ok(None);
    }

    let parse_f32 = || {
        val.parse::<f32>()
            .map_err(|_| ProfileParseError::InvalidValue {
                key: key.to_string(),
                value: val.to_string(),
                line: line_no,
            })
    };

    let parse_norm = || -> Result<f32, ProfileParseError> {
        let raw = parse_f32()?;
        // Valve `BotProfile.db` uses 0..100 integers; clamp into 0.0..=1.0.
        Ok((raw / 100.0).clamp(0.0, 1.0))
    };

    let parse_bool = || -> Result<bool, ProfileParseError> {
        if val == "1" || val.eq_ignore_ascii_case("true") || val.eq_ignore_ascii_case("yes") {
            Ok(true)
        } else if val == "0" || val.eq_ignore_ascii_case("false") || val.eq_ignore_ascii_case("no")
        {
            Ok(false)
        } else {
            Err(ProfileParseError::InvalidValue {
                key: key.to_string(),
                value: val.to_string(),
                line: line_no,
            })
        }
    };

    if key.eq_ignore_ascii_case("Skill") {
        Ok(Some(BlockPatch::Skill(parse_norm()?)))
    } else if key.eq_ignore_ascii_case("Aggression") {
        Ok(Some(BlockPatch::Aggression(parse_norm()?)))
    } else if key.eq_ignore_ascii_case("ReactionTime") {
        Ok(Some(BlockPatch::ReactionTime(parse_f32()?.max(0.0))))
    } else if key.eq_ignore_ascii_case("AttackDelay") {
        Ok(Some(BlockPatch::AttackDelay(parse_f32()?.max(0.0))))
    } else if key.eq_ignore_ascii_case("Teamwork") {
        Ok(Some(BlockPatch::Teamwork(parse_norm()?)))
    } else if key.eq_ignore_ascii_case("AimFocusInitial") {
        Ok(Some(BlockPatch::AimFocusInitial(parse_f32()?.max(0.0))))
    } else if key.eq_ignore_ascii_case("AimFocusDecay") {
        Ok(Some(BlockPatch::AimFocusDecay(parse_f32()?.clamp(0.0, 1.0))))
    } else if key.eq_ignore_ascii_case("AimFocusOffsetScale") {
        Ok(Some(BlockPatch::AimFocusOffsetScale(parse_f32()?.max(0.0))))
    } else if key.eq_ignore_ascii_case("AimFocusInterval") {
        Ok(Some(BlockPatch::AimFocusInterval(parse_f32()?.max(0.05))))
    } else if key.eq_ignore_ascii_case("WeaponPreference") {
        if val.eq_ignore_ascii_case("none") {
            Ok(Some(BlockPatch::ClearWeaponPreferences))
        } else {
            let weapon = BotWeapon::from_alias(val).ok_or_else(|| ProfileParseError::InvalidValue {
                key: key.to_string(),
                value: val.to_string(),
                line: line_no,
            })?;
            Ok(Some(BlockPatch::AddWeaponPreference(weapon)))
        }
    } else if key.eq_ignore_ascii_case("Cost") {
        let c = val.parse::<u8>().map_err(|_| ProfileParseError::InvalidValue {
            key: key.to_string(),
            value: val.to_string(),
            line: line_no,
        })?;
        Ok(Some(BlockPatch::Cost(c)))
    } else if key.eq_ignore_ascii_case("Difficulty") {
        let mask =
            BotDifficultyMask::parse(val).ok_or_else(|| ProfileParseError::InvalidValue {
                key: key.to_string(),
                value: val.to_string(),
                line: line_no,
            })?;
        Ok(Some(BlockPatch::Difficulty(mask)))
    } else if key.eq_ignore_ascii_case("VoicePitch") {
        let p = val.parse::<u16>().map_err(|_| ProfileParseError::InvalidValue {
            key: key.to_string(),
            value: val.to_string(),
            line: line_no,
        })?;
        Ok(Some(BlockPatch::VoicePitch(p)))
    } else if key.eq_ignore_ascii_case("Skin") {
        let s = val.parse::<u8>().map_err(|_| ProfileParseError::InvalidValue {
            key: key.to_string(),
            value: val.to_string(),
            line: line_no,
        })?;
        Ok(Some(BlockPatch::Skin(s)))
    } else if key.eq_ignore_ascii_case("Walk") {
        Ok(Some(BlockPatch::Walk(parse_bool()?)))
    } else if key.eq_ignore_ascii_case("Crouch") {
        Ok(Some(BlockPatch::Crouch(parse_bool()?)))
    } else if key.eq_ignore_ascii_case("VoiceBank") {
        if val.is_empty() || val.eq_ignore_ascii_case("none") {
            Ok(Some(BlockPatch::VoiceBank(None)))
        } else {
            Ok(Some(BlockPatch::VoiceBank(Some(val.to_string()))))
        }
    } else if key.eq_ignore_ascii_case("Team") {
        if val.eq_ignore_ascii_case("t") || val.eq_ignore_ascii_case("terrorist") {
            Ok(Some(BlockPatch::Team(Some(BotTeam::Terrorist))))
        } else if val.eq_ignore_ascii_case("ct") || val.eq_ignore_ascii_case("counterterrorist") {
            Ok(Some(BlockPatch::Team(Some(BotTeam::CounterTerrorist))))
        } else if val.eq_ignore_ascii_case("any") {
            Ok(Some(BlockPatch::Team(None)))
        } else {
            Err(ProfileParseError::InvalidValue {
                key: key.to_string(),
                value: val.to_string(),
                line: line_no,
            })
        }
    } else if key.eq_ignore_ascii_case("LookAngleMaxAccelNormal") {
        Ok(Some(BlockPatch::LookMaxAccelNormal(parse_f32()?.max(1.0))))
    } else if key.eq_ignore_ascii_case("LookAngleStiffnessNormal") {
        Ok(Some(BlockPatch::LookStiffnessNormal(parse_f32()?.max(1.0))))
    } else if key.eq_ignore_ascii_case("LookAngleDampingNormal") {
        Ok(Some(BlockPatch::LookDampingNormal(parse_f32()?.max(0.5))))
    } else if key.eq_ignore_ascii_case("LookAngleMaxAccelAttacking") {
        Ok(Some(BlockPatch::LookMaxAccelAttacking(parse_f32()?.max(1.0))))
    } else if key.eq_ignore_ascii_case("LookAngleStiffnessAttacking") {
        Ok(Some(BlockPatch::LookStiffnessAttacking(parse_f32()?.max(1.0))))
    } else if key.eq_ignore_ascii_case("LookAngleDampingAttacking") {
        Ok(Some(BlockPatch::LookDampingAttacking(parse_f32()?.max(0.5))))
    } else {
        // Ignore unknown future extension attributes gracefully, matching Valve's parser.
        Ok(None)
    }
}

fn unquote(s: &str) -> &str {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[inline]
pub(crate) fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Built-in CS 1.6 ZBot profile database covering Easy, Normal, Hard, and Expert templates
/// and a full roster of named bots.
pub const BUILTIN_BOT_PROFILE_DB: &str = r#"
// Built-in Counter-Strike 1.6 ZBot Profile Database (clean-room default roster)

Default
    Skill = 50
    Aggression = 50
    ReactionTime = 0.3
    AttackDelay = 0.0
    Teamwork = 75
    AimFocusInitial = 20
    AimFocusDecay = 0.7
    AimFocusOffsetScale = 0.30
    AimFocusInterval = 0.8
    WeaponPreference = none
    Cost = 0
    Difficulty = EASY+NORMAL+HARD+EXPERT
    VoicePitch = 100
    Skin = 0
    Walk = 0
    Crouch = 0
    VoiceBank = "BotChatter.db"
End

Template Rifle
    WeaponPreference = m4a1
    WeaponPreference = ak47
    WeaponPreference = famas
    WeaponPreference = galil
    WeaponPreference = mp5
End

Template RifleT
    WeaponPreference = ak47
    WeaponPreference = m4a1
    WeaponPreference = galil
    WeaponPreference = famas
    WeaponPreference = mp5
End

Template Punch
    WeaponPreference = aug
    WeaponPreference = sg552
    WeaponPreference = famas
    WeaponPreference = galil
    WeaponPreference = mp5
End

Template Sniper
    WeaponPreference = awp
    WeaponPreference = sg550
    WeaponPreference = g3sg1
    WeaponPreference = scout
    WeaponPreference = deagle
    WeaponPreference = mp5
End

Template Power
    WeaponPreference = m249
    WeaponPreference = xm1014
    WeaponPreference = m3
    WeaponPreference = p90
    WeaponPreference = deagle
End

Template Shotgun
    WeaponPreference = xm1014
    WeaponPreference = m3
    WeaponPreference = mp5
    WeaponPreference = deagle
End

Template SMG
    WeaponPreference = mp5
    WeaponPreference = p90
    WeaponPreference = ump45
    WeaponPreference = mac10
    WeaponPreference = tmp
End

Template Shield
    WeaponPreference = shield
    WeaponPreference = deagle
    WeaponPreference = fn57
    WeaponPreference = usp
End

Template Spray
    WeaponPreference = p90
    WeaponPreference = mac10
    WeaponPreference = tmp
    WeaponPreference = mp5
End

Template Easy
    Skill = 20
    Aggression = 30
    ReactionTime = 0.48
    AttackDelay = 0.45
    Teamwork = 45
    AimFocusInitial = 28
    AimFocusDecay = 0.85
    AimFocusOffsetScale = 0.45
    AimFocusInterval = 1.0
    Cost = 1
    Difficulty = EASY
End

Template Normal
    Skill = 50
    Aggression = 55
    ReactionTime = 0.32
    AttackDelay = 0.15
    Teamwork = 65
    AimFocusInitial = 18
    AimFocusDecay = 0.70
    AimFocusOffsetScale = 0.30
    AimFocusInterval = 0.80
    Cost = 2
    Difficulty = NORMAL
End

Template Tough
    Skill = 75
    Aggression = 68
    ReactionTime = 0.22
    AttackDelay = 0.05
    Teamwork = 80
    AimFocusInitial = 12
    AimFocusDecay = 0.58
    AimFocusOffsetScale = 0.20
    AimFocusInterval = 0.60
    Cost = 3
    Difficulty = HARD
End

Template Elite
    Skill = 96
    Aggression = 78
    ReactionTime = 0.12
    AttackDelay = 0.0
    Teamwork = 92
    AimFocusInitial = 6
    AimFocusDecay = 0.45
    AimFocusOffsetScale = 0.12
    AimFocusInterval = 0.45
    Cost = 4
    Difficulty = EXPERT
End

// ---- Expert Bots ----
Elite+Rifle Cliffe
    Skin = 1
    VoicePitch = 96
End

Elite+RifleT Minh
    Skin = 2
    VoicePitch = 104
End

Elite+Sniper Phoenix
    Aggression = 45
    Walk = 1
    Crouch = 1
    VoicePitch = 92
End

Elite+Rifle Viper
    Aggression = 88
    VoicePitch = 98
End

Elite+Sniper Hawk
    Walk = 1
    VoicePitch = 94
End

Elite+Punch Steel
    Aggression = 85
    VoicePitch = 88
End

Elite+Power Tank
    Aggression = 95
    VoicePitch = 84
End

Elite+SMG Ghost
    Walk = 1
    Teamwork = 95
    VoicePitch = 106
End

// ---- Hard Bots ----
Tough+Rifle Stone
    Skin = 3
    VoicePitch = 95
End

Tough+RifleT Crusher
    Aggression = 82
    VoicePitch = 89
End

Tough+Sniper Wolf
    Crouch = 1
    VoicePitch = 97
End

Tough+Shotgun Gunner
    Aggression = 80
    VoicePitch = 93
End

Tough+Punch Spike
    VoicePitch = 103
End

Tough+Shield Wyatt
    Teamwork = 95
    VoicePitch = 99
End

Tough+SMG Rico
    Aggression = 76
    VoicePitch = 108
End

Tough+Power Boris
    Aggression = 86
    VoicePitch = 86
End

// ---- Normal Bots ----
Normal+Rifle Vinnie
    VoicePitch = 101
End

Normal+RifleT Yogi
    Aggression = 60
    VoicePitch = 94
End

Normal+SMG Gabe
    VoicePitch = 102
End

Normal+Shotgun Toby
    Aggression = 70
    VoicePitch = 110
End

Normal+Sniper Zane
    Crouch = 1
    VoicePitch = 96
End

Normal+Spray Reed
    Aggression = 65
    VoicePitch = 105
End

Normal+Punch Dean
    VoicePitch = 98
End

Normal+Shield Dustin
    Teamwork = 85
    VoicePitch = 100
End

// ---- Easy Bots ----
Easy+SMG Shawn
    VoicePitch = 108
End

Easy+Shotgun Wade
    Aggression = 40
    VoicePitch = 92
End

Easy+Rifle Seth
    VoicePitch = 104
End

Easy+Spray Brett
    Aggression = 55
    VoicePitch = 112
End

Easy+RifleT Kurt
    VoicePitch = 96
End

Easy+SMG Kyle
    VoicePitch = 107
End

Easy+Power Hank
    Aggression = 45
    VoicePitch = 88
End

Easy+Shotgun Pete
    VoicePitch = 95
End
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_builtin_roster_for_all_difficulties() {
        let db = BotProfileDb::builtin();
        assert!(db.profiles.len() >= 32);
        for diff in [
            BotDifficulty::Easy,
            BotDifficulty::Normal,
            BotDifficulty::Hard,
            BotDifficulty::Expert,
        ] {
            let list = db.profiles_for_difficulty(diff);
            assert!(
                list.len() >= 8,
                "expected at least 8 profiles for {diff:?}, got {}",
                list.len()
            );
        }

        let cliffe = db.find_by_name("Cliffe").expect("Cliffe exists");
        assert!(cliffe.matches_difficulty(BotDifficulty::Expert));
        assert!(!cliffe.matches_difficulty(BotDifficulty::Easy));
        assert!((cliffe.skill - 0.96).abs() < 1e-4);
        assert_eq!(cliffe.weapon_preferences[0], BotWeapon::M4a1);
        assert_eq!(cliffe.voice_pitch, 96);
        assert_eq!(cliffe.skin, 1);

        let phoenix = db.find_by_name("Phoenix").expect("Phoenix exists");
        assert!(phoenix.walk);
        assert!(phoenix.crouch);
        assert_eq!(phoenix.weapon_preferences[0], BotWeapon::Awp);
        assert!((phoenix.aggression - 0.45).abs() < 1e-4);
    }

    #[test]
    fn parses_multi_level_template_inheritance_and_overrides() {
        let src = r#"
// Comment header
Default
    Skill = 40
    Teamwork = 60
    AimFocusInitial = 22
    VoiceBank = "CustomBank.db"
End

Template BaseRifle
    WeaponPreference = m4a1
    WeaponPreference = ak47
    Skill = 70
End

Template BaseRifle EliteRifle
    Skill = 90
    ReactionTime = 0.14
    AttackDelay = 0.02
    AimFocusDecay = 0.48
    AimFocusOffsetScale = 0.15
    AimFocusInterval = 0.50
    Cost = 4
    Difficulty = HARD+EXPERT
End

Template Shotgun
    WeaponPreference = xm1014
End

EliteRifle+Shotgun "Agent Zero"
    Aggression = 85
    Walk = 1
    Crouch = 1
    VoicePitch = 112
    Skin = 4
End
"#;
        let db = BotProfileDb::parse(src).unwrap();
        let p = db.find_by_name("Agent Zero").expect("Agent Zero parsed");
        assert!((p.skill - 0.90).abs() < 1e-4);
        assert!((p.aggression - 0.85).abs() < 1e-4);
        assert!((p.teamwork - 0.60).abs() < 1e-4);
        assert!((p.reaction_time - 0.14).abs() < 1e-4);
        assert!((p.attack_delay - 0.02).abs() < 1e-4);
        assert!((p.aim_focus_initial - 22.0).abs() < 1e-4);
        assert!((p.aim_focus_decay - 0.48).abs() < 1e-4);
        assert!((p.aim_focus_offset_scale - 0.15).abs() < 1e-4);
        assert!((p.aim_focus_interval - 0.50).abs() < 1e-4);
        assert_eq!(p.cost, 4);
        assert!(p.difficulty.contains(BotDifficulty::Hard));
        assert!(p.difficulty.contains(BotDifficulty::Expert));
        assert!(!p.difficulty.contains(BotDifficulty::Easy));
        assert!(p.walk);
        assert!(p.crouch);
        assert_eq!(p.voice_pitch, 112);
        assert_eq!(p.skin, 4);
        assert_eq!(p.voice_bank.as_deref(), Some("CustomBank.db"));
        // Shotgun template didn't exist, wait: we used `+Shotgun`? No, wait—let's check!
        // Here `Shotgun` wasn't defined; if `Shotgun` isn't defined, `UnknownTemplate` would fire!
        // Wait: in this test `Shotgun` wasn't defined! Let's make sure the test only tests defined templates,
        // and add a separate test for UnknownTemplate!
    }

    #[test]
    fn errors_on_unknown_and_cyclic_templates() {
        let missing = "UnknownTpl BotOne\nEnd\n";
        assert!(matches!(
            BotProfileDb::parse(missing),
            Err(ProfileParseError::UnknownTemplate { .. })
        ));

        let cycle = r#"
Template A : B
    Skill = 50
End
Template B : A
    Skill = 60
End
"#;
        assert!(matches!(
            BotProfileDb::parse(cycle),
            Err(ProfileParseError::CyclicInheritance { .. })
        ));
    }

    #[test]
    fn select_profile_avoids_duplicate_names_when_available() {
        let db = BotProfileDb::builtin();
        let first = db.select_profile(BotDifficulty::Normal, &[], 42);
        let second = db.select_profile(BotDifficulty::Normal, &[&first.name], 42);
        assert_ne!(first.name, second.name);
        assert!(second.matches_difficulty(BotDifficulty::Normal));
    }
}
