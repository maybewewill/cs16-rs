//! The grenades and the C4: `CHEGrenade`, `CFlashbang` and `CSmokeGrenade` (ReGameDLL_CS
//! `wpn_shared/wpn_hegrenade.cpp`, `wpn_flashbang.cpp`, `wpn_smokegrenade.cpp`, read
//! 2026-11-22), the vanilla branch (`CLIENT_WEAPONS`, not `REGAMEDLL_FIXES`). The thrown
//! grenade itself is [`crate::grenade`]. The C4 is [`super::c4`] and the planted bomb is in
//! [`crate::grenade`] too (phase 2E).
//!
//! What the three have in common:
//!
//! - The throw is in two halves. `PrimaryAttack` pulls the pin: `m_flStartThrow` is the time,
//!   `m_flReleaseThrow` 0, the pin animation plays and the weapon is busy for half a second.
//!   The throw itself is made by `WeaponIdle`, which runs when the buttons are up: the first
//!   call after the pin was pulled records the release (`m_flReleaseThrow = time`); once the
//!   idle time is spent the grenade flies ([`throw_it`]).
//! - The throw's pitch is bent: `-10 + pitch * (80 / 90)` for a pitch above the horizon (which
//!   is negative), `-10 + pitch * (100 / 90)` below, so a grenade thrown level flies a little
//!   up. The speed is `(90 - bent pitch) * 6`, at most 750, plus the player's own velocity.
//!   The angles are `v_angle + punchangle`; the grenade starts 16 units ahead of the eyes. The
//!   fuse is 1.5 seconds.
//! - Throwing the last grenade pushes every timer back by half a second so that the throw
//!   animation can finish; the next `WeaponIdle` then retires the weapon ([`retire_weapon`]).
//! - A player with a shield may lift it with the secondary attack unless a throw is on
//!   (`ShieldSecondaryFire`); the shield blocks the pin (`PrimaryAttack` returns).
//! - Holstering a grenade of which the player has no more destroys it (`DestroyItem`: here
//!   [`WeaponCtx::remove_self`]). QUIRK: the speed, `m_flStartThrow` and `m_flReleaseThrow`
//!   are reset by the holster, so a pulled pin is lost by changing weapon.
//!
//! Where they differ: the HE grenade redraws after a throw and, idle, plays its idle animation
//! with 10 to 15 seconds in between; the flashbang does not redraw: the first idle after a
//! throw retires it, and its idle draws a random number that picks between the idle animation
//! and, over three quarters, `*(int *)&flRand` (QUIRK: the bits of the float read as an
//! animation number, `TODO: This is a bug?` in the source; with `cl_lw` the client never hears
//! of it) 2.5 seconds later; the smoke grenade waits for `m_flReleaseThrow == 0` alone (not for
//! a pulled pin as well), and its idle is the flashbang's with a sane animation. The flashbang
//! is `IsPistol()` in vanilla ("why the object flashbang is IsPistol?"). Only the HE and the
//! smoke grenade pass an event to `ThrowGrenade`.
//!
//! Not ported, and named: `Spawn`/`FallInit` (the entity half, [`crate::inventory`]);
//! `UpdateShieldCrosshair` (the shield task); `SetAnimation(PLAYER_ATTACK1)` is called, and
//! does nothing until the player animation task.

use cs_engine::EngineApi;
use cs_sim::pm::math::angle_vectors;

use super::core::{PlayerAnim, WeaponCtx, precache_model, precache_sound, with_weapon};
use super::info::{WPNSTATE_SHIELD_DRAWN, WeaponId, item_info};
use super::weapon::{Weapon, WeaponBehavior};

