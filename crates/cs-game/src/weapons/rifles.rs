//! The rifles and the machine gun: Galil, Famas (with its burst), AK-47, M4A1 (with its
//! silencer), AUG and SG552 (with their scopes) and the M249. Ported from ReGameDLL_CS
//! `wpn_shared/wpn_galil.cpp`, `wpn_famas.cpp`, `wpn_ak47.cpp`, `wpn_m4a1.cpp`, `wpn_aug.cpp`,
//! `wpn_sg552.cpp` and `wpn_m249.cpp`, with the constants of `weapons.h`, read 2026-11-22; the
//! vanilla build (`REGAMEDLL_API`, `CLIENT_WEAPONS`, none of `REGAMEDLL_FIXES`,
//! `REGAMEDLL_ADD`, `BUILD_LATEST`). The module notes of [`super::smgs`] (the `int` accuracy
//! divisors, the `double` spreads, what is not ported, the fire events looked up by name)
//! hold here too, and its helpers are used.
//!
//! ## Scopes
//!
//! The AUG and the SG552 zoom 90 to 55 and back with `SecondaryAttack`
//! (`pev->fov = m_iFOV = 55`), and both reload out of the scope. Vanilla does not call
//! `ResetMaxSpeed` after the zoom (that is `REGAMEDLL_FIXES`), so the SG552's slower walk
//! (`GetMaxSpeed` reads `m_iFOV`) only reaches the player the next time something resets the
//! speed. The trait's `max_speed` sees the weapon and not the player, so the SG552 keeps the
//! answer in `Weapon::max_speed` (the C `m_fMaxSpeed`, unused by this weapon), refreshed
//! whenever the weapon zooms, deploys and in `item_pre_frame` (assumption: that mirrors
//! `m_iFOV` as often as the player's code asks for the speed).

use cs_engine::EngineApi;
use cs_engine::event_flags::FEV_NOTHOST;

use super::core::{DEFAULT_FOV, PlayerAnim, TAB_556NATO, WeaponCtx, muzzle_flash, punch};
use super::info::{BulletType, WPNSTATE_FAMAS_BURST_MODE, WPNSTATE_M4A1_SILENCED, WeaponId};
use super::smgs::{
    accuracy_cubed, aim, event_index, f32_of, fire_on_empty, int_x100, kick, stance, weapon_idle,
};
use super::weapon::{Weapon, WeaponBehavior};
use crate::chat::{HUD_PRINTCENTER, client_print};

/// The scope of the AUG and the SG552.
const ZOOM_FOV: u8 = 55;

/// The punch of the Galil's and the Famas's event: `int(punchangle * 10000000)`.
fn int_x1e7(v: f32) -> i32 {
    (v * 10_000_000.0) as i32
}

/// `pev->fov = m_iFOV = fov`: the player's zoom and the edict's.
fn set_fov(w: &mut WeaponCtx<'_, '_>, fov: u8) {
    w.player_mut().fov = fov;
    let id = w.edict();
    if let Some(v) = w.eng.ent_mut(id) {
        v.fov = f32::from(fov);
    }
}

/// `m_pPlayer->m_iFOV == DEFAULT_FOV`.
fn not_zoomed(w: &WeaponCtx<'_, '_>) -> bool {
    w.player().fov == DEFAULT_FOV
}

/// The `float` `pev->fov == DEFAULT_FOV` of the AUG's and the SG552's `PrimaryAttack`.
fn edict_fov_is_default(w: &WeaponCtx<'_, '_>) -> bool {
    let id = w.edict();
    w.eng.ent(id).is_some_and(|v| v.fov == f32::from(DEFAULT_FOV))
}

// ---------------------------------------------------------------------------------------- Galil

const GALIL_MAX_SPEED: f32 = 240.0;
const GALIL_DAMAGE: f32 = 30.0;
const GALIL_RANGE_MODIFER: f32 = 0.98;
const GALIL_RELOAD_TIME: f32 = 2.45;
const GALIL_ACCURACY_DIVISOR: i32 = 200;
const GALIL_IDLE1: i32 = 0;
const GALIL_RELOAD: i32 = 1;
const GALIL_DRAW: i32 = 2;

/// The Galil (`CGalil`); it does not fire underwater (the item flag).
#[derive(Debug, Clone, Default)]
pub struct Galil;

impl WeaponBehavior for Galil {
    fn id(&self) -> WeaponId {
        WeaponId::Galil
    }

    /// `CGalil::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_galil.mdl", "models/w_galil.mdl"] {
            let _ = eng.precache_model(model);
        }
        for sound in [
            "weapons/galil-1.wav",
            "weapons/galil-2.wav",
            "weapons/galil_clipout.wav",
            "weapons/galil_clipin.wav",
            "weapons/galil_boltpull.wav",
        ] {
            let _ = eng.precache_sound(sound);
        }
        let _ = eng.precache_model("models/rshell.mdl");
        eng.precache_event("events/galil.sc");
    }

    /// `CGalil::Deploy`.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.accuracy = 0.2;
        w.weapon.shots_fired = 0;
        w.default_deploy(&*self, "models/v_galil.mdl", "models/p_galil.mdl", GALIL_DRAW, "ak47", true)
    }

    /// `CGalil::SecondaryAttack`: nothing (the table says it has none).
    fn secondary_attack(&mut self, _w: &mut WeaponCtx<'_, '_>) {}

    /// `CGalil::PrimaryAttack`.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let accuracy = f64::from(w.weapon.accuracy);
        let s = stance(w);
        if !s.on_ground {
            galil_fire(w, f32_of(0.04 + (0.3 * accuracy)), 0.0875);
        } else if s.speed_2d > 140.0 {
            galil_fire(w, f32_of(0.04 + (0.07 * accuracy)), 0.0875);
        } else {
            galil_fire(w, f32_of(0.0375 * accuracy), 0.0875);
        }
    }

    /// `CGalil::Reload`: no ammo test.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.default_reload(w.max_clip(), GALIL_RELOAD, GALIL_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.2;
            w.weapon.shots_fired = 0;
            w.weapon.delay_fire = false;
        }
    }

    /// `CGalil::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        weapon_idle(w, GALIL_IDLE1);
    }

    fn max_speed(&self, _w: &Weapon) -> f32 {
        GALIL_MAX_SPEED
    }
}

