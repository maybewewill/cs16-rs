//! The small HUD of the game: the radio (`radio1`/`radio2`/`radio3`, their menus and aliases),
//! the radar, the status bar that names the player under the crosshair, and the night vision
//! toggle. Ported from ReGameDLL_CS `player.cpp` (`CBasePlayer::Radio`, `UpdateClientData`'s
//! radar and status bar calls, `UpdateStatusBar`, `InitStatusBar`, `Spawn`, `Killed`),
//! `client.cpp` (`Radio1`..`Radio3`, `HandleRadioAliasCommands`, `ClientCommand`'s `radio1..3`,
//! `nightvision`, `drop` and `ignorerad`), `gamerules.h` (`RADIO_TIMEOUT`), `game.h`/`game.cpp`
//! (`mp_playerid`, `PLAYERID_MODE_*`), `player.h` (`MAX_ID_RANGE`, `SBAR_*`) and `weapons.cpp`
//! (`W_Precache`'s `radio.spr`), read 2026-11-22: the vanilla `mp.dll` behaviour, not the
//! `REGAMEDLL_ADD`/`REGAMEDLL_FIXES` builds (no `radio_timeout`, `radio_maxinround`,
//! `show_radioicon`, `playerid_showhealth`, `playerid_field`, no `zmax` id range).
//!
//! ## What is sent
//!
//! - Radio: to every living or dead teammate of the speaker (the speaker too) that has not set
//!   `ignorerad`, a `SendAudio` (sender edict index, the sentence `%!MRAD_*`, pitch 100), a
//!   `TextMsg` to `HUD_PRINTRADIO` (the destination 5) whose message is the sender's edict index
//!   as text and whose parameters are `#Game_radio`, the sender's name and the text key
//!   (`ClientPrint(pev, HUD_PRINTRADIO, NumAsString(entindex()), "#Game_radio", name, text)`),
//!   and the icon over the speaker's head (`TE_PLAYERATTACHMENT`: entity index, vertical offset
//!   35, `sprites/radio.spr`, life 15 tenths of a second). A player may speak once every 1.5
//!   seconds and 60 times a spawn (`m_flRadioTime`, `m_iRadioMessages`, reset by `Spawn`); the
//!   dead and spectators say nothing.
//! - Radar: `UpdateClientData` tells the living teammates where the player is, at most once a
//!   second and only after it has moved 64 units; `InitHUD` sends the newcomer everyone's
//!   position (see [`crate::messages`]).
//! - Status bar: every 0.2 seconds a line of 2048 units from the eye along the view is traced; a
//!   player it hits is named to the viewer by `StatusText` (the format) and `StatusValue` (the
//!   target type, its edict index and, for a teammate, its health in percent), kept on the
//!   screen for 2 seconds after the player leaves the crosshair.
//! - Night vision: `nightvision` toggles `NVGToggle` and plays `items/nvg_on.wav` or
//!   `items/nvg_off.wav` for a player that has it (`has_night_vision`, which the buy menu sets);
//!   a spawn and a death switch it off.
//!
//! ## Differences from the original, each deliberate
//!
//! - Observers (all ported from the source): a spectator in `OBS_CHASE_LOCKED`, `OBS_CHASE_FREE`
//!   or `OBS_IN_EYE` with a target hears the radio of the speaker when its target is the speaker
//!   or a teammate (player.cpp:446-464); its status bar uses the teammate format and health for
//!   any target and looks 8192 units (player.cpp:8288, 8314-8316); the players that watch a
//!   player in first person get its night vision toggled with the sound (client.cpp:3466-3512),
//!   switched off at its spawn (player.cpp:6066-6081) and, at its death, by [`crate::observer`].
//!   The radar sends nothing to observers (only living players are told, player.cpp:7939), and
//!   `nightvision` itself is for a living player only (client.cpp:3445).
//! - `HintMessage` (`#Hint_spotted_a_friend`, `#Hint_spotted_an_enemy`) and the hostage branch of
//!   the status bar need the hint queue and the hostages; they are not sent. `m_blindUntilTime`
//!   (a flashbang) is never in the future yet.
//! - `Location` (`UpdateLocation`, the place name of the player) is sent only by the bots and
//!   Condition Zero (`AreRunningCZero`, the bot phrase list); the vanilla radio shows `#Game_radio`,
//!   never `#Game_radio_location`. It is not sent.
//! - `drop` does nothing until the weapons (`DropPlayerItem`) exist; it is accepted and ignored.
//! - The status bar tests the target with the player entity of the trace; the original also asks
//!   the bots (`TheBots->IsLineBlockedBySmoke`), which do not exist here.
//! - `pev->max_health` of a player is 100 (set by `Spawn`); the edict has no such field here.

