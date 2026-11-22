//! The sniper rifles: the Scout, the AWP, the G3SG1 and the SG550 (ReGameDLL_CS
//! `wpn_shared/wpn_scout.cpp`, `wpn_awp.cpp`, `wpn_g3sg1.cpp`, `wpn_sg550.cpp`, read
//! 2026-11-22), the vanilla branch of every `#ifdef` (not `REGAMEDLL_FIXES`, not
//! `REGAMEDLL_ADD`): the recoil is a direct change of `punchangle`, the zoom cycles have a
//! `case 15` and no `default`, and the reloads of the AWP, the G3SG1 and the SG550 refuse
//! without ammo.
//!
//! ## What the C code keeps on the player
//!
//! The zoom (`m_iFOV`, `pev->fov`, `m_bResumeZoom`, `m_iLastZoom`) and the late brass
//! (`m_flEjectBrass`) are fields of the player; the state machine of [`super::core`] opens the
//! scope again after a shot and ejects the brass.
//!
//! ## Deviations (each is a limit of the interface, not of the source)
//!
//! - The behaviours are unit structs (the registry builds them as values), so the event ids
//!   (`m_usFireScout`, ...) are looked up in the engine's event table at the shot
//!   ([`event_index`]) instead of being stored by `Precache`; `m_iShellId` is set on the weapon
//!   when it is deployed or fired ([`set_shell`]), because `Spawn` is not run for a carried
//!   weapon.
//! - `GetMaxSpeed` reads `m_pPlayer->m_iFOV`, which a [`WeaponBehavior::max_speed`] cannot see
//!   (it gets the weapon only). The weapon's `m_fMaxSpeed` field holds the speed for the
//!   current zoom instead ([`sync_speed`]), refreshed whenever the zoom changes and after every
//!   `ItemPostFrame` (the `item_post_frame_extra` hook), where the core resumes the zoom.
//! - Not ported, and named: `TheBots->OnEvent(...)` (no bots), `SetSuitUpdate` (the HEV suit
//!   voice), `m_iWeaponVolume`/`m_iWeaponFlash` (nothing listens: they feed monster hearing),
//!   `GetAutoaimVector` (no effect on the player).

use cs_engine::EngineApi;
use cs_engine::event_flags::FEV_NOTHOST;
use cs_sim::pm::math::angle_vectors;
use glam::Vec3;

use super::core::{DEFAULT_FOV, EF_MUZZLEFLASH, PlayerAnim, WeaponCtx};
use super::info::{BulletType, WeaponId};
use super::random::shared_random_float;
use super::weapon::{Weapon, WeaponBehavior};
use crate::player::{CHAN_ITEM, PITCH_NORM};

/// `FL_ONGROUND`, `FL_DUCKING` (`pev->flags`).
const FL_ONGROUND: u32 = cs_engine::consts::FL_ONGROUND;
const FL_DUCKING: u32 = cs_engine::consts::FL_DUCKING;

/// `SCOUT_*` (`weapons.h`).
pub const SCOUT_MAX_SPEED: f32 = 260.0;
pub const SCOUT_MAX_SPEED_ZOOM: f32 = 220.0;
pub const SCOUT_DAMAGE: f32 = 75.0;
pub const SCOUT_RANGE_MODIFER: f32 = 0.98;
pub const SCOUT_RELOAD_TIME: f32 = 2.0;
/// `scout_e`.
const SCOUT_IDLE: i32 = 0;
const SCOUT_RELOAD: i32 = 3;
const SCOUT_DRAW: i32 = 4;

/// `AWP_*`.
pub const AWP_MAX_SPEED: f32 = 210.0;
pub const AWP_MAX_SPEED_ZOOM: f32 = 150.0;
pub const AWP_DAMAGE: f32 = 115.0;
pub const AWP_RANGE_MODIFER: f32 = 0.99;
pub const AWP_RELOAD_TIME: f32 = 2.5;
/// `awp_e`.
const AWP_IDLE: i32 = 0;
const AWP_RELOAD: i32 = 4;
const AWP_DRAW: i32 = 5;

/// `G3SG1_*`.
pub const G3SG1_MAX_SPEED: f32 = 210.0;
pub const G3SG1_MAX_SPEED_ZOOM: f32 = 150.0;
pub const G3SG1_DAMAGE: f32 = 80.0;
pub const G3SG1_RANGE_MODIFER: f32 = 0.98;
pub const G3SG1_RELOAD_TIME: f32 = 3.5;
/// `g3sg1_e`.
const G3SG1_IDLE: i32 = 0;
const G3SG1_RELOAD: i32 = 3;
const G3SG1_DRAW: i32 = 4;

