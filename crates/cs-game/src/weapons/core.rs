//! The weapon state machine: `CBasePlayerWeapon::ItemPostFrame` and the helpers every weapon
//! calls (`DefaultDeploy`, `DefaultReload`, `DefaultShotgunReload`, `PlayEmptySound`,
//! `KickBack`, `FireRemaining`, `GetNextAttackDelay`, `SendWeaponAnim`, `UpdateClientData`'s
//! `CurWeapon`, ...) and the player's side of it (`ItemPreFrame`, `ItemPostFrame`, the decay of
//! the timers at the end of `PostThink`, `SelectItem`, `GiveAmmo`, `SendAmmoUpdate`). Ported
//! from ReGameDLL_CS `weapons.cpp` and `player.cpp`, read 2026-11-22, the vanilla build
//! (`CLIENT_WEAPONS`, no `REGAMEDLL_FIXES`, `REGAMEDLL_ADD`, `BUILD_LATEST`).
//!
//! ## Timers
//!
//! With `CLIENT_WEAPONS` `UTIL_WeaponTimeBase()` is 0: the weapon timers are "seconds left",
//! counted down at the end of `PostThink` ([`decay_timers`]) by the command's frame time, and
//! `m_flNextAttack` of the player too. A few timers are absolute server time instead
//! (`m_flGlock18Shoot`, `m_flFamasShoot`, `m_flDecreaseShotsFired`, `m_flEjectBrass`); every
//! one is named where it is used. Server time is a `float` in C (`gpGlobals->time`), so it is
//! taken as `f32` here ([`WeaponCtx::now`]).
//!
//! ## How a weapon runs
//!
//! A weapon's [`WeaponSlotItem`] is checked out of the player's list for the length of a call
//! ([`with_weapon`]): the context [`WeaponCtx`] then holds the game, the engine and the
//! weapon's fields, and the behaviour the trait methods. While it is out, a placeholder with the
//! same id stands in the list, so a lookup by id still finds it (its fields are the
//! placeholder's, not the weapon's).
//!
//! Not ported and named: the legacy `m_pTank` (func_tank), `ImpulseCommands`' use, flashlight
//! and spray (see [`crate::player`]), `ResetAutoaim`/`GetAutoaimVector` (no weapon of
//! CS 1.6 uses them for more than a call), the shield's crosshair, and `m_fOnTarget` (only the
//! autoaim of the Half-Life weapons sets it).

use cs_engine::consts::FL_DORMANT;
use cs_engine::event_flags::FEV_NOTHOST;
use cs_engine::{EdictId, EngineApi, MsgDest};
use cs_proto48::bitbuf::MsgWriter;
use cs_sim::pm::consts::{IN_ATTACK, IN_ATTACK2, IN_RELOAD};
use cs_sim::pm::math::angle_vectors;
use glam::Vec3;

use super::info::{
    BulletType, ITEM_FLAG_NOAUTORELOAD, ITEM_FLAG_NOFIREUNDERWATER, ItemInfo, MAX_AMMO_SLOTS,
    WEAPON_NOCLIP, WPNSTATE_SHIELD_DRAWN, WeaponId, ammo_index, is_secondary_weapon, item_info,
    max_ammo_carry,
};
use super::weapon::{MAX_ITEM_TYPES, Weapon, WeaponBehavior, WeaponSlotItem, item_slot};
use crate::GameState;
use crate::messages;
use crate::player::{ATTN_NORM, CHAN_ITEM, PITCH_NORM, Player};

/// `CHAN_WEAPON`.
pub const CHAN_WEAPON: u8 = 1;
/// `EF_MUZZLEFLASH`.
const EF_MUZZLEFLASH: u32 = 2;
/// `DEFAULT_FOV`.
const DEFAULT_FOV: u8 = 90;
/// `MAX_DIST_RELOAD_SOUND` (`weapons.h`): how far a reload is heard on the radar HUD.
const MAX_DIST_RELOAD_SOUND: f32 = 512.0;
/// `GLOCK18_DAMAGE`: the damage of a Glock 18 bullet (`CSPlayerWeapon()->m_flBaseDamage` of
/// `FireRemaining`, which `REGAMEDLL_API` initialises to it).
pub const GLOCK18_DAMAGE: f32 = 25.0;
/// `FAMAS_DAMAGE`: the damage of a Famas bullet of a burst's later shots.
pub const FAMAS_DAMAGE: f32 = 30.0;
/// `TE_MODEL`, the effect `EjectBrass` sends.
const TE_MODEL: u8 = 106;
/// `TE_BOUNCE_SHELL` and `TE_BOUNCE_SHOTSHELL`: the sound the brass makes when it lands.
const TE_BOUNCE_SHELL: u8 = 1;
const TE_BOUNCE_SHOTSHELL: u8 = 2;

/// The legacy per-ammo counters of `CBasePlayer` (`ammo_buckshot`, `ammo_9mm`, ...) that
/// `TabulateAmmo` copies from `m_rgAmmo`; the client data carries them. The order of the array
/// [`Player::tabulated_ammo`] is this one.
pub const TABULATED_AMMO: [&str; 10] = [
    "buckshot", "9mm", "556Nato", "556NatoBox", "762Nato", "45acp", "50AE", "338Magnum", "57mm",
    "357SIG",
];
/// The index of `ammo_buckshot` in [`Player::tabulated_ammo`].
pub const TAB_BUCKSHOT: usize = 0;
/// The index of `ammo_9mm`.
pub const TAB_9MM: usize = 1;
/// The index of `ammo_556nato`.
pub const TAB_556NATO: usize = 2;

/// `PLAYER_ANIM`: what `CBasePlayer::SetAnimation` is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerAnim {
    Idle,
    Walk,
    Jump,
    SuperJump,
    Die,
    Attack1,
    Attack2,
    Flinch,
    LargeFlinch,
    Reload,
    HoldBomb,
}

/// `CanAttack` (`weapons.cpp`, read 2026-11-22), with `CLIENT_WEAPONS`: a weapon the client
/// predicts (`is_predicted`) may attack when its timer is spent (`<= 0`), one it does not when
/// the timer is not after `curtime`. Both are the same for `curtime` 0.
pub fn can_attack(attack_time: f32, curtime: f32, is_predicted: bool) -> bool {
    if !is_predicted { attack_time <= curtime } else { attack_time <= 0.0 }
}

/// The table of `CBasePlayerWeapon::HasSecondaryAttack` (vanilla: no `UMP45`), without the
/// shield rule: weapons that have no secondary attack.
pub fn has_secondary_attack_by_id(id: WeaponId) -> bool {
    use WeaponId::*;
    !matches!(
        id,
        Ak47 | Xm1014 | Mac10 | Elite | FiveSeven | Mp5n | M249 | M3 | Tmp | Deagle | P228 | P90
            | C4 | Galil
    )
}

