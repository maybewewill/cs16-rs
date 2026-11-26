//! The HUD state the user messages build up, and [`ClientEffects`], which ties the effect
//! modules (events, temp entities, sound, HUD) together behind one `handle` the connection calls
//! for every parsed svc message.
//!
//! Registration and framing follow Xash3D FWGS (GPL-3.0-or-later, read 2026-11-23):
//! `CL_RegisterUserMessage` (`engine/client/cl_parse.c`: a number, a size, a 16 byte name; the
//! table is by name) and `CL_ParseUserMessage` (`svc_*` ids above the engine's own are looked up
//! and handed to the game by name). The decoding of each message is `cs_proto48::usermsg`'s
//! (`CsUserMsg`); this module is the reducer over it. The game's HUD code (cs16-client
//! `cl_dll/hud*.cpp`, Valve-licensed) is not ported: what each message means comes from what the
//! server writes (`ReGameDLL_CS`) and from `cs-game`'s own HUD tests, and where a meaning could
//! not be settled it is marked "assumption".
//!
//! A message the registry has a name for but this reducer does not model, and a message with a
//! name this client has never heard of, is kept in [`HudState::unknown_messages`]: a mod's HUD
//! reads its own messages from there.

use std::collections::{BTreeMap, HashMap, VecDeque};

use cs_proto48::messages::{CustomizationMsg, SpawnStaticMsg, SvcMessage};
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

use crate::events::{EventFx, EventQueue, FiredEvent, FxEnv, ViewAnim};
use crate::sound::{SoundCommand, SoundQueue, VoiceEvent, VoiceQueue};
use crate::tempents::{
    FxRandom, ModelRef, RenderEffect, SmokeCloudConfig, TeTextMessage, TempEntClock, TempEnts,
};

/// `FFADE_*` flags (`screenfade.h`).
pub const FFADE_IN: u16 = 0x0000;
pub const FFADE_OUT: u16 = 0x0001;
pub const FFADE_MODULATE: u16 = 0x0002;
pub const FFADE_STAYOUT: u16 = 0x0004;
pub const FFADE_PURGE: u16 = 0x0010;

/// The entries each log of the HUD keeps (oldest dropped).
pub const LOG_CAPACITY: usize = 64;
/// Messages of unknown name kept for mods.
pub const UNKNOWN_CAPACITY: usize = 256;
/// Bytes of MOTD kept (a longer one is cut).
pub const MAX_MOTD_BYTES: usize = 64 * 1024;
/// Bytes of a multipart menu kept.
pub const MAX_MENU_BYTES: usize = 8 * 1024;
/// Status icons shown at once.
pub const MAX_STATUS_ICONS: usize = 64;
/// Teams with a score.
pub const MAX_TEAM_SCORES: usize = 16;
/// `SendAudio` radar flash lifetime (`client.dll` `FUN_10064bc0`: 22 half-cycles @ 0.15s = 3.3s).
pub const RADIO_FLASH_DURATION_SECS: f64 = 22.0 * 0.15;

/// One weapon of the HUD's weapon table (`WeaponList`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeaponInfo {
    pub name: String,
    pub ammo1_type: u8,
    pub ammo1_max: u8,
    pub ammo2_type: u8,
    pub ammo2_max: u8,
    pub slot: u8,
    pub position: u8,
    pub weapon_id: u8,
    pub flags: u8,
}

/// `CurWeapon` as the server last sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentWeapon {
    pub is_active: bool,
    pub weapon_id: u8,
    pub clip: i8,
}

/// A row of the scoreboard (`ScoreInfo`, `ScoreAttrib`, `TeamInfo`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayerScore {
    pub frags: i16,
    pub deaths: i16,
    pub class_id: i16,
    pub team_id: i16,
    /// `ScoreAttrib`: bit 0 dead (`SCORE_STATUS_DEAD`), bit 1 carries the bomb
    /// (`SCORE_STATUS_BOMB`), bit 2 is the VIP (`SCORE_STATUS_VIP`), ReGameDLL_CS
    /// `regamedll/dlls/cdll_dll.h:63-67`.
    pub attrib: u8,
    /// `TeamInfo`: `TERRORIST`, `CT`, `SPECTATOR`, `UNASSIGNED`.
    pub team: String,
}

/// A status icon (`StatusIcon`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusIcon {
    /// 0 hides it, 1 shows it, 2 makes it flash.
    pub status: u8,
    pub color: Option<[u8; 3]>,
}

/// A kill (`DeathMsg`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeathNotice {
    pub killer: u8,
    pub victim: u8,
    pub headshot: bool,
    pub weapon: String,
}

/// A chat line (`SayText`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLine {
    pub sender: u8,
    pub text: String,
    pub params: Vec<String>,
}

/// A text message (`TextMsg`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextLine {
    /// `HUD_PRINTNOTIFY` 1, `HUD_PRINTCONSOLE` 2, `HUD_PRINTTALK` 3, `HUD_PRINTCENTER` 4,
    /// `HUD_PRINTRADIO` 5.
    pub dest: u8,
    pub message: String,
    pub params: Vec<String>,
}

/// A menu the server asks for (`ShowMenu`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuState {
    pub keys: u16,
    /// Seconds it stays; -1 for ever.
    pub display_time: i8,
    pub multipart: bool,
    pub text: String,
}

/// A VGUI menu the server asks for (`VGUIMenu`): the buy menu, the team menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VguiMenuState {
    pub menu_id: u8,
    pub keys: u16,
    pub time: i8,
    pub multipart: bool,
    pub name: String,
}

/// The progress bar of defusing and planting (`BarTime`, `BarTime2`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarState {
    /// Seconds the bar takes; 0 clears it.
    pub duration: u16,
    pub start_percent: u16,
    /// Client time it started.
    pub started_at: f64,
}

/// The damage taken last (`Damage`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageState {
    pub armor: u8,
    pub health: u8,
    pub bits: u32,
    pub origin: Vec3,
}

/// Localized tutor/hint entry from `HudText`, `HudTextPro`, or `HudTextArgs`.
#[derive(Debug, Clone, PartialEq)]
pub struct HudTextEntry {
    pub text: String,
    pub args: Vec<String>,
    pub style: u8,
    pub received_at: f64,
}

/// A screen fade (`ScreenFade`): the engine's own message, kept for the shell to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenFade {
    pub duration: u16,
    pub hold_time: u16,
    pub flags: u16,
    pub color: [u8; 4],
    pub started_at: f64,
}

/// A screen shake (`ScreenShake`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenShake {
    pub amplitude: u16,
    pub duration: u16,
    pub frequency: u16,
    pub started_at: f64,
}

/// The objective icon (`Scenario`).
#[derive(Debug, Clone, PartialEq)]
pub struct ScenarioIcon {
    pub sprite: String,
    pub alpha: u8,
    pub flash: Option<(i16, i16)>,
}

/// A user message the reducer keeps raw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownUserMsg {
    pub id: u8,
    pub name: String,
    pub payload: Vec<u8>,
}

/// A registered user message (`svc_newusermsg`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMessageInfo {
    pub name: String,
    /// The fixed size, or -1 for a variable one.
    pub size: i8,
}

/// Distance fog parameters (`Fog` usermsg / `env_fog`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FogState {
    pub color: [u8; 3],
    pub density: f32,
}

/// Player custom spray metadata (`svc_customization`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomDecalInfo {
    pub player: u8,
    pub filename: String,
    pub flags: u8,
    pub md5: Option<[u8; 16]>,
}

/// Authentic GoldSrc default lightstyle pattern for style index `idx` (0..64).
pub fn default_lightstyle(idx: usize) -> String {
    match idx {
        0 => "m",
        1 => "mmnmmommommnonmmonqnmmo",
        2 => "abcdefghijklmnopqrstuvwxyzyxwvutsrqponmlkjihgfedcba",
        3 => "mmmmmaaaaammmmmaaaaaabcdefgabcdefg",
        4 => "mamamamamama",
        5 => "jklmnopqrstuvwxyzyxwvutsrqponmlkj",
        6 => "nmonqnmomnmomomno",
        7 => "mmmaaaabcdefgmmmmaaaammmaamm",
        8 => "mmmaaammmaaammmabcdefaaaammmmabcdefmmmaaaa",
        9 => "aaaaaaaazzzzzzzz",
        10 => "mmamammmmammamamaaamammma",
        11 => "abcdefghijklmnopqrrqponmlkjihgfedcba",
        _ => "m",
    }
    .to_string()
}

pub fn default_lightstyles() -> [String; 64] {
    std::array::from_fn(default_lightstyle)
}

/// Evaluate GoldSrc 10 Hz lightstyles (`R_AnimateLight`): `'a' -> 0.0`, `'m' -> 1.0`, `'z' -> 25/12`.
pub fn evaluate_lightstyles(styles: &[String; 64], time: f32) -> [f32; 64] {
    let mut out = [1.0f32; 64];
    let frame = (time.max(0.0) * 10.0).floor() as usize;
    for (slot, pat) in styles.iter().enumerate() {
        let bytes = pat.as_bytes();
        if bytes.is_empty() {
            out[slot] = 1.0;
            continue;
        }
        let b = bytes[frame % bytes.len()];
        let val = (f32::from(b.saturating_sub(b'a')) / 12.0).clamp(0.0, 2.5);
        out[slot] = val;
    }
    out
}

