//! What a player carries and what lies on the ground: the item list operations of `CBasePlayer`
//! (`AddPlayerItem`, `RemovePlayerItem`, `SwitchWeapon`, `SelectLastItem`, `GiveNamedItem`,
//! `RemoveAllItems`, `GiveDefaultItems`, `DropPlayerItem`, `PackDeadPlayerItems`), the rules'
//! `FShouldSwitchWeapon` and `GetNextBestWeapon`, the commands that use them (`drop`, `use`,
//! `weapon_*`, `lastinv`), the entities that hold weapons in the world (`weaponbox`, the
//! `armoury_entity`, a `weapon_*` placed by a map) and the weapon list the HUD is told at sign-on.
//! Ported from ReGameDLL_CS `player.cpp`, `weapons.cpp`, `multiplay_gamerules.cpp`,
//! `gamerules.cpp`, `client.cpp` and `ammo.cpp`, read 2026-11-22, the vanilla build (no
//! `REGAMEDLL_FIXES`, `REGAMEDLL_ADD`, `BUILD_LATEST`).
//!
//! ## How a weapon in the world is kept
//!
//! A carried weapon is a [`WeaponSlotItem`] in the player's list (see [`crate::weapons`]); the C
//! code also keeps an entity for it that follows the player. Here a weapon has an edict only
//! while it lies in the world: inside a [`WeaponBox`] (the item is moved in and out of the box)
//! or as a [`WorldWeapon`]. `GiveNamedItem` of a weapon makes the item and touches the player
//! with it at once, with no entity in between.
//!
//! Not ported, each named where it matters: the shield (`DropShield`, the shield pickup rules,
//! `ITEM_FLAG_CUSTOM` handling aside) and the VIP's limits (neither exists), the C4 (phase 2E), the
//! `game_player_equip` entity of `OnSpawnEquip`, bots, the tutor, and the cheat impulses (101, 102, ...).

use cs_engine::consts::{FL_ONGROUND, MOVETYPE_TOSS, SOLID_BBOX, SOLID_TRIGGER};
use cs_engine::event_flags::{FEV_GLOBAL, FEV_RELIABLE};
use cs_engine::{EdictId, EngineApi, MsgDest};
use cs_proto48::usermsg::CsUserMsg;
use glam::Vec3;

use crate::GameState;
use crate::chat::client_print;
use crate::entity::{GameEntity, attach};
use crate::menus::HUD_PRINTCENTER;
use crate::messages;
use crate::player::{
    ATTN_NORM, CHAN_ITEM, CHAN_VOICE, DEAD_NO, EF_NODRAW, MAX_PLAYERS, PITCH_NORM, Player, Team,
    player_slot,
};
use crate::weapons::core::{give_ammo, select_item, with_weapon};
use crate::weapons::weapon::item_slot;
use crate::weapons::{
    ITEM_FLAG_EXHAUSTIBLE, ITEM_FLAG_EXHAUST_SECONDARYAMMO, ITEM_FLAG_NOFIREUNDERWATER, MAX_AMMO_SLOTS,
    MAX_ITEM_TYPES, MAX_WEAPONS, WEAPON_NOCLIP, WeaponEvents, WeaponId, WeaponSlotItem, ammo_index,
    item_info, item_infos, max_ammo_carry, new_weapon,
};

/// The inventory slots (`InventorySlotType`).
pub(crate) const PRIMARY_WEAPON_SLOT: usize = 1;
/// `PISTOL_SLOT`.
pub(crate) const PISTOL_SLOT: usize = 2;
/// `KNIFE_SLOT`.
pub(crate) const KNIFE_SLOT: usize = 3;
/// `GRENADE_SLOT`.
pub(crate) const GRENADE_SLOT: usize = 4;

/// `ITEM_KILL_DELAY` (`gamerules.h`): the seconds a dropped weapon box lies in the world.
pub(crate) const ITEM_KILL_DELAY: f32 = 300.0;
/// `ITEM_FLAG_CUSTOM`: the flags `WriteSigonMessages` hides from the client.
const ITEM_FLAG_CUSTOM: i32 = ITEM_FLAG_NOFIREUNDERWATER | ITEM_FLAG_EXHAUST_SECONDARYAMMO;
/// The most a client's command argument is looked at.
const MAX_COMMAND_NAME: usize = 64;
/// How often a weapon box looks at itself (to grow its touch box once it lies on the ground).
const BOX_THINK_INTERVAL: f64 = 0.1;

// ---- the sign-on messages ----

/// `WriteSigonMessages` (`client.cpp` 235): the HUD's table of weapons, one `WeaponList` per
/// weapon of the item table in the order of the weapon ids, into the init buffer every client
/// entering the server is sent. The ammo types are registry indices (a byte of -1, 255, for
/// none) and the flags are `iFlags` without the two custom bits (`PLAY_GAMEDLL`).
pub(crate) fn write_sigon_messages(eng: &mut EngineApi<'_>) {
    for number in 0..MAX_WEAPONS {
        let Some(info) = item_infos().iter().find(|i| i.id as i32 == number as i32 && number != 0)
        else {
            continue;
        };
        let msg = CsUserMsg::WeaponList {
            name: info.name.to_string(),
            ammo1_type: ammo_index_of(info.ammo1) as u8,
            ammo1_max: info.max_ammo1 as u8,
            ammo2_type: ammo_index_of(info.ammo2) as u8,
            ammo2_max: info.max_ammo2 as u8,
            slot: info.slot as u8,
            position: info.position as u8,
            weapon_id: info.id as i32 as u8,
            flags: (info.flags & !ITEM_FLAG_CUSTOM) as u8,
        };
        eng.send_usermsg(MsgDest::Init, &msg);
    }
}

/// `CBasePlayer::GetAmmoIndex` of an optional ammo name: -1 for none or unknown.
fn ammo_index_of(name: Option<&str>) -> i32 {
    name.map_or(-1, ammo_index)
}

// ---- the models and the precache ----

/// `GetCSModelName` (`player.cpp` 166): the model of a weapon lying in the world. `None` for an
/// id the original does not handle (it logs and makes no weapon box).
pub(crate) fn cs_model_name(id: WeaponId) -> Option<&'static str> {
    use WeaponId::*;
    Some(match id {
        P228 => "models/w_p228.mdl",
        Scout => "models/w_scout.mdl",
        HeGrenade => "models/w_hegrenade.mdl",
        Xm1014 => "models/w_xm1014.mdl",
        C4 => "models/w_backpack.mdl",
        Mac10 => "models/w_mac10.mdl",
        Aug => "models/w_aug.mdl",
        SmokeGrenade => "models/w_smokegrenade.mdl",
        Elite => "models/w_elite.mdl",
        FiveSeven => "models/w_fiveseven.mdl",
        Ump45 => "models/w_ump45.mdl",
        Sg550 => "models/w_sg550.mdl",
        Galil => "models/w_galil.mdl",
        Famas => "models/w_famas.mdl",
        Usp => "models/w_usp.mdl",
        Glock18 => "models/w_glock18.mdl",
        Awp => "models/w_awp.mdl",
        Mp5n => "models/w_mp5.mdl",
        M249 => "models/w_m249.mdl",
        M3 => "models/w_m3.mdl",
        M4a1 => "models/w_m4a1.mdl",
        Tmp => "models/w_tmp.mdl",
        G3sg1 => "models/w_g3sg1.mdl",
        Flashbang => "models/w_flashbang.mdl",
        Deagle => "models/w_deagle.mdl",
        Sg552 => "models/w_sg552.mdl",
        Ak47 => "models/w_ak47.mdl",
        Knife => "models/w_knife.mdl",
        P90 => "models/w_p90.mdl",
        ShieldGun => "models/w_shield.mdl",
        WeaponId::None | Glock => return Option::None,
    })
}

/// The sounds of `WeaponsPrecache` that belong to no weapon (`weapons.cpp` 411-423), in the
/// source's order (`items/9mmclip1.wav` and `items/gunpickup2.wav` come earlier, from the ammo and
/// the battery).
const WEAPONS_SOUNDS: [&str; 10] = [
    "weapons/debris1.wav",
    "weapons/debris2.wav",
    "weapons/debris3.wav",
    "weapons/grenade_hit1.wav",
    "weapons/grenade_hit2.wav",
    "weapons/grenade_hit3.wav",
    "weapons/bullet_hit1.wav",
    "weapons/bullet_hit2.wav",
    "items/weapondrop1.wav",
    "weapons/generic_reload.wav",
];

/// The sprites and models of `WeaponsPrecache` that belong to no weapon (`weapons.cpp`
/// 381-402): explosions, smoke, blood, the radio icon, the laser.
const WEAPONS_MODELS: [&str; 18] = [
    "sprites/zerogxplode.spr",
    "sprites/WXplo1.spr",
    "sprites/steam1.spr",
    "sprites/bubble.spr",
    "sprites/bloodspray.spr",
    "sprites/blood.spr",
    "sprites/smokepuff.spr",
    "sprites/eexplo.spr",
    "sprites/fexplo.spr",
    "sprites/fexplo1.spr",
    // `g_sModelIndexRadio`: the icon over a player that used the radio.
    crate::hud_misc::RADIO_SPRITE,
    "sprites/b-tele1.spr",
    "sprites/c-tele1.spr",
    "sprites/ledglow.spr",
    "sprites/laserbeam.spr",
    "sprites/laserdot.spr",
    "models/grenade.mdl",
    "sprites/explode1.spr",
];

/// Every `PRECACHE_EVENT(1, ...)` of the weapons, in the order `WeaponsPrecache` runs the weapons'
/// `Precache` in ([`crate::weapons::WEAPONS_PRECACHE_ORDER`]): the index a client plays an
/// event by is its place here.
pub const WEAPON_EVENTS: [&str; 28] = [
    "events/awp.sc",
    "events/g3sg1.sc",
    "events/ak47.sc",
    "events/scout.sc",
    "events/m249.sc",
    "events/m4a1.sc",
    "events/sg552.sc",
    "events/aug.sc",
    "events/sg550.sc",
    "events/m3.sc",
    "events/xm1014.sc",
    "events/usp.sc",
    "events/mac10.sc",
    "events/ump45.sc",
    "events/fiveseven.sc",
    "events/p90.sc",
    "events/deagle.sc",
    "events/p228.sc",
    "events/knife.sc",
    "events/glock18.sc",
    "events/mp5n.sc",
    "events/tmp.sc",
    "events/elite_left.sc",
    "events/elite_right.sc",
    "events/createexplo.sc",
    "events/createsmoke.sc",
    "events/galil.sc",
    "events/famas.sc",
];

/// What the world items `WeaponsPrecache` precaches first precache (weapons.cpp 315-322; ReGameDLL
/// `items.cpp`, read 2026-11-22): `item_suit` (the kevlar model and its pickup sound), `item_battery`
/// (the model and the gun pickup sound), `item_antidote`, `item_security`, `item_longjump`,
/// `item_kevlar` (its model is already there), `item_assaultsuit` and `item_thighpack`: the model
/// and the sound, if any.
const ITEM_PRECACHE: [(&str, Option<&str>); 7] = [
    ("models/w_kevlar.mdl", Some("items/tr_kevlar.wav")),
    ("models/w_battery.mdl", Some("items/gunpickup2.wav")),
    ("models/w_antidote.mdl", None),
    ("models/w_security.mdl", None),
    ("models/w_longjump.mdl", None),
    ("models/w_assault.mdl", None),
    ("models/w_thighpack.mdl", None),
];

