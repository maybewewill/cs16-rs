//! The observer: the death camera of the dead and the cameras of the spectators. Ported from
//! ReGameDLL_CS (`observer.cpp`, `player.cpp` `StartDeathCam`/`StartObserver`/`Killed`,
//! `client.cpp` `specmode`/`follownext`/`follow`/`SetupVisibility`/`UpdateClientData`,
//! `pm_shared.cpp` `PM_SpectatorMove`, read 2026-11-22), the vanilla `mp.dll` behaviour (not the
//! `REGAMEDLL_FIXES`/`REGAMEDLL_ADD` builds).
//!
//! What the clients see of an observer lives in the edict: `iuser1` is the mode (`OBS_*`),
//! `iuser2` the edict index of the player it follows (0 when roaming) and `iuser3` the killer
//! while the player is dying (cleared when the observer mode starts). The engine sends them in
//! the client data ([`crate::player::client_data`]); the client draws the camera from them. A
//! player in first person (`OBS_IN_EYE`) is sent the data of the player it follows, but its own
//! `iuser*`.
//!
//! The server side of the camera is `PM_SpectatorMove`, run by `PM_Move` like any movement: a
//! roaming observer flies with friction and acceleration, capped by `sv_spectatormaxspeed`,
//! through everything. One that follows takes the origin of its target in
//! [`setup_visibility`] (`GameDll::setup_visibility`: `UTIL_SetOrigin`), so that the engine sends
//! it what the target sees.
//!
//! Constants: `OBS_*` are `pm_shared.h`, `CAMERA_MODE_*` and `FADETOBLACK_*` `observer.h`,
//! `FFADE_*` `engine/shake.h`, `HIDEHUD_*` `cdll_dll.h` (all read 2026-11-22);
//! `UTIL_ScreenFade` is `util.h`/`util.cpp` (read 2026-11-22).
//!
//! Deviations and gaps:
//! - The state the first-person view copies from the followed player is only partly there. NOT
//!   ported (the bomb and the defuser exist now, the observer side of them does not):
//!   `m_iObserverC4State` and `m_bObserverHasDefuser` (the C4 and defuser icons an observer
//!   sees on the player it follows, and their `StatusIcon` updates in `UpdateClientData`) and
//!   `SetProgressBarTime2` (the plant/defuse bar an observer sees, with the time already run).
//!   The messages that clear that state when the mode changes are sent as the original sends
//!   them. The blindness is copied (a flashbang, [`crate::grenade`]).
//! - `GetPlayerSpawnSpot` ([`crate::world::get_player_spawn_spot`]) does not fire the `target` of
//!   the spawn point (the trigger entities do not exist yet; `spawn` skips it too).
//! - The `mp_fadetoblack` of the dying player is the vanilla one: any value but 0 fades the
//!   screen to black and does not follow the killer. The state the career mode, the bots and
//!   the HLTV (`TheBots`, `MSG_SPEC`) keep is not ported.
//! - The target an observer follows is a slot; the original keeps an `EHANDLE`. A target that left
//!   is found out by [`check_target`] (a free slot), as the original finds out by
//!   `UTIL_PlayerByIndex`; a search then starts from the observer's own index, as with a null
//!   handle.
//! - Tracking mode copies the target's `angles` through `PM_SpectatorMove`, which finds the
//!   target among the observer's physents (within 256 units, as in the original); farther away
//!   only the origin follows ([`setup_visibility`]).

use cs_engine::consts::{FL_DUCKING, MOVETYPE_NONE, SOLID_NOT};
use cs_engine::{EdictId, EngineApi, MsgDest};
use cs_proto48::messages::svc;
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

use crate::GameState;
use crate::menus::{atoi, print_center};
use crate::messages::send_raw;
use crate::player::{
    DEAD_DEAD, DEAD_RESPAWNABLE, EF_NODRAW, HIDEHUD_HEALTH, HIDEHUD_WEAPONS, Team,
};
use crate::teams::player_name;
use crate::weapons::core::DEFAULT_FOV;
use crate::world::atof;

/// `OBS_*`: the observer modes (`pev->iuser1`).
pub const OBS_NONE: i32 = 0;
pub const OBS_CHASE_LOCKED: i32 = 1;
pub const OBS_CHASE_FREE: i32 = 2;
pub const OBS_ROAMING: i32 = 3;
pub const OBS_IN_EYE: i32 = 4;
pub const OBS_MAP_FREE: i32 = 5;
pub const OBS_MAP_CHASE: i32 = 6;

