//! The state of one weapon (`CBasePlayerWeapon`, ReGameDLL_CS `weapons.h`, read 2026-11-22), the
//! trait that carries what differs between the weapons (the virtuals of the C++ classes) and the
//! registry that makes the behaviour of a weapon id.
//!
//! A weapon a player carries is a [`WeaponSlotItem`]: its [`Weapon`] fields next to its
//! [`WeaponBehavior`]. The state machine that runs them is in [`super::core`].

use cs_engine::{EdictId, EngineApi};
use glam::Vec3;

use super::core::WeaponCtx;
use super::info::{WEAPONS_PRECACHE_ORDER, WeaponId, default_give, item_info};

/// `MAX_ITEM_TYPES`: the item slots of a player (`m_rgpPlayerItems`), indexed by `iItemSlot()`
/// (1 primary, 2 pistol, 3 knife, 4 grenades, 5 C4; 0 is not used).
pub const MAX_ITEM_TYPES: usize = 6;

/// The events `CBasePlayerWeapon` itself plays (`m_usFireGlock18`, `m_usFireFamas`, from
/// `FireRemaining`). The other weapons keep the ids of their own events in their own struct.
/// 0 is "not precached".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WeaponEvents {
    /// `m_usFireGlock18` (`events/glock18.sc`).
    pub glock18: u16,
    /// `m_usFireFamas` (`events/famas.sc`).
    pub famas: u16,
}

/// The fields of `CBasePlayerWeapon` (and the few its subclasses add that more than one weapon
/// needs). Timers are floats, as in C; the ones that count down are "seconds left"
/// (`UTIL_WeaponTimeBase() == 0`), see [`super::core`].
///
/// Not here, because the C code keeps them on the player: `m_bResumeZoom`, `m_iLastZoom` and
/// `m_flEjectBrass` (they are [`crate::player::Player`] fields), and `m_szAnimExtention`.
#[derive(Debug, Clone)]
pub struct Weapon {
    /// `m_iId`.
    pub id: WeaponId,
    /// `m_iPlayEmptySound`: set by `ResetEmptySound`, never cleared by `PlayEmptySound`
    /// (vanilla; the C memory starts at 0).
    pub play_empty_sound: bool,
    /// `m_fFireOnEmpty`.
    pub fire_on_empty: bool,
    /// `m_flNextPrimaryAttack`.
    pub next_primary_attack: f32,
    /// `m_flNextSecondaryAttack`.
    pub next_secondary_attack: f32,
    /// `m_flTimeWeaponIdle`.
    pub time_weapon_idle: f32,
    /// `m_iPrimaryAmmoType`: the player's ammo slot (`m_rgAmmo[]`).
    pub primary_ammo_type: i32,
    /// `m_iSecondaryAmmoType`.
    pub secondary_ammo_type: i32,
    /// `m_iClip`.
    pub clip: i32,
    /// `m_iClientClip`: the clip the HUD was last told.
    pub client_clip: i32,
    /// `m_iClientWeaponState`: the state the HUD was last told.
    pub client_weapon_state: i32,
    /// `m_fInReload`.
    pub in_reload: bool,
    /// `m_fInSpecialReload`: the shotgun's reload stage (0 none, 1 waiting, 2 loading).
    pub in_special_reload: i32,
    /// `m_iDefaultAmmo`.
    pub default_ammo: i32,
    /// `m_iShellId`: the model index of the brass.
    pub shell_id: i32,
    /// `m_fMaxSpeed`.
    pub max_speed: f32,
    /// `m_bDelayFire`.
    pub delay_fire: bool,
    /// `m_iDirection` (a `BOOL`: 0 or 1; the recoil's lateral direction).
    pub direction: i32,
    /// `m_bSecondarySilencerOn`.
    pub secondary_silencer_on: bool,
    /// `m_flAccuracy`.
    pub accuracy: f32,
    /// `m_flLastFire`.
    pub last_fire: f32,
    /// `m_iShotsFired`.
    pub shots_fired: i32,
    /// `m_vVecAiming`.
    pub vec_aiming: Vec3,
    /// `model_name`: the view model the weapon was last deployed with.
    pub model_name: String,
    /// `m_flGlock18Shoot`: server time of the next burst shot, 0 for none.
    pub glock18_shoot: f32,
    /// `m_iGlock18ShotsFired`.
    pub glock18_shots_fired: i32,
    /// `m_flFamasShoot`.
    pub famas_shoot: f32,
    /// `m_iFamasShotsFired`.
    pub famas_shots_fired: i32,
    /// `m_fBurstSpread`.
    pub burst_spread: f32,
    /// `m_iWeaponState` (`WPNSTATE_*`).
    pub weapon_state: i32,
    /// `m_flNextReload`.
    pub next_reload: f32,
    /// `m_flDecreaseShotsFired`.
    pub decrease_shots_fired: f32,
    /// `m_flPrevPrimaryAttack` (the "hle time creep" bookkeeping of `GetNextAttackDelay`).
    pub prev_primary_attack: f32,
    /// `m_flLastFireTime`.
    pub last_fire_time: f32,
    /// `m_flPumpTime` (M3, XM1014).
    pub pump_time: f32,
    /// `m_flStartThrow` (grenades).
    pub start_throw: f32,
    /// `m_flReleaseThrow` (grenades).
    pub release_throw: f32,
    /// `m_iSwing` (knife).
    pub swing: i32,
    /// `pev->body` of the weapon entity, the body group `SendWeaponAnim` sends.
    pub body: i32,
    /// The events the weapon plays itself.
    pub events: WeaponEvents,
    /// The world entity of the weapon while it lies in the world (dropped or armoury); `None`
    /// while a player carries it.
    pub ent: Option<EdictId>,
}