/// `*_IDLE`, `*_PULLPIN` (`*_PINPULL` of the smoke grenade), `*_THROW` and `*_DRAW`
/// (`flashbang_e`, `hegrenade_e`, `smokegrenade_e`: the same numbers in all three).
const IDLE: i32 = 0;
const PULLPIN: i32 = 1;
const THROW: i32 = 2;
const DRAW: i32 = 3;
/// `SHIELDGREN_*` (`weapontype.h`): "3 is last grenade viewmodel sequence".
const SHIELDGREN_IDLE: i32 = 4;
const SHIELDGREN_UP: i32 = 5;
const SHIELDGREN_DOWN: i32 = 6;
/// `*_MAX_SPEED` and `*_MAX_SPEED_SHIELD`: the same for the three.
const MAX_SPEED: f32 = 250.0;
const MAX_SPEED_SHIELD: f32 = 180.0;
/// The fuse every thrown grenade has, seconds.
const FUSE: f32 = 1.5;

/// The names of a grenade's models: `(view, weapon, shield view, shield weapon)`.
fn models(id: WeaponId) -> (&'static str, &'static str, &'static str, &'static str) {
    match id {
        WeaponId::HeGrenade => (
            "models/v_hegrenade.mdl",
            "models/p_hegrenade.mdl",
            "models/shield/v_shield_hegrenade.mdl",
            "models/shield/p_shield_hegrenade.mdl",
        ),
        WeaponId::Flashbang => (
            "models/v_flashbang.mdl",
            "models/p_flashbang.mdl",
            "models/shield/v_shield_flashbang.mdl",
            "models/shield/p_shield_flashbang.mdl",
        ),
        _ => (
            "models/v_smokegrenade.mdl",
            "models/p_smokegrenade.mdl",
            "models/shield/v_shield_smokegrenade.mdl",
            "models/shield/p_shield_smokegrenade.mdl",
        ),
    }
}

/// `CHEGrenade::Precache`, `CFlashbang::Precache`, `CSmokeGrenade::Precache`: the two view
/// models, the sounds and (HE, smoke) the event.
fn precache(eng: &mut EngineApi<'_>, id: WeaponId) {
    let (view, _, shield_view, _) = models(id);
    precache_model(eng, view);
    precache_model(eng, shield_view);
    let sounds: &[&str] = match id {
        WeaponId::HeGrenade => &[
            "weapons/hegrenade-1.wav",
            "weapons/hegrenade-2.wav",
            "weapons/he_bounce-1.wav",
            "weapons/pinpull.wav",
        ],
        WeaponId::Flashbang => &["weapons/flashbang-1.wav", "weapons/flashbang-2.wav", "weapons/pinpull.wav"],
        _ => &["weapons/pinpull.wav", "weapons/sg_explode.wav"],
    };
    for s in sounds {
        precache_sound(eng, s);
    }
    match id {
        WeaponId::HeGrenade => {
            eng.precache_event("events/createexplo.sc");
        }
        WeaponId::SmokeGrenade => {
            eng.precache_event("events/createsmoke.sc");
        }
        _ => {}
    }
}

/// `Deploy`: no throw pending, the speed, the shield down, the view model (the shield's with a
/// shield).
fn deploy(me: &dyn WeaponBehavior, w: &mut WeaponCtx<'_, '_>) -> bool {
    w.weapon.release_throw = -1.0;
    w.weapon.max_speed = MAX_SPEED;
    w.weapon.weapon_state &= !WPNSTATE_SHIELD_DRAWN;
    w.player_mut().shield_drawn = false;
    let (view, weapon, shield_view, shield_weapon) = models(w.weapon.id);
    if w.has_shield() {
        w.default_deploy(me, shield_view, shield_weapon, DRAW, "shieldgren", true)
    } else {
        w.default_deploy(me, view, weapon, DRAW, "grenade", true)
    }
}

/// `Holster`: the player may not act for half a second; a grenade the player has no more of
/// is destroyed (`DestroyItem`); a pulled pin is forgotten.
fn holster(w: &mut WeaponCtx<'_, '_>) {
    w.player_mut().next_attack = 0.5;
    if w.ammo(w.weapon.primary_ammo_type) == 0 {
        // `m_pPlayer->pev->weapons &= ~(1 << id)` (in `with_weapon`'s removal) and
        // `DestroyItem()`.
        w.remove_self = true;
    }
    w.weapon.start_throw = 0.0;
    w.weapon.release_throw = -1.0;
}

