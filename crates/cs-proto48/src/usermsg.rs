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
    ResetHud,
    InitHud,
    BuyClose,
    ShowTimer,
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
        "ResetHUD" => CsUserMsg::ResetHud,
        "InitHUD" => CsUserMsg::InitHud,
        "BuyClose" => CsUserMsg::BuyClose,
        "ShowTimer" => CsUserMsg::ShowTimer,
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
        CsUserMsg::ResetHud => "ResetHUD",
        CsUserMsg::InitHud => "InitHUD",
        CsUserMsg::BuyClose => "BuyClose",
        CsUserMsg::ShowTimer => "ShowTimer",
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
}