/// `CAMERA_MODE_*`: what `mp_forcecamera` and `mp_forcechasecam` allow.
pub const CAMERA_MODE_SPEC_ANYONE: i32 = 0;
pub const CAMERA_MODE_SPEC_ONLY_TEAM: i32 = 1;
pub const CAMERA_MODE_SPEC_ONLY_FIRST_PERSON: i32 = 2;

/// `mp_fadetoblack 1`: the screen of the dead stays black.
pub(crate) const FADETOBLACK_STAY: i32 = 1;
/// `FFADE_IN`, `FFADE_OUT` and `FFADE_STAYOUT` (`engine/shake.h`).
pub(crate) const FFADE_IN: u16 = 0;
pub(crate) const FFADE_OUT: u16 = 1;
pub(crate) const FFADE_STAYOUT: u16 = 4;
/// `UTIL_ScreenFade` writes the seconds as fixed point with 12 fractional bits.
const FADE_UNIT: f32 = 4096.0;
/// `TE_KILLPLAYERATTACHMENTS`.
const TE_KILLPLAYERATTACHMENTS: u8 = 125;
/// Seconds before a follower may pick the next player again (`m_flNextFollowTime`).
const FOLLOW_INTERVAL: f64 = 0.25;
/// Seconds between two button presses of an observer (`m_flNextObserverInput`).
const INPUT_INTERVAL: f64 = 0.2;
/// Seconds a followed player has been dead before its follower looks for another.
const DEAD_TARGET_GRACE: f64 = 2.0;
/// `IN_ATTACK`, `IN_JUMP`, `IN_ATTACK2`.
const IN_ATTACK: u16 = 1 << 0;
const IN_JUMP: u16 = 1 << 1;
const IN_ATTACK2: u16 = 1 << 11;
/// `VEC_HULL_MIN.z - VEC_DUCK_HULL_MIN.z`: how much lower a ducking player's eye is taken.
const DUCK_EYE_DROP: f32 = -18.0;

/// What `CBasePlayer` keeps for the observer (`m_hObserverTarget`, `m_iObserverLastMode`,
/// `m_flNextFollowTime`, `m_flNextObserverInput`, `m_bWasFollowing`, `m_canSwitchObserverModes`,
/// `m_bObserverAutoDirector`).
#[derive(Debug, Clone)]
pub(crate) struct ObserverState {
    /// The slot of the player followed.
    pub target: Option<u8>,
    /// The mode `StartObserver` goes back to, and the one the player last asked for.
    pub last_mode: i32,
    pub next_follow_time: f64,
    pub next_input: f64,
    /// The followed player is gone and the observer roams until it finds another.
    pub was_following: bool,
    /// Whether `specmode` and `follownext` are heard (not while dying).
    pub can_switch: bool,
    pub auto_director: bool,
}

impl Default for ObserverState {
    fn default() -> Self {
        Self {
            target: None,
            last_mode: OBS_ROAMING,
            next_follow_time: 0.0,
            next_input: 0.0,
            was_following: false,
            can_switch: true,
            auto_director: false,
        }
    }
}

/// The mode of the observer of `slot` (`GetObserverMode`, `pev->iuser1`).
pub(crate) fn mode_of(eng: &EngineApi<'_>, slot: u8) -> i32 {
    eng.ent(eng.client_edict(slot)).map_or(OBS_NONE, |v| v.iuser1)
}

/// `GetForceCamera`: what a dead player may watch. `mp_fadetoblack 1` allows only the first
/// person view of its own (black) screen; otherwise `mp_forcechasecam`, and when that allows
/// anyone, `mp_forcecamera`.
pub(crate) fn force_camera(eng: &EngineApi<'_>) -> i32 {
    if eng.cvar_f32("mp_fadetoblack") as i32 != FADETOBLACK_STAY {
        let chase = eng.cvar_f32("mp_forcechasecam") as i32;
        if chase == CAMERA_MODE_SPEC_ANYONE {
            eng.cvar_f32("mp_forcecamera") as i32
        } else {
            chase
        }
    } else {
        CAMERA_MODE_SPEC_ONLY_FIRST_PERSON
    }
}

/// The edict index of `slot` (`ENTINDEX`).
fn index_of(slot: u8) -> i32 {
    i32::from(slot) + 1
}