impl Weapon {
    /// A weapon as the C++ constructor leaves it (the entity's memory is zeroed), with the
    /// default ammo of its kind (`m_iDefaultAmmo`, the `*_DEFAULT_GIVE` of the weapon's
    /// `Spawn`).
    pub fn new(id: WeaponId) -> Self {
        Self {
            id,
            play_empty_sound: false,
            fire_on_empty: false,
            next_primary_attack: 0.0,
            next_secondary_attack: 0.0,
            time_weapon_idle: 0.0,
            primary_ammo_type: 0,
            secondary_ammo_type: 0,
            clip: 0,
            client_clip: 0,
            client_weapon_state: 0,
            in_reload: false,
            in_special_reload: 0,
            default_ammo: default_give(id).unwrap_or(0),
            shell_id: 0,
            max_speed: 0.0,
            delay_fire: false,
            direction: 0,
            secondary_silencer_on: false,
            accuracy: 0.0,
            last_fire: 0.0,
            shots_fired: 0,
            vec_aiming: Vec3::ZERO,
            model_name: String::new(),
            glock18_shoot: 0.0,
            glock18_shots_fired: 0,
            famas_shoot: 0.0,
            famas_shots_fired: 0,
            burst_spread: 0.0,
            weapon_state: 0,
            next_reload: 0.0,
            decrease_shots_fired: 0.0,
            prev_primary_attack: 0.0,
            last_fire_time: 0.0,
            pump_time: 0.0,
            start_throw: 0.0,
            release_throw: 0.0,
            swing: 0,
            body: 0,
            events: WeaponEvents::default(),
            ent: None,
        }
    }
}

/// Lets a `Box<dyn WeaponBehavior>` be cloned (a [`crate::player::Player`] is `Clone`). Every
/// behaviour that is `Clone` gets it; a behaviour only needs `#[derive(Clone)]`.
pub trait WeaponClone {
    /// A copy of the behaviour with its state.
    fn clone_box(&self) -> Box<dyn WeaponBehavior>;
}

impl<T: WeaponBehavior + Clone> WeaponClone for T {
    fn clone_box(&self) -> Box<dyn WeaponBehavior> {
        Box::new(self.clone())
    }
}