/// `CGalil::GalilFire` (`wpn_galil.cpp`, read 2026-11-22). Accuracy cubed over 200 plus 0.35, at
/// most 1.25; two penetrations; the event's punch is multiplied by 10 million (not 100); the
/// idle waits 1.28 s; the moving test of `KickBack` comes before the airborne one.
fn galil_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, cycle_time: f32) {
    w.weapon.delay_fire = true;
    w.weapon.shots_fired += 1;
    w.weapon.accuracy = accuracy_cubed(w.weapon.shots_fired, GALIL_ACCURACY_DIVISOR, 0.35, 1.25);

    if w.weapon.clip <= 0 {
        fire_on_empty(w);
        return;
    }

    w.weapon.clip -= 1;
    muzzle_flash(w);
    w.set_animation(PlayerAnim::Attack1);

    let (src, aiming) = aim(w);
    let edict = w.edict();
    let seed = w.player().random_seed;
    let dir = w.fire_bullets3(
        src, aiming, spread, 8192.0, 2, BulletType::Player556mm, GALIL_DAMAGE as i32,
        GALIL_RANGE_MODIFER, Some(edict), false, seed,
    );

    let punch = punch(w);
    let event = event_index(w, "events/galil.sc");
    w.playback_event(
        FEV_NOTHOST, event, dir.x, dir.y, int_x1e7(punch.x), int_x1e7(punch.y), false, false,
    );

    w.weapon.next_primary_attack = w.get_next_attack_delay(cycle_time);
    w.weapon.next_secondary_attack = w.weapon.next_primary_attack;

    w.weapon.time_weapon_idle = 1.28;

    let s = stance(w);
    if s.speed_2d > 0.0 {
        kick(w, (1.0, 0.45, 0.28, 0.045, 3.75, 3.0, 7));
    } else if !s.on_ground {
        kick(w, (1.2, 0.5, 0.23, 0.15, 5.5, 3.5, 6));
    } else if s.ducking {
        kick(w, (0.6, 0.3, 0.2, 0.0125, 3.25, 2.0, 7));
    } else {
        kick(w, (0.65, 0.35, 0.25, 0.015, 3.5, 2.25, 7));
    }
}

// ---------------------------------------------------------------------------------------- Famas

const FAMAS_MAX_SPEED: f32 = 240.0;
const FAMAS_RELOAD_TIME: f32 = 3.3;
const FAMAS_DAMAGE: f32 = 30.0;
const FAMAS_DAMAGE_BURST: f32 = 34.0;
const FAMAS_RANGE_MODIFER: f32 = 0.96;
const FAMAS_ACCURACY_DIVISOR: i32 = 215;
const FAMAS_IDLE1: i32 = 0;
const FAMAS_RELOAD: i32 = 1;
const FAMAS_DRAW: i32 = 2;

/// The Famas (`CFamas`): full auto, or a burst of three (`WPNSTATE_FAMAS_BURST_MODE`); the
/// first shot of a burst is fired here, the other two by `FireRemaining`
/// ([`WeaponCtx::fire_remaining`]).
#[derive(Debug, Clone, Default)]
pub struct Famas;

impl Famas {
    /// `CFamas::SecondaryAttack` (`wpn_famas.cpp`): toggles the burst mode and says so in the
    /// middle of the screen; the next secondary attack waits 0.3 s.
    fn toggle_burst(w: &mut WeaponCtx<'_, '_>) {
        let slot = w.slot;
        if w.weapon.weapon_state & WPNSTATE_FAMAS_BURST_MODE != 0 {
            client_print(w.eng, slot, HUD_PRINTCENTER, "#Switch_To_FullAuto", &[]);
            w.weapon.weapon_state &= !WPNSTATE_FAMAS_BURST_MODE;
        } else {
            client_print(w.eng, slot, HUD_PRINTCENTER, "#Switch_To_BurstFire", &[]);
            w.weapon.weapon_state |= WPNSTATE_FAMAS_BURST_MODE;
        }
        w.weapon.next_secondary_attack = 0.3;
    }
}

impl WeaponBehavior for Famas {
    fn id(&self) -> WeaponId {
        WeaponId::Famas
    }