/// Everything the HUD shows, as the server's messages left it.
#[derive(Debug, Clone)]
pub struct HudState {
    /// The messages the server registered, by number.
    pub user_messages: BTreeMap<u8, UserMessageInfo>,
    pub health: u8,
    /// `Battery`: the armor points.
    pub armor: i16,
    /// `ArmorType`: `0` vest only, `1` vest and helmet (verified via Ghidra retail `client.dll`
    /// `FUN_10046f20`; matches `cs-game` `send_armor_type` and HUD sprite selection).
    pub armor_type: u8,
    pub money: i32,
    /// The last `Money` asked for the change to flash.
    pub money_flash: bool,
    /// `BlinkAcct`: how many times the account blinks.
    pub blink_account: u8,
    /// Monotonic counter incremented each time a `BlinkAcct` message arrives.
    pub blink_account_seq: u32,
    pub current_weapon: Option<CurrentWeapon>,
    /// The last clip count each weapon was reported with.
    pub clips: BTreeMap<u8, i8>,
    /// `AmmoX`: the ammunition by ammo type.
    pub ammo: BTreeMap<u8, u8>,
    /// `WeaponList`: the table of weapons by id.
    pub weapons: BTreeMap<u8, WeaponInfo>,
    /// Weapons picked up recently (`WeapPickup`), for the pickup history.
    pub weapon_pickups: VecDeque<u8>,
    pub ammo_pickups: VecDeque<(u8, u8)>,
    pub item_pickups: VecDeque<String>,
    pub fov: u8,
    /// `HideWeapon`: the `HIDEHUD_*` bits.
    pub hidden: u8,
    pub crosshair: u8,
    pub item_status: u8,
    /// `RoundTime`: the seconds left when it arrived.
    pub round_time: u16,
    /// Client time `RoundTime` arrived.
    pub round_time_at: f64,
    /// `ShowTimer` arrived: the round timer is drawn.
    pub timer_visible: bool,
    pub status_icons: BTreeMap<String, StatusIcon>,
    /// `StatusText`: the format of each status bar line.
    pub status_text: BTreeMap<u8, String>,
    /// `StatusValue`: the numbers the status text refers to.
    pub status_values: BTreeMap<u8, i16>,
    pub players: BTreeMap<u8, PlayerScore>,
    /// `TeamScore`.
    pub team_scores: BTreeMap<String, i16>,
    pub deaths: VecDeque<DeathNotice>,
    pub chat: VecDeque<ChatLine>,
    pub text: VecDeque<TextLine>,
    /// `svc_centerprint`.
    pub center_print: Option<String>,
    pub audio: VecDeque<(u8, String, i16)>,
    /// Active radio-sender flash expiry timestamp (`now + 3.3s`, `client.dll:FUN_10064bc0`).
    pub radio_flashes: HashMap<u8, f64>,
    pub reload_sounds: VecDeque<(u8, u8)>,
    pub radar: BTreeMap<u8, Vec3>,
    /// `HostagePos`: the flag and origin of each hostage.
    pub hostages: BTreeMap<u8, (u8, Vec3)>,
    pub hostages_killed: VecDeque<u8>,
    /// The bomb on the ground (`BombDrop`) or planted; `None` once picked up.
    pub bomb: Option<(Vec3, bool)>,
    pub bar: Option<BarState>,
    pub menu: Option<MenuState>,
    pub vgui_menu: Option<VguiMenuState>,
    pub buy_closed: bool,
    /// `MOTD`, the parts received so far; complete when [`HudState::motd_complete`].
    pub motd: String,
    pub motd_complete: bool,
    pub server_name: String,
    pub damage: Option<DamageState>,
    pub damage_history: VecDeque<(DamageState, f64)>,
    /// Monotonic counter incremented on every `Damage` user message.
    pub damage_seq: u32,
    pub hud_texts: VecDeque<HudTextEntry>,
    pub allow_spec: bool,
    pub spec_health: Option<(u8, u8)>,
    /// `HealthInfo` (build 10210): per-player health keyed by 1-based `player_id`.
    pub player_health: BTreeMap<u8, i32>,
    /// `Account` (build 10210): per-player money keyed by 1-based `player_id`.
    pub player_money: BTreeMap<u8, i32>,
    /// `ForceCam` (`mp_forcecamera`, `mp_forcechasecam`, `mp_fadetoblack`).
    pub force_cam: Option<(u8, u8, u8)>,
    /// Count of `ReqState` messages received.
    pub req_state_count: u32,
    pub screen_fade: Option<ScreenFade>,
    pub screen_shake: Option<ScreenShake>,
    pub scenario: Option<ScenarioIcon>,
    /// Monotonic counter incremented on every `Scenario` user message.
    pub scenario_seq: u32,
    pub night_vision: bool,
    pub flashlight_on: bool,
    pub flash_battery: u8,
    pub fog: Option<FogState>,
    pub weather: u8,
    pub lightstyles: [String; 64],
    pub custom_decals: HashMap<u8, CustomDecalInfo>,
    /// Validated `!MD5<hex>` player spray WADs cached in memory by MD5 digest.
    pub custom_sprays: BTreeMap<[u8; 16], Vec<u8>>,
    pub static_entities: Vec<SpawnStaticMsg>,
    pub te_text_messages: Vec<TeTextMessage>,
    /// `ClCorpse`: the bodies the shell draws.
    pub corpses: Vec<CsUserMsg>,
    pub server_weapon_anim: Option<ViewAnim>,
    /// Messages nothing here models, in order of arrival.
    pub unknown_messages: VecDeque<UnknownUserMsg>,
    /// How many of them came, kept or not.
    pub unknown_total: u64,
    /// `InitHUD` arrived (the server's sign-on messages start with it).
    pub initialized: bool,
    pub hud_resets: u32,
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            user_messages: BTreeMap::new(),
            health: 0,
            armor: 0,
            armor_type: 0,
            money: 0,
            money_flash: false,
            blink_account: 0,
            blink_account_seq: 0,
            current_weapon: None,
            clips: BTreeMap::new(),
            ammo: BTreeMap::new(),
            weapons: BTreeMap::new(),
            weapon_pickups: VecDeque::new(),
            ammo_pickups: VecDeque::new(),
            item_pickups: VecDeque::new(),
            fov: 0,
            hidden: 0,
            crosshair: 0,
            item_status: 0,
            round_time: 0,
            round_time_at: 0.0,
            timer_visible: false,
            status_icons: BTreeMap::new(),
            status_text: BTreeMap::new(),
            status_values: BTreeMap::new(),
            players: BTreeMap::new(),
            team_scores: BTreeMap::new(),
            deaths: VecDeque::new(),
            chat: VecDeque::new(),
            text: VecDeque::new(),
            center_print: None,
            audio: VecDeque::new(),
            radio_flashes: HashMap::new(),
            reload_sounds: VecDeque::new(),
            radar: BTreeMap::new(),
            hostages: BTreeMap::new(),
            hostages_killed: VecDeque::new(),
            bomb: None,
            bar: None,
            menu: None,
            vgui_menu: None,
            buy_closed: false,
            motd: String::new(),
            motd_complete: false,
            server_name: String::new(),
            damage: None,
            damage_history: VecDeque::new(),
            damage_seq: 0,
            hud_texts: VecDeque::new(),
            allow_spec: false,
            spec_health: None,
            player_health: BTreeMap::new(),
            player_money: BTreeMap::new(),
            force_cam: None,
            req_state_count: 0,
            screen_fade: None,
            screen_shake: None,
            scenario: None,
            scenario_seq: 0,
            night_vision: false,
            flashlight_on: false,
            flash_battery: 100,
            fog: None,
            weather: 0,
            lightstyles: default_lightstyles(),
            custom_decals: HashMap::new(),
            custom_sprays: BTreeMap::new(),
            static_entities: Vec::new(),
            te_text_messages: Vec::new(),
            corpses: Vec::new(),
            server_weapon_anim: None,
            unknown_messages: VecDeque::new(),
            unknown_total: 0,
            initialized: false,
            hud_resets: 0,
        }
    }
}