/// What a weapon's methods work with: the game, the engine, the slot of the player that holds
/// the weapon, and the weapon's own fields. Built by [`with_weapon`].
pub struct WeaponCtx<'a, 'e> {
    /// The game state, with the player (see [`WeaponCtx::player`]).
    pub g: &'a mut GameState,
    /// The engine.
    pub eng: &'a mut EngineApi<'e>,
    /// The slot of the player that holds the weapon.
    pub slot: u8,
    /// The weapon's fields.
    pub weapon: &'a mut Weapon,
    /// Set by a weapon that destroys itself (the last grenade thrown): [`with_weapon`] removes
    /// it from the player's list when the call returns.
    pub remove_self: bool,
    /// `UseDecrement()` of the weapon.
    use_decrement: bool,
}

impl<'a, 'e> WeaponCtx<'a, 'e> {
    /// A context for `weapon`, held by the player of `slot`. `use_decrement` is the weapon's
    /// `UseDecrement()`.
    pub fn new(
        g: &'a mut GameState,
        eng: &'a mut EngineApi<'e>,
        slot: u8,
        weapon: &'a mut Weapon,
        use_decrement: bool,
    ) -> Self {
        Self { g, eng, slot, weapon, remove_self: false, use_decrement }
    }

    /// The player that holds the weapon (`m_pPlayer`). The player outlives every call a
    /// weapon makes: a context is only built for a slot that has a player.
    pub fn player(&self) -> &Player {
        self.g.players[usize::from(self.slot)]
            .as_ref()
            .expect("the weapon's player exists while the weapon runs")
    }

    /// `m_pPlayer`, mutably.
    pub fn player_mut(&mut self) -> &mut Player {
        self.g.players[usize::from(self.slot)]
            .as_mut()
            .expect("the weapon's player exists while the weapon runs")
    }

    /// The player's edict.
    pub fn edict(&self) -> EdictId {
        self.eng.client_edict(self.slot)
    }

    /// `gpGlobals->time` as the `float` it is in C.
    pub fn now(&self) -> f32 {
        self.eng.time() as f32
    }

    /// `m_pPlayer->pev->button`.
    pub fn button(&self) -> i32 {
        self.eng.ent(self.edict()).map_or(0, |v| v.button)
    }

    /// `m_pPlayer->pev->button &= ~mask`.
    pub fn clear_button(&mut self, mask: i32) {
        let id = self.edict();
        if let Some(v) = self.eng.ent_mut(id) {
            v.button &= !mask;
        }
    }

