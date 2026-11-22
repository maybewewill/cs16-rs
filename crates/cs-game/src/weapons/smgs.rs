//! The submachine guns: MAC-10, TMP, MP5 Navy, UMP45 and P90. Ported from ReGameDLL_CS
//! `wpn_shared/wpn_mac10.cpp`, `wpn_tmp.cpp`, `wpn_mp5navy.cpp`, `wpn_ump45.cpp` and
//! `wpn_p90.cpp`, with the constants of `weapons.h`, read 2026-11-22; the vanilla build
//! (`REGAMEDLL_API`, `CLIENT_WEAPONS`, none of `REGAMEDLL_FIXES`, `REGAMEDLL_ADD`,
//! `BUILD_LATEST`).
//!
//! ## What is kept, and what is not ported
//!
//! - The accuracy divisors are `int` in vanilla, so `shots * shots * shots / DIVISOR` truncates
//!   before the float is added; the MP5 Navy's is a `double` and its sum is computed in `f64`.
//! - A spread such as `0.375 * m_flAccuracy` has a `double` literal, so it is computed in `f64`
//!   and cut to the `float` parameter of the fire function.
//! - The reload of every SMG but the TMP first tests the legacy counter (`ammo_45acp`,
//!   `ammo_9mm`, `ammo_57mm`, [`Player::tabulated_ammo`]); only `REGAMEDLL_FIXES` drops it.
//! - The fire events are looked up by name when the gun fires ([`event_index`]): `new_weapon`
//!   makes unit structs and `precache_all` drops the behaviour it precaches, so a weapon has
//!   nowhere to keep the index the C class keeps in `m_usFireMAC10` and the others.
//! - Not ported, and named: `SetSuitUpdate("!HEV_AMO0")` (the HEV suit voice), `TheBots`
//!   events, `GetAutoaimVector` (no weapon of CS uses its result), `m_iWeaponVolume` and
//!   `m_iWeaponFlash` (the player has no such fields; nothing hears or draws them yet), and
//!   `iShellOn` (read by the client's brass code only).
//!
//! The helpers shared with [`super::rifles`] are `pub(super)` here.

use cs_engine::consts::{FL_DUCKING, FL_ONGROUND};
use cs_engine::event_flags::FEV_NOTHOST;
use cs_engine::EngineApi;
use cs_sim::pm::math::angle_vectors;
use glam::Vec3;

use super::core::{PlayerAnim, TAB_9MM, WeaponCtx};
use super::info::{BulletType, WeaponId};
use super::weapon::WeaponBehavior;

/// `EF_MUZZLEFLASH`.
const EF_MUZZLEFLASH: u32 = 2;
/// The index of `ammo_45acp` in [`crate::player::Player::tabulated_ammo`] (`TabulateAmmo`).
const TAB_45ACP: usize = 5;
/// The index of `ammo_57mm`.
const TAB_57MM: usize = 8;

/// What a gun's movement tests read off the player: `pev->flags & FL_ONGROUND`,
/// `pev->flags & FL_DUCKING` and `pev->velocity.Length2D()`.
pub(super) struct Stance {
    pub on_ground: bool,
    pub ducking: bool,
    pub speed_2d: f32,
}

/// The player's [`Stance`] now.
pub(super) fn stance(w: &WeaponCtx<'_, '_>) -> Stance {
    let id = w.edict();
    w.eng.ent(id).map_or(Stance { on_ground: false, ducking: false, speed_2d: 0.0 }, |v| Stance {
        on_ground: v.flags & FL_ONGROUND != 0,
        ducking: v.flags & FL_DUCKING != 0,
        speed_2d: v.velocity.truncate().length(),
    })
}

/// The id of the fire event `name` (`events/mac10.sc`), the `m_usFireMAC10` of the C class.
/// `precache_event` hands back the index a name already has, so this is a lookup once the
/// world has precached it.
pub(super) fn event_index(w: &mut WeaponCtx<'_, '_>, name: &str) -> u16 {
    w.eng.precache_event(name)
}

/// The empty-clip part of every `*Fire`: `if (m_fFireOnEmpty) { PlayEmptySound();
/// m_flNextPrimaryAttack = GetNextAttackDelay(0.2); }` (the bots' event is not ported).
pub(super) fn fire_on_empty(w: &mut WeaponCtx<'_, '_>) {
    if w.weapon.fire_on_empty {
        w.play_empty_sound();
        w.weapon.next_primary_attack = w.get_next_attack_delay(0.2);
    }
}