/// `SG550_*`.
pub const SG550_MAX_SPEED: f32 = 210.0;
pub const SG550_MAX_SPEED_ZOOM: f32 = 150.0;
pub const SG550_DAMAGE: f32 = 70.0;
pub const SG550_RANGE_MODIFER: f32 = 0.98;
pub const SG550_RELOAD_TIME: f32 = 3.35;
/// `sg550_e`.
const SG550_IDLE: i32 = 0;
const SG550_RELOAD: i32 = 3;
const SG550_DRAW: i32 = 4;

// ---------------------------------------------------------------------------------------------
// Helpers shared with the shotguns and the knife.
// ---------------------------------------------------------------------------------------------

/// `m_pPlayer->GetGunPosition()`: the eye.
pub(super) fn gun_position(w: &WeaponCtx<'_, '_>) -> Vec3 {
    w.eng.ent(w.edict()).map_or(Vec3::ZERO, |v| v.origin + v.view_ofs)
}

/// `UTIL_MakeVectors(pev->v_angle + pev->punchangle)`, then `gpGlobals->v_forward`.
pub(super) fn view_forward(w: &WeaponCtx<'_, '_>) -> Vec3 {
    let angles = w.eng.ent(w.edict()).map_or(Vec3::ZERO, |v| v.v_angle + v.punchangle);
    angle_vectors(angles).0
}

/// `pev->fov` of the player.
pub(super) fn pev_fov(w: &WeaponCtx<'_, '_>) -> f32 {
    w.eng.ent(w.edict()).map_or(0.0, |v| v.fov)
}

/// `m_pPlayer->m_iFOV = m_pPlayer->pev->fov = fov`.
fn set_zoom(w: &mut WeaponCtx<'_, '_>, fov: u8) {
    w.player_mut().fov = fov;
    set_pev_fov(w, fov);
}

/// `m_pPlayer->pev->fov = fov`.
fn set_pev_fov(w: &mut WeaponCtx<'_, '_>, fov: u8) {
    let id = w.edict();
    if let Some(v) = w.eng.ent_mut(id) {
        v.fov = f32::from(fov);
    }
}

/// `pev->effects |= EF_MUZZLEFLASH`.
pub(super) fn muzzle_flash(w: &mut WeaponCtx<'_, '_>) {
    let id = w.edict();
    if let Some(v) = w.eng.ent_mut(id) {
        v.effects |= EF_MUZZLEFLASH;
    }
}

/// `pev->punchangle`.
pub(super) fn punch(w: &WeaponCtx<'_, '_>) -> Vec3 {
    w.eng.ent(w.edict()).map_or(Vec3::ZERO, |v| v.punchangle)
}

/// Sets `pev->punchangle`.
pub(super) fn set_punch(w: &mut WeaponCtx<'_, '_>, value: Vec3) {
    let id = w.edict();
    if let Some(v) = w.eng.ent_mut(id) {
        v.punchangle = value;
    }
}

/// The index of the event script `name` as the engine's table holds it (the `m_usFire*` of the
/// C class, which `Precache` filled; see the deviations in the module notes). 0 when the event
/// was never precached, which `PLAYBACK_EVENT_FULL` ignores.
pub(super) fn event_index(w: &mut WeaponCtx<'_, '_>, name: &str) -> u16 {
    w.eng.precache_event(name)
}

/// `m_iShellId`: the model index of the brass the weapon ejects.
pub(super) fn set_shell(w: &mut WeaponCtx<'_, '_>, model: &str) {
    w.weapon.shell_id = w.eng.model_index(model).map_or(0, i32::from);
}

/// `PRECACHE_MODEL`, the index (0 when refused).
pub(super) fn precache_model(eng: &mut EngineApi<'_>, name: &str) -> i32 {
    eng.precache_model(name).map_or(0, i32::from)
}

/// `PRECACHE_SOUND`.
pub(super) fn precache_sound(eng: &mut EngineApi<'_>, name: &str) {
    let _ = eng.precache_sound(name);
}

/// `m_pPlayer->pev->velocity.Length2D()`.
fn speed_2d(w: &WeaponCtx<'_, '_>) -> f32 {
    w.eng.ent(w.edict()).map_or(0.0, |v| (v.velocity.x * v.velocity.x + v.velocity.y * v.velocity.y).sqrt())
}

/// `m_pPlayer->pev->flags`.
fn flags(w: &WeaponCtx<'_, '_>) -> u32 {
    w.eng.ent(w.edict()).map_or(0, |v| v.flags)
}

/// The speed the weapon's `GetMaxSpeed` answers for the player's current zoom, kept in the
/// weapon's `m_fMaxSpeed` (see the module notes).
fn sync_speed(w: &mut WeaponCtx<'_, '_>, normal: f32, zoomed: f32) {
    w.weapon.max_speed = if w.player().fov == DEFAULT_FOV { normal } else { zoomed };
}

