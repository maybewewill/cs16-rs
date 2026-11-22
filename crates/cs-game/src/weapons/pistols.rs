//! The pistols: the Glock 18, USP, P228, Desert Eagle, Five-seven and the dual Berettas. Ported
//! from ReGameDLL_CS `wpn_shared/wpn_glock18.cpp`, `wpn_usp.cpp`, `wpn_p228.cpp`,
//! `wpn_deagle.cpp`, `wpn_fiveseven.cpp` and `wpn_elite.cpp` (and the constants of `weapons.h`),
//! read 2026-11-22, the vanilla build: `CLIENT_WEAPONS`, `REGAMEDLL_API`, and the branch that is
//! not under `REGAMEDLL_FIXES`, `REGAMEDLL_ADD` or `BUILD_LATEST`.
//!
//! What every pistol has in common, and what is kept from the source:
//!
//! - The spread is `K * (1 - m_flAccuracy)` with `K` a `double` literal picked by the state of
//!   the player (in the air, moving, ducking, standing); the product is a double that is cut to
//!   the `float` of the parameter.
//! - A pistol is semi-automatic by `++m_iShotsFired > 1`, which is tested before anything else of
//!   the shot (the Glock's burst does not count). The count goes back to 0 when the buttons are
//!   up (`ItemPostFrame`, [`super::core::item_post_frame`]).
//! - `m_flAccuracy` falls by `(K1 - time since the last shot) * K2` per shot, so a quick tap
//!   loses accuracy, and is bounded; vanilla never resets it when the shots fired go back to 0.
//! - The recoil of vanilla is the bare `punchangle.x -= 2` (`KickBack` is `REGAMEDLL_ADD`); the
//!   Glock has none.
//! - Each `*Fire` ends with `ResetPlayerShieldAnim` (not the Elite, which has no shield).
//! - `Reload` starts with `if (m_pPlayer->ammo_XX <= 0) return;` (`REGAMEDLL_FIXES` drops it):
//!   the legacy counter [`crate::player::Player::tabulated_ammo`] decides, not `m_rgAmmo`.
//!
//! Not ported, and named:
//!
//! - `Spawn` (`SET_MODEL`, `FallInit`, the entity half of a weapon) belongs to the weapon entity
//!   of Task 12. What it writes into the weapon is covered: `m_iDefaultAmmo` by
//!   [`Weapon::new`](super::weapon::Weapon::new), and `m_flAccuracy`, which every `Deploy`
//!   writes again before a shot is possible. `m_bBurstFire` is only ever written (false).
//! - `GetAutoaimVector(AUTOAIM_10DEGREES)` at the top of every `WeaponIdle` (no weapon of
//!   CS 1.6 uses the vector, see [`super::core`]), `SetSuitUpdate("!HEV_AMO0")` (no player of
//!   CS has the suit, so the function returns at once), `TheBots->OnEvent` (no bots),
//!   `m_iWeaponVolume`/`m_iWeaponFlash` (only the monsters and the bots' hearing read them).
//! - The events are looked up by name every time they are played (the C code keeps the index
//!   `PRECACHE_EVENT` returned in a member of the weapon): a name that was precached gives its
//!   index back, one that was not gives 0 and the engine drops the event. This keeps the
//!   behaviours stateless.

use cs_engine::consts::{FL_DUCKING, FL_ONGROUND};
use cs_engine::{EngineApi, EdictId};
use cs_engine::event_flags::FEV_NOTHOST;
use cs_sim::pm::math::angle_vectors;
use glam::Vec3;

use super::core::{GLOCK18_DAMAGE, PlayerAnim, TAB_9MM, WeaponCtx};
use super::info::{
    BulletType, WPNSTATE_ELITE_LEFT, WPNSTATE_GLOCK18_BURST_MODE, WPNSTATE_SHIELD_DRAWN,
    WPNSTATE_USP_SILENCED, WeaponId,
};
use super::weapon::{Weapon, WeaponBehavior};
use crate::chat::{HUD_PRINTCENTER, client_print};
use crate::player::HIDEHUD_CROSSHAIR;

/// `EF_MUZZLEFLASH`.
const EF_MUZZLEFLASH: u32 = 2;

/// The `K` of each pistol's spread, `[in the air, moving, ducking, standing]` (the literals of
/// the `PrimaryAttack` of each source file).
const GLOCK18_BURST_K: [f64; 4] = [1.2, 0.185, 0.095, 0.3];
const GLOCK18_SEMI_K: [f64; 4] = [1.0, 0.165, 0.075, 0.1];
const USP_SIL_K: [f64; 4] = [1.3, 0.25, 0.125, 0.15];
const USP_K: [f64; 4] = [1.2, 0.225, 0.08, 0.1];
const P228_K: [f64; 4] = [1.5, 0.255, 0.075, 0.15];
const DEAGLE_K: [f64; 4] = [1.5, 0.25, 0.115, 0.13];
const FIVESEVEN_K: [f64; 4] = [1.5, 0.255, 0.075, 0.15];
const ELITE_K: [f64; 4] = [1.3, 0.175, 0.08, 0.1];

/// The indices of the legacy ammo counters in [`crate::player::Player::tabulated_ammo`]
/// (`ammo_9mm`, `ammo_45acp`, `ammo_50ae`, `ammo_57mm`, `ammo_357sig`; the order of
/// [`super::core::TABULATED_AMMO`]).
const TAB_45ACP: usize = 5;
const TAB_50AE: usize = 6;
const TAB_57MM: usize = 8;
const TAB_357SIG: usize = 9;

/// The animations of the shield variants, the same sequence numbers in every pistol's
/// `*_shield_e` enum (`weapons.h`).
mod shield_anim {
    pub const RELOAD: i32 = 4;
    pub const DRAW: i32 = 5;
    pub const IDLE_UP: i32 = 6;
    pub const UP: i32 = 7;
    pub const DOWN: i32 = 8;
}

/// `glock18_e` (`weapons.h`).
mod glock_anim {
    pub const IDLE1: i32 = 0;
    pub const IDLE2: i32 = 1;
    pub const IDLE3: i32 = 2;
    pub const RELOAD: i32 = 7;
    pub const DRAW: i32 = 8;
    pub const DRAW2: i32 = 11;
    pub const RELOAD2: i32 = 12;
}

/// `usp_e`.
mod usp_anim {
    pub const IDLE: i32 = 0;
    pub const RELOAD: i32 = 5;
    pub const DRAW: i32 = 6;
    pub const ATTACH_SILENCER: i32 = 7;
    pub const UNSIL_IDLE: i32 = 8;
    pub const UNSIL_RELOAD: i32 = 13;
    pub const UNSIL_DRAW: i32 = 14;
    pub const DETACH_SILENCER: i32 = 15;
}

/// `p228_e`, `deagle_e`, `fiveseven_e`: the idle, the reload and the draw.
const P228_IDLE: i32 = 0;
const P228_RELOAD: i32 = 5;
const P228_DRAW: i32 = 6;
const DEAGLE_RELOAD: i32 = 4;
const DEAGLE_DRAW: i32 = 5;
const FIVESEVEN_IDLE: i32 = 0;
const FIVESEVEN_RELOAD: i32 = 4;
const FIVESEVEN_DRAW: i32 = 5;

/// `elite_e`.
mod elite_anim {
    pub const IDLE: i32 = 0;
    pub const IDLE_LEFTEMPTY: i32 = 1;
    pub const RELOAD: i32 = 14;
    pub const DRAW: i32 = 15;
}