/// `Observer_IsValidTarget`: the player of edict index `index`, when `observer` may follow it:
/// not itself, not an observer or an invisible player, not on no team, and, when
/// `same_team`, on the observer's team.
fn valid_target(
    g: &GameState,
    eng: &EngineApi<'_>,
    observer: u8,
    index: i32,
    same_team: bool,
) -> Option<u8> {
    if index > i32::from(eng.max_clients()) || index < 1 {
        return None;
    }
    let slot = u8::try_from(index - 1).ok()?;
    if slot == observer {
        return None;
    }
    let target = g.players.get(usize::from(slot))?.as_ref()?;
    let own_team = g.players.get(usize::from(observer))?.as_ref()?.team;
    let v = eng.ent(eng.client_edict(slot))?;
    if v.iuser1 != OBS_NONE || v.effects & EF_NODRAW != 0 {
        return None;
    }
    if target.team == Team::Unassigned || (same_team && target.team != own_team) {
        return None;
    }
    Some(slot)
}

/// The target of the observer of `slot`, when it is still a player in the game.
fn current_target(g: &GameState, slot: u8) -> Option<u8> {
    let target = g.players.get(usize::from(slot))?.as_ref()?.obs.target?;
    g.players.get(usize::from(target))?.as_ref().map(|_| target)
}

fn set_target(g: &mut GameState, slot: u8, target: Option<u8>) {
    if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        p.obs.target = target;
    }
}

/// `UTIL_ScreenFade(player, Vector(0, 0, 0), fade_time, hold, alpha, flags)` (`util.cpp`): the
/// seconds go out as fixed point with 12 fractional bits (`FixedUnsigned16`, clamped to 16
/// bits), the colour is black.
pub(crate) fn screen_fade(
    eng: &mut EngineApi<'_>,
    slot: u8,
    fade_time: f32,
    hold: f32,
    alpha: u8,
    flags: u16,
) {
    screen_fade_color(eng, slot, [0, 0, 0], fade_time, hold, alpha, flags);
}

/// `UTIL_ScreenFade(player, color, fade_time, hold, alpha, flags)` (`util.cpp` 546-593, read
/// 2026-11-22): `UTIL_ScreenFadeBuild` (the seconds as `FixedUnsigned16(value, 1 << 12)`, the
/// colour truncated to bytes) and `UTIL_ScreenFadeWrite` (`ScreenFade` to the one player).
pub(crate) fn screen_fade_color(
    eng: &mut EngineApi<'_>,
    slot: u8,
    color: [u8; 3],
    fade_time: f32,
    hold: f32,
    alpha: u8,
    flags: u16,
) {
    let fixed = |seconds: f32| (seconds * FADE_UNIT).clamp(0.0, 65535.0) as u16;
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::ScreenFade {
            duration: fixed(fade_time),
            hold_time: fixed(hold),
            flags,
            color: [color[0], color[1], color[2], alpha],
        },
    );
}

/// `UpdateClientEffects` (`observer.cpp` 49-137, read 2026-11-22): what the observer's screen shows
/// of the player it followed in first person is taken away when the mode changes. The one
/// thing it copies is the blindness: while the followed player is blind from a flashbang (and
/// `mp_fadetoblack` is not 1) the observer is faded white as the player is, for what is left of
/// the flash; a flash that has passed fades the screen back in. (The progress bar and the night
/// vision of the followed player are not copied: see the module docs.)
fn update_client_effects(g: &GameState, eng: &mut EngineApi<'_>, slot: u8, old_mode: i32) {
    let mode = mode_of(eng, slot);
    let blindness_ok = eng.cvar_f32("mp_fadetoblack") as i32 != FADETOBLACK_STAY;
    if mode != OBS_IN_EYE && old_mode != OBS_IN_EYE {
        return;
    }
    // `SetProgressBarTime(0)`.
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::BarTime(0));
    let mut clear_blindness = true;
    if mode == OBS_IN_EYE
        && blindness_ok
        && let Some(target) = current_target(g, slot)
        && let Some(p) = g.players.get(usize::from(target)).and_then(Option::as_ref)
        && p.blind_start_time != 0.0
        && p.blind_fade_time != 0.0
    {
        let now = eng.time() as f32;
        let end_time = p.blind_fade_time + p.blind_hold_time + p.blind_start_time;
        if end_time > now {
            clear_blindness = false;
            let mut fade_time = p.blind_fade_time;
            let mut alpha = p.blind_alpha as f32;
            let mut hold_time = p.blind_hold_time + p.blind_start_time - now;
            if hold_time <= 0.0 {
                hold_time = 0.0;
                let ratio = (end_time - now) / fade_time;
                alpha = p.blind_alpha as f32 * ratio;
                fade_time *= ratio;
            }
            // `UTIL_ScreenFade(pObserver, Vector(255, 255, 255), fadeTime, holdTime, alpha)`:
            // the float alpha is cut to an int, and to a byte on the wire.
            screen_fade_color(eng, slot, [255, 255, 255], fade_time, hold_time, alpha as i32 as u8, FFADE_IN);
        }
    }
    if blindness_ok && clear_blindness {
        // `UTIL_ScreenFade(pObserver, Vector(0, 0, 0), 0.001)`: the defaults of the rest, an
        // alpha of 0 and no flags.
        screen_fade(eng, slot, 0.001, 0.0, 0, FFADE_IN);
    }
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::NvgToggle(false));
}

