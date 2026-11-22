//! The shotguns: the M3 and the XM1014 (ReGameDLL_CS `wpn_shared/wpn_m3.cpp` and
//! `wpn_xm1014.cpp`, read 2026-11-22), the vanilla branch (`FireBullets`, not
//! `FireBuckshots`; `m_flPumpTime` kept; the recoil a direct change of `punchangle.x`).
//!
//! The cones are the source's own constants (`M3_CONE_VECTOR` and `XM1014_CONE_VECTOR`, the
//! first lines of each file), not the survey's.
//!
//! Not ported, and named: `TheBots->OnEvent(...)`, `SetSuitUpdate`, `m_iWeaponVolume`,
//! `m_iWeaponFlash`, `GetAutoaimVector`. As for the other guns, the event id is looked up in
//! the engine's table at the shot and `m_iShellId` is set at the deploy (see
//! [`super::snipers`]).
//!
//! `CM3::PlayEmptySound` and `CXM1014::PlayEmptySound` are virtual overrides that clear
//! `m_iPlayEmptySound` after the base click: both weapons implement
//! [`WeaponBehavior::play_empty_sound`], which the underwater click of
//! [`super::core::item_post_frame`] calls as well.

use cs_engine::EngineApi;
use cs_engine::event_flags::FEV_NOTHOST;
use glam::Vec3;

use super::core::{
    PlayerAnim, WeaponCtx, gun_position, muzzle_flash, precache_model, precache_sound, punch,
};
use super::info::{BulletType, WeaponId};
use super::random::shared_random_long;
use super::snipers::{event_index, set_punch, set_shell, view_forward};
use super::weapon::WeaponBehavior;

/// `FL_ONGROUND`.
const FL_ONGROUND: u32 = cs_engine::consts::FL_ONGROUND;

/// `M3_CONE_VECTOR` (`wpn_m3.cpp`): "special shotgun spreads".
pub const M3_CONE_VECTOR: Vec3 = Vec3::new(0.0675, 0.0675, 0.0);
/// `XM1014_CONE_VECTOR` (`wpn_xm1014.cpp`).
pub const XM1014_CONE_VECTOR: Vec3 = Vec3::new(0.0725, 0.0725, 0.0);
/// `M3_MAX_SPEED`, `M3_DAMAGE` (`weapons.h`; the damage is unused by the vanilla `FireBullets`
/// call, which passes 0).
pub const M3_MAX_SPEED: f32 = 230.0;
pub const M3_DAMAGE: f32 = 20.0;
/// `XM1014_MAX_SPEED`, `XM1014_DAMAGE`.
pub const XM1014_MAX_SPEED: f32 = 240.0;
pub const XM1014_DAMAGE: f32 = 20.0;

/// `m3_e`.
const M3_IDLE: i32 = 0;
const M3_RELOAD: i32 = 3;
const M3_PUMP: i32 = 4;
const M3_START_RELOAD: i32 = 5;
const M3_DRAW: i32 = 6;
/// `xm1014_e`.
const XM1014_IDLE: i32 = 0;
const XM1014_RELOAD: i32 = 3;
const XM1014_PUMP: i32 = 4;
const XM1014_START_RELOAD: i32 = 5;
const XM1014_DRAW: i32 = 6;

/// `CM3::PlayEmptySound` and `CXM1014::PlayEmptySound` (`wpn_m3.cpp` 65-70, `wpn_xm1014.cpp`
/// 63-68, read 2026-11-22): the base click, then `m_iPlayEmptySound = 0` (so the click is not
/// repeated until the idle allows it again).
fn shotgun_empty_sound(w: &mut WeaponCtx<'_, '_>) -> bool {
    let result = w.play_empty_sound();
    w.weapon.play_empty_sound = false;
    result
}

/// The empty-clip branch of both `PrimaryAttack`s (vanilla, not `BUILD_LATEST_FIXES`): start
/// the reload, click if the reload loaded nothing, wait a second.
fn empty_primary(me: &mut dyn WeaponBehavior, w: &mut WeaponCtx<'_, '_>) {
    me.reload(w);
    if w.weapon.clip == 0 {
        me.play_empty_sound(w);
    }
    w.weapon.next_primary_attack = w.get_next_attack_delay(1.0);
}