use cs_engine::consts::{FL_DORMANT, FL_SPECTATOR};
use cs_engine::{EngineApi, MoveFilter, MsgDest};
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

use crate::GameState;
use crate::chat::HUD_PRINTCENTER;
use crate::menus::{
    self, MENU_KEY_0, MENU_KEY_1, MENU_KEY_2, MENU_KEY_3, MENU_KEY_4, MENU_KEY_5, MENU_KEY_6,
    MENU_KEY_7, MENU_KEY_8, MENU_KEY_9, Menu,
};
use crate::observer::{OBS_CHASE_FREE, OBS_CHASE_LOCKED, OBS_IN_EYE, OBS_NONE};
use crate::player::{ATTN_NORM, CHAN_ITEM, DEAD_NO, PITCH_NORM, player_slot};

/// `HUD_PRINTRADIO` (`util.h`): the `TextMsg` destination of a radio line.
pub const HUD_PRINTRADIO: u8 = 5;
/// `RADIO_TIMEOUT` (`gamerules.h`): seconds a player must wait between two radio messages.
const RADIO_TIMEOUT: f64 = 1.5;
/// `m_iRadioMessages` after a spawn (the source's `60` when `REGAMEDLL_ADD` is off).
pub const RADIO_MESSAGES_PER_SPAWN: i32 = 60;
/// `TE_PLAYERATTACHMENT`: the temp entity that hangs a sprite on a player.
const TE_PLAYERATTACHMENT: u8 = 124;
/// The vertical offset and the life (tenths of a second) of the radio icon.
const RADIO_ICON_OFFSET: f32 = 35.0;
const RADIO_ICON_LIFE: u16 = 15;
/// The sprite of the radio icon (`g_sModelIndexRadio`, precached by `W_Precache`).
pub(crate) const RADIO_SPRITE: &str = "sprites/radio.spr";

/// `MAX_ID_RANGE` (`player.h`): how far the status bar looks.
const MAX_ID_RANGE: f32 = 2048.0;
/// `MAX_SPEC_ID_RANGE` (`player.h`): how far a spectator looks.
const MAX_SPEC_ID_RANGE: f32 = 8192.0;
/// Seconds between two status bar updates, and how long a name stays after the crosshair left.
const SBAR_UPDATE_INTERVAL: f64 = 0.2;
const SBAR_HOLD: f64 = 2.0;
/// `SBAR_ID_TARGETTYPE`, `SBAR_ID_TARGETNAME`, `SBAR_ID_TARGETHEALTH` and `SBAR_END`.
const SBAR_ID_TARGETTYPE: usize = 1;
const SBAR_ID_TARGETNAME: usize = 2;
const SBAR_ID_TARGETHEALTH: usize = 3;
const SBAR_END: usize = 4;
/// `SBAR_TARGETTYPE_TEAMMATE` and `SBAR_TARGETTYPE_ENEMY`.
const SBAR_TARGETTYPE_TEAMMATE: i32 = 1;
const SBAR_TARGETTYPE_ENEMY: i32 = 2;
/// `PLAYERID_MODE_TEAMONLY` and `PLAYERID_MODE_OFF` (`game.h`); `mp_playerid` defaults to 0
/// (`PLAYERID_MODE_EVERYONE`).
const PLAYERID_MODE_TEAMONLY: f32 = 1.0;
const PLAYERID_MODE_OFF: f32 = 2.0;
/// The status bar formats of `UpdateStatusBar`: `%c1` the name of the target (`StatusValue` 2 is
/// its player index), `%p2` the player name, `%h` "health", `%i3` the percentage.
const SBAR_FORMAT_TEAMMATE: &str = "1 %c1: %p2\n2  %h: %i3%%";
const SBAR_FORMAT_ENEMY: &str = "1 %c1: %p2";
const SBAR_FORMAT_NONE: &str = " ";