/// What `GetMaxSpeed` returns: the synced field, the unzoomed speed before the first sync.
fn synced_speed(w: &Weapon, normal: f32) -> f32 {
    if w.max_speed == 0.0 { normal } else { w.max_speed }
}

/// `EMIT_SOUND(m_pPlayer->edict(), CHAN_ITEM, "weapons/zoom.wav", 0.2, 2.4)`.
fn zoom_sound(w: &mut WeaponCtx<'_, '_>) {
    let id = w.edict();
    w.eng.emit_sound(id, CHAN_ITEM, "weapons/zoom.wav", 0.2, 2.4, PITCH_NORM);
}

/// `UTIL_WeaponTimeBase() + 60`, the idle that waits for the next shot or reload: the idle
/// animation, once.
fn idle_animation(w: &mut WeaponCtx<'_, '_>, anim: i32) {
    w.weapon.time_weapon_idle = 60.0;
    w.send_weapon_anim(anim, true);
}

/// The shot that every sniper makes after the clip test: `PLAYBACK_EVENT_FULL` with the
/// weapon's own arguments, from the player's own edict.
#[allow(clippy::too_many_arguments)]
fn playback(
    w: &mut WeaponCtx<'_, '_>,
    event: u16,
    angles: Vec3,
    fparam1: f32,
    fparam2: f32,
    iparam1: i32,
    iparam2: i32,
    bparam1: bool,
) {
    let id = w.edict();
    w.eng.playback_event(
        FEV_NOTHOST, Some(id), event, 0.0, Vec3::ZERO, angles, fparam1, fparam2, iparam1, iparam2,
        bparam1, false,
    );
}

/// The empty clip of a sniper's fire function: a click when the trigger is held on an empty
/// gun (`m_fFireOnEmpty`), and nothing else. The caller returns.
fn fire_on_empty(w: &mut WeaponCtx<'_, '_>) {
    if w.weapon.fire_on_empty {
        w.play_empty_sound();
        w.weapon.next_primary_attack = w.get_next_attack_delay(0.2);
    }
}

// ---------------------------------------------------------------------------------------------
// The Scout.
// ---------------------------------------------------------------------------------------------

/// The Scout (`CSCOUT`, `wpn_scout.cpp`, read 2026-11-22).
#[derive(Debug, Clone, Default)]
pub struct Scout;

impl Scout {
    /// `CSCOUT::SCOUTFire`: the shot. QUIRKS kept: the scope closes (and the zoom is to be
    /// resumed) before the clip is tested, so a click on an empty gun closes it too; only
    /// `m_iFOV` is reset (`pev->fov` stays until the zoom resumes); there is no muzzle flash
    /// flag; the event carries the view angles and the direction times 1000.
    fn scout_fire(&mut self, w: &mut WeaponCtx<'_, '_>, mut spread: f32, cycle_time: f32) {
        if pev_fov(w) != f32::from(DEFAULT_FOV) {
            let p = w.player_mut();
            p.resume_zoom = true;
            p.last_zoom = i32::from(p.fov);
            // Reset a fov.
            p.fov = DEFAULT_FOV;
        } else {
            spread += 0.025;
        }

        if w.weapon.clip <= 0 {
            fire_on_empty(w);
            return;
        }

        w.weapon.clip -= 1;
        w.set_animation(PlayerAnim::Attack1);

        let forward = view_forward(w);
        let now = w.now();
        w.player_mut().eject_brass = now + 0.56;
        set_shell(w, "models/rshell_big.mdl");

        let src = gun_position(w);
        let id = w.edict();
        let seed = w.player().random_seed;
        let dir = w.fire_bullets3(
            src, forward, spread, 8192.0, 3, BulletType::Player762mm, SCOUT_DAMAGE as i32,
            SCOUT_RANGE_MODIFER, Some(id), true, seed,
        );

        let event = event_index(w, "events/scout.sc");
        let angles = w.eng.ent(id).map_or(Vec3::ZERO, |v| v.angles);
        let punch_x = (punch(w).x * 100.0) as i32;
        playback(w, event, angles, dir.x * 1000.0, dir.y * 1000.0, punch_x, punch_x, false);

        let next = w.get_next_attack_delay(cycle_time);
        w.weapon.next_primary_attack = next;
        w.weapon.next_secondary_attack = next;

        w.weapon.time_weapon_idle = 1.8;
        // Vanilla recoil (`REGAMEDLL_ADD` calls `KickBack`).
        let mut p = punch(w);
        p.x -= 2.0;
        set_punch(w, p);
    }
}

impl WeaponBehavior for Scout {
    fn id(&self) -> WeaponId {
        WeaponId::Scout
    }