/// `CBasePlayer::Observer_FindNextPlayer`: the observer of `slot` follows the next player (the
/// previous one with `reverse`), or, with `name`, the next one of that name. Not oftener than
/// every [`FOLLOW_INTERVAL`] seconds. Moves the observer to the target.
pub(crate) fn find_next_player(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    reverse: bool,
    name: Option<&str>,
) {
    let now = eng.time();
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    if p.obs.next_follow_time != 0.0 && p.obs.next_follow_time > now {
        return;
    }
    p.obs.next_follow_time = now + FOLLOW_INTERVAL;
    let own_team = p.team;
    let old_target = p.obs.target;
    p.obs.target = None;
    // The old target that left is a null handle in the original: the search starts at the
    // observer itself.
    let start = old_target
        .filter(|&t| g.players.get(usize::from(t)).is_some_and(Option::is_some))
        .map_or(index_of(slot), index_of);

    let dir = if reverse { -1 } else { 1 };
    let same_team = force_camera(eng) != CAMERA_MODE_SPEC_ANYONE && own_team != Team::Spectator;
    let max = i32::from(eng.max_clients());
    let mut current = start;
    let mut target: Option<u8>;
    loop {
        current += dir;
        if current > max {
            current = 1;
        } else if current < 1 {
            current = max;
        }
        target = valid_target(g, eng, slot, current, same_team);
        if let Some(t) = target
            && name.is_none_or(|n| player_name(eng, t) == n)
        {
            break;
        }
        if current == start {
            break;
        }
    }
    set_target(g, slot, target);

    // Did we find a target? Move to it.
    let Some(t) = target else {
        return;
    };
    let id = eng.client_edict(slot);
    let target_id = eng.client_edict(t);
    let Some(tv) = eng.ent_mut(target_id) else {
        return;
    };
    if tv.health < 0.0 {
        tv.health = 0.0;
    }
    let (origin, health) = (tv.origin, tv.health);
    eng.set_origin(id, origin);
    // `SpecHealth2`: the health of the followed player and its index.
    send_raw(
        eng,
        MsgDest::One(slot),
        "SpecHealth2",
        &[health as i32 as u8, index_of(t) as u8],
    );
    // Store the target in the edict, for the movement and the clients.
    let old_mode = mode_of(eng, slot);
    if old_mode != OBS_ROAMING
        && let Some(v) = eng.ent_mut(id)
    {
        v.iuser2 = index_of(t);
    }
    update_client_effects(g, eng, slot, old_mode);
}