/// `pev->effects |= EF_MUZZLEFLASH`.
pub(super) fn muzzle_flash(w: &mut WeaponCtx<'_, '_>) {
    let id = w.edict();
    if let Some(v) = w.eng.ent_mut(id) {
        v.effects |= EF_MUZZLEFLASH;
    }
}

/// `UTIL_MakeVectors(v_angle + punchangle)` then `vecSrc = GetGunPosition()` and
/// `vecAiming = gpGlobals->v_forward`: the source and the direction of the shot.
pub(super) fn aim(w: &WeaponCtx<'_, '_>) -> (Vec3, Vec3) {
    let id = w.edict();
    w.eng.ent(id).map_or((Vec3::ZERO, Vec3::X), |v| {
        let (forward, ..) = angle_vectors(v.v_angle + v.punchangle);
        (v.origin + v.view_ofs, forward)
    })
}

/// `pev->punchangle`.
pub(super) fn punch(w: &WeaponCtx<'_, '_>) -> Vec3 {
    let id = w.edict();
    w.eng.ent(id).map_or(Vec3::ZERO, |v| v.punchangle)
}

/// `int(punchangle * 100)`: a float times an int is a float, cut toward zero.
pub(super) fn int_x100(v: f32) -> i32 {
    (v * 100.0) as i32
}

/// The `WeaponIdle` every gun of this file has (the others' differ in a `<=` that means the
/// same): the idle timer runs while it is above 0; then 20 s more and the idle animation.
/// `ResetEmptySound()` comes first, as in the source.
pub(super) fn weapon_idle(w: &mut WeaponCtx<'_, '_>, idle_anim: i32) {
    w.reset_empty_sound();
    // `m_pPlayer->GetAutoaimVector(AUTOAIM_10DEGREES)`: not ported (named in the module docs).
    if w.weapon.time_weapon_idle > 0.0 {
        return;
    }
    w.weapon.time_weapon_idle = 20.0;
    w.send_weapon_anim(idle_anim, true);
}

/// `m_flAccuracy` for a cubed shot count over an `int` divisor, plus `base` (a `float`), then
/// capped at `cap`: `((n * n * n) / DIVISOR) + base`. The count wraps like a C `int`.
pub(super) fn accuracy_cubed(shots: i32, divisor: i32, base: f32, cap: f32) -> f32 {
    let cubed = shots.wrapping_mul(shots).wrapping_mul(shots);
    let accuracy = (cubed / divisor) as f32 + base;
    if accuracy > cap { cap } else { accuracy }
}

/// `m_flAccuracy` for a squared shot count over an `int` divisor, plus `base`, capped.
pub(super) fn accuracy_squared(shots: i32, divisor: i32, base: f32, cap: f32) -> f32 {
    let squared = shots.wrapping_mul(shots);
    let accuracy = (squared / divisor) as f32 + base;
    if accuracy > cap { cap } else { accuracy }
}

/// Runs a spread expression the way C does: a `double` result handed to a `float` parameter.
pub(super) fn f32_of(spread: f64) -> f32 {
    spread as f32
}

/// `KickBack` of the SMGs and the rifles: by state, the arguments the source gives.
pub(super) type Kick = (f32, f32, f32, f32, f32, f32, i32);

/// Calls `KickBack` with a [`Kick`].
pub(super) fn kick(w: &mut WeaponCtx<'_, '_>, k: Kick) {
    w.kick_back(k.0, k.1, k.2, k.3, k.4, k.5, k.6);
}

// ----------------------------------------------------------------------------------- MAC-10

const MAC10_MAX_SPEED: f32 = 250.0;
const MAC10_DAMAGE: f32 = 29.0;
const MAC10_RANGE_MODIFER: f32 = 0.82;
const MAC10_RELOAD_TIME: f32 = 3.15;
const MAC10_ACCURACY_DIVISOR: i32 = 200;
const MAC10_IDLE1: i32 = 0;
const MAC10_RELOAD: i32 = 1;
const MAC10_DRAW: i32 = 2;

/// The MAC-10 (`CMAC10`).
#[derive(Debug, Clone, Default)]
pub struct Mac10;