/// `WeaponsPrecache` (`weapons.cpp` 300-425, called by `CWorld::Precache`): the weapons' own
/// `Precache` in the order of the game ([`crate::weapons::precache_all`]), the weapon box,
/// the models of the weapons lying in the world and of the armoury's items, the effects of
/// the explosions, and the sounds. Assumption: until the weapons' own ports precache their
/// `w_*` model (they do in the original), the models of [`cs_model_name`] are precached here
/// too (a name already there keeps its index), and the two events `CBasePlayerWeapon` itself
/// plays (`FireRemaining`) are precached, which the weapons need once the server is running (the events are precached here in the source's order
/// as well, [`WEAPON_EVENTS`], so a weapon that looks its event up by name always finds it).
pub(crate) fn weapons_precache(eng: &mut EngineApi<'_>) {
    // The common world objects (`UTIL_PrecacheOther("item_*")`) come first: the models of
    // the suit (`item_suit`: `CItemSuit::Precache` precaches the kevlar vest's model), the
    // battery, the antidote, the security item, the long jump module, the kevlar vest (already
    // there), the assault suit and the thigh pack, in that order.
    for (model, sound) in ITEM_PRECACHE {
        precache_model(eng, model);
        if let Some(sound) = sound
            && let Err(err) = eng.precache_sound(sound)
        {
            tracing::error!(sound, ?err, "sound not precached");
        }
    }
    // `precache_all` with the ammo boxes where the source precaches them: `ammo_338magnum`
    // after the AWP's group, `ammo_buckshot` after the XM1014's (the other ammo entities use
    // models that are already there).
    for id in crate::weapons::WEAPONS_PRECACHE_ORDER {
        if let Some(mut behavior) = crate::weapons::new_weapon(id) {
            behavior.precache(eng);
        }
        match id {
            // `C338MagnumAmmo::Precache` and `CBuckShotAmmo::Precache` (ammo.cpp, read 2026-11-22): a model and the
            // sound every ammo box plays.
            WeaponId::Awp => {
                precache_model(eng, "models/w_9mmclip.mdl");
                precache_ammo_sound(eng);
            }
            WeaponId::Xm1014 => {
                precache_model(eng, "models/w_shotbox.mdl");
                precache_ammo_sound(eng);
            }
            _ => {}
        }
    }
    for event in WEAPON_EVENTS {
        eng.precache_event(event);
    }
    for id in crate::weapons::WEAPONS_PRECACHE_ORDER {
        // The grenades' `w_` models are not their weapons' to precache: the original has
        // them in `ClientPrecache`, after the player models ([`crate::world`]).
        if matches!(id, WeaponId::Flashbang | WeaponId::HeGrenade | WeaponId::SmokeGrenade) {
            continue;
        }
        if let Some(model) = cs_model_name(id) {
            precache_model(eng, model);
        }
    }
    // `UTIL_PrecacheOther("weaponbox")` (`CWeaponBox::Precache`) comes after the weapons and
    // before the effects' sprites.
    precache_model(eng, "models/w_weaponbox.mdl");
    for model in WEAPONS_MODELS {
        precache_model(eng, model);
    }
    // (The armoury's models are its own to precache, in `Armoury::spawn`, as `CArmoury::Precache`
    // does: `WeaponsPrecache` does not.)
    for sound in WEAPONS_SOUNDS {
        if let Err(err) = eng.precache_sound(sound) {
            tracing::error!(sound, ?err, "sound not precached");
        }
    }
}

/// The pickup sound every ammo box precaches.
fn precache_ammo_sound(eng: &mut EngineApi<'_>) {
    if let Err(err) = eng.precache_sound("items/9mmclip1.wav") {
        tracing::error!(?err, "sound not precached");
    }
}

fn precache_model(eng: &mut EngineApi<'_>, name: &str) {
    if let Err(err) = eng.precache_model(name) {
        tracing::error!(model = %name, ?err, "model not precached");
    }
}

// ---- looking at the list ----

fn player_ref(g: &GameState, slot: u8) -> Option<&Player> {
    g.players.get(usize::from(slot)).and_then(Option::as_ref)
}

fn player_mut(g: &mut GameState, slot: u8) -> Option<&mut Player> {
    g.players.get_mut(usize::from(slot)).and_then(Option::as_mut)
}

/// The ids of the player's weapons, slot by slot, each slot's head first (the order the C
/// code walks `m_rgpPlayerItems` in).
fn item_ids(p: &Player) -> Vec<WeaponId> {
    p.items.iter().flatten().map(|i| i.weapon.id).collect()
}

/// `CBasePlayer::HasPlayerItem`: the player carries a weapon of the class of `id`.
pub fn has_player_item(p: &Player, id: WeaponId) -> bool {
    let slot = item_slot(id);
    p.items.get(slot).is_some_and(|list| list.iter().any(|i| i.weapon.id == id))
}

/// `CBasePlayer::GetItemByName`: the weapon whose classname is `name` (`FClassnameIs`, exact).
fn item_id_by_name(p: &Player, name: &str) -> Option<WeaponId> {
    let info = item_infos().iter().find(|i| i.name == name)?;
    has_player_item(p, info.id).then_some(info.id)
}

/// The weapon of classname `name` (a `weapon_*` entity class), whether or not anyone carries it.
fn weapon_id_of_class(name: &str) -> Option<WeaponId> {
    item_infos().iter().find(|i| i.name == name).map(|i| i.id)
}

/// Whether the player is alive (`pev->deadflag == DEAD_NO`).
fn is_alive(eng: &EngineApi<'_>, slot: u8) -> bool {
    eng.ent(eng.client_edict(slot)).is_some_and(|v| v.deadflag == DEAD_NO)
}

/// `CBasePlayer::HasWeapons`.
fn has_weapons(p: &Player) -> bool {
    p.items.iter().any(|l| !l.is_empty())
}

/// A new weapon as `CREATE_NAMED_ENTITY` and its `Spawn` leave it: the fields of
/// [`crate::weapons::Weapon::new`] (with `m_iDefaultAmmo` the weapon's `*_DEFAULT_GIVE`) and
/// the events `CBasePlayerWeapon` plays. `None` for an id that is not a weapon.
pub(crate) fn new_item(eng: &mut EngineApi<'_>, id: WeaponId) -> Option<WeaponSlotItem> {
    let behavior = new_weapon(id)?;
    let mut item = WeaponSlotItem::new(behavior);
    apply_spawn_fields(&mut item.weapon);
    item.weapon.events = WeaponEvents {
        glock18: eng.precache_event("events/glock18.sc"),
        famas: eng.precache_event("events/famas.sc"),
    };
    Some(item)
}

/// What each weapon class's `Spawn` sets on the entity (`wpn_*.cpp`, read 2026-11-22), besides
/// `m_iId` and `m_iDefaultAmmo` (which [`crate::weapons::Weapon::new`] sets): the starting
/// accuracy, `m_bDelayFire`, the knife's clip of -1, the grenades' `m_flReleaseThrow` of -1 and
/// the shield bit cleared. (The base-damage fields of `REGAMEDLL_API` are the weapons' own
/// constants.)
fn apply_spawn_fields(w: &mut crate::weapons::Weapon) {
    use WeaponId::*;
    match w.id {
        Ak47 | Aug | M249 | Sg552 | Tmp | P90 => w.accuracy = 0.2,
        M4a1 => {
            w.accuracy = 0.2;
            w.delay_fire = true;
        }
        Mac10 => w.accuracy = 0.15,
        Mp5n | Ump45 => w.accuracy = 0.0,
        Deagle | Glock18 | P228 | Sg550 => w.accuracy = 0.9,
        Elite => w.accuracy = 0.88,
        FiveSeven | Usp => w.accuracy = 0.92,
        Knife => w.clip = WEAPON_NOCLIP,
        HeGrenade | Flashbang | SmokeGrenade => w.release_throw = -1.0,
        _ => {}
    }
    w.weapon_state &= !crate::weapons::WPNSTATE_SHIELD_DRAWN;
}

// ---- the rules: what a player may pick up and switch to ----

/// `CGameRules::CanHaveAmmo` (`gamerules.cpp` 31): the ammo is known and the player carries
/// less than `max_carry` of it.
fn can_have_ammo(p: &Player, name: &str, max_carry: i32) -> bool {
    let index = ammo_index(name);
    usize::try_from(index)
        .ok()
        .and_then(|i| p.ammo.get(i))
        .is_some_and(|&have| have < max_carry)
}

/// `CGameRules::CanHavePlayerItem` (`gamerules.cpp` 72): only a living player may have an item;
/// one that cannot carry more ammo of the weapon's kind may have the weapon only if it has no
/// weapon of its class yet (and a weapon with no ammo, only if it has none of its class).
fn can_have_player_item(eng: &EngineApi<'_>, p: &Player, slot: u8, id: WeaponId) -> bool {
    if !is_alive(eng, slot) {
        return false;
    }
    if p.owns_shield && (item_slot(id) == PRIMARY_WEAPON_SLOT || id == WeaponId::Elite) {
        return false;
    }
    let Some(info) = item_info(id) else {
        // "will fall through to here if GetItemInfo doesn't fill the struct!"
        return true;
    };
    match info.ammo1 {
        Some(ammo) => {
            if !can_have_ammo(p, ammo, info.max_ammo1) && has_player_item(p, id) {
                return false;
            }
        }
        None => {
            if has_player_item(p, id) {
                return false;
            }
        }
    }
    true
}

/// `CHalfLifeMultiplay::FShouldSwitchWeapon` (`multiplay_gamerules.cpp` 3272): the new weapon
/// `id`, already in the list, is taken out at once when it can be deployed and the player has
/// no active item, or when the player's `_cl_autowepswitch` is not 0, its active item can be
/// put away and the new one is heavier. (`m_iAutoWepSwitch` is read from the userinfo as
/// `SetPrefsFromUserinfo` does: 1 when the key is missing.)
fn should_switch_weapon(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, id: WeaponId) -> bool {
    if !with_weapon(g, eng, slot, id, |b, w| b.can_deploy(w)).unwrap_or(false) {
        // That weapon can't deploy anyway.
        return false;
    }
    let Some(active) = player_ref(g, slot).and_then(|p| p.active_item) else {
        // Player doesn't have an active item!
        return true;
    };
    let auto_switch = match eng.client_key(slot, "_cl_autowepswitch") {
        Some(v) if !v.is_empty() => crate::menus::atoi(v),
        _ => 1,
    };
    if auto_switch == 0 {
        return false;
    }
    if !with_weapon(g, eng, slot, active, |b, w| b.can_holster(w)).unwrap_or(true) {
        // Can't put away the active item.
        return false;
    }
    let weight = |id| item_info(id).map_or(0, |i| i.weight);
    weight(id) > weight(active)
}