/// The idle of both shotguns (`CM3::WeaponIdle`, `CXM1014::WeaponIdle`, vanilla): the pump
/// time runs out; a gun that is empty reloads, one in the middle of a reload continues it (or
/// pumps when it is full or out of ammo), else the idle animation. QUIRK: with the timer at
/// exactly 0 nothing happens (the test is strictly below), and the idle animation is not
/// followed by a longer wait (`REGAMEDLL_FIXES` adds 20 s), so it plays at every frame the
/// timer is below 0.
fn shotgun_idle(me: &mut dyn WeaponBehavior, w: &mut WeaponCtx<'_, '_>, idle: i32, pump: i32) {
    w.reset_empty_sound();
    if w.weapon.pump_time != 0.0 && w.weapon.pump_time < 0.0 {
        w.weapon.pump_time = 0.0;
    }
    if w.weapon.time_weapon_idle < 0.0 {
        let reserve = w.ammo(w.weapon.primary_ammo_type);
        if w.weapon.clip == 0 && w.weapon.in_special_reload == 0 && reserve != 0 {
            me.reload(w);
        } else if w.weapon.in_special_reload != 0 {
            if w.weapon.clip != w.max_clip() && reserve != 0 {
                me.reload(w);
            } else {
                // Reload debounce has timed out.
                w.send_weapon_anim(pump, true);
                w.weapon.in_special_reload = 0;
                w.weapon.time_weapon_idle = 1.5;
            }
        } else {
            w.send_weapon_anim(idle, true);
        }
    }
}

/// The recoil of both (vanilla, `REGAMEDLL_ADD` calls `KickBack`): `punchangle.x` goes down by
/// a shared random integer, more in the air. `SharedRandomLong(seed + 1, lo, hi)`.
fn recoil(w: &mut WeaponCtx<'_, '_>, ground: (i32, i32), air: (i32, i32)) {
    let seed = w.player().random_seed.wrapping_add(1);
    let on_ground = w.eng.ent(w.edict()).is_some_and(|v| v.flags & FL_ONGROUND != 0);
    let (lo, hi) = if on_ground { ground } else { air };
    let mut p = punch(w);
    p.x -= shared_random_long(seed, lo, hi) as f32;
    set_punch(w, p);
}

// ---------------------------------------------------------------------------------------------
// The M3.
// ---------------------------------------------------------------------------------------------

/// The M3 (`CM3`, `wpn_m3.cpp`, read 2026-11-22).
#[derive(Debug, Clone, Default)]
pub struct M3;

impl WeaponBehavior for M3 {
    fn id(&self) -> WeaponId {
        WeaponId::M3
    }

    fn play_empty_sound(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        shotgun_empty_sound(w)
    }

    /// `CM3::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        precache_model(eng, "models/v_m3.mdl");
        precache_model(eng, "models/w_m3.mdl");
        precache_model(eng, "models/shotgunshell.mdl");
        for s in [
            "weapons/m3-1.wav",
            "weapons/m3_insertshell.wav",
            "weapons/m3_pump.wav",
            "weapons/reload1.wav",
            "weapons/reload3.wav",
        ] {
            precache_sound(eng, s);
        }
        eng.precache_event("events/m3.sc");
    }

    /// `CM3::Deploy`.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        set_shell(w, "models/shotgunshell.mdl");
        w.default_deploy(&*self, "models/v_m3.mdl", "models/p_m3.mdl", M3_DRAW, "shotgun", true)
    }

    /// `CM3::PrimaryAttack` (vanilla): nine pellets of the cone, range 3000, damage 0 (the
    /// pellet damage comes from the distance); the event carries nothing; the pump time is set
    /// when rounds are left; 0.875 s to the next shot; the idle 2.5 s, or 0.875 s when the clip
    /// is now empty; the special reload ends. The brass leaves 0.45 s late.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.weapon.clip <= 0 {
            empty_primary(self, w);
            return;
        }

        w.weapon.clip -= 1;
        muzzle_flash(w);
        // Player "shoot" animation.
        w.set_animation(PlayerAnim::Attack1);

        let forward = view_forward(w);
        let src = gun_position(w);
        w.fire_bullets(
            9, src, forward, M3_CONE_VECTOR, 3000.0, BulletType::PlayerBuckshot, 0, 0, None,
        );

        let event = event_index(w, "events/m3.sc");
        let id = w.edict();
        w.eng.playback_event(
            FEV_NOTHOST, Some(id), event, 0.0, Vec3::ZERO, Vec3::ZERO, 0.0, 0.0, 0, 0, false,
            false,
        );

        if w.weapon.clip != 0 {
            w.weapon.pump_time = 0.5;
        }
        w.weapon.next_primary_attack = w.get_next_attack_delay(0.875);
        w.weapon.next_secondary_attack = 0.875;
        w.weapon.time_weapon_idle = if w.weapon.clip != 0 { 2.5 } else { 0.875 };
        w.weapon.in_special_reload = 0;

        recoil(w, (4, 6), (8, 11));
        let now = w.now();
        w.player_mut().eject_brass = now + 0.45;
    }

    /// `CM3::Reload`: the shotgun reload without sounds of its own.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.default_shotgun_reload(M3_RELOAD, M3_START_RELOAD, 0.45, 0.55, None, None);
    }

    /// `CM3::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        shotgun_idle(self, w, M3_IDLE, M3_PUMP);
    }

    fn max_speed(&self, _w: &super::weapon::Weapon) -> f32 {
        M3_MAX_SPEED
    }
}

