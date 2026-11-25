//! Structured ABI v2 operation specs, domain enums, VFX primitives, custom weapons, menus,
//! HUD/DHUD/MOTD specs, localization requests, RBAC admin flags, polyglot RPC, leaderboards,
//! and capability-gated webhook specs.
//!
//! Clean-room GPL-3.0-or-later definitions referencing AMX Mod X (`amxconst.inc`, `cstrike.inc`,
//! `newmenus.inc`, `dhudmessage.inc`) and Protocol 48 `TE_*` / `UserMsg` wire semantics
//! (read 2026-11-26).

use serde::{Deserialize, Serialize};

/// Player armor protection kind (`cstrike.inc` `CS_ARMOR_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum ArmorKind {
    #[default]
    None = 0,
    Kevlar = 1,
    VestHelm = 2,
}

impl ArmorKind {
    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::Kevlar,
            2 => Self::VestHelm,
            _ => Self::None,
        }
    }
}

/// Counter-Strike 1.6 team identifier (`CS_TEAM_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum TeamKind {
    #[default]
    Unassigned = 0,
    Terrorist = 1,
    Ct = 2,
    Spectator = 3,
}

impl TeamKind {
    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::Terrorist,
            2 => Self::Ct,
            3 => Self::Spectator,
            _ => Self::Unassigned,
        }
    }
}

/// Declarative custom weapon archetype registered by plugins (`register_custom_weapon`) and
/// granted via `give_custom_weapon(slot, id_name)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomWeaponSpec {
    pub id_name: String,
    /// Underlying CS 1.6 `WeaponId` numeric value (`1..=30`, e.g., `28` = AK-47).
    pub base_weapon: u8,
    pub display_name: String,
    pub v_model: Option<String>,
    pub p_model: Option<String>,
    pub w_model: Option<String>,
    pub fire_sound: Option<String>,
    pub damage_mult: f32,
    pub clip_size: Option<i32>,
    pub deploy_speed_mult: f32,
    pub recoil_mult: f32,
    pub knockback: f32,
    pub cost: i32,
}

impl Default for CustomWeaponSpec {
    fn default() -> Self {
        Self {
            id_name: String::new(),
            base_weapon: 28,
            display_name: String::new(),
            v_model: None,
            p_model: None,
            w_model: None,
            fire_sound: None,
            damage_mult: 1.0,
            clip_size: None,
            deploy_speed_mult: 1.0,
            recoil_mult: 1.0,
            knockback: 1.0,
            cost: 0,
        }
    }
}

/// High-level 1-line VFX primitives emitted over Protocol 48 `svc_temp_entity` (`TE_*`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VfxOp {
    /// `TE_BEAMCYLINDER` expanding shockwave ring (Frost Nade, level-up, heal wave).
    ShockwaveRing {
        origin: [f32; 3],
        radius: f32,
        rgba: [u8; 4],
        life_decisec: u8,
        width: u8,
        slot: Option<u8>,
    },
    /// `TE_BEAMPOINTS` laser / railgun / tripwire beam.
    LaserBeam {
        start: [f32; 3],
        end: [f32; 3],
        rgba: [u8; 4],
        life_decisec: u8,
        width: u8,
        noise: u8,
        slot: Option<u8>,
    },
    /// `TE_BEAMFOLLOW` colored trail attached to an entity index.
    FollowTrail {
        ent_index: u16,
        rgba: [u8; 4],
        life_decisec: u8,
        width: u8,
        slot: Option<u8>,
    },
    /// `TE_KILLBEAM` removes all beams attached to an entity index.
    KillBeam {
        ent_index: u16,
        slot: Option<u8>,
    },
    /// `TE_DLIGHT` dynamic point glow (VIP/leader/zombie aura).
    AuraLight {
        origin: [f32; 3],
        radius: u8,
        rgb: [u8; 3],
        life_decisec: u8,
        decay: u8,
        slot: Option<u8>,
    },
    /// `TE_SPARKS` metallic spark shower.
    Sparks {
        origin: [f32; 3],
        slot: Option<u8>,
    },
    /// `TE_IMPLOSION` inward-moving particle burst.
    Implosion {
        origin: [f32; 3],
        radius: u8,
        count: u8,
        life_decisec: u8,
        slot: Option<u8>,
    },
    /// `TE_TELEPORT` classic Quake/GoldSrc teleport particles.
    TeleportSplash {
        origin: [f32; 3],
        slot: Option<u8>,
    },
    /// `TE_LAVASPLASH` classic red/orange particle fountain.
    LavaSplash {
        origin: [f32; 3],
        slot: Option<u8>,
    },
    /// `TE_EXPLOSION` fireball + particles + dlight.
    Explosion {
        origin: [f32; 3],
        scale: u8,
        flags: u8,
        slot: Option<u8>,
    },
    /// `TE_BLOODSTREAM` directional blood jet.
    BloodStream {
        origin: [f32; 3],
        dir: [f32; 3],
        color: u8,
        speed: u8,
        slot: Option<u8>,
    },
    /// `TE_WORLDDECAL` / `TE_WORLDDECALHIGH` permanent brush decal by WAD name (e.g. `{scorch1`).
    WorldDecal {
        origin: [f32; 3],
        decal_name: String,
        slot: Option<u8>,
    },
}