/// `GLOCK18_MAX_SPEED`, `USP_MAX_SPEED`, `P228_MAX_SPEED`, `DEAGLE_MAX_SPEED`,
/// `FIVESEVEN_MAX_SPEED` and `ELITE_MAX_SPEED`: all 250.
pub const PISTOL_MAX_SPEED: f32 = 250.0;
/// `GLOCK18_RANGE_MODIFER`.
pub const GLOCK18_RANGE_MODIFIER: f32 = 0.75;
/// `GLOCK18_RELOAD_TIME`.
pub const GLOCK18_RELOAD_TIME: f32 = 2.2;
/// `USP_DAMAGE`.
pub const USP_DAMAGE: f32 = 34.0;
/// `USP_DAMAGE_SIL`.
pub const USP_DAMAGE_SIL: f32 = 30.0;
/// `USP_RANGE_MODIFER`.
pub const USP_RANGE_MODIFIER: f32 = 0.79;
/// `USP_RELOAD_TIME`.
pub const USP_RELOAD_TIME: f32 = 2.7;
/// `USP_ADJUST_SIL_TIME` (vanilla: 3.0; `REGAMEDLL_FIXES` makes it 3.13).
pub const USP_ADJUST_SIL_TIME: f32 = 3.0;
/// `P228_DAMAGE`.
pub const P228_DAMAGE: f32 = 32.0;
/// `P228_RANGE_MODIFER`.
pub const P228_RANGE_MODIFIER: f32 = 0.8;
/// `P228_RELOAD_TIME`.
pub const P228_RELOAD_TIME: f32 = 2.7;
/// `DEAGLE_DAMAGE`.
pub const DEAGLE_DAMAGE: f32 = 54.0;
/// `DEAGLE_RANGE_MODIFER`.
pub const DEAGLE_RANGE_MODIFIER: f32 = 0.81;
/// `DEAGLE_RELOAD_TIME`.
pub const DEAGLE_RELOAD_TIME: f32 = 2.2;
/// `FIVESEVEN_DAMAGE`.
pub const FIVESEVEN_DAMAGE: f32 = 20.0;
/// `FIVESEVEN_RANGE_MODIFER`.
pub const FIVESEVEN_RANGE_MODIFIER: f32 = 0.885;
/// `FIVESEVEN_RELOAD_TIME`.
pub const FIVESEVEN_RELOAD_TIME: f32 = 2.7;
/// `ELITE_DAMAGE`.
pub const ELITE_DAMAGE: f32 = 36.0;
/// `ELITE_RANGE_MODIFER`.
pub const ELITE_RANGE_MODIFIER: f32 = 0.75;
/// `ELITE_RELOAD_TIME`.
pub const ELITE_RELOAD_TIME: f32 = 4.5;

/// `PRECACHE_MODEL`: a refused name is logged and the game goes on (the source would stop).
fn precache_model(eng: &mut EngineApi<'_>, name: &str) {
    if let Err(err) = eng.precache_model(name) {
        tracing::warn!(name, ?err, "pistol model not precached");
    }
}

/// `PRECACHE_SOUND`.
fn precache_sound(eng: &mut EngineApi<'_>, name: &str) {
    if let Err(err) = eng.precache_sound(name) {
        tracing::warn!(name, ?err, "pistol sound not precached");
    }
}

/// `m_pPlayer->HasShield()`'s `UpdateShieldCrosshair(draw)` (`player.cpp` 8799): the crosshair
/// is hidden while the shield is up.
fn update_shield_crosshair(w: &mut WeaponCtx<'_, '_>, draw: bool) {
    let p = w.player_mut();
    if draw {
        p.hide_hud &= !HIDEHUD_CROSSHAIR;
    } else {
        p.hide_hud |= HIDEHUD_CROSSHAIR;
    }
}

/// `CBasePlayerWeapon::SetPlayerShieldAnim` (`weapons.cpp` 656): a shield bearer's third-person
/// animation set while shooting.
fn set_player_shield_anim(w: &mut WeaponCtx<'_, '_>) {
    if !w.has_shield() {
        return;
    }
    let ext = if w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0 { "shield" } else { "shieldgun" };
    w.player_mut().anim_extension = ext.to_string();
}

/// `CBasePlayerWeapon::ResetPlayerShieldAnim` (`weapons.cpp` 671): with the shield up the
/// animation set goes back to "shieldgun". (The `shielded` set is only set by
/// [`shield_secondary_fire`].)
fn reset_player_shield_anim(w: &mut WeaponCtx<'_, '_>) {
    if w.has_shield() && w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0 {
        w.player_mut().anim_extension = "shieldgun".to_string();
    }
}

/// `CBasePlayerWeapon::ShieldSecondaryFire` (`weapons.cpp` 699): false without a shield. Else
/// lowers or raises the shield (the animation, the animation set, the speed 250 or 180, the
/// player's `m_bShieldDrawn`), shows or hides the crosshair, resets the player's speed and holds
/// everything for 0.4 s (the idle 0.6 s). QUIRK: these are not `UTIL_WeaponTimeBase() +` sums,
/// they are plain assignments (the same thing with `CLIENT_WEAPONS`).
fn shield_secondary_fire(
    me: &dyn WeaponBehavior,
    w: &mut WeaponCtx<'_, '_>,
    up_anim: i32,
    down_anim: i32,
) -> bool {
    if !w.has_shield() {
        return false;
    }
    if w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0 {
        w.weapon.weapon_state &= !WPNSTATE_SHIELD_DRAWN;
        w.send_weapon_anim(down_anim, me.use_decrement());
        w.player_mut().anim_extension = "shieldgun".to_string();
        w.weapon.max_speed = 250.0;
        w.player_mut().shield_drawn = false;
    } else {
        w.weapon.weapon_state |= WPNSTATE_SHIELD_DRAWN;
        w.send_weapon_anim(up_anim, me.use_decrement());
        w.player_mut().anim_extension = "shielded".to_string();
        w.weapon.max_speed = 180.0;
        w.player_mut().shield_drawn = true;
    }
    let drawn = w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN == WPNSTATE_SHIELD_DRAWN;
    update_shield_crosshair(w, !drawn);
    w.reset_max_speed(me);
    w.weapon.next_secondary_attack = 0.4;
    w.weapon.next_primary_attack = 0.4;
    w.weapon.time_weapon_idle = 0.6;
    true
}

/// The spread of the shot (`K * (1 - m_flAccuracy)`, the call's first argument) for the state
/// of the player: `[in the air, moving, ducking, standing]`, the `K` of each. `1 - m_flAccuracy`
/// is a float (an int minus a float), the `K` a double literal, and the product is cut to
/// `float` by the parameter.
fn spread_by_state(w: &WeaponCtx<'_, '_>, k: [f64; 4]) -> f32 {
    let (flags, velocity) = w.eng.ent(w.edict()).map_or((0, Vec3::ZERO), |v| (v.flags, v.velocity));
    spread_of(k, flags, velocity, w.weapon.accuracy)
}

/// [`spread_by_state`] on the state itself: the player's `pev->flags` and velocity.
fn spread_of(k: [f64; 4], flags: u32, velocity: Vec3, accuracy: f32) -> f32 {
    let k = if flags & FL_ONGROUND == 0 {
        k[0]
    } else if velocity.truncate().length() > 0.0 {
        k[1]
    } else if flags & FL_DUCKING != 0 {
        k[2]
    } else {
        k[3]
    };
    (k * f64::from(1.0_f32 - accuracy)) as f32
}