    /// `CSCOUT::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        precache_model(eng, "models/v_scout.mdl");
        precache_model(eng, "models/w_scout.mdl");
        for s in [
            "weapons/scout_fire-1.wav",
            "weapons/scout_bolt.wav",
            "weapons/scout_clipin.wav",
            "weapons/scout_clipout.wav",
            "weapons/zoom.wav",
        ] {
            precache_sound(eng, s);
        }
        precache_model(eng, "models/rshell_big.mdl");
        eng.precache_event("events/scout.sc");
    }

    /// `CSCOUT::Deploy`: the draw, then both the weapon and the player wait 1.25 s (the player
    /// is set to the same value, `m_flNextPrimaryAttack = m_pPlayer->m_flNextAttack = ...`).
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        set_shell(w, "models/rshell_big.mdl");
        let deployed = w.default_deploy(
            &*self, "models/v_scout.mdl", "models/p_scout.mdl", SCOUT_DRAW, "rifle", true,
        );
        sync_speed(w, SCOUT_MAX_SPEED, SCOUT_MAX_SPEED_ZOOM);
        if deployed {
            let next = w.get_next_attack_delay(1.25);
            w.weapon.next_primary_attack = next;
            w.player_mut().next_attack = next;
            w.weapon.next_secondary_attack = 1.0;
            return true;
        }
        false
    }

    /// `CSCOUT::SecondaryAttack`: 90, 40, 15, 90 (vanilla has `case 15`, no `default`: any
    /// other FOV is left alone), the speed, the zoom sound, 0.3 s to the next zoom.
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        match w.player().fov {
            90 => set_zoom(w, 40),
            40 => set_zoom(w, 15),
            15 => set_zoom(w, 90),
            _ => {}
        }
        sync_speed(w, SCOUT_MAX_SPEED, SCOUT_MAX_SPEED_ZOOM);
        w.reset_max_speed(&*self);
        zoom_sound(w);
        w.weapon.next_secondary_attack = 0.3;
    }

    /// `CSCOUT::PrimaryAttack`: spread by the stance, tested in the source's order.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if flags(w) & FL_ONGROUND == 0 {
            self.scout_fire(w, 0.2, 1.25);
        } else if speed_2d(w) > 170.0 {
            self.scout_fire(w, 0.075, 1.25);
        } else if flags(w) & FL_DUCKING != 0 {
            self.scout_fire(w, 0.0, 1.25);
        } else {
            self.scout_fire(w, 0.007, 1.25);
        }
    }

    /// `CSCOUT::Reload`: no ammo test (the AWP and the others have one). A reload in the
    /// scope drops the zoom: the FOV is set to 15 and the secondary attack run, which closes it.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let max = w.max_clip();
        if w.default_reload(max, SCOUT_RELOAD, SCOUT_RELOAD_TIME) {
            if pev_fov(w) != f32::from(DEFAULT_FOV) {
                set_zoom(w, 15);
                self.secondary_attack(w);
            }
            w.set_animation(PlayerAnim::Reload);
        }
    }

    /// `CSCOUT::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.reset_empty_sound();
        if w.weapon.time_weapon_idle > 0.0 {
            return;
        }
        if w.weapon.clip != 0 {
            idle_animation(w, SCOUT_IDLE);
        }
    }

    fn max_speed(&self, w: &Weapon) -> f32 {
        synced_speed(w, SCOUT_MAX_SPEED)
    }

    fn item_post_frame_extra(&mut self, w: &mut WeaponCtx<'_, '_>) {
        sync_speed(w, SCOUT_MAX_SPEED, SCOUT_MAX_SPEED_ZOOM);
    }
}

// ---------------------------------------------------------------------------------------------
// The AWP.
// ---------------------------------------------------------------------------------------------

/// The AWP (`CAWP`, `wpn_awp.cpp`, read 2026-11-22).
#[derive(Debug, Clone, Default)]
pub struct Awp;