/// What differs between the weapons: the virtuals of `CBasePlayerItem`/`CBasePlayerWeapon` the
/// weapon classes override. The state machine ([`super::core::item_post_frame`]) calls them with
/// a [`WeaponCtx`] that holds the game, the engine, the player and the weapon's [`Weapon`].
///
/// A weapon that C++ calls `CBasePlayerWeapon::X` from inside its own `X` calls the same-named
/// method on the context (`default_deploy`, `default_reload`, ...); where the C++ base method
/// calls a virtual back (`DefaultDeploy` -> `CanDeploy`, `ResetMaxSpeed` -> `GetMaxSpeed`) the
/// context method takes the behaviour as a parameter (`me`), because the behaviour is borrowed
/// by the caller.
pub trait WeaponBehavior: Send + WeaponClone + 'static {
    /// `m_iId`.
    fn id(&self) -> WeaponId;
    /// `Precache`: the models, sounds and events of the weapon.
    fn precache(&mut self, _eng: &mut EngineApi<'_>) {}
    /// `Deploy`; false when the weapon cannot be drawn.
    fn deploy(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool;
    /// `PrimaryAttack`.
    fn primary_attack(&mut self, _w: &mut WeaponCtx<'_, '_>) {}
    /// `SecondaryAttack`.
    fn secondary_attack(&mut self, _w: &mut WeaponCtx<'_, '_>) {}
    /// `Reload`.
    fn reload(&mut self, _w: &mut WeaponCtx<'_, '_>) {}
    /// `WeaponIdle`.
    fn weapon_idle(&mut self, _w: &mut WeaponCtx<'_, '_>) {}
    /// `Holster`; the default is `CBasePlayerWeapon::Holster` (cancels the reload, no models).
    fn holster(&mut self, w: &mut WeaponCtx<'_, '_>) {
        w.base_holster();
    }
    /// `PlayEmptySound`: the dry-fire click. The default is `CBasePlayerWeapon::PlayEmptySound`;
    /// the M3 and the XM1014 override it to clear `m_iPlayEmptySound` after the click.
    fn play_empty_sound(&mut self, w: &mut WeaponCtx<'_, '_>) -> bool {
        w.play_empty_sound()
    }
    /// `GetMaxSpeed`.
    fn max_speed(&self, _w: &Weapon) -> f32 {
        250.0
    }
    /// `UseDecrement`: whether the client predicts the weapon, so its timers count down
    /// (`CLIENT_WEAPONS`: every weapon of the game).
    fn use_decrement(&self) -> bool {
        true
    }
    /// `IsPistol`.
    fn is_pistol(&self) -> bool {
        false
    }
    /// `HasSecondaryAttack` of the weapon (the shield is the context's business). The default
    /// is the table of `CBasePlayerWeapon::HasSecondaryAttack`.
    fn has_secondary_attack(&self, w: &Weapon) -> bool {
        super::core::has_secondary_attack_by_id(w.id)
    }
    /// A hook run in `ItemPostFrame` after the base class's work, for a weapon whose class
    /// adds to it: the snipers use it to resume the zoom after a reload ([`super::snipers`]).
    fn item_post_frame_extra(&mut self, _w: &mut WeaponCtx<'_, '_>) {}
    /// `ItemPreFrame` (no weapon overrides it).
    fn item_pre_frame(&mut self, _w: &mut WeaponCtx<'_, '_>) {}
    /// `CanDeploy` (the grenades and the C4 override it).
    fn can_deploy(&self, _w: &WeaponCtx<'_, '_>) -> bool {
        true
    }
    /// `CanDrop` (the knife and the three grenades say no).
    fn can_drop(&self, _w: &WeaponCtx<'_, '_>) -> bool {
        true
    }
    /// `CanHolster`.
    fn can_holster(&self, _w: &WeaponCtx<'_, '_>) -> bool {
        true
    }
    /// `ShouldWeaponIdle` (`CBasePlayerWeapon`: false).
    fn should_weapon_idle(&self) -> bool {
        false
    }
}

/// A weapon in a player's item list: its fields and its behaviour.
pub struct WeaponSlotItem {
    /// The state.
    pub weapon: Weapon,
    /// The behaviour.
    pub behavior: Box<dyn WeaponBehavior>,
    /// True for the placeholder that stands in the list while the item is checked out for a
    /// call (see [`super::core::with_weapon`]).
    pub(crate) vacant: bool,
}

impl WeaponSlotItem {
    /// An item for `behavior`, with a fresh [`Weapon`] of its id.
    pub fn new(behavior: Box<dyn WeaponBehavior>) -> Self {
        let weapon = Weapon::new(behavior.id());
        Self { weapon, behavior, vacant: false }
    }

    /// The placeholder of a checked-out item.
    pub(crate) fn vacant(id: WeaponId) -> Self {
        Self { weapon: Weapon::new(id), behavior: Box::new(Vacant(id)), vacant: true }
    }
}

impl Clone for WeaponSlotItem {
    fn clone(&self) -> Self {
        Self {
            weapon: self.weapon.clone(),
            behavior: self.behavior.clone_box(),
            vacant: self.vacant,
        }
    }
}

impl std::fmt::Debug for WeaponSlotItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WeaponSlotItem").field("weapon", &self.weapon).finish_non_exhaustive()
    }
}