impl WeaponBehavior for Mac10 {
    fn id(&self) -> WeaponId {
        WeaponId::Mac10
    }

    /// `CMAC10::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_mac10.mdl", "models/w_mac10.mdl"] {
            let _ = eng.precache_model(model);
        }
        for sound in [
            "weapons/mac10-1.wav",
            "weapons/mac10_clipout.wav",
            "weapons/mac10_clipin.wav",
            "weapons/mac10_boltpull.wav",
        ] {
            let _ = eng.precache_sound(sound);
        }
        let _ = eng.precache_model("models/pshell.mdl");
        eng.precache_event("events/mac10.sc");
    }

    /// `CMAC10::Deploy`.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.accuracy = 0.15;
        w.weapon.delay_fire = false;
        w.default_deploy(&*self, "models/v_mac10.mdl", "models/p_mac10.mdl", MAC10_DRAW, "onehanded", true)
    }

    /// `CMAC10::PrimaryAttack`: `double` spreads, handed to the `float` parameter.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let accuracy = f64::from(w.weapon.accuracy);
        if !stance(w).on_ground {
            mac10_fire(w, f32_of(0.375 * accuracy), 0.07);
        } else {
            mac10_fire(w, f32_of(0.03 * accuracy), 0.07);
        }
    }

    /// `CMAC10::Reload`: the legacy `ammo_45acp` counter must be above 0 (vanilla).
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.player().tabulated_ammo[TAB_45ACP] <= 0 {
            return;
        }
        if w.default_reload(w.max_clip(), MAC10_RELOAD, MAC10_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.0;
            w.weapon.shots_fired = 0;
        }
    }

    /// `CMAC10::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        weapon_idle(w, MAC10_IDLE1);
    }

    fn max_speed(&self, _w: &super::weapon::Weapon) -> f32 {
        MAC10_MAX_SPEED
    }
}

/// `CMAC10::MAC10Fire` (`wpn_mac10.cpp`, read 2026-11-22). Accuracy cubed over 200 plus 0.6,
/// at most 1.65. The empty-clip test comes after the accuracy update (the shot count goes up
/// for a click as well).
fn mac10_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, cycle_time: f32) {
    w.weapon.delay_fire = true;
    w.weapon.shots_fired += 1;
    w.weapon.accuracy = accuracy_cubed(w.weapon.shots_fired, MAC10_ACCURACY_DIVISOR, 0.6, 1.65);

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
        src, aiming, spread, 8192.0, 1, BulletType::Player45acp, MAC10_DAMAGE as i32,
        MAC10_RANGE_MODIFER, Some(edict), false, seed,
    );

    let punch = punch(w);
    let event = event_index(w, "events/mac10.sc");
    w.playback_event(
        FEV_NOTHOST, event, dir.x, dir.y, int_x100(punch.x), int_x100(punch.y), false, false,
    );

    w.weapon.next_primary_attack = w.get_next_attack_delay(cycle_time);
    w.weapon.next_secondary_attack = w.weapon.next_primary_attack;

    w.weapon.time_weapon_idle = 2.0;

    let s = stance(w);
    if !s.on_ground {
        kick(w, (1.3, 0.55, 0.4, 0.05, 4.75, 3.75, 5));
    } else if s.speed_2d > 0.0 {
        kick(w, (0.9, 0.45, 0.25, 0.035, 3.5, 2.75, 7));
    } else if s.ducking {
        kick(w, (0.75, 0.4, 0.175, 0.03, 2.75, 2.5, 10));
    } else {
        kick(w, (0.775, 0.425, 0.2, 0.03, 3.0, 2.75, 9));
    }
}

// ---------------------------------------------------------------------------------------- TMP

const TMP_MAX_SPEED: f32 = 250.0;
const TMP_DAMAGE: f32 = 20.0;
const TMP_RANGE_MODIFER: f32 = 0.85;
const TMP_RELOAD_TIME: f32 = 2.12;
const TMP_ACCURACY_DIVISOR: i32 = 200;
const TMP_IDLE1: i32 = 0;
const TMP_RELOAD: i32 = 1;
const TMP_DRAW: i32 = 2;

/// The TMP (`CTMP`).
#[derive(Debug, Clone, Default)]
pub struct Tmp;

