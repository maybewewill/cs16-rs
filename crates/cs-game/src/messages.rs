//! The user messages of the game: the table `LinkUserMessages` registers, one small function per
//! message the game sends, and the two places that send a player the state of its HUD:
//! [`update_client_data`] (`CBasePlayer::UpdateClientData`) and [`init_hud`]
//! (`CHalfLifeMultiplay::InitHUD`). Every send of the player code goes through here, so a task
//! that gives a message a new source extends its function and nothing else.
//!
//! Ported from ReGameDLL_CS `client.cpp` (`LinkUserMessages`), `player.cpp`
//! (`UpdateClientData`, `SetScoreAttrib`, `SetBombIcon`, `SyncRoundTimer`, `SendWeatherInfo`,
//! `RemoveAllItems`) and `multiplay_gamerules.cpp` (`InitHUD`, `UpdateGameMode`,
//! `SendMOTDToClient`), read 2026-11-22. The messages `cs_proto48::usermsg::CsUserMsg` cannot
//! encode (`GameMode`, `ViewMode`, `ShadowIdx`, `AllowSpec`, `ForceCam`, `Train`, `ReceiveW`
//! and the `MOTD` text, whose chunks may end inside a UTF-8 sequence) are written as bytes with
//! [`send_raw`], against the same table.
//!
//! `InitHUD`'s "entered the game" log line (`UTIL_LogPrintf`) is not ported.
//!
//! Not sent yet, with the task that sends them: `Fog` (no `env_fog` entity yet), `HLTV` (there
//! is no HLTV; `SVC_INTERMISSION` is sent by the round rules), `HostagePos`/`Scenario`
//! (hostages), `Account`/`HealthInfo` (they exist only in the newest builds, `BUILD_LATEST`:
//! vanilla does not register them), the career messages (`CZCareer`, Condition Zero only).

use cs_engine::consts::FL_PROXY;
use cs_engine::{EngineApi, MsgDest};
use cs_proto48::bitbuf::MsgWriter;
use cs_proto48::usermsg::{CsUserMsg, USERMSG_BASE_ID, UserMsgRegistry};
use glam::Vec3;

use crate::GameState;
use crate::player::{Player, Team, dmg};
use crate::teams::team_name;

/// `LinkUserMessages`: every `REG_USER_MSG` in the order of the source (which fixes the ids the
/// clients see, `USERMSG_BASE_ID` and up) with its size, -1 for a variable one. The Condition
/// Zero and bot messages are registered like the others, so the ids stay those of the original
/// even where the game never sends them. `Account` and `HealthInfo` (`BUILD_LATEST`) are not
/// registered: assumption, the vanilla Steam `mp.dll` predates them.
pub(crate) const USER_MESSAGES: &[(&str, i8)] = &[
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
    ("TutorText", -1),
    ("TutorLine", -1),
    ("TutorState", -1),
    ("TutorClose", -1),
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
    ("CZCareer", -1),
    ("CZCareerHUD", -1),
    ("ShadowIdx", 4),
    ("TaskTime", 4),
    ("Scenario", -1),
    ("BotVoice", 2),
    ("BuyClose", 0),
    ("SpecHealth2", 2),
    ("BarTime2", 4),
    ("ItemStatus", 1),
    ("Location", -1),
    ("BotProgress", -1),
    ("Brass", -1),
    ("Fog", 7),
    ("ShowTimer", 0),
    ("HudTextArgs", -1),
    ("Account", 5),
    ("HealthInfo", 5),
];

/// `MAX_MOTD_CHUNK` and `MAX_MOTD_LENGTH` (`gamerules.h`).
const MAX_MOTD_CHUNK: usize = 60;
const MAX_MOTD_LENGTH: usize = 1536;

/// `SCORE_STATUS_*` (`cdll_dll.h` 64-66). `SCORE_STATUS_DEFKIT` (`1 << 3`) is `BUILD_LATEST` only.
pub(crate) const SCORE_STATUS_DEAD: u8 = 1 << 0;
pub(crate) const SCORE_STATUS_BOMB: u8 = 1 << 1;
pub(crate) const SCORE_STATUS_VIP: u8 = 1 << 2;

/// The registry the game gives the engine: [`USER_MESSAGES`] with ids from `USERMSG_BASE_ID`.
pub(crate) fn link_user_messages() -> UserMsgRegistry {
    let mut reg = UserMsgRegistry::new();
    for (i, &(name, size)) in USER_MESSAGES.iter().enumerate() {
        reg.register(USERMSG_BASE_ID + i as u8, size, name);
    }
    reg
}