/// `CBasePlayer::SwitchWeapon` (`player.cpp` 8807): the weapon `id` is deployed in place of the
/// active one (which is put away and becomes the last item). False when `id` cannot be deployed.
pub(crate) fn switch_weapon(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, id: WeaponId) -> bool {
    if !with_weapon(g, eng, slot, id, |b, w| b.can_deploy(w)).unwrap_or(false) {
        return false;
    }
    let active = player_ref(g, slot).and_then(|p| p.active_item);
    if let Some(active) = active {
        with_weapon(g, eng, slot, active, |b, w| b.holster(w));
    }
    if let Some(p) = player_mut(g, slot) {
        let temp = p.active_item;
        p.active_item = Some(id);
        p.last_item = temp;
    }
    with_weapon(g, eng, slot, id, |b, w| b.deploy(w));
    // `pWeapon->m_pPlayer->ResetMaxSpeed()`.
    crate::player::reset_max_speed(g, eng, slot);
    true
}

/// `CHalfLifeMultiplay::GetNextBestWeapon` (`multiplay_gamerules.cpp` 3319): the player is rid
/// of `current` (dropped, spent) and switches to the heaviest other weapon it can deploy (the
/// first of equal weight in list order). False when `current` cannot be put away or there is
/// nothing else.
pub(crate) fn get_next_best_weapon(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    current: WeaponId,
) -> bool {
    if !with_weapon(g, eng, slot, current, |b, w| b.can_holster(w)).unwrap_or(true) {
        // Can't put this gun away right now, so can't switch.
        return false;
    }
    let ids = player_ref(g, slot).map(item_ids).unwrap_or_default();
    let mut best_weight = -1;
    let mut best = None;
    for id in ids {
        let weight = item_info(id).map_or(0, |i| i.weight);
        // Don't reselect the weapon we're trying to get rid of.
        if weight > best_weight
            && id != current
            && with_weapon(g, eng, slot, id, |b, w| b.can_deploy(w)).unwrap_or(false)
        {
            best_weight = weight;
            best = Some(id);
        }
    }
    let Some(best) = best else {
        return false;
    };
    switch_weapon(g, eng, slot, best);
    true
}

// ---- adding and removing ----

/// `CBasePlayerWeapon::ExtractAmmo(pWeapon)` of the new weapon `item` for the weapon `target`
/// the player already has (`AddDuplicate`), or for `item` itself (`AddWeapon`): the default
/// ammo of `item` goes to `target`'s clip first, then its reserve. The result is the C
/// function's: whether the player got ammo of the type.
fn extract_ammo_into_existing(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    item: &mut WeaponSlotItem,
    target: WeaponId,
) -> bool {
    let Some(info) = item_info(item.weapon.id) else {
        return false;
    };
    let mut res = false;
    if let Some(ammo1) = info.ammo1 {
        // Blindly call with `m_iDefaultAmmo`. It's either going to be a value or zero. If it is
        // zero, we only get the ammo in the weapon's clip, which is what we want.
        let count = item.weapon.default_ammo;
        res = with_weapon(g, eng, slot, target, |_, w| {
            w.add_primary_ammo(count, ammo1, info.max_clip, info.max_ammo1)
        })
        .unwrap_or(false);
        item.weapon.default_ammo = 0;
    }
    if let Some(ammo2) = info.ammo2 {
        res = with_weapon(g, eng, slot, target, |_, w| w.add_secondary_ammo(0, ammo2, info.max_ammo2))
            .unwrap_or(false);
    }
    res
}

/// `CBasePlayerWeapon::AddToPlayer` and `AddWeapon` (`weapons.cpp` 1324): the weapon's bit in
/// `pev->weapons`, its ammo types, its default ammo (`ExtractAmmo(this)`, run on the weapon
/// before it is in the list, so `AddPrimaryAmmo` does not play the "got ammo" sound), then the
/// `WeapPickup` of `CBasePlayerItem::AddToPlayer`.
fn add_to_player(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, item: &mut WeaponSlotItem) {
    let id = item.weapon.id;
    let Some(info) = item_info(id) else {
        return;
    };
    if let Some(p) = player_mut(g, slot) {
        p.weapons |= id.bit();
    }
    if item.weapon.primary_ammo_type == 0 {
        item.weapon.primary_ammo_type = ammo_index_of(info.ammo1);
        item.weapon.secondary_ammo_type = ammo_index_of(info.ammo2);
    }
    // `AddWeapon()`: `ExtractAmmo(this)`.
    let use_decrement = item.behavior.use_decrement();
    let mut ctx =
        crate::weapons::core::WeaponCtx::new(g, eng, slot, &mut item.weapon, use_decrement);
    if let Some(ammo1) = info.ammo1 {
        let count = ctx.weapon.default_ammo;
        ctx.add_primary_ammo(count, ammo1, info.max_clip, info.max_ammo1);
        ctx.weapon.default_ammo = 0;
    }
    if let Some(ammo2) = info.ammo2 {
        ctx.add_secondary_ammo(0, ammo2, info.max_ammo2);
    }
    eng.send_usermsg(MsgDest::One(slot), &CsUserMsg::WeapPickup(id as i32 as u8));
}

/// `CBasePlayerWeapon::AddDuplicate` (`weapons.cpp` 1311): the new weapon `item` meets one the
/// player already has. A weapon from the world (default ammo) gives its default ammo, a dropped
/// one (none) only what is in its clip (`ExtractClipAmmo`). The C result is an `int`: any
/// nonzero answer, `-1` included, counts as taken.
fn add_duplicate(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, item: &mut WeaponSlotItem) -> bool {
    let id = item.weapon.id;
    if item.weapon.default_ammo != 0 {
        return extract_ammo_into_existing(g, eng, slot, item, id);
    }
    // `ExtractClipAmmo`: a gun with no clip comes empty when second hand.
    let Some(info) = item_info(id) else {
        return false;
    };
    let ammo = if item.weapon.clip == WEAPON_NOCLIP { 0 } else { item.weapon.clip };
    let given = give_ammo(g, eng, slot, ammo, info.ammo1, info.max_ammo1);
    given != 0
}

/// `CBasePlayer::AddPlayerItem` (`player.cpp` 7260): the weapon `item` is added to the list.
/// A weapon of its class that the player already has takes the ammo of the new one instead and
/// the new one is gone (false). Otherwise it goes in at the head of its slot, the primary slot
/// sets `m_bHasPrimary`, and the player switches to it when the rules say so (true).
pub(crate) fn add_player_item(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    item: WeaponSlotItem,
) -> bool {
    matches!(try_add_player_item(g, eng, slot, item), AddResult::Added)
}

/// What [`try_add_player_item`] did with the weapon.
pub(crate) enum AddResult {
    /// It is in the player's list (`AddPlayerItem` was TRUE).
    Added,
    /// The player has a weapon of its class and took the new one's ammo (`AddDuplicate` was
    /// nonzero): the new weapon is spent (`Kill`).
    Taken,
    /// Nothing was taken (`AddDuplicate` was 0, or the weapon cannot be added at all): the weapon
    /// is given back and stays where it was.
    Refused(Box<WeaponSlotItem>),
}

/// `AddPlayerItem` that gives the weapon back when nothing came of it, for a weapon that lies in
/// the world (`DefaultTouch` keeps the entity unless `AddDuplicate` took its ammo).
pub(crate) fn try_add_player_item(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    mut item: WeaponSlotItem,
) -> AddResult {
    let id = item.weapon.id;
    let slot_index = item_slot(id);
    if slot_index == 0 || slot_index >= MAX_ITEM_TYPES {
        return AddResult::Refused(Box::new(item));
    }
    let Some(p) = player_ref(g, slot) else {
        return AddResult::Refused(Box::new(item));
    };
    if has_player_item(p, id) {
        // `PlayerGotWeapon`, `CheckRespawn` and `UpdateItemInfo` do nothing in vanilla.
        return if add_duplicate(g, eng, slot, &mut item) {
            AddResult::Taken
        } else {
            AddResult::Refused(Box::new(item))
        };
    }
    add_to_player(g, eng, slot, &mut item);
    let Some(p) = player_mut(g, slot) else {
        return AddResult::Refused(Box::new(item));
    };
    if slot_index == PRIMARY_WEAPON_SLOT {
        p.has_primary = true;
    }
    p.items[slot_index].insert(0, item);
    // `if (HasShield()) pev->gamestate = HITGROUP_SHIELD_ENABLED` has no shield to see.
    // Should we switch to this item?
    if should_switch_weapon(g, eng, slot, id) && !player_ref(g, slot).is_some_and(|p| p.shield_drawn) {
        switch_weapon(g, eng, slot, id);
    }
    AddResult::Added
}

/// `CBasePlayer::RemovePlayerItem` (`player.cpp` 7321): the weapon `id` leaves the list and is
/// returned. The active item is forgotten with its models (the last item is forgotten only
/// when it is not the active one, as vanilla's `else if` does). `None` when the player does not
/// carry it.
pub(crate) fn remove_player_item(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    id: WeaponId,
) -> Option<WeaponSlotItem> {
    let ent = eng.client_edict(slot);
    let p = player_mut(g, slot)?;
    let (s, i) = p.find_item(id)?;
    if p.active_item == Some(id) {
        p.active_item = None;
        if let Some(v) = eng.ent_mut(ent) {
            v.viewmodel = 0;
            v.weaponmodel = 0;
        }
    } else if p.last_item == Some(id) {
        p.last_item = None;
    }
    Some(p.items[s].remove(i))
}

/// `CBasePlayer::GiveNamedItem` for a weapon class (`player.cpp` 6560): a new weapon of class
/// `classname` is made and the player touches it (`CBasePlayerItem::DefaultTouch`, without the
/// VIP rule): when the rules let the player have it, it is added. A weapon the player cannot
/// take stays in the world in the original (`FallInit`); here it is dropped. False for a class
/// that is no weapon.
pub(crate) fn give_named_item(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, classname: &str) -> bool {
    let Some(id) = weapon_id_of_class(classname) else {
        return false;
    };
    let Some(item) = new_item(eng, id) else {
        return false;
    };
    default_touch(g, eng, slot, item)
}

/// `CBasePlayerItem::DefaultTouch` (`weapons.cpp` 610) for a weapon that is not in the world:
/// the player takes `item` when `CanHavePlayerItem` allows, with the pickup sound. True when
/// the weapon was added.
fn default_touch(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, item: WeaponSlotItem) -> bool {
    let id = item.weapon.id;
    let mut have_ctx = cs_mod_api::CanHavePlayerItemCtx {
        slot,
        weapon_id: id as i32 as u8,
    };
    if crate::mod_hooks::fire_hook(g, eng, |sink, g, eng| {
        sink.on_can_have_player_item(g, eng, cs_mod_api::HookPhase::Pre, &mut have_ctx)
    }) == cs_mod_api::HookAction::Supercede
    {
        return false;
    }
    let Some(p) = player_ref(g, slot) else {
        return false;
    };
    if !can_have_player_item(eng, p, slot, id) {
        return false;
    }
    if add_player_item(g, eng, slot, item) {
        let ent = eng.client_edict(slot);
        eng.emit_sound(ent, CHAN_ITEM, "items/gunpickup2.wav", 1.0, ATTN_NORM, PITCH_NORM);
        let _ = crate::mod_hooks::fire_hook(g, eng, |sink, g, eng| {
            sink.on_can_have_player_item(g, eng, cs_mod_api::HookPhase::Post, &mut have_ctx)
        });
        return true;
    }
    false
}