/// `m_flAccuracy -= (base - (gpGlobals->time - m_flLastFire)) * k;` and the clamp to
/// `[low, high]` (an `else if`: the low bound is tested only when the high one is not
/// crossed). The caller tests `m_flLastFire` first.
fn lose_accuracy(w: &mut WeaponCtx<'_, '_>, base: f32, k: f32, high: f32, low: f32) {
    let elapsed = w.now() - w.weapon.last_fire;
    w.weapon.accuracy -= (base - elapsed) * k;
    if w.weapon.accuracy > high {
        w.weapon.accuracy = high;
    } else if w.weapon.accuracy < low {
        w.weapon.accuracy = low;
    }
}

/// The empty clip of a `*Fire`: `if (m_iClip <= 0) { if (m_fFireOnEmpty) { PlayEmptySound();
/// m_flNextPrimaryAttack = GetNextAttackDelay(0.2); } /* bots */ return; }`. True when the clip
/// was empty (the caller returns).
fn fire_on_empty(w: &mut WeaponCtx<'_, '_>) -> bool {
    if w.weapon.clip > 0 {
        return false;
    }
    if w.weapon.fire_on_empty {
        w.play_empty_sound();
        w.weapon.next_primary_attack = w.get_next_attack_delay(0.2_f64 as f32);
    }
    true
}

/// `m_pPlayer->GetGunPosition()`: the eye.
fn gun_position(w: &WeaponCtx<'_, '_>) -> Vec3 {
    w.eng.ent(w.edict()).map_or(Vec3::ZERO, |v| v.origin + v.view_ofs)
}

/// `UTIL_MakeVectors(m_pPlayer->pev->v_angle + m_pPlayer->pev->punchangle)`: `v_forward`,
/// `v_right`, `v_up`.
fn make_vectors(w: &WeaponCtx<'_, '_>) -> (Vec3, Vec3, Vec3) {
    angle_vectors(w.eng.ent(w.edict()).map_or(Vec3::ZERO, |v| v.v_angle + v.punchangle))
}

/// `m_pPlayer->pev->effects |= EF_MUZZLEFLASH`.
fn muzzle_flash(w: &mut WeaponCtx<'_, '_>) {
    let id = w.edict();
    if let Some(v) = w.eng.ent_mut(id) {
        v.effects |= EF_MUZZLEFLASH;
    }
}

/// `m_pPlayer->pev->punchangle`.
fn punch(w: &WeaponCtx<'_, '_>) -> Vec3 {
    w.eng.ent(w.edict()).map_or(Vec3::ZERO, |v| v.punchangle)
}

/// The vanilla recoil, `m_pPlayer->pev->punchangle.x -= amount`.
fn punch_up(w: &mut WeaponCtx<'_, '_>, amount: f32) {
    let id = w.edict();
    if let Some(v) = w.eng.ent_mut(id) {
        v.punchangle.x -= amount;
    }
}

/// `PLAYBACK_EVENT_FULL(flag, m_pPlayer->edict(), event, 0, g_vecZero, g_vecZero, ...)` with
/// `flag = FEV_NOTHOST` (`CLIENT_WEAPONS`), the event looked up by name (see the module
/// documentation).
#[allow(clippy::too_many_arguments)]
fn play_fire_event(
    w: &mut WeaponCtx<'_, '_>,
    event_name: &str,
    fparam1: f32,
    fparam2: f32,
    iparam1: i32,
    iparam2: i32,
    bparam1: bool,
    bparam2: bool,
) {
    let event = w.eng.precache_event(event_name);
    w.playback_event(FEV_NOTHOST, event, fparam1, fparam2, iparam1, iparam2, bparam1, bparam2);
}

/// The weapons' `m_pPlayer->ammo_*` test of `Reload`: true when the legacy counter is 0 or
/// below (including -1, an ammo that is not registered), and `Reload` returns.
fn legacy_ammo_empty(w: &WeaponCtx<'_, '_>, counter: usize) -> bool {
    w.player().tabulated_ammo[counter] <= 0
}

// ---------------------------------------------------------------------------------------------
// The Glock 18
// ---------------------------------------------------------------------------------------------

/// The Glock 18 (`CGLOCK18`).
#[derive(Debug, Clone, Default)]
pub struct Glock18;

impl WeaponBehavior for Glock18 {
    fn id(&self) -> WeaponId {
        WeaponId::Glock18
    }

    /// `CGLOCK18::Precache` (`wpn_glock18.cpp`). `m_iShellId = m_iShell` is the brass of the
    /// client's event; the server does not use it.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_glock18.mdl", "models/w_glock18.mdl", "models/shield/v_shield_glock18.mdl"] {
            precache_model(eng, model);
        }
        for sound in [
            "weapons/glock18-1.wav",
            "weapons/glock18-2.wav",
            "weapons/clipout1.wav",
            "weapons/clipin1.wav",
            "weapons/sliderelease1.wav",
            "weapons/slideback1.wav",
            "weapons/357_cock1.wav",
            "weapons/de_clipin.wav",
            "weapons/de_clipout.wav",
        ] {
            precache_sound(eng, sound);
        }
        precache_model(eng, "models/pshell.mdl");
        eng.precache_event("events/glock18.sc");
    }

    /// `CGLOCK18::Deploy` (`wpn_glock18.cpp`): the burst is cleared and the accuracy 0.9; with
    /// a shield the burst mode goes off, else the draw is one of two (`RANDOM_LONG(0, 1)`).
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.weapon_state &= !WPNSTATE_SHIELD_DRAWN;
        // `m_bBurstFire = false` is only ever written.
        w.weapon.glock18_shots_fired = 0;
        w.weapon.glock18_shoot = 0.0;
        w.weapon.accuracy = 0.9;
        w.weapon.max_speed = PISTOL_MAX_SPEED;
        w.player_mut().shield_drawn = false;

        if w.has_shield() {
            w.weapon.weapon_state &= !WPNSTATE_GLOCK18_BURST_MODE;
            return w.default_deploy(
                &*self,
                "models/shield/v_shield_glock18.mdl",
                "models/shield/p_shield_glock18.mdl",
                shield_anim::DRAW,
                "shieldgun",
                self.use_decrement(),
            );
        }
        let anim = if w.random_long(0, 1) != 0 { glock_anim::DRAW } else { glock_anim::DRAW2 };
        w.default_deploy(&*self, "models/v_glock18.mdl", "models/p_glock18.mdl", anim, "onehanded", self.use_decrement())
    }

    /// `CGLOCK18::PrimaryAttack`: the burst mode and the semi-automatic mode have each their own
    /// spread by state (the burst's is larger) and cycle time.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.weapon.weapon_state & WPNSTATE_GLOCK18_BURST_MODE != 0 {
            let spread = spread_by_state(w, GLOCK18_BURST_K);
            glock18_fire(w, spread, 0.5_f64 as f32, true);
        } else {
            let spread = spread_by_state(w, GLOCK18_SEMI_K);
            glock18_fire(w, spread, 0.2_f64 as f32, false);
        }
    }

    /// `CGLOCK18::SecondaryAttack`: the shield's, else the switch between the burst and the
    /// semi-automatic mode, announced on the HUD.
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if shield_secondary_fire(&*self, w, shield_anim::UP, shield_anim::DOWN) {
            return;
        }
        let slot = w.slot;
        if w.weapon.weapon_state & WPNSTATE_GLOCK18_BURST_MODE != 0 {
            client_print(w.eng, slot, HUD_PRINTCENTER, "#Switch_To_SemiAuto", &[]);
            w.weapon.weapon_state &= !WPNSTATE_GLOCK18_BURST_MODE;
        } else {
            client_print(w.eng, slot, HUD_PRINTCENTER, "#Switch_To_BurstFire", &[]);
            w.weapon.weapon_state |= WPNSTATE_GLOCK18_BURST_MODE;
        }
        w.weapon.next_secondary_attack = 0.3;
    }

    /// `CGLOCK18::Reload`: QUIRK, `ammo_9mm <= 0` returns first; the animation is picked before
    /// `DefaultReload` (a random one costs a `RANDOM_LONG` even if the reload does nothing).
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if legacy_ammo_empty(w, TAB_9MM) {
            return;
        }
        let anim = if w.has_shield() {
            shield_anim::RELOAD
        } else if w.random_long(0, 1) != 0 {
            glock_anim::RELOAD
        } else {
            glock_anim::RELOAD2
        };
        let max_clip = w.max_clip();
        if w.default_reload(max_clip, anim, GLOCK18_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.9;
        }
    }

    /// `CGLOCK18::WeaponIdle`: with a shield the idle is 20 s (and the "up" idle animation while
    /// the shield is drawn); else, while the slide is not back (`m_iClip`), one of three idle
    /// animations by `RANDOM_FLOAT(0, 1)`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.reset_empty_sound();
        if w.weapon.time_weapon_idle > 0.0 {
            return;
        }
        if w.has_shield() {
            w.weapon.time_weapon_idle = 20.0;
            if w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0 {
                w.send_weapon_anim(shield_anim::IDLE_UP, self.use_decrement());
            }
        } else if w.weapon.clip != 0 {
            let rand = w.random_float(0.0, 1.0);
            let (anim, idle) = if rand <= 0.3 {
                (glock_anim::IDLE3, 3.0625)
            } else if rand <= 0.6 {
                (glock_anim::IDLE1, 3.75)
            } else {
                (glock_anim::IDLE2, 2.5)
            };
            w.weapon.time_weapon_idle = idle;
            w.send_weapon_anim(anim, self.use_decrement());
        }
    }

    fn max_speed(&self, w: &Weapon) -> f32 {
        w.max_speed
    }

    fn is_pistol(&self) -> bool {
        true
    }
}