/// How far a player must have moved for the radar to tell the team again, and how often it may.
const RADAR_TOLERANCE: f32 = 64.0;
const RADAR_INTERVAL: f64 = 1.0;
/// Seconds before `nightvision` is accepted again (`m_flLastCommandTime[CMD_NIGHTVISION]`).
const NIGHTVISION_INTERVAL: f64 = 0.3;

/// What the game keeps of a player for the radio, the radar, the status bar and the night
/// vision.
#[derive(Debug, Clone)]
pub struct HudState {
    /// `m_flRadioTime`: server time before which the player may not use the radio.
    pub radio_time: f64,
    /// `m_iRadioMessages`: radio messages left until the next spawn.
    pub radio_messages: i32,
    /// `m_bIgnoreRadio`: the player hears no radio (`ignorerad`).
    pub ignore_radio: bool,
    /// `m_tmNextRadarUpdate`.
    pub(crate) next_radar_update: f64,
    /// `m_vLastOrigin`: where the radar last saw the player.
    pub(crate) last_origin: Vec3,
    /// `m_flNextSBarUpdateTime`.
    next_sbar_update: f64,
    /// `m_izSBarState`: the values the client has for the status bar.
    sbar_state: [i32; SBAR_END],
    /// `m_SbarString0`: the format the client has.
    sbar_string: String,
    /// `m_flStatusBarDisappearDelay`: until when the target stays on the bar once it left the
    /// crosshair.
    sbar_disappear: f64,
    /// `m_flLastCommandTime[CMD_NIGHTVISION]`.
    next_nightvision: f64,
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            radio_time: 0.0,
            radio_messages: RADIO_MESSAGES_PER_SPAWN,
            ignore_radio: false,
            next_radar_update: 0.0,
            last_origin: Vec3::ZERO,
            next_sbar_update: 0.0,
            sbar_state: [0; SBAR_END],
            sbar_string: String::new(),
            sbar_disappear: 0.0,
            next_nightvision: 0.0,
        }
    }
}

/// `CBasePlayer::Spawn`'s part for this module: the radio starts over (`m_flRadioTime = 0`,
/// `m_iRadioMessages = 60`), the radar is due at once from the origin (`m_tmNextRadarUpdate =
/// time`, `m_vLastOrigin = 0`), the status bar forgets its format and its hold (`InitStatusBar`;
/// `m_izSBarState` is kept, as in the original) and the night vision goes off (`NVGToggle 0`).
pub(crate) fn on_spawn(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time();
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    p.hud.radio_time = 0.0;
    p.hud.radio_messages = RADIO_MESSAGES_PER_SPAWN;
    p.hud.next_radar_update = now;
    p.hud.last_origin = Vec3::ZERO;
    p.hud.sbar_disappear = 0.0;
    p.hud.sbar_string.clear();
    // `m_flLastCommandTime[i] = -1` of `Spawn`.
    p.hud.next_nightvision = -1.0;
    switch_night_vision_off(g, eng, slot);
}

/// The observers that watch `slot` in first person (`IsObservingPlayer`: `OBS_IN_EYE` and
/// `iuser2` its edict index; the original skips an edict whose flags are exactly `FL_DORMANT`).
fn followers(g: &GameState, eng: &EngineApi<'_>, slot: u8) -> Vec<u8> {
    let index = i32::from(slot) + 1;
    (0..g.players.len() as u8)
        .filter(|&i| {
            g.players[usize::from(i)].is_some()
                && eng.ent(eng.client_edict(i)).is_some_and(|v| {
                    v.flags != FL_DORMANT && v.iuser1 == OBS_IN_EYE && v.iuser2 == index
                })
        })
        .collect()
}

/// Tells the client its night vision is off and records it, and does the same for the
/// observers that watch it in first person (`Spawn`, player.cpp:6060-6081).
fn switch_night_vision_off(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let mut who = vec![slot];
    who.extend(followers(g, eng, slot));
    for s in who {
        eng.send_usermsg(MsgDest::One(s), &CsUserMsg::NvgToggle(false));
        if let Some(p) = g.players.get_mut(usize::from(s)).and_then(Option::as_mut) {
            p.night_vision_on = false;
        }
    }
}