/// `CBasePlayer::Observer_SetMode`: the observer of `slot` asks for mode `mode`. Mode and target
/// are corrected by `mp_forcecamera`/`mp_forcechasecam`; a mode that follows needs a target, and
/// with none the observer roams.
pub(crate) fn set_mode(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, mode: i32) {
    let id = eng.client_edict(slot);
    let Some(old_mode) = eng.ent(id).map(|v| v.iuser1) else {
        return;
    };
    // Just abort if we're changing to the mode we're already in.
    if mode == old_mode {
        return;
    }
    let force = force_camera(eng);
    let Some(own_team) = g.players.get(usize::from(slot)).and_then(Option::as_ref).map(|p| p.team)
    else {
        return;
    };
    // Is it a valid mode?
    let mut mode = if (OBS_CHASE_LOCKED..=OBS_MAP_CHASE).contains(&mode) {
        mode
    } else {
        OBS_IN_EYE
    };
    if own_team != Team::Spectator {
        if force == CAMERA_MODE_SPEC_ONLY_TEAM {
            if mode == OBS_ROAMING {
                mode = OBS_MAP_FREE;
            }
        } else if force == CAMERA_MODE_SPEC_ONLY_FIRST_PERSON {
            mode = OBS_IN_EYE;
        }
    }
    // Verify the observer target again (the vanilla check, which does not look at the team of a
    // spectator the way `Observer_IsValidTarget` does).
    if let Some(t) = current_target(g, slot) {
        let target_team = g.players[usize::from(t)].as_ref().map(|p| p.team);
        let invalid = t == slot
            || eng.ent(eng.client_edict(t)).is_none_or(|v| {
                v.iuser1 != OBS_NONE
                    || v.effects & EF_NODRAW != 0
                    || (force != CAMERA_MODE_SPEC_ANYONE && target_team != Some(own_team))
            });
        if invalid {
            set_target(g, slot, None);
        }
    } else {
        set_target(g, slot, None);
    }
    // Set the spectator mode.
    if let Some(v) = eng.ent_mut(id) {
        v.iuser1 = mode;
    }
    // If we are not roaming, we need a valid target to track.
    if mode != OBS_ROAMING && current_target(g, slot).is_none() {
        find_next_player(g, eng, slot, false, None);
        // If we didn't find a valid target switch to roaming.
        if current_target(g, slot).is_none() {
            print_center(eng, slot, "#Spec_NoTarget");
            if let Some(v) = eng.ent_mut(id) {
                v.iuser1 = OBS_ROAMING;
            }
        }
    }
    let target = current_target(g, slot);
    let target_origin = target.and_then(|t| eng.ent(eng.client_edict(t)).map(|v| v.origin));
    if let Some(v) = eng.ent_mut(id) {
        // Set the target if not roaming; clear the second target from the death camera.
        v.iuser2 = match target {
            Some(t) if v.iuser1 != OBS_ROAMING => index_of(t),
            _ => 0,
        };
        v.iuser3 = 0;
    }
    if let Some(origin) = target_origin {
        eng.set_origin(id, origin);
    }
    // `gmsgCrosshair`: the crosshair shows only while roaming.
    send_raw(eng, MsgDest::One(slot), "Crosshair", &[u8::from(mode == OBS_ROAMING)]);
    update_client_effects(g, eng, slot, old_mode);
    // Print the spectator mode on the client's screen.
    let shown = eng.ent(id).map_or(mode, |v| v.iuser1);
    print_center(eng, slot, &format!("#Spec_Mode{shown}"));
    if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        p.obs.last_mode = mode;
        p.obs.was_following = false;
    }
}

/// `CBasePlayer::Observer_HandleButtons`: jump changes the mode, attack follows the next player,
/// attack2 the previous one; not oftener than every [`INPUT_INTERVAL`] seconds.
fn handle_buttons(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time();
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    // Slow down mouse clicks.
    if p.obs.next_input > now {
        return;
    }
    let (pressed, auto_director) = (p.buttons_pressed, p.obs.auto_director);
    let mut used = false;
    if pressed & IN_JUMP != 0 {
        let next = match mode_of(eng, slot) {
            OBS_CHASE_LOCKED | OBS_CHASE_FREE => OBS_IN_EYE,
            OBS_IN_EYE => OBS_ROAMING,
            OBS_ROAMING => OBS_MAP_FREE,
            OBS_MAP_FREE => OBS_MAP_CHASE,
            _ => {
                if auto_director {
                    OBS_CHASE_LOCKED
                } else {
                    OBS_CHASE_FREE
                }
            }
        };
        set_mode(g, eng, slot, next);
        used = true;
    }
    if pressed & IN_ATTACK != 0 {
        find_next_player(g, eng, slot, false, None);
        used = true;
    }
    if pressed & IN_ATTACK2 != 0 {
        find_next_player(g, eng, slot, true, None);
        used = true;
    }
    if used && let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        p.obs.next_input = now + INPUT_INTERVAL;
    }
}