/// `CGLOCK18::GLOCK18Fire` (`wpn_glock18.cpp`, read 2026-11-22). A burst shot resets the burst
/// count and starts the burst (`m_flGlock18Shoot = time + 0.1`, the rest is
/// [`WeaponCtx::fire_remaining`]); a semi-automatic shot is the usual pistol one, with 0.05 off
/// the cycle time. QUIRKS kept: the accuracy decays in both modes (0.325 and 0.275, between 0.6
/// and 0.9); the first shot of the burst takes the cycle time of the burst mode (0.5) as is; the
/// event carries the clip-empty flag as its first bool; there is no recoil.
fn glock18_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, mut cycle_time: f32, burst: bool) {
    if burst {
        w.weapon.glock18_shots_fired = 0;
    } else {
        w.weapon.shots_fired += 1;
        if w.weapon.shots_fired > 1 {
            return;
        }
        cycle_time -= 0.05;
    }

    if w.weapon.last_fire != 0.0 {
        // Mark the time of this shot and determine the accuracy modifier based on the last shot
        // fired.
        lose_accuracy(w, 0.325, 0.275, 0.9, 0.6);
    }
    w.weapon.last_fire = w.now();

    if fire_on_empty(w) {
        return;
    }

    w.weapon.clip -= 1;
    muzzle_flash(w);
    // Player "shoot" animation.
    w.set_animation(PlayerAnim::Attack1);
    let (forward, _, _) = make_vectors(w);
    let src = gun_position(w);
    let seed = w.player().random_seed;
    let attacker = w.edict();
    let dir = w.fire_bullets3(
        src,
        forward,
        spread,
        8192.0,
        1,
        BulletType::Player9mm,
        GLOCK18_DAMAGE as i32,
        GLOCK18_RANGE_MODIFIER,
        Some(attacker),
        true,
        seed,
    );
    let p = punch(w);
    let clip_empty = w.weapon.clip == 0;
    play_fire_event(
        w,
        "events/glock18.sc",
        dir.x,
        dir.y,
        (p.x * 100.0) as i32,
        (p.y * 100.0) as i32,
        clip_empty,
        false,
    );

    let delay = w.get_next_attack_delay(cycle_time);
    w.weapon.next_primary_attack = delay;
    w.weapon.next_secondary_attack = delay;
    // `SetSuitUpdate("!HEV_AMO0")`: no suit.
    w.weapon.time_weapon_idle = 2.5;

    if burst {
        // Fire off the next two rounds.
        w.weapon.glock18_shots_fired += 1;
        w.weapon.glock18_shoot = w.now() + 0.1;
    }
    reset_player_shield_anim(w);
}

// ---------------------------------------------------------------------------------------------
// The USP
// ---------------------------------------------------------------------------------------------

/// The USP (`CUSP`).
#[derive(Debug, Clone, Default)]
pub struct Usp;

impl WeaponBehavior for Usp {
    fn id(&self) -> WeaponId {
        WeaponId::Usp
    }