    /// `GetItemInfo` of the weapon.
    pub fn info(&self) -> Option<&'static ItemInfo> {
        item_info(self.weapon.id)
    }

    /// `iMaxClip()`.
    pub fn max_clip(&self) -> i32 {
        self.info().map_or(0, |i| i.max_clip)
    }

    /// `iFlags()`.
    pub fn flags(&self) -> i32 {
        self.info().map_or(0, |i| i.flags)
    }

    /// `m_pPlayer->m_rgAmmo[index]`, 0 for an index out of the table.
    pub fn ammo(&self, index: i32) -> i32 {
        usize::try_from(index).ok().and_then(|i| self.player().ammo.get(i).copied()).unwrap_or(0)
    }

    /// Adds `delta` to `m_pPlayer->m_rgAmmo[index]` (an index out of the table is ignored).
    pub fn add_ammo(&mut self, index: i32, delta: i32) {
        if let Ok(i) = usize::try_from(index)
            && let Some(a) = self.player_mut().ammo.get_mut(i)
        {
            *a += delta;
        }
    }

    /// `m_pPlayer->HasShield()`.
    pub fn has_shield(&self) -> bool {
        self.player().owns_shield
    }

    /// `RANDOM_LONG(lo, hi)`: the game's own generator.
    pub fn random_long(&mut self, lo: i32, hi: i32) -> i32 {
        self.g.random_range(lo, hi)
    }

    /// `RANDOM_FLOAT(lo, hi)`. Assumption: a uniform float from the game's generator (the
    /// engine's `RandomFloat` is not reproduced; nothing but the ejected brass uses it).
    pub fn random_float(&mut self, lo: f32, hi: f32) -> f32 {
        let r = self.g.random_range(0, 65535) as f32 / 65535.0;
        lo + (hi - lo) * r
    }

    /// `m_pPlayer->TabulateAmmo()` ([`tabulate_ammo`]).
    pub fn tabulate_ammo(&mut self) {
        tabulate_ammo(self.player_mut());
    }

    /// `CBasePlayer::SetAnimation(anim)`. Wired to the player animation by Task 9 (`player_anim`);
    /// until then nothing moves the third-person model.
    pub fn set_animation(&mut self, _anim: PlayerAnim) {}

    /// `CBasePlayerWeapon::SendWeaponAnim(iAnim, skiplocal)`: records the animation and sends
    /// `svc_weaponanim` unless the client predicts it (`skiplocal` and `cl_lw`).
    pub fn send_weapon_anim(&mut self, anim: i32, skiplocal: bool) {
        let id = self.edict();
        if let Some(v) = self.eng.ent_mut(id) {
            v.weaponanim = anim;
        }
        // `ENGINE_CANSKIP(m_pPlayer->edict())`: the client has `cl_lw` set.
        if skiplocal && self.eng.client_lw(self.slot) {
            return;
        }
        self.eng.weapon_anim(self.slot, anim, self.weapon.body);
    }

    /// `CBasePlayerWeapon::DefaultDeploy` (`weapons.cpp` 1492-1520): false when the weapon
    /// `me` (this weapon's behaviour) cannot deploy. QUIRKS kept: the view and weapon model
    /// are looked up by name (a name that is not precached is model 0); `m_flNextAttack` is
    /// set to 0.75 s relative, the idle to 1.5 s; the FOV is back to 90 and the zoom to resume
    /// forgotten.
    ///
    /// DEVIATION (assumption): C stores the model names as strings in `pev->viewmodel` and
    /// `pev->weaponmodel`; here the fields are the model indices (Task 4's `u16`), so the
    /// names are resolved through the precache table when deployed.
    pub fn default_deploy(
        &mut self,
        me: &dyn WeaponBehavior,
        view_model: &str,
        weapon_model: &str,
        anim: i32,
        anim_ext: &str,
        skiplocal: bool,
    ) -> bool {
        if !me.can_deploy(self) {
            return false;
        }
        self.tabulate_ammo();
        let view = self.eng.model_index(view_model).unwrap_or(0);
        let weapon = self.eng.model_index(weapon_model).unwrap_or(0);
        let id = self.edict();
        if let Some(v) = self.eng.ent_mut(id) {
            v.viewmodel = view;
            v.weaponmodel = weapon;
        }
        self.weapon.model_name = view_model.to_string();
        self.player_mut().anim_extension = anim_ext.to_string();
        self.send_weapon_anim(anim, skiplocal);

        let now = self.now();
        let p = self.player_mut();
        p.next_attack = 0.75;
        p.fov = DEFAULT_FOV;
        p.last_zoom = i32::from(DEFAULT_FOV);
        p.resume_zoom = false;
        self.weapon.time_weapon_idle = 1.5;
        self.weapon.last_fire_time = 0.0;
        self.weapon.decrease_shots_fired = now;
        if let Some(v) = self.eng.ent_mut(id) {
            v.fov = f32::from(DEFAULT_FOV);
        }
        true
    }

    /// `CBasePlayerWeapon::Holster` (`weapons.cpp` 1680): cancels a reload in progress and
    /// hides the models. (`m_fInSpecialReload` is only reset under `REGAMEDLL_FIXES`.)
    pub fn base_holster(&mut self) {
        self.weapon.in_reload = false;
        let id = self.edict();
        if let Some(v) = self.eng.ent_mut(id) {
            v.viewmodel = 0;
            v.weaponmodel = 0;
        }
    }

    /// `CBasePlayerWeapon::ReloadSound` (`weapons.cpp` 1522): the other players within 512
    /// units are sent `ReloadSound` (a volume byte falling with the distance, and 0 for a
    /// shotgun, 1 for any other weapon).
    pub fn reload_sound(&mut self) {
        let mine = self.edict();
        let Some(origin) = self.eng.ent(mine).map(|v| v.origin) else {
            return;
        };
        let shotgun = matches!(self.weapon.id, WeaponId::M3 | WeaponId::Xm1014);
        for other in 0..self.g.players.len() {
            let other_slot = other as u8;
            if other_slot == self.slot || self.g.players[other].is_none() {
                continue;
            }
            let Some(v) = self.eng.ent(self.eng.client_edict(other_slot)) else {
                continue;
            };
            if v.flags & FL_DORMANT != 0 {
                continue;
            }
            let distance = (origin - v.origin).length();
            if distance <= MAX_DIST_RELOAD_SOUND {
                let volume = ((1.0 - (distance / MAX_DIST_RELOAD_SOUND)) * 255.0) as i32 as u8;
                messages::reload_sound(self.eng, other_slot, volume, u8::from(!shotgun));
            }
        }
    }

    /// `CBasePlayerWeapon::DefaultReload` (`weapons.cpp` 1552-1572): false when there is no
    /// ammo to load or the clip is full. QUIRK: `m_flNextAttack` is set to the delay itself
    /// (a countdown), not to a time.
    pub fn default_reload(&mut self, clip_size: i32, anim: i32, delay: f32) -> bool {
        let reserve = self.ammo(self.weapon.primary_ammo_type);
        if reserve <= 0 {
            return false;
        }
        let j = (clip_size - self.weapon.clip).min(reserve);
        if j == 0 {
            return false;
        }
        self.player_mut().next_attack = delay;
        self.reload_sound();
        self.send_weapon_anim(anim, self.use_decrement);
        self.weapon.in_reload = true;
        self.weapon.time_weapon_idle = delay + 0.5;
        true
    }

    /// `CBasePlayerWeapon::DefaultShotgunReload` (`weapons.cpp` 1576-1636), the three stages
    /// of a shotgun's reload: 0 starts (the start animation), 1 waits for the idle timer and
    /// plays a random reload sound (`RANDOM_LONG(0, 1)` picks one of two, the pitch is
    /// `85 + RANDOM_LONG(0, 31)`), 2 puts a shell in the clip. Returns false when it did
    /// nothing. QUIRK: the last stage has no timer check (`BUILD_LATEST_FIXES` adds it); the
    /// legacy `ammo_buckshot` counter goes down with the reserve.
    pub fn default_shotgun_reload(
        &mut self,
        anim: i32,
        start_anim: i32,
        delay: f32,
        start_delay: f32,
        sound1: Option<&str>,
        sound2: Option<&str>,
    ) -> bool {
        if self.ammo(self.weapon.primary_ammo_type) <= 0 || self.weapon.clip == self.max_clip() {
            return false;
        }
        // Don't reload until recoil is done.
        if self.weapon.next_primary_attack > 0.0 {
            return false;
        }
        if self.weapon.in_special_reload == 0 {
            self.set_animation(PlayerAnim::Reload);
            self.send_weapon_anim(start_anim, self.use_decrement);
            self.weapon.in_special_reload = 1;
            let t = start_delay;
            self.weapon.next_secondary_attack = t;
            self.weapon.time_weapon_idle = t;
            self.player_mut().next_attack = t;
            self.weapon.next_primary_attack = self.get_next_attack_delay(start_delay);
        } else if self.weapon.in_special_reload == 1 {
            if self.weapon.time_weapon_idle > 0.0 {
                return false;
            }
            // Was waiting for the gun to move to the side.
            self.weapon.in_special_reload = 2;
            let sample = match (sound1, sound2) {
                (Some(a), Some(b)) => Some(if self.random_long(0, 1) != 0 { a } else { b }),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };
            if let Some(sample) = sample.filter(|s| !s.is_empty()) {
                let pitch = 85 + self.random_long(0, 31);
                let id = self.edict();
                self.eng.emit_sound_flags(
                    id,
                    i32::from(CHAN_ITEM),
                    sample,
                    1.0,
                    ATTN_NORM,
                    0,
                    pitch,
                );
            }
            self.send_weapon_anim(anim, self.use_decrement);
            self.weapon.time_weapon_idle = delay;
            self.weapon.next_reload = delay;
        } else {
            self.weapon.clip += 1;
            let ammo_type = self.weapon.primary_ammo_type;
            self.add_ammo(ammo_type, -1);
            self.player_mut().tabulated_ammo[TAB_BUCKSHOT] -= 1;
            self.weapon.in_special_reload = 1;
        }
        true
    }

    /// `CBasePlayerWeapon::PlayEmptySound` (`weapons.cpp` 1638): the dry-fire click, quieter
    /// (0.8) on the weapon channel; pistols have their own. Only while `m_iPlayEmptySound` is
    /// set, which `ResetEmptySound` (the weapons' idle) sets and nothing here clears (vanilla),
    /// so a held trigger clicks at every attempt. Always false.
    pub fn play_empty_sound(&mut self) -> bool {
        if self.weapon.play_empty_sound {
            let sample = if is_secondary_weapon(self.weapon.id) {
                "weapons/dryfire_pistol.wav"
            } else {
                "weapons/dryfire_rifle.wav"
            };
            let id = self.edict();
            self.eng.emit_sound(id, CHAN_WEAPON, sample, 0.8, ATTN_NORM, PITCH_NORM);
        }
        false
    }

    /// `CBasePlayerWeapon::ResetEmptySound`.
    pub fn reset_empty_sound(&mut self) {
        self.weapon.play_empty_sound = true;
    }

    /// `CBasePlayerWeapon::IsUseable` (`weapons.cpp` 1469): false when the clip is empty and
    /// the player has no ammo of the weapon's type left.
    pub fn is_useable(&self) -> bool {
        !(self.weapon.clip <= 0
            && self.ammo(self.weapon.primary_ammo_type) <= 0
            && self.info().is_some_and(|i| i.max_ammo1 != -1))
    }

    /// `CBasePlayerWeapon::AddPrimaryAmmo` (`weapons.cpp` 1416): gives `count` rounds of
    /// ammo `name`, the clip first when the weapon has one and it is empty. True when the
    /// player got ammo of the type (an ammo index above 0). Assumption: the "got ammo" sound
    /// plays from the player (the weapon has no entity of its own while carried).
    pub fn add_primary_ammo(&mut self, count: i32, name: &str, max_clip: i32, max_carry: i32) -> bool {
        let id_ammo = if max_clip < 1 {
            self.weapon.clip = WEAPON_NOCLIP;
            give_ammo(self.g, self.eng, self.slot, count, Some(name), max_carry)
        } else if self.weapon.clip == 0 {
            let i = (self.weapon.clip + count).min(max_clip);
            self.weapon.clip += i;
            give_ammo(self.g, self.eng, self.slot, count - i, Some(name), max_carry)
        } else {
            give_ammo(self.g, self.eng, self.slot, count, Some(name), max_carry)
        };
        if id_ammo > 0 {
            self.weapon.primary_ammo_type = id_ammo;
            let id = self.weapon.id;
            if self.player().items.iter().flatten().any(|i| i.weapon.id == id) {
                // Play the "got ammo" sound only if we gave some ammo to a player that already
                // had this gun.
                let ent = self.edict();
                self.eng.emit_sound(ent, CHAN_ITEM, "items/9mmclip1.wav", 1.0, ATTN_NORM, PITCH_NORM);
            }
        }
        id_ammo > 0
    }

    /// `CBasePlayerWeapon::AddSecondaryAmmo` (`weapons.cpp` 1450).
    pub fn add_secondary_ammo(&mut self, count: i32, name: &str, max: i32) -> bool {
        let id_ammo = give_ammo(self.g, self.eng, self.slot, count, Some(name), max);
        if id_ammo > 0 {
            self.weapon.secondary_ammo_type = id_ammo;
            let ent = self.edict();
            self.eng.emit_sound(ent, CHAN_ITEM, "items/9mmclip1.wav", 1.0, ATTN_NORM, PITCH_NORM);
        }
        id_ammo > 0
    }

    /// `CBasePlayerWeapon::GetNextAttackDelay` (`weapons.cpp` 1755): with the time creep code
    /// off (`REGAMEDLL_BUILD_6153` is never defined) the answer is `delay` itself; the
    /// bookkeeping of `m_flPrevPrimaryAttack` and `m_flLastFireTime` is kept.
    pub fn get_next_attack_delay(&mut self, delay: f32) -> f32 {
        let now = self.now();
        if self.weapon.last_fire_time == 0.0 || self.weapon.next_primary_attack == -1.0 {
            // The client has stopped firing: reset the bookkeeping.
            self.weapon.prev_primary_attack = delay;
            self.weapon.last_fire_time = now;
        }
        let next_attack = 0.0 + delay;
        // Save the last fire time.
        self.weapon.last_fire_time = now;
        // Remember what `m_flNextPrimaryAttack` is set to for each shot.
        self.weapon.prev_primary_attack = next_attack - 0.0;
        next_attack
    }

    /// `HasSecondaryAttack` (`weapons.cpp` 895): true with a shield, else the weapon's own
    /// answer.
    pub fn has_secondary_attack(&self, me: &dyn WeaponBehavior) -> bool {
        self.has_shield() || me.has_secondary_attack(self.weapon)
    }

    /// `CBasePlayerWeapon::KickBack` (`weapons.cpp` 769-810, the vanilla branch): the recoil
    /// that pushes the view up and to the side, bounded by `up_max` and `lateral_max`, then a
    /// 1 in `direction_change + 1` chance (`RANDOM_LONG`, the game's own generator, not the
    /// shared one) that the lateral direction flips. `real_t` is `float` here.
    #[allow(clippy::too_many_arguments)]
    pub fn kick_back(
        &mut self,
        up_base: f32,
        lateral_base: f32,
        up_modifier: f32,
        lateral_modifier: f32,
        up_max: f32,
        lateral_max: f32,
        direction_change: i32,
    ) {
        let shots = self.weapon.shots_fired as f32;
        let (kick_up, kick_lateral) = if self.weapon.shots_fired == 1 {
            (up_base, lateral_base)
        } else {
            (shots * up_modifier + up_base, shots * lateral_modifier + lateral_base)
        };
        let forward = self.weapon.direction == 1;
        let id = self.edict();
        if let Some(v) = self.eng.ent_mut(id) {
            v.punchangle.x -= kick_up;
            if v.punchangle.x < -up_max {
                v.punchangle.x = -up_max;
            }
            if forward {
                v.punchangle.y += kick_lateral;
                if v.punchangle.y > lateral_max {
                    v.punchangle.y = lateral_max;
                }
            } else {
                v.punchangle.y -= kick_lateral;
                if v.punchangle.y < -lateral_max {
                    v.punchangle.y = -lateral_max;
                }
            }
        }
        if self.random_long(0, direction_change) == 0 {
            self.weapon.direction = i32::from(self.weapon.direction == 0);
        }
    }

    /// `CBasePlayerWeapon::FireRemaining` (`weapons.cpp` 813-877): the rest of a burst, the
    /// Glock's (`glock18` true: 0.05 spread, range 0.9, 9 mm, 1 penetration) or the Famas's
    /// (the burst spread, range 0.96, 5.56 mm, 2 penetration). The shot is taken from the view
    /// (angles plus punch) at the eye; a spent clip ends the burst with the count at 3.
    /// QUIRKS kept: the reserve of the legacy `ammo_9mm`/`ammo_556nato` counters goes down, not
    /// `m_rgAmmo`; the damage is the float constant cut to an int by the call; the event's
    /// punch is multiplied by 10000 (Glock) or 10000000 (Famas), and the Glock's last argument
    /// is "the clip is empty".
    pub fn fire_remaining(&mut self, glock18: bool) {
        let next_time = 0.1_f32;
        let mut shots_fired = if glock18 {
            self.weapon.glock18_shots_fired
        } else {
            self.weapon.famas_shots_fired
        };
        self.weapon.clip -= 1;
        if self.weapon.clip < 0 {
            self.weapon.clip = 0;
            self.store_burst(glock18, 3, 0.0);
            return;
        }
        let id = self.edict();
        let Some(v) = self.eng.ent(id) else {
            return;
        };
        let (forward, ..) = angle_vectors(v.v_angle + v.punchangle);
        let src = v.origin + v.view_ofs;
        let flag = FEV_NOTHOST;
        let base_damage = if glock18 { GLOCK18_DAMAGE } else { FAMAS_DAMAGE };
        let seed = self.player().random_seed;
        let dir;
        if glock18 {
            dir = self.fire_bullets3(
                src, forward, 0.05, 8192.0, 1, BulletType::Player9mm, base_damage as i32, 0.9,
                Some(id), true, seed,
            );
            self.player_mut().tabulated_ammo[TAB_9MM] -= 1;
            let punch = self.eng.ent(id).map_or(Vec3::ZERO, |v| v.punchangle);
            let event = self.weapon.events.glock18;
            let clip_empty = self.weapon.clip == 0;
            self.playback_event(
                flag, event, dir.x, dir.y, (punch.x * 10000.0) as i32, (punch.y * 10000.0) as i32,
                clip_empty, false,
            );
        } else {
            let spread = self.weapon.burst_spread;
            dir = self.fire_bullets3(
                src, forward, spread, 8192.0, 2, BulletType::Player556mm, base_damage as i32, 0.96,
                Some(id), false, seed,
            );
            self.player_mut().tabulated_ammo[TAB_556NATO] -= 1;
            let punch = self.eng.ent(id).map_or(Vec3::ZERO, |v| v.punchangle);
            let event = self.weapon.events.famas;
            self.playback_event(
                flag, event, dir.x, dir.y, (punch.x * 10_000_000.0) as i32,
                (punch.y * 10_000_000.0) as i32, false, false,
            );
        }
        if let Some(v) = self.eng.ent_mut(id) {
            v.effects |= EF_MUZZLEFLASH;
        }
        self.set_animation(PlayerAnim::Attack1);
        shots_fired += 1;
        let shoot_time = if shots_fired != 3 { self.now() + next_time } else { 0.0 };
        self.store_burst(glock18, shots_fired, shoot_time);
    }

    /// Writes back the two by-reference arguments of `FireRemaining`.
    fn store_burst(&mut self, glock18: bool, shots_fired: i32, shoot_time: f32) {
        if glock18 {
            self.weapon.glock18_shots_fired = shots_fired;
            self.weapon.glock18_shoot = shoot_time;
        } else {
            self.weapon.famas_shots_fired = shots_fired;
            self.weapon.famas_shoot = shoot_time;
        }
    }

    /// `PLAYBACK_EVENT_FULL(flags, m_pPlayer->edict(), event, 0, g_vecZero, g_vecZero, f1, f2,
    /// i1, i2, b1, b2)`: what every weapon's fire event is called with (no delay, no origin or
    /// angles of its own).
    #[allow(clippy::too_many_arguments)]
    pub fn playback_event(
        &mut self,
        flags: i32,
        event: u16,
        fparam1: f32,
        fparam2: f32,
        iparam1: i32,
        iparam2: i32,
        bparam1: bool,
        bparam2: bool,
    ) {
        let id = self.edict();
        self.eng.playback_event(
            flags, Some(id), event, 0.0, Vec3::ZERO, Vec3::ZERO, fparam1, fparam2, iparam1, iparam2,
            bparam1, bparam2,
        );
    }

    /// `CBaseEntity::FireBullets3` (`cbase.cpp`/`combat.cpp`): the bullet of a gun, with its
    /// spread (from the shared random `shared_rand`), range, penetration and range modifier;
    /// returns the spread vector the event carries. `attacker` is the shooter's edict.
    /// Wired to `bullets::fire_bullets3` by the controller after Task 7; until then nothing is
    /// fired and the spread is zero.
    #[allow(clippy::too_many_arguments)]
    pub fn fire_bullets3(
        &mut self,
        _src: Vec3,
        _dir: Vec3,
        _spread: f32,
        _distance: f32,
        _penetration: i32,
        _bullet: BulletType,
        _damage: i32,
        _range_modifier: f32,
        _attacker: Option<EdictId>,
        _is_pistol: bool,
        _shared_rand: u32,
    ) -> Vec3 {
        Vec3::ZERO
    }

    /// `CBaseEntity::FireBullets` (the buckshot of the shotguns: `shots` pellets with a
    /// spread vector). Wired to `bullets::fire_bullets` by the controller after Task 7; until
    /// then nothing is fired.
    #[allow(clippy::too_many_arguments)]
    pub fn fire_bullets(
        &mut self,
        _shots: u32,
        _src: Vec3,
        _dir: Vec3,
        _spread: Vec3,
        _distance: f32,
        _bullet: BulletType,
        _tracer_freq: i32,
        _damage: i32,
        _attacker: Option<EdictId>,
    ) {
    }

    /// `CBasePlayer::ResetMaxSpeed` from inside the weapon `me`: the same as
    /// [`crate::player::reset_max_speed`], with the active weapon's speed taken from `me` (the
    /// weapon is checked out, so the list cannot answer for it).
    pub fn reset_max_speed(&mut self, me: &dyn WeaponBehavior) {
        let active = self.player().active_item == Some(self.weapon.id);
        let speed = if active {
            Some(me.max_speed(self.weapon))
        } else {
            self.player().active_max_speed()
        };
        crate::player::apply_max_speed(self.g, self.eng, self.slot, speed);
    }

    /// `CBasePlayerWeapon::EjectBrassLate` (`weapons.cpp` 682): the shell of a bolt-action or
    /// pump weapon leaves it late (its weapon sets `m_flEjectBrass`). A `Brass` message to
    /// the players that see the shooter: `TE_MODEL`, the position (eye, 9 down, 16 forward),
    /// the left vector, the velocity (the player's, plus 50..70 right, 100..150 up, 25
    /// forward), the yaw, the model, the landing sound, 25 (2.5 s) and the shooter's entity
    /// index. Assumption: the weapon entity follows the player, so its origin and yaw are the
    /// player's.
    pub fn eject_brass_late(&mut self) {
        let id = self.edict();
        let Some(v) = self.eng.ent(id) else {
            return;
        };
        let (v_angle, punch, velocity, origin, view_ofs, yaw) =
            (v.v_angle, v.punchangle, v.velocity, v.origin, v.view_ofs, v.angles.y);
        let (forward, right, up) = angle_vectors(v_angle + punch);
        let up_part = up * self.random_float(100.0, 150.0);
        let right_part = right * self.random_float(50.0, 70.0);
        let shell_velocity = (velocity + right_part + up_part) + forward * 25.0;
        let sound = if matches!(self.weapon.id, WeaponId::Xm1014 | WeaponId::M3) {
            TE_BOUNCE_SHOTSHELL
        } else {
            TE_BOUNCE_SHELL
        };
        let position = origin + view_ofs + up * -9.0 + forward * 16.0;
        let left = right * -9.0;
        let mut w = MsgWriter::new();
        w.write_u8(TE_MODEL);
        for c in [position, left, shell_velocity] {
            w.write_coord(c.x);
            w.write_coord(c.y);
            w.write_coord(c.z);
        }
        w.write_angle(yaw);
        w.write_u16(self.weapon.shell_id as u16);
        w.write_u8(sound);
        w.write_u8(25);
        w.write_u8(id.index as u8);
        messages::send_raw(self.eng, MsgDest::Pvs(position), "Brass", w.as_slice());
    }
}