/// Sends the message `name` of [`USER_MESSAGES`] with `payload` as its bytes. `false`, nothing
/// sent, for a name that is not registered, a payload of the wrong size for a fixed-size
/// message, or one over 255 bytes for a variable one.
pub(crate) fn send_raw(eng: &mut EngineApi<'_>, dest: MsgDest, name: &str, payload: &[u8]) -> bool {
    let Some(i) = USER_MESSAGES.iter().position(|&(n, _)| n == name) else {
        return false;
    };
    let size = USER_MESSAGES[i].1;
    let mut w = MsgWriter::new();
    w.write_u8(USERMSG_BASE_ID + i as u8);
    if size < 0 {
        let Ok(len) = u8::try_from(payload.len()) else {
            return false;
        };
        w.write_u8(len);
    } else if payload.len() != size as usize {
        return false;
    }
    w.write_bytes(payload);
    eng.send(dest, w.as_slice());
    true
}

fn clamp_i16(v: i32) -> i16 {
    v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// The edict index of `slot` (`ENTINDEX`), the number the clients know a player by.
fn entity_index(slot: usize) -> u8 {
    (slot + 1).min(usize::from(u8::MAX)) as u8
}

// ---- the messages ----

/// `gmsgScoreInfo` to `dest` (`CBasePlayer::Spawn`, `CHalfLifeMultiplay::InitHUD`): the
/// scoreboard line of the player of edict index `index`.
fn score_info_to(
    eng: &mut EngineApi<'_>,
    dest: MsgDest,
    index: u16,
    frags: i32,
    deaths: i32,
    team: Team,
) {
    let Ok(player_id) = u8::try_from(index) else {
        return;
    };
    eng.send_usermsg(
        dest,
        &CsUserMsg::ScoreInfo {
            player_id,
            frags: clamp_i16(frags),
            deaths: clamp_i16(deaths),
            class_id: 0,
            team_id: i16::from(team.id()),
        },
    );
}

/// `gmsgScoreInfo` to everyone: the scoreboard line of the player of edict index `index`
/// (ReGameDLL `CBasePlayer::Spawn`, `CHalfLifeMultiplay::PlayerKilled`, read 2026-11-22).
pub(crate) fn score_info(eng: &mut EngineApi<'_>, index: u16, p: &Player) {
    score_info_to(eng, MsgDest::All, index, p.frags, p.deaths, p.team);
}

/// `gmsgDeathMsg` to everyone (`CHalfLifeMultiplay::DeathNotice`): `killer` and `victim` are
/// edict indices, `killer` 0 for the world.
pub(crate) fn death_msg(
    eng: &mut EngineApi<'_>,
    killer: u16,
    victim: u16,
    headshot: bool,
    weapon: &str,
) {
    let (Ok(killer_id), Ok(victim_id)) = (u8::try_from(killer), u8::try_from(victim)) else {
        return;
    };
    eng.send_usermsg(
        MsgDest::All,
        &CsUserMsg::DeathMsg {
            killer_id,
            victim_id,
            headshot,
            weapon: weapon.to_string(),
        },
    );
}

/// `gmsgHealth` to `slot` (`CBasePlayer::UpdateClientData`, `Killed`).
pub(crate) fn health(eng: &mut EngineApi<'_>, slot: u8, hp: u8) {
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::Health(hp));
}

/// `gmsgSetFOV` to `slot` (`CBasePlayer::SendFOV`).
pub(crate) fn set_fov(eng: &mut EngineApi<'_>, slot: u8, fov: u8) {
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::SetFov(fov));
}

/// `gmsgSendCorpse` to everyone (`CBasePlayer::SpawnClientSideCorpse`).
pub(crate) fn corpse(eng: &mut EngineApi<'_>, msg: CsUserMsg) {
    eng.send_usermsg(MsgDest::All, &msg);
}

/// `gmsgCurWeapon` of `CBasePlayer::Killed`: not active, weapon `0xFF`, clip `0xFF` (a byte of
/// -1), so the ammo HUD of the dead shows nothing.
pub(crate) fn cur_weapon_dead(eng: &mut EngineApi<'_>, slot: u8) {
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::CurWeapon {
            is_active: false,
            weapon_id: 0xFF,
            clip_ammo: -1,
        },
    );
}

/// `gmsgCurWeapon` with no weapon (`RemoveAllItems`): the ammo HUD shows nothing.
pub(crate) fn cur_weapon_none(eng: &mut EngineApi<'_>, slot: u8) {
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::CurWeapon {
            is_active: false,
            weapon_id: 0,
            clip_ammo: 0,
        },
    );
}

/// `gmsgCurWeapon` of `CBasePlayerWeapon::UpdateClientData`: the state (1 for the active weapon,
/// 0 for the others), the weapon id and the clip, each a byte (a clip of -1 is 255).
pub(crate) fn cur_weapon(eng: &mut EngineApi<'_>, slot: u8, state: u8, id: u8, clip: u8) {
    send_raw(eng, MsgDest::One(slot), "CurWeapon", &[state, id, clip]);
}

/// `gmsgAmmoX` (`CBasePlayer::SendAmmoUpdate`): the player's count of ammo `index`.
pub(crate) fn ammo_x(eng: &mut EngineApi<'_>, slot: u8, index: u8, amount: u8) {
    send_raw(eng, MsgDest::One(slot), "AmmoX", &[index, amount]);
}