    /// `CFamas::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_famas.mdl", "models/w_famas.mdl"] {
            let _ = eng.precache_model(model);
        }
        for sound in [
            "weapons/famas-1.wav",
            "weapons/famas-2.wav",
            "weapons/famas_clipout.wav",
            "weapons/famas_clipin.wav",
            "weapons/famas_boltpull.wav",
            "weapons/famas_boltslap.wav",
            "weapons/famas_forearm.wav",
            "weapons/famas-burst.wav",
        ] {
            let _ = eng.precache_sound(sound);
        }
        let _ = eng.precache_model("models/rshell.mdl");
        eng.precache_event("events/famas.sc");
    }

    /// `CFamas::Deploy`.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.shots_fired = 0;
        w.weapon.famas_shots_fired = 0;
        w.weapon.famas_shoot = 0.0;
        w.weapon.accuracy = 0.2;
        w.default_deploy(&*self, "models/v_famas.mdl", "models/p_famas.mdl", FAMAS_DRAW, "carbine", true)
    }

    /// `CFamas::SecondaryAttack`.
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        Self::toggle_burst(w);
    }

    /// `CFamas::PrimaryAttack`.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let burst = w.weapon.weapon_state & WPNSTATE_FAMAS_BURST_MODE == WPNSTATE_FAMAS_BURST_MODE;
        let accuracy = f64::from(w.weapon.accuracy);
        let s = stance(w);
        if !s.on_ground {
            famas_fire(w, f32_of(0.030 + 0.3 * accuracy), 0.0825, burst);
        } else if s.speed_2d > 140.0 {
            famas_fire(w, f32_of(0.030 + 0.07 * accuracy), 0.0825, burst);
        } else {
            famas_fire(w, f32_of(0.02 * accuracy), 0.0825, burst);
        }
    }

    /// `CFamas::Reload`: the legacy `ammo_556nato` counter must be above 0 (vanilla). QUIRK
    /// kept: a Famas has no scope, but a player whose FOV is not 90 (another weapon's zoom
    /// carried over) toggles the burst mode here, with its message.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.player().tabulated_ammo[TAB_556NATO] <= 0 {
            return;
        }
        if w.default_reload(w.max_clip(), FAMAS_RELOAD, FAMAS_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            if !not_zoomed(w) {
                Self::toggle_burst(w);
            }
            w.weapon.accuracy = 0.0;
            w.weapon.shots_fired = 0;
            w.weapon.delay_fire = false;
        }
    }

    /// `CFamas::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        weapon_idle(w, FAMAS_IDLE1);
    }

    fn max_speed(&self, _w: &Weapon) -> f32 {
        FAMAS_MAX_SPEED
    }
}

/// `CFamas::FamasFire` (`wpn_famas.cpp`, read 2026-11-22). A burst resets its count and has a
/// cycle time of 0.55 s; full auto adds 0.01 to the spread. Accuracy cubed over 215 plus 0.3,
/// at most 1. The first shot of a burst does 34 damage (the later ones, `FireRemaining`, 30),
/// and ends by arming the next shot 0.05 s later (absolute server time) with the spread it used.
fn famas_fire(w: &mut WeaponCtx<'_, '_>, mut spread: f32, mut cycle_time: f32, burst: bool) {
    if burst {
        w.weapon.famas_shots_fired = 0;
        cycle_time = 0.55;
    } else {
        spread += 0.01;
    }

    w.weapon.delay_fire = true;
    w.weapon.shots_fired += 1;
    w.weapon.accuracy = accuracy_cubed(w.weapon.shots_fired, FAMAS_ACCURACY_DIVISOR, 0.3, 1.0);

    if w.weapon.clip <= 0 {
        fire_on_empty(w);
        return;
    }

    w.weapon.clip -= 1;
    muzzle_flash(w);
    w.set_animation(PlayerAnim::Attack1);

    let (src, aiming) = aim(w);
    let base_damage = if burst { FAMAS_DAMAGE_BURST } else { FAMAS_DAMAGE };
    let edict = w.edict();
    let seed = w.player().random_seed;
    let dir = w.fire_bullets3(
        src, aiming, spread, 8192.0, 2, BulletType::Player556mm, base_damage as i32,
        FAMAS_RANGE_MODIFER, Some(edict), false, seed,
    );

    let punch = punch(w);
    let event = event_index(w, "events/famas.sc");
    w.playback_event(
        FEV_NOTHOST, event, dir.x, dir.y, int_x1e7(punch.x), int_x1e7(punch.y), false, false,
    );

    w.weapon.next_primary_attack = w.get_next_attack_delay(cycle_time);
    w.weapon.next_secondary_attack = w.weapon.next_primary_attack;

    w.weapon.time_weapon_idle = 1.1;

    let s = stance(w);
    if s.speed_2d > 0.0 {
        kick(w, (1.0, 0.45, 0.275, 0.05, 4.0, 2.5, 7));
    } else if !s.on_ground {
        kick(w, (1.25, 0.45, 0.22, 0.18, 5.5, 4.0, 5));
    } else if s.ducking {
        kick(w, (0.575, 0.325, 0.2, 0.011, 3.25, 2.0, 8));
    } else {
        kick(w, (0.625, 0.375, 0.25, 0.0125, 3.5, 2.25, 8));
    }

    if burst {
        w.weapon.famas_shots_fired += 1;
        w.weapon.burst_spread = spread;
        w.weapon.famas_shoot = w.now() + 0.05;
    }
}

// ----------------------------------------------------------------------------------------- AK-47

const AK47_MAX_SPEED: f32 = 221.0;
const AK47_DAMAGE: f32 = 36.0;
const AK47_RANGE_MODIFER: f32 = 0.98;
const AK47_RELOAD_TIME: f32 = 2.45;
const AK47_ACCURACY_DIVISOR: i32 = 200;
const AK47_IDLE1: i32 = 0;
const AK47_RELOAD: i32 = 1;
const AK47_DRAW: i32 = 2;

/// The AK-47 (`CAK47`).
#[derive(Debug, Clone, Default)]
pub struct Ak47;

impl WeaponBehavior for Ak47 {
    fn id(&self) -> WeaponId {
        WeaponId::Ak47
    }