/// `CBasePlayer::TabulateAmmo` (`player.cpp` 8840): copies the reserves of ten ammo types
/// into the legacy per-type counters ([`TABULATED_AMMO`]). An ammo that is not registered
/// reads as -1 (`AmmoInventory(-1)`).
pub fn tabulate_ammo(p: &mut Player) {
    for (slot, name) in TABULATED_AMMO.iter().enumerate() {
        let index = ammo_index(name);
        p.tabulated_ammo[slot] = usize::try_from(index)
            .ok()
            .and_then(|i| p.ammo.get(i).copied())
            .unwrap_or(-1);
    }
}

/// `CBasePlayer::GiveAmmo` (`player.cpp` 7371): gives the player of `slot` up to `count`
/// rounds of ammo `name`, at most `max` in all (`-1`: the weapons' maximum of that ammo). The
/// ammo's slot index, or -1 when nothing can be given: a spectator, no name, the rules
/// refuse (`CanHaveAmmo`: the player already carries the maximum), an unknown name. An ammo
/// the player has the maximum of answers -1 as well; one that adds nothing but is not full
/// answers the index. Sends `AmmoPickup` for what was added.
pub(crate) fn give_ammo(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    count: i32,
    name: Option<&str>,
    max: i32,
) -> i32 {
    let id = eng.client_edict(slot);
    if eng.ent(id).is_some_and(|v| v.flags & cs_engine::consts::FL_SPECTATOR != 0) {
        return -1;
    }
    let Some(name) = name.filter(|n| !n.is_empty()) else {
        return -1;
    };
    let max = if max == -1 { max_ammo_carry(name) } else { max };
    let index = ammo_index(name);
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return -1;
    };
    // `CHalfLifeMultiplay::CanHaveAmmo`: the ammo is known and the player has less than max.
    let Some(have) = usize::try_from(index).ok().and_then(|i| p.ammo.get(i).copied()) else {
        return -1;
    };
    if have >= max {
        return -1;
    }
    let add = count.min(max - have);
    if add < 1 {
        return index;
    }
    p.ammo[index as usize] += add;
    messages::send_raw(eng, MsgDest::One(slot), "AmmoPickup", &[index as u8, add as u8]);
    if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        tabulate_ammo(p);
    }
    index
}