impl WeaponBehavior for Tmp {
    fn id(&self) -> WeaponId {
        WeaponId::Tmp
    }

    /// `CTMP::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_tmp.mdl", "models/w_tmp.mdl"] {
            let _ = eng.precache_model(model);
        }
        for sound in ["weapons/tmp-1.wav", "weapons/tmp-2.wav"] {
            let _ = eng.precache_sound(sound);
        }
        let _ = eng.precache_model("models/pshell.mdl");
        eng.precache_event("events/tmp.sc");
    }

    /// `CTMP::Deploy`.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.accuracy = 0.2;
        w.weapon.shots_fired = 0;
        w.weapon.delay_fire = false;
        w.default_deploy(&*self, "models/v_tmp.mdl", "models/p_tmp.mdl", TMP_DRAW, "onehanded", true)
    }

    /// `CTMP::PrimaryAttack`.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let accuracy = f64::from(w.weapon.accuracy);
        if !stance(w).on_ground {
            tmp_fire(w, f32_of(0.25 * accuracy), 0.07);
        } else {
            tmp_fire(w, f32_of(0.03 * accuracy), 0.07);
        }
    }

    /// `CTMP::Reload`: no ammo test (the TMP has none).
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.default_reload(w.max_clip(), TMP_RELOAD, TMP_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.2;
            w.weapon.shots_fired = 0;
        }
    }

    /// `CTMP::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        weapon_idle(w, TMP_IDLE1);
    }

    fn max_speed(&self, _w: &super::weapon::Weapon) -> f32 {
        TMP_MAX_SPEED
    }
}

/// `CTMP::TMPFire` (`wpn_tmp.cpp`, read 2026-11-22). QUIRKS kept: the TMP is the silenced SMG,
/// so there is no `EF_MUZZLEFLASH`; the event's first bool is the int 5 (`TRUE`), which the
/// event delta carries as one bit.
fn tmp_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, cycle_time: f32) {
    w.weapon.delay_fire = true;
    w.weapon.shots_fired += 1;
    w.weapon.accuracy = accuracy_cubed(w.weapon.shots_fired, TMP_ACCURACY_DIVISOR, 0.55, 1.4);

    if w.weapon.clip <= 0 {
        fire_on_empty(w);
        return;
    }

    w.weapon.clip -= 1;
    w.set_animation(PlayerAnim::Attack1);

    let (src, aiming) = aim(w);
    let edict = w.edict();
    let seed = w.player().random_seed;
    let dir = w.fire_bullets3(
        src, aiming, spread, 8192.0, 1, BulletType::Player9mm, TMP_DAMAGE as i32,
        TMP_RANGE_MODIFER, Some(edict), false, seed,
    );

    let punch = punch(w);
    let event = event_index(w, "events/tmp.sc");
    w.playback_event(
        FEV_NOTHOST, event, dir.x, dir.y, int_x100(punch.x), int_x100(punch.y), true, false,
    );

    w.weapon.next_primary_attack = w.get_next_attack_delay(cycle_time);
    w.weapon.next_secondary_attack = w.weapon.next_primary_attack;

    w.weapon.time_weapon_idle = 2.0;

    let s = stance(w);
    if !s.on_ground {
        kick(w, (1.1, 0.5, 0.35, 0.045, 4.5, 3.5, 6));
    } else if s.speed_2d > 0.0 {
        kick(w, (0.8, 0.4, 0.2, 0.03, 3.0, 2.5, 7));
    } else if s.ducking {
        kick(w, (0.7, 0.35, 0.125, 0.025, 2.5, 2.0, 10));
    } else {
        kick(w, (0.725, 0.375, 0.15, 0.025, 2.75, 2.25, 9));
    }
}

// ------------------------------------------------------------------------------------ MP5 Navy

const MP5N_MAX_SPEED: f32 = 250.0;
const MP5N_DAMAGE: f32 = 26.0;
const MP5N_RANGE_MODIFER: f32 = 0.84;
const MP5N_RELOAD_TIME: f32 = 2.63;
/// A `double` in vanilla (`REGAMEDLL_FIXES` makes it a float).
const MP5N_ACCURACY_DIVISOR: f64 = 220.1;
const MP5N_IDLE1: i32 = 0;
const MP5N_RELOAD: i32 = 1;
const MP5N_DRAW: i32 = 2;