/// `PrimaryAttack`: pull the pin when none is pulled and there is a grenade. Nothing with the
/// shield up.
fn primary_attack(w: &mut WeaponCtx<'_, '_>) {
    if w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0 {
        return;
    }
    if w.weapon.start_throw == 0.0 && w.ammo(w.weapon.primary_ammo_type) > 0 {
        w.weapon.release_throw = 0.0;
        w.weapon.start_throw = w.now();
        w.send_weapon_anim(PULLPIN, true);
        w.weapon.time_weapon_idle = 0.5;
    }
}

/// `ShieldSecondaryFire(iUpAnim, iDownAnim)`: with a shield, lifts it or lowers it (not while
/// a throw is on); false without one. `UpdateShieldCrosshair` is not ported.
fn shield_secondary_fire(me: &dyn WeaponBehavior, w: &mut WeaponCtx<'_, '_>) -> bool {
    if !w.has_shield() || w.weapon.start_throw > 0.0 {
        return false;
    }
    if w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0 {
        w.weapon.weapon_state &= !WPNSTATE_SHIELD_DRAWN;
        w.send_weapon_anim(SHIELDGREN_DOWN, true);
        w.player_mut().anim_extension = "shieldgren".to_string();
        w.weapon.max_speed = MAX_SPEED;
        w.player_mut().shield_drawn = false;
    } else {
        w.weapon.weapon_state |= WPNSTATE_SHIELD_DRAWN;
        w.send_weapon_anim(SHIELDGREN_UP, true);
        w.player_mut().anim_extension = "shielded".to_string();
        w.weapon.max_speed = MAX_SPEED_SHIELD;
        w.player_mut().shield_drawn = true;
    }
    let drawn = w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0;
    super::core::update_shield_crosshair(w, !drawn);
    w.reset_max_speed(me);
    w.weapon.next_secondary_attack = 0.4;
    w.weapon.next_primary_attack = w.get_next_attack_delay(0.4);
    w.weapon.time_weapon_idle = 0.6;
    true
}

/// `SetPlayerShieldAnim`: with a shield, the animation set of the shield in hand.
fn set_player_shield_anim(w: &mut WeaponCtx<'_, '_>) {
    if !w.has_shield() {
        return;
    }
    let drawn = w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0;
    w.player_mut().anim_extension = if drawn { "shield" } else { "shieldgren" }.to_string();
}

/// `ResetPlayerShieldAnim`: a shield that is up gets the throwing set back.
fn reset_player_shield_anim(w: &mut WeaponCtx<'_, '_>) {
    if !w.has_shield() {
        return;
    }
    if w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0 {
        w.player_mut().anim_extension = "shieldgren".to_string();
    }
}

/// `CBasePlayerWeapon::RetireWeapon` (`weapons.cpp` 1745, read 2026-11-22): "no more ammo for
/// this gun, put it away". The view and weapon models go, then `GetNextBestWeapon`
/// (`multiplay_gamerules.cpp` 3319): the heaviest other weapon the player can deploy (the first
/// of equal weight, in list order) is switched to (`CBasePlayer::SwitchWeapon`): this weapon
/// is holstered (which destroys a grenade the player has no more of), the other becomes active
/// and is deployed, and the speed is reset. Nothing happens when this weapon cannot be put away
/// or there is nothing to switch to.
///
/// DEVIATION: the weapon is checked out of the player's list while it runs, so the holster is
/// called here, not by [`crate::inventory::switch_weapon`] (which would find a placeholder); the
/// order of what happens is the source's. `m_pLastItem` is the active item after the holster:
/// nothing when the holster destroyed the grenade (as the source, whose `RemovePlayerItem`
/// clears the active item).
pub(crate) fn retire_weapon(me: &mut dyn WeaponBehavior, w: &mut WeaponCtx<'_, '_>) {
    let edict = w.edict();
    if let Some(v) = w.eng.ent_mut(edict) {
        v.viewmodel = 0;
        v.weaponmodel = 0;
    }
    if !me.can_holster(w) {
        return;
    }
    let current = w.weapon.id;
    let ids: Vec<WeaponId> = w.player().items.iter().flatten().map(|i| i.weapon.id).collect();
    let mut best_weight = -1;
    let mut best = None;
    for id in ids {
        let weight = item_info(id).map_or(0, |i| i.weight);
        // Don't reselect the weapon we're trying to get rid of.
        if weight > best_weight
            && id != current
            && with_weapon(w.g, w.eng, w.slot, id, |b, ctx| b.can_deploy(ctx)).unwrap_or(false)
        {
            best_weight = weight;
            best = Some(id);
        }
    }
    let Some(best) = best else {
        return;
    };
    // `SwitchWeapon(pBest)`.
    if !with_weapon(w.g, w.eng, w.slot, best, |b, ctx| b.can_deploy(ctx)).unwrap_or(false) {
        return;
    }
    me.holster(w);
    let slot = w.slot;
    let destroyed = w.remove_self;
    let p = w.player_mut();
    p.last_item = if destroyed { None } else { Some(current) };
    p.active_item = Some(best);
    with_weapon(w.g, w.eng, slot, best, |b, ctx| b.deploy(ctx));
    crate::player::reset_max_speed(w.g, w.eng, slot);
}

