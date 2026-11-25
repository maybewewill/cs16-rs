//! Counter-Strike 1.6 `UserMsg` registry and typed codecs (`ReGameDLL_CS/dlls/client.cpp`).

use std::collections::HashMap;

use glam::Vec3;

use crate::bitbuf::{MsgReader, MsgWriter, ProtoError};

/// First dynamic user-message opcode (`svc_startofusermessages` in `ReHLDS/engine/net.h`).
pub const USERMSG_BASE_ID: u8 = 64;

/// Registration entry sent by `svc_newusermsg` (opcode 39).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMsgDesc {
    pub id: u8,
    /// Fixed byte size (`>= 0`), or `-1` (`255u8` on wire) for variable-length messages
    /// prefixed with a 1-byte length.
    pub size: i8,
    pub name: String,
}

/// Dynamic table mapping opcode `>= 64` <-> CS 1.6 `UserMsg` name & size.
#[derive(Debug, Default, Clone)]
pub struct UserMsgRegistry {
    by_id: HashMap<u8, UserMsgDesc>,
    by_name: HashMap<String, u8>,
    order: Vec<u8>,
}

impl UserMsgRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, id: u8, size: i8, name: impl Into<String>) {
        let name = name.into();
        if !self.by_id.contains_key(&id) {
            self.order.push(id);
        }
        self.by_name.insert(name.clone(), id);
        self.by_id.insert(id, UserMsgDesc { id, size, name });
    }

    /// Registers `name` at the lowest free slot in `USERMSG_BASE_ID..=255`, or returns the
    /// existing id when `name` is already registered.
    pub fn register_next(&mut self, size: i8, name: impl Into<String>) -> Option<u8> {
        let name = name.into();
        if let Some(id) = self.by_name.get(&name).copied() {
            return Some(id);
        }
        let id = (USERMSG_BASE_ID..=u8::MAX).find(|candidate| !self.by_id.contains_key(candidate))?;
        self.register(id, size, name);
        Some(id)
    }

    pub fn get_by_id(&self, id: u8) -> Option<&UserMsgDesc> {
        self.by_id.get(&id)
    }

    pub fn id_for_name(&self, name: &str) -> Option<u8> {
        self.by_name.get(name).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = &UserMsgDesc> {
        self.order.iter().filter_map(|id| self.by_id.get(id))
    }

    /// Standard Counter-Strike 1.6 `UserMsg` table matching `ReGameDLL_CS/dlls/client.cpp:148-231`.
    pub fn cs16_default() -> Self {
        const DEFS: &[(&str, i8)] = &[
            ("CurWeapon", 3),
            ("Geiger", 1),
            ("Flashlight", 2),
            ("FlashBat", 1),
            ("Health", 1),
            ("Damage", 12),
            ("Battery", 2),
            ("Train", 1),
            ("HudTextPro", -1),
            ("HudText", -1),
            ("SayText", -1),
            ("TextMsg", -1),
            ("WeaponList", -1),
            ("ResetHUD", 0),
            ("InitHUD", 0),
            ("ViewMode", 0),
            ("GameTitle", 1),
            ("DeathMsg", -1),
            ("ScoreAttrib", 2),
            ("ScoreInfo", 9),
            ("TeamInfo", -1),
            ("TeamScore", -1),
            ("GameMode", 1),
            ("MOTD", -1),
            ("ServerName", -1),
            ("AmmoPickup", 2),
            ("WeapPickup", 1),
            ("ItemPickup", -1),
            ("HideWeapon", 1),
            ("SetFOV", 1),
            ("ShowMenu", -1),
            ("ScreenShake", 6),
            ("ScreenFade", 10),
            ("AmmoX", 2),
            ("SendAudio", -1),
            ("RoundTime", 2),
            ("Money", 5),
            ("ArmorType", 1),
            ("BlinkAcct", 1),
            ("StatusValue", -1),
            ("StatusText", -1),
            ("StatusIcon", -1),
            ("BarTime", 2),
            ("ReloadSound", 2),
            ("Crosshair", 1),
            ("NVGToggle", 1),
            ("Radar", 7),
            ("Spectator", 2),
            ("VGUIMenu", -1),
            ("AllowSpec", 1),
            ("BombDrop", 7),
            ("BombPickup", 0),
            ("ClCorpse", -1),
            ("HostagePos", 8),
            ("HostageK", 1),
            ("HLTV", 2),
            ("SpecHealth", 1),
            ("ForceCam", 3),
            ("ADStop", 0),
            ("ReceiveW", 1),
            ("ShadowIdx", 4),
            ("TaskTime", 4),
            ("Scenario", -1),
            ("BotVoice", 2),
            ("BuyClose", 0),
            ("SpecHealth2", 2),
            ("BarTime2", 4),
            ("ItemStatus", 1),
            ("Location", -1),
            ("Brass", -1),
            ("Fog", 7),
            ("ShowTimer", 0),
            ("HudTextArgs", -1),
        ];
        let mut reg = Self::new();
        for (idx, &(name, size)) in DEFS.iter().enumerate() {
            reg.register(USERMSG_BASE_ID + idx as u8, size, name);
        }
        reg
    }
}

