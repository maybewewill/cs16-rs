//! What the HEV suit does for the HUD of a player: the flashlight (`FlashlightTurnOn`,
//! `FlashlightTurnOff`, `FlashlightIsOn`, `EF_DIMLIGHT` and the battery that drains while it is
//! on and recharges while it is off, `FlashBat`) and the Geiger counter
//! (`CheckSuitUpdate` -> `UpdateGeigerCounter`). Ported from ReGameDLL_CS `player.cpp`
//! (`FlashlightIsOn`, `FlashlightTurnOn`, `FlashlightTurnOff`, the flashlight part of
//! `UpdateClientData`, `UpdateGeigerCounter`, `CheckSuitUpdate`, `Spawn`, `Precache`) and
//! `player.h` (`FLASH_DRAIN_TIME`, `FLASH_CHARGE_TIME`, `SOUND_FLASHLIGHT_*`), read 2026-11-22,
//! the vanilla `mp.dll` behaviour (not the `REGAMEDLL_FIXES` build): the flashlight is not
//! switched off by death, and a spawn clears `EF_DIMLIGHT` with the rest of the effects, with
//! no `Flashlight` message.
//!
//! The Geiger range is only ever lowered by radiation (`trigger_hurt` with radiation), which
//! does not exist here; the counter then tells the client its starting range once, as the
//! original does (`m_igeigerRangePrev` starts at 1000, the byte it sends at 250).
//!
//! `CheckSuitUpdate` goes on with the HEV voice in single player; a multiplayer game returns
//! after the Geiger (`IsMultiplayer()`), so the suit sentences are not ported.

use cs_engine::{EngineApi, MsgDest};

use crate::GameState;
use crate::messages::send_raw;
use crate::player::{ATTN_NORM, CHAN_ITEM, PITCH_NORM};
use crate::weapons::WEAPON_SUIT;

/// `EF_DIMLIGHT` (`const.h`): the player's flashlight.
pub(crate) const EF_DIMLIGHT: u32 = 1 << 3;
/// `FLASH_DRAIN_TIME`: seconds per battery unit while the light is on (the header's comment
/// says "100 units/3 minutes", the constant is 1.2).
pub(crate) const FLASH_DRAIN_TIME: f32 = 1.2;
/// `FLASH_CHARGE_TIME`: seconds per battery unit while it is off.
pub(crate) const FLASH_CHARGE_TIME: f32 = 0.2;
/// `SOUND_FLASHLIGHT_ON` and `SOUND_FLASHLIGHT_OFF` (the same sample).
const SOUND_FLASHLIGHT: &str = "items/flashlight1.wav";

/// `CBasePlayer::FlashlightIsOn`: `pev->effects & EF_DIMLIGHT`.
pub(crate) fn flashlight_is_on(eng: &EngineApi<'_>, slot: u8) -> bool {
    eng.ent(eng.client_edict(slot))
        .is_some_and(|v| v.effects & EF_DIMLIGHT != 0)
}

/// `CBasePlayer::FlashlightTurnOn`: nothing when the rules do not allow flashlights
/// (`FAllowFlashlight`, `mp_flashlight`); with the suit the sound, `EF_DIMLIGHT`, `Flashlight 1
/// <battery>` and the drain clock.
pub(crate) fn flashlight_turn_on(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if eng.cvar_f32("mp_flashlight") == 0.0 {
        return;
    }
    let id = eng.client_edict(slot);
    let now = eng.time() as f32;
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    if p.weapons & (1 << WEAPON_SUIT) == 0 {
        return;
    }
    let battery = p.flash_battery;
    p.flash_light_time = now + FLASH_DRAIN_TIME;
    eng.emit_sound(id, CHAN_ITEM, SOUND_FLASHLIGHT, 1.0, ATTN_NORM, PITCH_NORM);
    if let Some(v) = eng.ent_mut(id) {
        v.effects |= EF_DIMLIGHT;
    }
    send_raw(eng, MsgDest::One(slot), "Flashlight", &[1, battery as u8]);
}