/// The first half of the throw in `WeaponIdle`: the radio call, the angles, the speed and the
/// grenade (the animation and the timers are the caller's).
fn throw_it(w: &mut WeaponCtx<'_, '_>) {
    let slot = w.slot;
    // `Radio("%!MRAD_FIREINHOLE", "#Fire_in_the_hole")`.
    crate::hud_misc::radio(&*w.g, w.eng, slot, "%!MRAD_FIREINHOLE", "#Fire_in_the_hole");

    let edict = w.edict();
    let Some(v) = w.eng.ent(edict) else {
        return;
    };
    let (origin, view_ofs, velocity) = (v.origin, v.view_ofs, v.velocity);
    let mut ang_throw = v.v_angle + v.punchangle;
    // `angThrow.x = -10 + angThrow.x * ((90 - 10) / 90.0)`: a `double` expression.
    ang_throw.x = if ang_throw.x < 0.0 {
        (-10.0 + f64::from(ang_throw.x) * (f64::from(90 - 10) / 90.0)) as f32
    } else {
        (-10.0 + f64::from(ang_throw.x) * (f64::from(90 + 10) / 90.0)) as f32
    };
    let mut vel = (90.0 - ang_throw.x) * 6.0;
    if vel > 750.0 {
        vel = 750.0;
    }
    let forward = angle_vectors(ang_throw).0;
    let src = origin + view_ofs + forward * 16.0;
    let throw = forward * vel + velocity;
    let id = w.weapon.id;
    crate::grenade::throw_grenade(w.g, w.eng, slot, id, src, throw, FUSE);
}