/// The longest prefix of `text` of at most `max` bytes that ends on a character boundary.
fn truncated(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn push_bounded<T>(log: &mut VecDeque<T>, entry: T) {
    if log.len() >= LOG_CAPACITY {
        log.pop_front();
    }
    log.push_back(entry);
}

const MAX_DHUD_MESSAGES: usize = 8;

fn decode_director_text_message(bytes: &[u8], now: f64) -> Option<TeTextMessage> {
    // Wire layout (`DRC_CMD_MESSAGE` = 6):
    // u8 cmd | u8 effect | u32/rgba color | 6 * f32 (x, y, fadein, fadeout, hold, fxtime) | cstr text.
    if bytes.len() < 31 || bytes[0] != cs_proto48::messages::drc::CMD_MESSAGE {
        return None;
    }
    let effect = bytes[1];
    // AMXX `dhudmessage.inc` / `send_dhud_message` packs `0x00RRGGBB` as LE u32 (`[b, g, r, 0]`);
    // HLTV `encode_director_message` writes raw `[r, g, b, a]` bytes (`bytes[5] != 0`).
    let color1 = if bytes[5] == 0 {
        [bytes[4], bytes[3], bytes[2], 255]
    } else {
        [bytes[2], bytes[3], bytes[4], bytes[5]]
    };
    let read_f32 = |off: usize| -> f32 {
        f32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
    };
    let x_f = read_f32(6);
    let y_f = read_f32(10);
    let fadein_f = read_f32(14);
    let fadeout_f = read_f32(18);
    let hold_f = read_f32(22);
    let fxtime_f = read_f32(26);
    let text_bytes = &bytes[30..];
    let end = text_bytes
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(text_bytes.len());
    let text = String::from_utf8_lossy(&text_bytes[..end]).into_owned();
    if text.is_empty() {
        return None;
    }
    let to_coord = |v: f32| -> i16 {
        if !v.is_finite() || v < 0.0 {
            -1
        } else {
            (v * 8192.0).round().clamp(0.0, f32::from(i16::MAX)) as i16
        }
    };
    let to_u16_time = |v: f32| -> u16 {
        if !v.is_finite() || v <= 0.0 {
            0
        } else {
            (v * 256.0).round().clamp(0.0, f32::from(u16::MAX)) as u16
        }
    };
    Some(TeTextMessage {
        channel: 0,
        x: to_coord(x_f),
        y: to_coord(y_f),
        effect,
        color1,
        color2: [255, 255, 255, 255],
        fadein: to_u16_time(fadein_f),
        fadeout: to_u16_time(fadeout_f),
        holdtime: to_u16_time(hold_f),
        fxtime: (effect == 2).then(|| to_u16_time(fxtime_f)),
        text,
        received_at: now,
    })
}

impl HudState {
    /// Seconds the round has left now (`RoundTime` counts down on the client).
    pub fn round_seconds_left(&self, now: f64) -> f64 {
        (f64::from(self.round_time) - (now - self.round_time_at)).max(0.0)
    }

    /// Stores a `TE_TEXTMESSAGE` or `SVC_DIRECTOR` `DRC_CMD_MESSAGE` banner for HUD rendering.
    /// Non-zero channels (`1..=4`) replace any existing message on that channel; channel `0`
    /// (DHUD / auto-channel) keeps up to 8 concurrent banners.
    pub fn push_te_text_message(&mut self, mut msg: TeTextMessage, now: f64) {
        if !msg.received_at.is_finite() || msg.received_at < 0.0 {
            msg.received_at = now;
        }
        self.prune_te_text_messages(now);
        if msg.channel != 0 {
            self.te_text_messages.retain(|m| m.channel != msg.channel);
        } else {
            let dhud_count = self
                .te_text_messages
                .iter()
                .filter(|m| m.channel == 0)
                .count();
            if dhud_count >= MAX_DHUD_MESSAGES
                && let Some(pos) = self.te_text_messages.iter().position(|m| m.channel == 0)
            {
                self.te_text_messages.remove(pos);
            }
        }
        self.te_text_messages.push(msg);
    }

    /// Removes expired `TE_TEXTMESSAGE` / DHUD messages whose fade+hold window has elapsed.
    pub fn prune_te_text_messages(&mut self, now: f64) {
        self.te_text_messages.retain(|msg| {
            let fade_in = f64::from(msg.fadein) / 256.0;
            let hold = f64::from(msg.holdtime) / 256.0;
            let fade_out = f64::from(msg.fadeout) / 256.0;
            let total = if msg.effect == 2 {
                let char_rate = (f64::from(msg.fxtime.unwrap_or(0)) / 256.0).max(0.01);
                char_rate * (msg.text.chars().count().max(1) as f64) + hold + fade_out
            } else {
                fade_in + hold + fade_out
            };
            let elapsed = now - msg.received_at;
            elapsed < total.max(0.05) + 0.1
        });
    }

    /// Reacts to one parsed svc message at client time `now`; messages that are not about the
    /// HUD are ignored.
    pub fn handle(&mut self, msg: &SvcMessage, now: f64) {
        match msg {
            SvcMessage::NewUserMsg(desc) => {
                self.user_messages.insert(
                    desc.id,
                    UserMessageInfo {
                        name: desc.name.clone(),
                        size: desc.size,
                    },
                );
            }
            SvcMessage::CenterPrint(text) => self.center_print = Some(text.clone()),
            SvcMessage::WeaponAnim { anim, body } => {
                self.server_weapon_anim = Some(ViewAnim {
                    sequence: i32::from(*anim),
                    body: i32::from(*body),
                });
            }
            SvcMessage::Director(bytes) => {
                if let Some(te) = decode_director_text_message(bytes, now) {
                    self.push_te_text_message(te, now);
                }
            }
            SvcMessage::UserMsg { id, decoded, .. } => self.user_message(*id, decoded, now),
            _ => {}
        }
    }

    fn user_message(&mut self, msg_id: u8, msg: &CsUserMsg, now: f64) {
        match msg {
            CsUserMsg::Health(h) => self.health = *h,
            CsUserMsg::Battery(a) => self.armor = *a,
            CsUserMsg::ArmorType(t) => self.armor_type = *t,
            CsUserMsg::Money { amount, flash } => {
                self.money = *amount;
                self.money_flash = *flash;
            }
            CsUserMsg::BlinkAcct(n) => {
                self.blink_account = *n;
                if *n > 0 {
                    self.blink_account_seq = self.blink_account_seq.wrapping_add(1);
                }
            }
            CsUserMsg::CurWeapon {
                is_active,
                weapon_id,
                clip_ammo,
            } => {
                if *weapon_id != 0xFF {
                    self.clips.insert(*weapon_id, *clip_ammo);
                }
                if *is_active {
                    self.current_weapon = Some(CurrentWeapon {
                        is_active: *is_active,
                        weapon_id: *weapon_id,
                        clip: *clip_ammo,
                    });
                } else if *weapon_id == 0 || (*weapon_id == 0xFF && *clip_ammo == -1) {
                    self.current_weapon = None;
                }
            }
            CsUserMsg::AmmoX { ammo_id, amount } => {
                self.ammo.insert(*ammo_id, *amount);
            }
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
                self.weapons.insert(
                    *weapon_id,
                    WeaponInfo {
                        name: name.clone(),
                        ammo1_type: *ammo1_type,
                        ammo1_max: *ammo1_max,
                        ammo2_type: *ammo2_type,
                        ammo2_max: *ammo2_max,
                        slot: *slot,
                        position: *position,
                        weapon_id: *weapon_id,
                        flags: *flags,
                    },
                );
            }
            CsUserMsg::WeapPickup(weapon) => push_bounded(&mut self.weapon_pickups, *weapon),
            CsUserMsg::AmmoPickup { ammo_id, amount } => {
                push_bounded(&mut self.ammo_pickups, (*ammo_id, *amount));
            }
            CsUserMsg::ItemPickup(name) => push_bounded(&mut self.item_pickups, name.clone()),
            CsUserMsg::SetFov(fov) => self.fov = *fov,
            CsUserMsg::HideWeapon(bits) => self.hidden = *bits,
            CsUserMsg::Crosshair(c) => self.crosshair = *c,
            CsUserMsg::ItemStatus(s) => self.item_status = *s,
            CsUserMsg::RoundTime(t) => {
                self.round_time = *t;
                self.round_time_at = now;
            }
            CsUserMsg::ShowTimer => self.timer_visible = true,
            CsUserMsg::StatusIcon {
                status,
                sprite_name,
                color,
            } => {
                if *status == 0 {
                    self.status_icons.remove(sprite_name);
                } else if self.status_icons.len() < MAX_STATUS_ICONS
                    || self.status_icons.contains_key(sprite_name)
                {
                    self.status_icons.insert(
                        sprite_name.clone(),
                        StatusIcon {
                            status: *status,
                            color: *color,
                        },
                    );
                }
            }
            CsUserMsg::StatusText { line, text } => {
                self.status_text.insert(*line, text.clone());
            }
            CsUserMsg::StatusValue { index, value } => {
                self.status_values.insert(*index, *value);
            }
            CsUserMsg::ScoreInfo {
                player_id,
                frags,
                deaths,
                class_id,
                team_id,
            } => {
                let p = self.players.entry(*player_id).or_default();
                p.frags = *frags;
                p.deaths = *deaths;
                p.class_id = *class_id;
                p.team_id = *team_id;
            }
            CsUserMsg::ScoreAttrib { player_id, flags } => {
                self.players.entry(*player_id).or_default().attrib = *flags;
            }
            CsUserMsg::TeamInfo {
                player_id,
                team_name,
            } => {
                self.players.entry(*player_id).or_default().team = team_name.clone();
            }
            CsUserMsg::TeamScore { team_name, score } => {
                if self.team_scores.len() < MAX_TEAM_SCORES
                    || self.team_scores.contains_key(team_name)
                {
                    self.team_scores.insert(team_name.clone(), *score);
                }
            }
            CsUserMsg::DeathMsg {
                killer_id,
                victim_id,
                headshot,
                weapon,
            } => push_bounded(
                &mut self.deaths,
                DeathNotice {
                    killer: *killer_id,
                    victim: *victim_id,
                    headshot: *headshot,
                    weapon: weapon.clone(),
                },
            ),
            CsUserMsg::SayText {
                sender_id,
                text,
                params,
            } => push_bounded(
                &mut self.chat,
                ChatLine {
                    sender: *sender_id,
                    text: text.clone(),
                    params: params.clone(),
                },
            ),
            CsUserMsg::TextMsg {
                dest,
                message,
                params,
            } => {
                // `HUD_PRINTCENTER`: the line is the centre print.
                if *dest == 4 {
                    self.center_print = Some(message.clone());
                }
                push_bounded(
                    &mut self.text,
                    TextLine {
                        dest: *dest,
                        message: message.clone(),
                        params: params.clone(),
                    },
                );
            }
            CsUserMsg::SendAudio {
                sender_id,
                audio_code,
                pitch,
            } => {
                if *sender_id > 0 {
                    self.radio_flashes
                        .insert(*sender_id, now + RADIO_FLASH_DURATION_SECS);
                }
                push_bounded(&mut self.audio, (*sender_id, audio_code.clone(), *pitch));
            }
            CsUserMsg::ReloadSound { volume, kind } => {
                push_bounded(&mut self.reload_sounds, (*volume, *kind));
            }
            CsUserMsg::Radar { player_id, origin } => {
                self.radar.insert(*player_id, *origin);
            }
            CsUserMsg::HostagePos {
                flag,
                hostage_id,
                origin,
            } => {
                self.hostages.insert(*hostage_id, (*flag, *origin));
            }
            CsUserMsg::HostageK(hostage) => {
                self.hostages.remove(hostage);
                push_bounded(&mut self.hostages_killed, *hostage);
            }
            CsUserMsg::BombDrop { origin, planted } => self.bomb = Some((*origin, *planted)),
            CsUserMsg::BombPickup => self.bomb = None,
            CsUserMsg::BarTime(duration) => {
                self.bar = (*duration != 0).then_some(BarState {
                    duration: *duration,
                    start_percent: 0,
                    started_at: now,
                });
            }
            CsUserMsg::BarTime2 {
                duration,
                start_percent,
            } => {
                self.bar = (*duration != 0).then_some(BarState {
                    duration: *duration,
                    start_percent: *start_percent,
                    started_at: now,
                });
            }
            CsUserMsg::ShowMenu {
                keys_bitmask,
                display_time,
                multipart,
                text,
            } => {
                // A multipart menu continues the text of the one before it (assumption: the
                // `multipart` byte means more follows, as in the original HUD).
                match &mut self.menu {
                    Some(menu) if menu.multipart => {
                        if menu.text.len() + text.len() <= MAX_MENU_BYTES {
                            menu.text.push_str(text);
                        }
                        menu.keys = *keys_bitmask;
                        menu.display_time = *display_time;
                        menu.multipart = *multipart;
                    }
                    _ => {
                        self.menu = Some(MenuState {
                            keys: *keys_bitmask,
                            display_time: *display_time,
                            multipart: *multipart,
                            text: truncated(text, MAX_MENU_BYTES),
                        });
                    }
                }
                if self
                    .menu
                    .as_ref()
                    .is_some_and(|m| !m.multipart && (m.keys == 0 || m.text.is_empty()))
                {
                    self.menu = None;
                }
            }
            CsUserMsg::VguiMenu {
                menu_id,
                keys_bitmask,
                time,
                multipart,
                name,
            } => {
                self.vgui_menu = Some(VguiMenuState {
                    menu_id: *menu_id,
                    keys: *keys_bitmask,
                    time: *time,
                    multipart: *multipart,
                    name: name.clone(),
                });
            }
            CsUserMsg::BuyClose => {
                self.buy_closed = true;
                self.menu = None;
                self.vgui_menu = None;
            }
            CsUserMsg::Motd { finished, text } => {
                if self.motd_complete {
                    // A new MOTD starts over.
                    self.motd.clear();
                    self.motd_complete = false;
                }
                let room = MAX_MOTD_BYTES.saturating_sub(self.motd.len());
                self.motd.push_str(&truncated(text, room));
                self.motd_complete = *finished;
            }
            CsUserMsg::ServerName(n) => self.server_name = n.clone(),
            CsUserMsg::Damage {
                armor_damage,
                health_damage,
                damage_bits,
                origin,
            } => {
                let d = DamageState {
                    armor: *armor_damage,
                    health: *health_damage,
                    bits: *damage_bits,
                    origin: *origin,
                };
                self.damage = Some(d);
                self.damage_seq = self.damage_seq.wrapping_add(1);
                if d.health > 0 || d.armor > 0 || d.bits != 0 {
                    if self.damage_history.len() >= 8 {
                        self.damage_history.pop_front();
                    }
                    self.damage_history.push_back((d, now));
                }
            }
            CsUserMsg::HudText { text, style } => {
                if self.hud_texts.len() >= 16 {
                    self.hud_texts.pop_front();
                }
                self.hud_texts.push_back(HudTextEntry {
                    text: text.clone(),
                    args: Vec::new(),
                    style: *style,
                    received_at: now,
                });
            }
            CsUserMsg::HudTextArgs { text, style, args } => {
                if self.hud_texts.len() >= 16 {
                    self.hud_texts.pop_front();
                }
                self.hud_texts.push_back(HudTextEntry {
                    text: text.clone(),
                    args: args.clone(),
                    style: *style,
                    received_at: now,
                });
            }
            CsUserMsg::AllowSpec(allow) => {
                self.allow_spec = *allow;
            }
            CsUserMsg::SpecHealth(health) => {
                let prev_target = self.spec_health.map_or(0, |(_, t)| t);
                self.spec_health = Some((*health, prev_target));
            }
            CsUserMsg::SpecHealth2 { health, target } => {
                self.spec_health = Some((*health, *target));
            }
            CsUserMsg::HealthInfo { player_id, health } => {
                self.player_health.insert(*player_id, *health);
            }
            CsUserMsg::Account { player_id, amount } => {
                self.player_money.insert(*player_id, *amount);
            }
            CsUserMsg::ReqState => {
                self.req_state_count = self.req_state_count.wrapping_add(1);
            }
            CsUserMsg::ForceCam {
                forcecam,
                forcechasecam,
                fadetoblack,
            } => {
                self.force_cam = Some((*forcecam, *forcechasecam, *fadetoblack));
            }
            CsUserMsg::ScreenFade {
                duration,
                hold_time,
                flags,
                color,
            } => {
                self.screen_fade = Some(ScreenFade {
                    duration: *duration,
                    hold_time: *hold_time,
                    flags: *flags,
                    color: *color,
                    started_at: now,
                });
            }
            CsUserMsg::ScreenShake {
                amplitude,
                duration,
                frequency,
            } => {
                self.screen_shake = Some(ScreenShake {
                    amplitude: *amplitude,
                    duration: *duration,
                    frequency: *frequency,
                    started_at: now,
                });
            }
            CsUserMsg::Scenario {
                active,
                sprite,
                alpha,
                flash,
            } => {
                self.scenario_seq = self.scenario_seq.wrapping_add(1);
                self.scenario = active.then(|| ScenarioIcon {
                    sprite: sprite.clone(),
                    alpha: *alpha,
                    flash: *flash,
                });
            }
            CsUserMsg::NvgToggle(on) => self.night_vision = *on,
            CsUserMsg::Fog { color, density } => {
                self.fog = (*density > 0.0).then_some(FogState {
                    color: *color,
                    density: *density,
                });
            }
            CsUserMsg::Flashlight { on, battery } => {
                self.flashlight_on = *on;
                self.flash_battery = *battery;
            }
            CsUserMsg::FlashBat(battery) => {
                self.flash_battery = *battery;
            }
            CsUserMsg::ReceiveW(weather) => {
                self.weather = *weather;
            }
            CsUserMsg::Brass(_) => {}
            CsUserMsg::ClCorpse { .. } => {
                if self.corpses.len() >= LOG_CAPACITY {
                    self.corpses.remove(0);
                }
                self.corpses.push(msg.clone());
            }
            // `ResetHUD` starts a life over: what is drawn from the last one goes; the values
            // the server sends again (health, money, ammo) stay until it does (assumption).
            CsUserMsg::ResetHud => {
                self.hud_resets = self.hud_resets.saturating_add(1);
                self.bar = None;
                self.menu = None;
                self.vgui_menu = None;
                self.buy_closed = false;
                self.radar.clear();
                self.radio_flashes.clear();
                self.center_print = None;
                self.screen_fade = None;
                self.screen_shake = None;
                self.status_icons.clear();
            }
            CsUserMsg::InitHud => self.initialized = true,
            CsUserMsg::ViewMode
            | CsUserMsg::GameTitle(_)
            | CsUserMsg::GameMode(_)
            | CsUserMsg::Spectator { .. }
            | CsUserMsg::Hltv { .. }
            | CsUserMsg::ADStop
            | CsUserMsg::ShadowIdx(_)
            | CsUserMsg::TaskTime { .. }
            | CsUserMsg::BotVoice { .. }
            | CsUserMsg::Location { .. } => {}
            CsUserMsg::Unknown { name, payload } => {
                self.unknown_total = self.unknown_total.saturating_add(1);
                if self.unknown_messages.len() >= UNKNOWN_CAPACITY {
                    self.unknown_messages.pop_front();
                }
                self.unknown_messages.push_back(UnknownUserMsg {
                    id: msg_id,
                    name: name.clone(),
                    payload: payload.clone(),
                });
            }
        }
    }
}