/// The MP5 Navy (`CMP5N`).
#[derive(Debug, Clone, Default)]
pub struct Mp5n;

impl WeaponBehavior for Mp5n {
    fn id(&self) -> WeaponId {
        WeaponId::Mp5n
    }

    /// `CMP5N::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_mp5.mdl", "models/w_mp5.mdl"] {
            let _ = eng.precache_model(model);
        }
        for sound in [
            "weapons/mp5-1.wav",
            "weapons/mp5-2.wav",
            "weapons/mp5_clipout.wav",
            "weapons/mp5_clipin.wav",
            "weapons/mp5_slideback.wav",
        ] {
            let _ = eng.precache_sound(sound);
        }
        let _ = eng.precache_model("models/pshell.mdl");
        eng.precache_event("events/mp5n.sc");
    }

    /// `CMP5N::Deploy`.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.accuracy = 0.0;
        w.weapon.delay_fire = false;
        w.default_deploy(&*self, "models/v_mp5.mdl", "models/p_mp5.mdl", MP5N_DRAW, "mp5", true)
    }

    /// `CMP5N::PrimaryAttack`.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let accuracy = f64::from(w.weapon.accuracy);
        if !stance(w).on_ground {
            mp5n_fire(w, f32_of(0.2 * accuracy), 0.075);
        } else {
            mp5n_fire(w, f32_of(0.04 * accuracy), 0.075);
        }
    }

    /// `CMP5N::Reload`: the legacy `ammo_9mm` counter must be above 0 (vanilla).
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.player().tabulated_ammo[TAB_9MM] <= 0 {
            return;
        }
        if w.default_reload(w.max_clip(), MP5N_RELOAD, MP5N_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.0;
            w.weapon.shots_fired = 0;
        }
    }

    /// `CMP5N::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        weapon_idle(w, MP5N_IDLE1);
    }

    fn max_speed(&self, _w: &super::weapon::Weapon) -> f32 {
        MP5N_MAX_SPEED
    }
}

/// `CMP5N::MP5NFire` (`wpn_mp5navy.cpp`, read 2026-11-22). QUIRK kept: the divisor is a
/// `double`, so `(n * n) / 220.1 + 0.45f` is computed in `f64` (the float widened) and cut when
/// stored, at most 0.75. The event comes before `m_flNextPrimaryAttack` and the flash flag is
/// set before the animation, as in the source.
fn mp5n_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, cycle_time: f32) {
    w.weapon.delay_fire = true;
    w.weapon.shots_fired += 1;
    let squared = w.weapon.shots_fired.wrapping_mul(w.weapon.shots_fired);
    w.weapon.accuracy =
        (f64::from(squared) / MP5N_ACCURACY_DIVISOR + f64::from(0.45_f32)) as f32;
    if w.weapon.accuracy > 0.75 {
        w.weapon.accuracy = 0.75;
    }

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
        src, aiming, spread, 8192.0, 1, BulletType::Player9mm, MP5N_DAMAGE as i32,
        MP5N_RANGE_MODIFER, Some(edict), false, seed,
    );

    let punch = punch(w);
    let event = event_index(w, "events/mp5n.sc");
    w.playback_event(
        FEV_NOTHOST, event, dir.x, dir.y, int_x100(punch.x), int_x100(punch.y), false, false,
    );

    w.weapon.next_primary_attack = w.get_next_attack_delay(cycle_time);
    w.weapon.next_secondary_attack = w.weapon.next_primary_attack;

    w.weapon.time_weapon_idle = 2.0;

    let s = stance(w);
    if !s.on_ground {
        kick(w, (0.9, 0.475, 0.35, 0.0425, 5.0, 3.0, 6));
    } else if s.speed_2d > 0.0 {
        kick(w, (0.5, 0.275, 0.2, 0.03, 3.0, 2.0, 10));
    } else if s.ducking {
        kick(w, (0.225, 0.15, 0.1, 0.015, 2.0, 1.0, 10));
    } else {
        kick(w, (0.25, 0.175, 0.125, 0.02, 2.25, 1.25, 10));
    }
}

// --------------------------------------------------------------------------------------- UMP45