/// Decoded Counter-Strike 1.6 gameplay / HUD `UserMsg`.
#[derive(Debug, Clone, PartialEq)]
pub enum CsUserMsg {
    CurWeapon {
        is_active: bool,
        weapon_id: u8,
        clip_ammo: i8,
    },
    Health(u8),
    Battery(i16),
    ArmorType(u8),
    Damage {
        armor_damage: u8,
        health_damage: u8,
        damage_bits: u32,
        origin: Vec3,
    },
    Money {
        amount: i32,
        flash: bool,
    },
    RoundTime(u16),
    /// `BlinkAcct`: the account on the HUD blinks this many times.
    BlinkAcct(u8),
    AmmoX {
        ammo_id: u8,
        amount: u8,
    },
    SetFov(u8),
    DeathMsg {
        killer_id: u8,
        victim_id: u8,
        headshot: bool,
        weapon: String,
    },
    ScoreInfo {
        player_id: u8,
        frags: i16,
        deaths: i16,
        class_id: i16,
        team_id: i16,
    },
    ScoreAttrib {
        player_id: u8,
        flags: u8,
    },
    TeamInfo {
        player_id: u8,
        team_name: String,
    },
    TeamScore {
        team_name: String,
        score: i16,
    },
    SayText {
        sender_id: u8,
        text: String,
        params: Vec<String>,
    },
    TextMsg {
        dest: u8,
        message: String,
        params: Vec<String>,
    },
    StatusIcon {
        status: u8,
        sprite_name: String,
        color: Option<[u8; 3]>,
    },
    VguiMenu {
        menu_id: u8,
        keys_bitmask: u16,
        time: i8,
        multipart: bool,
        name: String,
    },
    ShowMenu {
        keys_bitmask: u16,
        display_time: i8,
        multipart: bool,
        text: String,
    },
    ScreenFade {
        duration: u16,
        hold_time: u16,
        flags: u16,
        color: [u8; 4],
    },
    ScreenShake {
        amplitude: u16,
        duration: u16,
        frequency: u16,
    },
    NvgToggle(bool),
    /// `gmsgStatusText`: the format string of a status bar line (`%c1`, `%p2`, `%h`, `%i3`: the
    /// client fills them from `StatusValue`).
    StatusText {
        line: u8,
        text: String,
    },
    /// `gmsgStatusValue`: the value `%i`/`%p`/`%c` of the status bar format number `index`
    /// stands for.
    StatusValue {
        index: u8,
        value: i16,
    },
    Radar {
        player_id: u8,
        origin: Vec3,
    },
    BombDrop {
        origin: Vec3,
        planted: bool,
    },
    BombPickup,
    HostagePos {
        flag: u8,
        hostage_id: u8,
        origin: Vec3,
    },
    HostageK(u8),
    BarTime(u16),
    /// `gmsgBarTime2`: progress bar (`duration` seconds) starting at `start_percent` (0..100).
    BarTime2 {
        duration: u16,
        start_percent: u16,
    },
    /// `gmsgScenarioIcon`: objective status icon (bomb ticking, hostages). When `active` is false,
    /// only the active byte (0) is sent on the wire; when `alpha != 0`, `flash` `(rate, delay)`
    /// follows (`ggrenade.cpp`, `player.cpp`).
    Scenario {
        active: bool,
        sprite: String,
        alpha: u8,
        flash: Option<(i16, i16)>,
    },
    SendAudio {
        sender_id: u8,
        audio_code: String,
        pitch: i16,
    },
    ServerName(String),
    Motd {
        finished: bool,
        text: String,
    },
    HideWeapon(u8),
    ItemStatus(u8),
    /// `gmsgItemPickup`: the classname of an item the player picked up (`item_kevlar`), which
    /// the HUD shows a pickup icon for.
    ItemPickup(String),
    ResetHud,
    InitHud,
    BuyClose,
    ShowTimer,
    /// `gmsgWeaponList` (`WriteSigonMessages`): one weapon of the HUD's weapon table. The ammo
    /// types are the ammo registry's indices (`0xFF`, a byte of -1, for none).
    WeaponList {
        name: String,
        ammo1_type: u8,
        ammo1_max: u8,
        ammo2_type: u8,
        ammo2_max: u8,
        slot: u8,
        position: u8,
        weapon_id: u8,
        flags: u8,
    },
    /// `gmsgAmmoPickup` (`CBasePlayer::GiveAmmo`): the ammo index and how much was added.
    AmmoPickup {
        ammo_id: u8,
        amount: u8,
    },
    /// `gmsgWeapPickup` (`CBasePlayerItem::AddToPlayer`): the id of the weapon picked up.
    WeapPickup(u8),
    /// `gmsgReloadSound` (`CBasePlayerWeapon::ReloadSound`): the volume byte and whether the
    /// sound is a pistol's (1) or a shotgun's (0).
    ReloadSound {
        volume: u8,
        kind: u8,
    },
    /// `gmsgCrosshair`.
    Crosshair(u8),
    /// `gmsgSendCorpse` (registered as `ClCorpse`): a dead player's body, left to the client to
    /// draw. `origin` is the position times 128 as longs, `angles` are coords, `time_diff` is
    /// `(animtime - time) * 100` (ReGameDLL `CBasePlayer::SpawnClientSideCorpse`).
    ClCorpse {
        model: String,
        origin: [i32; 3],
        angles: Vec3,
        time_diff: i32,
        sequence: u8,
        body: u8,
        team: u8,
        player_id: u8,
    },
    Fog {
        color: [u8; 3],
        density: f32,
    },
    Flashlight {
        on: bool,
        battery: u8,
    },
    FlashBat(u8),
    ReceiveW(u8),
    Brass(Vec<u8>),
    /// `gmsgHudText` / `gmsgHudTextPro`: localized `titles.txt` message name or raw text.
    HudText {
        text: String,
        style: u8,
    },
    /// `gmsgHudTextArgs`: localized hint/tutor message with format arguments.
    HudTextArgs {
        text: String,
        style: u8,
        args: Vec<String>,
    },
    AllowSpec(bool),
    SpecHealth(u8),
    SpecHealth2 {
        health: u8,
        target: u8,
    },
    ViewMode,
    GameTitle(u8),
    GameMode(u8),
    Spectator {
        client: u8,
        is_spectator: bool,
    },
    Hltv {
        client: u8,
        flags: u8,
    },
    ForceCam {
        forcecam: u8,
        forcechasecam: u8,
        fadetoblack: u8,
    },
    ADStop,
    ShadowIdx(i32),
    TaskTime {
        time: i16,
        active: u8,
        fade_time: u8,
    },
    BotVoice {
        active: bool,
        player: u8,
    },
    Location {
        player: u8,
        location: String,
    },
    /// `gmsgHealthInfo` (`BUILD_LATEST`, 5 bytes): observer/replay HP update for `player_id` (1-based).
    HealthInfo {
        player_id: u8,
        health: i32,
    },
    /// `gmsgAccount` (`BUILD_LATEST`, 5 bytes): observer/replay money update for `player_id` (1-based).
    Account {
        player_id: u8,
        amount: i32,
    },
    /// `gmsgReqState` (engine voice mask request, 0 bytes).
    ReqState,
    Unknown {
        name: String,
        payload: Vec<u8>,
    },
}