    /// `CUSP::Precache` (`wpn_usp.cpp`).
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_usp.mdl", "models/w_usp.mdl", "models/shield/v_shield_usp.mdl"] {
            precache_model(eng, model);
        }
        for sound in [
            "weapons/usp1.wav",
            "weapons/usp2.wav",
            "weapons/usp_unsil-1.wav",
            "weapons/usp_clipout.wav",
            "weapons/usp_clipin.wav",
            "weapons/usp_silencer_on.wav",
            "weapons/usp_silencer_off.wav",
            "weapons/usp_sliderelease.wav",
            "weapons/usp_slideback.wav",
        ] {
            precache_sound(eng, sound);
        }
        precache_model(eng, "models/pshell.mdl");
        eng.precache_event("events/usp.sc");
    }

    /// `CUSP::Deploy`: the accuracy 0.92; a shield drops the silencer; the silenced and the
    /// unsilenced gun have each their draw animation.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.weapon_state &= !WPNSTATE_SHIELD_DRAWN;
        w.weapon.accuracy = 0.92;
        w.weapon.max_speed = PISTOL_MAX_SPEED;
        w.player_mut().shield_drawn = false;

        if w.has_shield() {
            w.weapon.weapon_state &= !WPNSTATE_USP_SILENCED;
            return w.default_deploy(
                &*self,
                "models/shield/v_shield_usp.mdl",
                "models/shield/p_shield_usp.mdl",
                shield_anim::DRAW,
                "shieldgun",
                self.use_decrement(),
            );
        }
        let anim = if w.weapon.weapon_state & WPNSTATE_USP_SILENCED != 0 {
            usp_anim::DRAW
        } else {
            usp_anim::UNSIL_DRAW
        };
        w.default_deploy(&*self, "models/v_usp.mdl", "models/p_usp.mdl", anim, "onehanded", self.use_decrement())
    }

    /// `CUSP::PrimaryAttack`: the silenced gun is a little less accurate on the move.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let k = if w.weapon.weapon_state & WPNSTATE_USP_SILENCED != 0 {
            USP_SIL_K
        } else {
            USP_K
        };
        let spread = spread_by_state(w, k);
        usp_fire(w, spread, 0.225_f64 as f32);
    }

    /// `CUSP::SecondaryAttack`: the shield's, else the silencer is taken off or put on (its
    /// animation, the animation set "onehanded" either way), and everything waits
    /// [`USP_ADJUST_SIL_TIME`] (vanilla 3.0 s; the secondary and the idle take it as is, the
    /// primary through `GetNextAttackDelay`).
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if shield_secondary_fire(&*self, w, shield_anim::UP, shield_anim::DOWN) {
            return;
        }
        if w.weapon.weapon_state & WPNSTATE_USP_SILENCED != 0 {
            w.weapon.weapon_state &= !WPNSTATE_USP_SILENCED;
            w.send_weapon_anim(usp_anim::DETACH_SILENCER, self.use_decrement());
            w.player_mut().anim_extension = "onehanded".to_string();
        } else {
            w.weapon.weapon_state |= WPNSTATE_USP_SILENCED;
            w.send_weapon_anim(usp_anim::ATTACH_SILENCER, self.use_decrement());
            w.player_mut().anim_extension = "onehanded".to_string();
        }
        w.weapon.next_secondary_attack = USP_ADJUST_SIL_TIME;
        w.weapon.time_weapon_idle = USP_ADJUST_SIL_TIME;
        w.weapon.next_primary_attack = w.get_next_attack_delay(USP_ADJUST_SIL_TIME);
    }

    /// `CUSP::Reload`: QUIRK, `ammo_45acp <= 0` returns first.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if legacy_ammo_empty(w, TAB_45ACP) {
            return;
        }
        let anim = if w.has_shield() {
            shield_anim::RELOAD
        } else if w.weapon.weapon_state & WPNSTATE_USP_SILENCED != 0 {
            usp_anim::RELOAD
        } else {
            usp_anim::UNSIL_RELOAD
        };
        let max_clip = w.max_clip();
        if w.default_reload(max_clip, anim, USP_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.92;
        }
    }

    /// `CUSP::WeaponIdle`: the idle timer is compared with 0 (not with the time base), the
    /// idle animation depends on the silencer and the next idle is a minute away.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.reset_empty_sound();
        if w.weapon.time_weapon_idle > 0.0 {
            return;
        }
        if w.has_shield() {
            w.weapon.time_weapon_idle = 20.0;
            if w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0 {
                w.send_weapon_anim(shield_anim::IDLE_UP, self.use_decrement());
            }
        } else if w.weapon.clip != 0 {
            let anim = if !w.weapon.weapon_state & WPNSTATE_USP_SILENCED != 0 {
                usp_anim::UNSIL_IDLE
            } else {
                usp_anim::IDLE
            };
            w.weapon.time_weapon_idle = 60.0;
            w.send_weapon_anim(anim, self.use_decrement());
        }
    }

    fn max_speed(&self, w: &Weapon) -> f32 {
        w.max_speed
    }

    fn is_pistol(&self) -> bool {
        true
    }
}

/// `CUSP::USPFire` (`wpn_usp.cpp`, read 2026-11-22). QUIRKS kept: the cycle time loses 0.075
/// before the semi-automatic test; the next attack is set BEFORE the clip goes down; the
/// silenced gun has no muzzle flash and does 30 instead of 34; the event's second integer is 0
/// (not the lateral punch) and its last bool is the silencer; the shot is a pistol one at
/// 4096 units with the .45 ACP bullet.
fn usp_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, mut cycle_time: f32) {
    cycle_time -= 0.075;

    w.weapon.shots_fired += 1;
    if w.weapon.shots_fired > 1 {
        return;
    }

    if w.weapon.last_fire != 0.0 {
        lose_accuracy(w, 0.3, 0.275, 0.92, 0.6);
    }
    w.weapon.last_fire = w.now();

    if fire_on_empty(w) {
        return;
    }

    let delay = w.get_next_attack_delay(cycle_time);
    w.weapon.next_primary_attack = delay;
    w.weapon.next_secondary_attack = delay;

    w.weapon.clip -= 1;
    set_player_shield_anim(w);

    w.set_animation(PlayerAnim::Attack1);
    let (forward, _, _) = make_vectors(w);

    let silenced = w.weapon.weapon_state & WPNSTATE_USP_SILENCED != 0;
    if !silenced {
        muzzle_flash(w);
    }

    let src = gun_position(w);
    let damage = if silenced { USP_DAMAGE_SIL } else { USP_DAMAGE };
    let seed = w.player().random_seed;
    let attacker = w.edict();
    let dir = w.fire_bullets3(
        src,
        forward,
        spread,
        4096.0,
        1,
        BulletType::Player45acp,
        damage as i32,
        USP_RANGE_MODIFIER,
        Some(attacker),
        true,
        seed,
    );
    let p = punch(w);
    let clip_empty = w.weapon.clip == 0;
    play_fire_event(w, "events/usp.sc", dir.x, dir.y, (p.x * 100.0) as i32, 0, clip_empty, silenced);

    // `SetSuitUpdate("!HEV_AMO0")`: no suit.
    w.weapon.time_weapon_idle = 2.0;
    punch_up(w, 2.0);
    reset_player_shield_anim(w);
}

// ---------------------------------------------------------------------------------------------
// The P228
// ---------------------------------------------------------------------------------------------

/// The P228 (`CP228`).
#[derive(Debug, Clone, Default)]
pub struct P228;

impl WeaponBehavior for P228 {
    fn id(&self) -> WeaponId {
        WeaponId::P228
    }

    /// `CP228::Precache` (`wpn_p228.cpp`).
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_p228.mdl", "models/w_p228.mdl", "models/shield/v_shield_p228.mdl"] {
            precache_model(eng, model);
        }
        for sound in [
            "weapons/p228-1.wav",
            "weapons/p228_clipout.wav",
            "weapons/p228_clipin.wav",
            "weapons/p228_sliderelease.wav",
            "weapons/p228_slidepull.wav",
        ] {
            precache_sound(eng, sound);
        }
        precache_model(eng, "models/pshell.mdl");
        eng.precache_event("events/p228.sc");
    }

    /// `CP228::Deploy`: the accuracy 0.9 and the speed are set first, then the shield state.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.accuracy = 0.9;
        w.weapon.max_speed = PISTOL_MAX_SPEED;
        w.weapon.weapon_state &= !WPNSTATE_SHIELD_DRAWN;
        w.player_mut().shield_drawn = false;
        if w.has_shield() {
            w.default_deploy(
                &*self,
                "models/shield/v_shield_p228.mdl",
                "models/shield/p_shield_p228.mdl",
                shield_anim::DRAW,
                "shieldgun",
                self.use_decrement(),
            )
        } else {
            w.default_deploy(&*self, "models/v_p228.mdl", "models/p_p228.mdl", P228_DRAW, "onehanded", self.use_decrement())
        }
    }

    /// `CP228::PrimaryAttack`.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let spread = spread_by_state(w, P228_K);
        p228_fire(w, spread, 0.2_f64 as f32);
    }

    /// `CP228::SecondaryAttack`: only the shield (the table of `HasSecondaryAttack` lets
    /// IN_ATTACK2 through for a shield bearer alone).
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        shield_secondary_fire(&*self, w, shield_anim::UP, shield_anim::DOWN);
    }

    /// `CP228::Reload`: QUIRK, `ammo_357sig <= 0` returns first.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if legacy_ammo_empty(w, TAB_357SIG) {
            return;
        }
        let anim = if w.has_shield() { shield_anim::RELOAD } else { P228_RELOAD };
        let max_clip = w.max_clip();
        if w.default_reload(max_clip, anim, P228_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.9;
        }
    }

    /// `CP228::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.reset_empty_sound();
        if w.weapon.time_weapon_idle > 0.0 {
            return;
        }
        if w.has_shield() {
            w.weapon.time_weapon_idle = 20.0;
            if w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0 {
                w.send_weapon_anim(shield_anim::IDLE_UP, self.use_decrement());
            }
        } else if w.weapon.clip != 0 {
            w.weapon.time_weapon_idle = 3.0625;
            w.send_weapon_anim(P228_IDLE, self.use_decrement());
        }
    }

    fn max_speed(&self, w: &Weapon) -> f32 {
        w.max_speed
    }

    fn is_pistol(&self) -> bool {
        true
    }
}