/// `gmsgTrain` (`CBasePlayer::UpdateClientData`): the train/vehicle gear HUD icon (`0` off,
/// `1..=5` neutral/forward/reverse gear).
pub(crate) fn train(eng: &mut EngineApi<'_>, slot: u8, gear: u8) {
    send_raw(eng, MsgDest::One(slot), "Train", &[gear]);
}

/// `gmsgFog` (`CHalfLifeMultiplay::InitHUD` / `env_fog`): 3 RGB bytes + 4-byte LE float density.
pub(crate) fn fog(eng: &mut EngineApi<'_>, dest: MsgDest, color: [u8; 3], density: f32) {
    let mut payload = [0u8; 7];
    payload[0..3].copy_from_slice(&color);
    payload[3..7].copy_from_slice(&density.to_le_bytes());
    send_raw(eng, dest, "Fog", &payload);
}

/// `gmsgHudText` (`env_message`): null-terminated localized key / text.
pub(crate) fn hud_text(eng: &mut EngineApi<'_>, dest: MsgDest, text: &str) {
    let mut payload = text.as_bytes().to_vec();
    if !payload.ends_with(&[0]) {
        payload.push(0);
    }
    send_raw(eng, dest, "HudText", &payload);
}

/// `gmsgScreenShake` (`env_shake`): 3 signed/unsigned fixed-point shorts (`* 4096`).
pub(crate) fn screen_shake(
    eng: &mut EngineApi<'_>,
    dest: MsgDest,
    amplitude: f32,
    duration: f32,
    frequency: f32,
) {
    let fixed =
        |v: f32| -> [u8; 2] { ((v * 4096.0).round().clamp(0.0, 65535.0) as u16).to_le_bytes() };
    let mut payload = [0u8; 6];
    payload[0..2].copy_from_slice(&fixed(amplitude));
    payload[2..4].copy_from_slice(&fixed(duration));
    payload[4..6].copy_from_slice(&fixed(frequency));
    send_raw(eng, dest, "ScreenShake", &payload);
}

/// `gmsgScreenFade` (`env_fade`): duration (`u16`), hold (`u16`), flags (`u16`), RGBA (`[u8; 4]`).
pub(crate) fn screen_fade(
    eng: &mut EngineApi<'_>,
    dest: MsgDest,
    duration: f32,
    hold_time: f32,
    flags: u16,
    color: [u8; 4],
) {
    let fixed =
        |v: f32| -> [u8; 2] { ((v * 4096.0).round().clamp(0.0, 65535.0) as u16).to_le_bytes() };
    let mut payload = [0u8; 10];
    payload[0..2].copy_from_slice(&fixed(duration));
    payload[2..4].copy_from_slice(&fixed(hold_time));
    payload[4..6].copy_from_slice(&flags.to_le_bytes());
    payload[6..10].copy_from_slice(&color);
    send_raw(eng, dest, "ScreenFade", &payload);
}

/// `gmsgReloadSound` (`CBasePlayerWeapon::ReloadSound`) to `slot`: the volume byte and the
/// kind (0 for a shotgun, 1 for any other weapon).
pub(crate) fn reload_sound(eng: &mut EngineApi<'_>, slot: u8, volume: u8, kind: u8) {
    send_raw(eng, MsgDest::One(slot), "ReloadSound", &[volume, kind]);
}

/// `gmsgGameMode` (`CHalfLifeMultiplay::UpdateGameMode`): 1, multiplayer.
fn game_mode(eng: &mut EngineApi<'_>, slot: u8) {
    send_raw(eng, MsgDest::One(slot), "GameMode", &[1]);
}

/// `gmsgShadowIdx`: the model index of the shadow sprite (`g_iShadowSprite`), a long.
fn shadow_idx(eng: &mut EngineApi<'_>, slot: u8) {
    let sprite = eng.model_index("sprites/shadow_circle.spr").unwrap_or(0);
    send_raw(
        eng,
        MsgDest::One(slot),
        "ShadowIdx",
        &i32::from(sprite).to_le_bytes(),
    );
}

/// `gmsgTeamScore` to `slot`: the rounds `team` has won.
fn team_score(eng: &mut EngineApi<'_>, slot: u8, team: Team, wins: i32) {
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::TeamScore {
            team_name: team_name(team).to_string(),
            score: clamp_i16(wins),
        },
    );
}

/// `gmsgTeamInfo` to `dest_slot`: the team of the player of edict index `index`.
fn team_info(eng: &mut EngineApi<'_>, dest_slot: u8, index: u8, team: Team) {
    eng.send_usermsg(
        MsgDest::One(dest_slot),
        &CsUserMsg::TeamInfo {
            player_id: index,
            team_name: team_name(team).to_string(),
        },
    );
}