const UMP45_MAX_SPEED: f32 = 250.0;
const UMP45_DAMAGE: f32 = 30.0;
const UMP45_RANGE_MODIFER: f32 = 0.82;
const UMP45_RELOAD_TIME: f32 = 3.5;
const UMP45_ACCURACY_DIVISOR: i32 = 210;
const UMP45_IDLE1: i32 = 0;
const UMP45_RELOAD: i32 = 1;
const UMP45_DRAW: i32 = 2;

/// The UMP45 (`CUMP45`).
#[derive(Debug, Clone, Default)]
pub struct Ump45;

impl WeaponBehavior for Ump45 {
    fn id(&self) -> WeaponId {
        WeaponId::Ump45
    }

    /// `CUMP45::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_ump45.mdl", "models/w_ump45.mdl"] {
            let _ = eng.precache_model(model);
        }
        for sound in [
            "weapons/ump45-1.wav",
            "weapons/ump45_clipout.wav",
            "weapons/ump45_clipin.wav",
            "weapons/ump45_boltslap.wav",
        ] {
            let _ = eng.precache_sound(sound);
        }
        let _ = eng.precache_model("models/pshell.mdl");
        eng.precache_event("events/ump45.sc");
    }

    /// `CUMP45::Deploy`.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.accuracy = 0.0;
        w.weapon.delay_fire = false;
        w.default_deploy(&*self, "models/v_ump45.mdl", "models/p_ump45.mdl", UMP45_DRAW, "carbine", true)
    }

    /// `CUMP45::PrimaryAttack`.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let accuracy = f64::from(w.weapon.accuracy);
        if !stance(w).on_ground {
            ump45_fire(w, f32_of(0.24 * accuracy), 0.1);
        } else {
            ump45_fire(w, f32_of(0.04 * accuracy), 0.1);
        }
    }

    /// `CUMP45::Reload`: the legacy `ammo_45acp` counter must be above 0 (vanilla).
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.player().tabulated_ammo[TAB_45ACP] <= 0 {
            return;
        }
        if w.default_reload(w.max_clip(), UMP45_RELOAD, UMP45_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.0;
            w.weapon.shots_fired = 0;
        }
    }

    /// `CUMP45::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        weapon_idle(w, UMP45_IDLE1);
    }

    fn max_speed(&self, _w: &super::weapon::Weapon) -> f32 {
        UMP45_MAX_SPEED
    }
}

/// `CUMP45::UMP45Fire` (`wpn_ump45.cpp`, read 2026-11-22). Accuracy squared over 210 plus 0.5,
/// at most 1. QUIRK kept: the airborne `KickBack` has an up base of 0.125 and a lateral base
/// of 0.65 (the source's numbers, not the pattern of the other guns).
fn ump45_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, cycle_time: f32) {
    w.weapon.delay_fire = true;
    w.weapon.shots_fired += 1;
    w.weapon.accuracy = accuracy_squared(w.weapon.shots_fired, UMP45_ACCURACY_DIVISOR, 0.5, 1.0);

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
        src, aiming, spread, 8192.0, 1, BulletType::Player45acp, UMP45_DAMAGE as i32,
        UMP45_RANGE_MODIFER, Some(edict), false, seed,
    );

    let punch = punch(w);
    let event = event_index(w, "events/ump45.sc");
    w.playback_event(
        FEV_NOTHOST, event, dir.x, dir.y, int_x100(punch.x), int_x100(punch.y), false, false,
    );

    w.weapon.next_primary_attack = w.get_next_attack_delay(cycle_time);
    w.weapon.next_secondary_attack = w.weapon.next_primary_attack;

    w.weapon.time_weapon_idle = 2.0;

    let s = stance(w);
    if !s.on_ground {
        kick(w, (0.125, 0.65, 0.55, 0.0475, 5.5, 4.0, 10));
    } else if s.speed_2d > 0.0 {
        kick(w, (0.55, 0.3, 0.225, 0.03, 3.5, 2.5, 10));
    } else if s.ducking {
        kick(w, (0.25, 0.175, 0.125, 0.02, 2.25, 1.25, 10));
    } else {
        kick(w, (0.275, 0.2, 0.15, 0.0225, 2.5, 1.5, 10));
    }
}

// ----------------------------------------------------------------------------------------- P90