/// Decode a raw `UserMsg` payload using its registered CS 1.6 name.
pub fn decode_cs_usermsg(name: &str, payload: &[u8]) -> Result<CsUserMsg, ProtoError> {
    let mut r = MsgReader::new(payload);
    let msg = match name {
        "CurWeapon" => CsUserMsg::CurWeapon {
            is_active: r.read_u8()? != 0,
            weapon_id: r.read_u8()?,
            clip_ammo: r.read_i8()?,
        },
        "Health" => CsUserMsg::Health(r.read_u8()?),
        "Battery" => CsUserMsg::Battery(r.read_i16()?),
        "ArmorType" => CsUserMsg::ArmorType(r.read_u8()?),
        "Damage" => CsUserMsg::Damage {
            armor_damage: r.read_u8()?,
            health_damage: r.read_u8()?,
            damage_bits: r.read_u32()?,
            origin: Vec3::new(r.read_coord()?, r.read_coord()?, r.read_coord()?),
        },
        "Money" => CsUserMsg::Money {
            amount: r.read_i32()?,
            flash: r.read_u8()? != 0,
        },
        "RoundTime" => CsUserMsg::RoundTime(r.read_u16()?),
        "BlinkAcct" => CsUserMsg::BlinkAcct(r.read_u8()?),
        "AmmoX" => CsUserMsg::AmmoX {
            ammo_id: r.read_u8()?,
            amount: r.read_u8()?,
        },
        "SetFOV" => CsUserMsg::SetFov(r.read_u8()?),
        "DeathMsg" => CsUserMsg::DeathMsg {
            killer_id: r.read_u8()?,
            victim_id: r.read_u8()?,
            headshot: r.read_u8()? != 0,
            weapon: r.read_str()?,
        },
        "ScoreInfo" => CsUserMsg::ScoreInfo {
            player_id: r.read_u8()?,
            frags: r.read_i16()?,
            deaths: r.read_i16()?,
            class_id: r.read_i16()?,
            team_id: r.read_i16()?,
        },
        "ScoreAttrib" => CsUserMsg::ScoreAttrib {
            player_id: r.read_u8()?,
            flags: r.read_u8()?,
        },
        "TeamInfo" => CsUserMsg::TeamInfo {
            player_id: r.read_u8()?,
            team_name: r.read_str()?,
        },
        "TeamScore" => CsUserMsg::TeamScore {
            team_name: r.read_str()?,
            score: r.read_i16()?,
        },
        "SayText" => {
            let sender_id = r.read_u8()?;
            let text = r.read_str()?;
            let mut params = Vec::new();
            while r.remaining() > 0 {
                params.push(r.read_str()?);
            }
            CsUserMsg::SayText {
                sender_id,
                text,
                params,
            }
        }
        "TextMsg" => {
            let dest = r.read_u8()?;
            let message = r.read_str()?;
            let mut params = Vec::new();
            while r.remaining() > 0 {
                params.push(r.read_str()?);
            }
            CsUserMsg::TextMsg {
                dest,
                message,
                params,
            }
        }
        "StatusIcon" => {
            let status = r.read_u8()?;
            let sprite_name = r.read_str()?;
            let color = if status != 0 && r.remaining() >= 3 {
                Some([r.read_u8()?, r.read_u8()?, r.read_u8()?])
            } else {
                None
            };
            CsUserMsg::StatusIcon {
                status,
                sprite_name,
                color,
            }
        }
        "VGUIMenu" => CsUserMsg::VguiMenu {
            menu_id: r.read_u8()?,
            keys_bitmask: r.read_u16()?,
            time: r.read_i8()?,
            multipart: r.read_u8()? != 0,
            name: if r.remaining() > 0 {
                r.read_str()?
            } else {
                String::new()
            },
        },
        "ShowMenu" => CsUserMsg::ShowMenu {
            keys_bitmask: r.read_u16()?,
            display_time: r.read_i8()?,
            multipart: r.read_u8()? != 0,
            text: r.read_str()?,
        },
        "ScreenFade" => CsUserMsg::ScreenFade {
            duration: r.read_u16()?,
            hold_time: r.read_u16()?,
            flags: r.read_u16()?,
            color: [r.read_u8()?, r.read_u8()?, r.read_u8()?, r.read_u8()?],
        },
        "ScreenShake" => CsUserMsg::ScreenShake {
            amplitude: r.read_u16()?,
            duration: r.read_u16()?,
            frequency: r.read_u16()?,
        },
        "NVGToggle" => CsUserMsg::NvgToggle(r.read_u8()? != 0),
        "StatusText" => CsUserMsg::StatusText {
            line: r.read_u8()?,
            text: r.read_str()?,
        },
        "StatusValue" => CsUserMsg::StatusValue {
            index: r.read_u8()?,
            value: r.read_i16()?,
        },
        "Radar" => CsUserMsg::Radar {
            player_id: r.read_u8()?,
            origin: Vec3::new(r.read_coord()?, r.read_coord()?, r.read_coord()?),
        },
        "BombDrop" => CsUserMsg::BombDrop {
            origin: Vec3::new(r.read_coord()?, r.read_coord()?, r.read_coord()?),
            planted: r.read_u8()? != 0,
        },
        "BombPickup" => CsUserMsg::BombPickup,
        "HostagePos" => CsUserMsg::HostagePos {
            flag: r.read_u8()?,
            hostage_id: r.read_u8()?,
            origin: Vec3::new(r.read_coord()?, r.read_coord()?, r.read_coord()?),
        },
        "HostageK" => CsUserMsg::HostageK(r.read_u8()?),
        "BarTime" => CsUserMsg::BarTime(r.read_u16()?),
        "BarTime2" => CsUserMsg::BarTime2 {
            duration: r.read_u16()?,
            start_percent: r.read_u16()?,
        },
        "Scenario" => {
            let active = r.read_u8()? != 0;
            if !active {
                CsUserMsg::Scenario {
                    active: false,
                    sprite: String::new(),
                    alpha: 0,
                    flash: None,
                }
            } else {
                let sprite = r.read_str()?;
                let alpha = r.read_u8()?;
                let flash = if alpha != 0 {
                    Some((r.read_i16()?, r.read_i16()?))
                } else {
                    None
                };
                CsUserMsg::Scenario {
                    active: true,
                    sprite,
                    alpha,
                    flash,
                }
            }
        }
        "SendAudio" => CsUserMsg::SendAudio {
            sender_id: r.read_u8()?,
            audio_code: r.read_str()?,
            pitch: r.read_i16()?,
        },
        "ServerName" => CsUserMsg::ServerName(r.read_str()?),
        "MOTD" => CsUserMsg::Motd {
            finished: r.read_u8()? != 0,
            text: r.read_str()?,
        },
        "HideWeapon" => CsUserMsg::HideWeapon(r.read_u8()?),
        "ItemStatus" => CsUserMsg::ItemStatus(r.read_u8()?),
        "ItemPickup" => CsUserMsg::ItemPickup(r.read_str()?),
        "ResetHUD" => CsUserMsg::ResetHud,
        "InitHUD" => CsUserMsg::InitHud,
        "BuyClose" => CsUserMsg::BuyClose,
        "ShowTimer" => CsUserMsg::ShowTimer,
        "WeaponList" => CsUserMsg::WeaponList {
            name: r.read_str()?,
            ammo1_type: r.read_u8()?,
            ammo1_max: r.read_u8()?,
            ammo2_type: r.read_u8()?,
            ammo2_max: r.read_u8()?,
            slot: r.read_u8()?,
            position: r.read_u8()?,
            weapon_id: r.read_u8()?,
            flags: r.read_u8()?,
        },
        "AmmoPickup" => CsUserMsg::AmmoPickup {
            ammo_id: r.read_u8()?,
            amount: r.read_u8()?,
        },
        "WeapPickup" => CsUserMsg::WeapPickup(r.read_u8()?),
        "ReloadSound" => CsUserMsg::ReloadSound {
            volume: r.read_u8()?,
            kind: r.read_u8()?,
        },
        "Crosshair" => CsUserMsg::Crosshair(r.read_u8()?),
        "ClCorpse" => CsUserMsg::ClCorpse {
            model: r.read_str()?,
            origin: [r.read_i32()?, r.read_i32()?, r.read_i32()?],
            angles: Vec3::new(r.read_coord()?, r.read_coord()?, r.read_coord()?),
            time_diff: r.read_i32()?,
            sequence: r.read_u8()?,
            body: r.read_u8()?,
            team: r.read_u8()?,
            player_id: r.read_u8()?,
        },
        "Fog" => {
            let color = if r.remaining() >= 3 {
                [r.read_u8()?, r.read_u8()?, r.read_u8()?]
            } else {
                [0, 0, 0]
            };
            let density = if r.remaining() >= 4 {
                let bytes = r.read_bytes(4)?;
                let d = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                if d.is_finite() { d } else { 0.0 }
            } else {
                0.0
            };
            CsUserMsg::Fog { color, density }
        }
        "Flashlight" => CsUserMsg::Flashlight {
            on: r.read_u8()? != 0,
            battery: r.read_u8()?,
        },
        "FlashBat" => CsUserMsg::FlashBat(r.read_u8()?),
        "ReceiveW" => CsUserMsg::ReceiveW(r.read_u8()?),
        "Brass" => CsUserMsg::Brass(r.remaining_slice().to_vec()),
        "HudText" | "HudTextPro" => CsUserMsg::HudText {
            text: r.read_str()?,
            style: if r.remaining() > 0 { r.read_u8()? } else { 0 },
        },
        "HudTextArgs" => {
            let text = r.read_str()?;
            let style = if r.remaining() > 0 { r.read_u8()? } else { 0 };
            let count = if r.remaining() > 0 {
                usize::from(r.read_u8()?).min(8)
            } else {
                0
            };
            let mut args = Vec::with_capacity(count);
            for _ in 0..count {
                if r.remaining() == 0 {
                    break;
                }
                args.push(r.read_str()?);
            }
            CsUserMsg::HudTextArgs { text, style, args }
        }
        "AllowSpec" => CsUserMsg::AllowSpec(r.read_u8()? != 0),
        "SpecHealth" => CsUserMsg::SpecHealth(r.read_u8()?),
        "SpecHealth2" => CsUserMsg::SpecHealth2 {
            health: r.read_u8()?,
            target: r.read_u8()?,
        },
        "ViewMode" => CsUserMsg::ViewMode,
        "GameTitle" => CsUserMsg::GameTitle(r.read_u8()?),
        "GameMode" => CsUserMsg::GameMode(r.read_u8()?),
        "Spectator" => CsUserMsg::Spectator {
            client: r.read_u8()?,
            is_spectator: r.read_u8()? != 0,
        },
        "HLTV" => CsUserMsg::Hltv {
            client: r.read_u8()?,
            flags: r.read_u8()?,
        },
        "ForceCam" => CsUserMsg::ForceCam {
            forcecam: r.read_u8()?,
            forcechasecam: r.read_u8()?,
            fadetoblack: r.read_u8()?,
        },
        "ADStop" => CsUserMsg::ADStop,
        "ShadowIdx" => CsUserMsg::ShadowIdx(r.read_i32()?),
        "TaskTime" => CsUserMsg::TaskTime {
            time: r.read_i16()?,
            active: r.read_u8()?,
            fade_time: r.read_u8()?,
        },
        "BotVoice" => CsUserMsg::BotVoice {
            active: r.read_u8()? != 0,
            player: r.read_u8()?,
        },
        "Location" => CsUserMsg::Location {
            player: r.read_u8()?,
            location: r.read_str()?,
        },
        "HealthInfo" => CsUserMsg::HealthInfo {
            player_id: r.read_u8()?,
            health: r.read_i32()?,
        },
        "Account" => CsUserMsg::Account {
            player_id: r.read_u8()?,
            amount: r.read_i32()?,
        },
        "ReqState" => CsUserMsg::ReqState,
        _ => CsUserMsg::Unknown {
            name: name.to_string(),
            payload: payload.to_vec(),
        },
    };
    Ok(msg)
}