    /// `CAK47::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_ak47.mdl", "models/w_ak47.mdl"] {
            let _ = eng.precache_model(model);
        }
        for sound in [
            "weapons/ak47-1.wav",
            "weapons/ak47-2.wav",
            "weapons/ak47_clipout.wav",
            "weapons/ak47_clipin.wav",
            "weapons/ak47_boltpull.wav",
        ] {
            let _ = eng.precache_sound(sound);
        }
        let _ = eng.precache_model("models/rshell.mdl");
        eng.precache_event("events/ak47.sc");
    }

    /// `CAK47::Deploy`.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.accuracy = 0.2;
        w.weapon.shots_fired = 0;
        w.default_deploy(&*self, "models/v_ak47.mdl", "models/p_ak47.mdl", AK47_DRAW, "ak47", true)
    }

    /// `CAK47::SecondaryAttack`: nothing.
    fn secondary_attack(&mut self, _w: &mut WeaponCtx<'_, '_>) {}

    /// `CAK47::PrimaryAttack`.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let accuracy = f64::from(w.weapon.accuracy);
        let s = stance(w);
        if !s.on_ground {
            ak47_fire(w, f32_of(0.04 + (0.4 * accuracy)), 0.0955);
        } else if s.speed_2d > 140.0 {
            ak47_fire(w, f32_of(0.04 + (0.07 * accuracy)), 0.0955);
        } else {
            ak47_fire(w, f32_of(0.0275 * accuracy), 0.0955);
        }
    }

    /// `CAK47::Reload`: no ammo test.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.default_reload(w.max_clip(), AK47_RELOAD, AK47_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.2;
            w.weapon.shots_fired = 0;
            w.weapon.delay_fire = false;
        }
    }

    /// `CAK47::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        weapon_idle(w, AK47_IDLE1);
    }

    fn max_speed(&self, _w: &Weapon) -> f32 {
        AK47_MAX_SPEED
    }
}

/// `CAK47::AK47Fire` (`wpn_ak47.cpp`, read 2026-11-22). Accuracy cubed over 200 plus 0.35, at
/// most 1.25; two penetrations; the idle waits 1.9 s.
fn ak47_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, cycle_time: f32) {
    w.weapon.delay_fire = true;
    w.weapon.shots_fired += 1;
    w.weapon.accuracy = accuracy_cubed(w.weapon.shots_fired, AK47_ACCURACY_DIVISOR, 0.35, 1.25);

    if w.weapon.clip <= 0 {
        fire_on_empty(w);
        return;
    }

    w.weapon.clip -= 1;
    muzzle_flash(w);
    w.set_animation(PlayerAnim::Attack1);

    let (src, aiming) = aim(w);
    let edict = w.edict();
    let seed = w.player().random_seed;
    let dir = w.fire_bullets3(
        src, aiming, spread, 8192.0, 2, BulletType::Player762mm, AK47_DAMAGE as i32,
        AK47_RANGE_MODIFER, Some(edict), false, seed,
    );

    let punch = punch(w);
    let event = event_index(w, "events/ak47.sc");
    w.playback_event(
        FEV_NOTHOST, event, dir.x, dir.y, int_x100(punch.x), int_x100(punch.y), false, false,
    );

    w.weapon.next_primary_attack = w.get_next_attack_delay(cycle_time);
    w.weapon.next_secondary_attack = w.weapon.next_primary_attack;

    w.weapon.time_weapon_idle = 1.9;

    let s = stance(w);
    if s.speed_2d > 0.0 {
        kick(w, (1.5, 0.45, 0.225, 0.05, 6.5, 2.5, 7));
    } else if !s.on_ground {
        kick(w, (2.0, 1.0, 0.5, 0.35, 9.0, 6.0, 5));
    } else if s.ducking {
        kick(w, (0.9, 0.35, 0.15, 0.025, 5.5, 1.5, 9));
    } else {
        kick(w, (1.0, 0.375, 0.175, 0.0375, 5.75, 1.75, 8));
    }
}

// ----------------------------------------------------------------------------------------- M4A1

const M4A1_MAX_SPEED: f32 = 230.0;
const M4A1_DAMAGE: f32 = 32.0;
const M4A1_DAMAGE_SIL: f32 = 33.0;
const M4A1_RANGE_MODIFER: f32 = 0.97;
const M4A1_RANGE_MODIFER_SIL: f32 = 0.95;
const M4A1_RELOAD_TIME: f32 = 3.05;
const M4A1_ACCURACY_DIVISOR: i32 = 220;
const M4A1_IDLE: i32 = 0;
const M4A1_RELOAD: i32 = 4;
const M4A1_DRAW: i32 = 5;
const M4A1_ATTACH_SILENCER: i32 = 6;
const M4A1_UNSIL_IDLE: i32 = 7;
const M4A1_UNSIL_RELOAD: i32 = 11;
const M4A1_UNSIL_DRAW: i32 = 12;
const M4A1_DETACH_SILENCER: i32 = 13;

/// The M4A1 (`CM4A1`): `WPNSTATE_M4A1_SILENCED` picks the animations, the damage and the range.
#[derive(Debug, Clone, Default)]
pub struct M4a1;

fn m4a1_silenced(w: &Weapon) -> bool {
    w.weapon_state & WPNSTATE_M4A1_SILENCED == WPNSTATE_M4A1_SILENCED
}

impl WeaponBehavior for M4a1 {
    fn id(&self) -> WeaponId {
        WeaponId::M4a1
    }