/// `gmsgScoreAttrib` to `dest_slot` (`CBasePlayer::SetScoreAttrib`, vanilla: no team filter,
/// which is `REGAMEDLL_FIXES` only, and no `SCORE_STATUS_DEFKIT`, which is `BUILD_LATEST` only).
fn score_attrib(eng: &mut EngineApi<'_>, dest_slot: u8, index: u8, p: &Player) {
    let mut flags = 0;
    if !p.alive {
        flags |= SCORE_STATUS_DEAD;
    }
    if p.has_c4 {
        flags |= SCORE_STATUS_BOMB;
    }
    if p.is_vip {
        flags |= SCORE_STATUS_VIP;
    }
    eng.send_usermsg(
        MsgDest::One(dest_slot),
        &CsUserMsg::ScoreAttrib {
            player_id: index,
            flags,
        },
    );
}

/// `gmsgRadar` to `dest_slot`: where the player of edict index `index` is.
fn radar(eng: &mut EngineApi<'_>, dest_slot: u8, index: u8, origin: Vec3) {
    eng.send_usermsg(
        MsgDest::One(dest_slot),
        &CsUserMsg::Radar {
            player_id: index,
            origin,
        },
    );
}

/// `gmsgMoney` to `slot`: the account, no flash (`UpdateClientData` with `m_fInitHUD`).
pub(crate) fn money(eng: &mut EngineApi<'_>, slot: u8, amount: i32) {
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::Money {
            amount,
            flash: false,
        },
    );
}

/// Whether `obs_slot` is an eligible observer to receive `HealthInfo` / `Account` updates for
/// `subject_slot` (`ReGameDLL_CS` `BUILD_LATEST` observer anti-leak rules):
/// - Never leaks enemy HP/money to live opposing players (`obs.alive` or `OBS_NONE`).
/// - HLTV proxy slots (`FL_PROXY`) and `Team::Spectator` observers see both teams.
/// - Dead team observers (`!obs.alive`, `Team::Ct | Team::Terrorist`, `iuser1 != OBS_NONE`)
///   see teammates always, and enemies ONLY when `force_camera == CAMERA_MODE_SPEC_ANYONE`
///   (`mp_forcecamera == 0 && mp_forcechasecam == 0 && mp_fadetoblack == 0`).
pub(crate) fn observer_eligible_for(
    g: &GameState,
    eng: &EngineApi<'_>,
    obs_slot: u8,
    subject_slot: u8,
) -> bool {
    let Some(subject) = g
        .players
        .get(usize::from(subject_slot))
        .and_then(Option::as_ref)
    else {
        return false;
    };
    if !matches!(subject.team, Team::Ct | Team::Terrorist) {
        return false;
    }
    let Some(obs) = g
        .players
        .get(usize::from(obs_slot))
        .and_then(Option::as_ref)
    else {
        return false;
    };
    let Some(obs_ent) = eng.ent(eng.client_edict(obs_slot)) else {
        return false;
    };
    if (obs_ent.flags & FL_PROXY) != 0 {
        return true;
    }
    if obs.alive
        || obs_ent.iuser1 == crate::observer::OBS_NONE
        || obs_ent.deadflag == crate::player::DEAD_DYING
    {
        return false;
    }
    match obs.team {
        Team::Spectator => true,
        Team::Ct | Team::Terrorist => {
            if obs.team == subject.team {
                eng.cvar_f32("mp_fadetoblack") as i32 != crate::observer::FADETOBLACK_STAY
            } else {
                crate::observer::force_camera(eng) == crate::observer::CAMERA_MODE_SPEC_ANYONE
            }
        }
        Team::Unassigned => false,
    }
}

fn broadcast_observer_usermsg(
    g: &GameState,
    eng: &mut EngineApi<'_>,
    subject_slot: u8,
    msg: &CsUserMsg,
) {
    let Some(subject) = g
        .players
        .get(usize::from(subject_slot))
        .and_then(Option::as_ref)
    else {
        return;
    };
    if !matches!(subject.team, Team::Ct | Team::Terrorist) {
        return;
    }
    for obs_slot in 0..g.players.len() as u8 {
        if observer_eligible_for(g, eng, obs_slot, subject_slot) {
            eng.send_usermsg(MsgDest::One(obs_slot), msg);
        }
    }
    eng.send_replay_usermsg(msg);
}

/// Emits `HealthInfo` for `subject_slot` strictly to eligible observers and the `.dem` replay stream.
pub(crate) fn broadcast_health_info(
    g: &GameState,
    eng: &mut EngineApi<'_>,
    subject_slot: u8,
    health: i32,
) {
    let msg = CsUserMsg::HealthInfo {
        player_id: entity_index(usize::from(subject_slot)),
        health: health.max(0),
    };
    broadcast_observer_usermsg(g, eng, subject_slot, &msg);
}