/// Encode a `CsUserMsg` into `(name, raw_payload)`.
pub fn encode_cs_usermsg(msg: &CsUserMsg) -> (&'static str, Vec<u8>) {
    let mut w = MsgWriter::with_capacity(64);
    let name = match msg {
        CsUserMsg::CurWeapon {
            is_active,
            weapon_id,
            clip_ammo,
        } => {
            w.write_u8(u8::from(*is_active));
            w.write_u8(*weapon_id);
            w.write_i8(*clip_ammo);
            "CurWeapon"
        }
        CsUserMsg::Health(hp) => {
            w.write_u8(*hp);
            "Health"
        }
        CsUserMsg::Battery(ap) => {
            w.write_i16(*ap);
            "Battery"
        }
        CsUserMsg::ArmorType(t) => {
            w.write_u8(*t);
            "ArmorType"
        }
        CsUserMsg::Damage {
            armor_damage,
            health_damage,
            damage_bits,
            origin,
        } => {
            w.write_u8(*armor_damage);
            w.write_u8(*health_damage);
            w.write_u32(*damage_bits);
            w.write_coord(origin.x);
            w.write_coord(origin.y);
            w.write_coord(origin.z);
            "Damage"
        }
        CsUserMsg::Money { amount, flash } => {
            w.write_i32(*amount);
            w.write_u8(u8::from(*flash));
            "Money"
        }
        CsUserMsg::RoundTime(secs) => {
            w.write_u16(*secs);
            "RoundTime"
        }
        CsUserMsg::BlinkAcct(times) => {
            w.write_u8(*times);
            "BlinkAcct"
        }
        CsUserMsg::AmmoX { ammo_id, amount } => {
            w.write_u8(*ammo_id);
            w.write_u8(*amount);
            "AmmoX"
        }
        CsUserMsg::SetFov(fov) => {
            w.write_u8(*fov);
            "SetFOV"
        }
        CsUserMsg::DeathMsg {
            killer_id,
            victim_id,
            headshot,
            weapon,
        } => {
            w.write_u8(*killer_id);
            w.write_u8(*victim_id);
            w.write_u8(u8::from(*headshot));
            w.write_str(weapon);
            "DeathMsg"
        }
        CsUserMsg::ScoreInfo {
            player_id,
            frags,
            deaths,
            class_id,
            team_id,
        } => {
            w.write_u8(*player_id);
            w.write_i16(*frags);
            w.write_i16(*deaths);
            w.write_i16(*class_id);
            w.write_i16(*team_id);
            "ScoreInfo"
        }
        CsUserMsg::ScoreAttrib { player_id, flags } => {
            w.write_u8(*player_id);
            w.write_u8(*flags);
            "ScoreAttrib"
        }
        CsUserMsg::TeamInfo {
            player_id,
            team_name,
        } => {
            w.write_u8(*player_id);
            w.write_str(team_name);
            "TeamInfo"
        }
        CsUserMsg::TeamScore { team_name, score } => {
            w.write_str(team_name);
            w.write_i16(*score);
            "TeamScore"
        }
        CsUserMsg::SayText {
            sender_id,
            text,
            params,
        } => {
            w.write_u8(*sender_id);
            w.write_str(text);
            for p in params {
                w.write_str(p);
            }
            "SayText"
        }
        CsUserMsg::TextMsg {
            dest,
            message,
            params,
        } => {
            w.write_u8(*dest);
            w.write_str(message);
            for p in params {
                w.write_str(p);
            }
            "TextMsg"
        }
        CsUserMsg::StatusIcon {
            status,
            sprite_name,
            color,
        } => {
            w.write_u8(*status);
            w.write_str(sprite_name);
            if *status != 0
                && let Some([r, g, b]) = color
            {
                w.write_u8(*r);
                w.write_u8(*g);
                w.write_u8(*b);
            }
            "StatusIcon"
        }
        CsUserMsg::VguiMenu {
            menu_id,
            keys_bitmask,
            time,
            multipart,
            name,
        } => {
            w.write_u8(*menu_id);
            w.write_u16(*keys_bitmask);
            w.write_i8(*time);
            w.write_u8(u8::from(*multipart));
            if !name.is_empty() {
                w.write_str(name);
            }
            "VGUIMenu"
        }
        CsUserMsg::ShowMenu {
            keys_bitmask,
            display_time,
            multipart,
            text,
        } => {
            w.write_u16(*keys_bitmask);
            w.write_i8(*display_time);
            w.write_u8(u8::from(*multipart));
            w.write_str(text);
            "ShowMenu"
        }
        CsUserMsg::ScreenFade {
            duration,
            hold_time,
            flags,
            color,
        } => {
            w.write_u16(*duration);
            w.write_u16(*hold_time);
            w.write_u16(*flags);
            w.write_bytes(color);
            "ScreenFade"
        }
        CsUserMsg::ScreenShake {
            amplitude,
            duration,
            frequency,
        } => {
            w.write_u16(*amplitude);
            w.write_u16(*duration);
            w.write_u16(*frequency);
            "ScreenShake"
        }
        CsUserMsg::NvgToggle(on) => {
            w.write_u8(u8::from(*on));
            "NVGToggle"
        }
        CsUserMsg::StatusText { line, text } => {
            w.write_u8(*line);
            w.write_str(text);
            "StatusText"
        }
        CsUserMsg::StatusValue { index, value } => {
            w.write_u8(*index);
            w.write_i16(*value);
            "StatusValue"
        }
        CsUserMsg::Radar { player_id, origin } => {
            w.write_u8(*player_id);
            w.write_coord(origin.x);
            w.write_coord(origin.y);
            w.write_coord(origin.z);
            "Radar"
        }
        CsUserMsg::BombDrop { origin, planted } => {
            w.write_coord(origin.x);
            w.write_coord(origin.y);
            w.write_coord(origin.z);
            w.write_u8(u8::from(*planted));
            "BombDrop"
        }
        CsUserMsg::BombPickup => "BombPickup",
        CsUserMsg::HostagePos {
            flag,
            hostage_id,
            origin,
        } => {
            w.write_u8(*flag);
            w.write_u8(*hostage_id);
            w.write_coord(origin.x);
            w.write_coord(origin.y);
            w.write_coord(origin.z);
            "HostagePos"
        }
        CsUserMsg::HostageK(id) => {
            w.write_u8(*id);
            "HostageK"
        }
        CsUserMsg::BarTime(secs) => {
            w.write_u16(*secs);
            "BarTime"
        }
        CsUserMsg::BarTime2 {
            duration,
            start_percent,
        } => {
            w.write_u16(*duration);
            w.write_u16(*start_percent);
            "BarTime2"
        }
        CsUserMsg::Scenario {
            active,
            sprite,
            alpha,
            flash,
        } => {
            w.write_u8(u8::from(*active));
            if *active {
                w.write_str(sprite);
                w.write_u8(*alpha);
                if *alpha != 0 {
                    let (rate, delay) = flash.unwrap_or((0, 0));
                    w.write_i16(rate);
                    w.write_i16(delay);
                }
            }
            "Scenario"
        }
        CsUserMsg::SendAudio {
            sender_id,
            audio_code,
            pitch,
        } => {
            w.write_u8(*sender_id);
            w.write_str(audio_code);
            w.write_i16(*pitch);
            "SendAudio"
        }
        CsUserMsg::ServerName(s) => {
            w.write_str(s);
            "ServerName"
        }
        CsUserMsg::Motd { finished, text } => {
            w.write_u8(u8::from(*finished));
            w.write_str(text);
            "MOTD"
        }
        CsUserMsg::HideWeapon(flags) => {
            w.write_u8(*flags);
            "HideWeapon"
        }
        CsUserMsg::ItemStatus(flags) => {
            w.write_u8(*flags);
            "ItemStatus"
        }
        CsUserMsg::ItemPickup(name) => {
            w.write_str(name);
            "ItemPickup"
        }
        CsUserMsg::ResetHud => "ResetHUD",
        CsUserMsg::InitHud => "InitHUD",
        CsUserMsg::BuyClose => "BuyClose",
        CsUserMsg::ShowTimer => "ShowTimer",
        CsUserMsg::WeaponList {
            name,
            ammo1_type,
            ammo1_max,
            ammo2_type,
            ammo2_max,
            slot,
            position,
            weapon_id,
            flags,
        } => {
            w.write_str(name);
            for b in [
                ammo1_type, ammo1_max, ammo2_type, ammo2_max, slot, position, weapon_id, flags,
            ] {
                w.write_u8(*b);
            }
            "WeaponList"
        }
        CsUserMsg::AmmoPickup { ammo_id, amount } => {
            w.write_u8(*ammo_id);
            w.write_u8(*amount);
            "AmmoPickup"
        }
        CsUserMsg::WeapPickup(id) => {
            w.write_u8(*id);
            "WeapPickup"
        }
        CsUserMsg::ReloadSound { volume, kind } => {
            w.write_u8(*volume);
            w.write_u8(*kind);
            "ReloadSound"
        }
        CsUserMsg::Crosshair(on) => {
            w.write_u8(*on);
            "Crosshair"
        }
        CsUserMsg::ClCorpse {
            model,
            origin,
            angles,
            time_diff,
            sequence,
            body,
            team,
            player_id,
        } => {
            w.write_str(model);
            for v in origin {
                w.write_i32(*v);
            }
            w.write_coord(angles.x);
            w.write_coord(angles.y);
            w.write_coord(angles.z);
            w.write_i32(*time_diff);
            w.write_u8(*sequence);
            w.write_u8(*body);
            w.write_u8(*team);
            w.write_u8(*player_id);
            "ClCorpse"
        }
        CsUserMsg::Fog { color, density } => {
            w.write_bytes(color);
            w.write_bytes(&density.to_le_bytes());
            "Fog"
        }
        CsUserMsg::Flashlight { on, battery } => {
            w.write_u8(u8::from(*on));
            w.write_u8(*battery);
            "Flashlight"
        }
        CsUserMsg::FlashBat(battery) => {
            w.write_u8(*battery);
            "FlashBat"
        }
        CsUserMsg::ReceiveW(weather) => {
            w.write_u8(*weather);
            "ReceiveW"
        }
        CsUserMsg::Brass(raw) => {
            w.write_bytes(raw);
            "Brass"
        }
        CsUserMsg::HudText { text, style } => {
            w.write_str(text);
            w.write_u8(*style);
            "HudText"
        }
        CsUserMsg::HudTextArgs { text, style, args } => {
            w.write_str(text);
            w.write_u8(*style);
            w.write_u8(args.len().min(8) as u8);
            for a in args.iter().take(8) {
                w.write_str(a);
            }
            "HudTextArgs"
        }
        CsUserMsg::AllowSpec(allow) => {
            w.write_u8(u8::from(*allow));
            "AllowSpec"
        }
        CsUserMsg::SpecHealth(hp) => {
            w.write_u8(*hp);
            "SpecHealth"
        }
        CsUserMsg::SpecHealth2 { health, target } => {
            w.write_u8(*health);
            w.write_u8(*target);
            "SpecHealth2"
        }
        CsUserMsg::ViewMode => "ViewMode",
        CsUserMsg::GameTitle(v) => {
            w.write_u8(*v);
            "GameTitle"
        }
        CsUserMsg::GameMode(v) => {
            w.write_u8(*v);
            "GameMode"
        }
        CsUserMsg::Spectator {
            client,
            is_spectator,
        } => {
            w.write_u8(*client);
            w.write_u8(u8::from(*is_spectator));
            "Spectator"
        }
        CsUserMsg::Hltv { client, flags } => {
            w.write_u8(*client);
            w.write_u8(*flags);
            "HLTV"
        }
        CsUserMsg::ForceCam {
            forcecam,
            forcechasecam,
            fadetoblack,
        } => {
            w.write_u8(*forcecam);
            w.write_u8(*forcechasecam);
            w.write_u8(*fadetoblack);
            "ForceCam"
        }
        CsUserMsg::ADStop => "ADStop",
        CsUserMsg::ShadowIdx(idx) => {
            w.write_i32(*idx);
            "ShadowIdx"
        }
        CsUserMsg::TaskTime {
            time,
            active,
            fade_time,
        } => {
            w.write_i16(*time);
            w.write_u8(*active);
            w.write_u8(*fade_time);
            "TaskTime"
        }
        CsUserMsg::BotVoice { active, player } => {
            w.write_u8(u8::from(*active));
            w.write_u8(*player);
            "BotVoice"
        }
        CsUserMsg::Location { player, location } => {
            w.write_u8(*player);
            w.write_str(location);
            "Location"
        }
        CsUserMsg::HealthInfo { player_id, health } => {
            w.write_u8(*player_id);
            w.write_i32(*health);
            "HealthInfo"
        }
        CsUserMsg::Account { player_id, amount } => {
            w.write_u8(*player_id);
            w.write_i32(*amount);
            "Account"
        }
        CsUserMsg::ReqState => "ReqState",
        CsUserMsg::Unknown { payload, .. } => {
            w.write_bytes(payload);
            "Unknown"
        }
    };
    (name, w.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cs_usermsgs_roundtrip_cleanly() {
        let msgs = vec![
            CsUserMsg::CurWeapon {
                is_active: true,
                weapon_id: 28,
                clip_ammo: 30,
            },
            CsUserMsg::Money {
                amount: 16000,
                flash: true,
            },
            CsUserMsg::BlinkAcct(30),
            CsUserMsg::NvgToggle(true),
            CsUserMsg::Radar {
                player_id: 3,
                origin: Vec3::new(-1200.0, 64.5, 8.0),
            },
            CsUserMsg::StatusText {
                line: 0,
                text: "1 %c1: %p2
2  %h: %i3%%".to_string(),
            },
            CsUserMsg::StatusValue {
                index: 3,
                value: -7,
            },
            CsUserMsg::ItemPickup("item_kevlar".to_string()),
            CsUserMsg::DeathMsg {
                killer_id: 1,
                victim_id: 2,
                headshot: true,
                weapon: "ak47".to_string(),
            },
            CsUserMsg::ScoreInfo {
                player_id: 1,
                frags: 15,
                deaths: 3,
                class_id: 0,
                team_id: 2,
            },
            CsUserMsg::StatusIcon {
                status: 1,
                sprite_name: "buyzone".to_string(),
                color: Some([0, 160, 0]),
            },
            CsUserMsg::WeaponList {
                name: "weapon_ak47".to_string(),
                ammo1_type: 2,
                ammo1_max: 90,
                ammo2_type: 0xFF,
                ammo2_max: 0xFF,
                slot: 0,
                position: 1,
                weapon_id: 28,
                flags: 0,
            },
            CsUserMsg::AmmoPickup { ammo_id: 5, amount: 24 },
            CsUserMsg::WeapPickup(16),
            CsUserMsg::ReloadSound { volume: 200, kind: 1 },
            CsUserMsg::Crosshair(0),
            CsUserMsg::BarTime(3),
            CsUserMsg::BarTime2 {
                duration: 10,
                start_percent: 40,
            },
            CsUserMsg::Scenario {
                active: false,
                sprite: String::new(),
                alpha: 0,
                flash: None,
            },
            CsUserMsg::Scenario {
                active: true,
                sprite: "hostage4".to_string(),
                alpha: 0,
                flash: None,
            },
            CsUserMsg::Scenario {
                active: true,
                sprite: "bombticking".to_string(),
                alpha: 255,
                flash: Some((139, 0)),
            },
        ];

        for m in msgs {
            let (name, raw) = encode_cs_usermsg(&m);
            let decoded = decode_cs_usermsg(name, &raw).unwrap();
            assert_eq!(decoded, m);
        }
    }

    #[test]
    fn clcorpse_roundtrips() {
        let m = CsUserMsg::ClCorpse {
            model: "gign".to_string(),
            origin: [-100 * 128, 4096 * 128 + 64, 36 * 128],
            angles: Vec3::new(0.0, 90.5, -1.25),
            time_diff: -312,
            sequence: 12,
            body: 1,
            team: 2,
            player_id: 5,
        };
        let (name, raw) = encode_cs_usermsg(&m);
        assert_eq!(name, "ClCorpse");
        assert_eq!(decode_cs_usermsg(name, &raw).unwrap(), m);
    }

    #[test]
    fn phase8_usermsgs_roundtrip() {
        let msgs = vec![
            CsUserMsg::Fog {
                color: [128, 140, 155],
                density: 0.0025,
            },
            CsUserMsg::Flashlight {
                on: true,
                battery: 87,
            },
            CsUserMsg::FlashBat(42),
            CsUserMsg::ReceiveW(1),
            CsUserMsg::Brass(vec![106, 1, 2, 3, 4]),
        ];
        for m in msgs {
            let (name, raw) = encode_cs_usermsg(&m);
            let decoded = decode_cs_usermsg(name, &raw).unwrap();
            assert_eq!(decoded, m);
        }
    }

    #[test]
    fn register_next_and_raw_usermsg_roundtrip() {
        let mut reg = UserMsgRegistry::cs16_default();
        let id = reg.register_next(-1, "CsModEvent").expect("slot available");
        assert!(id >= USERMSG_BASE_ID);
        assert_eq!(reg.register_next(-1, "CsModEvent"), Some(id));
        let mut w = crate::bitbuf::MsgWriter::new();
        assert!(crate::messages::write_svc_raw_usermsg(&mut w, &reg, "CsModEvent", b"hi!"));
        assert_eq!(w.as_slice(), &[id, 3, b'h', b'i', b'!']);
    }

    #[test]
    fn phase10_hud_usermsgs_roundtrip() {
        let msgs = vec![
            CsUserMsg::HudText {
                text: "HINT_USE_BOMB".to_string(),
                style: 1,
            },
            CsUserMsg::HudTextArgs {
                text: "#Cstrike_Tutor_Bomb_Planted_T".to_string(),
                style: 2,
                args: vec!["Alpha".to_string(), "45".to_string()],
            },
            CsUserMsg::AllowSpec(true),
            CsUserMsg::SpecHealth(78),
            CsUserMsg::SpecHealth2 {
                health: 92,
                target: 4,
            },
            CsUserMsg::Damage {
                armor_damage: 12,
                health_damage: 34,
                damage_bits: 0x02,
                origin: Vec3::new(128.0, -64.0, 32.0),
            },
            CsUserMsg::StatusText {
                line: 0,
                text: "1 %c1: %p2  %h: %i3%%".to_string(),
            },
            CsUserMsg::StatusValue {
                index: 2,
                value: 3,
            },
            CsUserMsg::BarTime2 {
                duration: 10,
                start_percent: 45,
            },
        ];
        for m in msgs {
            let (name, raw) = encode_cs_usermsg(&m);
            let decoded = decode_cs_usermsg(name, &raw).unwrap();
            assert_eq!(decoded, m);
        }
        // `HudTextPro` shares the same wire format as `HudText`.
        let (_, pro_raw) = encode_cs_usermsg(&CsUserMsg::HudText {
            text: "BONUS_MSG".to_string(),
            style: 3,
        });
        assert_eq!(
            decode_cs_usermsg("HudTextPro", &pro_raw).unwrap(),
            CsUserMsg::HudText {
                text: "BONUS_MSG".to_string(),
                style: 3,
            }
        );
    }

    #[test]
    fn phase11_all_remaining_usermsgs_roundtrip() {
        let msgs = vec![
            CsUserMsg::Health(87),
            CsUserMsg::Battery(100),
            CsUserMsg::ArmorType(1),
            CsUserMsg::SayText {
                sender_id: 2,
                text: "#Cstrike_Chat_All".to_string(),
                params: vec!["Player".to_string(), "glhf".to_string()],
            },
            CsUserMsg::TeamScore {
                team_name: "CT".to_string(),
                score: 9,
            },
            CsUserMsg::ShowMenu {
                keys_bitmask: 0x03FF,
                display_time: -1,
                multipart: false,
                text: "\\ySelect Team\\w\n1. T".to_string(),
            },
            CsUserMsg::BuyClose,
            CsUserMsg::ViewMode,
            CsUserMsg::GameTitle(1),
            CsUserMsg::GameMode(0),
            CsUserMsg::BlinkAcct(2),
            CsUserMsg::Spectator {
                client: 3,
                is_spectator: true,
            },
            CsUserMsg::Hltv {
                client: 4,
                flags: 128,
            },
            CsUserMsg::ForceCam {
                forcecam: 1,
                forcechasecam: 2,
                fadetoblack: 0,
            },
            CsUserMsg::ADStop,
            CsUserMsg::ShadowIdx(14),
            CsUserMsg::TaskTime {
                time: 45,
                active: 1,
                fade_time: 3,
            },
            CsUserMsg::BotVoice {
                active: true,
                player: 6,
            },
            CsUserMsg::Location {
                player: 2,
                location: "#BombsiteA".to_string(),
            },
            CsUserMsg::HealthInfo {
                player_id: 1,
                health: 100,
            },
            CsUserMsg::Account {
                player_id: 1,
                amount: 800,
            },
            CsUserMsg::ReqState,
        ];
        for m in msgs {
            let (name, raw) = encode_cs_usermsg(&m);
            let decoded = decode_cs_usermsg(name, &raw).unwrap();
            assert_eq!(decoded, m, "roundtrip mismatch for {name}");
        }
    }
}