    /// `CM4A1::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_m4a1.mdl", "models/w_m4a1.mdl"] {
            let _ = eng.precache_model(model);
        }
        for sound in [
            "weapons/m4a1-1.wav",
            "weapons/m4a1_unsil-1.wav",
            "weapons/m4a1_unsil-2.wav",
            "weapons/m4a1_clipin.wav",
            "weapons/m4a1_clipout.wav",
            "weapons/m4a1_boltpull.wav",
            "weapons/m4a1_deploy.wav",
            "weapons/m4a1_silencer_on.wav",
            "weapons/m4a1_silencer_off.wav",
        ] {
            let _ = eng.precache_sound(sound);
        }
        let _ = eng.precache_model("models/rshell.mdl");
        eng.precache_event("events/m4a1.sc");
    }

    /// `CM4A1::Deploy`: the draw animation of the state the silencer is in.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.delay_fire = true;
        w.weapon.accuracy = 0.2;
        w.weapon.shots_fired = 0;
        let anim = if m4a1_silenced(w.weapon) { M4A1_DRAW } else { M4A1_UNSIL_DRAW };
        w.default_deploy(&*self, "models/v_m4a1.mdl", "models/p_m4a1.mdl", anim, "rifle", true)
    }

    /// `CM4A1::SecondaryAttack`: puts the silencer on or takes it off (2 s in which nothing
    /// else happens: the idle, the secondary and the primary attack all wait).
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if m4a1_silenced(w.weapon) {
            w.weapon.weapon_state &= !WPNSTATE_M4A1_SILENCED;
            w.send_weapon_anim(M4A1_DETACH_SILENCER, true);
            w.player_mut().anim_extension = "rifle".to_string();
        } else {
            w.weapon.weapon_state |= WPNSTATE_M4A1_SILENCED;
            w.send_weapon_anim(M4A1_ATTACH_SILENCER, true);
            w.player_mut().anim_extension = "rifle".to_string();
        }
        w.weapon.next_secondary_attack = 2.0;
        w.weapon.time_weapon_idle = w.weapon.next_secondary_attack;
        w.weapon.next_primary_attack = w.get_next_attack_delay(2.0);
    }

    /// `CM4A1::PrimaryAttack`: the two branches differ only in the standing spread (0.025
    /// silenced, 0.02 not).
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let accuracy = f64::from(w.weapon.accuracy);
        let silenced = m4a1_silenced(w.weapon);
        let s = stance(w);
        if !s.on_ground {
            m4a1_fire(w, f32_of(0.035 + (0.4 * accuracy)), 0.0875);
        } else if s.speed_2d > 140.0 {
            m4a1_fire(w, f32_of(0.035 + (0.07 * accuracy)), 0.0875);
        } else if silenced {
            m4a1_fire(w, f32_of(0.025 * accuracy), 0.0875);
        } else {
            m4a1_fire(w, f32_of(0.02 * accuracy), 0.0875);
        }
    }

    /// `CM4A1::Reload`: the legacy `ammo_556nato` counter must be above 0 (vanilla).
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.player().tabulated_ammo[TAB_556NATO] <= 0 {
            return;
        }
        let anim = if m4a1_silenced(w.weapon) { M4A1_RELOAD } else { M4A1_UNSIL_RELOAD };
        if w.default_reload(w.max_clip(), anim, M4A1_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.2;
            w.weapon.shots_fired = 0;
            w.weapon.delay_fire = false;
        }
    }

    /// `CM4A1::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let anim = if m4a1_silenced(w.weapon) { M4A1_IDLE } else { M4A1_UNSIL_IDLE };
        weapon_idle(w, anim);
    }

    /// `CM4A1::GetMaxSpeed`.
    fn max_speed(&self, _w: &Weapon) -> f32 {
        M4A1_MAX_SPEED
    }
}

/// `CM4A1::M4A1Fire` (`wpn_m4a1.cpp`, read 2026-11-22). Accuracy cubed over 220 plus 0.3, at
/// most 1. Silenced: 33 damage, range 0.95 and no muzzle flash; unsilenced: 32, 0.97 and the
/// flash (set after the shot). QUIRKS kept: the legacy `ammo_556nato` counter goes down by one
/// per shot (vanilla; the real ammo is the clip), and the event's first bool says "silenced".
fn m4a1_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, cycle_time: f32) {
    w.weapon.delay_fire = true;
    w.weapon.shots_fired += 1;
    w.weapon.accuracy = accuracy_cubed(w.weapon.shots_fired, M4A1_ACCURACY_DIVISOR, 0.3, 1.0);

    if w.weapon.clip <= 0 {
        fire_on_empty(w);
        return;
    }

    w.weapon.clip -= 1;
    w.set_animation(PlayerAnim::Attack1);

    let (src, aiming) = aim(w);
    let silenced = m4a1_silenced(w.weapon);
    let base_damage = if silenced { M4A1_DAMAGE_SIL } else { M4A1_DAMAGE };
    let edict = w.edict();
    let seed = w.player().random_seed;
    let dir;
    if silenced {
        dir = w.fire_bullets3(
            src, aiming, spread, 8192.0, 2, BulletType::Player556mm, base_damage as i32,
            M4A1_RANGE_MODIFER_SIL, Some(edict), false, seed,
        );
    } else {
        dir = w.fire_bullets3(
            src, aiming, spread, 8192.0, 2, BulletType::Player556mm, base_damage as i32,
            M4A1_RANGE_MODIFER, Some(edict), false, seed,
        );
        muzzle_flash(w);
    }

    w.player_mut().tabulated_ammo[TAB_556NATO] -= 1;

    let punch = punch(w);
    let event = event_index(w, "events/m4a1.sc");
    w.playback_event(
        FEV_NOTHOST, event, dir.x, dir.y, int_x100(punch.x), int_x100(punch.y), silenced, false,
    );

    w.weapon.next_primary_attack = w.get_next_attack_delay(cycle_time);
    w.weapon.next_secondary_attack = w.weapon.next_primary_attack;

    w.weapon.time_weapon_idle = 1.5;

    let s = stance(w);
    if s.speed_2d > 0.0 {
        kick(w, (1.0, 0.45, 0.28, 0.045, 3.75, 3.0, 7));
    } else if !s.on_ground {
        kick(w, (1.2, 0.5, 0.23, 0.15, 5.5, 3.5, 6));
    } else if s.ducking {
        kick(w, (0.6, 0.3, 0.2, 0.0125, 3.25, 2.0, 7));
    } else {
        kick(w, (0.65, 0.35, 0.25, 0.015, 3.5, 2.25, 7));
    }
}