/// A float in `lo..=hi` derived from the game's own `random_range` (the engine offers the game no
/// `RANDOM_FLOAT`); the original's `RANDOM_FLOAT(lo, hi)` is the engine's generator.
fn random_float(g: &mut GameState, lo: f32, hi: f32) -> f32 {
    lo + (hi - lo) * (g.random_range(0, 65535) as f32 / 65535.0)
}

/// The commands of this module (`ClientCommand_` of ReGameDLL `client.cpp`): `ignorerad`, and
/// for a living player `nightvision`, `radio1`, `radio2`, `radio3`, `drop` and the radio
/// aliases. Returns whether `argv[0]` was one of them.
pub(crate) fn client_command(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    argv: &[String],
) -> bool {
    let Some(command) = argv.first().map(String::as_str) else {
        return false;
    };
    if command == "ignorerad" {
        ignore_radio_command(g, eng, slot);
        return true;
    }
    // Everything else is for a player with `deadflag == DEAD_NO`; a dead one's commands fall
    // through to the unknown-command message the game does not send here.
    let alive = eng
        .ent(eng.client_edict(slot))
        .is_some_and(|v| v.deadflag == DEAD_NO);
    if !alive {
        return false;
    }
    match command {
        "nightvision" => night_vision(g, eng, slot),
        "radio1" => open_radio_menu(g, eng, slot, 1),
        "radio2" => open_radio_menu(g, eng, slot, 2),
        "radio3" => open_radio_menu(g, eng, slot, 3),
        // `DropPlayerItem` / `DropShield`: the weapons come with phase 2D.
        "drop" => {}
        _ => {
            let Some(&(group, choice, _)) = RADIO_ALIASES.iter().find(|(_, _, a)| *a == command)
            else {
                return false;
            };
            radio_choice(g, eng, slot, group, choice);
        }
    }
    true
}

/// `ignorerad`: toggles whether the player hears the radio and says which.
fn ignore_radio_command(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    p.hud.ignore_radio = !p.hud.ignore_radio;
    let key = if p.hud.ignore_radio {
        "#Ignore_Radio"
    } else {
        "#Accept_Radio"
    };
    crate::chat::client_print(eng, slot, HUD_PRINTCENTER, key, &[]);
}

/// `radio1`, `radio2` and `radio3`: the text menu of the group, and the player is in it.
fn open_radio_menu(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, group: u8) {
    let (keys, text, menu) = match group {
        1 => (
            MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_3 | MENU_KEY_4 | MENU_KEY_5 | MENU_KEY_6 | MENU_KEY_0,
            "#RadioA",
            Menu::Radio1,
        ),
        2 => (
            MENU_KEY_1 | MENU_KEY_2 | MENU_KEY_3 | MENU_KEY_4 | MENU_KEY_5 | MENU_KEY_6 | MENU_KEY_0,
            "#RadioB",
            Menu::Radio2,
        ),
        _ => (
            MENU_KEY_1
                | MENU_KEY_2
                | MENU_KEY_3
                | MENU_KEY_4
                | MENU_KEY_5
                | MENU_KEY_6
                | MENU_KEY_7
                | MENU_KEY_8
                | MENU_KEY_9
                | MENU_KEY_0,
            "#RadioC",
            Menu::Radio3,
        ),
    };
    menus::show_menu(eng, slot, keys, -1, text);
    if let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    {
        p.menu = menu;
    }
}

/// The key of a radio menu (`menuselect N` while the player is in `Menu_Radio1..3`).
pub(crate) fn menu_choice(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, menu: Menu, key: i32) {
    let group = match menu {
        Menu::Radio1 => 1,
        Menu::Radio2 => 2,
        Menu::Radio3 => 3,
        _ => return,
    };
    radio_choice(g, eng, slot, group, key);
}