/// `CBasePlayer::RemoveAllItems(removeSuit)` (`player.cpp` 1846, vanilla): the active weapon is
/// put away, every weapon and all the ammo go, `pev->weapons` keeps only the suit unless
/// `remove_suit`, the client is brought up to date (`UpdateClientData`) and told there is no
/// weapon: `CurWeapon 0 0 0`.
pub(crate) fn remove_all_items(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, remove_suit: bool) {
    if player_ref(g, slot).is_some_and(|p| p.has_defuser) {
        crate::player::remove_defuser(g, eng, slot);
    }
    if player_ref(g, slot).is_some_and(|p| p.has_c4) {
        if let Some(p) = player_mut(g, slot) {
            p.has_c4 = false;
        }
        if let Some(v) = eng.ent_mut(eng.client_edict(slot)) {
            v.body = 0;
        }
        crate::buy::set_bomb_icon(g, eng, slot, false);
        crate::player::set_progress_bar_time(g, eng, slot, 0);
    }
    // `RemoveShield()`: no shield exists.
    if let Some(p) = player_mut(g, slot) {
        p.owns_shield = false;
        p.shield_drawn = false;
    }
    let active = player_ref(g, slot).and_then(|p| p.active_item);
    if let Some(active) = active {
        // `ResetAutoaim(); m_pActiveItem->Holster();`
        with_weapon(g, eng, slot, active, |b, w| b.holster(w));
        if let Some(p) = player_mut(g, slot) {
            p.active_item = None;
        }
    }
    let ent = eng.client_edict(slot);
    let Some(p) = player_mut(g, slot) else {
        return;
    };
    p.last_item = None;
    for list in &mut p.items {
        // `Drop()`: the weapons are removed.
        list.clear();
    }
    p.active_item = None;
    p.has_primary = false;
    if let Some(v) = eng.ent_mut(ent) {
        v.viewmodel = 0;
        v.weaponmodel = 0;
    }
    // `pev->weapons &= ~WEAPON_ALLWEAPONS`, where that is everything but the suit's bit.
    if remove_suit {
        p.weapons = 0;
    } else {
        p.weapons &= 1u32 << 31;
    }
    p.ammo = [0; MAX_AMMO_SLOTS];
    messages::update_client_data(g, eng, slot);
    messages::cur_weapon_none(eng, slot);
}

/// `CBasePlayer::GiveDefaultItems` (`player.cpp` 1667, vanilla): everything goes, then a knife,
/// and a USP with 24 .45 ACP (12 for the VIP) for a counter-terrorist or a Glock 18 with 40 rounds
/// of 9 mm for a terrorist.
pub(crate) fn give_default_items(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    remove_all_items(g, eng, slot, false);
    // `m_bHasPrimary = false` is already done by `RemoveAllItems`.
    let Some((team, is_vip)) = player_ref(g, slot).map(|p| (p.team, p.is_vip)) else {
        return;
    };
    match team {
        crate::player::Team::Ct => {
            give_named_item(g, eng, slot, "weapon_knife");
            give_named_item(g, eng, slot, "weapon_usp");
            give_ammo(g, eng, slot, if is_vip { 12 } else { 24 }, Some("45acp"), -1);
        }
        crate::player::Team::Terrorist => {
            give_named_item(g, eng, slot, "weapon_knife");
            give_named_item(g, eng, slot, "weapon_glock18");
            give_ammo(g, eng, slot, 40, Some("9mm"), -1);
        }
        _ => {}
    }
}

/// `CBasePlayer::OnSpawnEquip(true, true)` (`player.cpp` 10660) as `PlayerSpawn` calls it: a
/// player that was not killed since it last spawned (unless the VIP) keeps what it carries; the
/// others get the default items. (The `game_player_equip` entities that would replace the defaults
/// are not ported.) `lost_items` is `!m_bNotKilled`.
pub(crate) fn on_spawn_equip(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, lost_items: bool) {
    let is_vip = player_ref(g, slot).is_some_and(|p| p.is_vip);
    if !lost_items && !is_vip {
        return;
    }
    if !is_vip && equip_from_map_entities(g, eng, slot) {
        return;
    }
    give_default_items(g, eng, slot);
}

fn equip_from_map_entities(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) -> bool {
    let mut equip_ents = Vec::new();
    let mut cursor = eng.find_by_classname(None, "game_player_equip");
    while let Some(id) = cursor {
        if eng.ent(id).is_some_and(|v| v.spawnflags & SF_PLAYEREQUIP_USEONLY == 0) {
            equip_ents.push(id);
        }
        cursor = eng.find_by_classname(Some(id), "game_player_equip");
    }
    if equip_ents.is_empty() {
        return false;
    }
    remove_all_items(g, eng, slot, false);
    for id in equip_ents {
        crate::entity::use_entity(g, eng, id, Some(eng.client_edict(slot)), Some(id), crate::entity::UseType::Set, 1.0);
    }
    true
}

// ---- selecting ----

/// `SelectItem(name)` for the console's `weapon_*` and `use`: the weapon of classname `name`,
/// when the player has it.
fn select_item_by_name(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, name: &str) {
    let Some(id) = player_ref(g, slot).and_then(|p| item_id_by_name(p, name)) else {
        return;
    };
    select_item(g, eng, slot, id);
}

/// `CBasePlayer::SelectLastItem` (`player.cpp` 6408, vanilla): back to the last weapon, or, with
/// none, to the first weapon of the primary, pistol or knife slot that is not the active one.
fn select_last_item(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = player_ref(g, slot) else {
        return;
    };
    let active = p.active_item;
    if let Some(active) = active
        && !with_weapon(g, eng, slot, active, |b, w| b.can_holster(w)).unwrap_or(true)
    {
        return;
    }
    let Some(p) = player_mut(g, slot) else {
        return;
    };
    if p.last_item.is_none() || p.last_item == p.active_item {
        for i in PRIMARY_WEAPON_SLOT..=KNIFE_SLOT {
            if let Some(first) = p.items[i].first()
                && Some(first.weapon.id) != p.active_item
            {
                p.last_item = Some(first.weapon.id);
                break;
            }
        }
    }
    let Some(last) = p.last_item else {
        return;
    };
    if Some(last) == p.active_item {
        return;
    }
    if let Some(active) = active {
        with_weapon(g, eng, slot, active, |b, w| b.holster(w));
    }
    let Some(p) = player_mut(g, slot) else {
        return;
    };
    if p.owns_shield {
        p.shield_drawn = false;
    }
    std::mem::swap(&mut p.active_item, &mut p.last_item);
    if let Some(now_active) = p.active_item {
        with_weapon(g, eng, slot, now_active, |b, w| b.deploy(w));
    }
    crate::player::reset_max_speed(g, eng, slot);
}

// ---- the commands ----

/// The weapon commands of `ClientCommand_` that belong to a living player (`client.cpp` 3532):
/// `use <name>` and `weapon_<name>` select a weapon, `lastinv` the last one. Only a living
/// player's commands reach this; a name of any length is compared as it is, so nothing here can
/// fail on a client's text. True when the command was one of these.
pub(crate) fn client_command(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) -> bool {
    let Some(command) = argv.first().map(String::as_str) else {
        return false;
    };
    let is_weapon_name = command.starts_with("weapon_");
    if command != "use" && command != "lastinv" && !is_weapon_name {
        return false;
    }
    if !is_alive(eng, slot) {
        // A dead player's commands fall through to the unknown-command message.
        return false;
    }
    match command {
        "use" => {
            if let Some(name) = argv.get(1) {
                select_item_by_name(g, eng, slot, name);
            }
        }
        "lastinv" => select_last_item(g, eng, slot),
        _ => select_item_by_name(g, eng, slot, command),
    }
    true
}

/// `drop [name]` (`client.cpp` 3532): the player drops the named weapon, or the active one with
/// no name. Without a shield (not ported) this is `DropPlayerItem(arg)`.
pub(crate) fn drop_command(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, argv: &[String]) {
    let name = argv.get(1).map_or("", String::as_str);
    let name: String = name.chars().take(MAX_COMMAND_NAME).collect();
    drop_player_item(g, eng, slot, (!name.is_empty()).then_some(name.as_str()));
}

// ---- dropping ----

/// `CBasePlayer::DropPlayerItem` (`player.cpp` 8432, vanilla): the weapon named `name`, or the
/// active one, is dropped as a weapon box 10 units in front of the player, thrown forward at 400
/// units a second. A weapon that cannot be dropped (the grenades say no) is refused with
/// `#Weapon_Cannot_Be_Dropped`. The player is first taken off the weapon (`GetNextBestWeapon`);
/// the weapon leaves the HUD (`pev->weapons`); a dropped primary clears `m_bHasPrimary`. The box
/// lives `ITEM_KILL_DELAY` seconds. Returns the box. The C4's extras are ported (`m_bHasC4`,
/// the bomb icon and bar, `m_bBombDropped`, `#Game_bomb_drop` and `BombDrop` to the living
/// terrorists while the round is not restarting), and the VIP cannot drop (`#Weapon_Cannot_Be_
/// Dropped`). Not ported: `SVC_DIRECTOR`, the `Dropped_The_Bomb` log line, the bots' and the
/// tutor's events.
pub(crate) fn drop_player_item(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    name: Option<&str>,
) -> Option<EdictId> {
    let p = player_ref(g, slot)?;
    if p.is_vip {
        client_print(eng, slot, HUD_PRINTCENTER, "#Weapon_Cannot_Be_Dropped", &[]);
        return None;
    }
    if name == Some("weapon_shield") {
        return p.owns_shield.then(|| drop_shield(g, eng, slot, true)).flatten();
    }
    if name.is_none() && p.owns_shield {
        let active_droppable = p
            .active_item
            .and_then(|id| {
                // `drop` while holding pistol drops pistol; holding non-droppable item (e.g. knife) drops shield.
                (!matches!(
                    id,
                    WeaponId::Knife | WeaponId::HeGrenade | WeaponId::Flashbang | WeaponId::SmokeGrenade
                ))
                .then_some(id)
            })
            .is_some();
        if !active_droppable {
            return drop_shield(g, eng, slot, true);
        }
    }
    let id = match name {
        Some(name) => item_id_by_name(p, name)?,
        None => p.active_item?,
    };
    if !with_weapon(g, eng, slot, id, |b, w| b.can_drop(w)).unwrap_or(false) {
        client_print(eng, slot, HUD_PRINTCENTER, "#Weapon_Cannot_Be_Dropped", &[]);
        return None;
    }
    // Take the item off the HUD.
    if let Some(p) = player_mut(g, slot) {
        p.weapons &= !id.bit();
    }
    get_next_best_weapon(g, eng, slot, id);
    // `UTIL_MakeVectors(pev->angles)`.
    let ent = eng.client_edict(slot);
    let v = eng.ent(ent)?;
    let (origin, angles) = (v.origin, v.angles);
    let (forward, ..) = cs_sim::pm::math::angle_vectors(angles);
    if item_slot(id) == PRIMARY_WEAPON_SLOT
        && let Some(p) = player_mut(g, slot)
    {
        // I may have more than just 1 primary weapon :)
        p.has_primary = false;
    }
    if id == WeaponId::C4 {
        if let Some(p) = player_mut(g, slot) {
            p.has_c4 = false;
        }
        if let Some(v) = eng.ent_mut(ent) {
            v.body = 0;
        }
        crate::buy::set_bomb_icon(g, eng, slot, false);
        crate::player::set_progress_bar_time(g, eng, slot, 0);
        if g.rules.restart_round_time() == 0.0 {
            g.rules.bomb_dropped = true;
            let dropper_name = crate::teams::player_name(eng, slot);
            for other_slot in 0..MAX_PLAYERS {
                let s = other_slot as u8;
                let living_t = player_ref(g, s).is_some_and(|p| p.team == Team::Terrorist)
                    && is_alive(eng, s);
                if living_t {
                    client_print(eng, s, HUD_PRINTCENTER, "#Game_bomb_drop", &[&dropper_name]);
                    eng.send_usermsg(
                        MsgDest::One(s),
                        &CsUserMsg::BombDrop {
                            origin,
                            planted: false,
                        },
                    );
                }
            }
        }
    }
    let model = cs_model_name(id)?;
    create_weapon_box(
        g,
        eng,
        Some(slot),
        id,
        model,
        origin + forward * 10.0,
        angles,
        forward * 300.0 + forward * 100.0,
        ITEM_KILL_DELAY,
        false,
    )
}