impl Awp {
    /// `CAWP::AWPFire`. QUIRKS kept: the scope closes before the clip is tested (both `m_iFOV`
    /// and `pev->fov` this time); an unscoped shot adds 0.08 to the spread; the event has no
    /// angles and carries the raw direction.
    fn awp_fire(&mut self, w: &mut WeaponCtx<'_, '_>, mut spread: f32, cycle_time: f32) {
        if pev_fov(w) != f32::from(DEFAULT_FOV) {
            let p = w.player_mut();
            p.resume_zoom = true;
            p.last_zoom = i32::from(p.fov);
            // Reset a fov.
            p.fov = DEFAULT_FOV;
            set_pev_fov(w, DEFAULT_FOV);
        } else {
            // If we are not zoomed in, the bullet diverts more.
            spread += 0.08;
        }

        if w.weapon.clip <= 0 {
            fire_on_empty(w);
            return;
        }

        w.weapon.clip -= 1;
        muzzle_flash(w);
        w.set_animation(PlayerAnim::Attack1);

        let forward = view_forward(w);
        let now = w.now();
        w.player_mut().eject_brass = now + 0.55;
        set_shell(w, "models/rshell_big.mdl");

        let src = gun_position(w);
        let id = w.edict();
        let seed = w.player().random_seed;
        let dir = w.fire_bullets3(
            src, forward, spread, 8192.0, 3, BulletType::Player338mag, AWP_DAMAGE as i32,
            AWP_RANGE_MODIFER, Some(id), true, seed,
        );

        let event = event_index(w, "events/awp.sc");
        let punch_x = (punch(w).x * 100.0) as i32;
        playback(w, event, Vec3::ZERO, dir.x, dir.y, punch_x, punch_x, false);

        let next = w.get_next_attack_delay(cycle_time);
        w.weapon.next_primary_attack = next;
        w.weapon.next_secondary_attack = next;

        w.weapon.time_weapon_idle = 2.0;
        let mut p = punch(w);
        p.x -= 2.0;
        set_punch(w, p);
    }
}

impl WeaponBehavior for Awp {
    fn id(&self) -> WeaponId {
        WeaponId::Awp
    }

    /// `CAWP::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        precache_model(eng, "models/v_awp.mdl");
        precache_model(eng, "models/w_awp.mdl");
        for s in [
            "weapons/awp1.wav",
            "weapons/boltpull1.wav",
            "weapons/boltup.wav",
            "weapons/boltdown.wav",
            "weapons/zoom.wav",
            "weapons/awp_deploy.wav",
            "weapons/awp_clipin.wav",
            "weapons/awp_clipout.wav",
        ] {
            precache_sound(eng, s);
        }
        precache_model(eng, "models/rshell_big.mdl");
        eng.precache_event("events/awp.sc");
    }

    /// `CAWP::Deploy`: the player waits 1.45 s and so does the weapon.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        set_shell(w, "models/rshell_big.mdl");
        let deployed =
            w.default_deploy(&*self, "models/v_awp.mdl", "models/p_awp.mdl", AWP_DRAW, "rifle", true);
        sync_speed(w, AWP_MAX_SPEED, AWP_MAX_SPEED_ZOOM);
        if deployed {
            let next = w.get_next_attack_delay(1.45);
            w.player_mut().next_attack = next;
            w.weapon.next_primary_attack = next;
            w.weapon.next_secondary_attack = 1.0;
            return true;
        }
        false
    }

    /// `CAWP::SecondaryAttack`: 90, 40, 10, and `default` goes back to 90.
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        match w.player().fov {
            90 => set_zoom(w, 40),
            40 => set_zoom(w, 10),
            _ => set_zoom(w, 90),
        }
        sync_speed(w, AWP_MAX_SPEED, AWP_MAX_SPEED_ZOOM);
        w.reset_max_speed(&*self);
        zoom_sound(w);
        w.weapon.next_secondary_attack = 0.3;
    }

    /// `CAWP::PrimaryAttack`.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if flags(w) & FL_ONGROUND == 0 {
            self.awp_fire(w, 0.85, 1.45);
        } else if speed_2d(w) > 140.0 {
            self.awp_fire(w, 0.25, 1.45);
        } else if speed_2d(w) > 10.0 {
            self.awp_fire(w, 0.1, 1.45);
        } else if flags(w) & FL_DUCKING != 0 {
            self.awp_fire(w, 0.0, 1.45);
        } else {
            self.awp_fire(w, 0.001, 1.45);
        }
    }

    /// `CAWP::Reload`: vanilla refuses when the legacy `ammo_338mag` counter is empty (the
    /// `#ifndef REGAMEDLL_FIXES` test). The zoom is dropped as the Scout's, through 10.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.player().tabulated_ammo[TAB_338MAG] <= 0 {
            return;
        }
        let max = w.max_clip();
        if w.default_reload(max, AWP_RELOAD, AWP_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            if pev_fov(w) != f32::from(DEFAULT_FOV) {
                set_zoom(w, 10);
                self.secondary_attack(w);
            }
        }
    }

    /// `CAWP::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.reset_empty_sound();
        if w.weapon.time_weapon_idle <= 0.0 && w.weapon.clip != 0 {
            idle_animation(w, AWP_IDLE);
        }
    }

    fn max_speed(&self, w: &Weapon) -> f32 {
        synced_speed(w, AWP_MAX_SPEED)
    }

    fn item_post_frame_extra(&mut self, w: &mut WeaponCtx<'_, '_>) {
        sync_speed(w, AWP_MAX_SPEED, AWP_MAX_SPEED_ZOOM);
    }
}

/// The index of `ammo_338mag` in [`crate::player::Player::tabulated_ammo`] (`TABULATED_AMMO`).
const TAB_338MAG: usize = 7;
/// `ammo_762nato`.
const TAB_762NATO: usize = 4;