/// The behaviour of a placeholder: nothing.
#[derive(Debug, Clone)]
struct Vacant(WeaponId);

impl WeaponBehavior for Vacant {
    fn id(&self) -> WeaponId {
        self.0
    }
    fn deploy(&mut self, _w: &mut WeaponCtx<'_, '_>) -> bool {
        false
    }
}

/// The behaviour of weapon `id`: every real weapon of the game (the 25 guns, the knife, the
/// three grenades, and the C4), the stub of it until its task fills it in. `None` for ids that
/// are not weapons (`WEAPON_NONE`, the unused `WEAPON_GLOCK`, the shield).
pub fn new_weapon(id: WeaponId) -> Option<Box<dyn WeaponBehavior>> {
    use super::{c4, grenades, knife, pistols, rifles, shotguns, smgs, snipers};
    use WeaponId::*;
    Some(match id {
        P228 => Box::new(pistols::P228),
        Glock18 => Box::new(pistols::Glock18),
        Usp => Box::new(pistols::Usp),
        Deagle => Box::new(pistols::Deagle),
        FiveSeven => Box::new(pistols::FiveSeven),
        Elite => Box::new(pistols::Elite),
        Mac10 => Box::new(smgs::Mac10),
        Tmp => Box::new(smgs::Tmp),
        Mp5n => Box::new(smgs::Mp5n),
        Ump45 => Box::new(smgs::Ump45),
        P90 => Box::new(smgs::P90),
        Galil => Box::new(rifles::Galil),
        Famas => Box::new(rifles::Famas),
        Ak47 => Box::new(rifles::Ak47),
        M4a1 => Box::new(rifles::M4a1),
        Aug => Box::new(rifles::Aug),
        Sg552 => Box::new(rifles::Sg552),
        M249 => Box::new(rifles::M249),
        Scout => Box::new(snipers::Scout),
        Awp => Box::new(snipers::Awp),
        G3sg1 => Box::new(snipers::G3sg1),
        Sg550 => Box::new(snipers::Sg550),
        M3 => Box::new(shotguns::M3),
        Xm1014 => Box::new(shotguns::Xm1014),
        Knife => Box::new(knife::Knife),
        HeGrenade => Box::new(grenades::HeGrenade),
        Flashbang => Box::new(grenades::Flashbang),
        SmokeGrenade => Box::new(grenades::SmokeGrenade),
        C4 => Box::new(c4::C4::default()),
        WeaponId::None | Glock | ShieldGun => return Option::None,
    })
}

/// `WeaponsPrecache`: runs `Precache` of every weapon in the order the game does
/// ([`WEAPONS_PRECACHE_ORDER`]). The server's precache (`crate::inventory`) does not call it:
/// it walks the same order itself, to put the ammo boxes' resources where the source precaches
/// them. This one is the plain list, for a test that wants the weapons' resources alone.
pub fn precache_all(eng: &mut EngineApi<'_>) {
    for id in WEAPONS_PRECACHE_ORDER {
        if let Some(mut behavior) = new_weapon(id) {
            behavior.precache(eng);
        }
    }
}

/// The inventory slot (`iItemSlot()`, 1 to 5) of a weapon: the `ItemInfo` slot plus one. 0 for
/// an id with no item info.
pub fn item_slot(id: WeaponId) -> usize {
    item_info(id).map_or(0, |i| (i.slot + 1) as usize)
}