/// `WeaponIdle` of the three; the weapon's id tells which differences apply.
fn weapon_idle(me: &mut dyn WeaponBehavior, w: &mut WeaponCtx<'_, '_>) {
    let id = w.weapon.id;
    // The HE grenade and the flashbang record the release only for a pulled pin; the smoke
    // grenade for any `m_flReleaseThrow == 0`.
    let releasing = if id == WeaponId::SmokeGrenade {
        w.weapon.release_throw == 0.0
    } else {
        w.weapon.release_throw == 0.0 && w.weapon.start_throw != 0.0
    };
    if releasing {
        w.weapon.release_throw = w.now();
    }
    if w.weapon.time_weapon_idle > 0.0 {
        return;
    }
    let ammo_type = w.weapon.primary_ammo_type;
    if w.weapon.start_throw != 0.0 {
        throw_it(w);
        w.send_weapon_anim(THROW, true);
        set_player_shield_anim(w);
        // Player "shoot" animation.
        w.set_animation(PlayerAnim::Attack1);

        w.weapon.start_throw = 0.0;
        w.weapon.next_primary_attack = w.get_next_attack_delay(0.5);
        w.weapon.time_weapon_idle = 0.75;
        w.add_ammo(ammo_type, -1);
        if w.ammo(ammo_type) <= 0 {
            // Just threw the last grenade: set the attack times in the future, and the idle
            // in the future so the whole throw animation can be seen; the idle retires the
            // weapon.
            let delay = w.get_next_attack_delay(0.5);
            w.weapon.time_weapon_idle = delay;
            w.weapon.next_secondary_attack = delay;
            w.weapon.next_primary_attack = delay;
        }
        reset_player_shield_anim(w);
    } else if w.weapon.release_throw > 0.0 {
        // We've finished the throw, restart.
        w.weapon.start_throw = 0.0;
        if id == WeaponId::Flashbang {
            retire_weapon(me, w);
            return;
        }
        if w.ammo(ammo_type) != 0 {
            w.send_weapon_anim(DRAW, true);
        } else {
            retire_weapon(me, w);
            return;
        }
        w.weapon.time_weapon_idle = w.random_float(10.0, 15.0);
        w.weapon.release_throw = -1.0;
    } else if w.ammo(ammo_type) != 0 {
        // The flashbang and the smoke grenade draw this number first, the HE grenade not at all.
        let rand = if id == WeaponId::HeGrenade { 0.0 } else { w.random_float(0.0, 1.0) };
        if w.has_shield() {
            w.weapon.time_weapon_idle = 20.0;
            if w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0 {
                w.send_weapon_anim(SHIELDGREN_IDLE, true);
            }
        } else if id == WeaponId::HeGrenade {
            w.send_weapon_anim(IDLE, true);
            // How long till we do this again.
            w.weapon.time_weapon_idle = w.random_float(10.0, 15.0);
        } else {
            let anim;
            if rand <= 0.75 {
                anim = IDLE;
                // How long till we do this again.
                w.weapon.time_weapon_idle = w.random_float(10.0, 15.0);
            } else {
                // The flashbang's `iAnim = *(int *)&flRand` (a bug of the source); the smoke
                // grenade plays its idle.
                anim = if id == WeaponId::Flashbang { rand.to_bits() as i32 } else { IDLE };
                w.weapon.time_weapon_idle = 75.0 / 30.0;
            }
            w.send_weapon_anim(anim, true);
        }
    }
}

/// Defines the behaviour of one of the three grenades over the shared functions above.
macro_rules! grenade_weapon {
    ($(#[$meta:meta])* $name:ident, $id:expr, $pistol:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Default)]
        pub struct $name;

        impl WeaponBehavior for $name {
            fn id(&self) -> WeaponId {
                $id
            }

            fn precache(&mut self, eng: &mut EngineApi<'_>) {
                precache(eng, $id);
            }

            fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
                deploy(&*self, w)
            }

            fn holster(&mut self, w: &mut WeaponCtx<'_, '_>) {
                holster(w);
            }

            fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
                primary_attack(w);
            }

            fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
                shield_secondary_fire(&*self, w);
            }

            fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
                weapon_idle(self, w);
            }

            /// `GetMaxSpeed`: `m_fMaxSpeed`, set by the deploy and the shield.
            fn max_speed(&self, w: &Weapon) -> f32 {
                w.max_speed
            }

            fn is_pistol(&self) -> bool {
                $pistol
            }

            /// `CanDeploy`: there is a grenade.
            fn can_deploy(&self, w: &WeaponCtx<'_, '_>) -> bool {
                w.ammo(w.weapon.primary_ammo_type) != 0
            }

            /// `CanDrop`: no.
            fn can_drop(&self, _w: &WeaponCtx<'_, '_>) -> bool {
                false
            }
        }
    };
}

grenade_weapon!(
    /// The HE grenade (`CHEGrenade`).
    HeGrenade, WeaponId::HeGrenade, false
);
grenade_weapon!(
    /// The flashbang (`CFlashbang`); `IsPistol()` is true in vanilla.
    Flashbang, WeaponId::Flashbang, true
);
grenade_weapon!(
    /// The smoke grenade (`CSmokeGrenade`).
    SmokeGrenade, WeaponId::SmokeGrenade, false
);