// ------------------------------------------------------------------------------------------- AUG

const AUG_MAX_SPEED: f32 = 240.0;
const AUG_DAMAGE: f32 = 32.0;
const AUG_RANGE_MODIFER: f32 = 0.96;
const AUG_RELOAD_TIME: f32 = 3.3;
const AUG_ACCURACY_DIVISOR: i32 = 215;
const AUG_IDLE1: i32 = 0;
const AUG_RELOAD: i32 = 1;
const AUG_DRAW: i32 = 2;

/// The AUG (`CAUG`), with its 55 degree scope.
#[derive(Debug, Clone, Default)]
pub struct Aug;

impl WeaponBehavior for Aug {
    fn id(&self) -> WeaponId {
        WeaponId::Aug
    }

    /// `CAUG::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_aug.mdl", "models/w_aug.mdl"] {
            let _ = eng.precache_model(model);
        }
        for sound in [
            "weapons/aug-1.wav",
            "weapons/aug_clipout.wav",
            "weapons/aug_clipin.wav",
            "weapons/aug_boltpull.wav",
            "weapons/aug_boltslap.wav",
            "weapons/aug_forearm.wav",
        ] {
            let _ = eng.precache_sound(sound);
        }
        let _ = eng.precache_model("models/rshell.mdl");
        eng.precache_event("events/aug.sc");
    }

    /// `CAUG::Deploy`.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.accuracy = 0.2;
        w.weapon.shots_fired = 0;
        w.default_deploy(&*self, "models/v_aug.mdl", "models/p_aug.mdl", AUG_DRAW, "carbine", true)
    }

    /// `CAUG::SecondaryAttack`: the scope, 90 to 55 and back, and 0.3 s before the next
    /// toggle. Vanilla: no `ResetMaxSpeed` (the AUG's speed does not depend on the zoom).
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if not_zoomed(w) {
            set_fov(w, ZOOM_FOV);
        } else {
            set_fov(w, DEFAULT_FOV);
        }
        w.weapon.next_secondary_attack = 0.3;
    }

    /// `CAUG::PrimaryAttack`: the cycle time is 0.0825 s unscoped and 0.135 s scoped (the test
    /// reads the edict's `fov`, a float).
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let cycle_time = if edict_fov_is_default(w) { 0.0825 } else { 0.135 };
        let accuracy = f64::from(w.weapon.accuracy);
        let s = stance(w);
        if !s.on_ground {
            aug_fire(w, f32_of(0.035 + (0.4 * accuracy)), cycle_time);
        } else if s.speed_2d > 140.0 {
            aug_fire(w, f32_of(0.035 + (0.07 * accuracy)), cycle_time);
        } else {
            aug_fire(w, f32_of(0.02 * accuracy), cycle_time);
        }
    }

    /// `CAUG::Reload`: the legacy `ammo_556nato` counter must be above 0 (vanilla); a scoped
    /// reload leaves the scope (through `SecondaryAttack`, after the player animation).
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.player().tabulated_ammo[TAB_556NATO] <= 0 {
            return;
        }
        if w.default_reload(w.max_clip(), AUG_RELOAD, AUG_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            if !not_zoomed(w) {
                self.secondary_attack(w);
            }
            w.weapon.accuracy = 0.0;
            w.weapon.shots_fired = 0;
            w.weapon.delay_fire = false;
        }
    }

    /// `CAUG::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        weapon_idle(w, AUG_IDLE1);
    }

    fn max_speed(&self, _w: &Weapon) -> f32 {
        AUG_MAX_SPEED
    }
}

/// `CAUG::AUGFire` (`wpn_aug.cpp`, read 2026-11-22). Accuracy cubed over 215 plus 0.3, at most
/// 1; the idle waits 1.9 s; the kick is the Famas's.
fn aug_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, cycle_time: f32) {
    w.weapon.delay_fire = true;
    w.weapon.shots_fired += 1;
    w.weapon.accuracy = accuracy_cubed(w.weapon.shots_fired, AUG_ACCURACY_DIVISOR, 0.3, 1.0);

    if w.weapon.clip <= 0 {
        fire_on_empty(w);
        return;
    }

    w.weapon.clip -= 1;
    muzzle_flash(w);
    w.set_animation(PlayerAnim::Attack1);

    let (src, aiming) = aim(w);
    let edict = w.edict();
    let seed = w.player().random_seed;
    let dir = w.fire_bullets3(
        src, aiming, spread, 8192.0, 2, BulletType::Player556mm, AUG_DAMAGE as i32,
        AUG_RANGE_MODIFER, Some(edict), false, seed,
    );

    let punch = punch(w);
    let event = event_index(w, "events/aug.sc");
    w.playback_event(
        FEV_NOTHOST, event, dir.x, dir.y, int_x100(punch.x), int_x100(punch.y), false, false,
    );

    w.weapon.next_primary_attack = w.get_next_attack_delay(cycle_time);
    w.weapon.next_secondary_attack = w.weapon.next_primary_attack;

    w.weapon.time_weapon_idle = 1.9;

    let s = stance(w);
    if s.speed_2d > 0.0 {
        kick(w, (1.0, 0.45, 0.275, 0.05, 4.0, 2.5, 7));
    } else if !s.on_ground {
        kick(w, (1.25, 0.45, 0.22, 0.18, 5.5, 4.0, 5));
    } else if s.ducking {
        kick(w, (0.575, 0.325, 0.2, 0.011, 3.25, 2.0, 8));
    } else {
        kick(w, (0.625, 0.375, 0.25, 0.0125, 3.5, 2.25, 8));
    }
}

