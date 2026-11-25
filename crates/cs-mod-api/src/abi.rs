//! Stable ABI v2 enums (`HookId`, `HookPhase`, `HookAction`), mutable ReAPI-style hook context
//! structs, client HUD draw commands, and compact auto-download wire packets carried over custom
//! user messages.
//!
//! Clean-room design referencing ReAPI / ReGameDLL_CS hook semantics (`reapi_gamedll_const.inc`,
//! `regamedll/dlls/player.cpp`, `client.cpp`, `multiplay_gamerules.cpp`, read 2026-11-26) under
//! GPL-3.0-or-later.

use serde::{Deserialize, Serialize};

/// Reserved custom usermsg carrying one client-side mod descriptor per sign-on entry.
pub const USERMSG_MOD_MANIFEST: &str = "CsModManifest";
/// Reserved custom usermsg carrying a single chunk (`<= MOD_CHUNK_MAX_DATA` bytes) of a client mod.
pub const USERMSG_MOD_CHUNK: &str = "CsModChunk";
/// General-purpose mod custom event usermsg prefixable by server mods.
pub const USERMSG_MOD_EVENT: &str = "CsModEvent";
/// Client-to-server reliable stringcmd requesting the next chunk (`csmod_req <idx> <offset>`).
pub const CMD_MOD_REQUEST: &str = "csmod_req";
/// Maximum data payload bytes per `CsModChunk` message (fits comfortably inside 255-byte usermsg limit).
pub const MOD_CHUNK_MAX_DATA: usize = 180;

/// Which execution side(s) a mod targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModSide {
    Server,
    Client,
    Both,
}

impl ModSide {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "server" => Some(Self::Server),
            "client" => Some(Self::Client),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Client => "client",
            Self::Both => "both",
        }
    }

    pub fn has_server(self) -> bool {
        matches!(self, Self::Server | Self::Both)
    }

    pub fn has_client(self) -> bool {
        matches!(self, Self::Client | Self::Both)
    }
}

/// Phase within a pre/post hook chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u32)]
pub enum HookPhase {
    Pre = 0,
    Post = 1,
}

impl HookPhase {
    pub fn from_i32(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::Pre),
            1 => Some(Self::Post),
            _ => None,
        }
    }
}

/// Control-flow directive returned by a mod hook invocation (`ReAPI`-style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum HookAction {
    /// Continue invoking remaining hooks and the underlying inner handler.
    Continue = 0,
    /// Mark handled, continue chain and invoke inner handler.
    Handled = 1,
    /// Replace return value/override args, continue chain and invoke inner handler.
    Override = 2,
    /// Cancel inner handler (skip base `GameDll` or `EngineApi` execution).
    Supercede = 3,
}

impl HookAction {
    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::Handled,
            2 => Self::Override,
            3 => Self::Supercede,
            _ => Self::Continue,
        }
    }
}

/// Every hook point across `GameDll`, ReAPI CS 1.6 gameplay lifecycle, and `EngineApi`.
///
/// All ABI v1 discriminants (`1..=18`, `30..=31`, `100..=107`) are preserved verbatim for
/// backwards compatibility with v1 `.wasm` mods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u32)]
pub enum HookId {
    // GameDll hooks (v1)
    ServerActivate = 1,
    ServerDeactivate = 2,
    SpawnEntity = 3,
    ClientConnect = 4,
    ClientPutInServer = 5,
    ClientDisconnect = 6,
    ClientCommand = 7,
    ClientUserinfoChanged = 8,
    RunUsercmd = 9,
    PostUsercmd = 10,
    StartFrame = 11,
    Think = 12,
    Touch = 13,
    Blocked = 14,
    UpdateClientData = 15,
    GetWeaponData = 16,
    ServerCommand = 17,
    AddToFullPack = 18,
    // Gameplay hooks (v1 + v2 ReAPI)
    PlayerDeath = 30,
    RoundStart = 31,
    PlayerSpawn = 32,
    PlayerTakeDamage = 33,
    PlayerKilled = 34,
    PlayerPreThink = 35,
    PlayerPostThink = 36,
    PlayerJump = 37,
    PlayersShouldCollide = 38,
    CanBuyItem = 39,
    CanHavePlayerItem = 40,
    WeaponDeploy = 41,
    WeaponPrimaryAttack = 42,
    WeaponReload = 43,
    PlayerSay = 44,
    MenuSelect = 45,
    FreezeTimeEnd = 46,
    RoundEnd = 47,
    BombPlanting = 48,
    BombPlanted = 49,
    BombDefusing = 50,
    BombDefused = 51,
    BombExploded = 52,
    HostageTouched = 53,
    HostageRescued = 54,
    GrenadeThrow = 55,
    // EngineApi hooks (v1)
    EngEmitSound = 100,
    EngSendUserMsg = 101,
    EngClientPrint = 102,
    EngCenterPrint = 103,
    EngCreateEntity = 104,
    EngRemoveEntity = 105,
    EngKick = 106,
    EngChangeLevel = 107,
}