/// `CP228::P228Fire` (`wpn_p228.cpp`, read 2026-11-22): 0.05 off the cycle time, the accuracy
/// decays by `(0.325 - dt) * 0.3` within 0.6 and 0.9, the shot is a pistol one at 4096 units
/// with the .357 SIG bullet; the event carries both punch angles.
fn p228_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, mut cycle_time: f32) {
    cycle_time -= 0.05;

    w.weapon.shots_fired += 1;
    if w.weapon.shots_fired > 1 {
        return;
    }

    if w.weapon.last_fire != 0.0 {
        lose_accuracy(w, 0.325, 0.3, 0.9, 0.6);
    }
    w.weapon.last_fire = w.now();

    if fire_on_empty(w) {
        return;
    }

    w.weapon.clip -= 1;
    muzzle_flash(w);
    set_player_shield_anim(w);

    w.set_animation(PlayerAnim::Attack1);
    let (forward, _, _) = make_vectors(w);

    let src = gun_position(w);
    let seed = w.player().random_seed;
    let attacker = w.edict();
    let dir = w.fire_bullets3(
        src,
        forward,
        spread,
        4096.0,
        1,
        BulletType::Player357sig,
        P228_DAMAGE as i32,
        P228_RANGE_MODIFIER,
        Some(attacker),
        true,
        seed,
    );
    let p = punch(w);
    let clip_empty = w.weapon.clip == 0;
    play_fire_event(
        w,
        "events/p228.sc",
        dir.x,
        dir.y,
        (p.x * 100.0) as i32,
        (p.y * 100.0) as i32,
        clip_empty,
        false,
    );

    let delay = w.get_next_attack_delay(cycle_time);
    w.weapon.next_primary_attack = delay;
    w.weapon.next_secondary_attack = delay;
    // `SetSuitUpdate("!HEV_AMO0")`: no suit.
    w.weapon.time_weapon_idle = 2.0;
    punch_up(w, 2.0);
    reset_player_shield_anim(w);
}

// ---------------------------------------------------------------------------------------------
// The Desert Eagle
// ---------------------------------------------------------------------------------------------

/// The Desert Eagle (`CDEAGLE`).
#[derive(Debug, Clone, Default)]
pub struct Deagle;

impl WeaponBehavior for Deagle {
    fn id(&self) -> WeaponId {
        WeaponId::Deagle
    }

    /// `CDEAGLE::Precache` (`wpn_deagle.cpp`).
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_deagle.mdl", "models/shield/v_shield_deagle.mdl", "models/w_deagle.mdl"] {
            precache_model(eng, model);
        }
        for sound in [
            "weapons/deagle-1.wav",
            "weapons/deagle-2.wav",
            "weapons/de_clipout.wav",
            "weapons/de_clipin.wav",
            "weapons/de_deploy.wav",
        ] {
            precache_sound(eng, sound);
        }
        precache_model(eng, "models/pshell.mdl");
        eng.precache_event("events/deagle.sc");
    }

    /// `CDEAGLE::Deploy`.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.accuracy = 0.9;
        w.weapon.max_speed = PISTOL_MAX_SPEED;
        w.weapon.weapon_state &= !WPNSTATE_SHIELD_DRAWN;
        w.player_mut().shield_drawn = false;
        if w.has_shield() {
            w.default_deploy(
                &*self,
                "models/shield/v_shield_deagle.mdl",
                "models/shield/p_shield_deagle.mdl",
                shield_anim::DRAW,
                "shieldgun",
                self.use_decrement(),
            )
        } else {
            w.default_deploy(&*self, "models/v_deagle.mdl", "models/p_deagle.mdl", DEAGLE_DRAW, "onehanded", self.use_decrement())
        }
    }

    /// `CDEAGLE::PrimaryAttack`.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let spread = spread_by_state(w, DEAGLE_K);
        deagle_fire(w, spread, 0.3_f64 as f32);
    }

    /// `CDEAGLE::SecondaryAttack`: only the shield.
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        shield_secondary_fire(&*self, w, shield_anim::UP, shield_anim::DOWN);
    }

    /// `CDEAGLE::Reload`: QUIRK, `ammo_50ae <= 0` returns first; no shield animation (the
    /// Deagle's reload is the same with a shield).
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if legacy_ammo_empty(w, TAB_50AE) {
            return;
        }
        let max_clip = w.max_clip();
        if w.default_reload(max_clip, DEAGLE_RELOAD, DEAGLE_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.9;
        }
    }

    /// `CDEAGLE::WeaponIdle`. QUIRK: the shield branch is not guarded by `HasShield()` (that is
    /// `REGAMEDLL_FIXES`), so every idle of a Desert Eagle sets the timer to 20 s, plays the
    /// "shield up" animation if the shield bit is set (never, without a shield) and never plays
    /// the normal idle.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.reset_empty_sound();
        if w.weapon.time_weapon_idle <= 0.0 {
            w.weapon.time_weapon_idle = 20.0;
            if w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0 {
                w.send_weapon_anim(shield_anim::IDLE_UP, self.use_decrement());
            }
        }
    }

    fn max_speed(&self, w: &Weapon) -> f32 {
        w.max_speed
    }

    fn is_pistol(&self) -> bool {
        true
    }
}

/// `CDEAGLE::DEAGLEFire` (`wpn_deagle.cpp`, read 2026-11-22): 0.075 off the cycle time, the
/// accuracy decays by `(0.4 - dt) * 0.35` within 0.55 and 0.9, the bullet is the .50 AE, 4096
/// units, penetration 2 and a pistol shot; the idle is 1.8 s.
fn deagle_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, mut cycle_time: f32) {
    cycle_time -= 0.075;

    w.weapon.shots_fired += 1;
    if w.weapon.shots_fired > 1 {
        return;
    }

    if w.weapon.last_fire != 0.0 {
        lose_accuracy(w, 0.4, 0.35, 0.9, 0.55);
    }
    w.weapon.last_fire = w.now();

    if fire_on_empty(w) {
        return;
    }

    w.weapon.clip -= 1;
    muzzle_flash(w);
    set_player_shield_anim(w);

    w.set_animation(PlayerAnim::Attack1);
    let (forward, _, _) = make_vectors(w);

    let src = gun_position(w);
    let seed = w.player().random_seed;
    let attacker = w.edict();
    let dir = w.fire_bullets3(
        src,
        forward,
        spread,
        4096.0,
        2,
        BulletType::Player50ae,
        DEAGLE_DAMAGE as i32,
        DEAGLE_RANGE_MODIFIER,
        Some(attacker),
        true,
        seed,
    );
    let p = punch(w);
    let clip_empty = w.weapon.clip == 0;
    play_fire_event(
        w,
        "events/deagle.sc",
        dir.x,
        dir.y,
        (p.x * 100.0) as i32,
        (p.y * 100.0) as i32,
        clip_empty,
        false,
    );

    let delay = w.get_next_attack_delay(cycle_time);
    w.weapon.next_primary_attack = delay;
    w.weapon.next_secondary_attack = delay;
    // `SetSuitUpdate("!HEV_AMO0")`: no suit.
    w.weapon.time_weapon_idle = 1.8;
    punch_up(w, 2.0);
    reset_player_shield_anim(w);
}