/// Emits `Account` for `subject_slot` strictly to eligible observers and the `.dem` replay stream.
pub(crate) fn broadcast_account(
    g: &GameState,
    eng: &mut EngineApi<'_>,
    subject_slot: u8,
    amount: i32,
) {
    let msg = CsUserMsg::Account {
        player_id: entity_index(usize::from(subject_slot)),
        amount,
    };
    broadcast_observer_usermsg(g, eng, subject_slot, &msg);
}

/// Syncs `HealthInfo` and `Account` for all eligible players when `obs_slot` becomes an observer.
pub(crate) fn sync_observer_hud_info(g: &GameState, eng: &mut EngineApi<'_>, obs_slot: u8) {
    for (i, subject) in g.players.iter().enumerate() {
        let Some(p) = subject else { continue };
        let subject_slot = i as u8;
        if !observer_eligible_for(g, eng, obs_slot, subject_slot) {
            continue;
        }
        let player_id = entity_index(i);
        let hp = if p.alive {
            eng.ent(eng.client_edict(subject_slot))
                .map_or(0, |v| (v.health as i32).max(0))
        } else {
            0
        };
        eng.send_usermsg(
            MsgDest::One(obs_slot),
            &CsUserMsg::HealthInfo {
                player_id,
                health: hp,
            },
        );
        eng.send_usermsg(
            MsgDest::One(obs_slot),
            &CsUserMsg::Account {
                player_id,
                amount: p.money,
            },
        );
    }
}

/// `CBasePlayer::SetScoreboardAttributes()` for the player of `slot`: `ScoreAttrib` to every
/// player in the game.
pub(crate) fn set_scoreboard_attributes(g: &GameState, eng: &mut EngineApi<'_>, slot: usize) {
    let Some(p) = g.players.get(slot).and_then(Option::as_ref) else {
        return;
    };
    for (dest, other) in g.players.iter().enumerate() {
        if other.is_some() {
            score_attrib(eng, dest as u8, entity_index(slot), p);
        }
    }
}

/// `SetScoreboardAttributes` as `Spawn()` of `ClientPutInServer` runs it: `Spawn` has just set
/// `pev->deadflag = DEAD_NO`, so the player is shown alive (flags 0); `ClientPutInServer` makes
/// it `DEAD_DEAD` only afterwards. The first `InitHUD` shows it dead.
pub(crate) fn set_scoreboard_attributes_alive(g: &GameState, eng: &mut EngineApi<'_>, slot: usize) {
    for (dest, other) in g.players.iter().enumerate() {
        if other.is_some() {
            eng.send_usermsg(
                MsgDest::One(dest as u8),
                &CsUserMsg::ScoreAttrib {
                    player_id: entity_index(slot),
                    flags: 0,
                },
            );
        }
    }
}

/// `gmsgRoundTime` to `slot` (`CBasePlayer::SyncRoundTimer`): the seconds left on the round
/// clock (`GetRoundRemainingTimeReal`, which is `GetRoundRemainingTime` in vanilla), or 0 when
/// it has run out. The freeze period counts down the freeze time. (The bomb and career
/// additions of the original are not ported.)
pub(crate) fn sync_round_timer(g: &GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let remaining = g.rules.round_remaining(eng.time()).max(0.0);
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::RoundTime(remaining.min(f64::from(u16::MAX)) as u16),
    );
}

/// `gmsgReceiveW` (`CBasePlayer::SendWeatherInfo`): 1 for a map with rain, 2 for snow.
fn send_weather_info(g: &GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let has = |eng: &EngineApi<'_>, classes: [&str; 2]| {
        classes
            .iter()
            .any(|c| eng.find_by_classname(None, c).is_some())
    };
    let weather = if has(eng, ["env_rain", "func_rain"]) {
        1
    } else if has(eng, ["env_snow", "func_snow"]) {
        2
    } else if g.weather != 0 {
        g.weather
    } else {
        return;
    };
    send_raw(eng, MsgDest::One(slot), "ReceiveW", &[weather]);
}

/// `gmsgServerName` to `slot`: the `hostname` cvar.
fn server_name(eng: &mut EngineApi<'_>, slot: u8) {
    let name = eng.cvar_str("hostname").unwrap_or_default();
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::ServerName(name));
}

/// The messages `SendMOTDToClient` sends for a MOTD file: the text in chunks of at most
/// [`MAX_MOTD_CHUNK`] bytes, each flagged with whether it is the last. The text ends at its first
/// NUL byte, and no chunk starts at or after [`MAX_MOTD_LENGTH`] bytes, so a long text is cut
/// at the end of the chunk that crosses that mark (up to 1559 bytes). A text with no bytes has
/// no chunks.
pub(crate) fn motd_chunks(file: &[u8]) -> Vec<(bool, &[u8])> {
    let text = file.split(|&b| b == 0).next().unwrap_or_default();
    let mut out = Vec::new();
    let mut sent = 0;
    while sent < text.len() && sent < MAX_MOTD_LENGTH {
        let chunk = &text[sent..(sent + MAX_MOTD_CHUNK).min(text.len())];
        sent += chunk.len();
        out.push((sent >= text.len() || sent >= MAX_MOTD_LENGTH, chunk));
    }
    out
}