const P90_MAX_SPEED: f32 = 245.0;
const P90_DAMAGE: f32 = 21.0;
const P90_RANGE_MODIFER: f32 = 0.885;
const P90_RELOAD_TIME: f32 = 3.4;
const P90_ACCURACY_DIVISOR: i32 = 175;
const P90_IDLE1: i32 = 0;
const P90_RELOAD: i32 = 1;
const P90_DRAW: i32 = 2;

/// The P90 (`CP90`).
#[derive(Debug, Clone, Default)]
pub struct P90;

impl WeaponBehavior for P90 {
    fn id(&self) -> WeaponId {
        WeaponId::P90
    }

    /// `CP90::Precache` (the brass is the rifle's `rshell`).
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_p90.mdl", "models/w_p90.mdl"] {
            let _ = eng.precache_model(model);
        }
        for sound in [
            "weapons/p90-1.wav",
            "weapons/p90_clipout.wav",
            "weapons/p90_clipin.wav",
            "weapons/p90_boltpull.wav",
            "weapons/p90_cliprelease.wav",
        ] {
            let _ = eng.precache_sound(sound);
        }
        let _ = eng.precache_model("models/rshell.mdl");
        eng.precache_event("events/p90.sc");
    }

    /// `CP90::Deploy`.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.shots_fired = 0;
        w.weapon.delay_fire = false;
        w.weapon.accuracy = 0.2;
        w.default_deploy(&*self, "models/v_p90.mdl", "models/p_p90.mdl", P90_DRAW, "carbine", true)
    }

    /// `CP90::PrimaryAttack`: the middle case is a speed over 170 (not "moving").
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let accuracy = f64::from(w.weapon.accuracy);
        let s = stance(w);
        if !s.on_ground {
            p90_fire(w, f32_of(0.3 * accuracy), 0.066);
        } else if s.speed_2d > 170.0 {
            p90_fire(w, f32_of(0.115 * accuracy), 0.066);
        } else {
            p90_fire(w, f32_of(0.045 * accuracy), 0.066);
        }
    }

    /// `CP90::Reload`: the legacy `ammo_57mm` counter must be above 0 (vanilla).
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.player().tabulated_ammo[TAB_57MM] <= 0 {
            return;
        }
        if w.default_reload(w.max_clip(), P90_RELOAD, P90_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.2;
            w.weapon.shots_fired = 0;
        }
    }

    /// `CP90::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        weapon_idle(w, P90_IDLE1);
    }

    /// `CP90::GetMaxSpeed`.
    fn max_speed(&self, _w: &super::weapon::Weapon) -> f32 {
        P90_MAX_SPEED
    }
}

/// `CP90::P90Fire` (`wpn_p90.cpp`, read 2026-11-22). Accuracy squared over 175 plus 0.45, at
/// most 1 (`n * n / DIVISOR` is a left-to-right `int` expression). The event's first bool is
/// the int 5 (`TRUE`).
fn p90_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, cycle_time: f32) {
    w.weapon.delay_fire = true;
    w.weapon.shots_fired += 1;
    w.weapon.accuracy = accuracy_squared(w.weapon.shots_fired, P90_ACCURACY_DIVISOR, 0.45, 1.0);

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
        src, aiming, spread, 8192.0, 1, BulletType::Player57mm, P90_DAMAGE as i32,
        P90_RANGE_MODIFER, Some(edict), false, seed,
    );

    let punch = punch(w);
    let event = event_index(w, "events/p90.sc");
    w.playback_event(
        FEV_NOTHOST, event, dir.x, dir.y, int_x100(punch.x), int_x100(punch.y), true, false,
    );

    w.weapon.next_primary_attack = w.get_next_attack_delay(cycle_time);
    w.weapon.next_secondary_attack = w.weapon.next_primary_attack;

    w.weapon.time_weapon_idle = 2.0;

    let s = stance(w);
    if !s.on_ground {
        kick(w, (0.9, 0.45, 0.35, 0.04, 5.25, 3.5, 4));
    } else if s.speed_2d > 0.0 {
        kick(w, (0.45, 0.3, 0.2, 0.0275, 4.0, 2.25, 7));
    } else if s.ducking {
        kick(w, (0.275, 0.2, 0.125, 0.02, 3.0, 1.0, 9));
    } else {
        kick(w, (0.3, 0.225, 0.125, 0.02, 3.25, 1.25, 8));
    }
}