impl Player {
    /// The `(slot list, position)` of the item with weapon id `id` in `items`.
    pub(crate) fn find_item(&self, id: WeaponId) -> Option<(usize, usize)> {
        self.items.iter().enumerate().find_map(|(s, list)| {
            list.iter().position(|i| i.weapon.id == id).map(|pos| (s, pos))
        })
    }

    /// The active weapon's `GetMaxSpeed()`, `None` without an active item.
    pub(crate) fn active_max_speed(&self) -> Option<f32> {
        let (s, i) = self.find_item(self.active_item?)?;
        let item = &self.items[s][i];
        Some(item.behavior.max_speed(&item.weapon))
    }

    /// `CBasePlayer::IsReloading`: the active weapon is in a reload.
    pub(crate) fn is_reloading(&self) -> bool {
        self.active_item
            .and_then(|id| self.find_item(id))
            .is_some_and(|(s, i)| self.items[s][i].weapon.in_reload)
    }
}

/// Checks the weapon `id` of the player in `slot` out of its list and runs `f` with its
/// behaviour and a [`WeaponCtx`]. `None` when the player has no such weapon. The weapon goes
/// back in the list where it was (if the list is not the one it was taken from any more, it is
/// dropped), unless `f` set [`WeaponCtx::remove_self`], which removes it (and, if it was the
/// active or the last item, forgets that).
pub(crate) fn with_weapon<R>(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    id: WeaponId,
    f: impl FnOnce(&mut dyn WeaponBehavior, &mut WeaponCtx<'_, '_>) -> R,
) -> Option<R> {
    let p = g.players.get_mut(usize::from(slot))?.as_mut()?;
    let (s, i) = p.find_item(id)?;
    if p.items[s][i].vacant {
        // Already checked out: a weapon does not run inside itself.
        return None;
    }
    let mut item = std::mem::replace(&mut p.items[s][i], WeaponSlotItem::vacant(id));
    let use_decrement = item.behavior.use_decrement();
    let (out, remove) = {
        let mut ctx = WeaponCtx::new(g, eng, slot, &mut item.weapon, use_decrement);
        let out = f(item.behavior.as_mut(), &mut ctx);
        (out, ctx.remove_self)
    };
    if let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) {
        let at = p.items[s].iter().position(|x| x.vacant && x.weapon.id == id);
        match (at, remove) {
            (Some(at), false) => p.items[s][at] = item,
            (Some(at), true) => {
                p.items[s].remove(at);
                forget_item(p, id);
            }
            (None, _) => {}
        }
    }
    Some(out)
}