/// `CHalfLifeMultiplay::SendMOTDToClient`: the server name, then the message of the day.
fn send_motd(g: &GameState, eng: &mut EngineApi<'_>, slot: u8) {
    server_name(eng, slot);
    for (finished, chunk) in motd_chunks(&g.motd) {
        let mut payload = Vec::with_capacity(chunk.len() + 2);
        payload.push(u8::from(finished));
        payload.extend_from_slice(chunk);
        payload.push(0);
        send_raw(eng, MsgDest::One(slot), "MOTD", &payload);
    }
}

// ---- the HUD of a player ----

/// `CHalfLifeMultiplay::InitHUD` for the player of `slot`, who has just entered: the game mode,
/// the scoreboard (everyone's score and team), the team scores, the MOTD and the spectator
/// rules. Without a dropped or planted bomb, which comes with the bomb.
fn init_hud(g: &GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(me) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    // `UpdateGameMode`.
    game_mode(eng, slot);
    if eng.cvar_f32("sv_cheats") == 0.0 {
        send_raw(eng, MsgDest::One(slot), "ViewMode", &[]);
    }
    // Sending just one score makes the scoreboard active.
    score_info_to(
        eng,
        MsgDest::One(slot),
        u16::from(entity_index(usize::from(slot))),
        0,
        0,
        me.team,
    );
    shadow_idx(eng, slot);
    send_motd(g, eng, slot);

    // Everyone's score goes to the newcomer.
    for (i, other) in g.players.iter().enumerate() {
        let Some(other) = other else { continue };
        score_info_to(
            eng,
            MsgDest::One(slot),
            u16::from(entity_index(i)),
            other.frags,
            other.deaths,
            other.team,
        );
    }
    team_score(eng, slot, Team::Terrorist, g.num_terrorist_wins);
    team_score(eng, slot, Team::Ct, g.num_ct_wins);
    send_raw(
        eng,
        MsgDest::One(slot),
        "AllowSpec",
        &[(eng.cvar_f32("allow_spectators") as i32) as u8],
    );
    let force_cam = [
        u8::from(eng.cvar_f32("mp_forcecamera") != 0.0),
        u8::from(eng.cvar_f32("mp_forcechasecam") != 0.0),
        u8::from(eng.cvar_f32("mp_fadetoblack") == 1.0),
    ];
    send_raw(eng, MsgDest::One(slot), "ForceCam", &force_cam);
    // A player that enters while the scoreboard is up sees it too.
    if g.rules.game_over() {
        eng.send(
            MsgDest::One(slot),
            &[cs_proto48::messages::svc::INTERMISSION],
        );
    }

    for (i, other) in g.players.iter().enumerate() {
        let Some(other) = other else { continue };
        let index = entity_index(i);
        team_info(eng, slot, index, other.team);
        score_attrib(eng, slot, index, other);
        if i != usize::from(slot) && other.alive {
            // Vanilla sends every living player's position, teammate or not
            // (`BUILD_LATEST_FIXES` limits it to the team).
            let origin = eng
                .ent(eng.client_edict(i as u8))
                .map_or(Vec3::ZERO, |v| v.origin);
            radar(eng, slot, index, origin);
        }
    }
    // Vanilla (`!REGAMEDLL_FIXES`): sends `BombDrop(DROPPED)` when the bomb lies dropped on the
    // ground; the planted-bomb `else` branch is `REGAMEDLL_FIXES` only.
    if g.rules.bomb_dropped
        && let Some(origin) = crate::inventory::dropped_bomb_origin(eng)
    {
        eng.send_usermsg(
            MsgDest::One(slot),
            &CsUserMsg::BombDrop {
                origin,
                planted: false,
            },
        );
    }
}

/// The ammo hint of `UpdateClientData` (player.cpp:7893, read 2026-11-22): the first time the
/// active weapon, one that is not used up by firing (`ITEM_FLAG_EXHAUSTIBLE`: the grenades and
/// the C4), has no ammo in the clip and none in reserve, the player is told
/// (`#Hint_out_of_ammo`). `AmmoInventory(-1)` is -1: a weapon with no primary ammo type (the
/// knife) counts as empty on the reserve side, but its clip is not 0.
fn out_of_ammo_hint(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    if p.display_history & crate::hint::dhf::AMMO_EXHAUSTED != 0 {
        return;
    }
    let Some(id) = p.active_item else {
        return;
    };
    let Some((s, i)) = p.find_item(id) else {
        return;
    };
    let weapon = &p.items[s][i].weapon;
    let flags = crate::weapons::item_info(id).map_or(0, |i| i.flags);
    if flags & crate::weapons::ITEM_FLAG_EXHAUSTIBLE != 0 {
        return;
    }
    let reserve = usize::try_from(weapon.primary_ammo_type)
        .ok()
        .and_then(|t| p.ammo.get(t).copied())
        .unwrap_or(-1);
    if reserve < 1 && weapon.clip == 0 {
        crate::hint::first_time(g, slot, crate::hint::dhf::AMMO_EXHAUSTED);
        crate::hint::hint(g, eng, slot, "#Hint_out_of_ammo");
    }
}