/// One item in a host-managed paginated GoldSrc menu (`\y`/`\w`/`\r`/`\d`/`\R` authentic styling).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuItemSpec {
    pub label: String,
    pub info: String,
    pub enabled: bool,
    /// Blank spacer line (`menu_addblank`), takes no selectable slot if `slot_blank` is false.
    #[serde(default)]
    pub is_blank: bool,
    /// Static descriptive line (`menu_addtext`), takes no key slot.
    #[serde(default)]
    pub is_text: bool,
}

impl MenuItemSpec {
    pub fn selectable(label: impl Into<String>, info: impl Into<String>, enabled: bool) -> Self {
        Self {
            label: label.into(),
            info: info.into(),
            enabled,
            is_blank: false,
            is_text: false,
        }
    }

    pub fn blank() -> Self {
        Self {
            label: String::new(),
            info: String::new(),
            enabled: false,
            is_blank: true,
            is_text: false,
        }
    }

    pub fn text(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            info: String::new(),
            enabled: false,
            is_blank: false,
            is_text: true,
        }
    }
}

/// Specification to create a paginated menu inside the host `MenuManager`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuCreateSpec {
    pub title: String,
    pub items: Vec<MenuItemSpec>,
    pub callback_id: i32,
    #[serde(default)]
    pub callback_name: String,
    #[serde(default)]
    pub back_label: Option<String>,
    #[serde(default)]
    pub next_label: Option<String>,
    #[serde(default)]
    pub exit_label: Option<String>,
    /// Items per page (`1..=7`, default `7`; `0` disables pagination).
    pub per_page: u8,
    /// Whether `0. Exit` is shown (`true` by default).
    pub allow_exit: bool,
    /// Color escape prefix for numbers (default `"\\r"` matching AMXX `newmenus`).
    pub number_color: String,
}

impl Default for MenuCreateSpec {
    fn default() -> Self {
        Self {
            title: String::new(),
            items: Vec::new(),
            callback_id: 0,
            callback_name: String::new(),
            back_label: None,
            next_label: None,
            exit_label: None,
            per_page: 7,
            allow_exit: true,
            number_color: "\\r".to_string(),
        }
    }
}

/// Request to show a previously created menu to a player slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuDisplaySpec {
    pub slot: u8,
    pub menu_id: u32,
    pub page: u16,
    pub time_sec: i8,
}

/// Classic GoldSrc `TE_TEXTMESSAGE` HUD message spec (channels `0..=4`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HudMessageSpec {
    pub slot: Option<u8>,
    pub channel: u8,
    pub x: f32,
    pub y: f32,
    pub rgba1: [u8; 4],
    pub rgba2: [u8; 4],
    pub effect: u8,
    pub fadein: f32,
    pub hold: f32,
    pub fadeout: f32,
    pub fxtime: f32,
    pub text: String,
}

/// Large director HUD message (`svc_director` `DRC_CMD_MESSAGE`, `dhudmessage.inc`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DhudMessageSpec {
    pub slot: Option<u8>,
    pub x: f32,
    pub y: f32,
    pub rgb: [u8; 3],
    pub effect: u8,
    pub fadein: f32,
    pub hold: f32,
    pub fadeout: f32,
    pub fxtime: f32,
    pub text: String,
}

/// VGUI2 `MOTD.res` black-and-orange HTML/text MOTD window display request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowMotdSpec {
    pub slot: u8,
    pub title: String,
    pub body: String,
}

/// Classic `ScreenFade` usermsg parameters (`duration`/`hold` in seconds, converted to 12.4 fixed-point).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScreenFadeSpec {
    pub slot: Option<u8>,
    pub duration_sec: f32,
    pub hold_sec: f32,
    pub flags: u16,
    pub rgba: [u8; 4],
}

/// Classic `ScreenShake` usermsg parameters (`amplitude`/`duration`/`frequency` converted to 12.4 fixed-point).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScreenShakeSpec {
    pub slot: Option<u8>,
    pub amplitude: f32,
    pub duration_sec: f32,
    pub frequency: f32,
}

/// Classic `BarTime` progress bar usermsg (`seconds`, `0` cancels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BarTimeSpec {
    pub slot: u8,
    pub seconds: u16,
}

/// Localization translation lookup request (`LangCatalog::tr`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LangTrSpec {
    pub slot: Option<u8>,
    pub key: String,
    pub args: Vec<(String, String)>,
}