/// A player that no longer has weapon `id` forgets it as the active, the last and the
/// client's active item, and its `pev->weapons` bit goes.
fn forget_item(p: &mut Player, id: WeaponId) {
    if p.active_item == Some(id) {
        p.active_item = None;
    }
    if p.last_item == Some(id) {
        p.last_item = None;
    }
    p.weapons &= !id.bit();
}

/// `CBasePlayer::ItemPreFrame` (`player.cpp` 7418): the active item's `ItemPreFrame`, skipped
/// while `m_flNextAttack` is above 0 (`CLIENT_WEAPONS`).
pub(crate) fn player_item_pre_frame(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = g.players.get(usize::from(slot)).and_then(Option::as_ref) else {
        return;
    };
    if p.next_attack > 0.0 {
        return;
    }
    let Some(active) = p.active_item else {
        return;
    };
    with_weapon(g, eng, slot, active, |b, w| b.item_pre_frame(w));
}

/// `CBasePlayer::ItemPostFrame` (`player.cpp` 7435): a shield bearer's reload is cut short by
/// IN_ATTACK2 (`m_flNextAttack = 0`); nothing runs while `m_flNextAttack` is above 0;
/// then `ImpulseCommands` and the active item's `ItemPostFrame`. The `m_pTank` check
/// (func_tank) is not ported.
pub(crate) fn player_item_post_frame(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let id = eng.client_edict(slot);
    let button = eng.ent(id).map_or(0, |v| v.button);
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    if p.active_item.is_some() && p.owns_shield && p.is_reloading() && button & IN_ATTACK2 != 0 {
        p.next_attack = 0.0;
    }
    if p.next_attack > 0.0 {
        return;
    }
    crate::player::impulse_commands(g, eng, slot);
    let Some(active) = g.players[usize::from(slot)].as_ref().and_then(|p| p.active_item) else {
        return;
    };
    with_weapon(g, eng, slot, active, |b, w| {
        item_post_frame(b, w);
        b.item_post_frame_extra(w);
    });
}