impl HookId {
    pub fn from_i32(v: i32) -> Option<Self> {
        let u = u32::try_from(v).ok()?;
        Self::from_u32(u)
    }

    pub fn from_u32(v: u32) -> Option<Self> {
        match v {
            1 => Some(Self::ServerActivate),
            2 => Some(Self::ServerDeactivate),
            3 => Some(Self::SpawnEntity),
            4 => Some(Self::ClientConnect),
            5 => Some(Self::ClientPutInServer),
            6 => Some(Self::ClientDisconnect),
            7 => Some(Self::ClientCommand),
            8 => Some(Self::ClientUserinfoChanged),
            9 => Some(Self::RunUsercmd),
            10 => Some(Self::PostUsercmd),
            11 => Some(Self::StartFrame),
            12 => Some(Self::Think),
            13 => Some(Self::Touch),
            14 => Some(Self::Blocked),
            15 => Some(Self::UpdateClientData),
            16 => Some(Self::GetWeaponData),
            17 => Some(Self::ServerCommand),
            18 => Some(Self::AddToFullPack),
            30 => Some(Self::PlayerDeath),
            31 => Some(Self::RoundStart),
            32 => Some(Self::PlayerSpawn),
            33 => Some(Self::PlayerTakeDamage),
            34 => Some(Self::PlayerKilled),
            35 => Some(Self::PlayerPreThink),
            36 => Some(Self::PlayerPostThink),
            37 => Some(Self::PlayerJump),
            38 => Some(Self::PlayersShouldCollide),
            39 => Some(Self::CanBuyItem),
            40 => Some(Self::CanHavePlayerItem),
            41 => Some(Self::WeaponDeploy),
            42 => Some(Self::WeaponPrimaryAttack),
            43 => Some(Self::WeaponReload),
            44 => Some(Self::PlayerSay),
            45 => Some(Self::MenuSelect),
            46 => Some(Self::FreezeTimeEnd),
            47 => Some(Self::RoundEnd),
            48 => Some(Self::BombPlanting),
            49 => Some(Self::BombPlanted),
            50 => Some(Self::BombDefusing),
            51 => Some(Self::BombDefused),
            52 => Some(Self::BombExploded),
            53 => Some(Self::HostageTouched),
            54 => Some(Self::HostageRescued),
            55 => Some(Self::GrenadeThrow),
            100 => Some(Self::EngEmitSound),
            101 => Some(Self::EngSendUserMsg),
            102 => Some(Self::EngClientPrint),
            103 => Some(Self::EngCenterPrint),
            104 => Some(Self::EngCreateEntity),
            105 => Some(Self::EngRemoveEntity),
            106 => Some(Self::EngKick),
            107 => Some(Self::EngChangeLevel),
            _ => None,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "server_activate" => Some(Self::ServerActivate),
            "server_deactivate" => Some(Self::ServerDeactivate),
            "spawn_entity" => Some(Self::SpawnEntity),
            "client_connect" => Some(Self::ClientConnect),
            "client_put_in_server" => Some(Self::ClientPutInServer),
            "client_disconnect" => Some(Self::ClientDisconnect),
            "client_command" => Some(Self::ClientCommand),
            "client_userinfo_changed" => Some(Self::ClientUserinfoChanged),
            "run_usercmd" => Some(Self::RunUsercmd),
            "post_usercmd" => Some(Self::PostUsercmd),
            "start_frame" => Some(Self::StartFrame),
            "think" => Some(Self::Think),
            "touch" => Some(Self::Touch),
            "blocked" => Some(Self::Blocked),
            "update_client_data" => Some(Self::UpdateClientData),
            "get_weapon_data" => Some(Self::GetWeaponData),
            "server_command" => Some(Self::ServerCommand),
            "add_to_full_pack" => Some(Self::AddToFullPack),
            "player_death" => Some(Self::PlayerDeath),
            "round_start" => Some(Self::RoundStart),
            "player_spawn" | "playerspawn" => Some(Self::PlayerSpawn),
            "player_take_damage" | "playertakedamage" | "take_damage" => {
                Some(Self::PlayerTakeDamage)
            }
            "player_killed" | "playerkilled" => Some(Self::PlayerKilled),
            "player_pre_think" | "playerprethink" => Some(Self::PlayerPreThink),
            "player_post_think" | "playerpostthink" => Some(Self::PlayerPostThink),
            "player_jump" | "playerjump" => Some(Self::PlayerJump),
            "players_should_collide" | "playersshouldcollide" | "should_collide" => {
                Some(Self::PlayersShouldCollide)
            }
            "can_buy_item" | "canbuyitem" => Some(Self::CanBuyItem),
            "can_have_player_item" | "canhaveplayeritem" => Some(Self::CanHavePlayerItem),
            "weapon_deploy" | "weapondeploy" => Some(Self::WeaponDeploy),
            "weapon_primary_attack" | "weaponprimaryattack" => Some(Self::WeaponPrimaryAttack),
            "weapon_reload" | "weaponreload" => Some(Self::WeaponReload),
            "player_say" | "playersay" => Some(Self::PlayerSay),
            "menu_select" | "menuselect" => Some(Self::MenuSelect),
            "freeze_time_end" | "freezetimeend" => Some(Self::FreezeTimeEnd),
            "round_end" | "roundend" => Some(Self::RoundEnd),
            "bomb_planting" | "bombplanting" => Some(Self::BombPlanting),
            "bomb_planted" | "bombplanted" => Some(Self::BombPlanted),
            "bomb_defusing" | "bombdefusing" => Some(Self::BombDefusing),
            "bomb_defused" | "bombdefused" => Some(Self::BombDefused),
            "bomb_exploded" | "bombexploded" => Some(Self::BombExploded),
            "hostage_touched" | "hostagetouched" => Some(Self::HostageTouched),
            "hostage_rescued" | "hostagerescued" => Some(Self::HostageRescued),
            "grenade_throw" | "grenadethrow" => Some(Self::GrenadeThrow),
            "eng_emit_sound" => Some(Self::EngEmitSound),
            "eng_send_usermsg" => Some(Self::EngSendUserMsg),
            "eng_client_print" => Some(Self::EngClientPrint),
            "eng_center_print" => Some(Self::EngCenterPrint),
            "eng_create_entity" => Some(Self::EngCreateEntity),
            "eng_remove_entity" => Some(Self::EngRemoveEntity),
            "eng_kick" => Some(Self::EngKick),
            "eng_change_level" => Some(Self::EngChangeLevel),
            _ => None,
        }
    }