/// The sentence and the text of every entry of the radio menus (`Radio1`, `Radio2`, `Radio3` of
/// `client.cpp`): `(group, key, sentence, text)`. Key 1 of group 3 is "affirmative" or "roger
/// that" by chance, and is listed as the first.
const RADIO_MESSAGES: [(u8, i32, &str, &str); 21] = [
    (1, 1, "%!MRAD_COVERME", "#Cover_me"),
    (1, 2, "%!MRAD_TAKEPOINT", "#You_take_the_point"),
    (1, 3, "%!MRAD_POSITION", "#Hold_this_position"),
    (1, 4, "%!MRAD_REGROUP", "#Regroup_team"),
    (1, 5, "%!MRAD_FOLLOWME", "#Follow_me"),
    (1, 6, "%!MRAD_HITASSIST", "#Taking_fire"),
    (2, 1, "%!MRAD_GO", "#Go_go_go"),
    (2, 2, "%!MRAD_FALLBACK", "#Team_fall_back"),
    (2, 3, "%!MRAD_STICKTOG", "#Stick_together_team"),
    (2, 4, "%!MRAD_GETINPOS", "#Get_in_position_and_wait"),
    (2, 5, "%!MRAD_STORMFRONT", "#Storm_the_front"),
    (2, 6, "%!MRAD_REPORTIN", "#Report_in_team"),
    (3, 1, "%!MRAD_AFFIRM", "#Affirmative"),
    (3, 2, "%!MRAD_ENEMYSPOT", "#Enemy_spotted"),
    (3, 3, "%!MRAD_BACKUP", "#Need_backup"),
    (3, 4, "%!MRAD_CLEAR", "#Sector_clear"),
    (3, 5, "%!MRAD_INPOS", "#In_position"),
    (3, 6, "%!MRAD_REPRTINGIN", "#Reporting_in"),
    (3, 7, "%!MRAD_BLOW", "#Get_out_of_there"),
    (3, 8, "%!MRAD_NEGATIVE", "#Negative"),
    (3, 9, "%!MRAD_ENEMYDOWN", "#Enemy_down"),
];
/// The other answer of key 1 of group 3.
const RADIO_ROGER: (&str, &str) = ("%!MRAD_ROGER", "#Roger_that");

/// `radioInfo` of `client.cpp`: `(group, key, alias)`.
const RADIO_ALIASES: [(u8, i32, &str); 21] = [
    (1, 1, "coverme"),
    (1, 2, "takepoint"),
    (1, 3, "holdpos"),
    (1, 4, "regroup"),
    (1, 5, "followme"),
    (1, 6, "takingfire"),
    (2, 1, "go"),
    (2, 2, "fallback"),
    (2, 3, "sticktog"),
    (2, 4, "getinpos"),
    (2, 5, "stormfront"),
    (2, 6, "report"),
    (3, 1, "roger"),
    (3, 2, "enemyspot"),
    (3, 3, "needbackup"),
    (3, 4, "sectorclear"),
    (3, 5, "inposition"),
    (3, 6, "reportingin"),
    (3, 7, "getout"),
    (3, 8, "negative"),
    (3, 9, "enemydown"),
];

/// `Radio1`, `Radio2` and `Radio3` (`client.cpp`): the player speaks entry `key` of `group`,
/// unless it spoke less than 1.5 seconds ago or has used up its messages. The timer and the
/// count are spent even when `key` is not an entry (the original does the same).
fn radio_choice(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, group: u8, key: i32) {
    let now = eng.time();
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if p.hud.radio_time >= now || p.hud.radio_messages <= 0 {
        return;
    }
    p.hud.radio_messages -= 1;
    p.hud.radio_time = now + RADIO_TIMEOUT;
    let Some(&(_, _, sentence, text)) = RADIO_MESSAGES
        .iter()
        .find(|&&(gr, k, _, _)| gr == group && k == key)
    else {
        return;
    };
    let (sentence, text) = if group == 3 && key == 1 && g.random_range(0, 1) == 0 {
        RADIO_ROGER
    } else {
        (sentence, text)
    };
    radio(g, eng, slot, sentence, text);
}