/// The state the effect modules build up from the svc stream.
#[derive(Debug, Clone)]
pub struct ClientEffects {
    pub events: EventQueue,
    pub tempents: TempEnts,
    pub sounds: SoundQueue,
    pub voice: VoiceQueue,
    pub hud: HudState,
    pub rng: FxRandom,
    /// `max_smokepuffs` CVar value (default `120`).
    pub max_smokepuffs: usize,
    /// `fastsprites` CVar value (default `0`).
    pub fastsprites: u8,
    /// The last time [`ClientEffects::update`] ran, for the frame time of the next.
    last_update: Option<f64>,
}

impl Default for ClientEffects {
    fn default() -> Self {
        Self {
            events: EventQueue::default(),
            tempents: TempEnts::default(),
            sounds: SoundQueue::default(),
            voice: VoiceQueue::default(),
            hud: HudState::default(),
            rng: FxRandom::default(),
            max_smokepuffs: 120,
            fastsprites: 0,
            last_update: None,
        }
    }
}

impl ClientEffects {
    /// A fresh state whose random numbers follow `seed`.
    pub fn with_seed(seed: u64) -> Self {
        Self {
            rng: FxRandom::new(seed),
            ..Self::default()
        }
    }

    /// Update smoke cloud CVars (`max_smokepuffs`, `fastsprites`) from client settings.
    pub fn set_smoke_cvars(&mut self, max_smokepuffs: usize, fastsprites: u8) {
        self.max_smokepuffs = max_smokepuffs;
        self.fastsprites = fastsprites;
    }