// ---------------------------------------------------------------------------------------------
// The Five-seven
// ---------------------------------------------------------------------------------------------

/// The Five-seven (`CFiveSeven`).
#[derive(Debug, Clone, Default)]
pub struct FiveSeven;

impl WeaponBehavior for FiveSeven {
    fn id(&self) -> WeaponId {
        WeaponId::FiveSeven
    }

    /// `CFiveSeven::Precache` (`wpn_fiveseven.cpp`).
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_fiveseven.mdl", "models/w_fiveseven.mdl", "models/shield/v_shield_fiveseven.mdl"] {
            precache_model(eng, model);
        }
        for sound in [
            "weapons/fiveseven-1.wav",
            "weapons/fiveseven_clipout.wav",
            "weapons/fiveseven_clipin.wav",
            "weapons/fiveseven_sliderelease.wav",
            "weapons/fiveseven_slidepull.wav",
        ] {
            precache_sound(eng, sound);
        }
        precache_model(eng, "models/pshell.mdl");
        eng.precache_event("events/fiveseven.sc");
    }

    /// `CFiveSeven::Deploy`: the accuracy 0.92.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.accuracy = 0.92;
        w.weapon.max_speed = PISTOL_MAX_SPEED;
        w.weapon.weapon_state &= !WPNSTATE_SHIELD_DRAWN;
        w.player_mut().shield_drawn = false;
        if w.has_shield() {
            w.default_deploy(
                &*self,
                "models/shield/v_shield_fiveseven.mdl",
                "models/shield/p_shield_fiveseven.mdl",
                shield_anim::DRAW,
                "shieldgun",
                self.use_decrement(),
            )
        } else {
            w.default_deploy(&*self, "models/v_fiveseven.mdl", "models/p_fiveseven.mdl", FIVESEVEN_DRAW, "onehanded", self.use_decrement())
        }
    }

    /// `CFiveSeven::PrimaryAttack`.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let spread = spread_by_state(w, FIVESEVEN_K);
        fiveseven_fire(w, spread, 0.2_f64 as f32);
    }

    /// `CFiveSeven::SecondaryAttack`: only the shield.
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        shield_secondary_fire(&*self, w, shield_anim::UP, shield_anim::DOWN);
    }

    /// `CFiveSeven::Reload`: QUIRK, `ammo_57mm <= 0` returns first; no shield animation.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if legacy_ammo_empty(w, TAB_57MM) {
            return;
        }
        let max_clip = w.max_clip();
        if w.default_reload(max_clip, FIVESEVEN_RELOAD, FIVESEVEN_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.92;
        }
    }

    /// `CFiveSeven::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.reset_empty_sound();
        if w.weapon.time_weapon_idle > 0.0 {
            return;
        }
        if w.has_shield() {
            w.weapon.time_weapon_idle = 20.0;
            if w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN != 0 {
                w.send_weapon_anim(shield_anim::IDLE_UP, self.use_decrement());
            }
        } else if w.weapon.clip != 0 {
            w.weapon.time_weapon_idle = 3.0625;
            w.send_weapon_anim(FIVESEVEN_IDLE, self.use_decrement());
        }
    }

    fn max_speed(&self, w: &Weapon) -> f32 {
        w.max_speed
    }

    fn is_pistol(&self) -> bool {
        true
    }
}

/// `CFiveSeven::FiveSevenFire` (`wpn_fiveseven.cpp`, read 2026-11-22). QUIRKS kept: the cycle
/// time loses `0.05` as a DOUBLE literal (`flCycleTime -= 0.05`, the others use `0.05f`); the
/// accuracy decays by `(0.275 - dt) * 0.25` within 0.725 and 0.92; and the `bPistol` argument of
/// `FireBullets3` is FALSE (the only pistol that is not one for the bullet code).
fn fiveseven_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, mut cycle_time: f32) {
    cycle_time = (f64::from(cycle_time) - 0.05) as f32;

    w.weapon.shots_fired += 1;
    if w.weapon.shots_fired > 1 {
        return;
    }

    if w.weapon.last_fire != 0.0 {
        lose_accuracy(w, 0.275, 0.25, 0.92, 0.725);
    }
    w.weapon.last_fire = w.now();

    if fire_on_empty(w) {
        return;
    }

    w.weapon.clip -= 1;
    muzzle_flash(w);
    set_player_shield_anim(w);

    w.set_animation(PlayerAnim::Attack1);
    let (forward, _, _) = make_vectors(w);

    let src = gun_position(w);
    let seed = w.player().random_seed;
    let attacker = w.edict();
    let dir = w.fire_bullets3(
        src,
        forward,
        spread,
        4096.0,
        1,
        BulletType::Player57mm,
        FIVESEVEN_DAMAGE as i32,
        FIVESEVEN_RANGE_MODIFIER,
        Some(attacker),
        false,
        seed,
    );
    let p = punch(w);
    let clip_empty = w.weapon.clip == 0;
    play_fire_event(
        w,
        "events/fiveseven.sc",
        dir.x,
        dir.y,
        (p.x * 100.0) as i32,
        (p.y * 100.0) as i32,
        clip_empty,
        false,
    );

    let delay = w.get_next_attack_delay(cycle_time);
    w.weapon.next_primary_attack = delay;
    w.weapon.next_secondary_attack = delay;
    // `SetSuitUpdate("!HEV_AMO0")`: no suit.
    w.weapon.time_weapon_idle = 2.0;
    punch_up(w, 2.0);
    reset_player_shield_anim(w);
}

// ---------------------------------------------------------------------------------------------
// The dual Berettas
// ---------------------------------------------------------------------------------------------

/// The dual Berettas (`CELITE`). No shield, no secondary attack.
#[derive(Debug, Clone, Default)]
pub struct Elite;

impl WeaponBehavior for Elite {
    fn id(&self) -> WeaponId {
        WeaponId::Elite
    }