/// `CBasePlayer::Radio(msg_id, msg_verbose, pitch = 100, showIcon = true)`: the sentence, its
/// text and the icon over the speaker, to every teammate of the speaker that has not set
/// `ignorerad` (see the module documentation). Nothing for a spectator or a dead player.
pub(crate) fn radio(
    g: &GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    msg_id: &str,
    msg_verbose: &str,
) {
    let id = eng.client_edict(slot);
    let Some(me) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    let Some(v) = eng.ent(id) else {
        return;
    };
    // Spectators don't say radio messages, neither do dead guys.
    if v.flags & FL_SPECTATOR != 0 || v.deadflag != DEAD_NO {
        return;
    }
    let sender = id.index.min(u16::from(u8::MAX)) as u8;
    let name = me.chat.netname.clone();
    let sender_text = id.index.to_string();
    let sprite = eng.model_index(RADIO_SPRITE).unwrap_or(0);
    let mut icon = MsgWriter::new();
    icon.write_u8(sender);
    icon.write_coord(RADIO_ICON_OFFSET);
    icon.write_u16(sprite);
    icon.write_u16(RADIO_ICON_LIFE);

    for (i, other) in g.players.iter().enumerate() {
        let Some(other) = other else { continue };
        let Some(ov) = eng.ent(eng.client_edict(i as u8)) else {
            continue;
        };
        if ov.flags & FL_DORMANT != 0 || other.hud.ignore_radio {
            continue;
        }
        if ov.flags & FL_SPECTATOR == 0 {
            // `PlayerRelationship`: the speaker and its team (even the dead hear).
            if !is_teammate(g, eng, slot, i as u8) {
                continue;
            }
        } else {
            // A spectator hears what the player it follows would (player.cpp:446-464): it must
            // be chasing or watching in first person, have a target that is not dormant, and
            // that target must be the speaker or a teammate.
            if !matches!(ov.iuser1, OBS_CHASE_LOCKED | OBS_CHASE_FREE | OBS_IN_EYE) {
                continue;
            }
            let Some(target) = other.obs.target else {
                continue;
            };
            if g.players.get(usize::from(target)).is_none_or(Option::is_none)
                || eng
                    .ent(eng.client_edict(target))
                    .is_none_or(|tv| tv.flags & FL_DORMANT != 0)
                || !is_teammate(g, eng, slot, target)
            {
                continue;
            }
        }
        let dest = MsgDest::One(i as u8);
        eng.send_usermsg(
            dest,
            &CsUserMsg::SendAudio {
                sender_id: sender,
                audio_code: msg_id.to_string(),
                pitch: i16::from(PITCH_NORM),
            },
        );
        if !msg_verbose.is_empty() {
            // No place name: the vanilla build has no nav areas outside Condition Zero.
            crate::chat::client_print(
                eng,
                i as u8,
                HUD_PRINTRADIO,
                &sender_text,
                &["#Game_radio", &name, msg_verbose],
            );
        }
        eng.temp_entity(dest, TE_PLAYERATTACHMENT, icon.as_slice());
    }
}

/// `CHalfLifeMultiplay::PlayerRelationship(slot, target) == GR_TEAMMATE`: the same player, or a
/// player (not a spectator) of the same team.
fn is_teammate(g: &GameState, eng: &EngineApi<'_>, slot: u8, target: u8) -> bool {
    if slot == target {
        return true;
    }
    let (Some(a), Some(b)) = (
        g.players.get(usize::from(slot)).and_then(Option::as_ref),
        g.players.get(usize::from(target)).and_then(Option::as_ref),
    ) else {
        return false;
    };
    eng.ent(eng.client_edict(target))
        .is_some_and(|v| v.flags & FL_SPECTATOR == 0)
        && a.team == b.team
}

/// `nightvision`: for a player that has night vision, switches it on or off with the sound and
/// `NVGToggle`; at most once every 0.3 seconds. The observers that watch the player in first
/// person get the same sound and toggle (client.cpp:3466-3512).
fn night_vision(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time();
    let Some(p) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if now < p.hud.next_nightvision {
        return;
    }
    p.hud.next_nightvision = now + NIGHTVISION_INTERVAL;
    if !p.has_night_vision {
        return;
    }
    let was_on = p.night_vision_on;
    p.night_vision_on = !was_on;
    let volume = random_float(g, 0.92, 1.0);
    let sample = if was_on {
        "items/nvg_off.wav"
    } else {
        "items/nvg_on.wav"
    };
    let id = eng.client_edict(slot);
    eng.emit_sound(id, CHAN_ITEM, sample, volume, ATTN_NORM, PITCH_NORM);
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::NvgToggle(!was_on));
    for watcher in followers(g, eng, slot) {
        let volume = random_float(g, 0.92, 1.0);
        let wid = eng.client_edict(watcher);
        eng.emit_sound(wid, CHAN_ITEM, sample, volume, ATTN_NORM, PITCH_NORM);
        eng.send_usermsg(MsgDest::One(watcher), &CsUserMsg::NvgToggle(!was_on));
        if let Some(w) = g
            .players
            .get_mut(usize::from(watcher))
            .and_then(Option::as_mut)
        {
            w.night_vision_on = !was_on;
        }
    }
}