// ------------------------------------------------------------------------------------------ SG552

const SG552_MAX_SPEED: f32 = 235.0;
const SG552_MAX_SPEED_ZOOM: f32 = 200.0;
const SG552_DAMAGE: f32 = 33.0;
const SG552_RANGE_MODIFER: f32 = 0.955;
const SG552_RELOAD_TIME: f32 = 3.0;
const SG552_ACCURACY_DIVISOR: i32 = 220;
const SG552_IDLE1: i32 = 0;
const SG552_RELOAD: i32 = 1;
const SG552_DRAW: i32 = 2;

/// The SG552 (`CSG552`), with its 55 degree scope and a slower walk while scoped.
#[derive(Debug, Clone, Default)]
pub struct Sg552;

/// Refreshes the SG552's answer to `GetMaxSpeed` from `m_iFOV` (see the module notes).
fn sg552_sync_speed(w: &mut WeaponCtx<'_, '_>) {
    w.weapon.max_speed = if not_zoomed(w) { SG552_MAX_SPEED } else { SG552_MAX_SPEED_ZOOM };
}

impl WeaponBehavior for Sg552 {
    fn id(&self) -> WeaponId {
        WeaponId::Sg552
    }

    /// `CSG552::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_sg552.mdl", "models/w_sg552.mdl"] {
            let _ = eng.precache_model(model);
        }
        for sound in [
            "weapons/sg552-1.wav",
            "weapons/sg552-2.wav",
            "weapons/sg552_clipout.wav",
            "weapons/sg552_clipin.wav",
            "weapons/sg552_boltpull.wav",
        ] {
            let _ = eng.precache_sound(sound);
        }
        let _ = eng.precache_model("models/rshell.mdl");
        eng.precache_event("events/sg552.sc");
    }

    /// `CSG552::Deploy`. `DefaultDeploy` puts the FOV back to 90, so the speed is the unscoped
    /// one.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.accuracy = 0.2;
        w.weapon.shots_fired = 0;
        let deployed =
            w.default_deploy(&*self, "models/v_sg552.mdl", "models/p_sg552.mdl", SG552_DRAW, "mp5", true);
        sg552_sync_speed(w);
        deployed
    }

    /// `CSG552::SecondaryAttack`: the scope; vanilla does not reset the speed here.
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if not_zoomed(w) {
            set_fov(w, ZOOM_FOV);
        } else {
            set_fov(w, DEFAULT_FOV);
        }
        sg552_sync_speed(w);
        w.weapon.next_secondary_attack = 0.3;
    }

    /// `CSG552::PrimaryAttack`: 0.0825 s unscoped, 0.135 s scoped.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let cycle_time = if edict_fov_is_default(w) { 0.0825 } else { 0.135 };
        let accuracy = f64::from(w.weapon.accuracy);
        let s = stance(w);
        if !s.on_ground {
            sg552_fire(w, f32_of(0.035 + (0.45 * accuracy)), cycle_time);
        } else if s.speed_2d > 140.0 {
            sg552_fire(w, f32_of(0.035 + (0.075 * accuracy)), cycle_time);
        } else {
            sg552_fire(w, f32_of(0.02 * accuracy), cycle_time);
        }
    }

    /// `CSG552::Reload`: the legacy `ammo_556nato` counter must be above 0 (vanilla). QUIRK:
    /// unlike the AUG, a scoped reload leaves the scope before the player animation.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.player().tabulated_ammo[TAB_556NATO] <= 0 {
            return;
        }
        if w.default_reload(w.max_clip(), SG552_RELOAD, SG552_RELOAD_TIME) {
            if !not_zoomed(w) {
                self.secondary_attack(w);
            }
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.2;
            w.weapon.shots_fired = 0;
            w.weapon.delay_fire = false;
        }
    }

    /// `CSG552::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        weapon_idle(w, SG552_IDLE1);
    }

    /// Keeps the mirror of `m_iFOV` current (see the module notes).
    fn item_pre_frame(&mut self, w: &mut WeaponCtx<'_, '_>) {
        sg552_sync_speed(w);
    }

    /// `CSG552::GetMaxSpeed`: 235, or 200 while `m_iFOV` is not 90 (the mirror in
    /// `Weapon::max_speed`; a fresh weapon, whose mirror is 0, is not scoped).
    fn max_speed(&self, w: &Weapon) -> f32 {
        if w.max_speed == SG552_MAX_SPEED_ZOOM { SG552_MAX_SPEED_ZOOM } else { SG552_MAX_SPEED }
    }
}

/// `CSG552::SG552Fire` (`wpn_sg552.cpp`, read 2026-11-22). Accuracy cubed over 220 plus 0.3, at
/// most 1; the idle waits 2 s; the event's first bool is the int 5 (`TRUE`).
fn sg552_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, cycle_time: f32) {
    w.weapon.delay_fire = true;
    w.weapon.shots_fired += 1;
    w.weapon.accuracy = accuracy_cubed(w.weapon.shots_fired, SG552_ACCURACY_DIVISOR, 0.3, 1.0);

    if w.weapon.clip <= 0 {
        fire_on_empty(w);
        return;
    }

    w.weapon.clip -= 1;
    muzzle_flash(w);
    w.set_animation(PlayerAnim::Attack1);

    let (src, aiming) = aim(w);
    let edict = w.edict();
    let seed = w.player().random_seed;
    let dir = w.fire_bullets3(
        src, aiming, spread, 8192.0, 2, BulletType::Player556mm, SG552_DAMAGE as i32,
        SG552_RANGE_MODIFER, Some(edict), false, seed,
    );

    let punch = punch(w);
    let event = event_index(w, "events/sg552.sc");
    w.playback_event(
        FEV_NOTHOST, event, dir.x, dir.y, int_x100(punch.x), int_x100(punch.y), true, false,
    );

    w.weapon.next_primary_attack = w.get_next_attack_delay(cycle_time);
    w.weapon.next_secondary_attack = w.weapon.next_primary_attack;

    w.weapon.time_weapon_idle = 2.0;

    let s = stance(w);
    if s.speed_2d > 0.0 {
        kick(w, (1.0, 0.45, 0.28, 0.04, 4.25, 2.5, 7));
    } else if !s.on_ground {
        kick(w, (1.25, 0.45, 0.22, 0.18, 6.0, 4.0, 5));
    } else if s.ducking {
        kick(w, (0.6, 0.35, 0.2, 0.0125, 3.7, 2.0, 10));
    } else {
        kick(w, (0.625, 0.375, 0.25, 0.0125, 4.0, 2.25, 9));
    }
}