    /// Forgets the effects of the map that ended (`CL_ClearEffects`); the HUD state stays.
    pub fn clear_effects(&mut self) {
        self.events.clear();
        self.tempents.clear();
        self.sounds.clear();
        self.voice.clear();
        self.last_update = None;
    }

    /// One parsed svc message. Events are queued and temp entities spawned at once; the events
    /// fire in [`ClientEffects::update`].
    pub fn handle(&mut self, msg: &SvcMessage, env: &FxEnv) {
        match msg {
            // A new server: the effects of the old map and the HUD of the old game go.
            SvcMessage::ServerInfo(_) => {
                self.clear_effects();
                self.hud = HudState::default();
            }
            SvcMessage::Event(_) | SvcMessage::EventReliable { .. } => {
                self.events.handle(msg, env);
            }
            SvcMessage::TempEntity { te_type, raw } => {
                // A payload the reader cannot decode was already consumed by the stream
                // parser; nothing is spawned for it.
                if let Ok(effect) = cs_proto48::tempent::TempEffect::parse(*te_type, raw) {
                    self.tempents.spawn_effect(
                        &effect,
                        env.time,
                        env.precache,
                        env.world,
                        &mut self.rng,
                        &mut self.sounds,
                    );
                }
            }
            SvcMessage::Sound(_)
            | SvcMessage::SpawnStaticSound(_)
            | SvcMessage::StopSound(_)
            | SvcMessage::SoundFade(_)
            | SvcMessage::RoomType(_)
            | SvcMessage::CdTrack { .. } => self.sounds.handle_with_local(
                msg,
                env.precache,
                (env.local_player > 0).then_some(env.local_player),
                env.prediction_active,
            ),
            SvcMessage::VoiceInit { codec, quality } => {
                self.voice.push(VoiceEvent::Init {
                    codec: codec.clone(),
                    quality: *quality,
                });
            }
            SvcMessage::VoiceData { player_slot, data } => {
                self.voice.push(VoiceEvent::Data {
                    player_slot: *player_slot,
                    data: data.clone(),
                });
            }
            SvcMessage::NewUserMsg(_)
            | SvcMessage::CenterPrint(_)
            | SvcMessage::WeaponAnim { .. }
            | SvcMessage::Director(_) => self.hud.handle(msg, env.time),
            SvcMessage::LightStyle { index, pattern } => {
                if let Some(slot) = self.hud.lightstyles.get_mut(usize::from(*index)) {
                    *slot = pattern.clone();
                }
            }
            SvcMessage::Particle(p) => {
                self.tempents.spawn_particle_msg(p);
            }
            SvcMessage::SpawnStatic(ent) => {
                if self.hud.static_entities.len() < 1024 {
                    self.hud.static_entities.push(**ent);
                }
            }
            SvcMessage::Customization(CustomizationMsg {
                player,
                res_type,
                filename,
                flags,
                md5,
                ..
            }) => {
                if *res_type == crate::resources::res_type::DECAL {
                    self.hud.custom_decals.insert(
                        *player,
                        CustomDecalInfo {
                            player: *player,
                            filename: filename.clone(),
                            flags: *flags,
                            md5: *md5,
                        },
                    );
                }
            }
            SvcMessage::UserMsg { decoded, .. } => {
                if let CsUserMsg::Brass(payload) = decoded {
                    self.tempents.spawn_brass_usermsg(
                        payload,
                        env.time,
                        env.precache,
                        env.world,
                        &mut self.rng,
                    );
                }
                self.hud.handle(msg, env.time);
            }
            _ => {}
        }
    }

    /// `CL_FireEvents` and `CL_TempEntUpdate`, which the engine runs right before drawing a
    /// frame: fires the events whose time has come, applies what they do to the temp entities
    /// and the sound queue, then steps the temp entities by the time since the last call.
    /// Returns the events that fired, for the shell to play the view-model animation, the muzzle
    /// flash and the bullets' impacts.
    pub fn update(&mut self, env: &FxEnv, gravity: f32) -> Vec<FiredEvent> {
        let fired = self.events.fire_due(env, &mut self.rng);
        for ev in &fired {
            match &ev.fx {
                EventFx::Weapon(fx) => {
                    if let Some(shell) = &fx.shell {
                        self.tempents.spawn_temp_model(
                            shell.origin,
                            shell.velocity,
                            Vec3::new(0.0, shell.yaw, 0.0),
                            2.5,
                            ModelRef::Client(shell.model),
                            shell.kind,
                            env.world,
                            &mut self.rng,
                            env.time,
                        );
                    }
                    if let Some(sound) = &fx.sound {
                        self.sounds.push(SoundCommand::Start(sound.clone()));
                    }
                }
                EventFx::Knife { sound, .. } => {
                    self.sounds.push(SoundCommand::Start(sound.clone()));
                }
                EventFx::Vehicle(command) => self.sounds.push(command.clone()),
                EventFx::DecalReset => {
                    self.tempents.clear_round();
                    self.hud.corpses.clear();
                }
                EventFx::CreateSmoke { first: true, args } => {
                    self.tempents.spawn_smoke_clouds_ex(
                        args.origin,
                        SmokeCloudConfig {
                            light_smoke: args.bparam1 != 0,
                            max_smokepuffs: self.max_smokepuffs,
                            fastsprites: self.fastsprites,
                        },
                        env.world,
                        &mut self.rng,
                        env.time,
                    );
                }
                EventFx::CreateSmoke { .. } | EventFx::CreateExplo | EventFx::Unhandled => {}
            }
        }
        let frametime = self
            .last_update
            .map_or(0.0, |t| (env.time - t).max(0.0) as f32);
        self.last_update = Some(env.time);
        self.tempents.update(
            TempEntClock {
                time: env.time,
                frametime,
                gravity,
            },
            env.world,
            &mut self.rng,
            &mut self.sounds,
        );
        fired
    }