/// The radar half of `UpdateClientData`: a living player that has moved 64 units since the last
/// time tells its living teammates where it is, at most once a second.
pub(crate) fn update_radar(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time();
    let id = eng.client_edict(slot);
    let Some(v) = eng.ent(id) else {
        return;
    };
    if v.deadflag != DEAD_NO {
        return;
    }
    let origin = v.origin;
    let Some(me) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    else {
        return;
    };
    if now <= me.hud.next_radar_update {
        return;
    }
    me.hud.next_radar_update = now + RADAR_INTERVAL;
    let moved = (origin - me.hud.last_origin).length() >= RADAR_TOLERANCE;
    me.hud.last_origin = origin;
    if !moved {
        return;
    }
    let team = me.team;
    let index = (usize::from(slot) + 1).min(usize::from(u8::MAX)) as u8;
    for (i, other) in g.players.iter().enumerate() {
        let Some(other) = other else { continue };
        if i == usize::from(slot) || other.team != team {
            continue;
        }
        if eng
            .ent(eng.client_edict(i as u8))
            .is_none_or(|ov| ov.deadflag != DEAD_NO || ov.flags & FL_DORMANT != 0)
        {
            continue;
        }
        eng.send_usermsg(
            MsgDest::One(i as u8),
            &CsUserMsg::Radar {
                player_id: index,
                origin,
            },
        );
    }
}

/// `AngleVectors`' forward vector of `angles` (pitch, yaw), computed in double as the engine does.
fn forward_vector(angles: Vec3) -> Vec3 {
    let sc = |degrees: f32| {
        let a = (f64::from(degrees) * (std::f64::consts::PI * 2.0 / 360.0)) as f32;
        let a = f64::from(a);
        (a.sin() as f32, a.cos() as f32)
    };
    let (sy, cy) = sc(angles.y);
    let (sp, cp) = sc(angles.x);
    Vec3::new(cp * cy, cp * sy, -sp)
}

/// `int((health / max_health) * 100)` in single precision, as the original computes it.
fn health_percent(health: f32) -> i32 {
    ((health / 100.0f32) * 100.0f32) as i32
}