    pub fn as_name(self) -> &'static str {
        match self {
            Self::ServerActivate => "server_activate",
            Self::ServerDeactivate => "server_deactivate",
            Self::SpawnEntity => "spawn_entity",
            Self::ClientConnect => "client_connect",
            Self::ClientPutInServer => "client_put_in_server",
            Self::ClientDisconnect => "client_disconnect",
            Self::ClientCommand => "client_command",
            Self::ClientUserinfoChanged => "client_userinfo_changed",
            Self::RunUsercmd => "run_usercmd",
            Self::PostUsercmd => "post_usercmd",
            Self::StartFrame => "start_frame",
            Self::Think => "think",
            Self::Touch => "touch",
            Self::Blocked => "blocked",
            Self::UpdateClientData => "update_client_data",
            Self::GetWeaponData => "get_weapon_data",
            Self::ServerCommand => "server_command",
            Self::AddToFullPack => "add_to_full_pack",
            Self::PlayerDeath => "player_death",
            Self::RoundStart => "round_start",
            Self::PlayerSpawn => "player_spawn",
            Self::PlayerTakeDamage => "player_take_damage",
            Self::PlayerKilled => "player_killed",
            Self::PlayerPreThink => "player_pre_think",
            Self::PlayerPostThink => "player_post_think",
            Self::PlayerJump => "player_jump",
            Self::PlayersShouldCollide => "players_should_collide",
            Self::CanBuyItem => "can_buy_item",
            Self::CanHavePlayerItem => "can_have_player_item",
            Self::WeaponDeploy => "weapon_deploy",
            Self::WeaponPrimaryAttack => "weapon_primary_attack",
            Self::WeaponReload => "weapon_reload",
            Self::PlayerSay => "player_say",
            Self::MenuSelect => "menu_select",
            Self::FreezeTimeEnd => "freeze_time_end",
            Self::RoundEnd => "round_end",
            Self::BombPlanting => "bomb_planting",
            Self::BombPlanted => "bomb_planted",
            Self::BombDefusing => "bomb_defusing",
            Self::BombDefused => "bomb_defused",
            Self::BombExploded => "bomb_exploded",
            Self::HostageTouched => "hostage_touched",
            Self::HostageRescued => "hostage_rescued",
            Self::GrenadeThrow => "grenade_throw",
            Self::EngEmitSound => "eng_emit_sound",
            Self::EngSendUserMsg => "eng_send_usermsg",
            Self::EngClientPrint => "eng_client_print",
            Self::EngCenterPrint => "eng_center_print",
            Self::EngCreateEntity => "eng_create_entity",
            Self::EngRemoveEntity => "eng_remove_entity",
            Self::EngKick => "eng_kick",
            Self::EngChangeLevel => "eng_change_level",
        }
    }
}