/// `CBasePlayer::FlashlightTurnOff`: the sound, `Flashlight 0 <battery>`, and the charge
/// clock.
pub(crate) fn flashlight_turn_off(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    let now = eng.time() as f32;
    eng.emit_sound(id, CHAN_ITEM, SOUND_FLASHLIGHT, 1.0, ATTN_NORM, PITCH_NORM);
    if let Some(v) = eng.ent_mut(id) {
        v.effects &= !EF_DIMLIGHT;
    }
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    let battery = p.flash_battery;
    p.flash_light_time = now + FLASH_CHARGE_TIME;
    send_raw(eng, MsgDest::One(slot), "Flashlight", &[0, battery as u8]);
}

/// Impulse 100 of `ImpulseCommands`, "temporary flashlight for level designers": the light
/// goes off if it is on, else on.
pub(crate) fn flashlight_toggle(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if flashlight_is_on(eng, slot) {
        flashlight_turn_off(g, eng, slot);
    } else {
        flashlight_turn_on(g, eng, slot);
    }
}

/// The "Update Flashlight" block of `UpdateClientData`: when the clock is due the battery
/// drains one unit (the light going off at 0) or, with the light off, charges one (the clock
/// stopping at 100); `FlashBat <battery>` follows either way.
pub(crate) fn update_flashlight(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time() as f32;
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    if p.flash_light_time == 0.0 || p.flash_light_time > now {
        return;
    }
    if flashlight_is_on(eng, slot) {
        if p.flash_battery != 0 {
            p.flash_light_time = now + FLASH_DRAIN_TIME;
            p.flash_battery -= 1;
            if p.flash_battery <= 0 {
                flashlight_turn_off(g, eng, slot);
            }
        }
    } else if p.flash_battery < 100 {
        p.flash_light_time = now + FLASH_CHARGE_TIME;
        p.flash_battery += 1;
    } else {
        p.flash_light_time = 0.0;
    }
    let battery = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .map_or(0, |p| p.flash_battery);
    send_raw(eng, MsgDest::One(slot), "FlashBat", &[battery as u8]);
}

/// `CBasePlayer::CheckSuitUpdate` of a multiplayer game: nothing without the suit; the Geiger
/// counter is updated; the rest (the HEV voice) is single player only.
pub(crate) fn check_suit_update(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let has_suit = g
        .players
        .get(usize::from(slot))
        .and_then(Option::as_ref)
        .is_some_and(|p| p.weapons & (1 << WEAPON_SUIT) != 0);
    if has_suit {
        update_geiger_counter(g, eng, slot);
    }
}

/// `CBasePlayer::UpdateGeigerCounter`: at most every quarter second, the range to the nearest
/// radiation source goes to the client when it changed (`Geiger <range / 4>`), and one time in
/// four the range counter is reset to 1000 ("reset counter and semaphore").
fn update_geiger_counter(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let now = eng.time() as f32;
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    // "delay per update ie: don't flood net with these msgs"
    if now < p.geiger_delay {
        return;
    }
    // `m_flgeigerDelay = gpGlobals->time + 0.25` adds a float and a double and stores a float.
    p.geiger_delay = (f64::from(now) + 0.25) as f32;
    // `byte(m_flgeigerRange / 4.0)`: the quotient is a double, cut to a byte.
    let range = (f64::from(p.geiger_range) / 4.0) as i64 as u8;
    let mut send = None;
    if i32::from(range) != p.geiger_range_prev {
        p.geiger_range_prev = i32::from(range);
        send = Some(range);
    }
    if let Some(range) = send {
        send_raw(eng, MsgDest::One(slot), "Geiger", &[range]);
    }
    // `if (!RANDOM_LONG(0, 3)) m_flgeigerRange = 1000.0`.
    if g.random_range(0, 3) == 0
        && let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut)
    {
        p.geiger_range = 1000.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constants_are_those_of_player_h() {
        assert_eq!(FLASH_DRAIN_TIME, 1.2);
        assert_eq!(FLASH_CHARGE_TIME, 0.2);
        assert_eq!(EF_DIMLIGHT, 8);
    }
}