/// `CBasePlayer::UpdateStatusBar`, run by `UpdateClientData` every 0.2 seconds: names the
/// player under the crosshair (see the module documentation) and sends what changed.
pub(crate) fn update_status_bar(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time();
    let id = eng.client_edict(slot);
    let Some(me) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    if me.hud.next_sbar_update >= now {
        return;
    }
    let (team, old_state, old_string, mut disappear) = (
        me.team,
        me.hud.sbar_state,
        me.hud.sbar_string.clone(),
        me.hud.sbar_disappear,
    );
    let Some(v) = eng.ent(id) else {
        return;
    };
    let (src, view) = (v.origin + v.view_ofs, v.v_angle + v.punchangle);
    let range = if v.flags & FL_SPECTATOR != 0 {
        MAX_SPEC_ID_RANGE
    } else {
        MAX_ID_RANGE
    };
    let observing = v.iuser1 != OBS_NONE;
    let mut state = [0i32; SBAR_END];
    let mut text = old_string.clone();

    // Find an ID target.
    let end = src + forward_vector(view) * range;
    let tr = eng.trace_line(src, end, MoveFilter::Normal, Some(id));
    if tr.trace.fraction != 1.0 {
        // `FNullEnt(tr.pHit)`: the world counts as none.
        let target = tr
            .ent
            .filter(|&hit| hit != eng.world())
            .and_then(|hit| player_slot(g, eng, hit))
            .and_then(|t| {
                let tp = g.players[t].as_ref()?;
                let health = eng.ent(eng.client_edict(t as u8))?.health;
                Some((t, tp.team == team, health))
            });
        if let Some((t, same_team, health)) = target {
            let playerid = eng.cvar_f32("mp_playerid");
            state[SBAR_ID_TARGETNAME] = (t + 1) as i32;
            state[SBAR_ID_TARGETTYPE] = if same_team {
                SBAR_TARGETTYPE_TEAMMATE
            } else {
                SBAR_TARGETTYPE_ENEMY
            };
            if same_team || observing {
                text = if playerid != PLAYERID_MODE_OFF || observing {
                    SBAR_FORMAT_TEAMMATE
                } else {
                    SBAR_FORMAT_NONE
                }
                .to_string();
                state[SBAR_ID_TARGETHEALTH] = health_percent(health);
                // `#Hint_spotted_a_friend` needs the hint queue.
            } else {
                text = if playerid != PLAYERID_MODE_TEAMONLY && playerid != PLAYERID_MODE_OFF {
                    SBAR_FORMAT_ENEMY
                } else {
                    SBAR_FORMAT_NONE
                }
                .to_string();
                // `#Hint_spotted_an_enemy` needs the hint queue.
            }
            disappear = now + SBAR_HOLD;
        }
        // A hostage (`CLASS_HUMAN_PASSIVE`) would be named here: there are none yet.
    } else if disappear > now {
        // Hold the values for a short while after the target left the crosshair.
        state[SBAR_ID_TARGETTYPE] = old_state[SBAR_ID_TARGETTYPE];
        state[SBAR_ID_TARGETNAME] = old_state[SBAR_ID_TARGETNAME];
        state[SBAR_ID_TARGETHEALTH] = old_state[SBAR_ID_TARGETHEALTH];
    }

    let force_resend = text != old_string;
    if force_resend {
        eng.send_usermsg(
            MsgDest::One(slot),
            &CsUserMsg::StatusText {
                line: 0,
                text: text.clone(),
            },
        );
    }
    let mut sent = old_state;
    for i in 1..SBAR_END {
        if state[i] != old_state[i] || force_resend {
            eng.send_usermsg(
                MsgDest::One(slot),
                &CsUserMsg::StatusValue {
                    index: i as u8,
                    value: state[i].clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16,
                },
            );
            sent[i] = state[i];
        }
    }
    if let Some(me) = g
        .players
        .get_mut(usize::from(slot))
        .and_then(Option::as_mut)
    {
        me.hud.next_sbar_update = now + SBAR_UPDATE_INTERVAL;
        me.hud.sbar_state = sent;
        me.hud.sbar_string = text;
        me.hud.sbar_disappear = disappear;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radio_tables_follow_client_cpp() {
        // Every alias names an entry of the menus, and every entry has an alias.
        for &(group, key, alias) in &RADIO_ALIASES {
            assert!(
                RADIO_MESSAGES.iter().any(|&(g, k, _, _)| (g, k) == (group, key)),
                "{alias}"
            );
        }
        assert_eq!(RADIO_ALIASES.len(), RADIO_MESSAGES.len());
        // Group 3 has nine keys, the others six.
        let count = |group| RADIO_MESSAGES.iter().filter(|m| m.0 == group).count();
        assert_eq!((count(1), count(2), count(3)), (6, 6, 9));
        assert!(RADIO_MESSAGES.iter().all(|m| m.2.starts_with("%!MRAD_") && m.3.starts_with('#')));
    }

    #[test]
    fn health_percent_truncates_in_single_precision() {
        assert_eq!(health_percent(100.0), 100);
        assert_eq!(health_percent(50.0), 50);
        assert_eq!(health_percent(0.0), 0);
        // A fraction of a point is dropped, not rounded.
        assert_eq!(health_percent(99.5), 99);
    }

    #[test]
    fn forward_vector_looks_along_yaw_and_down_with_pitch() {
        let f = forward_vector(Vec3::new(0.0, 0.0, 0.0));
        assert!((f - Vec3::X).length() < 1e-6);
        let f = forward_vector(Vec3::new(0.0, 90.0, 0.0));
        assert!((f - Vec3::Y).length() < 1e-6);
        // A positive pitch looks down.
        let f = forward_vector(Vec3::new(90.0, 0.0, 0.0));
        assert!((f - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-6);
    }
}