/// Classic AMX Mod X RBAC access flags (`a`..`z`, `amxconst.inc`, read 2026-11-26).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AdminFlags(pub u32);

impl AdminFlags {
    pub const ALL: Self = Self(0);
    pub const IMMUNITY: Self = Self(1 << 0); // a
    pub const RESERVATION: Self = Self(1 << 1); // b
    pub const KICK: Self = Self(1 << 2); // c
    pub const BAN: Self = Self(1 << 3); // d
    pub const SLAY: Self = Self(1 << 4); // e
    pub const MAP: Self = Self(1 << 5); // f
    pub const CVAR: Self = Self(1 << 6); // g
    pub const CFG: Self = Self(1 << 7); // h
    pub const CHAT: Self = Self(1 << 8); // i
    pub const VOTE: Self = Self(1 << 9); // j
    pub const PASSWORD: Self = Self(1 << 10); // k
    pub const RCON: Self = Self(1 << 11); // l
    pub const LEVEL_A: Self = Self(1 << 12); // m
    pub const LEVEL_B: Self = Self(1 << 13); // n
    pub const LEVEL_C: Self = Self(1 << 14); // o
    pub const LEVEL_D: Self = Self(1 << 15); // p
    pub const LEVEL_E: Self = Self(1 << 16); // q
    pub const LEVEL_F: Self = Self(1 << 17); // r
    pub const LEVEL_G: Self = Self(1 << 18); // s
    pub const LEVEL_H: Self = Self(1 << 19); // t (VIP)
    pub const MENU: Self = Self(1 << 20); // u
    pub const BAN_TEMP: Self = Self(1 << 21); // v
    pub const ADMIN: Self = Self(1 << 24); // y
    pub const USER: Self = Self(1 << 25); // z

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bit_for_char(ch: char) -> Option<u32> {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() {
            Some(1u32 << ((lower as u8) - b'a'))
        } else {
            None
        }
    }

    pub fn parse_str(flags: &str) -> Self {
        let mut mask = 0u32;
        for ch in flags.chars() {
            if let Some(bit) = Self::bit_for_char(ch) {
                mask |= bit;
            }
        }
        Self(mask)
    }

    pub fn has_char(self, ch: char) -> bool {
        Self::bit_for_char(ch).is_some_and(|bit| (self.0 & bit) != 0)
    }

    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }

    pub fn to_flag_string(self) -> String {
        let mut out = String::new();
        for i in 0..26u8 {
            if (self.0 & (1u32 << i)) != 0 {
                out.push((b'a' + i) as char);
            }
        }
        out
    }
}

/// Polyglot value exchanged across plugins over the inter-plugin RPC / event bus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RpcValue {
    Int(i32),
    Float(f32),
    Str(String),
    Bytes(Vec<u8>),
}

/// Request to export a named RPC function from a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcExportSpec {
    pub fn_name: String,
    pub callback_id: i32,
    pub callback_name: String,
}

/// Request to synchronously call an exported polyglot RPC function.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcCallSpec {
    pub fn_name: String,
    pub args: Vec<RpcValue>,
}

/// Request to broadcast a named cross-plugin event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcEmitEventSpec {
    pub event_name: String,
    pub payload_json: String,
    pub args: Vec<RpcValue>,
}

/// One row stored in a sorted `LeaderboardStore` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaderboardEntry {
    pub rank: usize,
    pub key: String,
    pub score: f64,
    pub payload_json: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaderboardUpsertSpec {
    pub table: String,
    pub key: String,
    pub score: f64,
    pub payload_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaderboardTopQuery {
    pub table: String,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaderboardRankQuery {
    pub table: String,
    pub key: String,
}

/// Non-blocking capability-gated HTTP POST JSON webhook request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpPostJsonSpec {
    pub url: String,
    pub body: String,
    pub callback_id: i32,
}

/// Completed non-blocking HTTP webhook delivery handed back to the owning plugin on a later tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpWebhookResult {
    pub mod_id: String,
    pub callback_id: i32,
    pub status_code: u16,
    pub body: String,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_flags_match_amxx_bit_positions() {
        let flags = AdminFlags::parse_str("abctuz");
        assert!(flags.contains(AdminFlags::IMMUNITY));
        assert!(flags.contains(AdminFlags::RESERVATION));
        assert!(flags.contains(AdminFlags::KICK));
        assert!(flags.contains(AdminFlags::LEVEL_H));
        assert!(flags.contains(AdminFlags::MENU));
        assert!(flags.contains(AdminFlags::USER));
        assert!(flags.has_char('t'));
        assert!(!flags.has_char('d'));
        assert_eq!(flags.to_flag_string(), "abctuz");
        assert_eq!(AdminFlags::LEVEL_H.bits(), 1 << 19);
        assert_eq!(AdminFlags::USER.bits(), 1 << 25);
    }
}