/// The end of `CBasePlayer::PostThink` (`pt_end`, `player.cpp` 5481-5513): the timers of
/// every weapon that the client predicts count down by the frame time (the next primary
/// attack to at least -1, the secondary and the idle to at least -0.001, an idle of 1000 left
/// alone), then the player's `m_flNextAttack` (to at least -0.001).
pub(crate) fn decay_timers(g: &mut GameState, eng: &EngineApi<'_>, slot: u8) {
    let frametime = eng.frametime();
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    for item in p.items.iter_mut().flatten() {
        if !item.behavior.use_decrement() {
            continue;
        }
        let w = &mut item.weapon;
        w.next_primary_attack = (w.next_primary_attack - frametime).max(-1.0);
        w.next_secondary_attack = (w.next_secondary_attack - frametime).max(-0.001);
        if w.time_weapon_idle != 1000.0 {
            w.time_weapon_idle = (w.time_weapon_idle - frametime).max(-0.001);
        }
    }
    p.next_attack -= frametime;
    // `if (m_flNextAttack < -0.001)`: the float is compared as a double.
    if f64::from(p.next_attack) < -0.001 {
        p.next_attack = -0.001_f64 as f32;
    }
}

/// `CBasePlayerWeapon::ItemPostFrame` (`weapons.cpp` 981-1204, vanilla): the order of the
/// source, statement by statement.
///
/// 1. The usable buttons: IN_ATTACK2 only if the weapon has a secondary attack.
/// 2. A Glock burst in progress fires its next shot (`m_flGlock18Shoot != 0`; QUIRK: no time
///    test, so the shot follows on the next command, whatever `FireRemaining` wrote into the
///    timer), else a Famas burst when server time is past `m_flFamasShoot` (absolute time).
/// 3. At `m_flNextPrimaryAttack <= 0` a scope that a shot closed opens again (`m_bResumeZoom`).
/// 4. The late brass (`m_flEjectBrass`, absolute time).
/// 5. No IN_ATTACK: `m_flLastFireTime = 0`.
/// 6. A shield bearer reloading who presses IN_ATTACK2 cancels the reload with the shield's
///    secondary attack.
/// 7. A reload completes when `m_flNextAttack <= 0`: the clip takes `min(max - clip, reserve)`.
/// 8. Then exactly one of: secondary attack (IN_ATTACK2 and the timer spent), primary attack
///    (IN_ATTACK and the timer spent; it needs `m_bCanShoot`, no freeze period, no defusing;
///    underwater a `NOFIREUNDERWATER` weapon only clicks and waits 0.15), reload (IN_RELOAD, a
///    clip, not reloading, and the primary timer STRICTLY below 0; not during a burst, not with
///    the shield up), or no fire button (the delayed-fire bookkeeping, `m_fFireOnEmpty = 0`,
///    the pistols' `m_iShotsFired = 0` or the others' slow decay, the auto reload of an empty
///    clip, and `WeaponIdle`, which returns).
/// 9. Whatever branch fell through: `WeaponIdle` if `ShouldWeaponIdle`.
///
/// QUIRKS kept: vanilla does not reset `m_flAccuracy` when the shots fall to 0 (that is
/// `REGAMEDLL_FIXES`); the `!IsUseable()` branch is `#if 0` in the source, so it does nothing
/// and the auto reload is skipped for an unusable weapon only because the `else` is not taken.
pub fn item_post_frame(b: &mut dyn WeaponBehavior, w: &mut WeaponCtx<'_, '_>) {
    let mut usable = w.button();
    if !w.has_secondary_attack(b) {
        usable &= !IN_ATTACK2;
    }

    if w.weapon.glock18_shoot != 0.0 {
        w.fire_remaining(true);
    } else if w.now() > w.weapon.famas_shoot && w.weapon.famas_shoot != 0.0 {
        w.fire_remaining(false);
    }

    // Return zoom level back to the one before we fired a shot (the AWP and the scout).
    if w.weapon.next_primary_attack <= 0.0 && w.player().resume_zoom {
        let id = w.edict();
        let p = w.player_mut();
        p.fov = p.last_zoom as u8;
        let fov = f32::from(p.fov);
        // `if (m_iFOV == m_iLastZoom) m_bResumeZoom = false`: always true right after the
        // assignment above, kept as written.
        if i32::from(p.fov) == p.last_zoom {
            p.resume_zoom = false;
        }
        if let Some(v) = w.eng.ent_mut(id) {
            v.fov = fov;
        }
    }

    let eject = w.player().eject_brass;
    if eject != 0.0 && eject <= w.now() {
        w.player_mut().eject_brass = 0.0;
        w.eject_brass_late();
    }

    if w.button() & IN_ATTACK == 0 {
        w.weapon.last_fire_time = 0.0;
    }

    if w.has_shield() && w.weapon.in_reload && w.button() & IN_ATTACK2 != 0 {
        b.secondary_attack(w);
        w.clear_button(IN_ATTACK2);
        w.weapon.in_reload = false;
        w.player_mut().next_attack = 0.0;
    }

    if w.weapon.in_reload && w.player().next_attack <= 0.0 {
        // Complete the reload.
        let j = (w.max_clip() - w.weapon.clip).min(w.ammo(w.weapon.primary_ammo_type));
        w.weapon.clip += j;
        let ammo_type = w.weapon.primary_ammo_type;
        w.add_ammo(ammo_type, -j);
        w.tabulate_ammo();
        w.weapon.in_reload = false;
    }

    let use_decrement = b.use_decrement();
    if usable & IN_ATTACK2 != 0
        && can_attack(w.weapon.next_secondary_attack, 0.0, use_decrement)
    {
        if w.info().is_some_and(|i| i.ammo2.is_some()) && w.ammo(w.weapon.secondary_ammo_type) == 0 {
            w.weapon.fire_on_empty = true;
        }
        b.secondary_attack(w);
        w.clear_button(IN_ATTACK2);
    } else if w.button() & IN_ATTACK != 0
        && can_attack(w.weapon.next_primary_attack, 0.0, use_decrement)
    {
        let has_ammo1 = w.info().is_some_and(|i| i.ammo1.is_some());
        if (w.weapon.clip == 0 && has_ammo1)
            || (w.max_clip() == WEAPON_NOCLIP && w.ammo(w.weapon.primary_ammo_type) == 0)
        {
            w.weapon.fire_on_empty = true;
        }
        w.tabulate_ammo();

        // Can't shoot during the freeze period. Always multiplayer here.
        let freeze = w.g.rules.is_freeze_period();
        let p = w.player();
        if p.can_shoot && !freeze && !p.is_defusing {
            let id = w.edict();
            let waterlevel = w.eng.ent(id).map_or(0, |v| v.waterlevel);
            // Don't fire underwater.
            if waterlevel == 3 && w.flags() & ITEM_FLAG_NOFIREUNDERWATER != 0 {
                w.play_empty_sound();
                w.weapon.next_primary_attack = w.get_next_attack_delay(0.15);
            } else {
                b.primary_attack(w);
            }
        }
    } else if w.button() & IN_RELOAD != 0
        && w.max_clip() != WEAPON_NOCLIP
        && !w.weapon.in_reload
        && w.weapon.next_primary_attack < 0.0
    {
        // Reload when reload is pressed, or if no buttons are down and the weapon is empty.
        if w.weapon.famas_shoot == 0.0
            && w.weapon.glock18_shoot == 0.0
            && w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN == 0
        {
            b.reload(w);
        }
    } else if usable & (IN_ATTACK | IN_ATTACK2) == 0 {
        // No fire buttons down. The code that follows keeps the player from tapping the fire
        // button repeatedly to simulate full auto and retain the single shot's accuracy.
        if w.weapon.delay_fire {
            w.weapon.delay_fire = false;
            if w.weapon.shots_fired > 15 {
                w.weapon.shots_fired = 15;
            }
            w.weapon.decrease_shots_fired = w.now() + 0.4;
        }
        w.weapon.fire_on_empty = false;

        // A pistol sets the shots fired to 0 after the player releases the button.
        if is_secondary_weapon(w.weapon.id) {
            w.weapon.shots_fired = 0;
        } else if w.weapon.shots_fired > 0 && w.weapon.decrease_shots_fired < w.now() {
            w.weapon.decrease_shots_fired = w.now() + 0.0225;
            w.weapon.shots_fired -= 1;
        }

        // `if (!IsUseable() && m_flNextPrimaryAttack < 0) { /* #if 0 */ } else { ... }`
        let unusable_wait = !w.is_useable() && w.weapon.next_primary_attack < 0.0;
        if !unusable_wait
            && w.weapon.weapon_state & WPNSTATE_SHIELD_DRAWN == 0
            && w.weapon.clip == 0
            && w.flags() & ITEM_FLAG_NOAUTORELOAD == 0
            && w.weapon.next_primary_attack < 0.0
            && w.weapon.famas_shoot == 0.0
            && w.weapon.glock18_shoot == 0.0
        {
            b.reload(w);
            return;
        }

        b.weapon_idle(w);
        return;
    }

    // Catch all.
    if b.should_weapon_idle() {
        b.weapon_idle(w);
    }
}