// ---------------------------------------------------------------------------------------------
// The G3SG1.
// ---------------------------------------------------------------------------------------------

/// The G3SG1 (`CG3SG1`, `wpn_g3sg1.cpp`, read 2026-11-22).
#[derive(Debug, Clone, Default)]
pub struct G3sg1;

impl G3sg1 {
    /// `CG3SG1::G3SG1Fire`. QUIRKS kept: the scope is not closed by a shot (this gun only
    /// adds 0.025 to the spread unscoped); the accuracy grows with the time since the last shot
    /// and is updated even by a click on an empty gun; the spread is `(1 - accuracy) * spread`;
    /// the recoil is the vanilla direct change of `punchangle` (x by a shared random 0.75 to
    /// 1.75 plus a quarter of the current punch, y by a shared random -0.75 to 0.75).
    fn g3sg1_fire(&mut self, w: &mut WeaponCtx<'_, '_>, mut spread: f32, cycle_time: f32) {
        if pev_fov(w) == f32::from(DEFAULT_FOV) {
            spread += 0.025;
        }

        let now = w.now();
        if w.weapon.last_fire != 0.0 {
            w.weapon.accuracy = (now - w.weapon.last_fire) * 0.3 + 0.55;
            if w.weapon.accuracy > 0.98 {
                w.weapon.accuracy = 0.98;
            }
        } else {
            w.weapon.accuracy = 0.98;
        }
        w.weapon.last_fire = now;

        if w.weapon.clip <= 0 {
            fire_on_empty(w);
            return;
        }

        w.weapon.clip -= 1;
        muzzle_flash(w);
        w.set_animation(PlayerAnim::Attack1);

        let forward = view_forward(w);
        let src = gun_position(w);
        let id = w.edict();
        let seed = w.player().random_seed;
        let accuracy = w.weapon.accuracy;
        let dir = w.fire_bullets3(
            src, forward, (1.0 - accuracy) * spread, 8192.0, 3, BulletType::Player762mm,
            G3SG1_DAMAGE as i32, G3SG1_RANGE_MODIFER, Some(id), true, seed,
        );

        let event = event_index(w, "events/g3sg1.sc");
        let punch_x = (punch(w).x * 100.0) as i32;
        playback(w, event, Vec3::ZERO, dir.x, dir.y, punch_x, punch_x, false);

        let next = w.get_next_attack_delay(cycle_time);
        w.weapon.next_primary_attack = next;
        w.weapon.next_secondary_attack = next;

        w.weapon.time_weapon_idle = 1.8;
        let mut p = punch(w);
        p.x -= shared_random_float(seed.wrapping_add(4), 0.75, 1.75) + p.x * 0.25;
        p.y += shared_random_float(seed.wrapping_add(5), -0.75, 0.75);
        set_punch(w, p);
    }
}

impl WeaponBehavior for G3sg1 {
    fn id(&self) -> WeaponId {
        WeaponId::G3sg1
    }

    /// `CG3SG1::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        precache_model(eng, "models/v_g3sg1.mdl");
        precache_model(eng, "models/w_g3sg1.mdl");
        for s in [
            "weapons/g3sg1-1.wav",
            "weapons/g3sg1_slide.wav",
            "weapons/g3sg1_clipin.wav",
            "weapons/g3sg1_clipout.wav",
            "weapons/zoom.wav",
        ] {
            precache_sound(eng, s);
        }
        precache_model(eng, "models/rshell.mdl");
        eng.precache_event("events/g3sg1.sc");
    }

    /// `CG3SG1::Deploy`: the accuracy starts at 0.2; the deploy times are the default's.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.weapon.accuracy = 0.2;
        set_shell(w, "models/rshell.mdl");
        let deployed = w.default_deploy(
            &*self, "models/v_g3sg1.mdl", "models/p_g3sg1.mdl", G3SG1_DRAW, "mp5", true,
        );
        sync_speed(w, G3SG1_MAX_SPEED, G3SG1_MAX_SPEED_ZOOM);
        deployed
    }

    /// `CG3SG1::SecondaryAttack`: 90, 40, 15, 90 (`case 15`). `ResetMaxSpeed` comes before the
    /// sound here.
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        match w.player().fov {
            90 => set_zoom(w, 40),
            40 => set_zoom(w, 15),
            15 => set_zoom(w, 90),
            _ => {}
        }
        sync_speed(w, G3SG1_MAX_SPEED, G3SG1_MAX_SPEED_ZOOM);
        w.reset_max_speed(&*self);
        zoom_sound(w);
        w.weapon.next_secondary_attack = 0.3;
    }

    /// `CG3SG1::PrimaryAttack`: any movement at all (`Length2D() > 0`) is the moving spread.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if flags(w) & FL_ONGROUND == 0 {
            self.g3sg1_fire(w, 0.45, 0.25);
        } else if speed_2d(w) > 0.0 {
            self.g3sg1_fire(w, 0.15, 0.25);
        } else if flags(w) & FL_DUCKING != 0 {
            self.g3sg1_fire(w, 0.035, 0.25);
        } else {
            self.g3sg1_fire(w, 0.055, 0.25);
        }
    }

    /// `CG3SG1::Reload`: refuses without `ammo_762nato`; a reload resets the accuracy to 0.2.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.player().tabulated_ammo[TAB_762NATO] <= 0 {
            return;
        }
        let max = w.max_clip();
        if w.default_reload(max, G3SG1_RELOAD, G3SG1_RELOAD_TIME) {
            w.weapon.accuracy = 0.2;
            w.set_animation(PlayerAnim::Reload);
            if pev_fov(w) != f32::from(DEFAULT_FOV) {
                set_zoom(w, 15);
                self.secondary_attack(w);
            }
        }
    }

    /// `CG3SG1::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.reset_empty_sound();
        if w.weapon.time_weapon_idle <= 0.0 && w.weapon.clip != 0 {
            idle_animation(w, G3SG1_IDLE);
        }
    }

    fn max_speed(&self, w: &Weapon) -> f32 {
        synced_speed(w, G3SG1_MAX_SPEED)
    }

    fn item_post_frame_extra(&mut self, w: &mut WeaponCtx<'_, '_>) {
        sync_speed(w, G3SG1_MAX_SPEED, G3SG1_MAX_SPEED_ZOOM);
    }
}