// ------------------------------------------------------------------------------------------ M249

const M249_MAX_SPEED: f32 = 220.0;
const M249_DAMAGE: f32 = 32.0;
const M249_RANGE_MODIFER: f32 = 0.97;
const M249_RELOAD_TIME: f32 = 4.7;
const M249_ACCURACY_DIVISOR: i32 = 175;
const M249_IDLE1: i32 = 0;
const M249_RELOAD: i32 = 3;
const M249_DRAW: i32 = 4;

/// The M249 (`CM249`). Its animations are in another order (idle 0, shoot 1 and 2, reload 3,
/// draw 4).
#[derive(Debug, Clone, Default)]
pub struct M249;

impl WeaponBehavior for M249 {
    fn id(&self) -> WeaponId {
        WeaponId::M249
    }

    /// `CM249::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_m249.mdl", "models/w_m249.mdl"] {
            let _ = eng.precache_model(model);
        }
        for sound in [
            "weapons/m249-1.wav",
            "weapons/m249-2.wav",
            "weapons/m249_boxout.wav",
            "weapons/m249_boxin.wav",
            "weapons/m249_chain.wav",
            "weapons/m249_coverup.wav",
            "weapons/m249_coverdown.wav",
        ] {
            let _ = eng.precache_sound(sound);
        }
        let _ = eng.precache_model("models/rshell.mdl");
        eng.precache_event("events/m249.sc");
    }

    /// `CM249::Deploy`.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.accuracy = 0.2;
        w.weapon.shots_fired = 0;
        w.default_deploy(&*self, "models/v_m249.mdl", "models/p_m249.mdl", M249_DRAW, "m249", true)
    }

    /// `CM249::PrimaryAttack`.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let accuracy = f64::from(w.weapon.accuracy);
        let s = stance(w);
        if !s.on_ground {
            m249_fire(w, f32_of(0.045 + (0.5 * accuracy)), 0.1);
        } else if s.speed_2d > 140.0 {
            m249_fire(w, f32_of(0.045 + (0.095 * accuracy)), 0.1);
        } else {
            m249_fire(w, f32_of(0.03 * accuracy), 0.1);
        }
    }

    /// `CM249::Reload`: no ammo test; the box takes 4.7 s.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.default_reload(w.max_clip(), M249_RELOAD, M249_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.2;
            w.weapon.delay_fire = false;
            w.weapon.shots_fired = 0;
        }
    }

    /// `CM249::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        weapon_idle(w, M249_IDLE1);
    }

    fn max_speed(&self, _w: &Weapon) -> f32 {
        M249_MAX_SPEED
    }
}

/// `CM249::M249Fire` (`wpn_m249.cpp`, read 2026-11-22). Accuracy cubed over 175 plus 0.4, at
/// most 0.9; the idle waits 1.6 s; the airborne case of `KickBack` comes before the moving one.
fn m249_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, cycle_time: f32) {
    w.weapon.delay_fire = true;
    w.weapon.shots_fired += 1;
    w.weapon.accuracy = accuracy_cubed(w.weapon.shots_fired, M249_ACCURACY_DIVISOR, 0.4, 0.9);

    if w.weapon.clip <= 0 {
        fire_on_empty(w);
        return;
    }

    w.weapon.clip -= 1;
    muzzle_flash(w);
    w.set_animation(PlayerAnim::Attack1);

    let (src, aiming) = aim(w);
    let edict = w.edict();
    let seed = w.player().random_seed;
    let dir = w.fire_bullets3(
        src, aiming, spread, 8192.0, 2, BulletType::Player556mm, M249_DAMAGE as i32,
        M249_RANGE_MODIFER, Some(edict), false, seed,
    );

    let punch = punch(w);
    let event = event_index(w, "events/m249.sc");
    w.playback_event(
        FEV_NOTHOST, event, dir.x, dir.y, int_x100(punch.x), int_x100(punch.y), false, false,
    );

    w.weapon.next_primary_attack = w.get_next_attack_delay(cycle_time);
    w.weapon.next_secondary_attack = w.weapon.next_primary_attack;

    w.weapon.time_weapon_idle = 1.6;

    let s = stance(w);
    if !s.on_ground {
        kick(w, (1.8, 0.65, 0.45, 0.125, 5.0, 3.5, 8));
    } else if s.speed_2d > 0.0 {
        kick(w, (1.1, 0.5, 0.3, 0.06, 4.0, 3.0, 8));
    } else if s.ducking {
        kick(w, (0.75, 0.325, 0.25, 0.025, 3.5, 2.5, 9));
    } else {
        kick(w, (0.8, 0.35, 0.3, 0.03, 3.75, 3.0, 9));
    }
}