/// `CBasePlayer::SelectItem` for weapon `id` (`player.cpp` 6364, vanilla: no `CanHolster` or
/// `CanDeploy` test): false when the player has no such weapon or it is the active one.
/// Holsters the active item, makes this one active (with the shield down), deploys it and
/// resets the player's speed. (Task 12's inventory operations call it; until then only the
/// tests do.)
#[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
pub(crate) fn select_item(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, id: WeaponId) -> bool {
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    if p.find_item(id).is_none() || p.active_item == Some(id) {
        return false;
    }
    let previous = p.active_item;
    // FIX, this needs to queue them up and delay (the original's comment).
    if let Some(previous) = previous {
        with_weapon(g, eng, slot, previous, |b, w| b.holster(w));
    }
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    p.last_item = previous;
    p.active_item = Some(id);
    p.shield_drawn = false;
    if let Some((s, i)) = p.find_item(id) {
        p.items[s][i].weapon.weapon_state &= !WPNSTATE_SHIELD_DRAWN;
    }
    with_weapon(g, eng, slot, id, |b, w| b.deploy(w));
    crate::player::reset_max_speed(g, eng, slot);
    true
}

/// Gives the player of `slot` a weapon directly, without the pickup rules (`AddPlayerItem`,
/// `GiveNamedItem`: Task 12): `behavior` goes at the head of its inventory slot (a weapon of
/// the same id is replaced), with `clip` rounds loaded, its ammo types set from its item info
/// and `reserve` rounds of its primary ammo (not above the maximum). The player's `weapons`
/// bit is set, and the events of `CBasePlayerWeapon` itself are precached. False when the
/// player does not exist or the weapon has no inventory slot. (Task 12's inventory operations
/// replace it; until then only the tests call it.)
#[cfg_attr(not(feature = "test-hooks"), allow(dead_code))]
pub(crate) fn give_weapon(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    behavior: Box<dyn WeaponBehavior>,
    clip: i32,
    reserve: i32,
) -> bool {
    let id = behavior.id();
    let slot_index = item_slot(id);
    if slot_index == 0 || slot_index >= MAX_ITEM_TYPES {
        return false;
    }
    let events = super::weapon::WeaponEvents {
        glock18: eng.precache_event("events/glock18.sc"),
        famas: eng.precache_event("events/famas.sc"),
    };
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return false;
    };
    let mut item = WeaponSlotItem::new(behavior);
    item.weapon.clip = clip;
    item.weapon.events = events;
    if let Some(info) = item_info(id) {
        item.weapon.primary_ammo_type = info.ammo1.map_or(-1, ammo_index);
        item.weapon.secondary_ammo_type = info.ammo2.map_or(-1, ammo_index);
        if let Ok(i) = usize::try_from(item.weapon.primary_ammo_type)
            && i < MAX_AMMO_SLOTS
        {
            p.ammo[i] = reserve.min(info.max_ammo1);
        }
    }
    for list in &mut p.items {
        list.retain(|i| i.weapon.id != id);
    }
    p.items[slot_index].insert(0, item);
    p.weapons |= id.bit();
    tabulate_ammo(p);
    true
}

/// `CBasePlayer::SendAmmoUpdate` (`player.cpp` 7490): `AmmoX` for every ammo whose count the
/// client was not told.
pub(crate) fn send_ammo_update(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    let mut updates = Vec::new();
    for i in 0..MAX_AMMO_SLOTS {
        if p.ammo[i] != p.ammo_last[i] {
            p.ammo_last[i] = p.ammo[i];
            updates.push((i as u8, p.ammo[i].clamp(0, 255) as u8));
        }
    }
    for (index, amount) in updates {
        messages::ammo_x(eng, slot, index, amount);
    }
}

/// The weapons' part of `CBasePlayer::UpdateClientData` (`player.cpp` 7871-7878, and
/// `CBasePlayerWeapon::UpdateClientData`, `weapons.cpp` 1353): every carried weapon, slot by
/// slot, sends `CurWeapon` (state 1 for the active weapon, 0 for the others; the clip) when
/// the HUD is stale: nothing was sent since the spawn, the active item changed (to or from
/// this weapon), the clip or the state changed, or the FOV did. QUIRK: the byte of the
/// clip is `WRITE_BYTE(m_iClip)`, so a weapon with no clip (-1) sends 255.
pub(crate) fn update_items_client_data(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = g.players.get_mut(usize::from(slot)).and_then(Option::as_mut) else {
        return;
    };
    let fov_changed = p.fov != p.client_fov;
    let (active, client_active) = (p.active_item, p.client_active_item);
    let mut sends = Vec::new();
    for item in p.items.iter_mut().flatten() {
        let w = &mut item.weapon;
        let state = i32::from(active == Some(w.id));
        let mut send = !p.f_weapon;
        if (active == Some(w.id) || client_active == Some(w.id)) && active != client_active {
            send = true;
        }
        if w.clip != w.client_clip || state != w.client_weapon_state || fov_changed {
            send = true;
        }
        if send {
            sends.push((state as u8, w.id as i32 as u8, w.clip as u8));
            w.client_clip = w.clip;
            w.client_weapon_state = state;
            p.f_weapon = true;
        }
    }
    for (state, id, clip) in sends {
        messages::cur_weapon(eng, slot, state, id, clip);
    }
}