// ---------------------------------------------------------------------------
// ABI v2 Mutable Hook Context Structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerSpawnCtx {
    pub slot: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerTakeDamageCtx {
    pub victim: u8,
    pub inflictor: u16,
    pub attacker: Option<u8>,
    pub damage: f32,
    pub damage_bits: u32,
    pub hitgroup: u8,
    pub weapon_id: u8,
    pub team_attack: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerKilledCtx {
    pub victim: u8,
    pub killer: Option<u8>,
    pub weapon_name: String,
    pub weapon_id: u8,
    pub hitgroup: u8,
    pub headshot: bool,
    pub team_kill: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerPreThinkCtx {
    pub slot: u8,
    pub buttons: u16,
    pub oldbuttons: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerPostThinkCtx {
    pub slot: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerJumpCtx {
    pub slot: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayersShouldCollideCtx {
    pub slot_a: u8,
    pub slot_b: u8,
    pub should_collide: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanBuyItemCtx {
    pub slot: u8,
    /// `WeaponId` (`1..=30`, `99` shield) or equipment code (`101`=Kevlar, `102`=VestHelm, `106`=NVG, `107`=Defuser).
    pub item_id: u8,
    pub price: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanHavePlayerItemCtx {
    pub slot: u8,
    pub weapon_id: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeaponEventCtx {
    pub slot: u8,
    pub weapon_id: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerSayCtx {
    pub slot: u8,
    pub team_only: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuSelectCtx {
    pub slot: u8,
    pub menu_id: u32,
    pub item_index: usize,
    pub info: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundStartCtx {
    pub round_num: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundEndCtx {
    /// `0` = Draw, `1` = Terrorists, `2` = Counter-Terrorists.
    pub winner: u8,
    /// Reason code (1 = TargetBombed, 2 = VipEscaped, 3 = VipAssassinated, 4 = TerroristsEscaped,
    /// 5 = CtsPreventEscape, 6 = EscapingTerroristsNeutralized, 7 = BombDefused, 8 = CtsWin,
    /// 9 = TerroristsWin, 10 = RoundDraw, 11 = AllHostagesRescued, 12 = TargetSaved,
    /// 13 = HostagesNotRescued, 14 = TerroristsNotEscaped, 15 = VipNotEscaped, 16 = GameCommencing).
    pub reason: u8,
    pub bonus: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BombEventCtx {
    pub slot: Option<u8>,
    pub origin: [f32; 3],
    pub has_kit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostageEventCtx {
    pub slot: u8,
    pub ent_index: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrenadeThrowCtx {
    pub slot: u8,
    pub weapon_id: u8,
    pub ent_index: u16,
}

/// Immediate-mode HUD draw command emitted by a sandboxed client WASM mod (`0..=1` normalized
/// screen coordinates).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HudDrawCmd {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        rgba: [u8; 4],
    },
    Text {
        x: f32,
        y: f32,
        size: f32,
        rgba: [u8; 4],
        text: String,
    },
    ProgressBar {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fraction: f32,
        rgba: [u8; 4],
    },
    Panel {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        rgba: [u8; 4],
        title: String,
    },
}

/// Client mod descriptor advertised in `CsModManifest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientModEntry {
    pub id: String,
    pub version: String,
    pub sha256_hex: String,
    pub byte_len: u32,
    pub permissions_mask: u32,
}

/// Decoded `CsModManifest` payload (fits inside a 255-byte variable-length `UserMsg`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModManifestPacket {
    pub mod_index: u8,
    pub total_mods: u8,
    pub entry: ClientModEntry,
}

impl ModManifestPacket {
    pub fn encode(&self) -> Option<Vec<u8>> {
        let id = self.entry.id.as_bytes();
        let ver = self.entry.version.as_bytes();
        let sha = self.entry.sha256_hex.as_bytes();
        if id.len() > 48 || ver.len() > 24 || sha.len() != 64 {
            return None;
        }
        let mut out = Vec::with_capacity(2 + 1 + id.len() + 1 + ver.len() + 64 + 8);
        out.push(self.mod_index);
        out.push(self.total_mods);
        out.push(id.len() as u8);
        out.extend_from_slice(id);
        out.push(ver.len() as u8);
        out.extend_from_slice(ver);
        out.extend_from_slice(sha);
        out.extend_from_slice(&self.entry.byte_len.to_le_bytes());
        out.extend_from_slice(&self.entry.permissions_mask.to_le_bytes());
        (out.len() <= 255).then_some(out)
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 3 {
            return None;
        }
        let mod_index = bytes[0];
        let total_mods = bytes[1];
        let mut pos = 2usize;

        let id_len = usize::from(*bytes.get(pos)?);
        pos += 1;
        let id = std::str::from_utf8(bytes.get(pos..pos + id_len)?).ok()?.to_string();
        pos += id_len;

        let ver_len = usize::from(*bytes.get(pos)?);
        pos += 1;
        let version = std::str::from_utf8(bytes.get(pos..pos + ver_len)?).ok()?.to_string();
        pos += ver_len;

        let sha256_hex = std::str::from_utf8(bytes.get(pos..pos + 64)?).ok()?.to_string();
        pos += 64;

        let byte_len = u32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?);
        pos += 4;
        let permissions_mask = u32::from_le_bytes(bytes.get(pos..pos + 4)?.try_into().ok()?);

        Some(Self {
            mod_index,
            total_mods,
            entry: ClientModEntry {
                id,
                version,
                sha256_hex,
                byte_len,
                permissions_mask,
            },
        })
    }
}

/// Decoded `CsModChunk` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModChunkPacket {
    pub mod_index: u8,
    pub offset: u32,
    pub data: Vec<u8>,
}

impl ModChunkPacket {
    pub fn encode(&self) -> Option<Vec<u8>> {
        if self.data.len() > MOD_CHUNK_MAX_DATA {
            return None;
        }
        let mut out = Vec::with_capacity(5 + self.data.len());
        out.push(self.mod_index);
        out.extend_from_slice(&self.offset.to_le_bytes());
        out.extend_from_slice(&self.data);
        Some(out)
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 5 {
            return None;
        }
        let mod_index = bytes[0];
        let offset = u32::from_le_bytes(bytes[1..5].try_into().ok()?);
        let data = bytes[5..].to_vec();
        Some(Self {
            mod_index,
            offset,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_action_and_wire_codecs_roundtrip() {
        assert_eq!(HookAction::from_i32(0), HookAction::Continue);
        assert_eq!(HookAction::from_i32(2), HookAction::Override);
        assert_eq!(HookAction::from_i32(3), HookAction::Supercede);
        assert_eq!(HookId::from_name("start_frame"), Some(HookId::StartFrame));
        assert_eq!(HookId::from_name("eng_emit_sound"), Some(HookId::EngEmitSound));
        assert_eq!(
            HookId::from_name("player_take_damage"),
            Some(HookId::PlayerTakeDamage)
        );
        assert_eq!(
            HookId::from_name("players_should_collide"),
            Some(HookId::PlayersShouldCollide)
        );
        assert_eq!(
            HookId::from_u32(HookId::BombDefused as u32),
            Some(HookId::BombDefused)
        );
        assert_eq!(HookId::HostageRescued.as_name(), "hostage_rescued");

        let manifest = ModManifestPacket {
            mod_index: 0,
            total_mods: 2,
            entry: ClientModEntry {
                id: "custom_hud".to_string(),
                version: "1.0.0".to_string(),
                sha256_hex: "a".repeat(64),
                byte_len: 4096,
                permissions_mask: 0b101,
            },
        };
        let enc_m = manifest.encode().expect("fits usermsg limit");
        assert!(enc_m.len() <= 255);
        assert_eq!(ModManifestPacket::decode(&enc_m), Some(manifest));

        let chunk = ModChunkPacket {
            mod_index: 1,
            offset: 180,
            data: vec![0xCA, 0xFE, 0xBA, 0xBE],
        };
        let enc_c = chunk.encode().expect("chunk fits");
        assert_eq!(ModChunkPacket::decode(&enc_c), Some(chunk));
    }
}