    /// The decals, lights, beams and particle bursts asked for since the last call.
    pub fn take_render_effects(&mut self) -> Vec<RenderEffect> {
        self.tempents.take_effects()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempents::NoWorld;
    use cs_proto48::usermsg::UserMsgDesc;

    fn um(name: &str, decoded: CsUserMsg) -> SvcMessage {
        SvcMessage::UserMsg {
            id: 70,
            name: name.to_string(),
            decoded,
        }
    }

    #[test]
    fn the_reducer_keeps_the_values_the_server_sent() {
        let mut h = HudState::default();
        h.handle(
            &SvcMessage::NewUserMsg(UserMsgDesc {
                id: 70,
                size: 1,
                name: "Health".into(),
            }),
            0.0,
        );
        h.handle(&um("Health", CsUserMsg::Health(87)), 1.0);
        h.handle(&um("Battery", CsUserMsg::Battery(55)), 1.0);
        h.handle(
            &um(
                "Money",
                CsUserMsg::Money {
                    amount: 800,
                    flash: true,
                },
            ),
            1.0,
        );
        h.handle(
            &um(
                "CurWeapon",
                CsUserMsg::CurWeapon {
                    is_active: true,
                    weapon_id: 7,
                    clip_ammo: 30,
                },
            ),
            1.0,
        );
        h.handle(
            &um(
                "AmmoX",
                CsUserMsg::AmmoX {
                    ammo_id: 2,
                    amount: 90,
                },
            ),
            1.0,
        );
        h.handle(&um("RoundTime", CsUserMsg::RoundTime(115)), 10.0);
        assert_eq!(h.user_messages[&70].name, "Health");
        assert_eq!((h.health, h.armor, h.money), (87, 55, 800));
        assert!(h.money_flash);
        assert_eq!(
            h.current_weapon,
            Some(CurrentWeapon {
                is_active: true,
                weapon_id: 7,
                clip: 30
            })
        );
        assert_eq!(h.clips[&7], 30);
        assert_eq!(h.ammo[&2], 90);
        assert_eq!(h.round_seconds_left(25.0), 100.0);
        assert_eq!(h.round_seconds_left(1000.0), 0.0);
    }

    #[test]
    fn scores_teams_and_status_icons() {
        let mut h = HudState::default();
        h.handle(
            &um(
                "ScoreInfo",
                CsUserMsg::ScoreInfo {
                    player_id: 3,
                    frags: 5,
                    deaths: 2,
                    class_id: 1,
                    team_id: 2,
                },
            ),
            0.0,
        );
        h.handle(
            &um(
                "TeamInfo",
                CsUserMsg::TeamInfo {
                    player_id: 3,
                    team_name: "CT".into(),
                },
            ),
            0.0,
        );
        h.handle(
            &um(
                "ScoreAttrib",
                CsUserMsg::ScoreAttrib {
                    player_id: 3,
                    flags: 1,
                },
            ),
            0.0,
        );
        h.handle(
            &um(
                "StatusIcon",
                CsUserMsg::StatusIcon {
                    status: 1,
                    sprite_name: "buyzone".into(),
                    color: Some([0, 160, 0]),
                },
            ),
            0.0,
        );
        h.handle(
            &um(
                "StatusIcon",
                CsUserMsg::StatusIcon {
                    status: 1,
                    sprite_name: "defuser".into(),
                    color: None,
                },
            ),
            0.0,
        );
        h.handle(
            &um(
                "StatusIcon",
                CsUserMsg::StatusIcon {
                    status: 0,
                    sprite_name: "buyzone".into(),
                    color: None,
                },
            ),
            0.0,
        );
        let p = &h.players[&3];
        assert_eq!((p.frags, p.deaths, p.team_id, p.attrib), (5, 2, 2, 1));
        assert_eq!(p.team, "CT");
        assert_eq!(h.status_icons.len(), 1);
        assert!(h.status_icons.contains_key("defuser"));
    }

    #[test]
    fn logs_menus_bars_and_the_motd() {
        let mut h = HudState::default();
        h.handle(
            &um(
                "DeathMsg",
                CsUserMsg::DeathMsg {
                    killer_id: 1,
                    victim_id: 2,
                    headshot: true,
                    weapon: "ak47".into(),
                },
            ),
            0.0,
        );
        h.handle(
            &um(
                "TextMsg",
                CsUserMsg::TextMsg {
                    dest: 4,
                    message: "#Game_scoring".into(),
                    params: vec![],
                },
            ),
            0.0,
        );
        h.handle(
            &um(
                "SayText",
                CsUserMsg::SayText {
                    sender_id: 2,
                    text: "hi".into(),
                    params: vec!["x".into()],
                },
            ),
            0.0,
        );
        h.handle(&um("BarTime", CsUserMsg::BarTime(5)), 7.0);
        h.handle(
            &um(
                "ShowMenu",
                CsUserMsg::ShowMenu {
                    keys_bitmask: 0x3ff,
                    display_time: -1,
                    multipart: true,
                    text: "Choose ".into(),
                },
            ),
            0.0,
        );
        h.handle(
            &um(
                "ShowMenu",
                CsUserMsg::ShowMenu {
                    keys_bitmask: 0x3ff,
                    display_time: -1,
                    multipart: false,
                    text: "a team".into(),
                },
            ),
            0.0,
        );
        for (finished, text) in [(false, "Welcome "), (true, "to the server")] {
            h.handle(
                &um(
                    "MOTD",
                    CsUserMsg::Motd {
                        finished,
                        text: text.into(),
                    },
                ),
                0.0,
            );
        }
        assert!(h.deaths[0].headshot);
        assert_eq!(h.text[0].dest, 4);
        assert_eq!(h.chat[0].params, vec!["x".to_string()]);
        assert_eq!(h.bar.unwrap().duration, 5);
        assert_eq!(h.menu.as_ref().unwrap().text, "Choose a team");
        assert!(!h.menu.as_ref().unwrap().multipart);
        assert_eq!(h.motd, "Welcome to the server");
        assert!(h.motd_complete);
        h.handle(&um("BarTime", CsUserMsg::BarTime(0)), 9.0);
        assert!(h.bar.is_none());
        h.handle(&um("BuyClose", CsUserMsg::BuyClose), 9.0);
        assert!(h.menu.is_none());
        // A second MOTD starts over.
        h.handle(
            &um(
                "MOTD",
                CsUserMsg::Motd {
                    finished: true,
                    text: "new".into(),
                },
            ),
            0.0,
        );
        assert_eq!(h.motd, "new");
    }

    #[test]
    fn reset_hud_clears_what_a_life_drew() {
        let mut h = HudState::default();
        h.handle(&um("Health", CsUserMsg::Health(100)), 0.0);
        h.handle(&um("BarTime", CsUserMsg::BarTime(3)), 0.0);
        h.handle(
            &um(
                "Radar",
                CsUserMsg::Radar {
                    player_id: 1,
                    origin: Vec3::X,
                },
            ),
            0.0,
        );
        h.handle(&SvcMessage::CenterPrint("hello".into()), 0.0);
        h.handle(&um("ResetHUD", CsUserMsg::ResetHud), 0.0);
        assert!(h.bar.is_none() && h.radar.is_empty() && h.center_print.is_none());
        assert_eq!(h.health, 100);
        assert_eq!(h.hud_resets, 1);
    }

    #[test]
    fn unknown_usermsg_is_kept() {
        let mut h = HudState::default();
        h.handle(
            &um(
                "ModThing",
                CsUserMsg::Unknown {
                    name: "ModThing".into(),
                    payload: vec![1, 2, 3],
                },
            ),
            0.0,
        );
        assert_eq!(h.unknown_total, 1);
        assert_eq!(
            h.unknown_messages[0],
            UnknownUserMsg {
                id: 70,
                name: "ModThing".into(),
                payload: vec![1, 2, 3]
            }
        );
        // The log is bounded; the count is not.
        for _ in 0..UNKNOWN_CAPACITY + 5 {
            h.handle(
                &um(
                    "ModThing",
                    CsUserMsg::Unknown {
                        name: "ModThing".into(),
                        payload: vec![],
                    },
                ),
                0.0,
            );
        }
        assert_eq!(h.unknown_messages.len(), UNKNOWN_CAPACITY);
        assert_eq!(h.unknown_total, UNKNOWN_CAPACITY as u64 + 6);
    }

    #[test]
    fn update_fires_events_into_tempents_and_sounds() {
        use crate::events::{FxEnv, fev};
        use crate::tempents::ftent;
        use cs_proto48::event::EventArgs;
        use cs_proto48::messages::ResourceEntry;

        let mut precache = crate::precache::PrecacheTables::default();
        precache.add(&[ResourceEntry {
            res_type: crate::resources::res_type::EVENTSCRIPT,
            filename: "events/ak47.sc".into(),
            index: 1,
            download_size: 0,
            flags: 0,
            md5: None,
            reserved: None,
        }]);
        let entities = BTreeMap::new();
        let env = FxEnv::new(5.0, &precache, &entities, &NoWorld);
        let mut fx = ClientEffects::with_seed(1);
        fx.events.queue(
            fev::SERVER,
            1,
            0.0,
            5.0,
            EventArgs {
                entindex: 4,
                ..Default::default()
            },
            Vec3::ZERO,
        );
        let fired = fx.update(&env, 800.0);
        assert_eq!(fired.len(), 1);
        assert_eq!(fx.tempents.active().len(), 1);
        let shell = &fx.tempents.active()[0];
        assert!(shell.has(ftent::GRAVITY));
        assert_eq!(shell.model, ModelRef::Client("models/rshell.mdl"));
        assert_eq!(fx.sounds.pending().len(), 1);
        // The shell lives 2.5 seconds, as `EV_EjectBrass` asks.
        assert_eq!(shell.die, 7.5);
    }

    fn server_info() -> SvcMessage {
        SvcMessage::ServerInfo(Box::new(cs_proto48::messages::ServerInfoMsg {
            protocol: 48,
            spawn_count: 1,
            map_crc: 0,
            client_dll_md5: [0; 16],
            max_clients: 8,
            player_slot: 0,
            is_deathmatch: true,
            game_dir: "cstrike".into(),
            hostname: String::new(),
            map_path: "maps/x.bsp".into(),
            map_cycle: String::new(),
            vac_secure: false,
        }))
    }

    #[test]
    fn a_new_server_resets_the_hud_and_the_effects() {
        let precache = crate::precache::PrecacheTables::default();
        let entities = BTreeMap::new();
        let env = FxEnv::new(1.0, &precache, &entities, &NoWorld);
        let mut fx = ClientEffects::with_seed(1);
        fx.hud.handle(&um("Health", CsUserMsg::Health(50)), 0.0);
        fx.tempents
            .spawn_smoke_clouds(Vec3::ZERO, &NoWorld, &mut FxRandom::new(1), 0.0);
        fx.handle(&server_info(), &env);
        assert_eq!(fx.hud.health, 0);
        assert!(fx.tempents.active().is_empty());
    }

    #[test]
    fn a_new_round_clears_the_corpses() {
        let mut precache = crate::precache::PrecacheTables::default();
        precache.add(&[cs_proto48::messages::ResourceEntry {
            res_type: crate::resources::res_type::EVENTSCRIPT,
            filename: "events/decal_reset.sc".into(),
            index: 1,
            download_size: 0,
            flags: 0,
            md5: None,
            reserved: None,
        }]);
        let entities = BTreeMap::new();
        let env = FxEnv::new(1.0, &precache, &entities, &NoWorld);
        let mut fx = ClientEffects::with_seed(1);
        fx.hud.handle(
            &um(
                "ClCorpse",
                CsUserMsg::ClCorpse {
                    model: "leet".into(),
                    origin: [0; 3],
                    angles: Vec3::ZERO,
                    time_diff: 0,
                    sequence: 0,
                    body: 0,
                    team: 1,
                    player_id: 2,
                },
            ),
            0.0,
        );
        assert_eq!(fx.hud.corpses.len(), 1);
        fx.events.queue(
            crate::events::fev::SERVER,
            1,
            0.0,
            1.0,
            Default::default(),
            Vec3::ZERO,
        );
        fx.update(&env, 800.0);
        assert!(fx.hud.corpses.is_empty());
    }

    #[test]
    fn the_hud_collections_are_capped() {
        let mut h = HudState::default();
        let long = "x".repeat(MAX_MOTD_BYTES);
        for _ in 0..3 {
            h.handle(
                &um(
                    "MOTD",
                    CsUserMsg::Motd {
                        finished: false,
                        text: long.clone(),
                    },
                ),
                0.0,
            );
        }
        assert_eq!(h.motd.len(), MAX_MOTD_BYTES);
        for i in 0..MAX_STATUS_ICONS + 10 {
            h.handle(
                &um(
                    "StatusIcon",
                    CsUserMsg::StatusIcon {
                        status: 1,
                        sprite_name: format!("icon{i}"),
                        color: None,
                    },
                ),
                0.0,
            );
        }
        assert_eq!(h.status_icons.len(), MAX_STATUS_ICONS);
        for i in 0..MAX_TEAM_SCORES + 10 {
            h.handle(
                &um(
                    "TeamScore",
                    CsUserMsg::TeamScore {
                        team_name: format!("team{i}"),
                        score: 1,
                    },
                ),
                0.0,
            );
        }
        assert_eq!(h.team_scores.len(), MAX_TEAM_SCORES);
        // A team already known still updates.
        h.handle(
            &um(
                "TeamScore",
                CsUserMsg::TeamScore {
                    team_name: "team0".into(),
                    score: 9,
                },
            ),
            0.0,
        );
        assert_eq!(h.team_scores["team0"], 9);
        let chunk = "m".repeat(MAX_MENU_BYTES / 2 + 1);
        for _ in 0..4 {
            h.handle(
                &um(
                    "ShowMenu",
                    CsUserMsg::ShowMenu {
                        keys_bitmask: 1,
                        display_time: -1,
                        multipart: true,
                        text: chunk.clone(),
                    },
                ),
                0.0,
            );
        }
        assert!(h.menu.as_ref().unwrap().text.len() <= MAX_MENU_BYTES);
    }

    #[test]
    fn centre_text_sets_the_centre_print() {
        let mut h = HudState::default();
        h.handle(
            &um(
                "TextMsg",
                CsUserMsg::TextMsg {
                    dest: 4,
                    message: "Round start".into(),
                    params: vec![],
                },
            ),
            0.0,
        );
        assert_eq!(h.center_print.as_deref(), Some("Round start"));
    }

    #[test]
    fn sounds_and_temp_entities_go_where_they_belong() {
        let precache = crate::precache::PrecacheTables::default();
        let entities = BTreeMap::new();
        let env = FxEnv::new(1.0, &precache, &entities, &NoWorld);
        let mut fx = ClientEffects::with_seed(2);
        fx.handle(&SvcMessage::StopSound((9 << 3) | 2), &env);
        assert_eq!(
            fx.sounds.pending(),
            &[SoundCommand::Stop {
                entity: 9,
                channel: 2
            }]
        );
        let te = cs_proto48::tempent::sparks(Vec3::new(8.0, 8.0, 8.0));
        fx.handle(
            &SvcMessage::TempEntity {
                te_type: te.te_type,
                raw: te.payload,
            },
            &env,
        );
        assert_eq!(fx.tempents.active().len(), 1);
    }

    #[test]
    fn local_predicted_footsteps_suppressed_end_to_end_in_client_effects() {
        let mut precache = crate::precache::PrecacheTables::default();
        precache.sounds.insert(
            1,
            crate::precache::PrecacheEntry {
                name: "player/pl_step1.wav".into(),
                download_size: 0,
                flags: 0,
            },
        );
        let entities = BTreeMap::new();
        let step = |entity: u16| {
            SvcMessage::Sound(cs_proto48::messages::SoundMsg {
                flags: 0,
                volume: 128,
                attenuation: 0.8,
                channel: 4,
                entity,
                sound_index: 1,
                origin: Vec3::ZERO,
                pitch: 100,
            })
        };

        let mut env = FxEnv::new(1.0, &precache, &entities, &NoWorld);
        env.local_player = 2;
        env.prediction_active = true;
        let mut fx = ClientEffects::with_seed(3);
        fx.handle(&step(2), &env);
        assert!(fx.sounds.pending().is_empty());
        assert_eq!(fx.sounds.suppressed_local_steps, 1);

        // Remote player's footstep still passes through.
        fx.handle(&step(3), &env);
        assert_eq!(fx.sounds.pending().len(), 1);
        fx.sounds.clear();

        // With prediction disabled (`cl_nopred 1` or demo playback), local footsteps play.
        env.prediction_active = false;
        fx.handle(&step(2), &env);
        assert_eq!(fx.sounds.pending().len(), 1);
    }

    #[test]
    fn inactive_cur_weapon_does_not_clobber_active_weapon() {
        let mut h = HudState::default();
        h.handle(
            &um(
                "CurWeapon",
                CsUserMsg::CurWeapon {
                    is_active: true,
                    weapon_id: 16,
                    clip_ammo: 12,
                },
            ),
            0.0,
        );
        h.handle(
            &um(
                "CurWeapon",
                CsUserMsg::CurWeapon {
                    is_active: false,
                    weapon_id: 29,
                    clip_ammo: -1,
                },
            ),
            0.0,
        );
        assert_eq!(
            h.current_weapon,
            Some(CurrentWeapon {
                is_active: true,
                weapon_id: 16,
                clip: 12,
            })
        );
        assert_eq!(h.clips.get(&29), Some(&-1));

        h.handle(
            &um(
                "CurWeapon",
                CsUserMsg::CurWeapon {
                    is_active: false,
                    weapon_id: 0,
                    clip_ammo: 0,
                },
            ),
            0.0,
        );
        assert!(h.current_weapon.is_none());
    }

    #[test]
    fn reset_menu_clears_open_menu_while_multipart_prefix_stays_open() {
        let mut h = HudState::default();
        h.handle(
            &um(
                "ShowMenu",
                CsUserMsg::ShowMenu {
                    keys_bitmask: 0b11,
                    display_time: 10,
                    multipart: false,
                    text: "1. CT\n2. T\n".to_string(),
                },
            ),
            0.0,
        );
        assert!(h.menu.is_some());

        h.handle(
            &um(
                "ShowMenu",
                CsUserMsg::ShowMenu {
                    keys_bitmask: 0,
                    display_time: 0,
                    multipart: false,
                    text: String::new(),
                },
            ),
            0.0,
        );
        assert!(h.menu.is_none());

        h.handle(
            &um(
                "ShowMenu",
                CsUserMsg::ShowMenu {
                    keys_bitmask: 0b11,
                    display_time: 10,
                    multipart: true,
                    text: String::new(),
                },
            ),
            0.0,
        );
        assert!(h.menu.as_ref().is_some_and(|m| m.multipart));
    }

    #[test]
    fn phase8_hud_and_effect_messages_update_state() {
        let mut fx = ClientEffects::default();
        let precache = crate::precache::PrecacheTables::default();
        let entities = BTreeMap::new();
        let env = FxEnv::new(1.5, &precache, &entities, &crate::tempents::NoWorld);

        fx.handle(
            &SvcMessage::LightStyle {
                index: 12,
                pattern: "azm".to_string(),
            },
            &env,
        );
        let vals_t0 = evaluate_lightstyles(&fx.hud.lightstyles, 0.0);
        let vals_t1 = evaluate_lightstyles(&fx.hud.lightstyles, 0.1);
        let vals_t2 = evaluate_lightstyles(&fx.hud.lightstyles, 0.2);
        assert_eq!(vals_t0[12], 0.0);
        assert!((vals_t1[12] - 25.0 / 12.0).abs() < 1e-4);
        assert!((vals_t2[12] - 1.0).abs() < 1e-4);

        fx.handle(
            &SvcMessage::Customization(CustomizationMsg {
                player: 2,
                res_type: crate::resources::res_type::DECAL,
                filename: "!MD5deadbeef".to_string(),
                index: 0,
                download_size: 512,
                flags: 1,
                md5: Some([0xAB; 16]),
            }),
            &env,
        );
        assert_eq!(
            fx.hud.custom_decals.get(&2).map(|c| c.filename.as_str()),
            Some("!MD5deadbeef")
        );

        fx.hud.handle(
            &um(
                "Fog",
                CsUserMsg::Fog {
                    color: [10, 20, 30],
                    density: 0.004,
                },
            ),
            1.5,
        );
        assert_eq!(
            fx.hud.fog,
            Some(FogState {
                color: [10, 20, 30],
                density: 0.004,
            })
        );
        fx.hud.handle(
            &um(
                "Flashlight",
                CsUserMsg::Flashlight {
                    on: true,
                    battery: 85,
                },
            ),
            1.5,
        );
        assert!(fx.hud.flashlight_on);
        assert_eq!(fx.hud.flash_battery, 85);
        fx.hud.handle(&um("FlashBat", CsUserMsg::FlashBat(70)), 1.6);
        assert_eq!(fx.hud.flash_battery, 70);
        fx.hud.handle(&um("ReceiveW", CsUserMsg::ReceiveW(2)), 1.6);
        assert_eq!(fx.hud.weather, 2);
    }

    #[test]
    fn phase10_hud_text_spec_and_damage_history_reduce() {
        let mut h = HudState::default();
        h.handle(
            &um(
                "HudTextArgs",
                CsUserMsg::HudTextArgs {
                    text: "#Cstrike_Tutor_Buy_Time".to_string(),
                    style: 1,
                    args: vec!["45".to_string()],
                },
            ),
            2.0,
        );
        h.handle(
            &um(
                "HudText",
                CsUserMsg::HudText {
                    text: "GAMEOVER".to_string(),
                    style: 0,
                },
            ),
            2.1,
        );
        assert_eq!(h.hud_texts.len(), 2);
        assert_eq!(h.hud_texts[0].args, vec!["45".to_string()]);

        h.handle(&um("AllowSpec", CsUserMsg::AllowSpec(true)), 2.2);
        assert!(h.allow_spec);
        h.handle(
            &um(
                "SpecHealth2",
                CsUserMsg::SpecHealth2 {
                    health: 88,
                    target: 3,
                },
            ),
            2.3,
        );
        assert_eq!(h.spec_health, Some((88, 3)));

        h.handle(
            &um(
                "Damage",
                CsUserMsg::Damage {
                    armor_damage: 5,
                    health_damage: 27,
                    damage_bits: 0x04,
                    origin: Vec3::new(300.0, 100.0, 0.0),
                },
            ),
            2.4,
        );
        assert_eq!(h.damage_history.len(), 1);
        assert_eq!(h.damage_history[0].0.health, 27);
    }

    #[test]
    fn send_audio_sets_radio_flash_for_positive_sender() {
        let mut h = HudState::default();
        h.handle(
            &um(
                "SendAudio",
                CsUserMsg::SendAudio {
                    sender_id: 3,
                    audio_code: "%!MRAD_GO".to_string(),
                    pitch: 100,
                },
            ),
            10.0,
        );
        assert_eq!(h.radio_flashes.get(&3), Some(&(10.0 + 3.3)));

        h.handle(
            &um(
                "SendAudio",
                CsUserMsg::SendAudio {
                    sender_id: 0,
                    audio_code: "%!MRAD_ctwin".to_string(),
                    pitch: 100,
                },
            ),
            11.0,
        );
        assert!(!h.radio_flashes.contains_key(&0));

        h.handle(&um("ResetHUD", CsUserMsg::ResetHud), 12.0);
        assert!(h.radio_flashes.is_empty());
    }

    #[test]
    fn health_info_account_req_state_and_force_cam_reduce() {
        let mut h = HudState::default();
        h.handle(
            &um(
                "HealthInfo",
                CsUserMsg::HealthInfo {
                    player_id: 2,
                    health: 73,
                },
            ),
            1.0,
        );
        h.handle(
            &um(
                "Account",
                CsUserMsg::Account {
                    player_id: 2,
                    amount: 4250,
                },
            ),
            1.0,
        );
        h.handle(&um("ReqState", CsUserMsg::ReqState), 1.0);
        h.handle(
            &um(
                "ForceCam",
                CsUserMsg::ForceCam {
                    forcecam: 1,
                    forcechasecam: 0,
                    fadetoblack: 0,
                },
            ),
            1.0,
        );
        assert_eq!(h.player_health.get(&2).copied(), Some(73));
        assert_eq!(h.player_money.get(&2).copied(), Some(4250));
        assert_eq!(h.req_state_count, 1);
        assert_eq!(h.force_cam, Some((1, 0, 0)));
    }

    #[test]
    fn te_text_message_channels_and_director_dhud_decode_and_expire() {
        let mut h = HudState::default();
        // Channel 1 replaces channel 1 without clobbering channel 2.
        let mk_te = |channel: u8, text: &str, hold_sec: u16, received_at: f64| TeTextMessage {
            channel,
            x: -1,
            y: 2048,
            effect: 0,
            color1: [255, 176, 0, 255],
            color2: [255, 255, 255, 255],
            fadein: 26,
            fadeout: 51,
            holdtime: hold_sec * 256,
            fxtime: None,
            text: text.to_string(),
            received_at,
        };
        h.push_te_text_message(mk_te(1, "FIRST", 2, 1.0), 1.0);
        h.push_te_text_message(mk_te(2, "SECOND", 2, 1.0), 1.0);
        h.push_te_text_message(mk_te(1, "REPLACED", 2, 1.2), 1.2);
        assert_eq!(h.te_text_messages.len(), 2);
        assert!(h.te_text_messages.iter().any(|m| m.text == "REPLACED"));
        assert!(h.te_text_messages.iter().any(|m| m.text == "SECOND"));

        // AMXX / mod `SVC_DIRECTOR` `DRC_CMD_MESSAGE` (LE 0x00RRGGBB color).
        let mut drc = Vec::new();
        drc.push(cs_proto48::messages::drc::CMD_MESSAGE);
        drc.push(0); // fade effect
        let packed: u32 = (255 << 16) | (120 << 8) | 40;
        drc.extend_from_slice(&packed.to_le_bytes());
        for f in [-1.0f32, 0.25, 0.1, 0.2, 2.0, 0.0] {
            drc.extend_from_slice(&f.to_le_bytes());
        }
        drc.extend_from_slice(b"RETAKES BOMBSITE A\0");
        h.handle(&SvcMessage::Director(drc), 1.5);
        let dhud = h
            .te_text_messages
            .iter()
            .find(|m| m.text == "RETAKES BOMBSITE A")
            .expect("decoded DRC_CMD_MESSAGE");
        assert_eq!(dhud.channel, 0);
        assert_eq!(dhud.color1, [255, 120, 40, 255]);
        assert_eq!(dhud.x, -1);
        assert_eq!(dhud.y, 2048);

        // Expiry cleans up after fadein + hold + fadeout.
        h.prune_te_text_messages(6.0);
        assert!(h.te_text_messages.is_empty());
    }
}