/// `CBasePlayer::DropPrimary` (`player.cpp` 10533, vanilla): drops the head of the primary slot
/// or the tactical shield when carried.
pub(crate) fn drop_primary(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if player_ref(g, slot).is_some_and(|p| p.owns_shield) {
        drop_shield(g, eng, slot, true);
        return;
    }
    drop_slot_head(g, eng, slot, PRIMARY_WEAPON_SLOT);
}

/// Gives a tactical shield (`CBasePlayer::GiveShield`, `player.cpp`) to `slot`, updating
/// `has_primary`, `pev->gamestate`, and optionally re-deploying the active weapon so shield
/// view/world models take effect.
pub(crate) fn give_shield(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, deploy: bool) {
    let ent = eng.client_edict(slot);
    let active = {
        let Some(p) = player_mut(g, slot) else {
            return;
        };
        p.owns_shield = true;
        p.has_primary = true;
        p.shield_drawn = false;
        p.rebuy_record.shield = true;
        p.rebuy_record.primary = None;
        p.active_item
    };
    if let Some(v) = eng.ent_mut(ent) {
        v.gamestate = crate::player::HITGROUP_SHIELD_ENABLED;
    }
    eng.emit_sound(ent, CHAN_ITEM, "items/gunpickup2.wav", 1.0, ATTN_NORM, PITCH_NORM);
    if deploy && let Some(active_id) = active {
        with_weapon(g, eng, slot, active_id, |b, w| {
            b.deploy(w);
        });
    }
    crate::player::reset_max_speed(g, eng, slot);
}

/// Clears tactical shield ownership from `slot`, restores `pev->gamestate`, and clears
/// `HIDEHUD_CROSSHAIR` / `WPNSTATE_SHIELD_DRAWN`.
pub(crate) fn remove_shield(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let ent = eng.client_edict(slot);
    let active = {
        let Some(p) = player_mut(g, slot) else {
            return;
        };
        p.owns_shield = false;
        p.shield_drawn = false;
        p.has_primary = !p.items[PRIMARY_WEAPON_SLOT].is_empty();
        p.hide_hud &= !crate::player::HIDEHUD_CROSSHAIR;
        for list in &mut p.items {
            for item in list {
                item.weapon.weapon_state &= !crate::weapons::WPNSTATE_SHIELD_DRAWN;
            }
        }
        p.active_item
    };
    if let Some(v) = eng.ent_mut(ent) {
        v.gamestate = crate::player::HITGROUP_SHIELD_DISABLED;
    }
    if let Some(active_id) = active {
        with_weapon(g, eng, slot, active_id, |b, w| {
            b.deploy(w);
        });
    }
    crate::player::reset_max_speed(g, eng, slot);
}

/// Drops the player's tactical shield into the world as a `weapon_shield` entity.
pub(crate) fn drop_shield(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    slot: u8,
    deploy: bool,
) -> Option<EdictId> {
    if !player_ref(g, slot).is_some_and(|p| p.owns_shield) {
        return None;
    }
    let ent = eng.client_edict(slot);
    let v = eng.ent(ent)?;
    let (origin, angles) = (v.origin, v.angles);
    let (forward, ..) = cs_sim::pm::math::angle_vectors(angles);
    remove_shield(g, eng, slot);
    if !deploy
        && let Some(p) = player_mut(g, slot)
    {
        p.rebuy_record.shield = false;
    }

    let now = eng.time();
    let shield_ent = eng.create_entity("weapon_shield")?;
    attach(
        eng,
        shield_ent,
        Box::new(WorldShield {
            dropper: Some(ent),
            ignore_dropper_until: now + 0.5,
        }),
    );
    let mut angles_flat = angles;
    angles_flat.x = 0.0;
    angles_flat.z = 0.0;
    let drop_origin = origin + forward * 10.0;
    let vel = forward * 400.0;
    if let Some(sv) = eng.ent_mut(shield_ent) {
        sv.origin = drop_origin;
        sv.angles = angles_flat;
        sv.velocity = vel;
        sv.movetype = MOVETYPE_TOSS;
        sv.solid = SOLID_TRIGGER;
        sv.nextthink = now + f64::from(ITEM_KILL_DELAY);
    }
    eng.set_model(shield_ent, "models/w_shield.mdl");
    eng.set_size(
        shield_ent,
        Vec3::new(-16.0, -16.0, 0.0),
        Vec3::new(16.0, 16.0, 16.0),
    );
    eng.set_origin(shield_ent, drop_origin);
    Some(shield_ent)
}

/// `CBasePlayer::DropSecondary` (`player.cpp` 10507, vanilla): drops the head of the pistol slot.
pub(crate) fn drop_secondary(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    drop_slot_head(g, eng, slot, PISTOL_SLOT);
}

fn drop_slot_head(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, slot_index: usize) {
    let Some(id) = player_ref(g, slot).and_then(|p| p.items[slot_index].first().map(|i| i.weapon.id))
    else {
        return;
    };
    let Some(name) = item_info(id).map(|i| i.name) else {
        return;
    };
    drop_player_item(g, eng, slot, Some(name));
}

/// `CreateWeaponBox` (`player.cpp` 1384, vanilla): a `weaponbox` at `origin` with the weapon `id`
/// taken out of the owner's list (`PackWeapon`: the active weapon is put away first). The box
/// is level, moves with `velocity` and removes itself after `life_time` seconds. For a weapon
/// that is exhaustible (grenades) or with `pack_ammo`, the owner's reserve of its ammo goes into
/// the box (and for the exhaustible ones leaves the owner). The box takes the weapon's world
/// model last. `None` when no edict is free.
#[allow(clippy::too_many_arguments)]
pub(crate) fn create_weapon_box(
    g: &mut GameState,
    eng: &mut EngineApi<'_>,
    owner: Option<u8>,
    id: WeaponId,
    model: &str,
    origin: Vec3,
    angles: Vec3,
    velocity: Vec3,
    life_time: f32,
    pack_ammo: bool,
) -> Option<EdictId> {
    let ent = eng.create_entity("weaponbox")?;
    let owner_edict = owner.map(|s| eng.client_edict(s));
    let now = eng.time();
    if let Some(v) = eng.ent_mut(ent) {
        v.owner = owner_edict;
        v.origin = origin;
        v.angles = Vec3::new(0.0, angles.y, 0.0);
        v.movetype = MOVETYPE_TOSS;
        v.solid = SOLID_TRIGGER;
        v.velocity = velocity;
        v.nextthink = now + BOX_THINK_INTERVAL;
    }
    // `CWeaponBox::Spawn`.
    eng.set_size(ent, Vec3::ZERO, Vec3::ZERO);
    eng.set_model(ent, "models/w_weaponbox.mdl");
    let mut weapon_box = WeaponBox::new(now + f64::from(life_time));
    if id == WeaponId::C4 {
        weapon_box.is_bomb = true;
        weapon_box.next_bomb_think = now + 1.0;
        weapon_box.bomb_weapon_origin = owner
            .and_then(|s| eng.ent(eng.client_edict(s)).map(|v| v.origin))
            .unwrap_or(origin);
    }

    // `PackWeapon`.
    let packed = match owner {
        Some(slot) => {
            if player_ref(g, slot).is_some_and(|p| p.active_item == Some(id)) {
                with_weapon(g, eng, slot, id, |b, w| b.holster(w));
            }
            remove_player_item(g, eng, slot, id)
        }
        None => None,
    };
    let Some(mut item) = packed else {
        eng.remove_entity(ent);
        return None;
    };
    // The weapon never respawns, and is nobody's now.
    item.weapon.ent = None;
    let flags = item_info(id).map_or(0, |i| i.flags);
    let ammo1 = item_info(id).and_then(|i| i.ammo1);
    let ammo_type = item.weapon.primary_ammo_type;
    let slot_index = item_slot(id);
    weapon_box.items[slot_index].insert(0, item);

    // The owner is the ammo source.
    if let Some(slot) = owner {
        let exhaustible = flags & ITEM_FLAG_EXHAUSTIBLE == ITEM_FLAG_EXHAUSTIBLE;
        // QUIRK: by removing ammo only on exhaustible weapons (slots 4 and 5) a player can
        // duplicate ammo (the original's own comment).
        if (exhaustible || pack_ammo)
            && let Some(name) = ammo1
            && let Some(p) = player_mut(g, slot)
        {
            let have = usize::try_from(ammo_type).ok().and_then(|i| p.ammo.get(i).copied()).unwrap_or(0);
            weapon_box.pack_ammo(name, have);
            if exhaustible && let Some(a) = usize::try_from(ammo_type).ok().and_then(|i| p.ammo.get_mut(i)) {
                *a = 0;
            }
        }
    }
    eng.set_model(ent, model);
    attach(eng, ent, Box::new(weapon_box));
    Some(ent)
}

/// `PackPlayerItem` (`player.cpp` 1453): a box with the weapon `id` at the player's feet,
/// moving with three quarters of the player's velocity.
fn pack_player_item(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8, id: WeaponId, pack_ammo: bool) {
    let Some(model) = cs_model_name(id) else {
        return;
    };
    let ent = eng.client_edict(slot);
    let Some(v) = eng.ent(ent) else {
        return;
    };
    let (origin, angles, velocity) = (v.origin, v.angles, v.velocity * 0.75);
    create_weapon_box(g, eng, Some(slot), id, model, origin, angles, velocity, ITEM_KILL_DELAY, pack_ammo);
}