    /// `CELITE::Precache` (`wpn_elite.cpp`): two events, one per hand.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        for model in ["models/v_elite.mdl", "models/w_elite.mdl"] {
            precache_model(eng, model);
        }
        for sound in [
            "weapons/elite_fire.wav",
            "weapons/elite_reloadstart.wav",
            "weapons/elite_leftclipin.wav",
            "weapons/elite_clipout.wav",
            "weapons/elite_sliderelease.wav",
            "weapons/elite_rightclipin.wav",
            "weapons/elite_deploy.wav",
        ] {
            precache_sound(eng, sound);
        }
        precache_model(eng, "models/pshell.mdl");
        eng.precache_event("events/elite_left.sc");
        eng.precache_event("events/elite_right.sc");
    }

    /// `CELITE::Deploy`: the accuracy 0.88; an even clip starts with the left gun (an odd one
    /// leaves the hand as it was). QUIRKS: no `m_fMaxSpeed` (the speed is a constant), no
    /// shield bookkeeping.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.accuracy = 0.88;
        if w.weapon.clip & 1 == 0 {
            w.weapon.weapon_state |= WPNSTATE_ELITE_LEFT;
        }
        w.default_deploy(&*self, "models/v_elite.mdl", "models/p_elite.mdl", elite_anim::DRAW, "dualpistols", self.use_decrement())
    }

    /// `CELITE::PrimaryAttack`.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let spread = spread_by_state(w, ELITE_K);
        elite_fire(w, spread, 0.2_f64 as f32);
    }

    /// `CELITE::Reload`: QUIRK, `ammo_9mm <= 0` returns first.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if legacy_ammo_empty(w, TAB_9MM) {
            return;
        }
        let max_clip = w.max_clip();
        if w.default_reload(max_clip, elite_anim::RELOAD, ELITE_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            w.weapon.accuracy = 0.88;
        }
    }

    /// `CELITE::WeaponIdle`: the idle animation shows the empty left gun at one round left; the
    /// next idle is a minute away.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.reset_empty_sound();
        if w.weapon.time_weapon_idle <= 0.0 && w.weapon.clip != 0 {
            let anim = if w.weapon.clip == 1 { elite_anim::IDLE_LEFTEMPTY } else { elite_anim::IDLE };
            w.weapon.time_weapon_idle = 60.0;
            w.send_weapon_anim(anim, self.use_decrement());
        }
    }

    /// `GetMaxSpeed() { return ELITE_MAX_SPEED; }`.
    fn max_speed(&self, _w: &Weapon) -> f32 {
        PISTOL_MAX_SPEED
    }

    fn is_pistol(&self) -> bool {
        true
    }
}

/// `CELITE::ELITEFire` (`wpn_elite.cpp`, read 2026-11-22). QUIRKS kept: the cycle time loses
/// 0.125 (`REGAMEDLL_FIXES` makes it 0.078), so a 0.2 s cycle is 0.075 s; `flTimeDiff` is taken
/// before `m_flLastFire` is updated (and with `m_flLastFire` 0 it is the whole server time) and
/// is the event's first float; the next attack is set before the clip goes down; each hand
/// shoots from 5 units to its side along `v_right`; the left hand plays the player's
/// `ATTACK1`, the right one `ATTACK2`; the event's integers are `int(vecDir.y * 100)` and the
/// clip left. The source calls `FireBullets3(..., 8192, BULLET_PLAYER_9MM, 1, ...)` with the
/// penetration and the bullet type in swapped places: `BULLET_PLAYER_9MM` is 1, so the shot is
/// the same (penetration 1, bullet type 1, the 9 mm).
fn elite_fire(w: &mut WeaponCtx<'_, '_>, spread: f32, mut cycle_time: f32) {
    cycle_time -= 0.125;

    w.weapon.shots_fired += 1;
    if w.weapon.shots_fired > 1 {
        return;
    }

    let time_diff = w.now() - w.weapon.last_fire;

    if w.weapon.last_fire != 0.0 {
        lose_accuracy(w, 0.325, 0.275, 0.88, 0.55);
    }
    w.weapon.last_fire = w.now();

    if fire_on_empty(w) {
        return;
    }

    let delay = w.get_next_attack_delay(cycle_time);
    w.weapon.next_primary_attack = delay;
    w.weapon.next_secondary_attack = delay;

    w.weapon.clip -= 1;

    let (forward, right, _) = make_vectors(w);
    muzzle_flash(w);

    let mut src = gun_position(w);
    let seed = w.player().random_seed;
    let attacker: EdictId = w.edict();

    let left = w.weapon.weapon_state & WPNSTATE_ELITE_LEFT != 0;
    let (event, side) = if left {
        w.set_animation(PlayerAnim::Attack1);
        w.weapon.weapon_state &= !WPNSTATE_ELITE_LEFT;
        ("events/elite_left.sc", -5.0)
    } else {
        w.set_animation(PlayerAnim::Attack2);
        w.weapon.weapon_state |= WPNSTATE_ELITE_LEFT;
        ("events/elite_right.sc", 5.0)
    };
    src += right * side;
    let dir = w.fire_bullets3(
        src,
        forward,
        spread,
        8192.0,
        1,
        BulletType::Player9mm,
        ELITE_DAMAGE as i32,
        ELITE_RANGE_MODIFIER,
        Some(attacker),
        true,
        seed,
    );
    let clip = w.weapon.clip;
    play_fire_event(w, event, time_diff, dir.x, (dir.y * 100.0) as i32, clip, false, false);

    // `SetSuitUpdate("!HEV_AMO0")`: no suit.
    w.weapon.time_weapon_idle = 2.0;
    punch_up(w, 2.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four states pick the four `K`s of every pistol, in the order of the source's
    /// `if`/`else if` chain: no ground first, then a moving player (any 2D speed above 0, even
    /// when ducking), then ducking, else standing.
    #[test]
    fn spread_picks_the_k_of_the_state() {
        let tables = [
            ("glock burst", GLOCK18_BURST_K, [1.2, 0.185, 0.095, 0.3]),
            ("glock semi", GLOCK18_SEMI_K, [1.0, 0.165, 0.075, 0.1]),
            ("usp silenced", USP_SIL_K, [1.3, 0.25, 0.125, 0.15]),
            ("usp", USP_K, [1.2, 0.225, 0.08, 0.1]),
            ("p228", P228_K, [1.5, 0.255, 0.075, 0.15]),
            ("deagle", DEAGLE_K, [1.5, 0.25, 0.115, 0.13]),
            ("fiveseven", FIVESEVEN_K, [1.5, 0.255, 0.075, 0.15]),
            ("elite", ELITE_K, [1.3, 0.175, 0.08, 0.1]),
        ];
        let acc = 0.5_f32;
        let want = |k: f64| (k * 0.5) as f32;
        let run = Vec3::new(3.0, 4.0, 0.0);
        for (name, k, source) in tables {
            assert_eq!(k, source, "{name}: the literals of the source");
            // In the air, whatever else (ducking, running).
            assert_eq!(spread_of(k, FL_DUCKING, run, acc), want(k[0]), "{name} air");
            // On the ground and moving (also when ducking), ducking, standing.
            assert_eq!(spread_of(k, FL_ONGROUND, run, acc), want(k[1]), "{name} moving");
            assert_eq!(spread_of(k, FL_ONGROUND | FL_DUCKING, run, acc), want(k[1]), "{name} moving ducked");
            assert_eq!(spread_of(k, FL_ONGROUND | FL_DUCKING, Vec3::ZERO, acc), want(k[2]), "{name} ducking");
            // Only the 2D speed counts: a vertical speed is standing.
            assert_eq!(spread_of(k, FL_ONGROUND, Vec3::new(0.0, 0.0, 300.0), acc), want(k[3]), "{name} standing");
        }
        // `1 - m_flAccuracy` is a float; the product is a double cut to float.
        assert_eq!(
            spread_of(USP_K, FL_ONGROUND, Vec3::ZERO, 0.92),
            (0.1_f64 * f64::from(1.0_f32 - 0.92_f32)) as f32
        );
    }
}