/// `CBasePlayer::UpdateClientData` for the player of `slot`: what the client does not know yet.
/// Called from `PreThink` on every command (player.cpp:4831, read 2026-11-22) and from
/// `RemoveAllItems`; not by the engine's `UpdateClientData` export.
/// On the first update after a spawn (`m_fInitHUD`) the HUD starts over: `ResetHUD`, once per
/// entry `InitHUD` and the rules' welcome, the money, the bomb icon, the round timer, the
/// weather and the team scores; then, every update, what changed: the hidden HUD elements, the
/// field of view, the health, the armor, the damage, the flashlight battery (`FlashBat`), the
/// train HUD, the ammo, the weapons (their `CurWeapon`), the status bar and the out-of-ammo
/// hint.
pub(crate) fn update_client_data(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let index = usize::from(slot);
    let Some(p) = g.players.get_mut(index).and_then(Option::as_mut) else {
        return;
    };
    let (start_over, first_entry) = (p.init_hud, !p.game_hud_initialized);
    p.init_hud = false;
    if start_over {
        // `m_fInitHUD = FALSE; gInitHUD = FALSE;`
        g.init_hud_global = false;
        // Vanilla (`m_signals.Update()`): the zones the player touched count as the ones it
        // stands in, with no icon message for the change. Preserve `SIGNAL_BUY` geometrically so
        // tick-0 buying stays armed until `handle_signals_when_due` emits the `buyzone` icon.
        let in_buy_now = crate::buy::is_in_buy_zone_now(g, eng, slot);
        if let Some(p) = g.players.get_mut(index).and_then(Option::as_mut) {
            if in_buy_now {
                p.signals.signal(crate::buy::SIGNAL_BUY);
            }
            p.signals.update();
            p.shown_zone_signals = p.signals.state() & !crate::buy::SIGNAL_BUY;
        }
        eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::ResetHud);
        if first_entry {
            eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::InitHud);
            if let Some((color, density)) = g.map_fog {
                fog(eng, MsgDest::One(slot), color, density);
            }
            init_hud(g, eng, slot);
            if let Some(p) = g.players.get_mut(index).and_then(Option::as_mut) {
                p.game_hud_initialized = true;
            }
        }
        let (account, has_defuser) = g.players[index]
            .as_ref()
            .map_or((0, false), |p| (p.money, p.has_defuser));
        money(eng, slot, account);
        broadcast_account(g, eng, slot, account);
        if has_defuser {
            eng.send_usermsg(
                MsgDest::One(slot),
                &CsUserMsg::StatusIcon {
                    status: 1,
                    sprite_name: "defuser".to_string(),
                    color: Some([0, 160, 0]),
                },
            );
        }
        crate::buy::set_bomb_icon(g, eng, slot, false);
        sync_round_timer(g, eng, slot);
        crate::hostage::send_hostage_pos_to_player(g, eng, slot);
        send_weather_info(g, eng, slot);
        team_score(eng, slot, Team::Ct, g.num_ct_wins);
        team_score(eng, slot, Team::Terrorist, g.num_terrorist_wins);
    }

    let origin = eng
        .ent(eng.client_edict(slot))
        .map_or(Vec3::ZERO, |v| v.origin);
    let health_now = eng.ent(eng.client_edict(slot)).map_or(0.0, |v| v.health);
    let Some(p) = g.players.get_mut(index).and_then(Option::as_mut) else {
        return;
    };
    if p.hide_hud != p.client_hide_hud {
        eng.send_usermsg(
            MsgDest::One(slot),
            &CsUserMsg::HideWeapon((p.hide_hud & 0xff) as u8),
        );
        p.client_hide_hud = p.hide_hud;
    }
    if p.fov != p.client_fov {
        // The cache is updated at the end of the function, so the weapons' `UpdateClientData`
        // see that the FOV changed. `pev->fov = m_iFOV` first.
        if let Some(v) = eng.ent_mut(eng.client_edict(slot)) {
            v.fov = f32::from(p.fov);
        }
        eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::SetFov(p.fov));
        // `MSG_SPEC` `HLTV` follows: there is no HLTV.
    }
    let obs_hp_update = if health_now as i32 != p.client_health {
        let mut shown = (health_now as i32).clamp(0, 255) as u8;
        if health_now > 0.0 && health_now <= 1.0 {
            shown = 1;
        }
        p.client_health = health_now as i32;
        health(eng, slot, shown);
        Some(if p.alive {
            (health_now as i32).max(0)
        } else {
            0
        })
    } else {
        None
    };
    if p.armor as i32 != p.client_battery {
        p.client_battery = p.armor as i32;
        eng.send_usermsg(
            MsgDest::One(slot),
            &CsUserMsg::Battery(clamp_i16(p.client_battery)),
        );
    }
    if p.dmg_take != 0.0 || p.hud_damage != p.damage_bits as i32 {
        // Damage comes from inside the player if nothing is known to have caused it; a fall
        // shows no direction.
        let mut from = origin;
        if let Some(v) = p.dmg_inflictor.and_then(|i| eng.ent(i)) {
            from = (v.absmin + v.absmax) * 0.5;
        }
        if p.dmg_inflictor == Some(eng.client_edict(slot)) && p.damage_bits & dmg::FALL != 0 {
            from = Vec3::ZERO;
        }
        eng.send_usermsg(
            MsgDest::One(slot),
            &CsUserMsg::Damage {
                armor_damage: 0, // `pev->dmg_save` is never set (`basemonster.cpp`)
                // `WRITE_BYTE(int(pev->dmg_take))`: the integer part, as a byte.
                health_damage: (p.dmg_take as i32) as u8,
                damage_bits: p.damage_bits & dmg::SHOWNHUD,
                origin: from,
            },
        );
        p.dmg_take = 0.0;
        p.hud_damage = p.damage_bits as i32;
        // Non-time-based kinds are not shown again.
        p.damage_bits &= dmg::TIMEBASED;
    }
    if let Some(obs_hp) = obs_hp_update {
        broadcast_health_info(g, eng, slot, obs_hp);
    }
    // "Update Flashlight": the battery drains or charges, `FlashBat`.
    crate::suit::update_flashlight(g, eng, slot);
    let Some(p) = g.players.get_mut(index).and_then(Option::as_mut) else {
        return;
    };
    if p.train_new {
        send_raw(eng, MsgDest::One(slot), "Train", &[0]);
        p.train_new = false;
    }
    // `SendAmmoUpdate`, then every item sends its `CurWeapon` if the HUD is stale.
    crate::weapons::core::send_ammo_update(g, eng, slot);
    crate::weapons::core::update_items_client_data(g, eng, slot);
    // Cache the client's weapon and FOV, after the weapon updates have seen the change.
    let Some(p) = g.players.get_mut(index).and_then(Option::as_mut) else {
        return;
    };
    p.client_active_item = p.active_item;
    p.client_fov = p.fov;
    crate::hud_misc::update_status_bar(g, eng, slot);
    out_of_ammo_hint(g, eng, slot);
    // The end of `UpdateClientData`: every half second the zones the player is in.
    crate::buy::handle_signals_when_due(g, eng, slot);
    crate::hud_misc::update_radar(g, eng, slot);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_sizes(file: &[u8]) -> Vec<(bool, usize)> {
        motd_chunks(file)
            .iter()
            .map(|(finished, c)| (*finished, c.len()))
            .collect()
    }

    #[test]
    fn motd_is_cut_in_chunks_of_sixty() {
        assert!(motd_chunks(b"").is_empty());
        assert_eq!(chunk_sizes(b"hello"), [(true, 5)]);
        assert_eq!(chunk_sizes(&[b'a'; 60]), [(true, 60)]);
        assert_eq!(chunk_sizes(&[b'a'; 61]), [(false, 60), (true, 1)]);
        assert_eq!(
            chunk_sizes(&[b'a'; 150]),
            [(false, 60), (false, 60), (true, 30)]
        );
    }

    #[test]
    fn motd_ends_at_a_nul_and_at_the_length_limit() {
        assert_eq!(chunk_sizes(b"abc\0def"), [(true, 3)]);
        assert!(motd_chunks(b"\0abc").is_empty());
        // The chunk that crosses 1536 is the last one: 26 chunks, 1560 bytes.
        let long = vec![b'x'; 5000];
        let chunks = chunk_sizes(&long);
        assert_eq!(chunks.len(), 26);
        assert_eq!(chunks.iter().map(|c| c.1).sum::<usize>(), 1560);
        assert!(chunks[..25].iter().all(|c| !c.0) && chunks[25].0);
        // A text of exactly 1536 bytes ends on a chunk boundary of 25.6 chunks.
        assert_eq!(chunk_sizes(&vec![b'x'; 1536]).len(), 26);
    }

    #[test]
    fn the_table_registers_every_message_once_with_a_wire_name() {
        let reg = link_user_messages();
        assert_eq!(reg.iter().count(), USER_MESSAGES.len());
        for &(name, _) in USER_MESSAGES {
            assert!(name.len() < 16, "{name}");
        }
        assert_eq!(reg.id_for_name("CurWeapon"), Some(USERMSG_BASE_ID));
        assert!(USER_MESSAGES.len() + usize::from(USERMSG_BASE_ID) <= 255);
    }
}