// ---------------------------------------------------------------------------------------------
// The SG550.
// ---------------------------------------------------------------------------------------------

/// The SG550 (`CSG550`, `wpn_sg550.cpp`, read 2026-11-22).
#[derive(Debug, Clone, Default)]
pub struct Sg550;

impl Sg550 {
    /// `CSG550::SG550Fire`. QUIRKS kept: the accuracy is only updated when there was a last
    /// shot (no `else`: vanilla leaves it as it was, 0 on a fresh gun); the deploy does not set
    /// it (`REGAMEDLL_FIXES` does); the event's `bparam1` is the int 5 (a bool on the wire);
    /// the recoil is as the G3SG1's but the shared random is 0.75 to 1.25, and the quarter is
    /// the `double` literal `0.25`, so the subtraction is made in `double`.
    fn sg550_fire(&mut self, w: &mut WeaponCtx<'_, '_>, mut spread: f32, cycle_time: f32) {
        if pev_fov(w) == f32::from(DEFAULT_FOV) {
            spread += 0.025;
        }

        let now = w.now();
        if w.weapon.last_fire != 0.0 {
            w.weapon.accuracy = (now - w.weapon.last_fire) * 0.35 + 0.65;
            if w.weapon.accuracy > 0.98 {
                w.weapon.accuracy = 0.98;
            }
        }
        w.weapon.last_fire = now;

        if w.weapon.clip <= 0 {
            fire_on_empty(w);
            return;
        }

        w.weapon.clip -= 1;
        muzzle_flash(w);
        w.set_animation(PlayerAnim::Attack1);

        let forward = view_forward(w);
        let src = gun_position(w);
        let id = w.edict();
        let seed = w.player().random_seed;
        let dir = w.fire_bullets3(
            src, forward, spread, 8192.0, 2, BulletType::Player556mm, SG550_DAMAGE as i32,
            SG550_RANGE_MODIFER, Some(id), true, seed,
        );

        let event = event_index(w, "events/sg550.sc");
        let punch_x = (punch(w).x * 100.0) as i32;
        playback(w, event, Vec3::ZERO, dir.x, dir.y, punch_x, punch_x, true);

        let next = w.get_next_attack_delay(cycle_time);
        w.weapon.next_primary_attack = next;
        w.weapon.next_secondary_attack = next;

        w.weapon.time_weapon_idle = 1.8;
        let mut p = punch(w);
        let random = f64::from(shared_random_float(seed.wrapping_add(4), 0.75, 1.25));
        p.x = (f64::from(p.x) - (random + f64::from(p.x) * 0.25)) as f32;
        p.y += shared_random_float(seed.wrapping_add(5), -0.75, 0.75);
        set_punch(w, p);
    }
}

impl WeaponBehavior for Sg550 {
    fn id(&self) -> WeaponId {
        WeaponId::Sg550
    }