/// `CBasePlayer::Observer_CheckTarget`: finds a target when there is none, another when the
/// followed player is gone, has no body to watch or has been dead for two seconds, and roams
/// when there is nobody (going back to the mode it asked for when somebody appears).
fn check_target(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time();
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    let was_following = p.obs.was_following;
    if mode_of(eng, slot) == OBS_ROAMING && !was_following {
        return;
    }
    if was_following {
        find_next_player(g, eng, slot, false, None);
        if current_target(g, slot).is_some() {
            let last = g.players[usize::from(slot)].as_ref().map_or(OBS_ROAMING, |p| p.obs.last_mode);
            set_mode(g, eng, slot, last);
        }
        return;
    }
    // Try to find a target if we have none.
    if p.obs.target.is_none() {
        find_next_player(g, eng, slot, false, None);
    }
    let roam = |g: &mut GameState, eng: &mut EngineApi<'_>, following: bool| {
        // Don't overwrite the user's last mode.
        let last = mode_of(eng, slot);
        set_mode(g, eng, slot, OBS_ROAMING);
        if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
            p.obs.last_mode = last;
            p.obs.was_following |= following;
        }
    };
    let has_target = g.players[usize::from(slot)]
        .as_ref()
        .is_some_and(|p| p.obs.target.is_some());
    if !has_target {
        // No target found at all.
        roam(g, eng, false);
        return;
    }
    // Check the target.
    let Some(t) = current_target(g, slot) else {
        find_next_player(g, eng, slot, false, None);
        return;
    };
    let dead_time = g.players[usize::from(t)].as_ref().map_or(0.0, |p| p.dead_time);
    let Some((deadflag, effects)) = eng.ent(eng.client_edict(t)).map(|v| (v.deadflag, v.effects))
    else {
        find_next_player(g, eng, slot, false, None);
        return;
    };
    if deadflag == DEAD_RESPAWNABLE || effects & EF_NODRAW != 0 {
        find_next_player(g, eng, slot, false, None);
    } else if deadflag == DEAD_DEAD && now > dead_time + DEAD_TARGET_GRACE {
        // Two seconds after the death, change the target.
        find_next_player(g, eng, slot, false, None);
        if g.players[usize::from(slot)]
            .as_ref()
            .is_some_and(|p| p.obs.target.is_none())
        {
            roam(g, eng, true);
        }
    }
}

/// `CBasePlayer::Observer_CheckProperties`: in first person the observer takes the field of view
/// of the player it follows; otherwise the default one. (The weapon, C4 and defuser icons of the
/// followed player and `SetProgressBarTime2` are not ported: see the module doc.)
fn check_properties(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let target = if mode_of(eng, slot) == OBS_IN_EYE {
        current_target(g, slot)
    } else {
        None
    };
    let target_fov = target.and_then(|t| g.players[usize::from(t)].as_ref().map(|p| p.fov));
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    match target_fov {
        Some(fov) => {
            // Use the field of view of the tracked client.
            if p.fov != fov {
                p.fov = fov;
                p.client_fov = fov;
                eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::SetFov(fov));
            }
        }
        None => p.fov = DEFAULT_FOV,
    }
}

/// `CBasePlayer::Observer_Think`, run from `PlayerPreThink` of an observer.
fn think(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    handle_buttons(g, eng, slot);
    check_target(g, eng, slot);
    check_properties(g, eng, slot);
}

/// `StartDeathCam` as `PlayerDeathThink` calls it after it has sent the corpse: the dead player
/// becomes an observer where it died. Nothing happens for one that already is (its view offset
/// is zero then). Vanilla lets the player switch modes from the corpse on
/// (`m_canSwitchObserverModes`, set after `SpawnClientSideCorpse`).
pub(crate) fn start_death_cam(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        p.obs.can_switch = true;
    }
    let id = eng.client_edict(slot);
    let Some(v) = eng.ent(id) else {
        return;
    };
    // Don't accept subsequent attempts to `StartDeathCam`.
    if v.view_ofs == Vec3::ZERO {
        return;
    }
    let (origin, angles) = (v.origin, v.angles);
    start_observer(g, eng, slot, origin, angles);
}