// ---------------------------------------------------------------------------------------------
// The XM1014.
// ---------------------------------------------------------------------------------------------

/// The XM1014 (`CXM1014`, `wpn_xm1014.cpp`, read 2026-11-22).
#[derive(Debug, Clone, Default)]
pub struct Xm1014;

impl WeaponBehavior for Xm1014 {
    fn id(&self) -> WeaponId {
        WeaponId::Xm1014
    }

    fn play_empty_sound(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        shotgun_empty_sound(w)
    }

    /// `CXM1014::Precache`.
    fn precache(&mut self, eng: &mut EngineApi<'_>) {
        precache_model(eng, "models/v_xm1014.mdl");
        precache_model(eng, "models/w_xm1014.mdl");
        precache_model(eng, "models/shotgunshell.mdl");
        for s in ["weapons/xm1014-1.wav", "weapons/reload1.wav", "weapons/reload3.wav"] {
            precache_sound(eng, s);
        }
        eng.precache_event("events/xm1014.sc");
    }

    /// `CXM1014::Deploy`.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        set_shell(w, "models/shotgunshell.mdl");
        w.default_deploy(
            &*self, "models/v_xm1014.mdl", "models/p_xm1014.mdl", XM1014_DRAW, "m249", true,
        )
    }

    /// `CXM1014::PrimaryAttack` (vanilla): the round is taken after the volume (the M3 takes it
    /// first; no observable difference), six pellets of the cone, range 3048; the event carries
    /// `m_vVecAiming` (never set by this weapon: zero), 7, `int(m_vVecAiming.x * 100)` and "the
    /// clip is empty"; 0.25 s to the next shot; the idle 2.25 s, or 0.75 s when empty.
    fn primary_attack(&mut self, w: &mut WeaponCtx<'_, '_>) {
        if w.weapon.clip <= 0 {
            empty_primary(self, w);
            return;
        }

        w.weapon.clip -= 1;
        muzzle_flash(w);
        // Player "shoot" animation.
        w.set_animation(PlayerAnim::Attack1);

        let forward = view_forward(w);
        let src = gun_position(w);
        w.fire_bullets(
            6, src, forward, XM1014_CONE_VECTOR, 3048.0, BulletType::PlayerBuckshot, 0, 0, None,
        );

        let event = event_index(w, "events/xm1014.sc");
        let id = w.edict();
        let aiming = w.weapon.vec_aiming;
        let clip_empty = w.weapon.clip == 0;
        w.eng.playback_event(
            FEV_NOTHOST, Some(id), event, 0.0, Vec3::ZERO, Vec3::ZERO, aiming.x, aiming.y, 7,
            (aiming.x * 100.0) as i32, clip_empty, false,
        );

        if w.weapon.clip != 0 {
            w.weapon.pump_time = 0.125;
        }
        w.weapon.next_primary_attack = w.get_next_attack_delay(0.25);
        w.weapon.next_secondary_attack = 0.25;
        w.weapon.time_weapon_idle = if w.weapon.clip != 0 { 2.25 } else { 0.75 };
        w.weapon.in_special_reload = 0;

        recoil(w, (3, 5), (7, 10));
    }

    /// `CXM1014::Reload`: 0.3 s per shell, with the two reload sounds.
    fn reload(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.default_shotgun_reload(
            XM1014_RELOAD,
            XM1014_START_RELOAD,
            0.3,
            0.55,
            Some("weapons/reload1.wav"),
            Some("weapons/reload3.wav"),
        );
    }

    /// `CXM1014::WeaponIdle`.
    fn weapon_idle(&mut self, w: &mut WeaponCtx<'_, '_>) {
        shotgun_idle(self, w, XM1014_IDLE, XM1014_PUMP);
    }

    fn max_speed(&self, _w: &super::weapon::Weapon) -> f32 {
        XM1014_MAX_SPEED
    }
}