/// `CBasePlayer::PackDeadPlayerItems` (`player.cpp` 1529, vanilla): `DeadPlayerWeapons` is
/// `GR_PLR_DROP_GUN_ACTIVE` and `DeadPlayerAmmo` is `GR_PLR_DROP_AMMO_ACTIVE`, so the player's
/// best weapon of the primary and pistol slots (the heaviest; the first of equal weight) goes into
/// a box with the ammo of its type; the grenades are not dropped (`nadedrops` is a
/// `REGAMEDLL_ADD` cvar). Then everything is taken away, the suit too.
pub(crate) fn pack_dead_player_items(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    let Some(p) = player_ref(g, slot) else {
        return;
    };
    let mut best_weight = 0;
    let mut best = None;
    for list in p.items.iter().take(KNIFE_SLOT) {
        for item in list {
            if let Some(info) = item_info(item.weapon.id)
                && info.weight > best_weight
            {
                best_weight = info.weight;
                best = Some(item.weapon.id);
            }
        }
    }
    if let Some(best) = best {
        pack_player_item(g, eng, slot, best, true);
    }
    remove_all_items(g, eng, slot, true);
}

/// `PlayerDeathThink`'s `if (HasWeapons()) PackDeadPlayerItems()`.
pub(crate) fn death_think_pack(g: &mut GameState, eng: &mut EngineApi<'_>, slot: u8) {
    if player_ref(g, slot).is_some_and(has_weapons) {
        pack_dead_player_items(g, eng, slot);
    }
}

// ---- the round ----

/// The weapons part of `CHalfLifeMultiplay::CleanUpMap` (`multiplay_gamerules.cpp` 611, vanilla),
/// run by `RestartRound` after the players have respawned: the `armoury_entity`s start over,
/// the grenades (and the C4 with them) in flight go, and `RemoveGuns` kills every weapon box.
/// (The other entity classes `UTIL_RestartOther` recreates are not ported entities.) It ends
/// with `PLAYBACK_EVENT((FEV_GLOBAL | FEV_RELIABLE), 0, m_usResetDecals)`
/// (`multiplay_gamerules.cpp` 685, read 2026-11-22): `events/decal_reset.sc` tells every client
/// to wipe the decals off the map.
pub(crate) fn clean_up_map(g: &mut GameState, eng: &mut EngineApi<'_>) {
    for id in eng.entity_ids() {
        let Some(class) = eng.ent(id).map(|v| v.classname.clone()) else {
            continue;
        };
        match class.as_str() {
            "grenade" | "weaponbox" | "item_thighpack" => eng.remove_entity(id),
            // Hostages already restarted above in `rules::restart_round`.
            "hostage_entity" | "monster_scientist" => {}
            _ => {
                crate::entity::with_entity(g, eng, id, |entity, g, eng| {
                    entity.restart(g, eng, id);
                });
            }
        }
    }
    // `PLAYBACK_EVENT(flags, ent, idx)` is `PLAYBACK_EVENT_FULL` with a zero delay, origin,
    // angles and parameters, and no invoker. `m_usResetDecals` is `PRECACHE_EVENT(1,
    // "events/decal_reset.sc")` of `ClientPrecache`; asking again gives its index.
    let decal_reset = eng.precache_event("events/decal_reset.sc");
    eng.playback_event(
        FEV_GLOBAL | FEV_RELIABLE,
        None,
        decal_reset,
        0.0,
        Vec3::ZERO,
        Vec3::ZERO,
        0.0,
        0.0,
        0,
        0,
        false,
        false,
    );
}

// ---- weaponbox ----

/// `CWeaponBox`: the weapons and ammo that lie in the world, a dropped weapon or a dead
/// player's. Whoever touches it (a living player standing on the ground) takes what the rules
/// let the player have; what is left stays until the box is gone.
pub(crate) struct WeaponBox {
    /// `m_rgpPlayerItems`: the weapons in the box, by inventory slot.
    items: [Vec<WeaponSlotItem>; MAX_ITEM_TYPES],
    /// `m_rgiszAmmo`: the ammo names, indexed like `m_rgAmmo` (slot 0 is not used).
    ammo_names: [Option<String>; MAX_AMMO_SLOTS],
    /// `m_rgAmmo`: how much of each.
    ammo: [i32; MAX_AMMO_SLOTS],
    /// Server time the box removes itself (`SetThink(Kill)`).
    kill_time: f64,
    /// `m_bIsBomb`: whether this box holds a dropped C4 (`CWeaponBox::BombThink`).
    is_bomb: bool,
    /// Next server time `BombThink` broadcasts `BombDrop` to living terrorists (every 1.0 s).
    next_bomb_think: f64,
    /// Where the packed `weapon_c4` lies: the dropper's origin when it was dropped. The weapon
    /// entity inside a box is not moved with it, and `InitHUD` reads its `pev->origin`.
    bomb_weapon_origin: Vec3,
    /// The touch box has been grown (see [`WeaponBox::think`]).
    widened: bool,
}

impl WeaponBox {
    fn new(kill_time: f64) -> Self {
        Self {
            items: Default::default(),
            ammo_names: std::array::from_fn(|_| None),
            ammo: [0; MAX_AMMO_SLOTS],
            kill_time,
            is_bomb: false,
            next_bomb_think: 0.0,
            bomb_weapon_origin: Vec3::ZERO,
            widened: false,
        }
    }

    /// `CWeaponBox::PackAmmo`: `count` rounds of ammo `name` go into the box, when the ammo is
    /// one a weapon uses and `count` is above 0.
    fn pack_ammo(&mut self, name: &str, count: i32) {
        let max = max_ammo_carry(name);
        if max != -1 && count > 0 {
            self.give_ammo(count, name, max);
        }
    }

    /// `CWeaponBox::GiveAmmo`: adds to the box's own ammo list (compared without case). The
    /// index the ammo has in the box, -1 when it would add nothing.
    fn give_ammo(&mut self, count: i32, name: &str, max: i32) -> i32 {
        let mut i = 1;
        while i < MAX_AMMO_SLOTS && self.ammo_names[i].is_some() {
            if self.ammo_names[i].as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)) {
                let add = count.min(max - self.ammo[i]);
                if count == 0 || add > 0 {
                    self.ammo[i] += add;
                    return i as i32;
                }
                return -1;
            }
            i += 1;
        }
        if i < MAX_AMMO_SLOTS {
            self.ammo_names[i] = Some(name.to_string());
            self.ammo[i] = count;
            return i as i32;
        }
        i as i32
    }
}

impl GameEntity for WeaponBox {
    fn classname(&self) -> &'static str {
        "weaponbox"
    }

    fn is_dropped_bomb(&self) -> bool {
        self.is_bomb
    }

    fn dropped_bomb_origin(&self, _own_origin: Vec3) -> Option<Vec3> {
        self.is_bomb.then_some(self.bomb_weapon_origin)
    }

    /// `CWeaponBox::Kill` at the end of its life (replaced by `CWeaponBox::BombThink` when
    /// `is_bomb` so a dropped C4 never disappears); before landing, checks every tenth of a
    /// second until on the ground, then widens the touch box to 32 by 32 by 16.
    fn think(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let now = eng.time();
        let on_ground = eng.ent(id).is_some_and(|v| v.flags & FL_ONGROUND != 0);
        if on_ground && !self.widened {
            self.widened = true;
            eng.set_size(id, Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, 16.0));
        }
        if self.is_bomb {
            if now >= self.next_bomb_think {
                if let Some(origin) = eng.ent(id).map(|v| v.origin) {
                    for s in 0..MAX_PLAYERS {
                        let slot = s as u8;
                        let living_t = player_ref(g, slot).is_some_and(|p| p.team == Team::Terrorist)
                            && is_alive(eng, slot);
                        if living_t {
                            eng.send_usermsg(
                                MsgDest::One(slot),
                                &CsUserMsg::BombDrop {
                                    origin,
                                    planted: false,
                                },
                            );
                        }
                    }
                }
                self.next_bomb_think = now + 1.0;
            }
            let next = if self.widened {
                self.next_bomb_think
            } else {
                (now + BOX_THINK_INTERVAL).min(self.next_bomb_think)
            };
            if let Some(v) = eng.ent_mut(id) {
                v.nextthink = next;
            }
            return;
        }
        if now >= self.kill_time {
            eng.remove_entity(id);
            return;
        }
        // Looks every tenth of a second until it has landed, then sleeps until its end.
        let next = if self.widened { self.kill_time } else { (now + BOX_THINK_INTERVAL).min(self.kill_time) };
        if let Some(v) = eng.ent_mut(id) {
            v.nextthink = next;
        }
    }

    /// `CWeaponBox::Touch` (`weapons.cpp` 2012, vanilla): a living player touching the box while
    /// it lies on the ground is offered every weapon in it, in slot order.
    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if eng.ent(id).is_none_or(|v| v.flags & FL_ONGROUND == 0) {
            return;
        }
        // Only players may touch a weaponbox, no dead guys.
        let Some(slot) = player_slot(g, eng, other).map(|s| s as u8) else {
            return;
        };
        if !is_alive(eng, slot) {
            return;
        }
        if player_ref(g, slot).is_some_and(|p| p.is_vip || p.shield_drawn) {
            return;
        }
        let mut remove = true;
        let mut given = false;
        for i in 0..MAX_ITEM_TYPES {
            let mut at = 0;
            while at < self.items[i].len() {
                let item_id = self.items[i][at].weapon.id;
                if item_id == WeaponId::C4 {
                    // Vanilla (`!REGAMEDLL_FIXES`): only living terrorists pick up the bomb.
                    let is_living_t =
                        player_ref(g, slot).is_some_and(|p| p.team == Team::Terrorist)
                            && is_alive(eng, slot);
                    if !is_living_t {
                        return;
                    }
                    if let Some(p) = player_ref(g, slot)
                        && p.show_hints
                        && p.display_history & crate::hint::dhf::BOMB_RETRIEVED == 0
                    {
                        crate::hint::first_time(g, slot, crate::hint::dhf::BOMB_RETRIEVED);
                        crate::hint::hint_message(
                            g,
                            eng,
                            slot,
                            "#Hint_you_have_the_bomb",
                            false,
                            false,
                        );
                    } else {
                        client_print(eng, slot, HUD_PRINTCENTER, "#Got_bomb", &[]);
                    }
                    g.rules.bomb_dropped = false;
                    if let Some(p) = player_mut(g, slot) {
                        p.has_c4 = true;
                    }
                    if let Some(v) = eng.ent_mut(other) {
                        v.body = 1;
                    }
                    crate::buy::set_bomb_icon(g, eng, slot, false);
                    let picker_name = crate::teams::player_name(eng, slot);
                    for s in 0..MAX_PLAYERS {
                        let other_slot = s as u8;
                        let other_t = player_ref(g, other_slot)
                            .is_some_and(|p| p.team == Team::Terrorist)
                            && is_alive(eng, other_slot);
                        if other_t {
                            if other_slot != slot {
                                client_print(
                                    eng,
                                    other_slot,
                                    HUD_PRINTCENTER,
                                    "#Game_bomb_pickup",
                                    &[&picker_name],
                                );
                            }
                            eng.send_usermsg(MsgDest::One(other_slot), &CsUserMsg::BombPickup);
                        }
                    }
                }
                let holds_one = player_ref(g, slot).is_some_and(|p| !p.items[i].is_empty());
                if (PRIMARY_WEAPON_SLOT..=PISTOL_SLOT).contains(&i) && holds_one {
                    // The player has a weapon of this slot: leave it in the box.
                } else if i == GRENADE_SLOT {
                    let (max, name) = match item_id {
                        WeaponId::HeGrenade => (1, "weapon_hegrenade"),
                        WeaponId::SmokeGrenade => (1, "weapon_smokegrenade"),
                        WeaponId::Flashbang => (2, "weapon_flashbang"),
                        _ => (0, ""),
                    };
                    let held = usize::try_from(self.items[i][at].weapon.primary_ammo_type)
                        .ok()
                        .and_then(|a| player_ref(g, slot).and_then(|p| p.ammo.get(a).copied()))
                        .unwrap_or(0);
                    if held < max && !name.is_empty() {
                        given = true;
                        give_named_item(g, eng, slot, name);
                        // Unlink this weapon from the box.
                        self.items[i].remove(at);
                        continue;
                    }
                } else {
                    let item = self.items[i].remove(at);
                    if add_player_item(g, eng, slot, item) {
                        given = true;
                    }
                    continue;
                }
                remove = false;
                at += 1;
            }
        }
        if remove {
            // Dole out the ammo.
            for n in 0..MAX_AMMO_SLOTS {
                let Some(name) = self.ammo_names[n].take() else {
                    continue;
                };
                if self.ammo[n] > 0 {
                    give_ammo(g, eng, slot, self.ammo[n], Some(&name), max_ammo_carry(&name));
                }
                self.ammo[n] = 0;
            }
        }
        if given {
            eng.emit_sound(other, CHAN_ITEM, "items/gunpickup2.wav", 1.0, ATTN_NORM, PITCH_NORM);
        }
        if remove {
            eng.remove_entity(id);
        }
    }
}