/// `CBasePlayer::StartObserver`: the player of `slot` becomes an observer at `position`, looking
/// along `view_angle`: invisible, not solid, taking no damage, with its weapons and health
/// hidden, and in the mode it last used.
pub(crate) fn start_observer(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    position: Vec3,
    view_angle: Vec3,
) {
    let id = eng.client_edict(slot);
    let Some(current_origin) = eng.ent(id).map(|v| v.origin) else {
        return;
    };
    // Clear any clientside entities attached to this player.
    eng.send(
        MsgDest::Pas(current_origin),
        &[svc::TEMP_ENTITY, TE_KILLPLAYERATTACHMENTS, index_of(slot) as u8],
    );
    // Holster the weapon immediately, to let it clean up (`m_pActiveItem->Holster()`,
    // player.cpp:4163, read 2026-11-22).
    let active = g.players.get(usize::from(slot)).and_then(Option::as_ref).and_then(|p| p.active_item);
    if let Some(active) = active {
        crate::weapons::core::with_weapon(g, eng, slot, active, |b, w| b.holster(w));
    }
    // Tell the ammo HUD that the player is dead. (`DetachTank` and the suit sentence cache
    // `SetSuitUpdate(nullptr, ...)` follow in the source; there is no func_tank, and the suit's
    // sentence queue is not ported, so there is nothing to let go.)
    eng.send_usermsg(
        MsgDest::One(slot),
        &CsUserMsg::CurWeapon {
            is_active: false,
            weapon_id: 0xFF,
            clip_ammo: -1,
        },
    );
    // Reset the field of view.
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::SetFov(0));
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    p.fov = 0;
    p.client_fov = 0;
    // Setup the flags.
    p.hide_hud = HIDEHUD_WEAPONS | HIDEHUD_HEALTH;
    p.pflag_observer = true;
    p.takes_damage = false;
    p.health = 1.0;
    // Find a player to watch.
    p.obs.next_input = 0.0;
    let last_mode = p.obs.last_mode;
    if let Some(v) = eng.ent_mut(id) {
        v.fov = 0.0;
        v.effects = EF_NODRAW;
        // Set the position and the view angles.
        v.view_ofs = Vec3::ZERO;
        v.angles = view_angle;
        v.v_angle = view_angle;
        v.fixangle = 1;
        v.solid = SOLID_NOT;
        v.movetype = MOVETYPE_NONE;
        v.flags &= !FL_DUCKING;
        v.health = 1.0;
        v.iuser1 = OBS_NONE;
    }
    // Move it to the new position.
    eng.set_origin(id, position);
    set_mode(g, eng, slot, last_mode);
    crate::player::reset_max_speed(g, eng, slot);
    // Tell all clients this player is now a spectator.
    send_raw(eng, MsgDest::All, "Spectator", &[index_of(slot) as u8, 1]);
    crate::messages::sync_observer_hud_info(g, eng, slot);
}

/// The part of `CBasePlayer::Killed` that concerns the camera: a dying player cannot switch
/// modes; with `mp_fadetoblack 0` its camera chases the killer (`OBS_CHASE_FREE`, `iuser2` itself,
/// `iuser3` the attacker), otherwise its screen fades to black.
pub(crate) fn killed(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    attacker: Option<EdictId>,
) {
    if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        p.obs.can_switch = false;
    }
    if eng.cvar_f32("mp_fadetoblack") == 0.0 {
        let killer = attacker.and_then(|a| crate::player::player_slot(g, eng, a));
        let id = eng.client_edict(slot);
        if let Some(v) = eng.ent_mut(id) {
            v.iuser1 = OBS_CHASE_FREE;
            v.iuser2 = index_of(slot);
            v.iuser3 = attacker.map_or(0, |a| i32::from(a.index));
        }
        set_target(g, slot, killer.map(|k| k as u8));
        // `gmsgADStop`: the auto director is off.
        send_raw(eng, MsgDest::One(slot), "ADStop", &[]);
    } else {
        // `UTIL_ScreenFade(this, Vector(0, 0, 0), 3, 3, 255, FFADE_OUT | FFADE_STAYOUT)`.
        screen_fade(eng, slot, 3.0, 3.0, 255, FFADE_OUT | FFADE_STAYOUT);
    }
}

/// The night vision half of `CBasePlayer::Killed`, which comes right after `PlayerKilled` and
/// before the camera ([`killed`]): the dead player's goggles go off (`NVGToggle 0`), and so do
/// those of the observers that watch it in first person.
pub(crate) fn killed_night_vision(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::NvgToggle(false));
    if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        p.night_vision_on = false;
    }
    let victim_index = index_of(slot);
    for observer in 0..g.players.len() as u8 {
        let watching = g.players[usize::from(observer)].is_some()
            && eng
                .ent(eng.client_edict(observer))
                .is_some_and(|v| v.iuser1 == OBS_IN_EYE && v.iuser2 == victim_index);
        if watching {
            eng.send_usermsg(MsgDest::One(observer), &CsUserMsg::NvgToggle(false));
            if let Some(p) = g.players[usize::from(observer)].as_mut() {
                p.night_vision_on = false;
            }
        }
    }
}

/// The observer part of `CHalfLifeMultiplay::ClientDisconnected` (ReGameDLL
/// `multiplay_gamerules.cpp:3580`, read 2026-11-22): a spectator that was chasing the player of
/// slot `left` (already out of [`GameState::players`]) has its mode cleared and asked for again, so
/// `Observer_SetMode` looks for another player to follow.
pub(crate) fn on_target_left(g: &mut GameState, eng: &mut EngineApi<'_>, left: u8) {
    for slot in 0..g.players.len() as u8 {
        let follows = slot != left
            && g.players[usize::from(slot)]
                .as_ref()
                .is_some_and(|p| p.obs.target == Some(left));
        if !follows {
            continue;
        }
        let id = eng.client_edict(slot);
        let Some(v) = eng.ent_mut(id) else {
            continue;
        };
        let mode = v.iuser1;
        v.iuser1 = OBS_NONE;
        set_mode(g, eng, slot, mode);
    }
}