    /// `CSG550::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        precache_model(eng, "models/v_sg550.mdl");
        precache_model(eng, "models/w_sg550.mdl");
        for s in [
            "weapons/sg550-1.wav",
            "weapons/sg550_boltpull.wav",
            "weapons/sg550_clipin.wav",
            "weapons/sg550_clipout.wav",
            "weapons/zoom.wav",
        ] {
            precache_sound(eng, s);
        }
        precache_model(eng, "models/rshell.mdl");
        eng.precache_event("events/sg550.sc");
    }

    /// `CSG550::Deploy`.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        set_shell(w, "models/rshell.mdl");
        let deployed = w.default_deploy(
            &*self, "models/v_sg550.mdl", "models/p_sg550.mdl", SG550_DRAW, "rifle", true,
        );
        sync_speed(w, SG550_MAX_SPEED, SG550_MAX_SPEED_ZOOM);
        deployed
    }

    /// `CSG550::SecondaryAttack`.
    fn secondary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        match w.player().fov {
            90 => set_zoom(w, 40),
            40 => set_zoom(w, 15),
            15 => set_zoom(w, 90),
            _ => {}
        }
        sync_speed(w, SG550_MAX_SPEED, SG550_MAX_SPEED_ZOOM);
        w.reset_max_speed(&*self);
        zoom_sound(w);
        w.weapon.next_secondary_attack = 0.3;
    }

    /// `CSG550::PrimaryAttack`: the spread constants are `double`s (`0.45 * (1 - accuracy)`),
    /// cut to the `float` parameter.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        let one_minus = f64::from(1.0_f32 - w.weapon.accuracy);
        if flags(w) & FL_ONGROUND == 0 {
            self.sg550_fire(w, (0.45 * one_minus) as f32, 0.25);
        } else if speed_2d(w) > 0.0 {
            self.sg550_fire(w, 0.15, 0.25);
        } else if flags(w) & FL_DUCKING != 0 {
            self.sg550_fire(w, (0.04 * one_minus) as f32, 0.25);
        } else {
            self.sg550_fire(w, (0.05 * one_minus) as f32, 0.25);
        }
    }

    /// `CSG550::Reload`: refuses without `ammo_556nato`.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.player().tabulated_ammo[super::core::TAB_556NATO] <= 0 {
            return;
        }
        let max = w.max_clip();
        if w.default_reload(max, SG550_RELOAD, SG550_RELOAD_TIME) {
            w.set_animation(PlayerAnim::Reload);
            if pev_fov(w) != f32::from(DEFAULT_FOV) {
                set_zoom(w, 15);
                self.secondary_attack(w);
            }
        }
    }

    /// `CSG550::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.reset_empty_sound();
        if w.weapon.time_weapon_idle > 0.0 {
            return;
        }
        if w.weapon.clip != 0 {
            idle_animation(w, SG550_IDLE);
        }
    }

    fn max_speed(&self, w: &Weapon) -> f32 {
        synced_speed(w, SG550_MAX_SPEED)
    }

    fn item_post_frame_extra(&mut self, w: &mut WeaponCtx<'_, '_>) {
        sync_speed(w, SG550_MAX_SPEED, SG550_MAX_SPEED_ZOOM);
    }
}

/// What [`crate::CsGame::test_weapon_call`] runs on a weapon.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestCall {
    Primary,
    Secondary,
    Reload,
}

/// Entry points for the tests (cargo feature `test-hooks`): the weapon's own methods run
/// directly, without the movement of a command (which would clear `FL_ONGROUND` on a map with
/// no floor), with the player's command seed set.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
impl crate::CsGame {
    /// Runs `call` on weapon `id` of the player of `slot` with `m_random_seed = seed`, and
    /// returns what `Swing`/`Stab` return for the knife (false for every other call).
    pub fn test_weapon_call(
        &mut self,
        eng: &mut EngineApi<'_>,
        slot: u8,
        id: WeaponId,
        call: TestCall,
        seed: u32,
    ) -> bool {
        if let Some(p) = self.state.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
            p.random_seed = seed;
        }
        super::core::with_weapon(&mut self.state, eng, slot, id, |b, w| match call {
            TestCall::Primary if id == WeaponId::Knife => super::knife::Knife.swing(w, true),
            TestCall::Secondary if id == WeaponId::Knife => super::knife::Knife.stab(w, true),
            TestCall::Primary => {
                b.primary_attack(w);
                false
            }
            TestCall::Secondary => {
                b.secondary_attack(w);
                false
            }
            TestCall::Reload => {
                b.reload(w);
                false
            }
        })
        .unwrap_or(false)
    }

    /// The game's random generator state, to compare how many numbers a call drew.
    pub fn test_rng(&self) -> u32 {
        self.state.rng
    }

    /// Sets the game's random generator state.
    pub fn test_set_rng(&mut self, state: u32) {
        self.state.rng = state;
    }

    /// `RANDOM_LONG(lo, hi)` of the game's generator.
    pub fn test_random_long(&mut self, lo: i32, hi: i32) -> i32 {
        self.state.random_range(lo, hi)
    }
}