// ---- the armoury ----

/// `m_ItemModels` (`weapons.cpp` 2477, vanilla): the model of each `ArmouryItemPack` up to the
/// smoke grenade.
const ARMOURY_MODELS: [&str; 19] = [
    "models/w_mp5.mdl",
    "models/w_tmp.mdl",
    "models/w_p90.mdl",
    "models/w_mac10.mdl",
    "models/w_ak47.mdl",
    "models/w_sg552.mdl",
    "models/w_m4a1.mdl",
    "models/w_aug.mdl",
    "models/w_scout.mdl",
    "models/w_g3sg1.mdl",
    "models/w_awp.mdl",
    "models/w_m3.mdl",
    "models/w_xm1014.mdl",
    "models/w_m249.mdl",
    "models/w_flashbang.mdl",
    "models/w_hegrenade.mdl",
    "models/w_kevlar.mdl",
    "models/w_assault.mdl",
    "models/w_smokegrenade.mdl",
];

/// `ArmouryItemPack` values the vanilla touch code handles by name.
const ARMOURY_M249: i32 = 13;
const ARMOURY_FLASHBANG: i32 = 14;
const ARMOURY_HEGRENADE: i32 = 15;
const ARMOURY_KEVLAR: i32 = 16;
const ARMOURY_ASSAULT: i32 = 17;
const ARMOURY_SMOKEGRENADE: i32 = 18;

/// `armouryItemInfo` for the guns of the pack (`weapons.cpp` 2657): the class, the ammo name,
/// the ammo given with the gun and its limit.
const ARMOURY_GUNS: [(&str, &str, i32, i32); 14] = [
    ("weapon_mp5navy", "9mm", 60, 120),
    ("weapon_tmp", "9mm", 60, 120),
    ("weapon_p90", "57mm", 50, 100),
    ("weapon_mac10", "45acp", 60, 100),
    ("weapon_ak47", "762Nato", 60, 90),
    ("weapon_sg552", "556Nato", 60, 90),
    ("weapon_m4a1", "556Nato", 60, 90),
    ("weapon_aug", "556Nato", 60, 90),
    ("weapon_scout", "762Nato", 30, 90),
    ("weapon_g3sg1", "762Nato", 30, 90),
    ("weapon_awp", "338Magnum", 20, 30),
    ("weapon_m3", "buckshot", 24, 32),
    ("weapon_xm1014", "buckshot", 24, 32),
    ("weapon_m249", "556NatoBox", 60, 200),
];

/// `armoury_entity` (`CArmoury`, `weapons.cpp` 2514): a map's weapon rack. A player touching it
/// takes the item (a gun with its ammo when it has no primary weapon, a grenade it has room for,
/// a vest or a suit it lacks), and the rack counts down; at 0 it is hidden until the round
/// restarts.
#[derive(Default)]
pub(crate) struct Armoury {
    /// `m_iItem`.
    item: i32,
    /// `m_iCount`.
    count: i32,
    /// `m_iInitialCount`.
    initial_count: i32,
    /// `m_bAlreadyCounted`: the rack is in the rules' totals.
    already_counted: bool,
}

impl GameEntity for Armoury {
    fn classname(&self) -> &'static str {
        "armoury_entity"
    }

    /// `CArmoury::KeyValue`, `Precache` and `Spawn`: the keys `item` and `count` (read as `atoi`),
    /// the item's model, a trigger that falls to the floor with a 32 by 32 by 16 box.
    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, keys: &[(String, String)]) -> bool {
        for (key, value) in keys {
            match key.as_str() {
                "item" => self.item = crate::menus::atoi(value),
                "count" => self.count = crate::menus::atoi(value),
                _ => {}
            }
        }
        let model = usize::try_from(self.item).ok().and_then(|i| ARMOURY_MODELS.get(i).copied());
        if let Some(model) = model {
            precache_model(eng, model);
        }
        if let Some(v) = eng.ent_mut(id) {
            v.movetype = MOVETYPE_TOSS;
            v.solid = SOLID_TRIGGER;
        }
        eng.set_size(id, Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, 16.0));
        let origin = eng.ent(id).map_or(Vec3::ZERO, |v| v.origin);
        eng.set_origin(id, origin);
        if let Some(model) = model {
            eng.set_model(id, model);
        }
        if self.count <= 0 {
            self.count = 1;
        }
        self.initial_count = self.count;
        true
    }

    /// `CArmoury::ArmouryTouch` (vanilla: no pistols or `FAMAS`-and-later items; those need
    /// `REGAMEDLL_ADD`).
    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        let Some(slot) = player_slot(g, eng, other).map(|s| s as u8) else {
            return;
        };
        if self.count > 0 && (0..=ARMOURY_M249).contains(&self.item) {
            // Primary weapons.
            if player_ref(g, slot).is_none_or(|p| p.has_primary) {
                return;
            }
            self.count -= 1;
            let (class, ammo, amount, max) = ARMOURY_GUNS[self.item as usize];
            give_named_item(g, eng, slot, class);
            give_ammo(g, eng, slot, amount, Some(ammo), max);
        } else if self.count > 0 && self.item >= ARMOURY_FLASHBANG {
            // Items and grenades.
            let Some(p) = player_ref(g, slot) else {
                return;
            };
            let held = |ammo: &str| {
                usize::try_from(ammo_index(ammo)).ok().and_then(|i| p.ammo.get(i).copied()).unwrap_or(0)
            };
            let (grenade, class): (Option<(&str, WeaponId)>, &str) = match self.item {
                ARMOURY_FLASHBANG => (Some(("Flashbang", WeaponId::Flashbang)), "weapon_flashbang"),
                ARMOURY_HEGRENADE => (Some(("HEGrenade", WeaponId::HeGrenade)), "weapon_hegrenade"),
                ARMOURY_SMOKEGRENADE => {
                    (Some(("SmokeGrenade", WeaponId::SmokeGrenade)), "weapon_smokegrenade")
                }
                ARMOURY_KEVLAR => (None, "item_kevlar"),
                ARMOURY_ASSAULT => (None, "item_assaultsuit"),
                _ => return,
            };
            if let Some((ammo, weapon)) = grenade {
                if held(ammo) >= crate::weapons::max_ammo_carry_for_weapon(weapon) {
                    return;
                }
                give_named_item(g, eng, slot, class);
            } else if self.item == ARMOURY_KEVLAR {
                // Vanilla: only a player with a plain vest is turned away.
                if p.kevlar == crate::player::ArmorType::Vest {
                    return;
                }
                crate::buy::give_armor_item(g, eng, slot, crate::buy::ArmorItem::Kevlar);
            } else {
                if p.kevlar == crate::player::ArmorType::VestHelm {
                    return;
                }
                crate::buy::give_armor_item(g, eng, slot, crate::buy::ArmorItem::AssaultSuit);
            }
            self.count -= 1;
        }
        if self.count == 0 {
            // `Hide()`.
            if let Some(v) = eng.ent_mut(id) {
                v.effects |= EF_NODRAW;
            }
        }
    }

    /// `CArmoury::Restart` (`weapons.cpp` 2545, vanilla: the `REGAMEDLL_FIXES` escape-map test
    /// around the block is absent, so it always runs). The first restart counts the rack into the
    /// totals of its kind (guns, grenades, armor; zeroed only when the rules are made) and fills
    /// it with its own count. Every later one sets the count from the share the rack has in the
    /// total, times the terrorists in the game: `int((initial / total) * terrorists * k)` with
    /// the division of the two ints done as ints (so 0 or 1), `k` 0.85 for guns, 1.75 for
    /// flashbangs and grenades, 1 for armor; at least 1. Then `Draw`.
    fn restart(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let terrorists = g.rules.num_terrorist as f32;
        let (total, factor) = match self.item {
            ARMOURY_FLASHBANG | ARMOURY_HEGRENADE => (&mut g.rules.total_grenade_count, 1.75_f64),
            ARMOURY_KEVLAR | ARMOURY_ASSAULT => (&mut g.rules.total_armour_count, 1.0),
            _ => (&mut g.rules.total_gun_count, 0.85),
        };
        if !self.already_counted {
            self.already_counted = true;
            *total += self.initial_count;
            self.count = self.initial_count;
        } else {
            let share = (self.initial_count / (*total).max(1)) as f32;
            self.count = (f64::from(share * terrorists) * factor) as f32 as i32;
            if self.count < 1 {
                self.count = 1;
            }
        }
        if let Some(v) = eng.ent_mut(id) {
            v.effects &= !EF_NODRAW;
        }
    }
}

/// `SF_NORESPAWN` (`cbase.h`): dropped items do not respawn.
const SF_NORESPAWN: u32 = 1 << 30;

/// Origin of the dropped C4 (`CWeaponBox::m_bIsBomb` or world `weapon_c4`), for `InitHUD`.
pub(crate) fn dropped_bomb_origin(eng: &EngineApi<'_>) -> Option<Vec3> {
    eng.entity_ids().into_iter().find_map(|id| {
        let ent = eng.private::<Box<dyn GameEntity>>(id)?;
        let own = eng.ent(id)?.origin;
        ent.dropped_bomb_origin(own)
    })
}

/// Spawns a dropped defusal kit (`item_thighpack`) at `origin`, matching vanilla `Killed`'s
/// `GiveNamedItem("item_thighpack")` on a dead CT.
pub(crate) fn spawn_thighpack(eng: &mut EngineApi<'_>, origin: Vec3) -> Option<EdictId> {
    let id = eng.create_entity("item_thighpack")?;
    if let Some(v) = eng.ent_mut(id) {
        v.origin = origin;
        v.spawnflags |= SF_NORESPAWN;
    }
    let mut item = ItemThighPack;
    if !item.spawn(eng, id, &[]) {
        eng.remove_entity(id);
        return None;
    }
    attach(eng, id, Box::new(item));
    Some(id)
}