/// `CBasePlayer::Spawn`: a player in the game follows nobody.
pub(crate) fn on_spawn(p: &mut crate::player::Player) {
    p.obs.target = None;
    p.obs.can_switch = true;
    p.pflag_observer = false;
}

/// What `PlayerPreThink` does for an observer (`Observer_Think`). The movement is the engine's
/// `PM_Move` after it (`PM_SpectatorMove`, [`crate::pmove::run_pmove`]).
pub(crate) fn run_cmd(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    think(g, eng, slot);
}

/// `SetupVisibility`: the point a player sees the world from. An observer that follows somebody
/// uses the followed player, anybody else (and an observer that roams) the player itself: its
/// origin plus its view offset, 18 units lower when it ducks.
pub(crate) fn setup_visibility(
    g: &GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
) -> Option<Vec3> {
    let p = g.players.get(usize::from(slot))?.as_ref()?;
    let own_id = eng.client_edict(slot);
    let own = eng.ent(own_id)?;
    let mut view = own;
    let mut follow = None;
    if own.iuser2 != 0
        && p.pflag_observer
        && let Some(target) = current_target(g, slot)
    {
        let target_id = eng.client_edict(target);
        view = eng.ent(target_id)?;
        follow = Some(view.origin);
    }
    let mut eye = view.origin + view.view_ofs;
    if view.flags & FL_DUCKING != 0 {
        eye.z += DUCK_EYE_DROP;
    }
    // `UTIL_SetOrigin(pPlayer->pev, m_hObserverTarget->pev->origin)` (client.cpp
    // `SetupVisibility`, read 2026-11-22): the observer's origin follows its target, which
    // `PM_SpectatorMove` cannot find among the physents beyond 256 units.
    if let Some(origin) = follow {
        eng.set_origin(own_id, origin);
    }
    Some(eye)
}

/// The player whose state a first-person observer is sent as its own client data
/// (`UpdateClientData`): the one it follows, in `OBS_IN_EYE`.
pub(crate) fn first_person_source(g: &GameState, eng: &EngineApi<'_>, slot: u8) -> Option<u8> {
    if mode_of(eng, slot) != OBS_IN_EYE {
        return None;
    }
    current_target(g, slot)
}

/// `Observer_FindNextPlayer`'s other caller: the `SpecHealth` of a player that took damage goes
/// to the observers that follow it (`TakeDamage`).
pub(crate) fn notify_spec_health(g: &GameState, eng: &mut EngineApi<'_>, victim: u8) {
    let health = eng
        .ent(eng.client_edict(victim))
        .map_or(0.0, |v| v.health.max(0.0));
    for (i, p) in g.players.iter().enumerate() {
        if p.as_ref().is_some_and(|p| p.obs.target == Some(victim)) {
            send_raw(eng, MsgDest::One(i as u8), "SpecHealth", &[health as i32 as u8]);
        }
    }
}

/// The observer commands of `ClientCommand_` (`client.cpp`): `specmode N` picks a mode,
/// `follownext [reverse]` and `follow name` pick the player to watch, `spec_set_ad` turns the
/// auto director on. Returns whether `argv[0]` was one of them.
pub(crate) fn command(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    argv: &[String],
) -> bool {
    let Some(command) = argv.first() else {
        return false;
    };
    let arg = argv.get(1).map_or("", String::as_str);
    let observing = mode_of(eng, slot) != OBS_NONE
        && g.players
            .get(usize::from(slot))
            .and_then(Option::as_ref)
            .is_some_and(|p| p.obs.can_switch);
    match command.as_str() {
        "specmode" => {
            let mode = atoi(arg);
            if observing {
                set_mode(g, eng, slot, mode);
            } else if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
                p.obs.last_mode = mode;
            }
            if mode == OBS_CHASE_FREE {
                send_raw(eng, MsgDest::One(slot), "ADStop", &[]);
            }
        }
        "spec_set_ad" => {
            let on = atof(arg) > 0.0;
            if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
                p.obs.auto_director = on;
            }
        }
        "follownext" => {
            if observing {
                find_next_player(g, eng, slot, atoi(arg) != 0, None);
            }
        }
        "follow" => {
            if observing {
                find_next_player(g, eng, slot, false, Some(arg));
            }
        }
        _ => return false,
    }
    true
}