/// `CItemThighPack` (`item_thighpack`, `items.cpp` 462-497): a dropped defusal kit. Living CTs
/// without a defuser pick it up on touch, getting `has_defuser`, `#Got_defuser`, and
/// `items/kevlar.wav` on `CHAN_VOICE`.
pub(crate) struct ItemThighPack;

impl GameEntity for ItemThighPack {
    fn classname(&self) -> &'static str {
        "item_thighpack"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        precache_model(eng, "models/w_thighpack.mdl");
        if let Some(v) = eng.ent_mut(id) {
            v.movetype = MOVETYPE_TOSS;
            v.solid = SOLID_TRIGGER;
        }
        eng.set_model(id, "models/w_thighpack.mdl");
        let origin = eng.ent(id).map_or(Vec3::ZERO, |v| v.origin);
        eng.set_origin(id, origin);
        eng.set_size(id, Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, 16.0));
        // `CItem::Spawn` (`items.cpp` 102-106, read 2026-11-23): `if (!DROP_TO_FLOOR(ENT(pev)))
        // UTIL_Remove(this)`. Only a 0 (no floor within 256 units) removes the kit; -1 (it starts
        // stuck) does not. Deviation: the stub map (no geometry, only the tests') has no floor to
        // land on, so there the kit is kept.
        !(eng.drop_to_floor(id) == 0 && eng.has_world_collision())
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        let Some(slot) = player_slot(g, eng, other).map(|s| s as u8) else {
            return;
        };
        if !is_alive(eng, slot) {
            return;
        }
        if !player_ref(g, slot).is_some_and(|p| p.team == Team::Ct && !p.has_defuser) {
            return;
        }
        crate::player::give_defuser(g, eng, slot);
        client_print(eng, slot, HUD_PRINTCENTER, "#Got_defuser", &[]);
        eng.emit_sound(other, CHAN_VOICE, "items/kevlar.wav", 1.0, ATTN_NORM, PITCH_NORM);
        crate::targets::use_targets(g, eng, id, Some(other), crate::entity::UseType::Toggle, 0.0);
        eng.remove_entity(id);
    }
}

// ---- a weapon placed by a map ----

/// A `weapon_*` entity of a map (`CBasePlayerItem::FallInit`, `FallThink`, `Materialize` and
/// `DefaultTouch`): a weapon that falls to the floor, becomes a trigger once it lies there, and
/// goes to the first living player to touch it. QUIRK of vanilla: in multiplayer
/// `Materialize` makes it remove itself one second after it lands.
pub(crate) struct WorldWeapon {
    class: &'static str,
    id: WeaponId,
    item: Option<WeaponSlotItem>,
    /// The touch function is set (`Materialize` clears it for a weapon that cannot be dropped).
    touchable: bool,
}

impl WorldWeapon {
    /// The map entity of weapon class `class`, `None` for a class that is no weapon.
    pub(crate) fn of_class(class: &str) -> Option<Self> {
        let info = item_infos().iter().find(|i| i.name == class)?;
        new_weapon(info.id)?;
        Some(Self { class: info.name, id: info.id, item: None, touchable: true })
    }
}

impl GameEntity for WorldWeapon {
    fn classname(&self) -> &'static str {
        self.class
    }

    fn is_dropped_bomb(&self) -> bool {
        self.id == WeaponId::C4
    }

    /// `FallInit`: the weapon is a point that falls (`SOLID_BBOX`, `MOVETYPE_TOSS`) and looks at
    /// itself every tenth of a second. (The weapon's own `Spawn` and `Precache` are the
    /// weapons' ports.)
    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        self.item = new_item(eng, self.id);
        let model = cs_model_name(self.id);
        if let Some(model) = model {
            precache_model(eng, model);
        }
        let now = eng.time();
        if let Some(v) = eng.ent_mut(id) {
            v.movetype = MOVETYPE_TOSS;
            v.solid = SOLID_BBOX;
            v.nextthink = now + 0.1;
        }
        if let Some(model) = model {
            eng.set_model(id, model);
        }
        eng.set_size(id, Vec3::ZERO, Vec3::ZERO);
        let origin = eng.ent(id).map_or(Vec3::ZERO, |v| v.origin);
        eng.set_origin(id, origin);
        true
    }

    /// `FallThink` and, once the weapon is on the ground, `Materialize`: it lies flat, becomes a
    /// trigger with the pickup box (48 by 48 by 16), and (multiplayer) is gone a second later.
    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        let now = eng.time();
        let Some(v) = eng.ent(id) else {
            return;
        };
        if v.solid == SOLID_TRIGGER {
            // `SUB_Remove`, a second after the weapon landed.
            eng.remove_entity(id);
            return;
        }
        if v.flags & FL_ONGROUND == 0 {
            if let Some(v) = eng.ent_mut(id) {
                v.nextthink = now + 0.1;
            }
            return;
        }
        if let Some(v) = eng.ent_mut(id) {
            v.angles.x = 0.0;
            v.angles.z = 0.0;
            v.effects &= !EF_NODRAW;
            v.solid = SOLID_TRIGGER;
            v.nextthink = now + 1.0;
        }
        // A weapon that cannot be dropped (the knife and the three grenades: `CKnife`,
        // `CFlashbang`, `CHEGrenade`, `CSmokeGrenade` override `CanDrop` with FALSE,
        // rg_weapons.h:982-1679, read 2026-11-22) cannot be picked up off the floor either. `CC4`
        // has no override, so a `weapon_c4` in the world is touchable.
        self.touchable = !matches!(
            self.id,
            WeaponId::Knife | WeaponId::HeGrenade | WeaponId::Flashbang | WeaponId::SmokeGrenade
        );
        eng.set_size(id, Vec3::new(-24.0, -24.0, 0.0), Vec3::new(24.0, 24.0, 16.0));
    }

    /// `DefaultTouch`: a living player takes the weapon when the rules let it; the entity goes.
    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        let Some(slot) = player_slot(g, eng, other).map(|s| s as u8) else {
            return;
        };
        if eng.ent(id).is_none_or(|v| v.solid != SOLID_TRIGGER) {
            return;
        }
        // `Materialize`: `if (!CanDrop()) SetTouch(nullptr)`.
        if !self.touchable {
            return;
        }
        let Some(item) = self.item.take() else {
            return;
        };
        let weapon = item.weapon.id;
        let mut have_ctx = cs_mod_api::CanHavePlayerItemCtx {
            slot,
            weapon_id: weapon as i32 as u8,
        };
        if crate::mod_hooks::fire_hook(g, eng, |sink, g, eng| {
            sink.on_can_have_player_item(g, eng, cs_mod_api::HookPhase::Pre, &mut have_ctx)
        }) == cs_mod_api::HookAction::Supercede
        {
            self.item = Some(item);
            return;
        }
        let allowed = player_ref(g, slot).is_some_and(|p| can_have_player_item(eng, p, slot, weapon));
        if !allowed {
            self.item = Some(item);
            return;
        }
        match try_add_player_item(g, eng, slot, item) {
            AddResult::Added => {
                eng.emit_sound(other, CHAN_ITEM, "items/gunpickup2.wav", 1.0, ATTN_NORM, PITCH_NORM);
            }
            // The player took the ammo of the weapon: `Kill()`.
            AddResult::Taken => {}
            // `AddDuplicate` took nothing: the weapon stays where it lies.
            AddResult::Refused(item) => {
                self.item = Some(*item);
                return;
            }
        }
        // `AttachToPlayer`/`Kill`: the entity has served its purpose.
        eng.remove_entity(id);
    }
}

/// World tactical shield (`weapon_shield`, `CWShield`).
#[derive(Default)]
pub(crate) struct WorldShield {
    dropper: Option<EdictId>,
    ignore_dropper_until: f64,
}

impl GameEntity for WorldShield {
    fn classname(&self) -> &'static str {
        "weapon_shield"
    }

    fn spawn(&mut self, eng: &mut EngineApi<'_>, id: EdictId, _keys: &[(String, String)]) -> bool {
        precache_model(eng, "models/w_shield.mdl");
        if let Some(v) = eng.ent_mut(id) {
            v.movetype = MOVETYPE_TOSS;
            v.solid = SOLID_TRIGGER;
        }
        eng.set_model(id, "models/w_shield.mdl");
        eng.set_size(id, Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, 16.0));
        let origin = eng.ent(id).map_or(Vec3::ZERO, |v| v.origin);
        eng.set_origin(id, origin);
        true
    }

    fn think(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        eng.remove_entity(id);
    }

    fn restart(&mut self, _g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId) {
        eng.remove_entity(id);
    }

    fn touch(&mut self, g: &mut GameState, eng: &mut EngineApi<'_>, id: EdictId, other: EdictId) {
        if self.dropper == Some(other) && eng.time() < self.ignore_dropper_until {
            return;
        }
        let Some(slot) = player_slot(g, eng, other).map(|s| s as u8) else {
            return;
        };
        if !is_alive(eng, slot) {
            return;
        }
        let can_take = player_ref(g, slot).is_some_and(|p| {
            !p.is_vip
                && !p.has_primary
                && !p.owns_shield
                && !has_player_item(p, WeaponId::Elite)
        });
        if !can_take {
            return;
        }
        give_shield(g, eng, slot, true);
        eng.remove_entity(id);
    }
}

const SF_PLAYEREQUIP_USEONLY: u32 = 1;

/// `game_player_equip` (`CGamePlayerEquip`): grants configured weapons/armor/items on spawn or use.
#[derive(Default)]
pub(crate) struct GamePlayerEquip {
    items: Vec<(String, i32)>,
}

impl GameEntity for GamePlayerEquip {
    fn classname(&self) -> &'static str {
        "game_player_equip"
    }

    fn spawn(&mut self, _eng: &mut EngineApi<'_>, _id: EdictId, keys: &[(String, String)]) -> bool {
        self.items.clear();
        for (k, v) in keys {
            if matches!(
                k.as_str(),
                "classname" | "origin" | "angles" | "targetname" | "target" | "spawnflags" | "model"
            ) {
                continue;
            }
            let count = crate::menus::atoi(v).clamp(1, 16);
            self.items.push((k.clone(), count));
        }
        true
    }

    fn use_(
        &mut self,
        g: &mut GameState,
        eng: &mut EngineApi<'_>,
        _id: EdictId,
        activator: Option<EdictId>,
        _caller: Option<EdictId>,
        _use_type: crate::entity::UseType,
        _value: f32,
    ) {
        let Some(act) = activator else {
            return;
        };
        let Some(slot) = player_slot(g, eng, act).map(|s| s as u8) else {
            return;
        };
        if !is_alive(eng, slot) {
            return;
        }
        for (class, count) in &self.items {
            for _ in 0..*count {
                match class.as_str() {
                    "weapon_shield" => give_shield(g, eng, slot, true),
                    "item_kevlar" => crate::buy::give_armor_item(g, eng, slot, crate::buy::ArmorItem::Kevlar),
                    "item_assaultsuit" => crate::buy::give_armor_item(g, eng, slot, crate::buy::ArmorItem::AssaultSuit),
                    "item_thighpack" => crate::player::give_defuser(g, eng, slot),
                    other => {
                        give_named_item(g, eng, slot, other);
                    }
                }
            }
        }
    }
}
